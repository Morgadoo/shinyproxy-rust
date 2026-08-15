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

//! Authentication through a web service (`WebServiceAuthenticationBackend`).
//!
//! The credentials of the login form are sent to `proxy.webservice.authentication-url` as the body of
//! `proxy.webservice.authentication-request-body` (a format string with the user name and the password,
//! `%s` twice, as in Java). A `200` means the credentials are valid; `4xx` means they are not. The JSON of
//! the answer is available to expressions as `json`, and `proxy.webservice.groups-expression` extracts the
//! groups from it.

use super::{normalise_group, AuthBackend, AuthError, AuthKind, AuthenticatedUser, LoginForm};
use crate::config::Settings;

/// Name of the backend.
pub const NAME: &str = "webservice";

/// Authenticates users against a web service.
#[derive(Debug, Clone)]
pub struct WebServiceAuthenticationBackend {
    url: String,
    request_body_template: String,
    groups_expression: Option<String>,
}

impl WebServiceAuthenticationBackend {
    /// Creates the backend from the configuration, with the startup errors of the Java implementation.
    pub fn new(settings: &Settings) -> Result<Self, String> {
        let webservice = &settings.proxy.webservice;
        let request_body_template = webservice
            .authentication_request_body
            .clone()
            .filter(|body| !body.trim().is_empty())
            .ok_or_else(|| {
                "Webservice authentication enabled, but no \
                 'proxy.webservice.authentication-request-body' defined!"
                    .to_string()
            })?;
        let url = webservice
            .authentication_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| {
                "Webservice authentication enabled, but no 'proxy.webservice.authentication-url' \
                 defined!"
                    .to_string()
            })?;

        Ok(WebServiceAuthenticationBackend {
            url,
            request_body_template,
            groups_expression: webservice
                .groups_expression
                .clone()
                .filter(|expression| !expression.trim().is_empty()),
        })
    }

    /// The body of the authentication request (`String.format` with the name and the password).
    pub fn request_body(&self, form: &LoginForm) -> String {
        format_credentials(&self.request_body_template, &form.username, &form.password)
    }

    /// The expression that extracts the groups from the answer.
    pub fn groups_expression(&self) -> Option<&str> {
        self.groups_expression.as_deref()
    }

    /// The user of a successful answer.
    ///
    /// `groups` are the groups the caller extracted with `groups_expression` (which needs the expression
    /// engine, and therefore happens in the caller).
    pub fn user(
        &self,
        username: &str,
        body: Option<&str>,
        groups: Vec<String>,
    ) -> AuthenticatedUser {
        let mut user = AuthenticatedUser {
            id: username.to_string(),
            groups: groups.into_iter().map(normalise_group).collect(),
            kind: AuthKind::WebService,
            ..Default::default()
        };
        if let Some(body) = body {
            user.attributes
                .insert("response".to_string(), serde_json::json!(body));
            match serde_json::from_str::<serde_json::Value>(body) {
                Ok(json) => {
                    user.attributes.insert("json".to_string(), json);
                }
                Err(error) => tracing::warn!(
                    "Invalid json response returned by web service, response is: {body} ({error})"
                ),
            }
        }
        user
    }

    /// Sends the credentials to the web service and returns its answer.
    pub async fn call(&self, form: &LoginForm) -> Result<Option<String>, AuthError> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(self.request_body(form))
            .send()
            .await
            .map_err(|error| AuthError::Backend(format!("Internal error {error}")))?;

        let status = response.status();
        let body = response.text().await.ok();
        if status.is_success() {
            return Ok(body);
        }
        if status.is_client_error() {
            return Err(AuthError::InvalidCredentials);
        }
        Err(AuthError::Backend(format!(
            "Unknown response received {status}"
        )))
    }
}

/// Replaces the `%s` placeholders of the template with the user name and the password.
///
/// Java uses `String.format(template, username, password)`; only `%s` is supported here, which is what the
/// documented configurations use.
pub fn format_credentials(template: &str, username: &str, password: &str) -> String {
    let mut result = String::with_capacity(template.len() + username.len() + password.len());
    let mut values = [username, password].into_iter();
    let mut characters = template.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '%' {
            result.push(character);
            continue;
        }
        match characters.peek() {
            Some('s') => {
                characters.next();
                result.push_str(values.next().unwrap_or_default());
            }
            Some('%') => {
                characters.next();
                result.push('%');
            }
            _ => result.push('%'),
        }
    }
    result
}

