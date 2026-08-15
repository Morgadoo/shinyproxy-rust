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

//! End-to-end tests of the data plane: a minimal proxy server in front of the `sp-testapp` fixture.
//!
//! These cover the behaviour that Shiny apps depend on: streamed bodies, forwarded headers, WebSocket
//! tunnelling with heartbeat pings, cache headers, the injected iframe script and the error bodies of a
//! crashed app.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use containerproxy::dataplane::cache_headers;
use containerproxy::dataplane::http::{app_unavailable_response, ForwardOptions};
use containerproxy::dataplane::inject::ScriptInjector;
use containerproxy::dataplane::ws::{proxy_upgrade, TunnelObserver};
use containerproxy::model::spec::CacheHeadersMode;
use futures::StreamExt;

/// Counts the heartbeats the tunnel reports.
#[derive(Debug, Default)]
struct Heartbeats {
    count: AtomicUsize,
}

impl TunnelObserver for Heartbeats {
    fn heartbeat(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct ProxyState {
    app: String,
    heartbeats: Arc<Heartbeats>,
    cache_mode: CacheHeadersMode,
    inject_script: bool,
    heartbeat_rate: Duration,
}

/// Starts the fixture app and a proxy server in front of it.
async fn start(
    cache_mode: CacheHeadersMode,
    inject_script: bool,
    heartbeat_rate: Duration,
) -> Fixture {
    let (app_address, app_handle) = testapp::spawn(0).await.expect("fixture app");
    let heartbeats = Arc::new(Heartbeats::default());

    let state = ProxyState {
        app: format!("http://{app_address}"),
        heartbeats: heartbeats.clone(),
        cache_mode,
        inject_script,
        heartbeat_rate,
    };

    let router = Router::new()
        .route("/", any(handler))
        .route("/{*path}", any(handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let proxy_address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Fixture {
        proxy: proxy_address,
        app: app_address,
        heartbeats,
        app_handle,
    }
}

/// A running fixture app with a proxy in front of it.
struct Fixture {
    proxy: SocketAddr,
    app: SocketAddr,
    heartbeats: Arc<Heartbeats>,
    app_handle: tokio::task::JoinHandle<()>,
}

impl Fixture {
    /// Stops the app, so that the proxy cannot reach it any more (as if the container crashed).
    fn stop_app(&self) {
        self.app_handle.abort();
    }
}

/// The proxy handler: exactly the pieces a ShinyProxy request handler wires together.
async fn handler(State(state): State<ProxyState>, request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/').to_string();
    let query = request.uri().query().map(str::to_string);
    let method = request.method().clone();

    let url = match &query {
        Some(query) => format!("{}/{path}?{query}", state.app),
        None => format!("{}/{path}", state.app),
    };

    let mut options = ForwardOptions {
        extra_headers: std::sync::Arc::new(BTreeMap::from([
            ("X-SP-UserId".to_string(), "jack".to_string()),
            ("X-SP-UserGroups".to_string(), "SCIENTISTS".to_string()),
        ])),
        force_identity_encoding: false,
    };
    if state.inject_script {
        options.force_identity_encoding = true;
    }

    let response = proxy_upgrade(
        request,
        &url,
        &options,
        state.heartbeat_rate,
        state.heartbeats.clone(),
    )
    .await;

    let mut response = match response {
        Ok(response) => response,
        // the app is gone: this is what ShinyProxy answers
        Err(_) => return app_unavailable_response(true).into_response(),
    };

    cache_headers::apply(state.cache_mode, &method, response.headers_mut());

    if state.inject_script
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    {
        let (mut parts, body) = response.into_parts();
        parts.headers.remove(axum::http::header::CONTENT_LENGTH);
        let mut injector = ScriptInjector::new("/instance/js/shiny.iframe.js");
        let stream = body.into_data_stream().map(move |chunk| match chunk {
            Ok(chunk) => Ok(injector.push(&chunk)),
            Err(error) => Err(error),
        });
        // note: the trailing flush of the injector is exercised by its unit tests; here the fixture
        // always contains a <head> element in the first chunk
        return Response::from_parts(parts, Body::from_stream(stream));
    }

    response
}

#[tokio::test]
async fn proxies_requests_and_forwards_headers() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;

    let response = reqwest::get(format!("http://{proxy}/"))
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(body.contains("sp-testapp"), "{body}");

    // the app sees the ShinyProxy headers and the forwarded host
    let headers: BTreeMap<String, String> = reqwest::get(format!("http://{proxy}/headers"))
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(headers.get("x-sp-userid").map(String::as_str), Some("jack"));
    assert_eq!(
        headers.get("x-sp-usergroups").map(String::as_str),
        Some("SCIENTISTS")
    );
    assert_eq!(
        headers.get("x-forwarded-host").map(String::as_str),
        Some(proxy.to_string().as_str()),
        "the forwarded host includes the non-standard port"
    );
    // hop-by-hop headers are not forwarded
    assert!(!headers.contains_key("keep-alive"));
}

#[tokio::test]
async fn streams_large_downloads_and_uploads() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;

    // 8 MB download, consumed chunk by chunk
    let response = reqwest::get(format!("http://{proxy}/big/8388608"))
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    let mut total = 0usize;
    let mut chunks = 0usize;
    while let Some(chunk) = stream.next().await {
        total += chunk.expect("chunk").len();
        chunks += 1;
    }
    assert_eq!(total, 8 * 1024 * 1024);
    assert!(
        chunks > 1,
        "the response must be streamed, got {chunks} chunk(s)"
    );

    // 4 MB upload; the app reports the size and hash it received
    let payload = vec![b'z'; 4 * 1024 * 1024];
    let expected = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        hex::encode(hasher.finalize())
    };
    let response: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{proxy}/upload"))
        .body(payload)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(response["bytes"], 4 * 1024 * 1024);
    assert_eq!(response["sha256"], expected);
}

#[tokio::test]
async fn tunnels_websockets_and_reports_heartbeats() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        false,
        Duration::from_millis(100),
    )
    .await;
    let proxy = fixture.proxy;
    let app = fixture.app;
    let heartbeats = fixture.heartbeats.clone();

    use futures::SinkExt;
    let (mut socket, response) = tokio_tungstenite::connect_async(format!("ws://{proxy}/ws"))
        .await
        .expect("websocket connects through the proxy");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "hello".into(),
        ))
        .await
        .expect("send");
    let echoed = socket.next().await.expect("echo").expect("no error");
    assert_eq!(
        echoed,
        tokio_tungstenite::tungstenite::Message::Text("hello".into())
    );

    // ShinyProxy pings the browser while the connection is idle; tungstenite answers the pings
    // automatically, and those pongs are the heartbeats
    let mut pings = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), socket.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_)))) => pings += 1,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => panic!("websocket error: {error}"),
            Ok(None) => break,
            Err(_) => {}
        }
        if pings >= 2 {
            break;
        }
    }
    assert!(
        pings >= 2,
        "the browser must receive the injected pings, got {pings}"
    );
    assert!(
        heartbeats.count.load(Ordering::SeqCst) >= 2,
        "pongs of the browser must be reported as heartbeats"
    );

    // the app saw the upgrade and the echoed message, but none of the ping/pong traffic
    let stats: serde_json::Value = reqwest::get(format!("http://{app}/stats"))
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(stats["upgrades"], 1);
    assert_eq!(stats["messages"], 1);
    assert_eq!(
        stats["pings"], 0,
        "pings are not forwarded to the app: {stats}"
    );
    assert_eq!(
        stats["pongs"], 0,
        "pongs are not forwarded to the app: {stats}"
    );

    socket.close(None).await.ok();
}

