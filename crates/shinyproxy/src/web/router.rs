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

//! HTTP routes of the ShinyProxy user interface.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Form, Router};
use containerproxy::auth::{AuthError, AuthenticatedUser, LoginForm};
use containerproxy::spec::SpecProvider;
use containerproxy::web::security::{found, is_admin_path, is_public_path, no_cache_headers};
use containerproxy::web::session::SessionData;
use containerproxy::web::{assets, session};
use minijinja::Value as TemplateValue;
use tower_sessions::Session;

use super::model::{prepare_model, Page};
use super::state::AppState;

/// Builds the router of the ShinyProxy user interface.
///
/// Routes are registered with the context path as prefix (instead of using `Router::nest`, which
/// treats `/context` and `/context/` as different paths); the bare context path redirects to the
/// index page, exactly like the Java `IndexController` does for an empty servlet path.
pub fn router(state: Arc<AppState>) -> Router {
    let context = state.context_path();
    let path = |suffix: &str| format!("{context}{suffix}");

    let mut routes = Router::new()
        .route(&path("/"), get(index))
        // --- apps ---
        .route(&path("/app/{app}"), get(super::apps::app_page))
        .route(&path("/app/{app}/{*sub}"), get(super::apps::app_page))
        .route(&path("/app_i/{app}/{instance}"), get(super::apps::app_page))
        .route(
            &path("/app_i/{app}/{instance}/{*sub}"),
            get(super::apps::app_page),
        )
        .route(
            &path("/app_i/{app}/{instance}"),
            post(super::apps::start_app),
        )
        .route(&path("/app_proxy/{target}"), any(super::apps::app_proxy))
        .route(&path("/app_proxy/{target}/"), any(super::apps::app_proxy))
        .route(
            &path("/app_proxy/{target}/{*rest}"),
            any(super::apps::app_proxy),
        )
        .route(
            &path("/heartbeat/{proxy}"),
            get(super::apps::heartbeat_info).post(super::apps::heartbeat),
        )
        // apps that are embedded elsewhere: started on demand and proxied directly
        .route(&path("/app_direct/{app}"), any(super::apps::app_direct))
        .route(&path("/app_direct/{app}/"), any(super::apps::app_direct))
        .route(
            &path("/app_direct/{app}/{*rest}"),
            any(super::apps::app_direct),
        )
        .route(
            &path("/app_direct_i/{app}/{instance}"),
            any(super::apps::app_direct),
        )
        .route(
            &path("/app_direct_i/{app}/{instance}/"),
            any(super::apps::app_direct),
        )
        .route(
            &path("/app_direct_i/{app}/{instance}/{*rest}"),
            any(super::apps::app_direct),
        )
        .route(&path("/api/route/{target}"), any(super::apps::api_route))
        .route(&path("/api/route/{target}/"), any(super::apps::api_route))
        .route(
            &path("/api/route/{target}/{*rest}"),
            any(super::apps::api_route),
        )
        // --- api ---
        .route(&path("/api/proxyspec"), get(super::api::proxy_specs))
        .route(&path("/api/proxyspec/{spec}"), get(super::api::proxy_spec))
        .route(&path("/api/proxy"), get(super::api::proxies))
        .route(&path("/api/proxy/{proxy}"), get(super::api::proxy))
        .route(
            &path("/api/proxy/{proxy}/status"),
            get(super::api::proxy_status).put(super::api::change_proxy_status),
        )
        .route(
            &path("/api/proxy/{proxy}/userId"),
            axum::routing::put(super::api::change_proxy_user_id),
        )
        .route(
            &path("/api/proxy/{proxy}/details"),
            get(super::api::proxy_details),
        )
        .route(
            &path("/admin/delegate-proxy"),
            axum::routing::delete(super::api::remove_delegate_proxies),
        )
        // the actuator endpoints are also exposed on the public port, like
        // management.endpoints.web.exposure.include of ShinyProxy does
        .route(&path("/actuator/health"), get(super::management::health))
        .route(
            &path("/actuator/health/liveness"),
            get(super::management::liveness),
        )
        .route(
            &path("/actuator/health/readiness"),
            get(super::management::readiness),
        )
        .route(
            &path("/actuator/prometheus"),
            get(super::management::prometheus),
        )
        .route(
            &path("/actuator/recyclable"),
            get(super::management::recyclable),
        )
        // OpenID Connect: the login page sends the user to the provider, which sends them back to the
        // callback (the same paths as the Java implementation, so existing client registrations work)
        .route(
            &path(containerproxy::auth::openid::AUTHORIZATION_PATH),
            get(super::oidc::start_authorization),
        )
        .route(
            &path(containerproxy::auth::openid::CALLBACK_PATH),
            get(super::oidc::callback),
        )
        .route(&path("/refresh-openid"), get(super::oidc::refresh_openid))
        .route(&path("/issue"), post(super::issue::report_issue))
        .route(&path("/v3/api-docs"), get(super::openapi::api_docs))
        .route(
            &path("/swagger-ui/index.html"),
            get(super::openapi::swagger_ui),
        )
        // the Grafana proxy (administrators only, enforced by is_admin_path)
        .route(&path("/grafana"), any(super::monitoring::grafana))
        .route(&path("/grafana/"), any(super::monitoring::grafana))
        .route(&path("/grafana/{*rest}"), any(super::monitoring::grafana))
        .route(&path("/admin"), get(super::admin::admin_page))
        .route(&path("/admin/about"), get(super::admin::about_page))
        .route(&path("/admin/data"), get(super::api::admin_data))
        .route(&path("/login"), get(login_page).post(login_submit))
        .route(&path("/logout"), get(logout).post(logout))
        .route(&path("/logout-success"), get(logout_success))
        .route(&path("/auth-success"), get(auth_success))
        .route(&path("/auth-error"), get(auth_error))
        .route(&path("/app-access-denied"), get(app_access_denied))
        .route(&path("/error"), get(error_page))
        .route(&path("/favicon.ico"), get(favicon))
        // the catch-all serves the embedded assets and the instance-id prefixed favicon
        .route(&path("/{*path}"), get(static_asset));

    if !context.is_empty() {
        routes = routes.route(&context, get(redirect_to_index));
    }

    routes
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            authorize,
        ))
        // runs before authorization, so that no request is served while apps are being recovered
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            app_recovery_filter,
        ))
        .with_state(state)
}

