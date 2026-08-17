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

//! Converts app definitions from the "ShinyProxy notation" into the engine's [`ProxySpec`].
//!
//! The ShinyProxy notation is more compact than the ContainerProxy one: it describes a single container
//! per app and prefixes the container fields (`container-image`, `container-cmd`, ...). When no port is
//! configured, a port mapping for the default Shiny port 3838 is created.
//!
//! This mirrors `eu.openanalytics.shinyproxy.ShinyProxySpecProvider`.

use containerproxy::config::flex::{FlexBool, FlexI64, StringList, StringMap};
use containerproxy::config::Settings;
use containerproxy::model::spec::{
    AccessControl, CacheHeadersMode, ContainerSpec, DockerDeviceRequest, DockerSwarmSecret,
    Parameters, PortMapping, ProxySpec, SpecExtensions,
};
use containerproxy::model::spel_field::{SpelLong, SpelString, SpelStringList, SpelStringMap};
use containerproxy::spec::SpecProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Name under which the ShinyProxy specific fields are stored in [`SpecExtensions`].
pub const SHINYPROXY_EXTENSION: &str = "shinyproxy";
/// Name under which the external app fields are stored in [`SpecExtensions`].
pub const EXTERNAL_EXTENSION: &str = "external";
/// Name under which the Kubernetes specific fields are stored in [`SpecExtensions`].
pub const KUBERNETES_EXTENSION: &str = "kubernetes";
/// Name under which the ECS specific fields are stored in [`SpecExtensions`].
pub const ECS_EXTENSION: &str = "ecs";
/// Name under which the container sharing fields are stored in [`SpecExtensions`].
pub const PROXY_SHARING_EXTENSION: &str = "proxy-sharing";

/// The default port of a Shiny app.
pub const DEFAULT_SHINY_PORT: i64 = 3838;
/// Name of the default port mapping.
pub const DEFAULT_MAPPING_NAME: &str = "default";

/// Errors while reading app definitions.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("Configuration error: spec with id '{0}' is defined multiple times")]
    DuplicateId(String),
    #[error("Configuration error: app definition {index} has no id")]
    MissingId { index: usize },
    #[error("Configuration error: cannot read app definition {index}: {source}")]
    Invalid {
        index: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0}")]
    Parameters(String),
    #[error("Configuration error: cannot read template group {index}: {source}")]
    InvalidTemplateGroup {
        index: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// An app definition in ShinyProxy notation.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RawSpec {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    #[serde(alias = "logoUrl", alias = "logo-u-r-l")]
    pub logo_url: Option<String>,
    pub logo_width: Option<String>,
    pub logo_height: Option<String>,
    pub logo_style: Option<String>,
    pub logo_classes: Option<String>,
    pub favicon_path: Option<String>,

    pub container_image: SpelString,
    pub container_cmd: SpelStringList,
    pub container_env: SpelStringMap,
    pub container_env_file: SpelString,
    pub container_network: SpelString,
    pub container_network_connections: SpelStringList,
    pub container_dns: SpelStringList,
    pub container_volumes: SpelStringList,
    pub container_memory_request: SpelString,
    pub container_memory_limit: SpelString,
    pub container_cpu_request: SpelString,
    pub container_cpu_limit: SpelString,
    pub container_privileged: Option<FlexBool>,
    pub container_resource_name: SpelString,
    pub labels: SpelStringMap,

    pub port: Option<FlexI64>,
    pub target_path: SpelString,
    pub additional_port_mappings: Vec<RawPortMapping>,

    pub access_groups: StringList,
    pub access_users: StringList,
    pub access_expression: Option<String>,
    pub access_strict_expression: Option<String>,

    pub docker_swarm_secrets: Vec<DockerSwarmSecret>,
    pub docker_registry_domain: Option<String>,
    pub docker_registry_username: Option<String>,
    pub docker_registry_password: Option<String>,
    pub docker_runtime: SpelString,
    pub docker_user: SpelString,
    pub docker_ipc: SpelString,
    pub docker_group_add: SpelStringList,
    pub docker_device_requests: Vec<DockerDeviceRequest>,

    pub parameters: Option<Parameters>,
    pub max_lifetime: SpelLong,
    pub heartbeat_timeout: SpelLong,
    pub stop_on_logout: Option<FlexBool>,
    pub max_total_instances: Option<FlexI64>,

    pub add_default_http_headers: Option<FlexBool>,
    pub http_headers: SpelStringMap,
    pub cache_headers_mode: Option<CacheHeadersMode>,
}

