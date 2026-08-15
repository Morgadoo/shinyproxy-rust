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

//! The authentication backends that do not need an external identity provider in the test.
//!
//! Replaces the Java `TestCustomHeaderAuthentication` and the web service part of the authentication
//! tests.

mod common;

use common::TestInstance;

#[tokio::test]
async fn header_based_authentication() {
    let instance = TestInstance::start(
        r##"
proxy:
  title: Header Auth
  authentication: custom-header
  admin-groups: admins
  container-backend: local
  custom-header:
    username-header-name: X-SP-UserId
    groups-header-name: X-SP-UserGroups
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      access-groups: scientists
    - id: 02_admin_only
      display-name: Admin Application
      container-image: sp-testapp
      access-groups: admins
"##,
    )
    .await;
    let client = instance.client();

    // a request without the header is sent to the error page (there is no login form)
    let response = client
        .get(instance.url("/"))
        .header("accept", "text/html")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/auth-error")
    );

    // with the headers the user is logged in, without ever seeing a login page
    let body = client
        .get(instance.url("/"))
        .header("X-SP-UserId", "jack")
        .header("X-SP-UserGroups", "scientists")
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(!body.contains("Admin Application"), "{body}");
    assert!(body.contains("jack"), "{body}");
    // the user is logged in, so the navbar shows the sign out button (as in Java, where logging out
    // clears the session and the header logs the user in again)
    assert!(body.contains("href=\"/logout\""), "{body}");

    // an administrator sees the admin button and every app
    let admin = instance.client();
    let body = admin
        .get(instance.url("/"))
        .header("X-SP-UserId", "root")
        .header("X-SP-UserGroups", "admins")
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Admin Application"), "{body}");
    assert!(body.contains("href=\"/admin\""), "{body}");

    // the header decides the user on every request: another header means another user
    let body = admin
        .get(instance.url("/"))
        .header("X-SP-UserId", "jeff")
        .header("X-SP-UserGroups", "scientists")
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("jeff"), "{body}");
    assert!(!body.contains("Admin Application"), "{body}");

    // the API works with the headers as well
    let response: serde_json::Value = client
        .get(instance.url("/api/proxyspec"))
        .header("X-SP-UserId", "jack")
        .header("X-SP-UserGroups", "scientists")
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(response["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(response["data"][0]["id"], "01_hello");

    instance.stop();
}

#[tokio::test]
async fn header_based_authentication_without_a_groups_header() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: custom-header
  container-backend: local
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
"##,
    )
    .await;
    let client = instance.client();

    // the default header name is REMOTE_USER; without a groups header the user has no groups (which is
    // enough for an app without access control)
    let body = client
        .get(instance.url("/"))
        .header("REMOTE_USER", "jack")
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(body.contains("jack"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn webservice_authentication() {
    // a fake web service that accepts one user and answers with their groups
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let service = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/login",
            axum::routing::post(|body: String| async move {
                let credentials: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                if credentials["username"] == "jack" && credentials["password"] == "password" {
                    (
                        axum::http::StatusCode::OK,
                        r#"{"groups":["scientists","ROLE_users"],"organisation":"openanalytics"}"#,
                    )
                } else {
                    (axum::http::StatusCode::UNAUTHORIZED, "nope")
                }
            }),
        );
        axum::serve(listener, app).await.ok();
    });

    let instance = TestInstance::start(&format!(
        r##"
proxy:
  authentication: webservice
  container-backend: local
  webservice:
    authentication-url: http://{address}/login
    authentication-request-body: '{{"username":"%s","password":"%s"}}'
    groups-expression: "#{{json.groups}}"
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      access-groups: scientists
    - id: 02_other
      display-name: Other Application
      container-image: sp-testapp
      access-groups: others
"##
    ))
    .await;

    // wrong credentials end up on the login page with the error
    let client = instance.client();
    let token = instance.csrf_token(&client).await;
    let response = client
        .post(instance.url("/login"))
        .form(&[
            ("username", "jack"),
            ("password", "wrong"),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await
        .expect("login request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login?error=true")
    );

    // the right credentials log the user in, with the groups from the answer of the web service
    let client = instance.login("jack", "password").await;
    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(!body.contains("Other Application"), "{body}");
    assert!(body.contains("jack"), "{body}");

    instance.stop();
    service.abort();
}

#[tokio::test]
async fn webservice_authentication_needs_its_configuration() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("application.yml");
    std::fs::write(&path, "proxy:\n  authentication: webservice\n  specs: []\n").expect("write");
    let options = containerproxy::config::LoadOptions {
        args: vec![format!("--spring.config.location={}", path.display())],
        ..containerproxy::config::LoadOptions::default()
    };
    let (raw, mut settings) = shinyproxy::load_config(options).expect("configuration loads");
    settings.proxy.container_backend = Some("local".to_string());
    let error = shinyproxy::web::AppState::new(raw, settings)
        .await
        .expect_err("the configuration must be refused")
        .to_string();
    assert_eq!(
        error,
        "Webservice authentication enabled, but no \
         'proxy.webservice.authentication-request-body' defined!"
    );
}
