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

//! Verifies the contract of the test fixture app, which the ShinyProxy integration tests rely on.

use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

async fn start() -> SocketAddr {
    let (addr, _handle) = testapp::spawn(0).await.expect("fixture app starts");
    addr
}

#[tokio::test]
async fn serves_html_with_head_element() {
    let addr = start().await;
    let response = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/html;charset=UTF-8")
    );
    let body = response.text().await.unwrap();
    assert!(body.contains("<head>"), "body was: {body}");
    assert!(body.contains("id=\"app\""), "body was: {body}");
}

#[tokio::test]
async fn exposes_environment_and_headers() {
    let addr = start().await;

    let env: serde_json::Value = reqwest::get(format!("http://{addr}/env"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(env.get("PATH").is_some(), "env was: {env}");

    let client = reqwest::Client::new();
    let headers: serde_json::Value = client
        .get(format!("http://{addr}/headers"))
        .header("X-SP-UserId", "jack")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(headers["x-sp-userid"], "jack");
}

#[tokio::test]
async fn streams_large_responses_and_consumes_uploads() {
    let addr = start().await;

    let body = reqwest::get(format!("http://{addr}/big/1048576"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.len(), 1_048_576);

    let payload = vec![b'a'; 4096];
    let response: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{addr}/upload"))
        .body(payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["bytes"], 4096);
    assert_eq!(
        response["sha256"],
        // sha256 of 4096 'a' bytes, cross-checked with `python3 -c "import hashlib; ..."`
        "c93eee2d0db02f10acc7460d9576e122dcf8cd53c4bf8dfcae1b3e74ebcfff5a"
    );
}

#[tokio::test]
async fn echoes_websocket_messages_and_counts_pings() {
    let addr = start().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("websocket connects");

    socket
        .send(Message::Text("hello".into()))
        .await
        .expect("send text");
    let echoed = socket.next().await.expect("echo").expect("no error");
    assert_eq!(echoed, Message::Text("hello".into()));

    socket
        .send(Message::Ping(Vec::new().into()))
        .await
        .expect("send ping");
    let pong = socket.next().await.expect("pong").expect("no error");
    assert!(matches!(pong, Message::Pong(_)), "got {pong:?}");

    let stats: serde_json::Value = reqwest::get(format!("http://{addr}/stats"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["upgrades"], 1);
    assert_eq!(stats["messages"], 1);
    assert_eq!(stats["pings"], 1);
}

#[tokio::test]
async fn sleeps_and_reports_unknown_routes() {
    let addr = start().await;

    let response = reqwest::get(format!("http://{addr}/sleep/50"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "slept 50ms\n");

    let response = reqwest::get(format!("http://{addr}/nope")).await.unwrap();
    assert_eq!(response.status(), 404);
}