/// Answers 503 with `startup.html` while app recovery is running (`AppRecoveryFilter`).
///
/// Requests to `/actuator` are let through, so that the probes of an orchestrator keep working.
async fn app_recovery_filter(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if state.recovery.is_ready() {
        return next.run(request).await;
    }

    let path = strip_context_path(&state, request.uri().path());
    if path.starts_with("/actuator") {
        return next.run(request).await;
    }

    let html = state
        .templates
        .render(
            "startup.html",
            TemplateValue::from_serialize(serde_json::json!({
                "application_name": state.settings.application_name(),
            })),
        )
        .unwrap_or_else(|error| {
            tracing::error!("cannot render startup.html: {error}");
            format!(
                "{} is starting up, check back in a few seconds.\n",
                state.settings.application_name()
            )
        });
    (StatusCode::SERVICE_UNAVAILABLE, Html(html)).into_response()
}

/// Redirects `/{context-path}` to `/{context-path}/`.
async fn redirect_to_index(State(state): State<Arc<AppState>>) -> Response {
    found(&state.context_path_with_slash())
}

/// The current user of a request, resolved from the session.
#[derive(Debug, Clone, Default)]
pub struct CurrentUser(pub Option<AuthenticatedUser>);

/// Authorization middleware, mirroring the matcher order of the Java `WebSecurityConfig`.
async fn authorize(
    State(state): State<Arc<AppState>>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let path = strip_context_path(&state, &path);

    let mut data = SessionData::load(&session).await;

    // the `none` backend gives every session an anonymous user
    if data.user.is_none() && !state.auth.has_authorization() {
        if let Some(user) = state.auth.anonymous_user(&session::session_id(&session)) {
            data.user = Some(user);
            data.store(&session).await;
        }
    }

    // bearer tokens (`proxy.oauth2.*`): API clients authenticate per request, without a session
    if data.user.is_none() {
        if let (Some(authenticator), Some(token)) = (
            state.bearer.as_ref(),
            containerproxy::auth::bearer::bearer_token(request.headers()),
        ) {
            if let Some(user) = authenticator.authenticate(token).await {
                tracing::debug!(
                    "Request authenticated with a bearer token [user: {}]",
                    user.id
                );
                data.user = Some(user);
            }
        }
    }

    // header based authentication (`custom-header`): the reverse proxy in front of ShinyProxy decides who
    // the user is, so every request is checked (and a changed header replaces the user of the session)
    if !state.auth.uses_login_form() && state.auth.has_authorization() {
        let from_headers = state.auth.user_from_headers(request.headers());
        if let Some(user) = from_headers {
            let changed = data
                .user
                .as_ref()
                .map(|existing| existing != &user)
                .unwrap_or(true);
            if changed {
                if data.user.is_none() {
                    tracing::info!("User logged in [user: {}]", user.id);
                    state
                        .proxies
                        .events()
                        .publish(containerproxy::events::Event::UserLoggedIn {
                            user_id: user.id.clone(),
                        });
                }
                data.user = Some(user);
                data.store(&session).await;
            }
        }
    }

    // the access token of an OpenID Connect session is refreshed on the pages the Java filter matches
    if data.user.is_some()
        && state.openid.is_some()
        && refreshes_openid(&path)
        && !super::oidc::refresh_if_needed(&state, &session, &mut data).await
    {
        data.user = None;
        data.store(&session).await;
        let _ = session.flush().await;
        return found(&format!(
            "{}login?error=expired",
            state.context_path_with_slash()
        ))
        .into_response();
    }

    // using a session keeps it alive: Spring Session writes `lastAccessedTime` on every request, while
    // `tower-sessions` only moves the expiry when the session is *modified*. Without this, a user that is
    // active for longer than `spring.session.timeout` would suddenly be logged out (found by the soak run).
    keep_session_alive(&state, &session);

    let user = data.user.clone();
    // the session service counts the users that are logged in (`absolute_users_logged_in`) and those that
    // used their session in the last minute; with Redis sessions the counts are scanned from Redis instead
    if let Some(user) = &user {
        state
            .sessions
            .touch(&session::session_id(&session), &user.id);
    }
    request.extensions_mut().insert(CurrentUser(user.clone()));

    if !is_public_path(&path, &state.identifiers.instance_id) {
        if state.auth.has_authorization() && user.is_none() {
            // only the API paths answer with the document; every other path redirects to the login page,
            // whatever the request asked for (verified against the Java implementation)
            let mut answer = if needs_authentication_answer(&path) {
                // `AuthenticationRequiredFilter`: the API paths answer 410 with this exact document, which
                // the browser code of the app page recognises to reload the page
                (
                    StatusCode::GONE,
                    axum::Json(serde_json::json!({
                        "status": "fail",
                        "data": "shinyproxy_authentication_required"
                    })),
                )
                    .into_response()
            } else {
                // remember where the user wanted to go, like the Java saved-request handling; the
                // session itself is always created, exactly as Spring does for every request
                let mut data = data.clone();
                if is_saveable_request(&request) {
                    data.auth_success_url = Some(request.uri().to_string());
                }
                data.store(&session).await;
                found(&format!(
                    "{}{}",
                    state.context_path_with_slash(),
                    state.auth.login_redirect()
                ))
            };
            add_headers(&state, &path, &mut answer);
            return answer;
        }
        if is_admin_path(&path) && !state.is_admin(user.as_ref()) {
            // Spring's access denied handler answers with the API document, whatever the request asked for
            let mut answer = (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({"status": "fail", "data": "forbidden"})),
            )
                .into_response();
            add_headers(&state, &path, &mut answer);
            return answer;
        }
    }

    let mut response = next.run(request).await;
    add_headers(&state, &path, &mut response);
    response
}