/// An additional port mapping of an app.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RawPortMapping {
    pub name: String,
    pub port: Option<FlexI64>,
    pub target_path: SpelString,
}

/// The ShinyProxy specific fields of an app definition (`ShinyProxySpecExtension`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ShinyProxySpecExtension {
    pub websocket_reconnection_mode: Option<WebsocketReconnectionMode>,
    pub shiny_force_full_reload: Option<bool>,
    pub max_instances: SpelLong,
    pub hide_navbar_on_main_page_link: Option<bool>,
    pub always_show_switch_instance: Option<bool>,
    pub track_app_url: Option<bool>,
    pub template_group: Option<String>,
    pub template_properties: StringMap,
    pub support_mail_to_address: Option<String>,
    pub support_mail_subject: Option<String>,
    pub custom_app_details: Vec<CustomAppDetail>,
}

/// How the browser reconnects when a WebSocket connection of an app is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WebsocketReconnectionMode {
    /// Do not reconnect (default).
    #[default]
    None,
    /// Ask the user before reconnecting.
    Confirm,
    /// Reconnect automatically.
    Auto,
}

/// An extra detail of an app shown in the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CustomAppDetail {
    pub name: Option<String>,
    pub description: Option<String>,
    /// May contain expressions; evaluated per request by `/api/proxy/{id}/details`.
    pub value: Option<String>,
}

/// The fields of an app that is not managed by ShinyProxy (`ExternalAppSpecExtension`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ExternalAppSpecExtension {
    /// When set, the app link points to this URL instead of starting a container.
    pub external_url: Option<String>,
}

/// A group of apps shown together in the UI (`proxy.template-groups`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TemplateGroup {
    /// Identifier referenced by `template-group` of an app.
    pub id: String,
    /// Free form properties available in the templates.
    pub properties: StringMap,
}

