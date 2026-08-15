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

//! End-to-end tests of the user interface: authentication, the index page, assets and security.
//!
//! These replace the Java `IndexControllerTest` for the parts that do not need a running app
//! (starting apps follows in P5/P6).

mod common;

use common::TestInstance;

const SIMPLE_CONFIG: &str = r#"
proxy:
  title: Test Proxy
  authentication: simple
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
      display-name: Hello Application
      description: 'Demo with <b>markup</b><script>alert(1)</script>'
      container-image: sp-testapp
      access-groups: [ scientists, admins ]
    - id: 02_admin_only
      container-image: sp-testapp
      access-groups: admins
"#;

const ANONYMOUS_CONFIG: &str = r#"
proxy:
  authentication: none
  specs:
    - id: 01_hello
      container-image: sp-testapp
"#;

#[tokio::test]
async fn unauthenticated_requests_are_redirected_to_the_login_page() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.client();

    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );
    // a session cookie is created for every request, as in the Java implementation
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(cookies.contains("JSESSIONID"), "cookies were: {cookies}");
    assert!(cookies.contains("HttpOnly"), "cookies were: {cookies}");
    assert!(cookies.contains("SameSite=Lax"), "cookies were: {cookies}");

    instance.stop();
}

#[tokio::test]
async fn login_page_is_public_and_contains_a_csrf_token() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.client();

    let response = client
        .get(instance.url("/login"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(body.contains("Please sign in:"), "{body}");
    assert!(body.contains("name=\"_csrf\""), "{body}");
    assert!(body.contains("<title>Test Proxy</title>"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn wrong_credentials_are_rejected() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
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
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login?error=true")
    );

    // the index page is still unreachable
    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 303);

    instance.stop();
}

#[tokio::test]
async fn missing_csrf_token_shows_the_expired_message() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.client();
    // no visit to /login, so the session has no token
    let response = client
        .post(instance.url("/login"))
        .form(&[("username", "jack"), ("password", "password")])
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login?error=expired")
    );

    let body = client
        .get(instance.url("/login?error=expired"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Your session has expired"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn successful_login_renders_the_accessible_apps() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.login("jack", "password").await;

    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");

    // apps the user may access
    assert!(body.contains("Hello Application"), "{body}");
    assert!(body.contains("data-app-id=\"01_hello\""), "{body}");
    assert!(body.contains("data-app-url=\"/app/01_hello\""), "{body}");
    // apps of other groups are not listed (the id still appears in the max-instances map that the
    // front-end receives, exactly like in the Java implementation)
    assert!(!body.contains("data-app-id=\"02_admin_only\""), "{body}");
    // descriptions are sanitised but keep basic markup
    assert!(body.contains("Demo with <b>markup</b>"), "{body}");
    assert!(!body.contains("alert(1)"), "{body}");
    // the navbar shows the user and no admin button
    assert!(body.contains("jack"), "{body}");
    assert!(!body.contains("href=\"/admin\""), "{body}");
    // the front-end is initialised with the model the JavaScript expects
    assert!(
        body.contains("window.Shiny.common.init(\"/\", \"ShinyProxy\","),
        "{body}"
    );
    assert!(body.contains("\"01_hello\":1"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn auth_success_page_redirects_with_an_absolute_url() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.client();
    let token = instance.csrf_token(&client).await;

    // a user who wanted to open an app is sent there after logging in (a browser navigation, i.e. a GET
    // that asks for HTML)
    let response = client
        .get(instance.url("/app/01_hello"))
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);

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
        .expect("redirect")
        .to_string();
    assert_eq!(location, "/auth-success?continue=%2Fapp%2F01_hello");

    // background requests of the browser must not become the page shown after login (Chrome asks for
    // /.well-known/appspecific/com.chrome.devtools.json with Accept: */* while DevTools is open)
    let background = instance.client();
    let token = instance.csrf_token(&background).await;
    background
        .get(instance.url("/app/01_hello"))
        .header("accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .expect("request");
    background
        .get(instance.url("/.well-known/appspecific/com.chrome.devtools.json"))
        .header("accept", "*/*")
        .send()
        .await
        .expect("request");
    let response = background
        .post(instance.url("/login"))
        .form(&[
            ("username", "jack"),
            ("password", "password"),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await
        .expect("login request");
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/auth-success?continue=%2Fapp%2F01_hello"),
        "the navigation is remembered, not the background request"
    );

    let host = instance.base_url.trim_start_matches("http://").to_string();
    let body = client
        .get(instance.url("/auth-success?continue=%2Fapp%2F01_hello"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    // the page redirects with `new URL(...)`, which throws for a path, so the URL must be absolute
    // (the Java implementation renders an absolute URL as well)
    let expected = format!("http://{host}/app/01_hello");
    assert!(body.contains(&format!("new URL(\"{expected}\")")), "{body}");
    assert!(
        body.contains(&format!("window.location.href = \"{expected}\"")),
        "{body}"
    );

    // an external redirect target is replaced by the main page
    let body = client
        .get(instance.url("/auth-success?continue=https%3A%2F%2Fevil.example.com%2F"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains(&format!("http://{host}/")), "{body}");
    assert!(!body.contains("evil.example.com"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn administrators_see_all_apps_and_the_admin_button() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.login("root", "rootpw").await;

    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("data-app-id=\"01_hello\""), "{body}");
    assert!(body.contains("data-app-id=\"02_admin_only\""), "{body}");
    assert!(body.contains("href=\"/admin\""), "{body}");

    instance.stop();
}

#[tokio::test]
async fn admin_pages_are_forbidden_for_normal_users() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;

    let client = instance.login("jack", "password").await;
    let response = client
        .get(instance.url("/admin"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);

    // the admin page itself is not implemented yet (P7), but the authorization already applies
    let client = instance.login("root", "rootpw").await;
    let response = client
        .get(instance.url("/admin"))
        .send()
        .await
        .expect("request");
    assert_ne!(response.status(), 403);

    instance.stop();
}

#[tokio::test]
async fn logout_clears_the_session() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.login("jack", "password").await;

    let response = client
        .get(instance.url("/logout"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/logout-success")
    );

    let body = client
        .get(instance.url("/logout-success"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("You have been logged out successfully."),
        "{body}"
    );

    // the index page requires authentication again
    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 303);

    instance.stop();
}

#[tokio::test]
async fn anonymous_access_without_authentication() {
    let instance = TestInstance::start(ANONYMOUS_CONFIG).await;
    let client = instance.client();

    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(body.contains("data-app-id=\"01_hello\""), "{body}");
    // no sign out button and no user name, because nobody is logged in
    assert!(!body.contains("Sign Out"), "{body}");

    // the login page redirects to the index page for backends without a login form
    let response = client
        .get(instance.url("/login"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);

    instance.stop();
}

#[tokio::test]
async fn assets_are_served_with_and_without_the_instance_prefix() {
    let instance = TestInstance::start(ANONYMOUS_CONFIG).await;
    let client = instance.client();
    let instance_id = instance.state.identifiers.instance_id.clone();

    for path in [
        "/js/shiny.common.js".to_string(),
        "/css/default.css".to_string(),
        "/css/bootstrap.css".to_string(),
        "/webjars/jquery/3.7.1/jquery.min.js".to_string(),
        "/handlebars/precompiled.js".to_string(),
        format!("/{instance_id}/js/shiny.app.js"),
    ] {
        let response = client
            .get(instance.url(&path))
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 200, "{path}");
    }

    // only the instance-prefixed variant is cached for a long time
    let response = client
        .get(instance.url(&format!("/{instance_id}/js/shiny.app.js")))
        .send()
        .await
        .expect("request");
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=31536000, immutable")
    );
    let response = client
        .get(instance.url("/js/shiny.app.js"))
        .send()
        .await
        .expect("request");
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-cache")
    );

    // unknown assets produce the error page
    let response = client
        .get(instance.url("/js/does-not-exist.js"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 404);

    instance.stop();
}

#[tokio::test]
async fn favicons_follow_the_java_behaviour() {
    let directory = tempfile::tempdir().expect("temp dir");
    let global = directory.path().join("global.png");
    let app_icon = directory.path().join("app.png");
    std::fs::write(&global, b"\x89PNG\r\n\x1a\nglobal").expect("write");
    std::fs::write(&app_icon, b"\x89PNG\r\n\x1a\napp").expect("write");

    let instance = TestInstance::start(&format!(
        r#"
proxy:
  authentication: none
  favicon-path: {}
  specs:
    - id: with_icon
      container-image: sp-testapp
      favicon-path: {}
    - id: without_icon
      container-image: sp-testapp
"#,
        global.display(),
        app_icon.display()
    ))
    .await;
    let client = instance.client();
    let instance_id = instance.state.identifiers.instance_id.clone();

    // the global favicon
    for path in [
        "/favicon.ico".to_string(),
        format!("/{instance_id}/favicon"),
    ] {
        let response = client
            .get(instance.url(&path))
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            "max-age=86400"
        );
        assert!(
            response.bytes().await.unwrap().ends_with(b"global"),
            "{path}"
        );
    }

    // the favicon of an app, falling back to the global one
    let response = client
        .get(instance.url(&format!("/{instance_id}/favicon/with_icon")))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    assert!(response.bytes().await.unwrap().ends_with(b"app"));

    let response = client
        .get(instance.url(&format!("/{instance_id}/favicon/without_icon")))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    assert!(response.bytes().await.unwrap().ends_with(b"global"));

    // unknown apps are forbidden, and the answer has no body (browsers must not parse it)
    let response = client
        .get(instance.url(&format!("/{instance_id}/favicon/unknown")))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);
    assert!(response.text().await.unwrap().is_empty());

    instance.stop();
}

#[tokio::test]
async fn security_headers_are_present() {
    let instance = TestInstance::start(ANONYMOUS_CONFIG).await;
    let client = instance.client();

    let response = client.get(instance.url("/")).send().await.expect("request");
    let headers = response.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert!(headers.contains_key("strict-transport-security"));
    assert!(headers.contains_key("x-xss-protection"));
    // ShinyProxy pages must not be cached
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-cache, no-store, max-age=0, must-revalidate")
    );

    instance.stop();
}

#[tokio::test]
async fn context_path_is_honoured() {
    let instance = TestInstance::start(
        r#"
proxy:
  authentication: none
  specs:
    - id: 01_hello
      container-image: sp-testapp
server:
  servlet:
    context-path: /shinyproxy
"#,
    )
    .await;
    let client = instance.client();

    // the bare context path redirects to the index page
    let response = client
        .get(instance.url("/shinyproxy"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/shinyproxy/")
    );

    let response = client
        .get(instance.url("/shinyproxy/"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("data-app-url=\"/shinyproxy/app/01_hello\""),
        "{body}"
    );
    // assets are referenced through the instance id prefix, below the context path
    let instance_id = instance.state.identifiers.instance_id.clone();
    assert!(
        body.contains(&format!("/shinyproxy/{instance_id}/js/shiny.common.js")),
        "{body}"
    );

    for path in [
        "/shinyproxy/js/shiny.common.js".to_string(),
        format!("/shinyproxy/{instance_id}/js/shiny.common.js"),
    ] {
        let response = client
            .get(instance.url(&path))
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), 200, "{path}");
    }

    // outside the context path nothing is served
    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 404);

    instance.stop();
}

#[tokio::test]
async fn landing_page_can_redirect_to_an_app() {
    let instance = TestInstance::start(
        r#"
proxy:
  authentication: none
  landing-page: SingleApp
  specs:
    - id: only_app
      container-image: sp-testapp
"#,
    )
    .await;
    let client = instance.client();

    let response = client.get(instance.url("/")).send().await.expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/app/only_app")
    );

    instance.stop();
}

#[tokio::test]
async fn api_requests_of_unauthenticated_users_get_json() {
    let instance = TestInstance::start(SIMPLE_CONFIG).await;
    let client = instance.client();

    let response = client
        .get(instance.url("/api/proxy"))
        .header("Accept", "application/json")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 401);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["status"], "fail");
    assert_eq!(json["message"], "shinyproxy_authentication_required");

    instance.stop();
}