/// Adds the security and cache headers to an answer of the server itself.
fn add_headers(state: &AppState, _path: &str, response: &mut Response) {
    // an answer that came from an app keeps its own cache headers
    let from_app = response
        .extensions()
        .get::<containerproxy::dataplane::AppAnswer>()
        .is_some();
    let headers = response.headers_mut();
    for (name, value) in state.security_headers.headers() {
        headers.insert(name.clone(), value.clone());
    }
    // Spring Security adds the cache headers to every answer of the server itself; the answers of an app
    // keep the headers the app chose (`/app_proxy` and `/api/route` set them per the cache headers mode)
    if !from_app {
        for (name, value) in no_cache_headers() {
            headers.entry(name).or_insert(value);
        }
    }
}

/// Moves the expiry of the session forward, because the session is being used.
///
/// Spring Session writes the last access time of a session on every request, so a session that is used never
/// expires; `tower-sessions` only moves the expiry when a handler changes the session. Setting the expiry
/// marks the session as modified, so the store keeps it alive for another `spring.session.timeout`.
fn keep_session_alive(state: &AppState, session: &Session) {
    let timeout = state.settings.spring.session.timeout_duration();
    if let Ok(timeout) = time::Duration::try_from(timeout) {
        session.set_expiry(Some(tower_sessions::Expiry::OnInactivity(timeout)));
    }
}

