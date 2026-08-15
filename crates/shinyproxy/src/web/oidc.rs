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

//! The OpenID Connect endpoints (Spring Security's `oauth2Login`).
//!
//! * `/oauth2/authorization/shinyproxy` sends the user to the provider,
//! * `/login/oauth2/code/shinyproxy` receives the authorization code, exchanges it for tokens, verifies
//!   the id token and logs the user in.
//!
//! Failures end up on `/auth-error`, exactly like the failure handler of the Java implementation.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use containerproxy::auth::openid::{OpenIdAuthenticationBackend, CALLBACK_PATH};
use containerproxy::web::security::absolute_url;
use containerproxy::web::session::{OidcRequest, OidcTokens, SessionData};
use tower_sessions::Session;

use super::state::AppState;

/// `GET /oauth2/authorization/shinyproxy` — sends the user to the provider.
pub async fn start_authorization(
    State(state): State<Arc<AppState>>,
    session: Session,
    headers: HeaderMap,
) -> Response {
    let Some(backend) = openid_backend(&state) else {
        return auth_error(&state);
    };

    let redirect_uri = absolute_url(
        &headers,
        &format!("{}{CALLBACK_PATH}", state.context_path()),
    );
    let request = backend.authorization_request(&redirect_uri);

    let mut data = SessionData::load(&session).await;
    data.oidc_request = Some(OidcRequest {
        state: request.state.clone(),
        nonce: request.nonce.clone(),
        verifier: request.verifier.clone(),
        redirect_uri,
    });
    data.store(&session).await;

    containerproxy::web::security::found(&request.url)
}

/// `GET /login/oauth2/code/shinyproxy` — the provider sends the user back here.
pub async fn callback(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(backend) = openid_backend(&state) else {
        return auth_error(&state);
    };

    let mut data = SessionData::load(&session).await;
    let Some(request) = data.oidc_request.take() else {
        tracing::warn!("OpenID Connect callback without a request in the session");
        return auth_error(&state);
    };

    if let Some(error) = query.get("error") {
        tracing::error!(
            "OpenID Connect provider reported an error: {error} ({})",
            query.get("error_description").cloned().unwrap_or_default()
        );
        return auth_error(&state);
    }

    // the state must be the one of the request, otherwise this is not our flow
    if query.get("state").map(String::as_str) != Some(request.state.as_str()) {
        tracing::error!("OpenID Connect callback with an invalid state");
        return auth_error(&state);
    }

    let Some(code) = query.get("code") else {
        tracing::error!("OpenID Connect callback without an authorization code");
        return auth_error(&state);
    };

    let tokens = match backend
        .exchange_code(code, &request.redirect_uri, request.verifier.as_deref())
        .await
    {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::error!("OpenID Connect token exchange failed: {error}");
            return auth_error(&state);
        }
    };

    let Some(id_token) = tokens.id_token.clone() else {
        tracing::error!("the token endpoint did not return an id token");
        return auth_error(&state);
    };

    let id_claims = match backend
        .id_token_claims(&id_token, Some(&request.nonce))
        .await
    {
        Ok(claims) => claims,
        Err(error) => {
            tracing::error!("OpenID Connect id token is not usable: {error}");
            return auth_error(&state);
        }
    };

    // the user info endpoint may add claims (and groups)
    let userinfo = match tokens.access_token.as_deref() {
        Some(access_token) => match backend.userinfo_claims(access_token).await {
            Ok(claims) => claims,
            Err(error) => {
                tracing::warn!("Error while loading user info: {error}");
                Default::default()
            }
        },
        None => Default::default(),
    };

    let mut user = match backend.user(&id_claims, &userinfo) {
        Ok(user) => user,
        Err(error) => {
            tracing::error!("cannot build the user of the OpenID Connect claims: {error}");
            return auth_error(&state);
        }
    };
    // Microsoft Graph replaces the groups of the token (the Java authorities mapper does the same, and
    // warns when the `oid` claim is missing)
    if let Some(fetcher) = &state.ms_graph {
        match id_claims.get("oid").and_then(|value| value.as_str()) {
            Some(object_id) => user.groups = fetcher.groups(object_id).await,
            None => {
                tracing::warn!(
                    "Required claim 'oid' not found, make sure to include the 'profile' scope - \
                     continuing without groups"
                );
                user.groups = Vec::new();
            }
        }
    }

    // the tokens are attributes of the user, so that expressions can use them (as in Java, where
    // `oidcUser.accessToken` exists) and so that apps receive SHINYPROXY_OIDC_ACCESS_TOKEN
    if let Some(access_token) = &tokens.access_token {
        user.attributes
            .insert("accessToken".to_string(), serde_json::json!(access_token));
    }
    if let Some(refresh_token) = &tokens.refresh_token {
        user.attributes
            .insert("refreshToken".to_string(), serde_json::json!(refresh_token));
    }

    tracing::info!("User logged in [user: {}]", user.id);
    state
        .proxies
        .events()
        .publish(containerproxy::events::Event::UserLoggedIn {
            user_id: user.id.clone(),
        });

    // a new session id after logging in prevents session fixation
    let _ = session.cycle_id().await;
    let target = data
        .auth_success_url
        .clone()
        .unwrap_or_else(|| state.context_path_with_slash().to_string());
    data.user = Some(user);
    data.auth_success_url = None;
    data.user_initiated_logout = false;
    data.oidc_tokens = Some(OidcTokens {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens
            .expires_in
            .map(|seconds| containerproxy::model::proxy::now_millis() + seconds.max(0) * 1000),
    });
    data.store(&session).await;

    containerproxy::web::security::found(&format!(
        "{}auth-success?continue={}",
        state.context_path_with_slash(),
        super::router::urlencode(&target)
    ))
}