#[tokio::test]
async fn applies_cache_headers_of_the_configured_mode() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;
    let response = reqwest::get(format!("http://{proxy}/asset.js"))
        .await
        .expect("request");
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "no-cache, no-store, max-age=0, must-revalidate"
    );

    let fixture = start(
        CacheHeadersMode::EnforceCacheAssets,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;
    let response = reqwest::get(format!("http://{proxy}/asset.js"))
        .await
        .expect("request");
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "max-age=86400"
    );
    let response = reqwest::get(format!("http://{proxy}/"))
        .await
        .expect("request");
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "no-cache, no-store, max-age=0, must-revalidate",
        "html is never cached"
    );

    let fixture = start(
        CacheHeadersMode::Passthrough,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;
    let response = reqwest::get(format!("http://{proxy}/asset.js"))
        .await
        .expect("request");
    assert!(
        response.headers().get("cache-control").is_none(),
        "the headers of the app are kept"
    );
}

#[tokio::test]
async fn injects_the_iframe_script_into_html_responses() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        true,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;

    let body = reqwest::get(format!("http://{proxy}/"))
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("<script src=\"/instance/js/shiny.iframe.js\"></script>"),
        "{body}"
    );
    assert_eq!(body.matches("shiny.iframe.js").count(), 1, "{body}");
    // the app content itself is unchanged
    assert!(body.contains("<div id=\"app\">sp-testapp</div>"), "{body}");

    // non-html responses are not touched
    let body = reqwest::get(format!("http://{proxy}/asset.js"))
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert_eq!(body, "window.spTestApp = true;\n");
}

#[tokio::test]
async fn returns_the_java_error_body_when_the_app_is_gone() {
    let fixture = start(
        CacheHeadersMode::EnforceNoCache,
        false,
        Duration::from_millis(500),
    )
    .await;
    let proxy = fixture.proxy;

    // the app stops, which makes the proxy target unreachable (as when a container crashes)
    fixture.stop_app();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let response = reqwest::get(format!("http://{proxy}/"))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        HeaderValue::from_static("application/json")
    );
    let body = response.text().await.expect("body");
    assert_eq!(body, "{\"status\":\"fail\",\"data\":\"app_crashed\"}");
}