/// Whether an unauthenticated request to this path is answered with the API document instead of a redirect
/// to the login page (`AuthenticationRequiredFilter.REQUEST_MATCHER`).
fn needs_authentication_answer(path: &str) -> bool {
    path.starts_with("/app_proxy/")
        || path.starts_with("/heartbeat/")
        || path.starts_with("/api/")
        || path == "/admin/data"
        || path == "/issue"
}

/// Whether the access token is refreshed for this path (`OpenIdReAuthorizeFilter.REQUEST_MATCHER`).
fn refreshes_openid(path: &str) -> bool {
    path == "/"
        || path == "/refresh-openid"
        || path.starts_with("/app/")
        || path.starts_with("/app_i/")
}

/// Whether a request may be remembered as the page to open after logging in.
///
/// Mirrors the default request matcher of Spring Security's request cache
/// (`RequestCacheConfigurer.createDefaultSavedRequestMatcher`): only `GET` requests that ask for HTML
/// (an `Accept` header that favours `text/html`, not `*/*`) and that are not `XMLHttpRequest` are saved.
/// Without this, background requests of the browser (such as Chrome's
/// `/.well-known/appspecific/com.chrome.devtools.json`) would become the page shown after login.
fn is_saveable_request(request: &Request) -> bool {
    if request.method() != axum::http::Method::GET {
        return false;
    }
    let headers = request.headers();
    if headers
        .get("x-requested-with")
        .is_some_and(|value| value == "XMLHttpRequest")
    {
        return false;
    }
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept
                .split(',')
                .map(|media_type| media_type.split(';').next().unwrap_or("").trim())
                .any(|media_type| media_type == "text/html")
        })
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json") && !accept.contains("text/html"))
        || headers
            .get("x-requested-with")
            .is_some_and(|value| value == "XMLHttpRequest")
}

fn strip_context_path(state: &AppState, path: &str) -> String {
    let context = state.context_path();
    if context.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(&context) {
        Some("") => "/".to_string(),
        Some(rest) => rest.to_string(),
        None => path.to_string(),
    }
}

