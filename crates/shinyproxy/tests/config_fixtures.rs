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

//! Loads realistic ShinyProxy configurations (modelled on the examples of the ShinyProxy
//! documentation) and asserts that every property is understood and bound correctly.
//!
//! These fixtures are the safety net for the configuration compatibility promise: a property that is
//! not part of the schema shows up as an "unknown property" here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use containerproxy::spec::SpecProvider;

use containerproxy::config::{LoadOptions, RawConfig, Settings};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/configs");

fn fixture_path(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}

/// Loads a fixture through the same code path as the binary.
fn load(name: &str) -> (RawConfig, Settings) {
    load_with_env(name, BTreeMap::new())
}

fn load_with_env(name: &str, env: BTreeMap<String, String>) -> (RawConfig, Settings) {
    let path = fixture_path(name);
    let options = LoadOptions {
        args: vec![format!("--spring.config.location={}", path.display())],
        env,
        working_dir: None,
        fallback_config: None,
    };
    shinyproxy::load_config(options).unwrap_or_else(|error| panic!("{name} must load: {error}"))
}

fn assert_no_unknown_properties(name: &str, config: &RawConfig) {
    assert!(
        config.unknown_properties.is_empty(),
        "{name} contains unknown properties: {:#?}",
        config.unknown_properties
    );
}

#[test]
fn every_fixture_is_fully_understood() {
    let mut names: Vec<String> = std::fs::read_dir(FIXTURES)
        .expect("fixtures directory")
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
    assert!(
        names.len() >= 12,
        "expected the full fixture set, got {names:?}"
    );

    for name in names {
        let (config, _settings) = load(&name);
        assert_no_unknown_properties(&name, &config);
    }
}

#[test]
fn docker_configuration() {
    let (config, settings) = load("docker.yml");
    assert_no_unknown_properties("docker.yml", &config);

    assert_eq!(settings.proxy.title(), "My ShinyProxy");
    assert_eq!(settings.proxy.container_backend(), "docker");
    assert_eq!(settings.proxy.docker.port_range_start(), 20000);
    assert_eq!(settings.proxy.docker.port_range_max(), Some(21000));
    assert_eq!(
        settings.proxy.docker.url.as_deref(),
        Some("tcp://localhost:2375")
    );
    assert_eq!(
        settings.proxy.docker.image_pull_policy.as_deref(),
        Some("IfNotPresent")
    );
    assert!(!settings.proxy.docker.internal_networking());
    assert_eq!(
        settings.proxy.admin_groups.values(),
        ["scientists", "admins"]
    );
    assert_eq!(settings.proxy.container_wait_timeout_ms(), 20000);

    // spec details are interpreted by the ShinyProxy spec provider (phase P2); at this point they are
    // available as raw values.
    let spec = &settings.proxy.specs[0];
    assert_eq!(spec["id"], "01_hello");
    assert_eq!(spec["container-image"], "openanalytics/shinyproxy-demo");
    assert_eq!(spec["container-env"]["MY_VAR"], "#{proxy.userId}");
    assert_eq!(spec["additional-port-mappings"][0]["name"], "dashboard");
    assert_eq!(spec["access-groups"][0], "scientists");
}

#[test]
fn kubernetes_configuration() {
    let (config, settings) = load("kubernetes.yml");
    assert_no_unknown_properties("kubernetes.yml", &config);

    assert_eq!(settings.proxy.container_backend(), "kubernetes");
    assert_eq!(settings.proxy.kubernetes.namespace(), "shinyproxy");
    assert_eq!(
        settings.proxy.kubernetes.image_pull_secrets.values(),
        ["registry-secret"]
    );
    // the node selector accepts a map (this fixture) and the `key=value,key=value` string of Java
    assert_eq!(
        settings
            .proxy
            .kubernetes
            .node_selector
            .as_ref()
            .expect("node selector")
            .pairs()
            .expect("pairs")
            .get("kubernetes.io/hostname")
            .map(String::as_str),
        Some("node-1")
    );
    let spec = &settings.proxy.specs[0];
    assert!(spec["kubernetes-pod-patches"]
        .as_str()
        .expect("pod patches")
        .contains("volumeMounts"));
    assert_eq!(
        spec["kubernetes-authorized-pod-patches"][0]["access-control"]["groups"][0],
        "gpu-users"
    );
}

