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

//! Assembles the HTTP server: routes plus the session layer.

use std::sync::Arc;

use axum::Router;
use containerproxy::web::session;
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};

use super::state::AppState;

/// Builds the application including the session layer.
pub fn build(state: Arc<AppState>) -> Router {
    let secure_cookies = state.settings.server.secure_cookies();
    let same_site = session::same_site(state.settings.proxy.same_site_cookie());
    let cookie_name = session::cookie_name(state.settings.spring.session.is_redis()).to_string();
    let context_path = state.context_path_with_slash();

    // sessions live in Redis when Spring Session is configured that way, so that the servers of a realm
    // share them (`spring.session.store-type: redis`), and in memory otherwise
    let expiry = Expiry::OnInactivity(
        time::Duration::try_from(state.settings.spring.session.timeout_duration())
            .unwrap_or(time::Duration::minutes(30)),
    );

    match state.session_store.clone() {
        Some(store) => {
            let session_layer = SessionManagerLayer::new(store)
                .with_name(cookie_name)
                .with_path(context_path)
                .with_http_only(true)
                .with_secure(secure_cookies)
                .with_same_site(same_site)
                .with_expiry(expiry);
            super::router::router(state).layer(session_layer)
        }
        None => {
            let session_layer = SessionManagerLayer::new(MemoryStore::default())
                .with_name(cookie_name)
                .with_path(context_path)
                .with_http_only(true)
                .with_secure(secure_cookies)
                .with_same_site(same_site)
                .with_expiry(expiry);
            super::router::router(state).layer(session_layer)
        }
    }
}
