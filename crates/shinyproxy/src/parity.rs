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

//! The scenarios that compare this implementation with the Java one, and how their answers are normalised.
//!
//! The same code is used twice:
//!
//! * `cargo run -p shinyproxy --example record-parity -- --base-url http://127.0.0.1:8091` records the
//!   answers of a *running* server (the Java jar) into
//!   `crates/shinyproxy/tests/fixtures/parity/java-3.2.4.json`;
//! * `crates/shinyproxy/tests/parity.rs` replays the same scenarios against this implementation and compares
//!   them with the recorded answers.
//!
//! That way the parity with ShinyProxy 3.2.4 is checked by `cargo test`, without a JVM and without Docker.
//! The differences that are accepted (and documented in `docs/COMPATIBILITY.md`) are listed in
//! [`ACCEPTED_DIFFERENCES`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The configuration both implementations are started with when the fixture is recorded.
///
/// It only uses features both implementations have, and no app definition needs a container runtime for the
/// scenarios in [`SCENARIOS`].
pub const PARITY_CONFIG: &str = r##"
proxy:
  title: Parity
  port: 8080
  authentication: simple
  admin-groups: admins
  allow-transfer-app: true
  hide-navbar: false
  container-backend: docker
  container-wait-timeout: 20000
  heartbeat-rate: 10000
  heartbeat-timeout: 60000
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
      description: Application which demonstrates the basics of a Shiny app
      container-image: openanalytics/shinyproxy-demo
      container-cmd: [ "R", "-e", "shinyproxy::run_01_hello()" ]
      port: 3838
      access-groups: [ scientists, admins ]
    - id: 02_admin_only
      display-name: Admin Application
      container-image: openanalytics/shinyproxy-demo
      port: 3838
      access-groups: admins
"##;

/// Who makes the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum As {
    /// Nobody is logged in.
    Anonymous,
    /// `jack`, a member of `scientists`.
    User,
    /// `root`, an administrator.
    Admin,
}

/// One request that both implementations answer.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Name in the fixture.
    pub name: &'static str,
    /// HTTP method.
    pub method: &'static str,
    /// Path, without the base URL.
    pub path: &'static str,
    /// Who asks.
    pub who: As,
    /// `Accept` header, when the scenario needs one.
    pub accept: Option<&'static str>,
    /// Request body (JSON), when the scenario has one.
    pub body: Option<&'static str>,
}

impl Scenario {
    const fn get(name: &'static str, path: &'static str, who: As) -> Scenario {
        Scenario {
            name,
            method: "GET",
            path,
            who,
            accept: None,
            body: None,
        }
    }

    const fn json(name: &'static str, path: &'static str, who: As) -> Scenario {
        Scenario {
            name,
            method: "GET",
            path,
            who,
            accept: Some("application/json"),
            body: None,
        }
    }

    const fn post(name: &'static str, path: &'static str, who: As, body: &'static str) -> Scenario {
        Scenario {
            name,
            method: "POST",
            path,
            who,
            accept: Some("application/json"),
            body: Some(body),
        }
    }

    const fn put(name: &'static str, path: &'static str, who: As, body: &'static str) -> Scenario {
        Scenario {
            name,
            method: "PUT",
            path,
            who,
            accept: Some("application/json"),
            body: Some(body),
        }
    }
}

