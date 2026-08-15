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

//! The Grafana proxy (`MonitoringController`).
//!
//! When `proxy.monitoring.grafana-url` is set, `/grafana/**` is reverse proxied to that Grafana (only for
//! administrators, which the authorization middleware enforces because `/grafana/` is an admin path). The
//! current user is passed on in `X-SP-UserId`, so that Grafana can use auth proxy authentication exactly
//! as with the Java implementation.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use containerproxy::dataplane::http::ForwardOptions;
use containerproxy::dataplane::ws::{proxy_upgrade, TunnelObserver};

use super::router::CurrentUser;
use super::state::AppState;

/// Heartbeat interval of the WebSocket tunnel of Grafana (live dashboards use WebSockets).
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// An observer that does nothing: Grafana traffic is not app activity.
#[derive(Debug)]
struct NoObserver;

impl TunnelObserver for NoObserver {
    fn heartbeat(&self) {}
}

/// `ANY /grafana/**` — proxies to the configured Grafana.
pub async fn grafana(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    request: Request,
) -> Response {
    let Some(grafana_url) = state.grafana_url() else {
        // the Java implementation forwards to /error with status 403
        return (StatusCode::FORBIDDEN, "Forbidden\n").into_response();
    };

    let context = state.context_path_with_slash();
    let path = request.uri().path().to_string();
    let path_within_application = path
        .strip_prefix(context.trim_end_matches('/'))
        .unwrap_or(&path)
        .to_string();

    // Grafana needs to be served from a path that ends with a slash
    if path_within_application == "/grafana" {
        return containerproxy::web::security::found(&format!("{context}grafana/"));
    }
    let Some(rest) = path_within_application.strip_prefix("/grafana/") else {
        return (StatusCode::FORBIDDEN, "Forbidden\n").into_response();
    };

    let mut target = format!("{grafana_url}/{rest}");
    if let Some(query) = request.uri().query() {
        target.push('?');
        target.push_str(query);
    }

    let mut extra_headers = std::collections::BTreeMap::new();
    if let Some(user) = &user {
        extra_headers.insert("X-SP-UserId".to_string(), user.id.clone());
    }

    let options = ForwardOptions {
        extra_headers,
        force_identity_encoding: false,
    };
    match proxy_upgrade(
        request,
        &target,
        &options,
        HEARTBEAT_INTERVAL,
        Arc::new(NoObserver),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!("Failed request to Grafana ({target}): {error}");
            (StatusCode::BAD_GATEWAY, "Grafana is not reachable\n").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use containerproxy::config::Settings;

    /// The URL is used without its trailing slash, as in `MonitoringService`.
    #[test]
    fn normalises_the_configured_url() {
        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  monitoring:\n    grafana-url: http://grafana:3000/\n",
        )
        .expect("settings");
        assert_eq!(
            crate::web::state::grafana_url(&settings).as_deref(),
            Some("http://grafana:3000")
        );

        let settings = Settings::default();
        assert_eq!(crate::web::state::grafana_url(&settings), None);
    }
}