/// The index page (`IndexController`).
async fn index(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let landing_page = state.settings.proxy.landing_page().to_string();
    let accessible: Vec<String> = state
        .specs
        .specs()
        .iter()
        .filter(|spec| state.can_access(user.as_ref(), spec))
        .map(|spec| spec.id.clone())
        .collect();

    // `landing-page` may point at another page, or ask for a redirect to an app
    match landing_page.as_str() {
        "/" => {}
        "FirstApp" => {
            if let Some(first) = accessible.first() {
                return found(&format!("{}app/{first}", state.context_path_with_slash()))
                    .into_response();
            }
        }
        "SingleApp" => {
            if accessible.len() == 1 {
                return found(&format!(
                    "{}app/{}",
                    state.context_path_with_slash(),
                    accessible[0]
                ))
                .into_response();
            }
        }
        other => {
            let target = format!(
                "{}{}",
                state.context_path_with_slash(),
                other.trim_start_matches('/')
            );
            return found(&target);
        }
    }

    let hide_navbar = query.get("sp_hide_navbar").map(String::as_str) == Some("true");
    let model = prepare_model(&state, Page::Index, user.as_ref(), hide_navbar);
    render(&state, "index.html", model)
}

/// The login page (`AuthController.login`).
async fn login_page(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if state.auth.name() == containerproxy::auth::openid::NAME {
        // the Java login page redirects to the provider
        return found(&format!(
            "{}{}",
            state.context_path(),
            containerproxy::auth::openid::AUTHORIZATION_PATH
        ))
        .into_response();
    }
    if !state.auth.uses_login_form() {
        return found(&state.context_path_with_slash());
    }

    let mut data = SessionData::load(&session).await;
    let token = data.csrf_token(&session).await;

    let error = match query.get("error").map(String::as_str) {
        Some("expired") => Some("Your session has expired, please try again".to_string()),
        Some(_) => Some("Invalid user name or password".to_string()),
        None => None,
    };

    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert(
        "application_name".into(),
        serde_json::json!(state.settings.application_name()),
    );
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    model.insert(
        "bootstrapCss".into(),
        serde_json::json!("/css/bootstrap.css"),
    );
    model.insert("bootstrapJs".into(), serde_json::json!("/js/bootstrap.js"));
    model.insert(
        "jqueryJs".into(),
        serde_json::json!("/webjars/jquery/3.7.1/jquery.min.js"),
    );
    model.insert(
        "fontAwesomeCss".into(),
        serde_json::json!("/webjars/fontawesome/4.7.0/css/font-awesome.min.css"),
    );
    model.insert("csrfToken".into(), serde_json::json!(token));
    model.insert("csrfParameterName".into(), serde_json::json!("_csrf"));
    model.insert("error".into(), serde_json::json!(error));

    render(&state, "login.html", model)
}

/// Handles the login form (Spring's `formLogin`).
async fn login_submit(
    State(state): State<Arc<AppState>>,
    session: Session,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let context_path = state.context_path_with_slash();
    let mut data = SessionData::load(&session).await;

    // CSRF check, with the same redirect as the Java implementation when the token is missing/expired
    let token = form.get("_csrf").map(String::as_str).unwrap_or_default();
    if !data.csrf_token_matches(token) {
        return found(&format!("{context_path}login?error=expired"));
    }

    let credentials = LoginForm {
        username: form.get("username").cloned().unwrap_or_default(),
        password: form.get("password").cloned().unwrap_or_default(),
    };

    match state.auth.authenticate_async(&credentials).await {
        Ok(user) => {
            tracing::info!("User logged in [user: {}]", user.id);
            state
                .proxies
                .events()
                .publish(containerproxy::events::Event::UserLoggedIn {
                    user_id: user.id.clone(),
                });
            // a new session id after login prevents session fixation
            let _ = session.cycle_id().await;
            let target = data
                .auth_success_url
                .clone()
                .unwrap_or_else(|| context_path.clone());
            data.user = Some(user);
            data.csrf_token = None;
            data.auth_success_url = None;
            data.user_initiated_logout = false;
            data.store(&session).await;
            found(&format!(
                "{context_path}auth-success?continue={}",
                urlencode(&target)
            ))
            .into_response()
        }
        Err(AuthError::NoFormLogin) => found(&context_path),
        Err(error) => {
            tracing::info!(
                "Authentication failure [user: {}] [error: {error}]",
                credentials.username
            );
            state
                .proxies
                .events()
                .publish(containerproxy::events::Event::AuthenticationFailed {
                    user_id: credentials.username.clone(),
                });
            found(&format!("{context_path}login?error=true"))
        }
    }
}

