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

//! The REST API the ShinyProxy front-end uses.
//!
//! Mirrors the Java `ProxyController`/`ProxyStatusController`: the envelopes, messages and the
//! long-polling `watch` behaviour are identical.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use containerproxy::auth::AuthenticatedUser;
use containerproxy::model::proxy::{Proxy, ProxyStatus, ProxyStopReason};
use containerproxy::spec::SpecProvider;
use serde::Deserialize;
use serde_json::json;

use super::apps::{is_final_status, is_owner};
use super::router::CurrentUser;
use super::state::AppState;

/// Query parameters of `GET /api/proxy/{id}/status`.
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    /// Whether to wait for the status to change.
    #[serde(default)]
    pub watch: Option<bool>,
    /// How long to wait, in seconds (10..=60).
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Body of `PUT /api/proxy/{id}/status`.
#[derive(Debug, Deserialize)]
pub struct ChangeStatusBody {
    /// The requested status: `Stopping`, `Pausing` or `Resuming`.
    pub status: String,
    /// Parameters for `Resuming`.
    #[serde(default)]
    pub parameters: Option<HashMap<String, String>>,
}

/// `GET /api/proxyspec` — the app definitions the user may access.
pub async fn proxy_specs(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
) -> Response {
    let hide_details = state.settings.proxy.api_security.hide_spec_details();
    let specs: Vec<serde_json::Value> = state
        .specs
        .specs()
        .iter()
        .filter(|spec| state.can_access(user.as_ref(), spec))
        .map(|spec| spec.api_json(hide_details))
        .collect();
    success(json!(specs))
}

/// `GET /api/proxyspec/{id}` — one app definition.
pub async fn proxy_spec(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(spec_id): Path<String>,
) -> Response {
    let hide_details = state.settings.proxy.api_security.hide_spec_details();
    match state
        .specs
        .spec(&spec_id)
        .filter(|spec| state.can_access(user.as_ref(), spec))
    {
        Some(spec) => success(spec.api_json(hide_details)),
        None => forbidden(),
    }
}

/// `GET /api/proxy` — the proxies of the user.
pub async fn proxies(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
) -> Response {
    let Some(user) = user else { return forbidden() };
    let proxies: Vec<serde_json::Value> = state
        .proxies
        .user_proxies(&user.id)
        .iter()
        .map(Proxy::api_json)
        .collect();
    success(json!(proxies))
}

/// `GET /api/proxy/{id}` — one proxy of the user.
pub async fn proxy(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
) -> Response {
    match state.proxies.proxy(&proxy_id) {
        Some(proxy) if is_owner(&state, user.as_ref(), &proxy) => success(proxy.api_json()),
        _ => forbidden(),
    }
}

/// `GET /api/proxy/{id}/status` — the status of a proxy, optionally waiting for it to change.
pub async fn proxy_status(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
    Query(query): Query<StatusQuery>,
) -> Response {
    let proxy = state
        .proxies
        .proxy(&proxy_id)
        .filter(|proxy| is_owner(&state, user.as_ref(), proxy));

    // unknown proxies are reported as stopped, as in the Java implementation
    let Some(proxy) = proxy else {
        return success(stopped_proxy(&proxy_id));
    };

    if query.watch != Some(true) || is_final_status(proxy.status) {
        return success(proxy.api_json());
    }

    let timeout = query.timeout.unwrap_or(10);
    if !(10..=60).contains(&timeout) {
        return fail("Timeout must be between 10 and 60 seconds (inclusive).");
    }

    // wait for the status to change; the events of the engine wake us up immediately
    let mut events = state.proxies.events().subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(event)) => {
                if event
                    .proxy()
                    .map(|proxy| proxy.id == proxy_id)
                    .unwrap_or(false)
                {
                    break;
                }
            }
            // the channel lagged or closed: fall back to polling the store
            Ok(Err(_)) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => break,
        }
    }

    match state.proxies.proxy(&proxy_id) {
        Some(proxy) => success(proxy.api_json()),
        None => success(stopped_proxy(&proxy_id)),
    }
}