#[test]
fn openid_configuration() {
    let (config, settings) = load("openid.yml");
    assert_no_unknown_properties("openid.yml", &config);

    assert_eq!(settings.proxy.authentication(), "openid");
    assert_eq!(
        settings.proxy.openid.client_id.as_deref(),
        Some("shinyproxy")
    );
    assert_eq!(
        settings.proxy.openid.scopes.values(),
        ["openid", "email", "profile"]
    );
    assert_eq!(
        settings.proxy.openid.username_attribute.as_deref(),
        Some("preferred_username")
    );
    assert_eq!(
        settings.proxy.openid.with_pkce.map(|value| value.0),
        Some(true)
    );
    assert_eq!(settings.proxy.ms_graph.tenant_id.as_deref(), Some("tenant"));

    let spec = &settings.proxy.specs[0];
    assert!(spec["access-expression"]
        .as_str()
        .expect("expression")
        .contains("oidcUser"));
}

#[test]
fn ldap_configuration_single_and_multiple_providers() {
    let (config, settings) = load("ldap.yml");
    assert_no_unknown_properties("ldap.yml", &config);
    let providers = settings.proxy.ldap.providers();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].user_dn_pattern.as_deref(), Some("uid={0}"));
    assert_eq!(
        providers[0].manager_dn.as_deref(),
        Some("cn=read-only-admin,dc=example,dc=com")
    );

    let (config, settings) = load("ldap-multiple.yml");
    assert_no_unknown_properties("ldap-multiple.yml", &config);
    let providers = settings.proxy.ldap.providers();
    assert_eq!(providers.len(), 2);
    assert_eq!(
        providers[1].starttls.as_ref().map(|value| value.0.as_str()),
        Some("simple")
    );
}

#[test]
fn saml_configuration() {
    let (config, settings) = load("saml.yml");
    assert_no_unknown_properties("saml.yml", &config);
    assert_eq!(settings.proxy.authentication(), "saml");
    assert_eq!(
        settings.proxy.saml.idp_metadata_url.as_deref(),
        Some("https://idp.example.com/metadata")
    );
    assert_eq!(
        settings.proxy.saml.force_authn.map(|value| value.0),
        Some(true)
    );
}

#[test]
fn high_availability_configuration() {
    let (config, settings) = load("ha-redis.yml");
    assert_no_unknown_properties("ha-redis.yml", &config);

    assert_eq!(settings.proxy.store_mode(), "Redis");
    assert!(!settings.proxy.stop_proxies_on_shutdown());
    assert!(settings.spring.session.is_redis());
    assert_eq!(settings.spring.data.redis.host.as_deref(), Some("redis"));
    assert_eq!(
        settings.spring.data.redis.port.map(|value| value.0),
        Some(6379)
    );
    assert_eq!(
        settings.spring.data.redis.sentinel.nodes.values(),
        ["redis-1:26379", "redis-2:26379"]
    );

    // this configuration is valid, so it must not produce a fatal diagnostic
    let diagnostics = containerproxy::config::validate(&settings, &config.unknown_properties);
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != containerproxy::config::Severity::Fatal));
}

#[test]
fn parameters_configuration() {
    let (config, settings) = load("parameters.yml");
    assert_no_unknown_properties("parameters.yml", &config);

    let parameters = &settings.proxy.specs[0]["parameters"];
    assert_eq!(parameters["definitions"][0]["id"], "resources");
    assert_eq!(
        parameters["definitions"][0]["value-names"][1]["value"],
        "2-8"
    );
    assert_eq!(
        parameters["value-sets"][1]["access-control"]["groups"][0],
        "scientists"
    );
    assert_eq!(parameters["value-sets"][0]["values"]["resources"][0], "1-2");
    // configuration provided forms use the MiniJinja syntax in this implementation; Thymeleaf constructs
    // are refused at startup (see the parameters service)
    let template = parameters["template"].as_str().expect("template");
    assert!(
        template.contains("{% for parameter in parameterDefinitions %}"),
        "{template}"
    );
    assert!(!template.contains("th:"), "{template}");

    // the app definitions of this configuration are usable, including the parameter validation
    let provider = shinyproxy::spec_provider::ShinyProxySpecProvider::from_settings(&settings)
        .expect("the parameters of the fixture are valid");
    let spec = provider.spec("parameterized").expect("spec");
    let parameters = spec.parameters.as_ref().expect("parameters");
    assert_eq!(
        parameters.ids(),
        vec!["resources".to_string(), "dataset".to_string()]
    );
    assert_eq!(parameters.value_sets.len(), 2);
    assert_eq!(
        parameters.value_sets[1]
            .access_control
            .as_ref()
            .map(|access| access.groups()),
        Some(["scientists".to_string()].as_slice())
    );
    assert_eq!(parameters.value_sets[0].values_of("resources"), ["1-2"]);
}

