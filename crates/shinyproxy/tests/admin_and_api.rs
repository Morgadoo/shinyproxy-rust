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

//! The admin pages and the remaining API endpoints: app transfer, custom app details, issue reporting,
//! `/app_direct` and `/api/route`.
//!
//! Replaces the Java `AdminControllerTest`, `ProxyApiControllerTest`, `IssueControllerTest`,
//! `AppDirectControllerTest` and `DelegateProxyAdminControllerTest` for the behaviour that does not need
//! a container runtime.

mod common;

use common::TestInstance;

const CONFIG: &str = r##"
proxy:
  title: Test Proxy
  authentication: simple
  admin-groups: admins
  allow-transfer-app: true
  container-backend: local
  container-wait-timeout: 15000
  docker:
    port-range-start: 25000
  support:
    mail-to-address: support@example.com
  users:
    - name: jack
      password: password
      groups: scientists
    - name: jeff
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
      access-groups: [ scientists, admins ]
      custom-app-details:
        - name: Owner
          description: The user running this app
          value: "#{proxy.userId}"
        - name: Static
          value: fixed value
"##;

/// Starts an app and waits until it is up.
async fn start_app(instance: &TestInstance, client: &common::TestClient) -> String {
    let json: serde_json::Value = client
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = json["data"]["id"].as_str().expect("proxy id").to_string();
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

#[tokio::test]
async fn admin_pages_render_for_administrators() {
    let instance = TestInstance::start(CONFIG).await;
    let admin = instance.login("root", "rootpw").await;

    let body = admin
        .get(instance.url("/admin"))
        .send()
        .await
        .expect("admin page")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Active Proxies"), "{body}");
    assert!(body.contains("Last heartbeat"), "{body}");
    assert!(body.contains("window.Shiny.admin.init();"), "{body}");
    assert!(body.contains("js/shiny.admin.js"), "{body}");

    let body = admin
        .get(instance.url("/admin/about"))
        .send()
        .await
        .expect("about page")
        .text()
        .await
        .expect("body");
    assert!(body.contains("ShinyProxy RuntimeID"), "{body}");
    assert!(
        body.contains(&instance.state.identifiers.instance_id),
        "{body}"
    );
    assert!(body.contains("Rust implementation"), "{body}");
    assert!(body.contains("Container backend"), "{body}");
    assert!(body.contains("local"), "{body}");

    // the assets of the admin page exist
    for path in [
        "/webjars/datatables/1.13.5/js/dataTables.bootstrap.min.js",
        "/webjars/datatables-buttons/2.4.1/js/buttons.bootstrap.min.js",
        "/webjars/datatables-buttons/2.4.1/js/buttons.html5.min.js",
        "/webjars/datatables-responsive/2.2.7/js/responsive.bootstrap.min.js",
        "/webjars/datatables/1.13.5/css/dataTables.bootstrap.min.css",
        "/js/shiny.admin.js",
    ] {
        let response = admin
            .get(instance.url(path))
            .send()
            .await
            .expect("asset request");
        assert_eq!(response.status(), 200, "{path}");
    }

    // normal users may not see the admin pages
    let user = instance.login("jack", "password").await;
    for path in ["/admin", "/admin/about", "/admin/data"] {
        let response = user.get(instance.url(path)).send().await.expect("request");
        assert_eq!(response.status(), 403, "{path}");
    }

    instance.stop();
}

#[tokio::test]
async fn apps_can_be_transferred_to_another_user() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // no user id in the request
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/userId")))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("transfer request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(
        json["data"],
        "Cannot transfer app because no userId is provided in the request"
    );

    // transferring to yourself
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/userId")))
        .json(&serde_json::json!({"userId": "jack"}))
        .send()
        .await
        .expect("transfer request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(
        json["data"],
        "Cannot transfer app because the proxy is already owned by this user"
    );

    // a real transfer
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/userId")))
        .json(&serde_json::json!({"userId": "jeff"}))
        .send()
        .await
        .expect("transfer request");
    assert_eq!(response.status(), 200);

    // jack lost the app, jeff owns it and its instance was renamed
    let proxies: serde_json::Value = jack
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(0));

    let jeff = instance.login("jeff", "password").await;
    let proxies: serde_json::Value = jeff
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(proxies["data"][0]["userId"], "jeff");
    assert_eq!(
        proxies["data"][0]["runtimeValues"]["SHINYPROXY_APP_INSTANCE"],
        "jack-Default"
    );

    instance.stop();
}

