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

//! Apps that ask the user for parameters before they start.
//!
//! Replaces the Java `TestParameterValidationService` and the parameter part of `AppControllerTest`.

mod common;

use common::TestInstance;

/// Two parameters, two value sets, the second one only for the `scientists` group.
const CONFIG: &str = r##"
proxy:
  title: Parameters Test
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  docker:
    port-range-start: 26000
  users:
    - name: jack
      password: password
      groups: scientists
    - name: jeff
      password: password
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      container-env:
        CHOSEN_DATASET: "#{proxy.getRuntimeObject('SHINYPROXY_PARAMETERS').backendValues['dataset']}"
      parameters:
        definitions:
          - id: resources
            display-name: Resources
            description: The <b>amount</b> of resources<script>alert(1)</script>
            default-value: 1-2
            value-names:
              - value: 1-2
                name: 1 CPU core - 2G RAM
              - value: 2-8
                name: 2 CPU cores - 8G RAM
          - id: dataset
            display-name: Dataset
            default-value: public
        value-sets:
          - name: everyone
            values:
              resources: 1-2
              dataset: public
          - name: scientists
            access-control:
              groups: scientists
            values:
              resources: [ 1-2, 2-8 ]
              dataset: [ public, private ]
"##;

#[tokio::test]
async fn the_app_page_renders_the_parameter_form() {
    let instance = TestInstance::start(CONFIG).await;

    // a member of the scientists group may use both value sets
    let jack = instance.login("jack", "password").await;
    let body = jack
        .get(instance.url("/app/01_hello"))
        .send()
        .await
        .expect("app page")
        .text()
        .await
        .expect("body");

    assert!(
        body.contains("Choose the parameters for this app"),
        "{body}"
    );
    assert!(body.contains(">Resources</label>"), "{body}");
    assert!(body.contains(">Dataset</label>"), "{body}");
    // the values are shown with their human friendly names
    assert!(
        body.contains("<option>1 CPU core - 2G RAM</option>"),
        "{body}"
    );
    assert!(
        body.contains("<option>2 CPU cores - 8G RAM</option>"),
        "{body}"
    );
    assert!(body.contains("<option>private</option>"), "{body}");
    // descriptions keep basic markup but the script is removed from the rendered form (the raw
    // description is only part of the JSON the front-end receives, where it is escaped, exactly like in
    // the Java implementation)
    assert!(body.contains("The <b>amount</b> of resources"), "{body}");
    assert!(!body.contains("<script>alert(1)</script>"), "{body}");
    // the front-end receives every allowed combination (order follows the Java implementation: per
    // value set, value by value)
    for combination in ["[1,1]", "[2,1]", "[1,2]", "[2,2]"] {
        assert!(
            body.contains(combination),
            "the allowed combinations are passed to the front-end ({combination}): {body}"
        );
    }
    assert!(
        body.contains("loadDefaultParameters([1,1])"),
        "the configured defaults are selected: {body}"
    );

    // a user outside the group only sees the values of the first value set
    let jeff = instance.login("jeff", "password").await;
    let body = jeff
        .get(instance.url("/app/01_hello"))
        .send()
        .await
        .expect("app page")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("<option>1 CPU core - 2G RAM</option>"),
        "{body}"
    );
    assert!(!body.contains("2 CPU cores - 8G RAM"), "{body}");
    assert!(!body.contains("<option>private</option>"), "{body}");
    assert!(body.contains("loadDefaultParameters([1,1])"), "{body}");

    instance.stop();
}

#[tokio::test]
async fn starting_an_app_validates_the_chosen_parameters() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    // no parameters at all
    let response = jack
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({"timezone": "Europe/Brussels"}))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(
        json["data"],
        "No parameters provided, but proxy spec expects parameters"
    );

    // a value that does not exist
    let response = jack
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({
            "parameters": {"resources": "1 CPU core - 2G RAM", "dataset": "secret"}
        }))
        .send()
        .await
        .expect("start request");
    assert_eq!(response.status(), 400);
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["data"], "Provided parameter values are not allowed");

    // a combination that spans two value sets
    let response = jack
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({
            "parameters": {"resources": "1 CPU core - 2G RAM", "dataset": "private"}
        }))
        .send()
        .await
        .expect("start request");
    assert_eq!(
        response.status(),
        200,
        "the scientists value set allows this combination"
    );

    instance.stop();
}