#[test]
fn template_groups_and_external_apps() {
    let (config, settings) = load("template-groups.yml");
    assert_no_unknown_properties("template-groups.yml", &config);

    assert_eq!(settings.proxy.my_apps_mode.as_deref(), Some("Inline"));
    assert_eq!(settings.proxy.body_classes.values(), ["dark", "compact"]);
    assert_eq!(settings.proxy.template_groups.len(), 2);
    assert_eq!(settings.proxy.template_groups[0]["id"], "reporting");
    assert_eq!(
        settings.proxy.template_groups[0]["properties"]["color"],
        "blue"
    );
    assert_eq!(settings.proxy.specs[0]["template-group"], "reporting");
    assert_eq!(
        settings.proxy.specs[0]["custom-app-details"][0]["name"],
        "Dataset"
    );
    assert_eq!(
        settings.proxy.specs[1]["external-url"],
        "https://example.com/app"
    );
}

#[test]
fn usage_statistics_and_container_logs() {
    let (config, settings) = load("usage-stats.yml");
    assert_no_unknown_properties("usage-stats.yml", &config);

    assert_eq!(
        settings.proxy.usage_stats_url.as_deref(),
        Some("jdbc:postgresql://localhost:5432/shinyproxy")
    );
    assert_eq!(
        settings.proxy.usage_stats_attributes[0].name.as_deref(),
        Some("realm")
    );
    assert_eq!(settings.proxy.usage_stats.len(), 2);
    assert_eq!(
        settings.proxy.usage_stats[0].url.as_deref(),
        Some("micrometer")
    );
    assert_eq!(
        settings.proxy.usage_stats[1].attributes[0]
            .expression
            .as_deref(),
        Some("#{userId}")
    );
    assert_eq!(
        settings
            .proxy
            .usage_stats_hikari
            .maximum_pool_size
            .map(|value| value.0),
        Some(5)
    );
    assert_eq!(
        settings.proxy.container_log_path.as_deref(),
        Some("/var/log/shinyproxy/containers")
    );
}

#[test]
fn ecs_configuration() {
    let (config, settings) = load("ecs.yml");
    assert_no_unknown_properties("ecs.yml", &config);

    assert_eq!(
        settings.proxy.ecs.name.as_deref(),
        Some("shinyproxy-cluster")
    );
    assert_eq!(
        settings.proxy.ecs.subnets.values(),
        ["subnet-123", "subnet-456"]
    );
    // `enable-cloudwatch` is the legacy spelling of `enable-cloud-watch`
    assert_eq!(
        settings.proxy.ecs.enable_cloud_watch.map(|value| value.0),
        Some(true)
    );

    let spec = &settings.proxy.specs[0];
    assert_eq!(spec["ecs-efs-volumes"][0]["file-system-id"], "fs-123");
    assert_eq!(spec["ecs-managed-secrets"][0]["name"], "API_KEY");
}

#[test]
fn proxy_sharing_configuration() {
    let (config, settings) = load("proxy-sharing.yml");
    assert_no_unknown_properties("proxy-sharing.yml", &config);

    assert_eq!(
        settings.proxy.seat_wait_time.map(|value| value.0),
        Some(60000)
    );
    let spec = &settings.proxy.specs[0];
    assert_eq!(spec["minimum-seats-available"], 2);
    assert_eq!(spec["seats-per-container"], 4);
    assert_eq!(spec["allow-container-re-use"], true);
}