/// `PUT /api/proxy/{id}/status` — stop, pause or resume a proxy.
pub async fn change_proxy_status(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
    Json(body): Json<ChangeStatusBody>,
) -> Response {
    let Some(proxy) = state.proxies.proxy(&proxy_id) else {
        return forbidden();
    };
    let owner = is_owner(&state, user.as_ref(), &proxy);
    let admin_stopping = body.status == "Stopping" && state.is_admin(user.as_ref());
    if !owner && !admin_stopping {
        return forbidden();
    }

    match body.status.as_str() {
        "Stopping" => {
            if proxy.status == ProxyStatus::Stopped {
                return fail("Cannot stop proxy because it is already stopped");
            }
            let state_for_stop = state.clone();
            let proxy_to_stop = proxy.clone();
            tokio::spawn(async move {
                state_for_stop.router.remove_mappings(&proxy_to_stop.id);
                if let Err(error) = state_for_stop
                    .proxies
                    .stop_proxy(&proxy_to_stop, ProxyStopReason::ByUser)
                    .await
                {
                    tracing::warn!("cannot stop proxy {}: {error}", proxy_to_stop.id);
                }
            });
            success(serde_json::Value::Null)
        }
        "Pausing" => {
            if proxy.status != ProxyStatus::Up {
                return fail(&format!(
                    "Cannot pause proxy because it is not in Up status (status is {})",
                    proxy.status
                ));
            }
            if !state.backend.supports_pause() {
                return fail("Pausing apps is not supported by this backend");
            }
            // pausing takes a while, so the client is told to watch the status (as in Java)
            let state_for_pause = state.clone();
            let proxy_to_pause = proxy.clone();
            tokio::spawn(async move {
                state_for_pause.router.remove_mappings(&proxy_to_pause.id);
                if let Err(error) = state_for_pause.proxies.pause_proxy(&proxy_to_pause).await {
                    tracing::warn!("cannot pause proxy {}: {error}", proxy_to_pause.id);
                }
            });
            success(serde_json::Value::Null)
        }
        "Resuming" => {
            if proxy.status != ProxyStatus::Paused {
                return fail(&format!(
                    "Cannot resume proxy because it is not in Paused status (status is {})",
                    proxy.status
                ));
            }
            if !state.backend.supports_pause() {
                return fail("Resuming apps is not supported by this backend");
            }
            // only the owner may resume an app (an admin may only stop it)
            if !owner {
                return forbidden();
            }
            let Some(spec) = proxy
                .spec_id
                .as_deref()
                .and_then(|spec_id| state.specs.spec(spec_id))
                .cloned()
            else {
                return fail("Cannot resume proxy because the app definition no longer exists");
            };

            // choosing parameters again while resuming arrives with the parameters feature (P9); the
            // app is resumed with the parameters it was started with, which is what happens in Java
            // when the request contains no parameters
            let resuming = proxy.clone();

            let state_for_resume = state.clone();
            tokio::spawn(async move {
                match state_for_resume
                    .proxies
                    .resume_proxy(&resuming, &spec)
                    .await
                {
                    Ok(resumed) => state_for_resume.router.add_mappings(&resumed),
                    Err(error) => {
                        tracing::warn!("cannot resume proxy {}: {error}", resuming.id)
                    }
                }
            });
            success(serde_json::Value::Null)
        }
        _ => fail("Invalid status"),
    }
}

/// Body of `PUT /api/proxy/{id}/userId`.
#[derive(Debug, Deserialize)]
pub struct ChangeUserIdBody {
    /// The user the app is transferred to.
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
}