/// Every request that is compared.
///
/// None of them starts a container, so the fixture can be replayed by `cargo test` on any machine.
pub const SCENARIOS: &[Scenario] = &[
    // the pages
    Scenario::get("login-page", "/login", As::Anonymous),
    Scenario::get("login-page-expired", "/login?error=expired", As::Anonymous),
    Scenario::get("index-anonymous", "/", As::Anonymous),
    Scenario::get("index-user", "/", As::User),
    Scenario::get("index-admin", "/", As::Admin),
    Scenario::get("app-page", "/app/01_hello", As::User),
    Scenario::get("app-page-no-access", "/app/02_admin_only", As::User),
    Scenario::get("app-page-unknown", "/app/does_not_exist", As::User),
    Scenario::get("admin-page-user", "/admin", As::User),
    Scenario::get("admin-page-admin", "/admin", As::Admin),
    Scenario::get("about-page", "/admin/about", As::Admin),
    Scenario::get("logout-success-page", "/logout-success", As::Anonymous),
    Scenario::get("auth-error-page", "/auth-error", As::Anonymous),
    // the API
    Scenario::json("api-specs-anonymous", "/api/proxyspec", As::Anonymous),
    Scenario::json("api-specs-user", "/api/proxyspec", As::User),
    Scenario::json("api-specs-admin", "/api/proxyspec", As::Admin),
    Scenario::json("api-spec-user", "/api/proxyspec/01_hello", As::User),
    Scenario::json(
        "api-spec-no-access",
        "/api/proxyspec/02_admin_only",
        As::User,
    ),
    Scenario::json("api-spec-unknown", "/api/proxyspec/nope", As::User),
    Scenario::json("api-proxies-anonymous", "/api/proxy", As::Anonymous),
    Scenario::json("api-proxies-user", "/api/proxy", As::User),
    Scenario::json("api-proxy-unknown", "/api/proxy/does-not-exist", As::User),
    Scenario::json(
        "api-proxy-status-unknown",
        "/api/proxy/does-not-exist/status",
        As::User,
    ),
    Scenario::json(
        "api-proxy-status-watch-invalid-timeout",
        "/api/proxy/does-not-exist/status?watch=true&timeout=1",
        As::User,
    ),
    Scenario::json(
        "api-proxy-details-unknown",
        "/api/proxy/does-not-exist/details",
        As::User,
    ),
    Scenario::json("admin-data-user", "/admin/data", As::User),
    Scenario::json("admin-data-admin", "/admin/data", As::Admin),
    // the proxy paths of an app that does not exist
    Scenario::get("app-proxy-unknown", "/app_proxy/does-not-exist/", As::User),
    Scenario::get(
        "app-proxy-unknown-anonymous",
        "/app_proxy/does-not-exist/",
        As::Anonymous,
    ),
    Scenario::get("api-route-unknown", "/api/route/does-not-exist/", As::User),
    Scenario::json("heartbeat-unknown", "/heartbeat/does-not-exist", As::User),
    // requests with a body
    Scenario::post("api-start-unknown-spec", "/app_i/nope/_", As::User, "{}"),
    Scenario::post(
        "api-start-no-access",
        "/app_i/02_admin_only/_",
        As::User,
        "{}",
    ),
    Scenario::put(
        "api-status-unknown",
        "/api/proxy/does-not-exist/status",
        As::User,
        "{\"status\":\"Stopping\"}",
    ),
    Scenario::put(
        "api-status-invalid",
        "/api/proxy/does-not-exist/status",
        As::User,
        "{\"status\":\"Nonsense\"}",
    ),
    Scenario::put(
        "api-transfer-unknown",
        "/api/proxy/does-not-exist/userId",
        As::User,
        "{\"userId\":\"jeff\"}",
    ),
    Scenario::post(
        "issue-without-app",
        "/issue",
        As::User,
        "{\"message\":\"it does not work\",\"currentLocation\":\"/\"}",
    ),
    // the operational endpoints
    Scenario::json("actuator-health", "/actuator/health", As::Anonymous),
    Scenario::json("actuator-recyclable", "/actuator/recyclable", As::Anonymous),
    // assets and favicons
    Scenario::get("css", "/css/default.css", As::Anonymous),
    Scenario::get("favicon-without-path", "/favicon", As::Anonymous),
    Scenario::get("unknown-asset", "/css/does-not-exist.css", As::Anonymous),
];

/// The recorded answer of one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// HTTP status code.
    pub status: u16,
    /// The headers that are part of the contract, lower cased and normalised.
    pub headers: BTreeMap<String, String>,
    /// The body: normalised JSON, or the markers of an HTML page, or a short description.
    pub body: String,
}

/// A recorded fixture: the answers of one implementation to every scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// What produced the answers (for example `Java ShinyProxy 3.2.4`).
    pub implementation: String,
    /// When it was recorded.
    pub recorded_at: String,
    /// Scenario name to answer.
    pub answers: BTreeMap<String, Answer>,
}

