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

//! The actuator endpoints and the metrics.
//!
//! Replaces the Java `TestActuatorEndpoints` and the Micrometer part of the usage statistics tests.

mod common;

use std::time::Duration;

use common::TestInstance;

const CONFIG: &str = r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  usage-stats-micrometer-prefix: shinyproxy
  docker:
    port-range-start: 28000
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
    - id: 02_other
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##;

#[tokio::test]
async fn health_endpoints_are_public() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    for path in [
        "/actuator/health",
        "/actuator/health/liveness",
        "/actuator/health/readiness",
    ] {
        let response = client
            .get(instance.url(path))
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 200, "{path}");
        let json: serde_json::Value = response.json().await.expect("json");
        assert_eq!(json["status"], "UP", "{path}");
    }

    // the readiness probe names the app recovery component, like the Java health indicator
    let json: serde_json::Value = client
        .get(instance.url("/actuator/health/readiness"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(json["components"]["appRecovery"]["status"], "UP");

    instance.stop();
}

#[tokio::test]
async fn recyclable_reports_activity() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    let json: serde_json::Value = client
        .get(instance.url("/actuator/recyclable"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(json["isRecyclable"], true);
    assert_eq!(json["activeConnections"], 0);

    // a WebSocket connection to an app makes the server unrecyclable
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
    let status: serde_json::Value = jack
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

    let host = instance.base_url.trim_start_matches("http://").to_string();
    let cookie = instance.session_cookie(&jack).expect("session cookie");
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
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket connects");

    let mut connections = 0;
    for _ in 0..25 {
        let json: serde_json::Value = client
            .get(instance.url("/actuator/recyclable"))
            .send()
            .await
            .expect("request")
            .json()
            .await
            .expect("json");
        connections = json["activeConnections"].as_i64().unwrap_or_default();
        if connections > 0 {
            assert_eq!(json["isRecyclable"], false);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(connections, 1, "the open tunnel must be counted");

    // closing it makes the server recyclable again
    // dropping the socket closes the tunnel
    drop(socket);
    for _ in 0..25 {
        let json: serde_json::Value = client
            .get(instance.url("/actuator/recyclable"))
            .send()
            .await
            .expect("request")
            .json()
            .await
            .expect("json");
        if json["activeConnections"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let json: serde_json::Value = client
        .get(instance.url("/actuator/recyclable"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(json["activeConnections"], 0, "{json}");

    instance.stop();
}

#[tokio::test]
async fn prometheus_exposes_the_metrics_of_a_start_and_stop() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    // the metrics of every app definition exist before an app started
    let body = client
        .get(instance.url("/actuator/prometheus"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("# TYPE shinyproxy_appStarts_total counter"),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "shinyproxy_appStarts_total{{shinyproxy_instance=\"{}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 0",
            instance.state.identifiers.instance_id
        )),
        "{body}"
    );
    assert!(body.contains("spec_id=\"02_other\""), "{body}");

    // start an app, which increments the counters and sets the gauges
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
    let status: serde_json::Value = jack
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

    let mut body = String::new();
    for _ in 0..25 {
        body = client
            .get(instance.url("/actuator/prometheus"))
            .send()
            .await
            .expect("request")
            .text()
            .await
            .expect("body");
        if body.contains("spec_id=\"01_hello\"} 1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(
        body.contains(&format!(
            "shinyproxy_appStarts_total{{shinyproxy_instance=\"{}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 1",
            instance.state.identifiers.instance_id
        )),
        "{body}"
    );
    assert!(
        body.contains("shinyproxy_absolute_apps_running{")
            && body.contains("spec_id=\"01_hello\"} 1"),
        "the running app is counted: {body}"
    );
    assert!(
        body.contains("shinyproxy_startupTime_seconds_count{"),
        "the startup time is recorded: {body}"
    );
    assert!(
        body.contains("shinyproxy_appInfo{")
            && body.contains("proxy_id=\"")
            && body.contains("} 10"),
        "the appInfo gauge of the running app has value 10: {body}"
    );
    assert!(body.contains("shinyproxy_userLogins_total"), "{body}");

    // stopping the app increments appStops and records the usage time
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    for _ in 0..25 {
        body = client
            .get(instance.url("/actuator/prometheus"))
            .send()
            .await
            .expect("request")
            .text()
            .await
            .expect("body");
        if body.contains("shinyproxy_appStops_total") && body.contains("spec_id=\"01_hello\"} 1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        body.contains(&format!(
            "shinyproxy_appStops_total{{shinyproxy_instance=\"{}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 1",
            instance.state.identifiers.instance_id
        )),
        "{body}"
    );
    assert!(
        body.contains("shinyproxy_usageTime_seconds_count{")
            && body.contains("spec_id=\"01_hello\"} 1"),
        "the usage time is recorded: {body}"
    );
    assert!(
        body.contains("shinyproxy_appInfo{") && body.contains("} 40"),
        "the appInfo gauge of the stopped app has value 40: {body}"
    );

    instance.stop();
}

#[tokio::test]
async fn the_management_server_serves_the_same_endpoints() {
    let instance = TestInstance::start(CONFIG).await;

    // the management router is served on its own port in production; here it is exercised directly
    let router = shinyproxy::web::management::router(instance.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.ok();
    });

    let client = reqwest::Client::new();
    let json: serde_json::Value = client
        .get(format!("http://{address}/actuator"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(json["_links"]["health"]["href"].is_string(), "{json}");
    assert!(json["_links"]["prometheus"]["href"].is_string(), "{json}");

    let json: serde_json::Value = client
        .get(format!("http://{address}/actuator/info"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(json["build"]["name"], "shinyproxy");
    assert_eq!(
        json["shinyproxy"]["instanceId"],
        instance.state.identifiers.instance_id.as_str()
    );

    let body = client
        .get(format!("http://{address}/actuator/prometheus"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("shinyproxy_appStarts_total"), "{body}");

    server.abort();
    instance.stop();
}