#[tokio::test]
async fn transfer_requires_the_feature_to_be_enabled() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  docker:
    port-range-start: 25100
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/userId")))
        .json(&serde_json::json!({"userId": "jeff"}))
        .send()
        .await
        .expect("transfer request");
    assert_eq!(
        response.status(),
        403,
        "allow-transfer-app defaults to false"
    );

    instance.stop();
}

#[tokio::test]
async fn custom_app_details_are_resolved_per_request() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    let details: serde_json::Value = jack
        .get(instance.url(&format!("/api/proxy/{proxy_id}/details")))
        .send()
        .await
        .expect("details request")
        .json()
        .await
        .expect("json");
    assert_eq!(details["status"], "success");
    let entries = details["data"].as_array().expect("array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["name"], "Owner");
    assert_eq!(entries[0]["description"], "The user running this app");
    assert_eq!(entries[0]["value"], "jack", "the expression is resolved");
    assert_eq!(entries[1]["value"], "fixed value");

    // another user gets the "app is gone" answer
    let jeff = instance.login("jeff", "password").await;
    let response = jeff
        .get(instance.url(&format!("/api/proxy/{proxy_id}/details")))
        .send()
        .await
        .expect("details request");
    assert_eq!(response.status(), 410);

    instance.stop();
}

#[tokio::test]
async fn issue_reporting_validates_its_input() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    // no mail server configured in this test, so reporting is not available
    let response = jack
        .post(instance.url("/issue"))
        .json(&serde_json::json!({"message": "help", "currentLocation": "/"}))
        .send()
        .await
        .expect("issue request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "Report issue is not configured");

    instance.stop();
}

#[tokio::test]
async fn issue_reporting_checks_message_and_location() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  support:
    mail-to-address: support@example.com
  users:
    - name: jack
      password: password
  specs: []
spring:
  mail:
    host: localhost
    port: 2525
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;

    let response = jack
        .post(instance.url("/issue"))
        .json(&serde_json::json!({"currentLocation": "/"}))
        .send()
        .await
        .expect("issue request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "Cannot report issue: no message provided");

    let response = jack
        .post(instance.url("/issue"))
        .json(&serde_json::json!({"message": "help"}))
        .send()
        .await
        .expect("issue request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(
        json["data"],
        "Cannot report issue: no currentLocation provided"
    );

    // an unknown app in the report is forbidden
    let response = jack
        .post(instance.url("/issue"))
        .json(&serde_json::json!({
            "message": "help",
            "currentLocation": "/",
            "proxyId": "does-not-exist"
        }))
        .send()
        .await
        .expect("issue request");
    assert_eq!(response.status(), 403);

    // with everything in place the mail is attempted (and fails, because there is no mail server)
    let response = jack
        .post(instance.url("/issue"))
        .json(&serde_json::json!({"message": "help", "currentLocation": "/"}))
        .send()
        .await
        .expect("issue request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "Error while sending e-mail");

    instance.stop();
}

#[tokio::test]
async fn app_direct_starts_the_app_and_proxies_to_it() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    // without a trailing slash the request is redirected
    let response = jack
        .get(instance.url("/app_direct/01_hello"))
        .send()
        .await
        .expect("app_direct request");
    assert_eq!(response.status(), 302);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/app_direct/01_hello/")
    );

    // the app is started on demand and the response comes from the app
    let response = jack
        .get(instance.url("/app_direct/01_hello/"))
        .send()
        .await
        .expect("app_direct request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(body.contains("sp-testapp"), "{body}");
    // app_direct does not inject the iframe script
    assert!(!body.contains("shiny.iframe.js"), "{body}");

    // the app appears in the api with the app_direct public path
    let proxies: serde_json::Value = jack
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        proxies["data"][0]["runtimeValues"]["SHINYPROXY_PUBLIC_PATH"],
        "/app_direct_i/01_hello/_"
    );

    // a second request reuses the running app
    let body = jack
        .get(instance.url("/app_direct/01_hello/env"))
        .send()
        .await
        .expect("app_direct request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("SHINYPROXY_USERNAME"), "{body}");

    // and /api/route reaches the same app
    let proxy_id = proxies["data"][0]["id"].as_str().expect("id");
    let response = jack
        .get(instance.url(&format!("/api/route/{proxy_id}/")))
        .send()
        .await
        .expect("api route request");
    assert_eq!(response.status(), 200);
    assert!(response.text().await.expect("body").contains("sp-testapp"));

    instance.stop();
}