impl RawSpec {
    /// Converts the definition into the engine model.
    pub fn into_proxy_spec(self, raw: &Value) -> ProxySpec {
        let mut port_mappings: Vec<PortMapping> = self
            .additional_port_mappings
            .iter()
            .map(|mapping| PortMapping {
                name: mapping.name.clone(),
                port: mapping.port.map(|value| value.0),
                target_path: mapping.target_path.clone(),
            })
            .collect();

        // The Java implementation appends the default mapping last.
        port_mappings.push(PortMapping {
            name: DEFAULT_MAPPING_NAME.to_string(),
            port: Some(self.port.map(|value| value.0).unwrap_or(DEFAULT_SHINY_PORT)),
            target_path: self.target_path.clone(),
        });

        let container = ContainerSpec {
            index: 0,
            image: self.container_image,
            cmd: self.container_cmd,
            env: self.container_env,
            env_file: self.container_env_file,
            network: self.container_network,
            network_connections: self.container_network_connections,
            dns: self.container_dns,
            volumes: self.container_volumes,
            port_mapping: port_mappings,
            privileged: self
                .container_privileged
                .map(|value| value.0)
                .unwrap_or(false),
            memory_request: self.container_memory_request,
            memory_limit: self.container_memory_limit,
            cpu_request: self.container_cpu_request,
            cpu_limit: self.container_cpu_limit,
            labels: self.labels,
            docker_swarm_secrets: self.docker_swarm_secrets,
            docker_registry_domain: self.docker_registry_domain,
            docker_registry_username: self.docker_registry_username,
            docker_registry_password: self.docker_registry_password,
            docker_runtime: self.docker_runtime,
            docker_user: self.docker_user,
            docker_ipc: self.docker_ipc,
            docker_group_add: self.docker_group_add,
            docker_device_requests: self.docker_device_requests,
            resource_name: self.container_resource_name,
        };

        let access_control = AccessControl {
            groups: self.access_groups,
            users: self.access_users,
            expression: self.access_expression,
            strict_expression: self.access_strict_expression,
        };

        let mut spec_extensions = SpecExtensions::new();
        for (name, value) in [
            (
                SHINYPROXY_EXTENSION,
                extension_value::<ShinyProxySpecExtension>(raw),
            ),
            (
                EXTERNAL_EXTENSION,
                extension_value::<ExternalAppSpecExtension>(raw),
            ),
        ] {
            spec_extensions.insert(name, value);
        }
        // Backend specific extensions are stored as-is; the backends deserialize the fields they own
        // (Kubernetes in P12, ECS in P12, container sharing in P12).
        for (name, prefix) in [
            (KUBERNETES_EXTENSION, "kubernetes-"),
            (ECS_EXTENSION, "ecs-"),
        ] {
            spec_extensions.insert(name, fields_with_prefix(raw, prefix));
        }
        spec_extensions.insert(
            PROXY_SHARING_EXTENSION,
            fields_of(
                raw,
                &[
                    "minimum-seats-available",
                    "allow-container-re-use",
                    "scale-down-delay",
                    "seats-per-container",
                ],
            ),
        );

        let mut spec = ProxySpec {
            id: self.id.unwrap_or_default(),
            display_name: self.display_name,
            description: self.description,
            logo_url: self.logo_url,
            logo_width: self.logo_width,
            logo_height: self.logo_height,
            logo_style: self.logo_style,
            logo_classes: self.logo_classes,
            favicon_path: self.favicon_path,
            access_control,
            container_specs: vec![container],
            parameters: self.parameters,
            max_lifetime: self.max_lifetime,
            stop_on_logout: self.stop_on_logout.map(|value| value.0),
            heartbeat_timeout: self.heartbeat_timeout,
            http_headers: self.http_headers,
            add_default_http_headers: self.add_default_http_headers.map(|value| value.0),
            cache_headers_mode: self.cache_headers_mode,
            max_total_instances: self.max_total_instances.map(|value| value.0).unwrap_or(-1),
            spec_extensions,
        };
        spec.set_container_index();
        spec
    }
}

/// Deserializes an extension from the raw app definition, ignoring unknown fields.
///
/// Map fields that templates treat as strings (`template-properties`) are normalised first so that
/// numeric YAML values (for example `startup-time: 20`) do not fail the whole extension and wipe
/// unrelated fields via [`Default`].
fn extension_value<T: serde::de::DeserializeOwned + Default + Serialize>(raw: &Value) -> Value {
    let normalised = coerce_string_maps(raw, &["template-properties"]);
    let extension: T = serde_json::from_value(normalised).unwrap_or_default();
    serde_json::to_value(extension).unwrap_or(Value::Null)
}

/// Returns a copy of `raw` where the listed object fields have every value coerced to a string.
fn coerce_string_maps(raw: &Value, fields: &[&str]) -> Value {
    let Value::Object(map) = raw else {
        return raw.clone();
    };
    let mut clone = map.clone();
    for field in fields {
        if let Some(Value::Object(properties)) = clone.get(*field).cloned() {
            let coerced: serde_json::Map<String, Value> = properties
                .into_iter()
                .map(|(key, value)| (key, Value::String(json_value_as_string(value))))
                .collect();
            clone.insert((*field).to_string(), Value::Object(coerced));
        }
    }
    Value::Object(clone)
}

/// Converts a JSON value to the string form used by template property maps.
fn json_value_as_string(value: Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value,
        other => other.to_string(),
    }
}

/// Collects the fields of the raw definition that start with the given prefix.
fn fields_with_prefix(raw: &Value, prefix: &str) -> Value {
    let mut object = serde_json::Map::new();
    if let Value::Object(map) = raw {
        for (key, value) in map {
            if let Some(rest) = key.strip_prefix(prefix) {
                object.insert(rest.to_string(), value.clone());
            }
        }
    }
    Value::Object(object)
}

/// Collects the given fields of the raw definition.
fn fields_of(raw: &Value, fields: &[&str]) -> Value {
    let mut object = serde_json::Map::new();
    if let Value::Object(map) = raw {
        for field in fields {
            if let Some(value) = map.get(*field) {
                object.insert((*field).to_string(), value.clone());
            }
        }
    }
    Value::Object(object)
}

