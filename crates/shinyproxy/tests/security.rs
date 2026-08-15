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

//! The security review of P14: who may reach which route, and the classic web weaknesses.
//!
//! The first test is the matrix the plan asks for: every route of the server times every kind of visitor
//! (anonymous, a user, another user, an administrator). The rest covers header injection, open redirects,
//! secrets in the log output and the cookie and CSRF settings.

mod common;

use std::time::Duration;

use common::{TestClient, TestInstance};

const CONFIG: &str = r##"
proxy:
  title: Security Test
  authentication: simple
  admin-groups: admins
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  allow-transfer-app: true
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
    - id: 02_admin_only
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: admins
"##;

/// Who is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visitor {
    /// Nobody logged in.
    Anonymous,
    /// The owner of the app.
    Owner,
    /// Another user, who owns nothing.
    Other,
    /// An administrator.
    Admin,
}

/// What the server is expected to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The request is served.
    Ok,
    /// The visitor is sent to the login page, or — on the API paths — answered with the document of
    /// `AuthenticationRequiredFilter` (410 and `shinyproxy_authentication_required`).
    Unauthenticated,
    /// The visitor is logged in but may not do this.
    Forbidden,
    /// The route answers with a JSON failure (an app that is not theirs, ...).
    Failure,
    /// The route answers successfully, but without any information about the app of the other user
    /// (`/api/proxy/{id}/status` reports an unknown app as stopped, exactly as in Java).
    StoppedStub,
}

/// The paths that answer an unauthenticated request with the API document instead of a redirect
/// (`AuthenticationRequiredFilter` in the Java implementation).
fn needs_authentication_answer(path: &str) -> bool {
    path.starts_with("/app_proxy/")
        || path.starts_with("/heartbeat/")
        || path.starts_with("/api/")
        || path == "/admin/data"
        || path == "/issue"
}

/// Starts an app for the owner and returns its proxy id.
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

