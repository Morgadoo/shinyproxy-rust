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

//! Parity with the Java ShinyProxy 3.2.4, checked by `cargo test` without a JVM.
//!
//! `crates/shinyproxy/tests/fixtures/parity/java-3.2.4.json` holds the answers the Java implementation gave to
//! every scenario in `shinyproxy::parity::SCENARIOS` (recorded with
//! `cargo run -p shinyproxy --example record-parity`, see `docs/TESTING.md`). This test asks the same
//! questions of this implementation, normalises the answers the same way and compares them.
//!
//! A difference fails the test unless it is listed in `shinyproxy::parity::ACCEPTED_DIFFERENCES` — so a
//! regression in the HTTP contract cannot pass unnoticed, and every accepted deviation is documented in one
//! place (`docs/COMPATIBILITY.md` explains them for users).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{TestClient, TestInstance};
use shinyproxy::parity::{
    is_accepted, normalise_body, normalise_headers, Answer, As, Fixture, Scenario, PARITY_CONFIG,
    SCENARIOS,
};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parity/java-3.2.4.json")
}

/// The recorded answers of the Java implementation.
fn java_fixture() -> Fixture {
    let path = fixture_path();
    let document = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} must exist (record it with `cargo run -p shinyproxy --example record-parity`): {error}",
            path.display()
        )
    });
    serde_json::from_str(&document).expect("the fixture is valid JSON")
}

/// Asks this implementation the same question and normalises the answer the same way.
async fn ask(instance: &TestInstance, clients: &Clients, scenario: &Scenario) -> Answer {
    let client = match scenario.who {
        As::Anonymous => &clients.anonymous,
        As::User => &clients.user,
        As::Admin => &clients.admin,
    };
    let url = instance.url(scenario.path);
    let mut request = match scenario.method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        _ => client.get(url),
    };
    if let Some(accept) = scenario.accept {
        request = request.header("Accept", accept);
    }
    if let Some(body) = scenario.body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }

    let response = request
        .send()
        .await
        .unwrap_or_else(|error| panic!("{} must be answered: {error}", scenario.name));
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let headers = normalise_headers(
        response
            .headers()
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value))),
    );
    let body = response.bytes().await.expect("a body");

    Answer {
        status,
        headers,
        body: normalise_body(&content_type, &body),
    }
}

/// The three clients the scenarios use.
struct Clients {
    anonymous: TestClient,
    user: TestClient,
    admin: TestClient,
}

#[tokio::test]
async fn the_answers_match_the_java_implementation() {
    let java = java_fixture();
    // the recorded configuration, with the backend the test environment has (no app is started here)
    let configuration =
        PARITY_CONFIG.replace("container-backend: docker", "container-backend: local");
    let instance = TestInstance::start(&configuration).await;
    let clients = Clients {
        anonymous: instance.client(),
        user: instance.login("jack", "password").await,
        admin: instance.login("root", "rootpw").await,
    };

    let mut differences: Vec<String> = Vec::new();
    let mut accepted = 0;
    let mut compared = 0;

    for scenario in SCENARIOS {
        let Some(expected) = java.answers.get(scenario.name) else {
            differences.push(format!(
                "{}: the fixture has no recorded answer (re-record it)",
                scenario.name
            ));
            continue;
        };
        let actual = ask(&instance, &clients, scenario).await;
        compared += 1;

        if actual.status != expected.status {
            if is_accepted(scenario.name, "status") {
                accepted += 1;
            } else {
                differences.push(format!(
                    "{}: status {} instead of {}",
                    scenario.name, actual.status, expected.status
                ));
            }
        }

        // the headers of the contract, compared name by name
        let names: std::collections::BTreeSet<&String> = expected
            .headers
            .keys()
            .chain(actual.headers.keys())
            .collect();
        for name in names {
            let expected_value = expected.headers.get(name);
            let actual_value = actual.headers.get(name);
            if expected_value == actual_value {
                continue;
            }
            if is_accepted(scenario.name, name) {
                accepted += 1;
                continue;
            }
            differences.push(format!(
                "{}: header {name} is {:?} instead of {:?}",
                scenario.name, actual_value, expected_value
            ));
        }

        if actual.body != expected.body {
            if is_accepted(scenario.name, "body") {
                accepted += 1;
            } else {
                differences.push(format!(
                    "{}: the body differs\n--- java\n{}\n--- rust\n{}\n",
                    scenario.name, expected.body, actual.body
                ));
            }
        }
    }

    instance.stop();

    assert!(
        compared >= SCENARIOS.len(),
        "every scenario must be compared ({compared} of {})",
        SCENARIOS.len()
    );
    assert!(
        differences.is_empty(),
        "{} difference(s) with {} ({} accepted deviations):\n\n{}",
        differences.len(),
        java.implementation,
        accepted,
        differences.join("\n")
    );
}

