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

//! End-to-end tests of the Docker backend, against a real Docker daemon.
//!
//! These tests need a Docker daemon and the `sp-testapp:test` image (`scripts/build-test-image.sh`);
//! they are skipped unless `SP_TEST_DOCKER=1` is set, exactly like the Docker tests of the Java
//! implementation are skipped without a daemon.
//!
//! Run them with:
//!
//! ```sh
//! ./scripts/build-test-image.sh
//! SP_TEST_DOCKER=1 cargo test -p shinyproxy --test docker -- --test-threads=1
//! ```

mod common;

use std::collections::BTreeMap;

use common::TestInstance;

/// The image the tests start (override with `SP_TEST_IMAGE`).
fn image() -> String {
    std::env::var("SP_TEST_IMAGE").unwrap_or_else(|_| "sp-testapp:test".to_string())
}

/// Whether the Docker tests are enabled.
fn enabled() -> bool {
    std::env::var("SP_TEST_DOCKER").as_deref() == Ok("1")
}

/// A configuration with one app that runs in Docker.
///
/// `extra_properties` is inserted into the `proxy` block (two spaces of indentation), `extra_specs` after
/// the app definitions (four spaces).
fn config(port_range_start: u16, extra_properties: &str, extra_specs: &str) -> String {
    format!(
        r##"
proxy:
  title: Docker Test
  authentication: simple
  admin-groups: admins
  container-backend: docker
  container-wait-timeout: 30000
  hide-navbar: false
{extra_properties}  docker:
    port-range-start: {port_range_start}
    image-pull-policy: Never
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
      container-image: {image}
      container-env:
        MY_VARIABLE: my-value
      labels:
        my.own.label: my-label-value
      port: 3838
      access-groups: [ scientists, admins ]
{extra_specs}"##,
        image = image()
    )
}

/// Removes every `sp-container-*` container of the daemon.
///
/// Tests call this before they start, so that containers a previously failed test left behind do not
/// occupy the published ports. Only used in tests, and only for containers this implementation created.
async fn cleanup_all() {
    let output = tokio::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "name=sp-container-"])
        .output()
        .await
        .expect("docker ps runs");
    let ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut command = tokio::process::Command::new("docker");
    command.arg("rm").arg("-f");
    for id in &ids {
        command.arg(id);
    }
    let _ = command.output().await;
}

/// Removes the containers a test left behind, so that a failed test does not break the next one.
async fn cleanup(proxy_ids: &[String]) {
    for proxy_id in proxy_ids {
        let name = format!("sp-container-{proxy_id}-0");
        let _ = tokio::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await;
    }
}

/// Runs `docker inspect` and returns the parsed JSON of a container.
async fn inspect(name: &str) -> serde_json::Value {
    let output = tokio::process::Command::new("docker")
        .args(["inspect", name])
        .output()
        .await
        .expect("docker inspect runs");
    assert!(
        output.status.success(),
        "docker inspect {name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let documents: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("docker inspect returns json");
    documents.into_iter().next().expect("one container")
}

#[tokio::test]
async fn starts_an_app_in_docker_and_proxies_to_it() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_DOCKER=1 to run the Docker tests");
        return;
    }

    cleanup_all().await;
    let instance = TestInstance::start(&config(24000, "", "")).await;
    let jack = instance.login("jack", "password").await;

    // start the app through the API, as the browser does
    let started: serde_json::Value = jack
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();

    let status: serde_json::Value = jack
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=60"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");

    // the container looks exactly like the Java implementation creates it
    let container = inspect(&format!("sp-container-{proxy_id}-0")).await;
    let labels = container["Config"]["Labels"]
        .as_object()
        .expect("labels")
        .clone();
    assert_eq!(
        labels.get("my.own.label").and_then(|value| value.as_str()),
        Some("my-label-value"),
        "labels of the app definition are set"
    );
    assert_eq!(
        labels
            .get("openanalytics.eu/sp-proxy-id")
            .and_then(|value| value.as_str()),
        Some(proxy_id.as_str())
    );
    assert_eq!(
        labels
            .get("openanalytics.eu/sp-instance")
            .and_then(|value| value.as_str()),
        Some(instance.state.identifiers.instance_id.as_str())
    );
    assert_eq!(
        labels
            .get("openanalytics.eu/sp-user-id")
            .and_then(|value| value.as_str()),
        Some("jack")
    );
    assert_eq!(
        labels
            .get("openanalytics.eu/sp-proxied-app")
            .and_then(|value| value.as_str()),
        Some("true")
    );

    let environment: BTreeMap<String, String> = container["Config"]["Env"]
        .as_array()
        .expect("env")
        .iter()
        .filter_map(|entry| entry.as_str())
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    assert_eq!(
        environment.get("SHINYPROXY_USERNAME").map(String::as_str),
        Some("jack")
    );
    assert_eq!(
        environment.get("SHINYPROXY_USERGROUPS").map(String::as_str),
        Some("SCIENTISTS")
    );
    assert_eq!(
        environment.get("MY_VARIABLE").map(String::as_str),
        Some("my-value"),
        "container-env is injected"
    );

    // the port is published on the configured range and interface
    let bindings = container["HostConfig"]["PortBindings"]["3838/tcp"]
        .as_array()
        .expect("port bindings")
        .clone();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["HostIp"], "127.0.0.1");
    let host_port: u16 = bindings[0]["HostPort"]
        .as_str()
        .expect("host port")
        .parse()
        .expect("number");
    assert!(
        (24000..24100).contains(&host_port),
        "the host port comes from the configured range: {host_port}"
    );

    // the app is reachable through the proxy
    let body = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("app request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("sp-testapp"), "{body}");
    assert!(
        body.contains("shiny.iframe.js"),
        "the iframe script is injected: {body}"
    );

    // and it sees the ShinyProxy environment
    let container_environment: BTreeMap<String, String> = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("env request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        container_environment
            .get("SHINYPROXY_USERNAME")
            .map(String::as_str),
        Some("jack")
    );
    assert_eq!(
        container_environment
            .get("SHINYPROXY_PUBLIC_PATH")
            .map(String::as_str),
        Some(format!("/app_proxy/{proxy_id}/").as_str()),
        "the app knows the path it is served on"
    );
    assert_eq!(
        container_environment.get("MY_VARIABLE").map(String::as_str),
        Some("my-value")
    );

    // websockets are tunnelled to the container as well (the handshake needs the session cookie)
    use futures::{SinkExt, StreamExt};
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
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket connects");
    assert_eq!(response.status(), 101);
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "hello docker".into(),
        ))
        .await
        .expect("sends");
    let message = socket.next().await.expect("answer").expect("message");
    assert_eq!(message.into_text().expect("text"), "hello docker");
    socket.close(None).await.ok();

    // stopping the app removes the container
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    for _ in 0..60 {
        let output = tokio::process::Command::new("docker")
            .args(["inspect", &format!("sp-container-{proxy_id}-0")])
            .output()
            .await
            .expect("docker inspect runs");
        if !output.status.success() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let output = tokio::process::Command::new("docker")
        .args(["inspect", &format!("sp-container-{proxy_id}-0")])
        .output()
        .await
        .expect("docker inspect runs");
    assert!(
        !output.status.success(),
        "the container must be removed when the app is stopped"
    );

    cleanup(&[proxy_id]).await;
    instance.stop();
}

