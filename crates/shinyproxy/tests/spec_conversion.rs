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

//! Converts the app definitions of every configuration fixture into the engine model.

use std::path::{Path, PathBuf};

use containerproxy::config::LoadOptions;
use containerproxy::spec::SpecProvider;
use shinyproxy::spec_provider::ShinyProxySpecProvider;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/configs");

fn load(name: &str) -> ShinyProxySpecProvider {
    let path: PathBuf = Path::new(FIXTURES).join(name);
    let options = LoadOptions {
        args: vec![format!("--spring.config.location={}", path.display())],
        ..LoadOptions::default()
    };
    let (_raw, settings) = shinyproxy::load_config(options).expect("configuration loads");
    ShinyProxySpecProvider::from_settings(&settings)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn every_fixture_yields_usable_specs() {
    let mut names: Vec<String> = std::fs::read_dir(FIXTURES)
        .expect("fixtures")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .filter(|name| name.ends_with(".yml"))
        .collect();
    names.sort();

    for name in names {
        let provider = load(&name);
        for spec in provider.specs() {
            assert!(!spec.id.is_empty(), "{name}: spec without id");
            let container = spec
                .container_spec()
                .unwrap_or_else(|| panic!("{name}: spec {} has no container", spec.id));
            // ShinyProxy always creates exactly one container spec with a default port mapping.
            assert_eq!(spec.container_specs.len(), 1, "{name}: {}", spec.id);
            assert!(
                container
                    .port_mapping
                    .iter()
                    .any(|mapping| mapping.name == "default"),
                "{name}: {} has no default port mapping",
                spec.id
            );
            // external apps are the only specs without a container image
            let external = ShinyProxySpecProvider::external(spec)
                .external_url
                .is_some();
            assert!(
                external || container.image.is_present(),
                "{name}: {} has no container image",
                spec.id
            );
        }
    }
}

#[test]
fn docker_fixture_specs_match_the_configuration() {
    let provider = load("docker.yml");
    let spec = provider.spec("01_hello").expect("01_hello");

    assert_eq!(spec.display_name.as_deref(), Some("Hello Application"));
    assert_eq!(spec.access_control.groups, ["scientists"]);
    assert_eq!(spec.max_total_instances, 10);
    assert_eq!(spec.stop_on_logout, Some(true));
    assert_eq!(
        spec.max_lifetime.original().map(String::as_str),
        Some("120")
    );
    assert_eq!(
        spec.heartbeat_timeout.original().map(String::as_str),
        Some("90000")
    );
    assert_eq!(
        spec.cache_headers_mode,
        Some(containerproxy::model::spec::CacheHeadersMode::EnforceNoCache)
    );

    let container = spec.container_spec().expect("container");
    assert_eq!(
        container.image.original().map(String::as_str),
        Some("openanalytics/shinyproxy-demo")
    );
    assert_eq!(
        container
            .env
            .original()
            .unwrap()
            .get("MY_VAR")
            .map(String::as_str),
        Some("#{proxy.userId}"),
        "expressions are kept unresolved until a proxy is started"
    );
    assert_eq!(
        container.memory_limit.original().map(String::as_str),
        Some("2g")
    );
    assert_eq!(
        container.volumes.original().unwrap(),
        &vec!["/tmp:/tmp".to_string()]
    );

    // additional mappings first, default mapping last (as in the Java implementation)
    let names: Vec<&str> = container
        .port_mapping
        .iter()
        .map(|mapping| mapping.name.as_str())
        .collect();
    assert_eq!(names, ["dashboard", "default"]);
    assert_eq!(container.port_mapping[1].port, Some(3838));
    assert_eq!(
        container.port_mapping[1]
            .target_path
            .original()
            .map(String::as_str),
        Some("/app")
    );

    let extension = ShinyProxySpecProvider::extension(spec);
    assert_eq!(
        extension.max_instances.original().map(String::as_str),
        Some("2")
    );
    assert_eq!(
        extension.websocket_reconnection_mode,
        Some(shinyproxy::spec_provider::WebsocketReconnectionMode::Confirm)
    );
}

#[test]
fn template_group_fixture_exposes_groups_and_external_apps() {
    let provider = load("template-groups.yml");
    assert_eq!(provider.template_groups().len(), 2);
    assert_eq!(
        provider.template_groups()[0]
            .properties
            .get("color")
            .map(String::as_str),
        Some("blue")
    );

    let report = provider.spec("report").expect("report");
    let extension = ShinyProxySpecProvider::extension(report);
    assert_eq!(extension.template_group.as_deref(), Some("reporting"));
    assert_eq!(extension.custom_app_details.len(), 1);
    assert!(ShinyProxySpecProvider::always_show_switch_instance(
        report, false
    ));

    let external = provider.spec("external").expect("external");
    assert_eq!(
        ShinyProxySpecProvider::external(external)
            .external_url
            .as_deref(),
        Some("https://example.com/app")
    );
}
