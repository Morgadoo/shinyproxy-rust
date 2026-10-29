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
use tower_sessions::{Expiry, SessionManagerLayer};

use super::state::AppState;

/// Builds the application including the session layer.
pub fn build(state: Arc<AppState>) -> Router {
    let secure_cookies = state.settings.server.secure_cookies();
    let same_site = session::same_site(state.settings.proxy.same_site_cookie());
    let cookie_name = session::cookie_name(state.settings.spring.session.is_redis()).to_string();
    let browser_cookie_name = cookie_name.clone();
    let context_path = state.context_path_with_slash().to_string();

    // sessions live in Redis when Spring Session is configured that way, so that the servers of a realm
    // share them (`spring.session.store-type: redis`), and in memory otherwise
    let expiry = Expiry::OnInactivity(
        time::Duration::try_from(state.settings.session_timeout())
            .unwrap_or(time::Duration::minutes(30)),
    );

    let router = match state.session_store.clone() {
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
            let session_layer = SessionManagerLayer::new(session::MemorySessionStore::default())
                .with_name(cookie_name)
                .with_path(context_path)
                .with_http_only(true)
                .with_secure(secure_cookies)
                .with_same_site(same_site)
                .with_expiry(expiry);
            super::router::router(state).layer(session_layer)
        }
    };

    // the session cookie of the Java implementation is a browser session cookie: it has no `Max-Age`, so it
    // disappears when the browser is closed, while the session itself expires server side after
    // `spring.session.timeout`. `tower-sessions` derives the cookie lifetime from that timeout, so the
    // attribute is removed again here (the cookie that *clears* the session keeps its `Max-Age=0`).
    router.layer(axum::middleware::from_fn_with_state(
        browser_cookie_name,
        browser_session_cookie,
    ))
}

/// The application with the actuator endpoints next to it, for a `management.server.port` that is the
/// port of the application: Spring Boot then serves the actuator on that port, below the context path.
pub fn with_actuator(app: Router, state: Arc<AppState>) -> Router {
    let context = state.context_path().to_string();
    super::management::router_at(state, &context).merge(app)
}

/// How long a client may take to send the headers of a request, and how long a keep-alive connection may
/// sit idle before the next request (hyper's default, which only applies when a timer is configured).
pub const HEADER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Serves `router` on `listener` until `shutdown` completes, then waits for the open connections.
///
/// This is `axum::serve` with one difference: the connections get a timer, so that hyper enforces
/// [`HEADER_READ_TIMEOUT`]. `axum::serve` builds them without one, which silently disables the timeout: a
/// client that never finishes its headers (slowloris) or keeps an idle connection open holds a socket and
/// a task forever, and enough of them exhaust the file descriptors of the server.
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;

    // dropping the receiver tells the connections to shut down; the senders tell when they are done
    let (signal_tx, signal_rx) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        shutdown.await;
        drop(signal_rx);
    });
    let (close_tx, close_rx) = tokio::sync::watch::channel(());

    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    // e.g. too many open files: back off instead of spinning
                    tracing::debug!("cannot accept a connection: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = signal_tx.closed() => break,
        };

        let service = TowerToHyperService::new(router.clone());
        let signal_tx = signal_tx.clone();
        let close_rx = close_rx.clone();
        tokio::spawn(async move {
            let mut builder = Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT);
            builder
                .http2()
                .timer(TokioTimer::new())
                .enable_connect_protocol();
            let mut connection = std::pin::pin!(
                builder.serve_connection_with_upgrades(TokioIo::new(stream), service)
            );
            let mut signal_closed = std::pin::pin!(signal_tx.closed());
            loop {
                tokio::select! {
                    result = connection.as_mut() => {
                        if let Err(error) = result {
                            tracing::trace!("connection ended: {error}");
                        }
                        break;
                    }
                    _ = &mut signal_closed => connection.as_mut().graceful_shutdown(),
                }
            }
            drop(close_rx);
        });
    }

    drop(close_rx);
    drop(listener);
    close_tx.closed().await;
}

/// Removes `Max-Age`/`Expires` from the session cookie, so that it lives as long as the browser session.
///
/// Only the cookie called `cookie_name` is rewritten: the cookies of the apps pass through the same
/// layer and keep the lifetime the app gave them.
async fn browser_session_cookie(
    axum::extract::State(cookie_name): axum::extract::State<String>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let cookies: Vec<axum::http::HeaderValue> = headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .cloned()
        .collect();
    if cookies.is_empty() {
        return response;
    }

    let rewritten: Vec<axum::http::HeaderValue> = cookies
        .into_iter()
        .map(|value| {
            let Ok(text) = value.to_str() else {
                return value;
            };
            let is_session_cookie = text
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == cookie_name);
            // a cookie that clears the session says `Max-Age=0`; that one is left alone
            if !is_session_cookie || text.contains("Max-Age=0") {
                return value;
            }
            let kept: Vec<&str> = text
                .split(';')
                .map(str::trim)
                .filter(|attribute| {
                    !attribute.starts_with("Max-Age=") && !attribute.starts_with("Expires=")
                })
                .collect();
            axum::http::HeaderValue::from_str(&kept.join("; ")).unwrap_or(value)
        })
        .collect();

    headers.remove(axum::http::header::SET_COOKIE);
    for value in rewritten {
        headers.append(axum::http::header::SET_COOKIE, value);
    }
    response
}