/// Logs the user out (`UserService.logout` + Spring's logout handler).
async fn logout(State(state): State<Arc<AppState>>, session: Session) -> Response {
    let data = SessionData::load(&session).await;
    if let Some(user) = &data.user {
        tracing::info!("User logged out [user: {}]", user.id);
        state
            .proxies
            .events()
            .publish(containerproxy::events::Event::UserLoggedOut {
                user_id: user.id.clone(),
                expired: false,
            });

        // the apps of the user are stopped when they say so (`proxy.default-stop-proxy-on-logout` is
        // true by default), in the background so that the browser is not kept waiting
        let logout_state = state.clone();
        let user_id = user.id.clone();
        tokio::spawn(async move {
            let stopped = logout_state
                .release
                .on_logout(&user_id, &|proxy| logout_state.stop_on_logout(proxy))
                .await;
            for proxy_id in stopped {
                logout_state.router.remove_mappings(&proxy_id);
            }
        });
    }
    let mut data = data;
    data.user_initiated_logout = true;
    data.user = None;
    state.sessions.forget(&session::session_id(&session));
    data.store(&session).await;
    let _ = session.flush().await;

    // where a user lands after signing out depends on the authentication backend (`/login` by default,
    // `/logout-success` for header based authentication and the provider for OpenID Connect)
    let target = state.auth.logout_success_url();
    let target = if target.starts_with("http://") || target.starts_with("https://") {
        target
    } else {
        format!(
            "{}{}",
            state.context_path_with_slash(),
            target.trim_start_matches('/')
        )
    };
    found(&target)
}

/// The page shown after a successful authentication, which redirects to the requested page.
async fn auth_success(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let target = query
        .get("continue")
        .cloned()
        .unwrap_or_else(|| state.context_path_with_slash());
    // only local redirects are allowed (no open redirect)
    let target = if target.starts_with('/') && !target.starts_with("//") {
        target
    } else {
        state.context_path_with_slash()
    };
    // the page redirects with `new URL(...)`, which needs an absolute URL; the Java implementation puts
    // an absolute URL in the model as well (ServletUriComponentsBuilder.fromCurrentContextPath())
    let url = containerproxy::web::security::absolute_url(&headers, &target);
    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert("url".into(), serde_json::json!(url));
    render(&state, "auth-success.html", model)
}

async fn logout_success(State(state): State<Arc<AppState>>) -> Response {
    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    render(&state, "logout-success.html", model)
}