/// `PUT /api/proxy/{id}/userId` — transfers an app to another user (`ProxyApiController`).
pub async fn change_proxy_user_id(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
    Json(body): Json<ChangeUserIdBody>,
) -> Response {
    if !state.settings.proxy.allow_transfer_app() {
        return forbidden();
    }

    // ownership is checked before validation, so that the answer does not leak whether the app exists
    let Some(proxy) = state
        .proxies
        .proxy(&proxy_id)
        .filter(|proxy| is_owner(&state, user.as_ref(), proxy))
    else {
        return forbidden();
    };

    let Some(new_user_id) = body.user_id.filter(|value| !value.trim().is_empty()) else {
        return fail("Cannot transfer app because no userId is provided in the request");
    };

    if proxy.status != ProxyStatus::Up {
        return fail(&format!(
            "Cannot transfer app because it is not in Up status (status is {})",
            proxy.status
        ));
    }

    if state.username_equals(proxy.user_id.as_deref().unwrap_or_default(), &new_user_id) {
        return fail("Cannot transfer app because the proxy is already owned by this user");
    }

    // the instance is renamed so that the new owner does not get a name clash
    let instance = proxy
        .runtime_value(&crate::runtime_values::APP_INSTANCE)
        .unwrap_or_else(|| crate::runtime_values::DEFAULT_INSTANCE.to_string());
    let instance = crate::runtime_values::instance_display_name(&instance).to_string();
    let mut new_instance = format!("{}-{instance}", proxy.user_id.clone().unwrap_or_default());
    new_instance.truncate(64);

    let mut transferred = proxy.clone();
    transferred.user_id = Some(new_user_id);
    transferred.add_runtime_value(
        containerproxy::model::runtime_value::RuntimeValue::string(
            &crate::runtime_values::APP_INSTANCE,
            new_instance,
        ),
        true,
    );

    // remove and add so that the indexes of the store are rebuilt
    state.store.remove_proxy(&proxy);
    state.store.add_proxy(&transferred);

    success(serde_json::Value::Null)
}

/// `GET /api/proxy/{id}/details` — the custom app details, with their expressions resolved.
pub async fn proxy_details(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
) -> Response {
    let Some(proxy) = state
        .proxies
        .proxy(&proxy_id)
        .filter(|proxy| is_owner(&state, user.as_ref(), proxy))
    else {
        return app_stopped();
    };

    let details: Vec<crate::spec_provider::CustomAppDetail> = proxy
        .runtime_values
        .get(&crate::runtime_values::CUSTOM_APP_DETAILS)
        .and_then(|value| value.data.parse_json())
        .unwrap_or_default();
    if details.is_empty() {
        return success(json!([]));
    }

    let spec = proxy
        .spec_id
        .as_deref()
        .and_then(|spec_id| state.specs.spec(spec_id))
        .cloned();
    let resolver = state.resolver_for_proxy(user.as_ref(), &proxy, spec.as_ref());

    let resolved: Vec<serde_json::Value> = details
        .into_iter()
        .map(|detail| {
            let value = match &detail.value {
                Some(raw) => match resolver.evaluate_to_string(raw) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        tracing::warn!(
                            "Error while resolving CustomAppDetail expression '{}': {error}",
                            detail.name.clone().unwrap_or_default()
                        );
                        detail.value.clone()
                    }
                },
                None => None,
            };
            json!({"name": detail.name, "description": detail.description, "value": value})
        })
        .collect();

    success(json!(resolved))
}

/// The 410 answer for apps that are gone.
fn app_stopped() -> Response {
    (
        StatusCode::GONE,
        Json(json!({"status": "fail", "message": "app_stopped_or_non_existent"})),
    )
        .into_response()
}

/// `DELETE /admin/delegate-proxy` — removes the pre-initialized containers (admin only).
///
/// Container pre-initialization lands in P12; until then the endpoint answers successfully because there
/// is nothing to remove, which is also what a deployment without pre-initialization does in Java.
pub async fn remove_delegate_proxies(State(_state): State<Arc<AppState>>) -> Response {
    success(serde_json::Value::Null)
}

/// `GET /admin/data` — the proxies of all users (admin only).
pub async fn admin_data(State(state): State<Arc<AppState>>) -> Response {
    let proxies: Vec<serde_json::Value> = state
        .proxies
        .all_proxies()
        .iter()
        .map(|proxy| admin_proxy_info(&state, proxy))
        .collect();
    success(json!(proxies))
}