#[tokio::test]
async fn the_fixture_covers_the_whole_documented_surface() {
    let java = java_fixture();

    // the fixture must not contain scenarios that no longer exist, and vice versa
    let recorded: std::collections::BTreeSet<&str> =
        java.answers.keys().map(String::as_str).collect();
    let defined: std::collections::BTreeSet<&str> =
        SCENARIOS.iter().map(|scenario| scenario.name).collect();
    let missing: Vec<&&str> = defined.difference(&recorded).collect();
    let extra: Vec<&&str> = recorded.difference(&defined).collect();
    assert!(
        missing.is_empty(),
        "the fixture is missing {missing:?}; re-record it with `cargo run -p shinyproxy --example \
         record-parity`"
    );
    assert!(
        extra.is_empty(),
        "the fixture has answers for scenarios that no longer exist: {extra:?}"
    );

    // the surface that matters is covered: pages, the API, the proxy paths, the actuator and the assets
    let paths: Vec<&str> = SCENARIOS.iter().map(|scenario| scenario.path).collect();
    for prefix in [
        "/login",
        "/",
        "/app/",
        "/admin",
        "/api/proxyspec",
        "/api/proxy",
        "/app_proxy/",
        "/api/route/",
        "/heartbeat/",
        "/app_i/",
        "/issue",
        "/actuator/",
        "/css/",
        "/favicon",
    ] {
        assert!(
            paths.iter().any(|path| path.starts_with(prefix)),
            "no scenario covers {prefix}"
        );
    }

    // every kind of visitor is exercised
    for who in [As::Anonymous, As::User, As::Admin] {
        assert!(
            SCENARIOS.iter().any(|scenario| scenario.who == who),
            "no scenario asks as {who:?}"
        );
    }

    // and the accepted deviations are all documented in COMPATIBILITY.md
    let compatibility = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/COMPATIBILITY.md"),
    )
    .expect("docs/COMPATIBILITY.md exists");
    for (scenario, field, _) in shinyproxy::parity::ACCEPTED_DIFFERENCES {
        let mentioned = match *field {
            "set-cookie" => compatibility.contains("cookie"),
            "content-type" => compatibility.contains("application/json"),
            "body" if *scenario == "about-page" => compatibility.contains("/admin/about"),
            other => compatibility.contains(other),
        };
        assert!(
            mentioned,
            "the accepted difference ({scenario}, {field}) is not documented in COMPATIBILITY.md"
        );
    }
}

#[tokio::test]
async fn the_recorded_answers_are_normalised() {
    // nothing that differs per run may be left in the fixture, otherwise the comparison is flaky
    let java = java_fixture();
    let document = serde_json::to_string(&java).expect("json");

    for pattern in [
        // a raw uuid, a raw sha1, a raw epoch millisecond timestamp or a host with a port
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
        r"\b[0-9a-f]{40}\b",
        r"\b1[0-9]{12}\b",
        r"127\.0\.0\.1:\d+",
    ] {
        let regex = regex::Regex::new(pattern).expect("regex");
        // the timestamp of the recording is allowed to be a date
        let answers: BTreeMap<&String, &Answer> = java.answers.iter().collect();
        for (name, answer) in answers {
            let text = serde_json::to_string(answer).expect("json");
            assert!(
                !regex.is_match(&text),
                "the recorded answer of {name} still contains {pattern}: {text}"
            );
        }
    }
    assert!(!document.is_empty());
}
