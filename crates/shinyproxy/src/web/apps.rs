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

//! The app page, starting apps and proxying requests to them.
//!
//! Mirrors the Java `AppController`: `/app/{app}` (and `/app_i/{app}/{instance}`) render the page that
//! embeds the app in an iframe, `POST /app_i/{spec}/{instance}` starts it, and
//! `/app_proxy/{targetId}/**` proxies the traffic of the iframe to the container.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use containerproxy::auth::AuthenticatedUser;
use containerproxy::dataplane::cache_headers;
use containerproxy::dataplane::http::{app_unavailable_response, ForwardOptions};
use containerproxy::dataplane::inject::ScriptInjector;
use containerproxy::dataplane::ws::{proxy_upgrade, TunnelObserver};
use containerproxy::model::proxy::{now_millis, Proxy, ProxyStatus, ProxyStopReason};
use containerproxy::model::runtime_value::{
    RuntimeValue, PUBLIC_PATH, {self as runtime_value},
};
use containerproxy::model::spec::{CacheHeadersMode, ProxySpec};
use containerproxy::service::runtime_values::parse_cache_headers_mode;
use containerproxy::service::ParameterValues;
use containerproxy::spec::SpecProvider;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::model::{prepare_model, Page};
use super::router::CurrentUser;
use super::state::AppState;
use crate::runtime_values::{
    APP_INSTANCE, CUSTOM_APP_DETAILS, DEFAULT_INSTANCE, FORCE_FULL_RELOAD, TRACK_APP_URL,
    USER_TIMEZONE, WEBSOCKET_RECONNECTION_MODE,
};
use crate::spec_provider::ShinyProxySpecProvider;

/// Length of a target id (a UUID), as in `DefaultTargetMappingStrategy.TARGET_ID_LENGTH`.
pub const TARGET_ID_LENGTH: usize = 36;

