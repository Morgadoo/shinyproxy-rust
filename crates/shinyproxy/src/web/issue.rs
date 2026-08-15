/*
 * ShinyProxy
 *
 * Copyright (C) 2016-2026 Open Analytics
 *
 * ===========================================================================
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the Apache License as published by
 * The Apache Software Foundation, either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * Apache License for more details.
 *
 * You should have received a copy of the Apache License
 * along with this program.  If not, see <http://www.apache.org/licenses/>
 */

//! Issue reporting (`IssueController`).
//!
//! Users can report a problem from the UI; ShinyProxy mails the report to `proxy.support.mail-to-address`
//! (or to the address of the app). The mail body has the same layout as the Java implementation, so that
//! existing support workflows keep working.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use containerproxy::auth::AuthenticatedUser;
use containerproxy::model::proxy::Proxy;
use containerproxy::spec::SpecProvider;
use serde::Deserialize;
use serde_json::json;

use super::apps::is_owner;
use super::router::CurrentUser;
use super::state::AppState;
use crate::spec_provider::ShinyProxySpecProvider;

/// Default sender address (`proxy.support.mail-from-address`).
pub const DEFAULT_FROM_ADDRESS: &str = "issues@shinyproxy.io";
/// Default subject (`proxy.support.mail-subject`).
pub const DEFAULT_SUBJECT: &str = "ShinyProxy Error Report";

/// Body of `POST /issue`.
#[derive(Debug, Default, Deserialize)]
pub struct ReportIssueBody {
    /// The message the user typed.
    #[serde(default)]
    pub message: Option<String>,
    /// The page the user was on.
    #[serde(default, rename = "currentLocation")]
    pub current_location: Option<String>,
    /// The app the user was using, when there was one.
    #[serde(default, rename = "proxyId")]
    pub proxy_id: Option<String>,
}

/// The report that is sent by mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueReport {
    /// Recipient.
    pub to: String,
    /// Sender.
    pub from: String,
    /// Subject.
    pub subject: String,
    /// Body.
    pub body: String,
}

/// `POST /issue` — reports an issue.
pub async fn report_issue(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    body: Option<Json<ReportIssueBody>>,
) -> Response {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let Some(user) = user else {
        return fail("Report issue is not configured");
    };

    let support_address = state.settings.proxy.support.mail_to_address.clone();
    let mail_configured = state.settings.spring.mail.host.is_some();
    if support_address.is_none() || !mail_configured {
        return fail("Report issue is not configured");
    }

    let Some(message) = body
        .message
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return fail("Cannot report issue: no message provided");
    };
    let Some(location) = body
        .current_location
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return fail("Cannot report issue: no currentLocation provided");
    };

    // when the report is about a running app, the app decides the recipient and subject
    let mut proxy = None;
    if let Some(proxy_id) = body.proxy_id.as_deref().filter(|id| !id.trim().is_empty()) {
        match state
            .proxies
            .proxy(proxy_id)
            .filter(|proxy| is_owner(&state, Some(&user), proxy))
        {
            Some(found) => proxy = Some(found),
            None => return forbidden(),
        }
    }

    let report = build_report(&state, &user, proxy.as_ref(), &message, &location);

    match send_report(&state, &report).await {
        Ok(()) => {
            match &proxy {
                Some(proxy) => tracing::info!(
                    "User reported an issue, location: {location} [proxyId: {}]",
                    proxy.id
                ),
                None => tracing::info!(
                    "[user={}] User reported an issue, location: {location}",
                    user.id
                ),
            }
            success()
        }
        Err(error) => {
            tracing::error!("Error while sending issue report: {error}");
            fail("Error while sending e-mail")
        }
    }
}

/// Builds the mail of a report, with the same body layout as the Java implementation.
pub fn build_report(
    state: &AppState,
    user: &AuthenticatedUser,
    proxy: Option<&Proxy>,
    message: &str,
    location: &str,
) -> IssueReport {
    let support = &state.settings.proxy.support;
    let mut to = support.mail_to_address.clone().unwrap_or_default();
    let mut subject = support
        .mail_subject
        .clone()
        .unwrap_or_else(|| DEFAULT_SUBJECT.to_string());

    if let Some(proxy) = proxy {
        if let Some(spec) = proxy
            .spec_id
            .as_deref()
            .and_then(|spec_id| state.specs.spec(spec_id))
        {
            let extension = ShinyProxySpecProvider::extension(spec);
            if let Some(address) = extension.support_mail_to_address {
                to = address;
            }
            if let Some(app_subject) = extension.support_mail_subject {
                subject = app_subject;
            }
        }
    }

    let mut body = String::new();
    body.push_str("This is an error report generated by ShinyProxy\n");
    body.push_str(&format!("User: {}\n", user.id));
    body.push_str(&format!("Location: {location}\n"));
    body.push_str(&format!("Message: {message}\n"));
    if let Some(proxy) = proxy {
        body.push_str(&format!("AppId: {}\n", proxy.id));
        body.push_str(&format!(
            "App: {}\n",
            proxy.spec_id.clone().unwrap_or_default()
        ));
        let instance = proxy
            .runtime_value(&crate::runtime_values::APP_INSTANCE)
            .unwrap_or_else(|| crate::runtime_values::DEFAULT_INSTANCE.to_string());
        body.push_str(&format!(
            "Instance name: {}\n",
            crate::runtime_values::instance_display_name(&instance)
        ));
        // the log files of the app; the Java implementation attaches them when they exist and
        // mentions their paths otherwise (attaching them to the mail lands with the S3 log storage)
        if let Some(paths) = state.logs.log_paths(proxy) {
            body.push_str(&format!("Log (stdout): {}\n", paths.stdout.display()));
            body.push_str(&format!("Log (stderr): {}\n", paths.stderr.display()));
        }
    }

    IssueReport {
        to,
        from: support
            .mail_from_address
            .clone()
            .unwrap_or_else(|| DEFAULT_FROM_ADDRESS.to_string()),
        subject,
        body,
    }
}