/// The ShinyProxy app definitions.
#[derive(Debug, Clone, Default)]
pub struct ShinyProxySpecProvider {
    specs: Vec<ProxySpec>,
    template_groups: Vec<TemplateGroup>,
}

impl ShinyProxySpecProvider {
    /// Reads the app definitions from the configuration.
    pub fn from_settings(settings: &Settings) -> Result<Self, SpecError> {
        Self::from_values(&settings.proxy.specs, &settings.proxy.template_groups)
    }

    /// Reads the app definitions from raw configuration values.
    pub fn from_values(specs: &[Value], template_groups: &[Value]) -> Result<Self, SpecError> {
        let mut parsed = Vec::with_capacity(specs.len());
        for (index, raw) in specs.iter().enumerate() {
            let definition: RawSpec = serde_json::from_value(raw.clone())
                .map_err(|source| SpecError::Invalid { index, source })?;
            if definition.id.as_ref().is_none_or(|id| id.trim().is_empty()) {
                return Err(SpecError::MissingId { index });
            }
            parsed.push(definition.into_proxy_spec(raw));
        }

        // Duplicate ids are a configuration error (as in `afterPropertiesSet`).
        let mut seen = std::collections::HashSet::new();
        for spec in &parsed {
            if !seen.insert(spec.id.clone()) {
                return Err(SpecError::DuplicateId(spec.id.clone()));
            }
        }

        // The parameters of every app are validated at startup, as `ParametersService.init` does.
        for spec in &parsed {
            containerproxy::service::parameters::validate_spec(spec)
                .map_err(SpecError::Parameters)?;
        }

        let mut groups = Vec::with_capacity(template_groups.len());
        for (index, raw) in template_groups.iter().enumerate() {
            let normalised = coerce_string_maps(raw, &["properties"]);
            let group: TemplateGroup = serde_json::from_value(normalised)
                .map_err(|source| SpecError::InvalidTemplateGroup { index, source })?;
            groups.push(group);
        }

        Ok(ShinyProxySpecProvider {
            specs: parsed,
            template_groups: groups,
        })
    }

    /// The configured template groups.
    pub fn template_groups(&self) -> &[TemplateGroup] {
        &self.template_groups
    }

    /// The ShinyProxy specific fields of a spec.
    pub fn extension(spec: &ProxySpec) -> ShinyProxySpecExtension {
        spec.spec_extensions.get(SHINYPROXY_EXTENSION)
    }

    /// The external app fields of a spec.
    pub fn external(spec: &ProxySpec) -> ExternalAppSpecExtension {
        spec.spec_extensions.get(EXTERNAL_EXTENSION)
    }

    /// Whether the navbar is hidden when opening this app from the index page.
    pub fn hide_navbar_on_main_page_link(spec: &ProxySpec) -> bool {
        Self::extension(spec)
            .hide_navbar_on_main_page_link
            .unwrap_or(false)
    }

    /// Whether the switch-instance dialog is opened instead of the app.
    pub fn always_show_switch_instance(spec: &ProxySpec, default: bool) -> bool {
        Self::extension(spec)
            .always_show_switch_instance
            .unwrap_or(default)
    }

    /// Whether a full page reload is forced when the app reconnects.
    pub fn shiny_force_full_reload(spec: &ProxySpec) -> bool {
        Self::extension(spec)
            .shiny_force_full_reload
            .unwrap_or(false)
    }
}