/// The differences that are accepted, with the reason.
///
/// Every entry is a scenario name and the field that differs; the parity test skips exactly these and nothing
/// else, so a new difference always fails the test.
pub const ACCEPTED_DIFFERENCES: &[(&str, &str, &str)] = &[
    (
        "*",
        "set-cookie",
        "the attributes of the session cookie are written in another order, and Java re-issues the cookie in \
         answers where this implementation does not have to",
    ),
    (
        "*",
        "content-type",
        "some answers say `application/json` where Java says `application/json;charset=UTF-8`; JSON is UTF-8 \
         by definition",
    ),
    (
        "about-page",
        "body",
        "the about page shows the build information of this implementation (version, commit, compiler) \
         instead of the JVM details",
    ),
    (
        "api-route-unknown",
        "body",
        "both answer 403; Java lets the servlet container write its default body (35 bytes, without a \
         content type) while this implementation answers the API envelope \
         (`{\"status\":\"fail\",\"data\":\"forbidden\"}`)",
    ),
];

/// Whether a difference in a field of a scenario is accepted.
pub fn is_accepted(scenario: &str, field: &str) -> bool {
    ACCEPTED_DIFFERENCES
        .iter()
        .any(|(name, accepted_field, _)| {
            (*name == "*" || *name == scenario) && *accepted_field == field
        })
}

/// The headers that are compared (the ones that are part of the contract).
const COMPARED_HEADERS: &[&str] = &[
    "location",
    "content-type",
    "cache-control",
    "pragma",
    "expires",
    "x-content-type-options",
    "x-frame-options",
    "x-xss-protection",
    "set-cookie",
];

/// Normalises the headers of an answer: only the contract headers, lower cased, with the values that differ
/// per run replaced.
pub fn normalise_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if !COMPARED_HEADERS.contains(&name.as_str()) {
            continue;
        }
        let value = normalise_text(value);
        // a chain of `set-cookie` headers is joined, so the order does not matter
        result
            .entry(name)
            .and_modify(|existing: &mut String| {
                existing.push_str(" | ");
                existing.push_str(&value);
            })
            .or_insert(value);
    }
    result
}

/// Replaces everything that differs per run (ids, timestamps, ports, hosts, versions).
pub fn normalise_text(text: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static UUID: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").expect("regex")
    });
    static SHA1: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[0-9a-f]{40}\b").expect("regex"));
    static TIMESTAMP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b1[0-9]{12}\b").expect("regex"));
    static BASE_URL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"https?://127\.0\.0\.1:\d+").expect("regex"));
    static COOKIE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(JSESSIONID|SESSION)=[^;\s]+").expect("regex"));
    static EXPIRES: Lazy<Regex> = Lazy::new(|| Regex::new(r"Expires=[^;]+").expect("regex"));
    static CHARSET: Lazy<Regex> =
        Lazy::new(|| Regex::new(r";\s*charset=(?i)utf-8").expect("regex"));

    let text = UUID.replace_all(text, "<uuid>");
    let text = SHA1.replace_all(&text, "<sha1>");
    let text = TIMESTAMP.replace_all(&text, "<timestamp>");
    let text = BASE_URL.replace_all(&text, "");
    let text = COOKIE.replace_all(&text, "$1=<id>");
    let text = EXPIRES.replace_all(&text, "Expires=<date>");
    let text = CHARSET.replace_all(&text, ";charset=utf-8");
    text.trim().to_string()
}