async fn auth_error(State(state): State<Arc<AppState>>) -> Response {
    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert(
        "application_name".into(),
        serde_json::json!(state.settings.application_name()),
    );
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    model.insert(
        "mainPage".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    render(&state, "auth-error.html", model)
}

async fn app_access_denied(State(state): State<Arc<AppState>>) -> Response {
    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    model.insert(
        "resourcePrefix".into(),
        serde_json::json!(format!(
            "{}{}",
            state.context_path_with_slash().trim_end_matches('/'),
            format_args!("/{}", state.identifiers.instance_id)
        )),
    );
    (
        StatusCode::FORBIDDEN,
        render(&state, "app-access-denied.html", model),
    )
        .into_response()
}

/// The error page (`ErrorController`), rendered as HTML or JSON depending on the request.
async fn error_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let status = query
        .get("status")
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if wants_json(&headers) {
        return (
            status,
            axum::Json(serde_json::json!({"status": "fail", "data": status.canonical_reason()})),
        )
            .into_response();
    }

    let (short_error, description) = match status {
        StatusCode::NOT_FOUND => ("Not found", "The requested page could not be found."),
        StatusCode::FORBIDDEN => (
            "Forbidden",
            "You do not have access to this page or application.",
        ),
        _ => (
            "An error occurred",
            "Please try again or contact your administrator.",
        ),
    };

    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert("shortError".into(), serde_json::json!(short_error));
    model.insert("description".into(), serde_json::json!(description));
    model.insert(
        "mainPage".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    (status, render(&state, "error.html", model)).into_response()
}

/// `GET /favicon.ico` and `GET /{instanceId}/favicon` (`FaviconController`).
async fn favicon(State(state): State<Arc<AppState>>) -> Response {
    serve_favicon(&state, None, None)
}

/// Serves the favicon of ShinyProxy or of one app (`/{instanceId}/favicon/{specId}`).
///
/// Per-app favicons fall back to the configured default; an unknown or inaccessible app gives 403 and a
/// missing favicon gives 404 without a body (so that browsers do not log a JSON parse error), which is
/// what the Java `FaviconController` does.
fn serve_favicon(
    state: &AppState,
    spec_id: Option<&str>,
    user: Option<&AuthenticatedUser>,
) -> Response {
    let mut path = state.settings.proxy.favicon_path.clone();

    if let Some(spec_id) = spec_id {
        let Some(spec) = state.specs.spec(spec_id) else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if !state.can_access(user, spec) {
            return StatusCode::FORBIDDEN.into_response();
        }
        if let Some(spec_favicon) = &spec.favicon_path {
            path = Some(spec_favicon.clone());
        }
    }

    let Some(path) = path else {
        // ShinyProxy has no built-in favicon
        return StatusCode::NOT_FOUND.into_response();
    };

    match std::fs::read(&path) {
        Ok(data) => (
            [
                (
                    header::CONTENT_TYPE,
                    mime_guess::from_path(&path)
                        .first_or_octet_stream()
                        .to_string(),
                ),
                (header::CACHE_CONTROL, "max-age=86400".to_string()),
            ],
            data,
        )
            .into_response(),
        Err(error) => {
            tracing::warn!("Error while reading favicon {path}: {error}");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// Serves embedded assets, both with and without the instance id prefix, and the favicon.
async fn static_asset(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
    uri: Uri,
) -> Response {
    let path = strip_context_path(&state, uri.path());
    let prefix = format!("/{}/", state.identifiers.instance_id);
    let (path, cacheable) = match path.strip_prefix(&prefix) {
        Some(rest) => (format!("/{rest}"), true),
        None => (path, false),
    };
    if path == "/favicon" || path == "/favicon.ico" {
        return serve_favicon(&state, None, user.as_ref());
    }
    if let Some(spec_id) = path.strip_prefix("/favicon/") {
        return serve_favicon(&state, Some(spec_id), user.as_ref());
    }
    if !assets::exists(&path) {
        return not_found(State(state.clone()), HeaderMap::new()).await;
    }
    assets::serve(&path, cacheable)
}

async fn not_found(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if wants_json(&headers) {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"status": "fail", "data": "Not Found"})),
        )
            .into_response();
    }
    let mut model = serde_json::Map::new();
    model.insert("title".into(), serde_json::json!(state.resolve_title(None)));
    model.insert("shortError".into(), serde_json::json!("Not found"));
    model.insert(
        "description".into(),
        serde_json::json!("The requested page could not be found."),
    );
    model.insert(
        "mainPage".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    model.insert(
        "contextPath".into(),
        serde_json::json!(state.context_path_with_slash()),
    );
    (StatusCode::NOT_FOUND, render(&state, "error.html", model)).into_response()
}

/// Renders a template, turning failures into a plain error response (and a log line).
fn render(
    state: &AppState,
    template: &str,
    model: serde_json::Map<String, serde_json::Value>,
) -> Response {
    let value = TemplateValue::from_serialize(serde_json::Value::Object(model));
    match state.templates.render(template, value) {
        Ok(html) => (no_cache_headers(), Html(html)).into_response(),
        Err(error) => {
            tracing::error!("cannot render {template}: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error\n").into_response()
        }
    }
}

pub(crate) fn urlencode(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    // the unreserved characters of RFC 3986 are left alone, like Spring's UriUtils
    const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    utf8_percent_encode(value, UNRESERVED).to_string()
}
