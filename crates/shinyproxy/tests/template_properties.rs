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

//! End-to-end coverage of `proxy.specs[].template-properties` in custom templates.

mod common;

use common::TestInstance;

#[tokio::test]
async fn custom_index_template_reads_template_properties() {
    let templates = tempfile::tempdir().expect("templates dir");
    // Minimal override that exercises the Java-equivalent helper and the model map.
    std::fs::write(
        templates.path().join("index.html"),
        r#"<!DOCTYPE html>
<html><body>
{% for app in apps %}
<div class="app" data-id="{{ app.id }}">
  <span class="category">{{ get_template_property(app.id, 'category') }}</span>
  <span class="type">{{ app.templateProperties.type }}</span>
  <span class="icon">{{ getTemplateProperty(app.id, 'icon') }}</span>
  <span class="startup">{{ templateProperties[app.id]['startup-time'] }}</span>
  <span class="missing">{{ get_template_property(app.id, 'nope', 'fallback') }}</span>
</div>
{% endfor %}
</body></html>
"#,
    )
    .expect("write index.html");

    let yaml = format!(
        r#"
proxy:
  title: Template Properties Demo
  authentication: none
  container-backend: local
  template-path: {}
  specs:
    - id: my-app
      display-name: My Application
      description: Application description
      container-image: openanalytics/shinyproxy-demo
      template-properties:
        category: energy
        type: shiny
        icon: fa-bolt
        startup-time: 20
"#,
        templates.path().display()
    );

    let instance = TestInstance::start(&yaml).await;
    let client = instance.client();
    let response = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("index request");
    assert_eq!(response.status().as_u16(), 200);
    let html = response.text().await.expect("body");

    assert!(
        html.contains(r#"data-id="my-app""#),
        "app id missing: {html}"
    );
    assert!(
        html.contains(r#"<span class="category">energy</span>"#),
        "category missing: {html}"
    );
    assert!(
        html.contains(r#"<span class="type">shiny</span>"#),
        "type missing: {html}"
    );
    assert!(
        html.contains(r#"<span class="icon">fa-bolt</span>"#),
        "icon missing: {html}"
    );
    assert!(
        html.contains(r#"<span class="startup">20</span>"#),
        "startup-time missing: {html}"
    );
    assert!(
        html.contains(r#"<span class="missing">fallback</span>"#),
        "default missing: {html}"
    );

    // Keep the rendered page for the walkthrough artifact when run with --nocapture.
    eprintln!("--- rendered custom index ---\n{html}\n--- end ---");
}
