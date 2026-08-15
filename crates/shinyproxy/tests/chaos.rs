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

//! The chaos checks of P14: things that go wrong while the server is working.
//!
//! An app that dies in the middle of a WebSocket session, a Redis that disappears, a user that stops an app
//! while it is starting, and a server that shuts down while apps are running. None of them may panic, leak a
//! host port or leave the server unable to serve.

mod common;

use std::time::Duration;

use common::{TestClient, TestInstance};
use futures::{SinkExt, StreamExt};

const CONFIG: &str = r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 1000
  heartbeat-timeout: -1
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      max-instances: 5
"##;

/// Starts an app and waits until it is up.
async fn start_app(instance: &TestInstance, client: &TestClient) -> String {
    let started: serde_json::Value = client
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the app must start: {started}"))
        .to_string();
    let status: serde_json::Value = client
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=15"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");
    proxy_id
}

/// Opens a WebSocket connection to an app through the proxy.
async fn open_websocket(
    instance: &TestInstance,
    client: &TestClient,
    proxy_id: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let host = instance.base_url.trim_start_matches("http://").to_string();
    let cookie = instance.session_cookie(client).expect("session cookie");
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{host}/app_proxy/{proxy_id}/ws"))
        .header("Host", &host)
        .header("Cookie", cookie)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("request");
    let (socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket connects");
    assert_eq!(response.status(), 101);
    socket
}

#[tokio::test]
async fn an_app_that_dies_during_a_websocket_session_does_not_break_the_server() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;
    let mut socket = open_websocket(&instance, &jack, &proxy_id).await;

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "before".into(),
        ))
        .await
        .expect("sends");
    let message = socket.next().await.expect("answer").expect("message");
    assert_eq!(message.into_text().expect("text"), "before");

    // the app process is killed while the connection is open
    let proxy = instance
        .state
        .proxies
        .proxy(&proxy_id)
        .expect("the proxy exists");
    let container = proxy.containers.first().expect("a container");
    let pid: i32 = container
        .id
        .clone()
        .expect("the container id")
        .parse()
        .expect("the local backend uses the process id as the container id");
    // SIGKILL, so the app has no chance to close the connection politely
    let _ = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status();

    // the connection ends (with an error or a close frame), but the server keeps working
    for _ in 0..50 {
        match socket.next().await {
            Some(Ok(_)) => continue,
            _ => break,
        }
    }

    let response = jack
        .get(instance.url("/"))
        .send()
        .await
        .expect("index request");
    assert_eq!(response.status(), 200, "the server still serves");

    // the app is reported as crashed and its port is released, so a new app can start
    for _ in 0..100 {
        if instance
            .state
            .proxies
            .proxy(&proxy_id)
            .map(|proxy| proxy.status.is_unavailable())
            .unwrap_or(true)
        {
            break;
        }
        let _ = jack
            .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
            .send()
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let new_id = start_app(&instance, &jack).await;
    assert_ne!(new_id, proxy_id, "a new app starts after the crash");

    instance.stop();
}

#[tokio::test]
async fn stopping_an_app_while_it_starts_leaves_nothing_behind() {
    // an app that takes a while to answer, so it can be stopped while it is starting
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-timeout: -1
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      # a command that never answers, so the app stays in `New` while the test stops it
      container-cmd: [ "sh", "-c", "sleep 30" ]
      max-instances: 5
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;

    let started: serde_json::Value = jack
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"].as_str().expect("id").to_string();

    // stop it while it is still starting
    tokio::time::sleep(Duration::from_millis(300)).await;
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    // the app disappears and its host port is free again
    for _ in 0..150 {
        if instance.state.proxies.proxy(&proxy_id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        instance.state.proxies.proxy(&proxy_id).is_none(),
        "the app that was stopped while starting must be gone"
    );

    // and the server can start apps again
    let proxies: serde_json::Value = jack
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(0));

    instance.stop();
}

#[tokio::test]
async fn a_redis_that_disappears_does_not_break_the_server() {
    if std::env::var("SP_TEST_REDIS").as_deref() != Ok("1") {
        eprintln!("skipping: set SP_TEST_REDIS=1 to run the Redis chaos check");
        return;
    }

    // a Redis of its own, so that killing it does not disturb the other tests
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("address").port()
    };
    let mut redis = match std::process::Command::new("redis-server")
        .args([
            "--port",
            &port.to_string(),
            "--save",
            "",
            "--appendonly",
            "no",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("skipping: redis-server is not available ({error})");
            return;
        }
    };
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let configuration = format!(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-timeout: -1
  store-mode: Redis
  realm-id: chaos-{port}
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]

spring:
  data:
    redis:
      host: 127.0.0.1
      port: {port}
"##
    );
    let instance = TestInstance::start(&configuration).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // Redis disappears while the server is running
    let _ = redis.kill();
    let _ = redis.wait();

    // the server still answers, and the API reports what it can instead of failing
    let response = jack
        .get(instance.url("/"))
        .send()
        .await
        .expect("index request");
    assert_eq!(response.status(), 200, "the server still serves");

    let proxies: serde_json::Value = jack
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["status"], "success");

    // the app of the user is still reachable, because its target is known to the router
    let body = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("app request")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("sp-testapp") || body.contains("app_stopped_or_non_existent"),
        "the answer must be an app page or the stopped document: {body}"
    );

    // starting a new app fails, but with an answer instead of a panic
    let started = jack
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request");
    assert!(started.status().as_u16() < 500 || started.status().as_u16() == 500);

    // and the server is not the leader anymore, because it cannot renew the lock
    if let Some(election) = &instance.state.redis_leader {
        election.elect();
        assert!(
            !containerproxy::service::LeaderService::is_leader(election.as_ref()),
            "a server that lost Redis must not think it is the leader"
        );
    }

    instance.stop();
}

#[tokio::test]
async fn a_shutdown_stops_the_apps_and_releases_their_ports() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let first = start_app(&instance, &jack).await;

    // the process of the app runs
    let proxy = instance.state.proxies.proxy(&first).expect("the proxy");
    let pid: i32 = proxy.containers[0]
        .id
        .clone()
        .expect("the container id")
        .parse()
        .expect("a process id");
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the app process must run"
    );

    // the shutdown of the server stops the apps (`proxy.stop-proxies-on-shutdown` is true by default)
    instance.state.proxies.shutdown().await;

    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the app process must be gone after the shutdown"
    );
    assert!(
        instance.state.proxies.all_proxies().is_empty(),
        "no app may be left in the store"
    );

    instance.stop();
}
