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

//! Snapshots of the rendered pages.
//!
//! These lock down the HTML of every page, so that a change to a template or to the model that feeds it
//! shows up as a reviewable diff instead of a surprise in the browser. The instance id and the ports of
//! the test server are replaced by placeholders, because they change on every run.
//!
//! Update them with `cargo insta review` (or `INSTA_UPDATE=always cargo test`) after checking the diff.

mod common;

use common::{TestClient, TestInstance};

/// Replaces the values that change on every run.
fn normalise(instance: &TestInstance, html: &str) -> String {
    let host = instance.base_url.trim_start_matches("http://").to_string();
    html.replace(&instance.state.identifiers.instance_id, "{instance-id}")
        .replace(&instance.state.identifiers.runtime_id, "{runtime-id}")
        .replace(&host, "{host}")
        // proxy ids and CSRF tokens are UUIDs/random strings
        .replace('\r', "")
}

/// Fetches a page and normalises it; `expected_status` is the status the page answers with.
async fn page_with_status(
    instance: &TestInstance,
    client: &TestClient,
    path: &str,
    expected_status: u16,
) -> String {
    let response = client
        .get(instance.url(path))
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(
        response.status().as_u16(),
        expected_status,
        "unexpected status for {path}"
    );
    let body = response.text().await.expect("body");
    normalise(instance, &body)
}

/// Fetches a page that answers with 200.
async fn page(instance: &TestInstance, client: &TestClient, path: &str) -> String {
    page_with_status(instance, client, path, 200).await
}

/// A configuration with two apps, a group, an admin and a description.
const CONFIG: &str = r##"
proxy:
  title: My ShinyProxy
  logo-url: https://www.openanalytics.eu/shinyproxy/logo.png
  authentication: simple
  admin-groups: admins
  container-backend: local
  notification-message: "Read the <b>news</b>!"
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
      description: A demo with <b>markup</b>
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: scientists
    - id: 02_admin_only
      display-name: Admin Application
      container-image: sp-testapp
      access-groups: admins
"##;

#[tokio::test]
async fn login_page() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();
    let html = page(&instance, &client, "/login").await;
    // the CSRF token changes on every run
    let html = regex::Regex::new(r#"value="[0-9a-zA-Z_\-]{16,}""#)
        .expect("regex")
        .replace_all(&html, r#"value="{csrf-token}""#)
        .to_string();
    insta::assert_snapshot!("login_page", html);
    instance.stop();
}

#[tokio::test]
async fn index_page() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;
    insta::assert_snapshot!("index_page", page(&instance, &client, "/").await);
    instance.stop();
}

#[tokio::test]
async fn index_page_for_an_administrator() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("root", "rootpw").await;
    insta::assert_snapshot!("index_page_admin", page(&instance, &client, "/").await);
    instance.stop();
}

#[tokio::test]
async fn index_page_with_inline_my_apps_and_groups() {
    let instance = TestInstance::start(
        r##"
proxy:
  title: Grouped
  authentication: simple
  container-backend: local
  my-apps-mode: Inline
  body-classes: [ dark, compact ]
  hide-navbar: false
  users:
    - name: jack
      password: password
  template-groups:
    - id: reporting
      properties:
        display-name: Reporting
    - id: analysis
      properties:
        display-name: Analysis
  specs:
    - id: 01_hello
      display-name: Hello
      template-group: reporting
      container-image: sp-testapp
    - id: 02_other
      display-name: Other
      template-group: analysis
      container-image: sp-testapp
    - id: 03_ungrouped
      display-name: Ungrouped
      container-image: sp-testapp
"##,
    )
    .await;
    let client = instance.login("jack", "password").await;
    insta::assert_snapshot!(
        "index_page_inline_groups",
        page(&instance, &client, "/").await
    );
    instance.stop();
}

#[tokio::test]
async fn app_page() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("jack", "password").await;
    insta::assert_snapshot!("app_page", page(&instance, &client, "/app/01_hello").await);
    instance.stop();
}

#[tokio::test]
async fn app_page_with_parameters_and_hidden_navbar() {
    let instance = TestInstance::start(
        r##"
proxy:
  title: Parameters
  authentication: simple
  container-backend: local
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      display-name: Hello
      container-image: sp-testapp
      parameters:
        definitions:
          - id: dataset
            display-name: Dataset
            description: Which data to use
        value-sets:
          - values:
              dataset: [ public, private ]
"##,
    )
    .await;
    let client = instance.login("jack", "password").await;
    insta::assert_snapshot!(
        "app_page_parameters",
        page(&instance, &client, "/app/01_hello?sp_hide_navbar=true").await
    );
    instance.stop();
}

#[tokio::test]
async fn admin_pages() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.login("root", "rootpw").await;
    insta::assert_snapshot!("admin_page", page(&instance, &client, "/admin").await);

    // the About page contains the build information of this binary, which changes with every build
    let html = page(&instance, &client, "/admin/about").await;
    let html = regex::Regex::new(
        r#"(?m)^(\s*)<td class="admin-monospace">(?:0\.1\.0 \(rustc|--spring|\d+ (?:bytes|KB|MB|GB)).*$"#,
    )
    .expect("regex")
    .replace_all(&html, r#"$1<td class="admin-monospace">{build-information}</td>"#)
    .to_string();
    insta::assert_snapshot!("admin_about_page", html);
    instance.stop();
}

#[tokio::test]
async fn error_pages() {
    let instance = TestInstance::start(CONFIG).await;
    let client = instance.client();

    // the login page with an error
    insta::assert_snapshot!(
        "login_page_expired",
        regex::Regex::new(r#"value="[0-9a-zA-Z_\-]{16,}""#)
            .expect("regex")
            .replace_all(
                &page(&instance, &client, "/login?error=expired").await,
                r#"value="{csrf-token}""#
            )
            .to_string()
    );

    insta::assert_snapshot!(
        "auth_error_page",
        page(&instance, &client, "/auth-error").await
    );
    insta::assert_snapshot!(
        "logout_success_page",
        page(&instance, &client, "/logout-success").await
    );
    // the "no access to this app" page answers with 403, like the Java implementation
    insta::assert_snapshot!(
        "app_access_denied_page",
        page_with_status(&instance, &client, "/app-access-denied", 403).await
    );

    instance.stop();
}