#[test]
fn api_security_and_infrastructure_configuration() {
    let (config, settings) = load("api-security.yml");
    assert_no_unknown_properties("api-security.yml", &config);

    assert_eq!(settings.proxy.bind_address(), "127.0.0.1");
    assert_eq!(settings.proxy.same_site_cookie(), "None");
    assert!(!settings.proxy.username_case_sensitive());
    assert!(settings.proxy.allow_transfer_app());
    assert!(settings.proxy.log_as_json());
    assert!(settings.proxy.api_security.hide_spec_details());
    assert_eq!(
        settings.proxy.api_security.cors_allowed_origins.values(),
        ["https://example.com"]
    );
    assert_eq!(
        settings.proxy.api_security.custom_headers[0]
            .name
            .as_deref(),
        Some("X-Frame-Options")
    );
    assert_eq!(
        settings.proxy.oauth2.resource_id.as_deref(),
        Some("shinyproxy")
    );
    assert_eq!(
        settings.proxy.support.mail_to_address.as_deref(),
        Some("support@example.com")
    );
    assert_eq!(
        settings.proxy.monitoring.grafana_url.as_deref(),
        Some("https://grafana.example.com")
    );
    assert_eq!(settings.server.context_path(), "/shinyproxy");
    assert!(settings.server.secure_cookies());
    assert_eq!(settings.server.frame_options(), "sameorigin");
    assert_eq!(
        settings.spring.mail.host.as_deref(),
        Some("smtp.example.com")
    );
    assert_eq!(
        settings
            .spring
            .mail
            .properties
            .get("mail.smtp.starttls.enable")
            .map(|value| value.to_string()),
        Some("true".to_string())
    );
    assert_eq!(
        settings.logging.file.name.as_deref(),
        Some("shinyproxy.log")
    );
    assert_eq!(
        settings
            .logging
            .level
            .get("eu.openanalytics")
            .map(String::as_str),
        Some("DEBUG")
    );
    assert_eq!(settings.management.port(), 9091);
    assert!(settings.springdoc.swagger_ui.enabled());

    // secure-cookies with SameSite=None is valid, but hide-spec-details is on, so no warnings expected
    let diagnostics = containerproxy::config::validate(&settings, &config.unknown_properties);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn environment_variables_override_fixtures() {
    let env = BTreeMap::from([
        ("PROXY_PORT".to_string(), "9999".to_string()),
        ("PROXY_TITLE".to_string(), "From env".to_string()),
        (
            "PROXY_DOCKER_PORT_RANGE_START".to_string(),
            "30000".to_string(),
        ),
        ("PROXY_ADMIN_GROUPS_0".to_string(), "envgroup".to_string()),
        (
            "PROXY_SPECS_0_CONTAINER_IMAGE".to_string(),
            "env/image".to_string(),
        ),
    ]);
    let (_config, settings) = load_with_env("docker.yml", env);
    assert_eq!(settings.proxy.port(), 9999);
    assert_eq!(settings.proxy.title(), "From env");
    assert_eq!(settings.proxy.docker.port_range_start(), 30000);
    assert_eq!(settings.proxy.admin_groups.values(), ["envgroup"]);
    assert_eq!(settings.proxy.specs[0]["container-image"], "env/image");
}

#[test]
fn demo_configuration_is_used_as_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let options = LoadOptions {
        working_dir: Some(dir.path().to_path_buf()),
        ..LoadOptions::default()
    };
    let (config, settings) = shinyproxy::load_config(options).expect("demo config loads");
    assert_eq!(config.profiles, vec!["demo".to_string()]);
    assert_no_unknown_properties("application-demo.yml", &config);
    assert_eq!(settings.proxy.specs.len(), 2);
    assert_eq!(settings.proxy.users.len(), 2);
}

/// Every example configuration in `examples/` must load without unknown properties and yield usable apps.
///
/// The examples are what users copy, so a typo in one of them is a bug.
#[test]
fn every_example_configuration_is_understood() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut names: Vec<String> = std::fs::read_dir(&examples)
        .expect("examples directory")
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
    assert!(
        names.len() >= 3,
        "the examples of the plan must exist: {names:?}"
    );

    for name in names {
        let path = examples.join(&name);
        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (config, settings) = shinyproxy::load_config(options)
            .unwrap_or_else(|error| panic!("examples/{name} must load: {error}"));
        assert!(
            config.unknown_properties.is_empty(),
            "examples/{name} contains unknown properties: {:#?}",
            config.unknown_properties
        );

        let provider = shinyproxy::spec_provider::ShinyProxySpecProvider::from_settings(&settings)
            .unwrap_or_else(|error| panic!("examples/{name} must yield specs: {error}"));
        assert!(
            !provider.specs().is_empty(),
            "examples/{name} must define at least one app"
        );
    }
}