impl SpecProvider for ShinyProxySpecProvider {
    fn specs(&self) -> &[ProxySpec] {
        &self.specs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(yaml: &str) -> ShinyProxySpecProvider {
        let settings: Settings = serde_yaml_ng::from_str(yaml).expect("settings");
        ShinyProxySpecProvider::from_settings(&settings).expect("specs")
    }

    #[test]
    fn converts_the_demo_configuration() {
        let provider = provider(crate::DEMO_CONFIG);
        assert_eq!(provider.specs().len(), 2);

        let hello = provider.spec("01_hello").expect("01_hello");
        assert_eq!(hello.display_name.as_deref(), Some("Hello Application"));
        assert_eq!(
            hello.description.as_deref(),
            Some("Application which demonstrates the basics of a Shiny app")
        );
        assert_eq!(
            hello.access_control.groups(),
            ["scientists", "mathematicians"]
        );
        assert_eq!(hello.max_total_instances, -1);

        let container = hello.container_spec().expect("container spec");
        assert_eq!(container.index, 0);
        assert_eq!(
            container.image.original().map(String::as_str),
            Some("openanalytics/shinyproxy-demo")
        );
        assert_eq!(
            container.cmd.original().unwrap(),
            &vec![
                "R".to_string(),
                "-e".to_string(),
                "shinyproxy::run_01_hello()".to_string()
            ]
        );

        // a default port mapping for the Shiny port is created automatically
        assert_eq!(container.port_mapping.len(), 1);
        assert_eq!(container.port_mapping[0].name, "default");
        assert_eq!(container.port_mapping[0].port, Some(3838));

        // scalar access-groups notation is accepted as well
        let tabsets = provider.spec("06_tabsets").expect("06_tabsets");
        assert_eq!(tabsets.access_control.groups(), ["scientists"]);
    }

    #[test]
    fn maps_all_container_fields_and_additional_port_mappings() {
        let provider = provider(
            "proxy:\n  specs:\n    - id: full\n      container-image: img\n      container-env:\n        A: '1'\n      container-env-file: /tmp/env\n      container-network: net\n      container-network-connections: [ a, b ]\n      container-dns: [ 8.8.8.8 ]\n      container-volumes: [ '/tmp:/tmp' ]\n      container-memory-request: 1g\n      container-memory-limit: 2g\n      container-cpu-request: 1\n      container-cpu-limit: 2\n      container-privileged: true\n      container-resource-name: my-app\n      labels:\n        team: ds\n      port: 8080\n      target-path: /app\n      additional-port-mappings:\n        - name: dash\n          port: 9090\n          target-path: /dash\n      docker-registry-domain: registry.example.com\n      docker-registry-username: user\n      docker-registry-password: pass\n      docker-runtime: nvidia\n      docker-user: '1000'\n      docker-ipc: host\n      docker-group-add: [ '100' ]\n      docker-swarm-secrets:\n        - name: secret\n          target: /run/secret\n      docker-device-requests:\n        - driver: nvidia\n          count: 1\n          capabilities: [ [ gpu ] ]\n      max-lifetime: 120\n      heartbeat-timeout: 90000\n      stop-on-logout: true\n      max-total-instances: 5\n      add-default-http-headers: false\n      http-headers:\n        X-Custom: value\n      cache-headers-mode: Passthrough\n",
        );
        let spec = provider.spec("full").expect("spec");
        let container = spec.container_spec().expect("container");

        assert_eq!(
            container
                .env
                .original()
                .unwrap()
                .get("A")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            container.env_file.original().map(String::as_str),
            Some("/tmp/env")
        );
        assert_eq!(
            container.network.original().map(String::as_str),
            Some("net")
        );
        assert_eq!(container.network_connections.original().unwrap().len(), 2);
        assert_eq!(
            container.dns.original().unwrap(),
            &vec!["8.8.8.8".to_string()]
        );
        assert_eq!(
            container.volumes.original().unwrap(),
            &vec!["/tmp:/tmp".to_string()]
        );
        assert_eq!(
            container.memory_request.original().map(String::as_str),
            Some("1g")
        );
        assert_eq!(
            container.memory_limit.original().map(String::as_str),
            Some("2g")
        );
        assert_eq!(
            container.cpu_request.original().map(String::as_str),
            Some("1")
        );
        assert_eq!(
            container.cpu_limit.original().map(String::as_str),
            Some("2")
        );
        assert!(container.privileged);
        assert_eq!(
            container.resource_name.original().map(String::as_str),
            Some("my-app")
        );
        assert_eq!(
            container
                .labels
                .original()
                .unwrap()
                .get("team")
                .map(String::as_str),
            Some("ds")
        );
        assert_eq!(
            container.docker_registry_domain.as_deref(),
            Some("registry.example.com")
        );
        assert_eq!(
            container.docker_runtime.original().map(String::as_str),
            Some("nvidia")
        );
        assert_eq!(
            container.docker_user.original().map(String::as_str),
            Some("1000")
        );
        assert_eq!(
            container.docker_ipc.original().map(String::as_str),
            Some("host")
        );
        assert_eq!(
            container.docker_group_add.original().unwrap(),
            &vec!["100".to_string()]
        );
        assert_eq!(
            container.docker_swarm_secrets[0].name.as_deref(),
            Some("secret")
        );
        assert_eq!(container.docker_device_requests[0].count, Some(1));
        assert_eq!(
            container.docker_device_requests[0].capabilities,
            vec![vec!["gpu".to_string()]]
        );

        // the configured port becomes the default mapping, additional mappings come first
        assert_eq!(container.port_mapping.len(), 2);
        assert_eq!(container.port_mapping[0].name, "dash");
        assert_eq!(container.port_mapping[0].port, Some(9090));
        assert_eq!(
            container.port_mapping[0]
                .target_path
                .original()
                .map(String::as_str),
            Some("/dash")
        );
        assert_eq!(container.port_mapping[1].name, "default");
        assert_eq!(container.port_mapping[1].port, Some(8080));
        assert_eq!(
            container.port_mapping[1]
                .target_path
                .original()
                .map(String::as_str),
            Some("/app")
        );

        assert_eq!(
            spec.max_lifetime.original().map(String::as_str),
            Some("120")
        );
        assert_eq!(
            spec.heartbeat_timeout.original().map(String::as_str),
            Some("90000")
        );
        assert_eq!(spec.stop_on_logout, Some(true));
        assert_eq!(spec.max_total_instances, 5);
        assert_eq!(spec.add_default_http_headers, Some(false));
        assert_eq!(
            spec.http_headers
                .original()
                .unwrap()
                .get("X-Custom")
                .map(String::as_str),
            Some("value")
        );
        assert_eq!(spec.cache_headers_mode, Some(CacheHeadersMode::Passthrough));
    }

    #[test]
    fn reads_shinyproxy_and_external_extensions() {
        let provider = provider(
            "proxy:\n  specs:\n    - id: app\n      container-image: img\n      websocket-reconnection-mode: Confirm\n      shiny-force-full-reload: true\n      max-instances: 3\n      hide-navbar-on-main-page-link: true\n      always-show-switch-instance: true\n      track-app-url: true\n      template-group: reporting\n      template-properties:\n        category: finance\n      support-mail-to-address: support@example.com\n      custom-app-details:\n        - name: Dataset\n          description: The dataset\n          value: \"#{proxy.userId}\"\n    - id: external\n      external-url: https://example.com\n",
        );

        let spec = provider.spec("app").expect("app");
        let extension = ShinyProxySpecProvider::extension(spec);
        assert_eq!(
            extension.websocket_reconnection_mode,
            Some(WebsocketReconnectionMode::Confirm)
        );
        assert_eq!(extension.shiny_force_full_reload, Some(true));
        assert_eq!(
            extension.max_instances.original().map(String::as_str),
            Some("3")
        );
        assert_eq!(extension.template_group.as_deref(), Some("reporting"));
        assert_eq!(
            extension
                .template_properties
                .get("category")
                .map(String::as_str),
            Some("finance")
        );
        assert_eq!(
            extension.support_mail_to_address.as_deref(),
            Some("support@example.com")
        );
        assert_eq!(
            extension.custom_app_details[0].name.as_deref(),
            Some("Dataset")
        );
        assert_eq!(
            extension.custom_app_details[0].value.as_deref(),
            Some("#{proxy.userId}")
        );
        assert!(ShinyProxySpecProvider::hide_navbar_on_main_page_link(spec));
        assert!(ShinyProxySpecProvider::always_show_switch_instance(
            spec, false
        ));
        assert!(ShinyProxySpecProvider::shiny_force_full_reload(spec));

        let external = provider.spec("external").expect("external");
        assert_eq!(
            ShinyProxySpecProvider::external(external)
                .external_url
                .as_deref(),
            Some("https://example.com")
        );
        assert!(ShinyProxySpecProvider::external(spec)
            .external_url
            .is_none());
    }

    #[test]
    fn coerces_numeric_and_boolean_template_properties() {
        let provider = provider(
            "proxy:\n  template-groups:\n    - id: tools\n      properties:\n        display-name: Tools\n        order: 1\n  specs:\n    - id: app\n      container-image: img\n      shiny-force-full-reload: true\n      template-properties:\n        category: energy\n        type: shiny\n        icon: fa-bolt\n        startup-time: 20\n        featured: true\n",
        );

        let extension = ShinyProxySpecProvider::extension(provider.spec("app").expect("app"));
        assert_eq!(extension.shiny_force_full_reload, Some(true));
        assert_eq!(
            extension
                .template_properties
                .get("category")
                .map(String::as_str),
            Some("energy")
        );
        assert_eq!(
            extension
                .template_properties
                .get("type")
                .map(String::as_str),
            Some("shiny")
        );
        assert_eq!(
            extension
                .template_properties
                .get("icon")
                .map(String::as_str),
            Some("fa-bolt")
        );
        assert_eq!(
            extension
                .template_properties
                .get("startup-time")
                .map(String::as_str),
            Some("20")
        );
        assert_eq!(
            extension
                .template_properties
                .get("featured")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            provider.template_groups()[0]
                .properties
                .get("order")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn keeps_backend_specific_extensions() {
        let provider = provider(
            "proxy:\n  specs:\n    - id: app\n      container-image: img\n      kubernetes-pod-patches: 'patch'\n      kubernetes-additional-manifests: [ 'manifest' ]\n      ecs-task-role: role\n      minimum-seats-available: 2\n      seats-per-container: 4\n",
        );
        let spec = provider.spec("app").expect("app");
        assert_eq!(
            spec.spec_extensions.raw(KUBERNETES_EXTENSION).unwrap()["pod-patches"],
            serde_json::json!("patch")
        );
        assert_eq!(
            spec.spec_extensions.raw(KUBERNETES_EXTENSION).unwrap()["additional-manifests"],
            serde_json::json!(["manifest"])
        );
        assert_eq!(
            spec.spec_extensions.raw(ECS_EXTENSION).unwrap()["task-role"],
            serde_json::json!("role")
        );
        assert_eq!(
            spec.spec_extensions.raw(PROXY_SHARING_EXTENSION).unwrap()["seats-per-container"],
            serde_json::json!(4)
        );
    }

    #[test]
    fn reads_template_groups() {
        let provider = provider(
            "proxy:\n  template-groups:\n    - id: reporting\n      properties:\n        display-name: Reporting\n  specs:\n    - id: app\n      container-image: img\n",
        );
        assert_eq!(provider.template_groups().len(), 1);
        assert_eq!(provider.template_groups()[0].id, "reporting");
        assert_eq!(
            provider.template_groups()[0]
                .properties
                .get("display-name")
                .map(String::as_str),
            Some("Reporting")
        );
    }

    #[test]
    fn rejects_duplicate_and_missing_ids() {
        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  specs:\n    - id: same\n      container-image: a\n    - id: same\n      container-image: b\n",
        )
        .unwrap();
        let error = ShinyProxySpecProvider::from_settings(&settings).unwrap_err();
        assert!(
            matches!(&error, SpecError::DuplicateId(id) if id == "same"),
            "{error}"
        );

        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  specs:\n    - container-image: a\n").unwrap();
        let error = ShinyProxySpecProvider::from_settings(&settings).unwrap_err();
        assert!(
            matches!(error, SpecError::MissingId { index: 0 }),
            "{error}"
        );
    }

    #[test]
    fn parameters_are_kept() {
        let provider = provider(
            "proxy:\n  specs:\n    - id: app\n      container-image: img\n      parameters:\n        definitions:\n          - id: resources\n            display-name: Resources\n            value-names:\n              - value: 1-2\n                name: small\n        value-sets:\n          - values:\n              resources: [ 1-2 ]\n",
        );
        let spec = provider.spec("app").expect("app");
        let parameters = spec.parameters.as_ref().expect("parameters");
        assert_eq!(parameters.ids(), vec!["resources".to_string()]);
        assert_eq!(
            parameters.definitions[0].name_of_value("1-2"),
            Some("small")
        );
        assert!(parameters.value_sets[0].contains_parameter("resources"));
    }
}