/// The `ProxyInfo` document of the admin page.
fn admin_proxy_info(state: &AppState, proxy: &Proxy) -> serde_json::Value {
    use containerproxy::model::runtime_value::{
        BACKEND_CONTAINER_NAME, CONTAINER_IMAGE, HEARTBEAT_TIMEOUT, INSTANCE_ID, MAX_LIFETIME,
    };

    let uptime = if proxy.startup_timestamp > 0 {
        format_seconds(
            (containerproxy::model::proxy::now_millis() - proxy.startup_timestamp) / 1000,
        )
    } else {
        "N/A".to_string()
    };
    let last_heartbeat = match state.heartbeats.get(&proxy.id) {
        Some(timestamp) => {
            format_seconds((containerproxy::model::proxy::now_millis() - timestamp) / 1000)
        }
        None => "N/A".to_string(),
    };

    let (image_name, image_tag, backend_container_name) = match proxy.containers.first() {
        Some(container) => {
            let image = container
                .runtime_values
                .value_string(&CONTAINER_IMAGE)
                .unwrap_or_else(|| "N/A".to_string());
            let (name, tag) = match image.split_once(':') {
                Some((name, tag)) => (name.to_string(), tag.to_string()),
                None => (image, "N/A".to_string()),
            };
            let backend_name = container
                .runtime_values
                .value_string(&BACKEND_CONTAINER_NAME)
                .unwrap_or_else(|| "N/A".to_string());
            (name, tag, backend_name)
        }
        None => ("N/A".to_string(), "N/A".to_string(), "N/A".to_string()),
    };

    let heartbeat_timeout = proxy
        .runtime_values
        .get(&HEARTBEAT_TIMEOUT)
        .and_then(|value| value.data.as_int())
        .filter(|value| *value != -1)
        .map(|value| format_seconds(value / 1000));
    let max_lifetime = proxy
        .runtime_values
        .get(&MAX_LIFETIME)
        .and_then(|value| value.data.as_int())
        .filter(|value| *value != -1)
        .map(|value| format_seconds(value * 60));

    json!({
        "status": proxy.status.to_string(),
        "proxyId": proxy.id,
        "userId": proxy.user_id,
        "appName": proxy.spec_id,
        "instanceName": crate::runtime_values::instance_display_name(
            &proxy
                .runtime_value(&crate::runtime_values::APP_INSTANCE)
                .unwrap_or_else(|| "_".to_string()),
        ),
        "endpoint": proxy.default_target().unwrap_or("N/A"),
        "uptime": uptime,
        "lastHeartBeat": last_heartbeat,
        "imageName": image_name,
        "imageTag": image_tag,
        "heartbeatTimeout": heartbeat_timeout,
        "maxLifetime": max_lifetime,
        "spInstance": proxy
            .runtime_values
            .value_string(&INSTANCE_ID)
            .unwrap_or_else(|| "N/A".to_string()),
        "backendContainerName": backend_container_name,
        "parameters": serde_json::Value::Null,
    })
}

/// Formats a duration as `H:MM:SS`, as the admin page expects.
pub fn format_seconds(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn stopped_proxy(proxy_id: &str) -> serde_json::Value {
    Proxy::new(proxy_id, ProxyStatus::Stopped).api_json()
}

fn success(data: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({"status": "success", "data": data})),
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

/// Access to the authenticated user for handlers that need it.
pub type CurrentUserExtension = axum::Extension<CurrentUser>;

/// Convenience alias used by the router.
pub type MaybeUser = Option<AuthenticatedUser>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_like_the_admin_page() {
        assert_eq!(format_seconds(0), "0:00:00");
        assert_eq!(format_seconds(39), "0:00:39");
        assert_eq!(format_seconds(120), "0:02:00");
        assert_eq!(format_seconds(3661), "1:01:01");
        assert_eq!(format_seconds(-5), "0:00:00");
    }
}