#[tokio::test]
async fn recovers_running_apps_after_a_restart() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_DOCKER=1 to run the Docker tests");
        return;
    }

    // the same configuration file for both servers, so that they have the same instance id
    let configuration = config(24200, "  recover-running-proxies: true\n", "");

    cleanup_all().await;
    let first = TestInstance::start(&configuration).await;
    let jack = first.login("jack", "password").await;
    let started: serde_json::Value = jack
        .post(first.url("/app_i/01_hello/_"))
        // the browser sends its time zone, which ends up in a label; the Java implementation requires
        // that label to be present before it recovers a container
        .json(&serde_json::json!({"timezone": "Europe/Brussels"}))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();
    let status: serde_json::Value = jack
        .get(first.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=60"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");
    let instance_id = first.state.identifiers.instance_id.clone();

    // the server goes away without stopping the app (a crash, or a rolling update)
    first.stop();

    // a new server with the same configuration takes the app over
    let second = TestInstance::start(&configuration).await;
    assert_eq!(
        second.state.identifiers.instance_id, instance_id,
        "the same configuration must produce the same instance id"
    );
    assert!(second.state.recovery.enabled());

    let jack = second.login("jack", "password").await;
    let proxies: serde_json::Value = jack
        .get(second.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    let entries = proxies["data"].as_array().expect("array");
    assert_eq!(entries.len(), 1, "the running app is recovered: {proxies}");
    assert_eq!(entries[0]["id"], proxy_id.as_str());
    assert_eq!(entries[0]["status"], "Up");
    assert_eq!(entries[0]["userId"], "jack");
    assert_eq!(entries[0]["specId"], "01_hello");

    // and it is still reachable through the new server
    let body = jack
        .get(second.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("app request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("sp-testapp"), "{body}");

    // stopping it through the new server cleans up
    let response = jack
        .put(second.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    cleanup(&[proxy_id]).await;
    second.stop();
}

#[tokio::test]
async fn does_not_recover_apps_of_another_configuration() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_DOCKER=1 to run the Docker tests");
        return;
    }

    let first_configuration = config(24300, "  recover-running-proxies: true\n", "");
    cleanup_all().await;
    let first = TestInstance::start(&first_configuration).await;
    let jack = first.login("jack", "password").await;
    let started: serde_json::Value = jack
        .post(first.url("/app_i/01_hello/_"))
        // the browser sends its time zone, which ends up in a label; the Java implementation requires
        // that label to be present before it recovers a container
        .json(&serde_json::json!({"timezone": "Europe/Brussels"}))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();
    let status: serde_json::Value = jack
        .get(first.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=60"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");
    let first_instance_id = first.state.identifiers.instance_id.clone();
    first.stop();

    // a different configuration (an extra app) means a different instance id
    let extra_spec = format!("    - id: 02_other\n      container-image: {}\n", image());
    let second_configuration = config(24300, "  recover-running-proxies: true\n", &extra_spec);
    let second = TestInstance::start(&second_configuration).await;
    assert_ne!(second.state.identifiers.instance_id, first_instance_id);
    let jack = second.login("jack", "password").await;
    let proxies: serde_json::Value = jack
        .get(second.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        proxies["data"].as_array().map(Vec::len),
        Some(0),
        "apps of another configuration are not recovered: {proxies}"
    );
    second.stop();

    // ... unless recovery from a different configuration is enabled
    let third_configuration = config(
        24300,
        "  recover-running-proxies: true\n  recover-running-proxies-from-different-config: true\n",
        &extra_spec,
    );
    let third = TestInstance::start(&third_configuration).await;
    let jack = third.login("jack", "password").await;
    let proxies: serde_json::Value = jack
        .get(third.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        proxies["data"].as_array().map(Vec::len),
        Some(1),
        "with recover-running-proxies-from-different-config the app is taken over: {proxies}"
    );

    cleanup(&[proxy_id]).await;
    third.stop();
}