/// Body of `POST /app_i/{specId}/{instance}`.
#[derive(Debug, Default, Deserialize)]
pub struct StartAppBody {
    /// Parameters chosen by the user.
    #[serde(default)]
    pub parameters: Option<BTreeMap<String, String>>,
    /// Time zone of the browser.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// `GET /app/{app}/{*sub}` — the page that embeds the app.
pub async fn app_page(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(path): Path<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Response {
    let app_name = path.get("app").cloned().unwrap_or_default();
    let instance = path
        .get("instance")
        .cloned()
        .unwrap_or_else(|| DEFAULT_INSTANCE.to_string());
    let sub_path = path.get("sub").cloned().unwrap_or_default();

    let spec = state.specs.spec(&app_name).cloned();
    let proxy = find_user_proxy(&state, user.as_ref(), &app_name, &instance);

    // no access (or no such app) and no running app: Spring's access denied handler answers with the API
    // document, whatever the request asked for (verified against the Java implementation)
    let Some(spec) = spec.filter(|spec| state.can_access(user.as_ref(), spec)) else {
        if proxy.is_none() {
            return forbidden_document();
        }
        return render_app_page(
            &state,
            user.as_ref(),
            &app_name,
            &instance,
            None,
            proxy,
            &sub_path,
            &query,
        );
    };

    // external apps are a link, not a proxied app
    if let Some(url) = ShinyProxySpecProvider::external(&spec)
        .external_url
        .filter(|url| !url.trim().is_empty())
    {
        return containerproxy::web::security::found(&url);
    }

    // a sub path that names a port mapping must end with a slash
    if let Some(redirect) = redirect_for_mapping(&state, &spec, &sub_path, request.uri()) {
        return redirect;
    }

    render_app_page(
        &state,
        user.as_ref(),
        &app_name,
        &instance,
        Some(&spec),
        proxy,
        &sub_path,
        &query,
    )
}

/// Renders `app.html`.
#[allow(clippy::too_many_arguments)]
fn render_app_page(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    app_name: &str,
    instance: &str,
    spec: Option<&ProxySpec>,
    proxy: Option<Proxy>,
    sub_path: &str,
    query: &HashMap<String, String>,
) -> Response {
    let hide_navbar = query.get("sp_hide_navbar").map(String::as_str) == Some("true");
    let mut model = prepare_model(state, Page::App, user, hide_navbar);

    let app_path = if instance == DEFAULT_INSTANCE {
        format!("{}app/{app_name}", state.context_path_with_slash())
    } else {
        format!(
            "{}app_i/{app_name}/{instance}",
            state.context_path_with_slash()
        )
    };

    let title = match (&proxy, spec) {
        (Some(proxy), _) => proxy
            .runtime_value(&runtime_value::DISPLAY_NAME)
            .unwrap_or_else(|| app_name.to_string()),
        (None, Some(spec)) => spec.display_name_or_id().to_string(),
        (None, None) => "ShinyProxy".to_string(),
    };

    model.insert("appName".into(), json!(app_name));
    model.insert("appInstance".into(), json!(instance));
    model.insert("appPath".into(), json!(app_path));
    model.insert("appTitle".into(), json!(title));
    model.insert(
        "heartbeatRate".into(),
        json!(state.settings.proxy.heartbeat_rate_ms()),
    );
    model.insert(
        "containerSubPath".into(),
        json!(container_sub_path(sub_path, query)),
    );
    model.insert(
        "refreshOpenidEnabled".into(),
        json!(state.auth.name() == "openid"),
    );
    model.insert(
        "proxy".into(),
        match &proxy {
            Some(proxy) => proxy.api_json(),
            None => serde_json::Value::Null,
        },
    );

    // the parameter form: the values this user may choose, the allowed combinations and the selection
    // to show (the values of the app being resumed win over the configured defaults)
    let parameters = spec.and_then(|spec| spec.parameters.as_ref());
    match (spec, parameters) {
        (Some(spec), Some(parameters)) => {
            let previous: Option<ParameterValues> = proxy
                .as_ref()
                .and_then(|proxy| proxy.runtime_values.get(&runtime_value::PARAMETER_VALUES))
                .and_then(|value| value.data.parse_json());
            let allowed = state.allowed_parameters(user, spec, previous.as_ref());

            model.insert(
                "parameterDefinitions".into(),
                json!(parameters
                    .definitions
                    .iter()
                    .map(|definition| json!({
                        "id": definition.id,
                        "displayNameOrId": definition.display_name_or_id(),
                        "description": definition.description,
                    }))
                    .collect::<Vec<_>>()),
            );
            model.insert("parameterIds".into(), json!(parameters.ids()));
            model.insert("parameterValues".into(), json!(allowed.values));
            model.insert("parameterDefaults".into(), json!(allowed.default_value));
            model.insert(
                "parameterAllowedCombinations".into(),
                json!(allowed.allowed_combinations),
            );
            model.insert(
                "cleanedAppParameterDescriptions".into(),
                json!(parameters
                    .definitions
                    .iter()
                    .map(|definition| (
                        definition.id.clone(),
                        containerproxy::util::clean_html(
                            definition.description.as_deref().unwrap_or("")
                        )
                    ))
                    .collect::<BTreeMap<_, _>>()),
            );
            // an app may bring its own form (`parameters.template`), rendered with the same model
            model.insert(
                "parameterFragment".into(),
                match &parameters.template {
                    Some(template) => {
                        let context = serde_json::Value::Object(model.clone());
                        match state
                            .templates
                            .render_string(template, minijinja::Value::from_serialize(context))
                        {
                            Ok(html) => json!(html),
                            Err(error) => {
                                tracing::warn!(
                                    "cannot render the parameters template of {}: {error}",
                                    spec.id
                                );
                                json!(null)
                            }
                        }
                    }
                    None => json!(null),
                },
            );
        }
        _ => {
            for key in [
                "parameterDefinitions",
                "parameterIds",
                "parameterValues",
                "parameterDefaults",
                "parameterAllowedCombinations",
                "parameterFragment",
            ] {
                model.insert(key.into(), json!(null));
            }
        }
    }

    match state.templates.render(
        "app.html",
        minijinja::Value::from_serialize(serde_json::Value::Object(model)),
    ) {
        Ok(html) => (
            containerproxy::web::security::no_cache_headers(),
            axum::response::Html(html),
        )
            .into_response(),
        Err(error) => {
            tracing::error!("cannot render app.html: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error\n").into_response()
        }
    }
}

/// The path inside the app, with the ShinyProxy query parameters removed (`buildContainerSubPath`).
fn container_sub_path(sub_path: &str, query: &HashMap<String, String>) -> String {
    let mut remaining: Vec<String> = query
        .iter()
        .filter(|(name, _)| {
            name.as_str() != "sp_hide_navbar" && name.as_str() != "sp_automatic_reload"
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    remaining.sort();

    let path = sub_path.trim_start_matches('/');
    if remaining.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", remaining.join("&"))
    }
}

/// Redirects `/app/x/mapping` to `/app/x/mapping/`, as the Java implementation does.
fn redirect_for_mapping(
    state: &AppState,
    spec: &ProxySpec,
    sub_path: &str,
    uri: &axum::http::Uri,
) -> Option<Response> {
    let trimmed = sub_path.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    let is_mapping = spec
        .container_spec()
        .map(|container| {
            container
                .port_mapping
                .iter()
                .any(|mapping| mapping.name == trimmed)
        })
        .unwrap_or(false);
    if !is_mapping {
        return None;
    }
    let _ = state;
    let mut target = uri.path().to_string();
    target.push('/');
    if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    }
    Some(containerproxy::web::security::found(&target))
}

/// `POST /app_i/{specId}/{instance}` — starts an app.
pub async fn start_app(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path((spec_id, instance)): Path<(String, String)>,
    body: Option<Json<StartAppBody>>,
) -> Response {
    let Some(user) = user else {
        return api_forbidden();
    };
    let Some(spec) = state
        .specs
        .spec(&spec_id)
        .cloned()
        .filter(|spec| state.can_access(Some(&user), spec))
    else {
        return api_forbidden();
    };

    if instance.len() > 64 || !is_valid_instance_name(&instance) {
        return api_fail("Invalid app instance name");
    }

    if find_user_proxy(&state, Some(&user), &spec_id, &instance).is_some() {
        return api_fail("You already have an instance of this app with the given name");
    }

    // max-instances per user and app
    let max_instances = state
        .max_instances(Some(&user))
        .get(&spec_id)
        .copied()
        .unwrap_or(1);
    if max_instances >= 0 {
        let running = state.proxies.user_proxies_by_spec(&user.id, &spec_id).len() as i64;
        if running >= max_instances {
            return api_fail(&format!(
                "Cannot start this app because you are using the maximum number of instances ({max_instances}) of this app."
            ));
        }
    }

    let proxy_id = uuid::Uuid::new_v4().to_string();
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let mut runtime_values =
        shinyproxy_runtime_values(&state, &spec, &instance, &proxy_id, body.timezone);

    // the parameters the user chose (when the app asks for parameters)
    match state.parse_parameters(Some(&user), &spec, body.parameters.as_ref()) {
        Ok(values) => runtime_values.extend(values),
        Err(error) => return api_fail(&error.0),
    }

    let proxy =
        match state
            .proxies
            .create_proxy(&proxy_id, &user.to_user_context(), &spec, runtime_values)
        {
            Ok(proxy) => proxy,
            Err(error) => return api_fail(&error.to_string()),
        };

    // starting takes a while: answer immediately (status New) and continue in the background, exactly
    // like the Java AsyncProxyService
    let response = proxy.api_json();
    let background_state = state.clone();
    let user_context = user.to_user_context();
    tokio::spawn(async move {
        match background_state
            .proxies
            .start_proxy(proxy, &spec, &user_context)
            .await
        {
            Ok(proxy) => background_state.router.add_mappings(&proxy),
            Err(error) => tracing::warn!("failed to start app {spec_id}: {error}"),
        }
    });

    (
        StatusCode::OK,
        Json(json!({"status": "success", "data": response})),
    )
        .into_response()
}

/// The ShinyProxy specific runtime values of a new proxy (`ShinyProxySpecProvider.getRuntimeValues`).
fn shinyproxy_runtime_values(
    state: &AppState,
    spec: &ProxySpec,
    instance: &str,
    proxy_id: &str,
    timezone: Option<String>,
) -> Vec<RuntimeValue> {
    let extension = ShinyProxySpecProvider::extension(spec);
    let proxy_settings = &state.settings.proxy;

    let reconnection_mode = extension
        .websocket_reconnection_mode
        .map(|mode| format!("{mode:?}"))
        .or_else(|| proxy_settings.default_websocket_reconnection_mode.clone())
        .unwrap_or_else(|| "None".to_string());

    let track_app_url = extension.track_app_url.unwrap_or_else(|| {
        proxy_settings
            .default_track_app_url
            .map(|value| value.0)
            .unwrap_or(false)
    });

    let mut values = vec![
        RuntimeValue::string(&WEBSOCKET_RECONNECTION_MODE, reconnection_mode),
        RuntimeValue::boolean(
            &FORCE_FULL_RELOAD,
            extension.shiny_force_full_reload.unwrap_or(false),
        ),
        RuntimeValue::boolean(&TRACK_APP_URL, track_app_url),
        RuntimeValue::json(&CUSTOM_APP_DETAILS, &extension.custom_app_details),
        RuntimeValue::string(&APP_INSTANCE, instance),
        RuntimeValue::string(
            &PUBLIC_PATH,
            format!("{}app_proxy/{proxy_id}/", state.context_path_with_slash()),
        ),
    ];
    if let Some(timezone) = timezone {
        values.push(RuntimeValue::string(&USER_TIMEZONE, timezone));
    }
    values
}

/// `ANY /app_proxy/{targetId}/**` — proxies a request of the iframe to the app.
pub async fn app_proxy(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    session: tower_sessions::Session,
    request: Request,
) -> Response {
    let path = request.uri().path();
    let prefix = format!("{}app_proxy/", state.context_path_with_slash());
    let Some(rest) = path.strip_prefix(&prefix) else {
        return app_stopped_response();
    };
    let (target_id, sub_path) = match rest.split_once('/') {
        Some((target_id, sub_path)) => (target_id, sub_path),
        None => (rest, ""),
    };
    if target_id.len() != TARGET_ID_LENGTH {
        return app_stopped_response();
    }

    // `sp_proxy_id` overrides the lookup, as in the Java implementation; the query is only parsed when the
    // parameter can be there at all (this handler runs for every request an app receives)
    let override_id = request
        .uri()
        .query()
        .filter(|query| query.contains("sp_proxy_id"))
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(name, _)| name == "sp_proxy_id")
                .map(|(_, value)| value.to_string())
        });
    let proxy = match override_id {
        Some(proxy_id) => state.proxies.proxy_ref(&proxy_id),
        None => find_proxy_by_target(&state, user.as_ref(), target_id),
    };

    let Some(proxy) = proxy else {
        return app_stopped_response();
    };
    if proxy.status.is_unavailable() || !is_owner(&state, user.as_ref(), &proxy) {
        return app_stopped_response();
    }

    let Some(resolved) = state.router.resolve(&proxy, sub_path) else {
        return app_stopped_response();
    };

    let method = request.method().clone();
    let inject = should_inject_script(&method, request.headers());
    let url = resolved.url(request.uri().query());

    let mut options = ForwardOptions {
        extra_headers: state.proxy_headers(&proxy),
        force_identity_encoding: inject,
    };
    if !inject {
        options.force_identity_encoding = false;
    }

    let heartbeat_rate =
        Duration::from_millis(state.settings.proxy.heartbeat_rate_ms().max(1) as u64);
    let observer: Arc<dyn TunnelObserver> = Arc::new(HeartbeatObserver {
        state: state.clone(),
        proxy_id: proxy.id.clone(),
        session_id: Some(containerproxy::web::session::session_id(&session)),
        user_id: user.as_ref().map(|user| user.id.clone()),
    });

    // every request to the app counts as a heartbeat (as in Java, only for the proxy's own target)
    if proxy.target_id() == proxy.id {
        state.heartbeats.update(&proxy.id, now_millis());
    }

    let response = proxy_upgrade(request, &url, &options, heartbeat_rate, observer).await;

    let mut response = match response {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(
                "Failed request was proxied to {url} [proxyId: {}]: {error}",
                proxy.id
            );
            return app_crashed_or_stopped(&state, &proxy).await;
        }
    };

    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        return app_crashed_or_stopped(&state, &proxy).await;
    }

    // cache headers of the app, according to the mode of the proxy
    let mode = proxy
        .runtime_value(&runtime_value::CACHE_HEADERS_MODE)
        .as_deref()
        .and_then(parse_cache_headers_mode)
        .unwrap_or(CacheHeadersMode::EnforceNoCache);
    cache_headers::apply(mode, &method, response.headers_mut());

    if inject && is_html(response.headers()) {
        let script = format!(
            "{}{}/js/shiny.iframe.js",
            state.context_path_with_slash(),
            state.identifiers.instance_id
        );
        let (mut parts, body) = response.into_parts();
        parts.headers.remove(header::CONTENT_LENGTH);
        let mut injector = ScriptInjector::new(&script);
        let stream = body.into_data_stream().map(move |chunk| match chunk {
            Ok(chunk) => Ok(injector.push(&chunk)),
            Err(error) => Err(error),
        });
        return Response::from_parts(parts, Body::from_stream(stream));
    }

    response
}

