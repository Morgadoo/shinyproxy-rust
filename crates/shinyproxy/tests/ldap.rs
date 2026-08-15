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

//! LDAP authentication against a real directory.
//!
//! The test needs an LDAP server with the fixtures of `scripts/start-test-ldap.sh` (which starts an
//! OpenLDAP container and seeds it); it is skipped unless `SP_TEST_LDAP=1` is set. The directory contains:
//!
//! * `uid=jack,ou=people,dc=example,dc=com` (password `password`), member of `scientists` and `admins`,
//! * `uid=jeff,ou=people,dc=example,dc=com` (password `password`), member of nothing.
//!
//! Replaces the Java LDAP integration test.

mod common;

use common::TestInstance;

/// Whether the LDAP tests are enabled.
fn enabled() -> bool {
    std::env::var("SP_TEST_LDAP").as_deref() == Ok("1")
}

/// The URL of the test directory (override with `SP_TEST_LDAP_URL`).
fn url() -> String {
    std::env::var("SP_TEST_LDAP_URL")
        .unwrap_or_else(|_| "ldap://127.0.0.1:3899/dc=example,dc=com".to_string())
}

/// A configuration that authenticates against the test directory.
fn config(extra: &str) -> String {
    format!(
        r##"
proxy:
  title: LDAP Test
  authentication: ldap
  admin-groups: admins
  container-backend: local
  ldap:
    url: {url}
    manager-dn: cn=admin,dc=example,dc=com
    manager-password: admin
    group-search-base: ou=groups
    group-search-filter: (member={{0}})
{extra}
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
        url = url()
    )
}

#[tokio::test]
async fn authenticates_with_a_user_dn_pattern() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_LDAP=1 (and start the directory) to run the LDAP tests");
        return;
    }

    let instance = TestInstance::start(&config("    user-dn-pattern: uid={0},ou=people\n")).await;

    // the wrong password is refused
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
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login?error=true")
    );

    // jack is a member of scientists and admins, so every app and the admin page are available
    let jack = instance.login("jack", "password").await;
    let body = jack
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(body.contains("Admin Application"), "{body}");
    assert!(body.contains("href=\"/admin\""), "{body}");
    assert!(body.contains("jack"), "{body}");

    // jeff is a member of nothing, so no app is available
    let jeff = instance.login("jeff", "password").await;
    let body = jeff
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("There are no apps available for you."),
        "{body}"
    );
    assert!(!body.contains("href=\"/admin\""), "{body}");

    instance.stop();
}

#[tokio::test]
async fn authenticates_with_a_user_search() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_LDAP=1 (and start the directory) to run the LDAP tests");
        return;
    }

    // no user-dn-pattern: the DN is looked up with the manager account
    let instance = TestInstance::start(&config(
        "    user-search-base: ou=people\n    user-search-filter: (uid={0})\n",
    ))
    .await;

    let jack = instance.login("jack", "password").await;
    let body = jack
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(body.contains("Admin Application"), "{body}");

    // a user that does not exist is refused
    let client = instance.client();
    let token = instance.csrf_token(&client).await;
    let response = client
        .post(instance.url("/login"))
        .form(&[
            ("username", "nobody"),
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
        Some("/login?error=true")
    );

    instance.stop();
}

#[tokio::test]
async fn an_unreachable_directory_refuses_the_login() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_LDAP=1 to run the LDAP tests");
        return;
    }

    let instance = TestInstance::start(
        r##"
proxy:
  authentication: ldap
  container-backend: local
  ldap:
    url: ldap://127.0.0.1:1/dc=example,dc=com
    user-dn-pattern: uid={0},ou=people
  specs: []
"##,
    )
    .await;

    let client = instance.client();
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
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login?error=true"),
        "an unreachable directory must not authenticate anyone"
    );

    instance.stop();
}