/// Sends the report over SMTP (`spring.mail.*`).
async fn send_report(state: &AppState, report: &IssueReport) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let mail = &state.settings.spring.mail;
    let host = mail
        .host
        .clone()
        .ok_or("spring.mail.host is not configured")?;
    let port = mail.port.map(|value| value.0).unwrap_or(25) as u16;

    let message = Message::builder()
        .from(
            report
                .from
                .parse()
                .map_err(|error| format!("invalid from address: {error}"))?,
        )
        .to(report
            .to
            .parse()
            .map_err(|error| format!("invalid to address: {error}"))?)
        .subject(report.subject.clone())
        .header(ContentType::TEXT_PLAIN)
        .body(report.body.clone())
        .map_err(|error| error.to_string())?;

    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).port(port);
    if let (Some(username), Some(password)) = (mail.username.clone(), mail.password.clone()) {
        builder = builder.credentials(Credentials::new(username, password));
    }
    let transport = builder.build();
    transport
        .send(message)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn success() -> Response {
    (
        StatusCode::OK,
        Json(json!({"status": "success", "data": null})),
    )
        .into_response()
}

fn fail(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"status": "fail", "data": message})),
    )
        .into_response()
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"status": "fail", "data": "forbidden"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use containerproxy::config::LoadOptions;
    use containerproxy::model::proxy::{Proxy, ProxyStatus};
    use containerproxy::model::runtime_value::RuntimeValue;

    fn build_state(yaml: &str) -> AppState {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("application.yml");
        std::fs::write(&path, yaml).expect("write");
        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (raw, mut settings) = crate::load_config(options).expect("config");
        settings.proxy.container_backend = Some("local".to_string());
        AppState::new(raw, settings).expect("state")
    }

    #[test]
    fn builds_the_java_mail_body() {
        let state = build_state(
            "proxy:\n  authentication: none\n  support:\n    mail-to-address: support@example.com\n  container-log-path: /var/log/sp\n  specs:\n    - id: 01_hello\n      container-image: sp-testapp\n",
        );
        let user = AuthenticatedUser::new("jack", vec![]);
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".into());
        proxy.add_runtime_value(
            RuntimeValue::string(&crate::runtime_values::APP_INSTANCE, "_"),
            false,
        );

        let report = build_report(&state, &user, Some(&proxy), "it broke", "/app/01_hello");
        assert_eq!(report.to, "support@example.com");
        assert_eq!(report.from, "issues@shinyproxy.io");
        assert_eq!(report.subject, "ShinyProxy Error Report");
        let paths = state.logs.log_paths(&proxy).expect("log paths");
        assert_eq!(
            report.body,
            format!(
                "This is an error report generated by ShinyProxy\n\
                 User: jack\n\
                 Location: /app/01_hello\n\
                 Message: it broke\n\
                 AppId: proxy-1\n\
                 App: 01_hello\n\
                 Instance name: Default\n\
                 Log (stdout): {}\n\
                 Log (stderr): {}\n",
                paths.stdout.display(),
                paths.stderr.display()
            )
        );
        // the file names follow the Java layout: {specId}_{proxyId}_{timestamp}_stdout.log
        let name = paths
            .stdout
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_string();
        assert!(name.starts_with("01_hello_proxy-1_"), "{name}");
    }

    #[test]
    fn apps_can_override_recipient_and_subject() {
        let state = build_state(
            "proxy:\n  authentication: none\n  support:\n    mail-to-address: support@example.com\n    mail-from-address: sp@example.com\n    mail-subject: 'Global subject'\n  specs:\n    - id: 01_hello\n      container-image: sp-testapp\n      support-mail-to-address: app-support@example.com\n      support-mail-subject: 'App subject'\n",
        );
        let user = AuthenticatedUser::new("jack", vec![]);
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".into());

        let report = build_report(&state, &user, Some(&proxy), "message", "/app/01_hello");
        assert_eq!(report.to, "app-support@example.com");
        assert_eq!(report.subject, "App subject");
        assert_eq!(report.from, "sp@example.com");

        // without an app the global values are used
        let report = build_report(&state, &user, None, "message", "/");
        assert_eq!(report.to, "support@example.com");
        assert_eq!(report.subject, "Global subject");
        assert!(!report.body.contains("AppId"), "{}", report.body);
    }
}