/// Reports heartbeats of the WebSocket tunnel to the heartbeat store.
#[derive(Debug)]
struct HeartbeatObserver {
    state: Arc<AppState>,
    proxy_id: String,
    /// The session and the user of the browser, so that its session stays alive while the app is used
    /// (`SessionReActivatorService`).
    session_id: Option<String>,
    user_id: Option<String>,
}

impl TunnelObserver for HeartbeatObserver {
    fn heartbeat(&self) {
        self.state.heartbeats.update(&self.proxy_id, now_millis());
        // a user that only looks at their app still has an active session
        if let (Some(session_id), Some(user_id)) = (&self.session_id, &self.user_id) {
            self.state.sessions.reactivate(session_id, user_id);
        }
    }

    fn opened(&self) {
        self.state.websockets.opened();
    }

    fn closed(&self) {
        self.state.websockets.closed();
    }
}

/// Whether the iframe script has to be injected: navigations of HTML pages only.
fn should_inject_script(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET {
        return false;
    }
    let accepts_html = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html") || accept.contains("*/*"));
    if !accepts_html {
        return false;
    }
    // `Sec-Fetch-Mode: navigate` marks a real navigation; the header is relatively new, so the script
    // is injected when it is absent (see #30809 in the Java implementation)
    match headers
        .get("sec-fetch-mode")
        .and_then(|value| value.to_str().ok())
    {
        Some(mode) => mode == "navigate",
        None => true,
    }
}

fn is_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

/// The headers ShinyProxy adds to every request to the app.
/// Decides between `app_crashed` and `app_stopped_or_non_existent` and stops a crashed app.
async fn app_crashed_or_stopped(state: &Arc<AppState>, proxy: &Proxy) -> Response {
    let current = state.proxies.proxy(&proxy.id);
    match current {
        Some(current) if !current.status.is_unavailable() => {
            if !state.proxies.is_proxy_healthy(&current).await {
                tracing::info!(
                    "Proxy unreachable/crashed, stopping it now [proxyId: {}]",
                    current.id
                );
                state.router.remove_mappings(&current.id);
                let _ = state
                    .proxies
                    .stop_proxy(&current, ProxyStopReason::Crashed)
                    .await;
            }
            app_unavailable_response(true)
        }
        _ => app_unavailable_response(false),
    }
}

/// `ANY /app_direct/{app}/**` and `/app_direct_i/{app}/{instance}/**` — proxy that starts the app.
///
/// Used for embedding an app somewhere else: the app is started if it does not exist yet and the request
/// is proxied once it is up (`AppDirectController`). Parameters and resuming are not supported here,
/// exactly as in the Java implementation.
pub async fn app_direct(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    session: tower_sessions::Session,
    request: Request,
) -> Response {
    let context = state.context_path_with_slash();
    let path = request.uri().path().to_string();

    let Some(info) = AppRequestInfo::parse(&path, &context) else {
        return (StatusCode::FORBIDDEN, "Forbidden\n").into_response();
    };

    // the URL must end with a slash, otherwise relative links of the app break
    let Some(sub_path) = info.sub_path.clone() else {
        let mut target = path.clone();
        target.push('/');
        if let Some(query) = request.uri().query() {
            target.push('?');
            target.push_str(query);
        }
        return containerproxy::web::security::found(&target);
    };

    let Some(user) = user else {
        return (StatusCode::FORBIDDEN, "Forbidden\n").into_response();
    };

    let mut proxy = find_user_proxy(&state, Some(&user), &info.app_name, &info.app_instance);
    if proxy.is_none() {
        let Some(spec) = state
            .specs
            .spec(&info.app_name)
            .cloned()
            .filter(|spec| state.can_access(Some(&user), spec))
        else {
            return (StatusCode::FORBIDDEN, "Forbidden\n").into_response();
        };

        // max-instances per user and app
        let max_instances = state
            .max_instances(Some(&user))
            .get(&info.app_name)
            .copied()
            .unwrap_or(1);
        if max_instances >= 0
            && state
                .proxies
                .user_proxies_by_spec(&user.id, &info.app_name)
                .len() as i64
                >= max_instances
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot start new proxy because the maximum amount of instances of this proxy has been reached\n",
            )
                .into_response();
        }

        let proxy_id = uuid::Uuid::new_v4().to_string();
        let mut runtime_values =
            shinyproxy_runtime_values(&state, &spec, &info.app_instance, &proxy_id, None);
        // app_direct serves the app under its own path instead of /app_proxy
        runtime_values.retain(|value| value.key.env_var != PUBLIC_PATH.env_var);
        runtime_values.push(RuntimeValue::string(
            &PUBLIC_PATH,
            format!(
                "{context}app_direct_i/{}/{}",
                info.app_name, info.app_instance
            ),
        ));

        let created = match state.proxies.create_proxy(
            &proxy_id,
            &user.to_user_context(),
            &spec,
            runtime_values,
        ) {
            Ok(created) => created,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to start app {}: {error}\n", info.app_name),
                )
                    .into_response()
            }
        };

        match state
            .proxies
            .start_proxy(created, &spec, &user.to_user_context())
            .await
        {
            Ok(started) => {
                state.router.add_mappings(&started);
                proxy = Some(started);
            }
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to start app {}: {error}\n", info.app_name),
                )
                    .into_response()
            }
        }
    }

    let Some(proxy) = proxy else {
        return app_stopped_response();
    };

    // wait for an app that is still starting (the Java implementation waits up to 10 minutes)
    let proxy = match wait_until_up(&state, &proxy).await {
        Some(proxy) => proxy,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to start app {}\n", info.app_name),
            )
                .into_response()
        }
    };

    let Some(resolved) = state.router.resolve(&proxy, &sub_path) else {
        return app_stopped_response();
    };
    let url = resolved.url(request.uri().query());
    forward_to_app(&state, &proxy, request, &url, &session, Some(&user)).await
}

/// Waits until a proxy is up, giving up after ten minutes (as in `AppDirectController`).
async fn wait_until_up(state: &Arc<AppState>, proxy: &Proxy) -> Option<Proxy> {
    if proxy.status == ProxyStatus::Up {
        return Some(proxy.clone());
    }
    for _ in 0..600 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match state.proxies.proxy(&proxy.id) {
            Some(current) if current.status == ProxyStatus::Up => return Some(current),
            Some(current) if current.status == ProxyStatus::New => continue,
            _ => return None,
        }
    }
    None
}

/// `ANY /api/route/{targetId}/**` — the raw proxy route used by embedded clients.
pub async fn api_route(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    session: tower_sessions::Session,
    request: Request,
) -> Response {
    // unlike `/app_proxy/**` (which the browser code of the app page watches for the "stopped" document),
    // this route answers a target the user may not use with the access denied document of Spring
    let context = state.context_path_with_slash();
    let prefix = format!("{context}api/route/");
    let Some(rest) = request.uri().path().strip_prefix(&prefix) else {
        return forbidden_document();
    };
    let (target_id, sub_path) = match rest.split_once('/') {
        Some((target_id, sub_path)) => (target_id.to_string(), sub_path.to_string()),
        None => (rest.to_string(), String::new()),
    };

    let Some(proxy) = find_proxy_by_target(&state, user.as_ref(), &target_id) else {
        return forbidden_document();
    };
    if proxy.status.is_unavailable() {
        return forbidden_document();
    }
    let Some(resolved) = state.router.resolve(&proxy, &sub_path) else {
        return forbidden_document();
    };
    let url = resolved.url(request.uri().query());
    forward_to_app(&state, &proxy, request, &url, &session, user.as_ref()).await
}

/// Forwards a request to an app, without the iframe script injection.
async fn forward_to_app(
    state: &Arc<AppState>,
    proxy: &Proxy,
    request: Request,
    url: &str,
    session: &tower_sessions::Session,
    user: Option<&containerproxy::auth::AuthenticatedUser>,
) -> Response {
    let method = request.method().clone();
    let options = ForwardOptions {
        extra_headers: state.proxy_headers(proxy),
        force_identity_encoding: false,
    };
    let heartbeat_rate =
        Duration::from_millis(state.settings.proxy.heartbeat_rate_ms().max(1) as u64);
    let observer: Arc<dyn TunnelObserver> = Arc::new(HeartbeatObserver {
        state: state.clone(),
        proxy_id: proxy.id.clone(),
        session_id: Some(containerproxy::web::session::session_id(session)),
        user_id: user.map(|user| user.id.clone()),
    });
    state.heartbeats.update(&proxy.id, now_millis());

    match proxy_upgrade(request, url, &options, heartbeat_rate, observer).await {
        Ok(mut response) => {
            if response.status() == StatusCode::SERVICE_UNAVAILABLE {
                return app_crashed_or_stopped(state, proxy).await;
            }
            let mode = proxy
                .runtime_value(&runtime_value::CACHE_HEADERS_MODE)
                .as_deref()
                .and_then(parse_cache_headers_mode)
                .unwrap_or(CacheHeadersMode::EnforceNoCache);
            cache_headers::apply(mode, &method, response.headers_mut());
            response
        }
        Err(error) => {
            tracing::info!("Failed request to {url} [proxyId: {}]: {error}", proxy.id);
            app_crashed_or_stopped(state, proxy).await
        }
    }
}

/// The app name, instance and sub path of an `/app`, `/app_i`, `/app_direct` or `/app_direct_i` URL.
///
/// Mirrors `AppRequestInfo.fromURI`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRequestInfo {
    /// Name of the app.
    pub app_name: String,
    /// Name of the instance (`_` when the URL has none).
    pub app_instance: String,
    /// The path inside the app, `None` when the URL does not end with a slash.
    pub sub_path: Option<String>,
}

impl AppRequestInfo {
    /// Parses a request path.
    pub fn parse(path: &str, context_path: &str) -> Option<Self> {
        let path = path.strip_prefix(context_path).unwrap_or(path);
        let path = path.trim_start_matches('/');
        let mut segments = path.split('/');
        let prefix = segments.next()?;
        let with_instance = matches!(prefix, "app_i" | "app_direct_i");
        if !matches!(prefix, "app" | "app_i" | "app_direct" | "app_direct_i") {
            return None;
        }

        let app_name = segments.next().filter(|name| !name.is_empty())?.to_string();
        let app_instance = if with_instance {
            let instance = segments.next().filter(|name| !name.is_empty())?;
            if instance.len() > 64 || !is_valid_instance_name(instance) {
                return None;
            }
            instance.to_string()
        } else {
            DEFAULT_INSTANCE.to_string()
        };

        let rest: Vec<&str> = segments.collect();
        let sub_path = if rest.is_empty() {
            None
        } else {
            Some(rest.join("/"))
        };

        Some(AppRequestInfo {
            app_name,
            app_instance,
            sub_path,
        })
    }
}

/// `POST /heartbeat/{proxyId}` — forces a heartbeat.
pub async fn heartbeat(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
) -> Response {
    let Some(proxy) = owned_available_proxy(&state, user.as_ref(), &proxy_id) else {
        return app_stopped_api_response();
    };
    state.heartbeats.update(&proxy.id, now_millis());
    (
        StatusCode::OK,
        Json(json!({"status": "success", "data": null})),
    )
        .into_response()
}

/// `GET /heartbeat/{proxyId}` — information about the heartbeat of an app.
pub async fn heartbeat_info(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Path(proxy_id): Path<String>,
) -> Response {
    let Some(proxy) = owned_available_proxy(&state, user.as_ref(), &proxy_id) else {
        return app_stopped_api_response();
    };
    let last_heartbeat = state.heartbeats.get(&proxy.id);
    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "lastHeartbeat": last_heartbeat,
                "heartbeatRate": state.settings.proxy.heartbeat_rate_ms(),
            }
        })),
    )
        .into_response()
}

/// The proxy of a user with the given app name and instance.
pub fn find_user_proxy(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    app_name: &str,
    instance: &str,
) -> Option<Proxy> {
    let user = user?;
    state
        .proxies
        .user_proxies_by_spec(&user.id, app_name)
        .into_iter()
        .find(|proxy| {
            proxy
                .runtime_value(&APP_INSTANCE)
                .is_some_and(|value| value == instance)
        })
}

/// The proxy of a user with the given target id.
fn find_proxy_by_target(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    target_id: &str,
) -> Option<std::sync::Arc<Proxy>> {
    let user = user?;
    // shared instead of copied: this runs on every request an app receives
    state.proxies.find_user_proxy_by_target(&user.id, target_id)
}

/// A proxy of the user that is currently available.
fn owned_available_proxy(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    proxy_id: &str,
) -> Option<Proxy> {
    let proxy = state.proxies.proxy(proxy_id)?;
    if proxy.status.is_unavailable() || !is_owner(state, user, &proxy) {
        return None;
    }
    Some(proxy)
}

/// Whether the user owns the proxy (`UserService.isOwner`).
pub fn is_owner(state: &AppState, user: Option<&AuthenticatedUser>, proxy: &Proxy) -> bool {
    match (user, &proxy.user_id) {
        (Some(user), Some(owner)) => state.username_equals(&user.id, owner),
        _ => false,
    }
}

/// Whether an instance name is valid (`INSTANCE_NAME_PATTERN`).
pub fn is_valid_instance_name(instance: &str) -> bool {
    instance
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-'))
}

fn api_forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"status": "fail", "data": "forbidden"})),
    )
        .into_response()
}

fn api_fail(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"status": "fail", "data": message})),
    )
        .into_response()
}

/// The 410 answer of the ShinyProxy API for apps that are gone.
fn app_stopped_api_response() -> Response {
    (
        StatusCode::GONE,
        Json(json!({"status": "fail", "data": "app_stopped_or_non_existent"})),
    )
        .into_response()
}

/// The document Spring's access denied handler produces (403 with the API envelope).
pub fn forbidden_document() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"status": "fail", "data": "forbidden"})),
    )
        .into_response()
}

/// The same answer, for requests of the iframe.
fn app_stopped_response() -> Response {
    app_stopped_api_response()
}

/// Status values that end a `watch` request.
pub fn is_final_status(status: ProxyStatus) -> bool {
    matches!(
        status,
        ProxyStatus::Up | ProxyStatus::Stopped | ProxyStatus::Paused
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_instance_names() {
        assert!(is_valid_instance_name("default"));
        assert!(is_valid_instance_name("my-instance_1.2"));
        assert!(is_valid_instance_name("_"));
        assert!(!is_valid_instance_name("with space"));
        assert!(!is_valid_instance_name("with/slash"));
        assert!(!is_valid_instance_name("with?query"));
    }

    #[test]
    fn builds_the_container_sub_path() {
        let query = HashMap::from([
            ("sp_hide_navbar".to_string(), "true".to_string()),
            ("sp_automatic_reload".to_string(), "true".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);
        assert_eq!(container_sub_path("/sub/page", &query), "sub/page?a=1");
        assert_eq!(container_sub_path("", &HashMap::new()), "");
        assert_eq!(
            container_sub_path(
                "/x",
                &HashMap::from([("sp_hide_navbar".to_string(), "true".to_string())])
            ),
            "x"
        );
    }

    #[test]
    fn decides_when_to_inject_the_script() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "text/html".parse().unwrap());
        assert!(should_inject_script(&Method::GET, &headers));

        headers.insert("sec-fetch-mode", "navigate".parse().unwrap());
        assert!(should_inject_script(&Method::GET, &headers));

        headers.insert("sec-fetch-mode", "cors".parse().unwrap());
        assert!(!should_inject_script(&Method::GET, &headers));

        // POST requests and non-html requests are never rewritten
        headers.remove("sec-fetch-mode");
        assert!(!should_inject_script(&Method::POST, &headers));
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "application/json".parse().unwrap());
        assert!(!should_inject_script(&Method::GET, &headers));
    }

    #[test]
    fn knows_the_final_statuses_of_a_watch_request() {
        assert!(is_final_status(ProxyStatus::Up));
        assert!(is_final_status(ProxyStatus::Stopped));
        assert!(is_final_status(ProxyStatus::Paused));
        assert!(!is_final_status(ProxyStatus::New));
        assert!(!is_final_status(ProxyStatus::Resuming));
    }
}