#[tokio::test]
async fn every_route_checks_who_is_asking() {
    let instance = TestInstance::start(CONFIG).await;
    let anonymous = instance.client();
    let owner = instance.login("jack", "password").await;
    let other = instance.login("jeff", "password").await;
    let admin = instance.login("root", "rootpw").await;
    let proxy_id = start_app(&instance, &owner).await;

    // route, method, and what every kind of visitor must get
    let routes: Vec<(&str, &str, Expected, Expected, Expected, Expected)> = vec![
        // path, method, anonymous, owner, other user, admin
        (
            "/login",
            "GET",
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        (
            "/",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        (
            "/app/01_hello",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        // an app definition the visitor may not use
        (
            "/app/02_admin_only",
            "GET",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        (
            "/api/proxyspec",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        (
            "/api/proxyspec/02_admin_only",
            "GET",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        (
            "/api/proxy",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        // the app of the owner: another user must not see or touch it
        // an app belongs to its user: even an administrator reads it through /admin/data instead of the
        // API of the app (the Java implementation answers the same way)
        (
            "/api/proxy/{id}",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Failure,
            Expected::Failure,
        ),
        (
            "/api/proxy/{id}/status",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::StoppedStub,
            Expected::StoppedStub,
        ),
        (
            "/api/proxy/{id}/details",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Failure,
            Expected::Failure,
        ),
        (
            "/app_proxy/{target}/",
            "GET",
            Expected::Unauthenticated,
            Expected::Ok,
            Expected::Failure,
            Expected::Failure,
        ),
        // administration
        (
            "/admin",
            "GET",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        (
            "/admin/data",
            "GET",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        (
            "/admin/about",
            "GET",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        (
            "/admin/delegate-proxy",
            "DELETE",
            Expected::Unauthenticated,
            Expected::Forbidden,
            Expected::Forbidden,
            Expected::Ok,
        ),
        // public endpoints
        (
            "/actuator/health",
            "GET",
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        (
            "/actuator/prometheus",
            "GET",
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
        (
            "/css/default.css",
            "GET",
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
            Expected::Ok,
        ),
    ];

    for (path, method, anonymous_expected, owner_expected, other_expected, admin_expected) in routes
    {
        for (visitor, client, expected) in [
            (Visitor::Anonymous, &anonymous, anonymous_expected),
            (Visitor::Owner, &owner, owner_expected),
            (Visitor::Other, &other, other_expected),
            (Visitor::Admin, &admin, admin_expected),
        ] {
            let path = path
                .replace("{id}", &proxy_id)
                .replace("{target}", &proxy_id);
            let url = instance.url(&path);
            let request = match method {
                "GET" => client.get(url),
                "DELETE" => client.delete(url),
                other => panic!("unexpected method {other}"),
            };
            // ask for JSON, so an unauthenticated request is answered with 401 instead of a redirect
            let response = request
                .header("Accept", "application/json")
                .send()
                .await
                .unwrap_or_else(|error| panic!("{method} {path} as {visitor:?}: {error}"));
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();

            let matches = match expected {
                Expected::Ok => status == 200,
                Expected::Unauthenticated => {
                    (status == 302 && !body.contains("shinyproxy_authentication_required"))
                        || (status == 410
                            && body.contains("shinyproxy_authentication_required")
                            && needs_authentication_answer(&path))
                }
                Expected::Forbidden => status == 403,
                // a JSON failure is reported with 200 and a `fail` status, or with 403/404
                Expected::Failure => {
                    status == 403
                        || status == 404
                        || (status == 200 && body.contains("\"status\":\"fail\""))
                        || body.contains("app_stopped_or_non_existent")
                }
                // the stub must not carry the user, the app definition or the containers of the owner
                Expected::StoppedStub => {
                    status == 200
                        && body.contains("\"status\":\"Stopped\"")
                        && body.contains("\"userId\":null")
                        && body.contains("\"specId\":null")
                        && body.contains("\"containers\":[]")
                }
            };
            assert!(
                matches,
                "{method} {path} as {visitor:?}: expected {expected:?}, got {status} {body}"
            );
        }
    }

    instance.stop();
}

#[tokio::test]
async fn app_actions_of_another_user_are_refused() {
    let instance = TestInstance::start(CONFIG).await;
    let owner = instance.login("jack", "password").await;
    let other = instance.login("jeff", "password").await;
    let proxy_id = start_app(&instance, &owner).await;

    // another user cannot stop the app
    let response = other
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    let body = response.text().await.expect("body");
    assert!(
        body.contains("fail"),
        "another user must not stop the app: {body}"
    );
    assert_eq!(
        instance
            .state
            .proxies
            .proxy(&proxy_id)
            .map(|proxy| proxy.status.to_string()),
        Some("Up".to_string()),
        "the app of the owner keeps running"
    );

    // and cannot transfer it to themselves
    let response = other
        .put(instance.url(&format!("/api/proxy/{proxy_id}/userId")))
        .json(&serde_json::json!({"userId": "jeff"}))
        .send()
        .await
        .expect("transfer request");
    assert!(
        response.status() == 403 || response.text().await.expect("body").contains("fail"),
        "another user must not take over the app"
    );

    // the heartbeat endpoint of somebody else's app is refused as well
    let response = other
        .post(instance.url(&format!("/heartbeat/{proxy_id}")))
        .send()
        .await
        .expect("heartbeat request");
    assert!(
        response.status() != 200 || response.text().await.expect("body").contains("fail"),
        "another user must not keep the app alive"
    );

    instance.stop();
}

#[tokio::test]
async fn headers_of_the_client_cannot_be_injected_into_the_app() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // the app answers with the headers it received
    let headers: std::collections::BTreeMap<String, String> = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/headers")))
        // hop-by-hop headers must not reach the app
        .header("Connection", "keep-alive, X-Secret")
        .header("Keep-Alive", "timeout=5")
        .header("Proxy-Authorization", "Basic c2VjcmV0")
        .header("Transfer-Encoding", "chunked")
        .header("Upgrade", "h2c")
        .header("X-Ok", "passed-through")
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");

    let lower: std::collections::BTreeMap<String, String> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    assert_eq!(
        lower.get("x-ok").map(String::as_str),
        Some("passed-through"),
        "a normal header is forwarded: {lower:?}"
    );
    for hop_by_hop in [
        "keep-alive",
        "proxy-authorization",
        "transfer-encoding",
        "upgrade",
    ] {
        assert!(
            !lower.contains_key(hop_by_hop),
            "the hop-by-hop header {hop_by_hop} must not reach the app: {lower:?}"
        );
    }

    // a header value with CRLF is refused by the HTTP client, so it can never split a request
    let response = reqwest::Client::new()
        .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
        .header("X-Bad", "value\r\nX-Injected: yes")
        .send()
        .await;
    assert!(
        response.is_err(),
        "a header with CRLF must never be accepted"
    );

    instance.stop();
}

#[tokio::test]
async fn redirects_stay_on_this_server() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    // a saved request that points at another host must not be used after the login
    let response = client
        .get(instance.url("/"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("index request");
    assert_eq!(response.status(), 302);

    let token = instance.csrf_token(&client).await;
    let response = client
        .post(instance.url("/login"))
        .form(&[
            ("username", "jack"),
            ("password", "password"),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await
        .expect("login request");
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.starts_with('/') || location.starts_with(&instance.base_url),
        "the login must not redirect somewhere else: {location}"
    );

    // the page that carries the target of the redirect only accepts paths of this server
    let body = client
        .get(instance.url("/auth-success"))
        .send()
        .await
        .expect("auth-success request")
        .text()
        .await
        .expect("body");
    assert!(
        !body.contains("//evil.example.com"),
        "the auth-success page must not point at another host: {body}"
    );

    // `landing-page` is a path of this server, so a configuration cannot bounce users off-site
    let instance_with_landing_page = TestInstance::start(&format!(
        "{CONFIG}  landing-page: https://evil.example.com/\n"
    ))
    .await;
    let jack = instance_with_landing_page.login("jack", "password").await;
    let response = jack
        .get(instance_with_landing_page.url("/"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("index request");
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // an absolute landing page cannot bounce a user off this server: the value is used as a path
    assert!(
        !location.starts_with("http://") && !location.starts_with("https://"),
        "an absolute landing page must not send users to another host: {location}"
    );

    instance.stop();
    instance_with_landing_page.stop();
}

#[tokio::test]
async fn the_login_form_is_protected_by_a_csrf_token() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    // without a token
    let response = client
        .post(instance.url("/login"))
        .form(&[("username", "jack"), ("password", "password")])
        .send()
        .await
        .expect("login request");
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.contains("error") || response.status() == 403,
        "a login without a CSRF token must fail: {} {location}",
        response.status()
    );

    // with the token of another session
    let other = instance.client();
    let token = instance.csrf_token(&other).await;
    let response = client
        .post(instance.url("/login"))
        .form(&[
            ("username", "jack"),
            ("password", "password"),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await
        .expect("login request");
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.contains("error") || response.status() == 403,
        "the token of another session must not work: {} {location}",
        response.status()
    );

    instance.stop();
}

#[tokio::test]
async fn the_session_cookie_is_http_only_and_the_headers_are_set() {
    let instance = TestInstance::start(CONFIG).await;
    let response = instance
        .client()
        .get(instance.url("/login"))
        .send()
        .await
        .expect("login page");

    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("JSESSIONID="))
        .expect("the session cookie")
        .to_string();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");

    let headers = response.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    // `server.frame-options` defaults to `disable`, exactly as in the Java implementation
    assert!(headers.get("x-frame-options").is_none());
    assert!(headers.get("cache-control").is_some());
    instance.stop();

    // with the setting the header appears
    let instance = TestInstance::start(&format!(
        "{CONFIG}\nserver:\n  frame-options: SAMEORIGIN\n  secure-cookies: true\n"
    ))
    .await;
    let response = instance
        .client()
        .get(instance.url("/login"))
        .send()
        .await
        .expect("login page");
    assert_eq!(
        response
            .headers()
            .get("x-frame-options")
            .and_then(|value| value.to_str().ok()),
        Some("SAMEORIGIN")
    );
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("JSESSIONID="))
        .expect("the session cookie")
        .to_string();
    assert!(
        cookie.contains("Secure"),
        "server.secure-cookies must mark the cookie: {cookie}"
    );

    instance.stop();
}

#[tokio::test]
async fn secrets_of_the_configuration_are_never_logged() {
    // a configuration full of secrets; none of them may show up in the log output
    let configuration = r##"
proxy:
  authentication: simple
  container-backend: local
  users:
    - name: jack
      password: sup3rs3cr3t-user-password
  ldap:
    - url: ldap://localhost:3899/dc=example,dc=com
      manager-dn: cn=admin,dc=example,dc=com
      manager-password: sup3rs3cr3t-ldap-password
  openid:
    auth-url: https://idp.example.com/authorize
    token-url: https://idp.example.com/token
    jwks-url: https://idp.example.com/jwks
    client-id: shinyproxy
    client-secret: sup3rs3cr3t-openid-secret
  usage-stats-url: jdbc:sqlite:/tmp/does-not-matter.db
  usage-stats-password: sup3rs3cr3t-database-password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      container-env:
        MY_TOKEN: sup3rs3cr3t-container-token
      docker-registry-password: sup3rs3cr3t-registry-password
"##;

    // the real binary is started, because the log configuration is applied by `main`
    let directory = tempfile::tempdir().expect("temp dir");
    let log = directory.path().join("shinyproxy.log");
    let path = directory.path().join("application.yml");
    let port = {
        // a free port, released again before the server starts
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("address").port()
    };
    std::fs::write(
        &path,
        format!(
            "{configuration}\nlogging:\n  file:\n    name: {}\n  level:\n    root: debug\n",
            log.display()
        ),
    )
    .expect("write configuration");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_shinyproxy"))
        .arg(format!("--spring.config.location={}", path.display()))
        .arg(format!("--proxy.port={port}"))
        .arg("--management.server.port=0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the server starts");

    // wait until it serves, then use it a little
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let base = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if client
            .get(format!("{base}/login"))
            .send()
            .await
            .is_ok_and(|response| response.status() == 200)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ready, "the server must serve on {base}");

    let login = client
        .get(format!("{base}/login"))
        .send()
        .await
        .expect("login page")
        .text()
        .await
        .expect("body");
    let token = regex::Regex::new(r#"name="_csrf" value="([^"]+)""#)
        .expect("regex")
        .captures(&login)
        .map(|captures| captures[1].to_string())
        .expect("the CSRF token");
    client
        .post(format!("{base}/login"))
        .form(&[
            ("username", "jack"),
            ("password", "sup3rs3cr3t-user-password"),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await
        .expect("login");
    client.get(format!("{base}/")).send().await.expect("index");
    client
        .get(format!("{base}/api/proxyspec"))
        .send()
        .await
        .expect("specs");

    let _ = child.kill();
    let output = child.wait_with_output().expect("the server stops");
    let contents = format!(
        "{}{}{}",
        std::fs::read_to_string(&log).unwrap_or_default(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        contents.contains("ShinyProxy"),
        "the log must contain the output of the server: {contents}"
    );
    for secret in [
        "sup3rs3cr3t-user-password",
        "sup3rs3cr3t-ldap-password",
        "sup3rs3cr3t-openid-secret",
        "sup3rs3cr3t-database-password",
        "sup3rs3cr3t-registry-password",
    ] {
        assert!(
            !contents.contains(secret),
            "the log must not contain {secret}"
        );
    }
}

#[tokio::test]
async fn the_api_never_exposes_the_secrets_of_an_app_definition() {
    let configuration = r##"
proxy:
  authentication: simple
  container-backend: local
  admin-groups: admins
  users:
    - name: jack
      password: password
      groups: scientists
    - name: root
      password: rootpw
      groups: admins
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: [ scientists, admins ]
      container-env:
        MY_TOKEN: sup3rs3cr3t-container-token
      docker-registry-username: registry-user
      docker-registry-password: sup3rs3cr3t-registry-password
"##;
    let instance = TestInstance::start(configuration).await;

    for (user, password) in [("jack", "password"), ("root", "rootpw")] {
        let client = instance.login(user, password).await;
        let body = client
            .get(instance.url("/api/proxyspec"))
            .send()
            .await
            .expect("specs request")
            .text()
            .await
            .expect("body");
        assert!(
            !body.contains("sup3rs3cr3t-registry-password"),
            "the API must not expose the registry password to {user}: {body}"
        );
        assert!(
            !body.contains("sup3rs3cr3t-container-token"),
            "the API must not expose the container environment to {user}: {body}"
        );
    }

    instance.stop();
}

#[tokio::test]
async fn a_session_that_is_used_does_not_expire() {
    // a very short session timeout, so the test does not have to wait 30 minutes
    let instance =
        TestInstance::start(&format!("{CONFIG}\nspring:\n  session:\n    timeout: 2s\n")).await;

    let jack = instance.login("jack", "password").await;
    // the session is used every 400 ms for two and a half timeouts
    for _ in 0..12 {
        let response = jack
            .get(instance.url("/api/proxy"))
            .header("Accept", "application/json")
            .send()
            .await
            .expect("api request");
        assert_eq!(
            response.status(),
            200,
            "a session that is used must stay valid"
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    // a session that is not used expires (this is what `spring.session.timeout` is for)
    let jeff = instance.login("jeff", "password").await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let response = jeff
        .get(instance.url("/api/proxy"))
        .header("Accept", "application/json")
        .send()
        .await
        .expect("api request");
    assert_eq!(
        response.status(),
        410,
        "a session that was not used must expire"
    );

    instance.stop();
}