/// Normalises a body for comparison.
///
/// JSON is parsed, normalised and printed with sorted keys; HTML is reduced to the markers that matter
/// (title, element ids, form field names, the app ids of the tiles); anything else is described by its size.
pub fn normalise_body(content_type: &str, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    if content_type.contains("json") {
        return match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                let normalised = normalise_json(&value);
                serde_json::to_string_pretty(&normalised).unwrap_or_default()
            }
            Err(_) => format!("<invalid json: {}>", normalise_text(&text)),
        };
    }
    if content_type.contains("html") {
        use once_cell::sync::Lazy;
        use regex::Regex;
        static MARKERS: Lazy<Regex> = Lazy::new(|| {
            Regex::new(
                r#"<title>[^<]*</title>|data-app-id="[^"]*"|id="[a-zA-Z0-9_-]+"|name="[a-zA-Z0-9_-]+""#,
            )
            .expect("regex")
        });
        let mut markers: Vec<String> = MARKERS
            .find_iter(&text)
            .map(|found| normalise_text(found.as_str()))
            .collect();
        markers.sort();
        markers.dedup();
        return markers.join("\n");
    }
    if content_type.contains("css") || content_type.contains("javascript") {
        // the asset itself is compared by the tests of the assets; here only its presence matters
        return format!("<{} bytes of {}>", body.len().min(1), content_type);
    }
    if body.is_empty() {
        return String::new();
    }
    format!("<{} bytes>", body.len())
}

/// Normalises a JSON document: ids, timestamps and ports inside strings, and sorted keys.
fn normalise_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            // the keys are sorted, because the order of the fields of a JSON object means nothing
            let sorted: std::collections::BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), normalise_json(value)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalise_json).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(normalise_text(text)),
        serde_json::Value::Number(number) => {
            // timestamps are numbers in the API documents
            match number.as_i64() {
                Some(value) if value > 1_000_000_000_000 => {
                    serde_json::Value::String("<timestamp>".to_string())
                }
                _ => serde_json::Value::Number(number.clone()),
            }
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_has_a_unique_name() {
        let mut names: Vec<&str> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "the scenario names must be unique");
        assert!(count >= 40, "there are {count} scenarios");
    }

    #[test]
    fn normalises_what_differs_per_run() {
        let text = normalise_text(
            "http://127.0.0.1:8091/app_proxy/5f39a7cf-c9ff-4a85-9313-d561ec79cca9/ at 1786779809238 \
             instance 0921be2f0d4dd567e61fae84be56eadca15418b1",
        );
        assert_eq!(text, "/app_proxy/<uuid>/ at <timestamp> instance <sha1>");
        assert_eq!(
            normalise_text("JSESSIONID=abcdef; Path=/; Expires=Thu, 01-Jan-1970 00:00:00 GMT"),
            "JSESSIONID=<id>; Path=/; Expires=<date>"
        );
    }

    #[test]
    fn normalises_json_bodies() {
        let body = br#"{"status":"success","data":{"id":"5f39a7cf-c9ff-4a85-9313-d561ec79cca9","createdTimestamp":1786779809238}}"#;
        let normalised = normalise_body("application/json", body);
        assert!(normalised.contains("\"<uuid>\""), "{normalised}");
        assert!(normalised.contains("\"<timestamp>\""), "{normalised}");
    }

    #[test]
    fn normalises_html_to_its_markers() {
        let body = b"<html><head><title>Parity</title></head><body><div id=\"applist\">\
                     <a data-app-id=\"01_hello\">Hello</a><input name=\"username\"/></div></body></html>";
        let normalised = normalise_body("text/html;charset=UTF-8", body);
        assert!(normalised.contains("<title>Parity</title>"), "{normalised}");
        assert!(
            normalised.contains("data-app-id=\"01_hello\""),
            "{normalised}"
        );
        assert!(normalised.contains("name=\"username\""), "{normalised}");
    }

    #[test]
    fn compares_only_the_contract_headers() {
        let headers = normalise_headers([
            ("Content-Type", "text/html;charset=UTF-8"),
            ("Date", "Sat, 15 Aug 2026 06:00:00 GMT"),
            ("X-Content-Type-Options", "nosniff"),
            ("Transfer-Encoding", "chunked"),
        ]);
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("text/html;charset=utf-8")
        );
        assert_eq!(
            headers.get("x-content-type-options").map(String::as_str),
            Some("nosniff")
        );
        assert!(!headers.contains_key("date"), "{headers:?}");
        assert!(!headers.contains_key("transfer-encoding"), "{headers:?}");
    }

    #[test]
    fn accepted_differences_are_scoped() {
        assert!(is_accepted("login-page", "set-cookie"));
        assert!(is_accepted("about-page", "body"));
        assert!(!is_accepted("index-user", "body"));
        assert!(!is_accepted("login-page", "location"));
    }
}