/// The OpenID Connect backend, when it is the configured one.
fn openid_backend(state: &AppState) -> Option<&OpenIdAuthenticationBackend> {
    state.openid.as_deref()
}

/// The redirect to the error page, as the Java failure handler does.
fn auth_error(state: &AppState) -> Response {
    containerproxy::web::security::found(&format!("{}auth-error", state.context_path_with_slash()))
}

/// Refreshes the access token of a session when it is about to expire (`OpenIdReAuthorizeFilter`).
///
/// Returns `false` when the session has to be invalidated (the provider refused the refresh token and
/// `proxy.openid.ignore-session-expire` is not set), which logs the user out.
pub async fn refresh_if_needed(
    state: &AppState,
    session: &Session,
    data: &mut SessionData,
) -> bool {
    let Some(backend) = openid_backend(state) else {
        return true;
    };
    let Some(tokens) = data.oidc_tokens.clone() else {
        // the session has no tokens (e.g. it was recovered from Redis without them)
        return state.settings.proxy.openid.ignore_session_expire();
    };

    // 40 seconds of clock skew, like the filter of the Java implementation (60 seconds would refresh a
    // token that is valid for one minute on every request)
    let skew = 40_000;
    let expired = tokens
        .expires_at
        .map(|expires_at| containerproxy::model::proxy::now_millis() + skew >= expires_at)
        .unwrap_or(false);
    if !expired {
        return true;
    }

    let Some(refresh_token) = tokens.refresh_token.clone() else {
        return state.settings.proxy.openid.ignore_session_expire();
    };

    match backend.refresh(&refresh_token).await {
        Ok(refreshed) => {
            let user_id = data
                .user
                .as_ref()
                .map(|user| user.id.clone())
                .unwrap_or_default();
            tracing::debug!("OpenID access token refreshed [user: {user_id}]");
            let access_token = refreshed.access_token.clone().or(tokens.access_token);
            if let (Some(user), Some(token)) = (data.user.as_mut(), access_token.clone()) {
                user.attributes
                    .insert("accessToken".to_string(), serde_json::json!(token));
            }
            data.oidc_tokens = Some(OidcTokens {
                access_token,
                refresh_token: refreshed.refresh_token.or(Some(refresh_token)),
                expires_at: refreshed.expires_in.map(|seconds| {
                    containerproxy::model::proxy::now_millis() + seconds.max(0) * 1000
                }),
            });
            data.store(session).await;
            true
        }
        Err(error) => {
            if state.settings.proxy.openid.ignore_session_expire() {
                tracing::debug!(
                    "OpenID access token expired, internal session stays active ({error})"
                );
                true
            } else {
                tracing::info!(
                    "OpenID access token could not be refreshed, ending the session: {error}"
                );
                false
            }
        }
    }
}

/// `GET /refresh-openid` — the front-end calls this while an app is open, so that the session of a user
/// who is watching an app does not expire (`refreshOpenidEnabled`).
pub async fn refresh_openid() -> Response {
    axum::Json(serde_json::json!({"status": "success"})).into_response()
}