#[tokio::test]
async fn openapi_is_disabled_by_default_and_can_be_enabled() {
    // disabled by default, exactly like springdoc in the Java implementation
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();
    for path in ["/v3/api-docs", "/swagger-ui/index.html"] {
        let response = client
            .get(instance.url(path))
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 404, "{path}");
    }
    instance.stop();

    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
springdoc:
  api-docs:
    enabled: true
  swagger-ui:
    enabled: true
"##,
    )
    .await;
    let client = instance.client();

    // the description is public, as springdoc's endpoints are
    let document: serde_json::Value = client
        .get(instance.url("/v3/api-docs"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(document["openapi"], "3.0.1");
    assert!(
        document["paths"]["/api/proxy"]["get"]["summary"].is_string(),
        "{document}"
    );
    assert!(
        document["paths"]["/heartbeat/{proxyId}"]["post"].is_object(),
        "{document}"
    );

    let body = client
        .get(instance.url("/swagger-ui/index.html"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("ShinyProxy API"), "{body}");
    assert!(body.contains("/api/proxy/{proxyId}/status"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn grafana_is_proxied_for_administrators_when_configured() {
    // without proxy.monitoring.grafana-url the route answers 403, as the Java implementation does
    let instance = TestInstance::start(CONFIG).await;
    let admin = instance.login("root", "rootpw").await;
    let response = admin
        .get(instance.url("/grafana/"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);
    instance.stop();

    // a fake Grafana that echoes the request
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let grafana_address = listener.local_addr().expect("address");
    let grafana = tokio::spawn(async move {
        let app = axum::Router::new().fallback(|request: axum::extract::Request| async move {
            let user = request
                .headers()
                .get("x-sp-userid")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            format!("grafana {} user={user}", request.uri())
        });
        axum::serve(listener, app).await.ok();
    });

    let instance = TestInstance::start(&format!(
        r##"
proxy:
  authentication: simple
  admin-groups: admins
  container-backend: local
  monitoring:
    grafana-url: http://{grafana_address}/
  users:
    - name: jack
      password: password
      groups: scientists
    - name: root
      password: rootpw
      groups: admins
  specs: []
"##
    ))
    .await;

    // a normal user may not reach it
    let user = instance.login("jack", "password").await;
    let response = user
        .get(instance.url("/grafana/d/my-dashboard"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);

    // an administrator does, and the request keeps its path, query and the user header
    let admin = instance.login("root", "rootpw").await;
    let body = admin
        .get(instance.url("/grafana/d/my-dashboard?orgId=1"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert_eq!(body, "grafana /d/my-dashboard?orgId=1 user=root");

    // the bare path is redirected to the slash variant
    let response = admin
        .get(instance.url("/grafana"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 302);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/grafana/")
    );

    instance.stop();
    grafana.abort();
}

#[tokio::test]
async fn delegate_proxy_endpoint_is_admin_only() {
    let instance = TestInstance::start(CONFIG).await;

    let user = instance.login("jack", "password").await;
    let response = user
        .delete(instance.url("/admin/delegate-proxy"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);

    let admin = instance.login("root", "rootpw").await;
    let response = admin
        .delete(instance.url("/admin/delegate-proxy"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["status"], "success");

    instance.stop();
}
