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

//! Test fixture application, standing in for a real Shiny app.
//!
//! It is used in two ways:
//!
//! * started in-process by integration tests that need a target to proxy to;
//! * started as a child process by the `local` container backend, which allows testing the complete
//!   proxy lifecycle without a container runtime.
//!
//! Routes (see `docs/TESTING.md`):
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /` | HTML page with a `<head>` element (used by the script injection tests) |
//! | `GET /ws` | WebSocket echo endpoint; counts received ping frames |
//! | `GET /env` | JSON map of the process environment |
//! | `GET /headers` | JSON map of the received request headers |
//! | `GET /big/{bytes}` | streamed response of N bytes |
//! | `POST /upload` | consumes the request body, replies with its size and sha256 |
//! | `GET /sleep/{ms}` | delayed response |
//! | `GET /exit/{code}` | terminates the process (crash detection tests) |
//! | `GET /stats` | counters: requests, upgrades, pings, pongs |
//! | `GET /slowheaders` | response whose headers are sent after a delay |
//! | `GET /asset.js` | `application/javascript` response (cache header tests) |

#![forbid(unsafe_code)]

pub mod load;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

/// Counters exposed on `GET /stats`.
#[derive(Debug, Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub upgrades: AtomicU64,
    pub pings: AtomicU64,
    pub pongs: AtomicU64,
    pub messages: AtomicU64,
}

impl Stats {
    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "requests": self.requests.load(Ordering::Relaxed),
            "upgrades": self.upgrades.load(Ordering::Relaxed),
            "pings": self.pings.load(Ordering::Relaxed),
            "pongs": self.pongs.load(Ordering::Relaxed),
            "messages": self.messages.load(Ordering::Relaxed),
        })
    }
}

/// Shared state of the fixture app.
#[derive(Clone, Default)]
pub struct AppState {
    pub stats: Arc<Stats>,
}

/// Builds the fixture router.
pub fn router() -> Router {
    router_with_state(AppState::default())
}

/// Builds the fixture router with a caller provided state (so tests can inspect the counters).
pub fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", any(ws_handler))
        .route("/env", get(env_handler))
        .route("/headers", get(headers_handler))
        .route("/big/{bytes}", get(big_handler))
        .route("/upload", post(upload_handler))
        .route("/sleep/{ms}", get(sleep_handler))
        .route("/exit/{code}", get(exit_handler))
        .route("/stats", get(stats_handler))
        .route("/slowheaders", get(slow_headers_handler))
        .route("/asset.js", get(asset_handler))
        .fallback(any(fallback_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            count_requests,
        ))
        .with_state(state)
}

async fn count_requests(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    state.stats.requests.fetch_add(1, Ordering::Relaxed);
    next.run(request).await
}

/// Serves the fixture app on the given listener until the process ends.
pub async fn serve(listener: TcpListener) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    println!("sp-testapp ready on http://{addr}");
    axum::serve(listener, router()).await?;
    Ok(())
}

/// Binds the fixture app on `127.0.0.1:port` (port `0` selects a free port) and spawns it.
///
/// Returns the bound address and the join handle of the server task.
pub async fn spawn(port: u16) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router()).await {
            eprintln!("sp-testapp stopped: {error}");
        }
    });
    Ok((addr, handle))
}

async fn index() -> Response {
    let body = concat!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n",
        "    <meta charset=\"utf-8\">\n    <title>sp-testapp</title>\n</head>\n<body>\n",
        "    <div id=\"app\">sp-testapp</div>\n</body>\n</html>\n"
    );
    ([(header::CONTENT_TYPE, "text/html;charset=UTF-8")], body).into_response()
}

async fn asset_handler() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        "window.spTestApp = true;\n",
    )
        .into_response()
}

async fn env_handler() -> Json<BTreeMap<String, String>> {
    Json(std::env::vars().collect())
}

async fn headers_handler(headers: HeaderMap) -> Json<BTreeMap<String, String>> {
    let map = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    Json(map)
}

async fn big_handler(Path(bytes): Path<usize>) -> Response {
    const CHUNK: usize = 64 * 1024;
    let mut remaining = bytes;
    let stream = futures::stream::poll_fn(move |_| {
        if remaining == 0 {
            return std::task::Poll::Ready(None);
        }
        let len = remaining.min(CHUNK);
        remaining -= len;
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(bytes::Bytes::from(vec![
            b'x';
            len
        ]))))
    });
    (
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn upload_handler(request: Request) -> Response {
    let mut stream = request.into_body().into_data_stream();
    let mut hasher = Sha256::new();
    let mut total: usize = 0;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                total += chunk.len();
                hasher.update(&chunk);
            }
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": error.to_string()})),
                )
                    .into_response()
            }
        }
    }
    Json(serde_json::json!({
        "bytes": total,
        "sha256": hex::encode(hasher.finalize()),
    }))
    .into_response()
}

async fn sleep_handler(Path(ms): Path<u64>) -> Response {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    format!("slept {ms}ms\n").into_response()
}

async fn exit_handler(Path(code): Path<i32>) -> Response {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::process::exit(code);
    });
    (StatusCode::OK, format!("exiting with {code}\n")).into_response()
}

async fn stats_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.stats.snapshot())
}

async fn slow_headers_handler() -> Response {
    tokio::time::sleep(Duration::from_millis(500)).await;
    "slow headers\n".into_response()
}

async fn fallback_handler(request: Request) -> Response {
    (
        StatusCode::NOT_FOUND,
        format!(
            "no route for {} {}\n",
            request.method(),
            request.uri().path()
        ),
    )
        .into_response()
}

async fn ws_handler(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    state.stats.upgrades.fetch_add(1, Ordering::Relaxed);
    upgrade.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                state.stats.messages.fetch_add(1, Ordering::Relaxed);
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            Message::Binary(data) => {
                state.stats.messages.fetch_add(1, Ordering::Relaxed);
                if sender.send(Message::Binary(data)).await.is_err() {
                    break;
                }
            }
            Message::Ping(_) => {
                // Pong replies are sent automatically by the WebSocket implementation.
                state.stats.pings.fetch_add(1, Ordering::Relaxed);
            }
            Message::Pong(_) => {
                state.stats.pongs.fetch_add(1, Ordering::Relaxed);
            }
            Message::Close(_) => break,
        }
    }
}