#[async_trait::async_trait]
impl AuthBackend for WebServiceAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        true
    }

    async fn authenticate_async(&self, form: &LoginForm) -> Result<AuthenticatedUser, AuthError> {
        let body = self.call(form).await?;
        let groups = match (&self.groups_expression, &body) {
            (Some(expression), Some(body)) => self.extract_groups(expression, body),
            _ => Vec::new(),
        };
        Ok(self.user(&form.username, body.as_deref(), groups))
    }
}

impl WebServiceAuthenticationBackend {
    /// Evaluates `groups-expression` against the answer of the web service.
    fn extract_groups(&self, expression: &str, body: &str) -> Vec<String> {
        let json: serde_json::Value = match serde_json::from_str(body) {
            Ok(json) => json,
            Err(_) => return Vec::new(),
        };
        let context = crate::spec::expression::ExpressionContextBuilder::new()
            .json(json)
            .build();
        let resolver = crate::spec::expression::SpelResolver::new(context);
        match resolver.evaluate_to_list(expression) {
            Ok(groups) => groups,
            Err(error) => {
                tracing::warn!("cannot evaluate proxy.webservice.groups-expression: {error}");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    const CONFIG: &str = r##"
proxy:
  authentication: webservice
  webservice:
    authentication-url: http://localhost:8000/login
    authentication-request-body: '{"username":"%s","password":"%s"}'
    groups-expression: "#{json.groups}"
"##;

    #[test]
    fn requires_the_url_and_the_body() {
        let error = WebServiceAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: webservice\n",
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "Webservice authentication enabled, but no \
             'proxy.webservice.authentication-request-body' defined!"
        );

        let error = WebServiceAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: webservice\n  webservice:\n    \
             authentication-request-body: '{}'\n",
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "Webservice authentication enabled, but no 'proxy.webservice.authentication-url' defined!"
        );

        let backend = WebServiceAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        assert_eq!(backend.name(), "webservice");
        assert!(backend.has_authorization());
        assert!(backend.uses_login_form());
    }

    #[test]
    fn formats_the_request_body_like_java() {
        let backend = WebServiceAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        let body = backend.request_body(&LoginForm {
            username: "jack".to_string(),
            password: "s3cret".to_string(),
        });
        assert_eq!(body, r#"{"username":"jack","password":"s3cret"}"#);

        // literal percent signs and missing placeholders
        assert_eq!(format_credentials("100%% sure", "a", "b"), "100% sure");
        assert_eq!(format_credentials("%s only", "a", "b"), "a only");
        assert_eq!(
            format_credentials("no placeholders", "a", "b"),
            "no placeholders"
        );
        assert_eq!(format_credentials("%d", "a", "b"), "%d");
    }

    #[test]
    fn builds_the_user_of_an_answer() {
        let backend = WebServiceAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        assert_eq!(backend.groups_expression(), Some("#{json.groups}"));

        let user = backend.user(
            "jack",
            Some(r#"{"groups":["scientists","ROLE_admins"],"other":1}"#),
            vec!["scientists".to_string(), "ROLE_admins".to_string()],
        );
        assert_eq!(user.id, "jack");
        assert_eq!(user.groups, vec!["SCIENTISTS", "ADMINS"]);
        assert_eq!(user.kind, AuthKind::WebService);
        assert_eq!(
            user.attributes["json"]["other"],
            serde_json::json!(1),
            "the answer is available to expressions"
        );

        // an answer that is not JSON is kept as a string
        let user = backend.user("jack", Some("not json"), Vec::new());
        assert_eq!(user.attributes["response"], serde_json::json!("not json"));
        assert!(!user.attributes.contains_key("json"));

        // no answer at all
        let user = backend.user("jack", None, Vec::new());
        assert!(user.attributes.is_empty());
    }
}
