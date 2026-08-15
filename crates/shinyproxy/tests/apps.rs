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

//! The complete app flow through the HTTP API: start an app, wait for it, open its page, use it
//! through the proxy (including WebSockets), read heartbeat and admin information, and stop it.
//!
//! This is what the Java integration tests `AppControllerTest`, `ProxyApiControllerTest`,
//! `HeartbeatControllerTest` and `AdminControllerTest` cover with a Docker daemon; here the apps run as
//! local processes.

mod common;

use std::time::Duration;

use common::TestInstance;
use futures::{SinkExt, StreamExt};

const CONFIG: &str = r##"
proxy:
  title: Test Proxy
  authentication: simple
  admin-groups: admins
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 300
  docker:
    port-range-start: 23000
  users:
    - name: jack
      password: password
      groups: scientists
    - name: root
      password: rootpw
      groups: admins
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      container-env:
        MY_VAR: "#{userId}"
      access-groups: [ scientists, admins ]
"##;

/// Starts an app and waits until it is up.
async fn start_app(instance: &TestInstance, client: &common::TestClient) -> serde_json::Value {
    let response = client
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({"timezone": "Europe/Brussels"}))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 200);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["status"], "success");
    assert_eq!(json["data"]["status"], "New");
    let proxy_id = json["data"]["id"].as_str().expect("proxy id").to_string();

    // the front-end waits for the status to change with a long poll
    let response: serde_json::Value = client
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=15"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(response["status"], "success");
    assert_eq!(
        response["data"]["status"], "Up",
        "the app must be up: {response}"
    );
    response["data"].clone()
}

#[tokio::test]
async fn starts_an_app_and_proxies_requests_to_it() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;

    let proxy = start_app(&instance, &client).await;
    let proxy_id = proxy["id"].as_str().expect("id").to_string();
    assert_eq!(proxy["specId"], "01_hello");
    assert_eq!(proxy["userId"], "jack");
    assert_eq!(proxy["displayName"], "Hello Application");
    assert_eq!(proxy["runtimeValues"]["SHINYPROXY_APP_INSTANCE"], "_");
    assert_eq!(
        proxy["runtimeValues"]["SHINYPROXY_PUBLIC_PATH"],
        format!("/app_proxy/{proxy_id}/")
    );
    assert_eq!(
        proxy["runtimeValues"]["SHINYPROXY_USER_TIMEZONE"],
        "Europe/Brussels"
    );

    // the app page embeds the app and hands the proxy to the front-end
    let body = client
        .get(instance.url("/app/01_hello"))
        .send()
        .await
        .expect("app page")
        .text()
        .await
        .expect("body");
    assert!(body.contains("<div id=\"iframeinsert\"></div>"), "{body}");
    assert!(body.contains("window.Shiny.app.start("), "{body}");
    assert!(
        body.contains(&proxy_id),
        "the page contains the proxy: {body}"
    );
    assert!(
        body.contains("Launching <span>Hello Application</span>"),
        "{body}"
    );

    // requests to /app_proxy are proxied to the app, and the iframe script is injected
    let response = client
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .header("Accept", "text/html")
        .header("Sec-Fetch-Mode", "navigate")
        .send()
        .await
        .expect("proxy request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(body.contains("sp-testapp"), "{body}");
    assert!(
        body.contains("js/shiny.iframe.js"),
        "the iframe script is injected: {body}"
    );

    // the app receives the ShinyProxy headers and environment
    let headers: serde_json::Value = client
        .get(instance.url(&format!("/app_proxy/{proxy_id}/headers")))
        .send()
        .await
        .expect("proxy request")
        .json()
        .await
        .expect("json");
    assert_eq!(headers["x-sp-userid"], "jack");
    assert_eq!(headers["x-sp-usergroups"], "SCIENTISTS");

    let environment: serde_json::Value = client
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("proxy request")
        .json()
        .await
        .expect("json");
    assert_eq!(environment["SHINYPROXY_USERNAME"], "jack");
    assert_eq!(environment["MY_VAR"], "jack", "expressions are resolved");

    // ... and the api lists the proxy
    let proxies: serde_json::Value = client
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(proxies["data"][0]["id"], proxy_id);

    // stopping the app through the api
    let response = client
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    // the status endpoint reports the app as stopped, and the proxy route answers with the Java body
    let mut stopped = false;
    for _ in 0..50 {
        let status: serde_json::Value = client
            .get(instance.url(&format!("/api/proxy/{proxy_id}/status")))
            .send()
            .await
            .expect("status request")
            .json()
            .await
            .expect("json");
        if status["data"]["status"] == "Stopped" {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(stopped, "the app must be stopped");

    let response = client
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("proxy request");
    assert_eq!(response.status(), 410);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "app_stopped_or_non_existent");

    instance.stop();
}

#[tokio::test]
async fn websockets_of_the_app_are_tunnelled_and_keep_the_app_alive() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;
    let proxy = start_app(&instance, &client).await;
    let proxy_id = proxy["id"].as_str().expect("id").to_string();

    // the session cookie is needed for the WebSocket handshake
    let cookie = instance.session_cookie(&client).expect("session cookie");
    let url = format!(
        "ws://{}/app_proxy/{proxy_id}/ws",
        instance.base_url.trim_start_matches("http://")
    );
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", instance.base_url.trim_start_matches("http://"))
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

    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket connects through ShinyProxy");
    assert_eq!(response.status(), 101);

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "ping-pong".into(),
        ))
        .await
        .expect("send");
    let echoed = socket.next().await.expect("echo").expect("no error");
    assert_eq!(
        echoed,
        tokio_tungstenite::tungstenite::Message::Text("ping-pong".into())
    );

    // the heartbeat endpoint reports activity (the heartbeat rate of the test config is 300ms)
    tokio::time::sleep(Duration::from_millis(700)).await;
    let info: serde_json::Value = client
        .get(instance.url(&format!("/heartbeat/{proxy_id}")))
        .send()
        .await
        .expect("heartbeat request")
        .json()
        .await
        .expect("json");
    assert_eq!(info["status"], "success");
    assert_eq!(info["data"]["heartbeatRate"], 300);
    assert!(
        info["data"]["lastHeartbeat"].as_i64().unwrap_or(0) > 0,
        "the websocket traffic must produce heartbeats: {info}"
    );

    socket.close(None).await.ok();
    instance.stop();
}

