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

//! End-to-end tests of the Kubernetes backend, against a real cluster.
//!
//! The tests need a cluster with the `sp-testapp:test` image (`scripts/start-test-k3s.sh` starts a k3s in
//! Docker and imports it) and are skipped unless `SP_TEST_K8S=1` is set:
//!
//! ```sh
//! ./scripts/build-test-image.sh
//! ./scripts/start-test-k3s.sh
//! SP_TEST_K8S=1 cargo test -p shinyproxy --test kubernetes -- --test-threads=1
//! ```
//!
//! ShinyProxy runs outside the cluster here, which is the `NodePort` path of the backend (the pods are
//! reached through the host IP of the node).

mod common;

use std::time::Duration;

use common::TestInstance;

/// Whether the Kubernetes tests are enabled.
fn enabled() -> bool {
    std::env::var("SP_TEST_K8S").as_deref() == Ok("1")
}

/// The image the tests start.
fn image() -> String {
    std::env::var("SP_TEST_IMAGE").unwrap_or_else(|_| "sp-testapp:test".to_string())
}

/// Runs `kubectl` inside the k3s container and returns its output.
fn kubectl(arguments: &[&str]) -> String {
    let container =
        std::env::var("SP_TEST_K3S_CONTAINER").unwrap_or_else(|_| "test-k3s".to_string());
    let mut command = std::process::Command::new("docker");
    command.arg("exec").arg(&container).arg("kubectl");
    command.args(arguments);
    let output = command.output().expect("kubectl runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A configuration that runs the apps in the cluster.
fn config(extra_spec: &str) -> String {
    // the kubeconfig of the cluster is used through the environment (`kube::Config::infer`)
    format!(
        r##"
proxy:
  title: Kubernetes Test
  authentication: simple
  admin-groups: admins
  container-backend: kubernetes
  container-wait-timeout: 60000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  kubernetes:
    namespace: default
    image-pull-policy: IfNotPresent
    pod-wait-time: 60000
  users:
    - name: jack
      password: password
      groups: scientists
  specs:
    - id: 01_hello
      display-name: Hello Kubernetes
      container-image: {image}
      port: 3838
      access-groups: scientists
{extra_spec}
"##,
        image = image()
    )
}

/// Removes the pods and services of previous runs.
fn cleanup() {
    kubectl(&[
        "delete",
        "pods",
        "-l",
        "openanalytics.eu/sp-proxied-app=true",
        "--grace-period=0",
        "--force",
    ]);
    kubectl(&[
        "delete",
        "services",
        "-l",
        "openanalytics.eu/sp-instance",
        "--grace-period=0",
    ]);
    kubectl(&[
        "delete",
        "secrets,configmaps",
        "-l",
        "openanalytics.eu/sp-additional-manifest=true",
        "--grace-period=0",
    ]);
}

/// Starts an app through the API and waits until it is up.
async fn start_app(instance: &TestInstance, client: &common::TestClient) -> String {
    let started: serde_json::Value = client
        .post(instance.url("/app_i/01_hello/_"))
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
    let status: serde_json::Value = client
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
    proxy_id
}

#[tokio::test]
async fn starts_an_app_as_a_pod_and_proxies_to_it() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_K8S=1 (and start a cluster) to run the Kubernetes tests");
        return;
    }
    cleanup();

    let instance = TestInstance::start(&config("")).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // the pod looks the way the Java implementation creates it
    let pod_name = format!("sp-pod-{proxy_id}-0");
    let pod: serde_json::Value =
        serde_json::from_str(&kubectl(&["get", "pod", &pod_name, "-o", "json"]))
            .expect("the pod exists");

    assert_eq!(pod["metadata"]["namespace"], "default");
    assert_eq!(
        pod["metadata"]["labels"]["openanalytics.eu/sp-proxied-app"],
        "true"
    );
    assert_eq!(
        pod["metadata"]["labels"]["openanalytics.eu/sp-instance"],
        instance.state.identifiers.instance_id.as_str()
    );
    assert_eq!(
        pod["metadata"]["labels"]["openanalytics.eu/sp-headless-service"],
        "true"
    );
    // annotation backed runtime values are annotations, not labels (unlike the Docker backend)
    assert_eq!(
        pod["metadata"]["annotations"]["openanalytics.eu/sp-proxy-id"],
        proxy_id.as_str()
    );
    assert_eq!(
        pod["metadata"]["annotations"]["openanalytics.eu/sp-user-id"],
        "jack"
    );
    assert_eq!(pod["spec"]["restartPolicy"], "Never");
    assert_eq!(pod["spec"]["hostname"], pod_name.as_str());
    assert_eq!(pod["spec"]["subdomain"], "sp-headless-service");

    let container = &pod["spec"]["containers"][0];
    assert_eq!(container["name"], "sp-container-0");
    assert_eq!(container["imagePullPolicy"], "IfNotPresent");
    assert_eq!(container["ports"][0]["containerPort"], 3838);
    assert_eq!(
        container["terminationMessagePolicy"],
        "FallbackToLogsOnError"
    );
    let environment = container["env"].as_array().expect("env");
    let username = environment
        .iter()
        .find(|variable| variable["name"] == "SHINYPROXY_USERNAME")
        .expect("the username variable");
    assert_eq!(username["value"], "jack");

    // the NodePort service publishes the port of the app
    let service_name = format!("sp-service-{proxy_id}-0");
    let service: serde_json::Value =
        serde_json::from_str(&kubectl(&["get", "service", &service_name, "-o", "json"]))
            .expect("the service exists");
    assert_eq!(service["spec"]["type"], "NodePort");
    assert_eq!(service["spec"]["ports"][0]["port"], 3838);
    assert!(
        service["spec"]["ports"][0]["nodePort"]
            .as_i64()
            .unwrap_or(0)
            > 0
    );

    // the app answers through the proxy, with the injected iframe script
    let body = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .send()
        .await
        .expect("app request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("sp-testapp"), "{body}");
    assert!(body.contains("shiny.iframe.js"), "{body}");

    // and it received its ShinyProxy environment
    let environment: std::collections::BTreeMap<String, String> = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("env request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        environment.get("SHINYPROXY_USERNAME").map(String::as_str),
        Some("jack")
    );
    assert_eq!(
        environment.get("SHINYPROXY_USERGROUPS").map(String::as_str),
        Some("SCIENTISTS")
    );

    // websockets are tunnelled to the pod as well
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
            "hello kubernetes".into(),
        ))
        .await
        .expect("sends");
    let message = socket.next().await.expect("answer").expect("message");
    assert_eq!(message.into_text().expect("text"), "hello kubernetes");
    socket.close(None).await.ok();

    // stopping the app removes the pod and the service
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    for _ in 0..60 {
        if kubectl(&["get", "pod", &pod_name, "--ignore-not-found"]).is_empty()
            && kubectl(&["get", "service", &service_name, "--ignore-not-found"]).is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        kubectl(&["get", "pod", &pod_name, "--ignore-not-found"]).is_empty(),
        "the pod must be removed"
    );
    assert!(
        kubectl(&["get", "service", &service_name, "--ignore-not-found"]).is_empty(),
        "the service must be removed"
    );

    instance.stop();
    cleanup();
}

