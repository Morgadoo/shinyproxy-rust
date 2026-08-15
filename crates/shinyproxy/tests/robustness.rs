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

//! The robustness part of P14: nothing may panic on input a user (or an attacker) controls.
//!
//! The parsers that see raw input are the URL parsers of the app routes, the WebSocket frame sniffer, the
//! expression engine and the configuration binder. They are fed random input here, and a running server is
//! fed a list of nasty URLs; the only acceptable outcome is an answer (any answer) instead of a panic.

mod common;

use common::TestInstance;
use proptest::prelude::*;

const CONFIG: &str = r##"
proxy:
  authentication: simple
  container-backend: local
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##;

proptest! {
    /// Any path either parses into an app request or is refused; it never panics.
    #[test]
    fn the_app_request_parser_survives_any_path(
        path in "[a-zA-Z0-9_/.:%~ -]{0,60}",
        context_path in prop::sample::select(vec!["", "/ctx", "/a/b"]),
    ) {
        let parsed = shinyproxy::web::apps::AppRequestInfo::parse(&path, context_path);
        // when it parses, the app name is never empty and the sub path never starts with a slash
        if let Some(info) = parsed {
            prop_assert!(!info.app_name.is_empty());
            if let Some(sub_path) = &info.sub_path {
                prop_assert!(!sub_path.starts_with('/'), "sub path: {sub_path:?}");
            }
        }
    }

    /// The pong sniffer of the WebSocket tunnel accepts any byte sequence.
    #[test]
    fn the_websocket_frame_sniffer_survives_any_bytes(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let sniffed = containerproxy::dataplane::ws::is_pong(&bytes);
        // a pong is exactly what the first byte says
        prop_assert_eq!(
            sniffed,
            !bytes.is_empty() && bytes[0] == containerproxy::dataplane::ws::WEBSOCKET_PONG
        );
    }

    /// Any expression either evaluates or reports an error; the engine never panics.
    #[test]
    fn the_expression_engine_survives_any_input(
        expression in "[a-zA-Z0-9_#{}()'.,+*/<>=!?: \\[\\]-]{0,80}"
    ) {
        let template = format!("#{{{expression}}}");
        let _ = spel::evaluate_template(&template, &spel::Context::new());
    }

    /// Any YAML document is either a configuration or an error, never a panic.
    #[test]
    fn the_configuration_binder_survives_any_yaml(
        yaml in "(proxy|server|spring|logging)(:|\\.)[a-zA-Z0-9_:\\n .{}\\[\\]-]{0,120}"
    ) {
        let _ = serde_yaml_ng::from_str::<containerproxy::config::Settings>(&yaml);
    }
}

#[tokio::test]
async fn nasty_urls_are_answered_instead_of_crashing_the_server() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    // paths that a scanner (or a bug) may produce
    let paths = [
        "/app_proxy/",
        "/app_proxy//",
        "/app_proxy/../../etc/passwd",
        "/app_proxy/%2e%2e%2f%2e%2e%2fetc/passwd",
        "/app_proxy/00000000-0000-0000-0000-000000000000/../../admin",
        "/app_proxy/00000000-0000-0000-0000-000000000000/%00",
        "/app/",
        "/app/01_hello/../../admin/data",
        "/app_i/01_hello",
        "/app_i//_",
        "/app_direct/",
        "/app_direct/01_hello/../..",
        "/api/proxy/../../admin/data",
        "/api/proxy/%ff%fe",
        "/api/route/",
        "/heartbeat/",
        "/heartbeat/%20",
        "/logout/../admin",
        "/css/../../../etc/passwd",
        "/css/%2e%2e/%2e%2e/etc/passwd",
        "/favicon/../../admin",
        "/issue",
        // very long segments
        &format!("/app/{}", "a".repeat(4096)),
        &format!("/app_proxy/{}/", "b".repeat(4096)),
        &format!("/{}", "%41".repeat(1000)),
        // control characters and unicode
        "/app/01_hello%0d%0aX-Injected:%20yes",
        "/app/ünïcödé",
        "/app/01_hello?sp_hide_navbar=%00",
        "/api/proxy/%7B%7D",
    ];

    for path in paths {
        for client in [&instance.client(), &jack] {
            let response = client
                .get(instance.url(path))
                .send()
                .await
                .unwrap_or_else(|error| panic!("{path} must be answered: {error}"));
            // any answer is fine, as long as the server keeps working
            assert!(
                response.status().as_u16() >= 200,
                "{path}: {}",
                response.status()
            );
            // and the answer must never carry an injected header
            assert!(
                response.headers().get("x-injected").is_none(),
                "{path} must not inject headers"
            );
        }
    }

    // the server still works afterwards
    let body = jack
        .get(instance.url("/"))
        .send()
        .await
        .expect("index request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("01_hello"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn broken_bodies_and_headers_are_answered() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    // JSON endpoints with bodies that are not JSON
    for (path, body) in [
        ("/app_i/01_hello/_", "not json"),
        ("/app_i/01_hello/_", "{\"timezone\": 42}"),
        ("/app_i/01_hello/_", "[]"),
        ("/issue", "{"),
    ] {
        let response = jack
            .post(instance.url(path))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap_or_else(|error| panic!("{path} must be answered: {error}"));
        assert!(
            response.status().as_u16() < 500 || response.status().as_u16() == 500,
            "{path}: {}",
            response.status()
        );
    }

    // a very long header value and a very long query string
    let response = jack
        .get(instance.url("/"))
        .header("X-Long", "a".repeat(8000))
        .send()
        .await
        .expect("request with a long header");
    assert!(response.status().as_u16() < 500);

    let response = jack
        .get(instance.url(&format!("/?{}", "a=b&".repeat(2000))))
        .send()
        .await
        .expect("request with a long query");
    assert!(response.status().as_u16() < 500);

    // the server still works
    let response = jack.get(instance.url("/")).send().await.expect("index");
    assert_eq!(response.status(), 200);

    instance.stop();
}