#[tokio::test]
async fn chosen_parameters_reach_the_app_and_the_api() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    let started: serde_json::Value = jack
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({
            "parameters": {"resources": "2 CPU cores - 8G RAM", "dataset": "private"},
            "timezone": "Europe/Brussels"
        }))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();

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

    // the API shows the chosen values with their display names, never the backend values
    let names = &status["data"]["runtimeValues"]["SHINYPROXY_PARAMETER_NAMES"];
    assert_eq!(names[0]["displayName"], "Resources");
    assert_eq!(names[0]["value"], "2 CPU cores - 8G RAM");
    assert!(
        names[0]["description"]
            .as_str()
            .expect("description")
            .contains("amount"),
        "{names}"
    );
    assert_eq!(names[1]["displayName"], "Dataset");
    assert_eq!(names[1]["value"], "private");
    assert!(
        status["data"]["runtimeValues"]["SHINYPROXY_PARAMETERS"].is_null(),
        "the backend values are not part of the API: {status}"
    );

    // the backend values are available to expressions, so the app got the value in its environment
    let environment: std::collections::BTreeMap<String, String> = jack
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("env request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        environment.get("CHOSEN_DATASET").map(String::as_str),
        Some("private"),
        "the chosen backend value reached the app: {environment:?}"
    );

    instance.stop();
}

#[tokio::test]
async fn the_form_shows_the_values_of_the_app_that_is_resumed() {
    let instance = TestInstance::start(CONFIG).await;
    let jack = instance.login("jack", "password").await;

    let started: serde_json::Value = jack
        .post(instance.url("/app_i/01_hello/_"))
        .json(&serde_json::json!({
            "parameters": {"resources": "2 CPU cores - 8G RAM", "dataset": "private"}
        }))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();
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

    // the page of the running app preselects the values it was started with (index 2 of both parameters)
    let body = jack
        .get(instance.url("/app/01_hello"))
        .send()
        .await
        .expect("app page")
        .text()
        .await
        .expect("body");
    assert!(
        body.contains("loadDefaultParameters([2,2])"),
        "the values of the running app are preselected: {body}"
    );

    instance.stop();
}

#[tokio::test]
async fn a_configuration_provided_form_is_rendered() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  docker:
    port-range-start: 26100
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      parameters:
        definitions:
          - id: dataset
            display-name: Dataset
        value-sets:
          - values:
              dataset: [ public, private ]
        template: |
          <form class="form-horizontal default-parameter-form" id="my-own-form">
            {% for parameter in parameterDefinitions %}
            <label for="parameter-{{ parameter.id }}">Pick a {{ parameter.displayNameOrId }}</label>
            <select id="parameter-{{ parameter.id }}" name="{{ parameter.id }}">
              {% for value in parameterValues[parameter.id] %}
              <option>{{ value }}</option>
              {% endfor %}
            </select>
            {% endfor %}
            <button type="submit">Start my app</button>
          </form>
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;

    let body = jack
        .get(instance.url("/app/01_hello"))
        .send()
        .await
        .expect("app page")
        .text()
        .await
        .expect("body");
    assert!(body.contains("id=\"my-own-form\""), "{body}");
    assert!(body.contains("Pick a Dataset"), "{body}");
    assert!(body.contains("<option>private</option>"), "{body}");
    assert!(body.contains("Start my app"), "{body}");
    // the default form is replaced, not added
    assert!(
        !body.contains("Choose the parameters for this app"),
        "{body}"
    );

    instance.stop();
}

#[tokio::test]
async fn invalid_parameters_are_refused_at_startup() {
    // a Thymeleaf template cannot be rendered by this implementation, so it is refused with the
    // constructs it found
    let error = start_and_expect_error(
        r##"
proxy:
  authentication: none
  specs:
    - id: 01_hello
      container-image: sp-testapp
      parameters:
        definitions:
          - id: dataset
        value-sets:
          - values:
              dataset: public
        template: |
          <div th:each="parameter : ${parameterDefinitions}"></div>
"##,
    );
    assert!(error.contains("th:each"), "{error}");
    assert!(error.contains("MiniJinja"), "{error}");

    // the Java validation messages
    let error = start_and_expect_error(
        r##"
proxy:
  authentication: none
  specs:
    - id: 01_hello
      container-image: sp-testapp
      parameters:
        definitions:
          - id: dataset
          - id: dataset
        value-sets:
          - values:
              dataset: public
"##,
    );
    assert_eq!(
        error,
        "Configuration error: error in parameters of spec '01_hello', error: duplicate parameter id \
         'dataset'"
    );
}

/// Loads a configuration and returns the error it produces.
fn start_and_expect_error(yaml: &str) -> String {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("application.yml");
    std::fs::write(&path, yaml).expect("write configuration");
    let options = containerproxy::config::LoadOptions {
        args: vec![format!("--spring.config.location={}", path.display())],
        ..containerproxy::config::LoadOptions::default()
    };
    let (raw, mut settings) = shinyproxy::load_config(options).expect("configuration loads");
    settings.proxy.container_backend = Some("local".to_string());
    shinyproxy::web::AppState::new(raw, settings)
        .expect_err("the configuration must be refused")
        .to_string()
}