#[tokio::test]
async fn applies_pod_patches_and_additional_manifests() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_K8S=1 to run the Kubernetes tests");
        return;
    }
    cleanup();

    // a pod patch that adds a label and an environment variable, plus two additional manifests
    let instance = TestInstance::start(&config(
        r##"      kubernetes-pod-patches: |
        - op: add
          path: /metadata/labels/patched
          value: "yes"
        - op: add
          path: /spec/containers/0/env/-
          value:
            name: PATCHED_VARIABLE
            value: patched-value
      kubernetes-additional-manifests:
        - |
          apiVersion: v1
          kind: ConfigMap
          metadata:
            name: sp-test-configmap
          data:
            key: value
      kubernetes-additional-persistent-manifests:
        - |
          apiVersion: v1
          kind: ConfigMap
          metadata:
            name: sp-test-persistent-configmap
          data:
            key: value
"##,
    ))
    .await;

    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // the patch was applied
    let pod: serde_json::Value = serde_json::from_str(&kubectl(&[
        "get",
        "pod",
        &format!("sp-pod-{proxy_id}-0"),
        "-o",
        "json",
    ]))
    .expect("the pod exists");
    assert_eq!(pod["metadata"]["labels"]["patched"], "yes");
    let environment = pod["spec"]["containers"][0]["env"].as_array().expect("env");
    assert!(
        environment
            .iter()
            .any(|variable| variable["name"] == "PATCHED_VARIABLE"
                && variable["value"] == "patched-value"),
        "the patched variable must be there: {environment:?}"
    );

    // the app itself sees the patched variable
    let container_environment: std::collections::BTreeMap<String, String> = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("env request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        container_environment
            .get("PATCHED_VARIABLE")
            .map(String::as_str),
        Some("patched-value")
    );

    // both additional manifests exist, with the ShinyProxy labels
    let configmap: serde_json::Value = serde_json::from_str(&kubectl(&[
        "get",
        "configmap",
        "sp-test-configmap",
        "-o",
        "json",
    ]))
    .expect("the configmap exists");
    assert_eq!(configmap["data"]["key"], "value");
    assert_eq!(
        configmap["metadata"]["labels"]["openanalytics.eu/sp-additional-manifest"],
        "true"
    );
    assert_eq!(
        configmap["metadata"]["labels"]["openanalytics.eu/sp-persistent-manifest"],
        "false"
    );
    assert!(!kubectl(&["get", "configmap", "sp-test-persistent-configmap"]).is_empty());

    // stopping the app removes the manifest, but keeps the persistent one
    jack.put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");

    for _ in 0..60 {
        if kubectl(&[
            "get",
            "configmap",
            "sp-test-configmap",
            "--ignore-not-found",
        ])
        .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        kubectl(&[
            "get",
            "configmap",
            "sp-test-configmap",
            "--ignore-not-found"
        ])
        .is_empty(),
        "the additional manifest must be removed with the app"
    );
    assert!(
        !kubectl(&[
            "get",
            "configmap",
            "sp-test-persistent-configmap",
            "--ignore-not-found"
        ])
        .is_empty(),
        "the persistent manifest must survive the app"
    );

    kubectl(&[
        "delete",
        "configmap",
        "sp-test-persistent-configmap",
        "--ignore-not-found",
    ]);
    instance.stop();
    cleanup();
}

#[tokio::test]
async fn recovers_running_pods_after_a_restart() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_K8S=1 to run the Kubernetes tests");
        return;
    }
    cleanup();

    let configuration = format!("{}  recover-running-proxies: true\n", config(""));
    // the recovery flag belongs to the `proxy` block, so it is inserted there
    let configuration = configuration.replace(
        "  container-backend: kubernetes",
        "  container-backend: kubernetes\n  recover-running-proxies: true",
    );
    let configuration = configuration.replace("  recover-running-proxies: true\n\n", "\n");

    let first = TestInstance::start(&configuration).await;
    let jack = first.login("jack", "password").await;
    let proxy_id = start_app(&first, &jack).await;
    let instance_id = first.state.identifiers.instance_id.clone();
    first.stop();

    // a new server with the same configuration takes the pod over
    let second = TestInstance::start(&configuration).await;
    assert_eq!(second.state.identifiers.instance_id, instance_id);

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

    jack.put(second.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    tokio::time::sleep(Duration::from_secs(2)).await;

    second.stop();
    cleanup();
}
