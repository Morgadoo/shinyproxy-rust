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

//! Collecting the output of the apps (`proxy.container-log-path`).
//!
//! Replaces the Java `TestContainerLogging`. Uses the `local` backend, whose processes write to pipes
//! that the log service reads, exactly as it reads the log stream of a Docker container.

mod common;

use std::time::Duration;

use common::TestInstance;

#[tokio::test]
async fn the_output_of_an_app_is_written_to_files() {
    let directory = tempfile::tempdir().expect("temp dir");
    let log_path = directory.path().join("container-logs");

    let instance = TestInstance::start(&format!(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  container-log-path: {}
  docker:
    port-range-start: 29000
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##,
        log_path.display()
    ))
    .await;

    assert!(instance.state.logs.is_enabled());
    assert!(log_path.is_dir(), "the log directory is created at startup");

    let jack = instance.login("jack", "password").await;
    let started: serde_json::Value = jack
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"].as_str().expect("id").to_string();
    let status: serde_json::Value = jack
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

    // the app writes a line per request, so a request produces output to collect
    for _ in 0..3 {
        jack.get(instance.url(&format!("/app_proxy/{proxy_id}/")))
            .send()
            .await
            .expect("app request");
    }

    // the files are named like the Java implementation names them
    let mut stdout_path = None;
    for _ in 0..50 {
        let files: Vec<std::path::PathBuf> = std::fs::read_dir(&log_path)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        let stdout = files.iter().find(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().ends_with("_stdout.log"))
                .unwrap_or(false)
        });
        if let Some(path) = stdout {
            if std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) > 0 {
                stdout_path = Some(path.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let stdout_path = stdout_path.expect("the app must write to its stdout log");
    let name = stdout_path
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    assert!(
        name.starts_with(&format!("01_hello_{proxy_id}_")),
        "the file name has the Java layout: {name}"
    );
    let stderr_path = log_path.join(name.replace("_stdout.log", "_stderr.log"));
    assert!(stderr_path.is_file(), "the stderr file exists as well");

    let contents = std::fs::read_to_string(&stdout_path).expect("stdout log");
    assert!(
        contents.contains("sp-testapp"),
        "the output of the app is collected: {contents}"
    );

    // the report of an issue mentions the log files of the app
    let user = containerproxy::auth::AuthenticatedUser::new("jack", vec![]);
    let proxy = instance
        .state
        .proxies
        .proxy(&proxy_id)
        .expect("the proxy exists");
    let report = shinyproxy::web::issue::build_report(
        &instance.state,
        &user,
        Some(&proxy),
        "help",
        "/app/01_hello",
    );
    assert!(
        report.body.contains(&stdout_path.display().to_string()),
        "{}",
        report.body
    );

    // stopping the app stops the collection but keeps the files
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(stdout_path.is_file(), "the log files are kept");

    instance.stop();
}

#[tokio::test]
async fn logging_is_disabled_without_a_path() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: none
  container-backend: local
  specs:
    - id: 01_hello
      container-image: sp-testapp
"##,
    )
    .await;
    assert!(!instance.state.logs.is_enabled());
    instance.stop();
}
