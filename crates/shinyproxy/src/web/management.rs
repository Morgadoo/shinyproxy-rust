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

//! The management server (Spring Boot Actuator).
//!
//! Spring Boot serves the actuator endpoints on `management.server.port` (9090 in the ShinyProxy
//! defaults) so that they are not reachable through the public port. This module serves the same
//! endpoints, with the same JSON shapes:
//!
//! * `/actuator/health`, `/actuator/health/liveness` and `/actuator/health/readiness` (which is `DOWN`
//!   while app recovery is running, exactly like the app-recovery health indicator),
//! * `/actuator/prometheus` with the metrics of this server,
//! * `/actuator/recyclable`, which says whether the server can be replaced,
//! * `/actuator/info`.
//!
//! The endpoints of the public port keep working as well (`/actuator/health` is public there), because
//! `management.endpoints.web.exposure.include` of ShinyProxy exposes them on both.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use super::state::AppState;

/// Builds the router of the management server.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/actuator", get(index))
        .route("/actuator/health", get(health))
        .route("/actuator/health/liveness", get(liveness))
        .route("/actuator/health/readiness", get(readiness))
        .route("/actuator/info", get(info))
        .route("/actuator/prometheus", get(prometheus))
        .route("/actuator/metrics", get(prometheus))
        .route("/actuator/recyclable", get(recyclable))
        .with_state(state)
}

/// The endpoints of this server, as Spring's discovery page lists them.
async fn index(State(state): State<Arc<AppState>>) -> Response {
    let base = format!(
        "http://{}:{}/actuator",
        state.settings.proxy.bind_address(),
        state.settings.management.port()
    );
    let link = |path: &str| json!({"href": format!("{base}{path}"), "templated": false});
    Json(json!({
        "_links": {
            "self": link(""),
            "health": link("/health"),
            "health-path": {"href": format!("{base}/health/{{*path}}"), "templated": true},
            "info": link("/info"),
            "prometheus": link("/prometheus"),
            "recyclable": link("/recyclable"),
        }
    }))
    .into_response()
}

/// `GET /actuator/health`.
pub async fn health(State(state): State<Arc<AppState>>) -> Response {
    let ready = state.recovery.is_ready();
    let status = if ready { "UP" } else { "DOWN" };
    let code = if ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": status,
            "groups": ["liveness", "readiness"],
        })),
    )
        .into_response()
}

/// `GET /actuator/health/liveness` — up as soon as the server runs.
pub async fn liveness() -> Response {
    Json(json!({"status": "UP"})).into_response()
}

/// `GET /actuator/health/readiness` — down while app recovery is running.
pub async fn readiness(State(state): State<Arc<AppState>>) -> Response {
    let ready = state.recovery.is_ready();
    let code = if ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": if ready { "UP" } else { "DOWN" },
            "components": {
                "appRecovery": {"status": if ready { "UP" } else { "DOWN" }},
            }
        })),
    )
        .into_response()
}

/// `GET /actuator/info`.
async fn info(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({
        "build": {
            "version": crate::VERSION,
            "name": "shinyproxy",
        },
        "shinyproxy": {
            "instanceId": state.identifiers.instance_id,
            "runtimeId": state.identifiers.runtime_id,
            "realmId": state.identifiers.realm_id,
            "compatibleWith": containerproxy::COMPATIBLE_WITH_JAVA_VERSION,
        }
    }))
    .into_response()
}

/// `GET /actuator/prometheus` — the metrics of this server.
pub async fn prometheus(State(state): State<Arc<AppState>>) -> Response {
    // the gauges of the running apps are refreshed on every scrape (the Java implementation refreshes
    // them every 20 seconds from a timer; a scrape is at least as often and gives fresher numbers)
    state
        .metrics
        .update_running_apps(&state.store.all_proxies());
    // the users that are logged in are counted by the session service (`absolute_users_logged_in` and
    // `absolute_users_active` in the Java implementation)
    if let Some(count) = state.sessions.logged_in_users().await {
        state.metrics.set_gauge(
            "absolute_users_logged_in",
            std::collections::BTreeMap::new(),
            count as f64,
        );
    }
    // the seats of the apps with pre-initialized containers (`ProxySharingMicrometer`)
    for scaler in &state.sharing_scalers {
        let labels =
            std::collections::BTreeMap::from([("spec_id".to_string(), scaler.spec().id.clone())]);
        let seats = scaler.seats();
        state.metrics.set_gauge(
            "seats_unclaimed",
            labels.clone(),
            seats.unclaimed_count() as f64,
        );
        state.metrics.set_gauge(
            "seats_claimed",
            labels.clone(),
            seats.claimed_count() as f64,
        );
        state
            .metrics
            .set_gauge("seats_creating", labels, scaler.pending_seats() as f64);
    }

    if let Some(count) = state.sessions.active_users().await {
        state.metrics.set_gauge(
            "absolute_users_active",
            std::collections::BTreeMap::new(),
            count as f64,
        );
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.to_prometheus(),
    )
        .into_response()
}

/// `GET /actuator/recyclable` — whether this server can be replaced.
pub async fn recyclable(State(state): State<Arc<AppState>>) -> Response {
    let busy = state.proxies.is_busy();
    let connections = state.websockets.count();
    Json(json!({
        "isRecyclable": connections == 0 && !busy,
        "activeConnections": connections,
    }))
    .into_response()
}
