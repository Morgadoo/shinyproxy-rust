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

//! Records the answers of a running ShinyProxy as a parity fixture.
//!
//! Point it at the Java implementation to refresh
//! `crates/shinyproxy/tests/fixtures/parity/java-3.2.4.json`, which `cargo test -p shinyproxy --test parity`
//! then compares this implementation against:
//!
//! ```sh
//! java -jar shinyproxy-3.2.4-exec.jar --spring.config.location=parity.yml --proxy.port=8091 &
//! cargo run -p shinyproxy --example record-parity -- \
//!     --base-url http://127.0.0.1:8091 --implementation "Java ShinyProxy 3.2.4" \
//!     --out crates/shinyproxy/tests/fixtures/parity/java-3.2.4.json
//! ```
//!
//! The configuration both servers need is `shinyproxy::parity::PARITY_CONFIG`; `--write-config <path>` writes
//! it out so the server can be started with it.

use std::collections::BTreeMap;

use shinyproxy::parity::{
    normalise_body, normalise_headers, Answer, As, Fixture, Scenario, PARITY_CONFIG, SCENARIOS,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str, fallback: &str| -> String {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    };

    if let Some(index) = args.iter().position(|arg| arg == "--write-config") {
        let path = args
            .get(index + 1)
            .cloned()
            .unwrap_or_else(|| "parity.yml".to_string());
        std::fs::write(&path, PARITY_CONFIG)?;
        println!("wrote {path}");
        return Ok(());
    }

    let base_url = value("--base-url", "http://127.0.0.1:8091");
    let implementation = value("--implementation", "Java ShinyProxy 3.2.4");
    let out = value(
        "--out",
        "crates/shinyproxy/tests/fixtures/parity/java-3.2.4.json",
    );

    let mut answers = BTreeMap::new();
    for scenario in SCENARIOS {
        let answer = record(&base_url, scenario).await?;
        println!("{:<40} {}", scenario.name, answer.status);
        answers.insert(scenario.name.to_string(), answer);
    }

    let fixture = Fixture {
        implementation,
        recorded_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        answers,
    };
    let document = serde_json::to_string_pretty(&fixture)?;
    std::fs::write(&out, format!("{document}\n"))?;
    println!("\nwrote {out} ({} scenarios)", fixture.answers.len());
    Ok(())
}

/// Asks one scenario and records the normalised answer.
async fn record(base_url: &str, scenario: &Scenario) -> anyhow::Result<Answer> {
    let client = login(base_url, scenario.who).await?;
    let url = format!("{base_url}{}", scenario.path);
    let mut request = match scenario.method {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };
    if let Some(accept) = scenario.accept {
        request = request.header("Accept", accept);
    }
    if let Some(body) = scenario.body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }

    let response = request.send().await?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let headers = normalise_headers(
        response
            .headers()
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value))),
    );
    let body = response.bytes().await?;

    Ok(Answer {
        status,
        headers,
        body: normalise_body(&content_type, &body),
    })
}

/// A client with the session of the given user (or without one).
async fn login(base_url: &str, who: As) -> anyhow::Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (username, password) = match who {
        As::Anonymous => return Ok(client),
        As::User => ("jack", "password"),
        As::Admin => ("root", "rootpw"),
    };

    let login = client
        .get(format!("{base_url}/login"))
        .send()
        .await?
        .text()
        .await?;
    let token = login
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string();
    client
        .post(format!("{base_url}/login"))
        .form(&[
            ("username", username),
            ("password", password),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await?;
    Ok(client)
}