#[tokio::test]
async fn apps_are_private_to_their_owner() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let proxy = start_app(&instance, &jack).await;
    let proxy_id = proxy["id"].as_str().expect("id").to_string();

    // another user cannot see or use the app
    let root = instance.login("root", "rootpw").await;
    let proxies: serde_json::Value = root
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(0));

    let response = root
        .get(instance.url(&format!("/api/proxy/{proxy_id}")))
        .send()
        .await
        .expect("api request");
    assert_eq!(response.status(), 403);

    let response = root
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("proxy request");
    assert_eq!(response.status(), 410);

    // ... but an administrator sees it on the admin page and may stop it
    let admin_data: serde_json::Value = root
        .get(instance.url("/admin/data"))
        .send()
        .await
        .expect("admin request")
        .json()
        .await
        .expect("json");
    assert_eq!(admin_data["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(admin_data["data"][0]["proxyId"], proxy_id);
    assert_eq!(admin_data["data"][0]["userId"], "jack");
    assert_eq!(admin_data["data"][0]["appName"], "01_hello");
    assert_eq!(admin_data["data"][0]["instanceName"], "Default");
    assert_eq!(admin_data["data"][0]["status"], "Up");

    let response = root
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(
        response.status(),
        200,
        "admins may stop apps of other users"
    );

    instance.stop();
}

#[tokio::test]
async fn instance_names_and_limits_are_validated() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;

    // invalid instance name
    let response = client
        .post(instance.url("/app_i/01_hello/invalid name"))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "Invalid app instance name");

    // unknown app
    let response = client
        .post(instance.url("/app_i/does-not-exist/_"))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 403);

    // the default limit is one instance per app and user
    start_app(&instance, &client).await;
    let response = client
        .post(instance.url("/app_i/01_hello/second"))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert!(
        json["data"]
            .as_str()
            .unwrap_or_default()
            .contains("maximum number of instances (1)"),
        "{json}"
    );

    instance.stop();
}

#[tokio::test]
async fn the_api_reports_specs_and_unknown_proxies() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;

    let specs: serde_json::Value = client
        .get(instance.url("/api/proxyspec"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(specs["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(specs["data"][0]["id"], "01_hello");
    assert_eq!(specs["data"][0]["displayName"], "Hello Application");
    // spec details are hidden by default
    assert!(specs["data"][0].get("containerSpecs").is_none(), "{specs}");

    // an unknown proxy is reported as stopped, as in the Java implementation
    let status: serde_json::Value = client
        .get(instance.url("/api/proxy/does-not-exist/status"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["status"], "success");
    assert_eq!(status["data"]["status"], "Stopped");
    assert_eq!(status["data"]["id"], "does-not-exist");

    // the watch timeout is validated
    let status: serde_json::Value = client
        .get(instance.url("/api/proxy/does-not-exist/status?watch=true&timeout=5"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    // unknown proxies answer immediately, so the timeout is not validated for them
    assert_eq!(status["data"]["status"], "Stopped");

    instance.stop();
}
