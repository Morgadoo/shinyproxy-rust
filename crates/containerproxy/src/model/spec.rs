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

//! App definitions (`ProxySpec` and friends).
//!
//! A spec is the *configuration* of an app; a [`super::proxy::Proxy`] is a *running instance* of it.
//! Expressions in a spec are resolved in two phases, exactly like in the Java implementation:
//!
//! * `first_resolve` runs before the containers exist: image, command, volumes, limits, port mappings,
//!   lifetimes;
//! * `final_resolve` runs when the runtime values of the proxy are known: environment variables,
//!   labels, resource names and HTTP headers (which may refer to those runtime values).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::spel_field::{
    ResolveError, SpecResolver, SpelLong, SpelString, SpelStringList, SpelStringMap,
};

/// How cache headers of an app are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CacheHeadersMode {
    /// Always send no-cache headers (default).
    #[default]
    EnforceNoCache,
    /// Keep whatever the app sends.
    Passthrough,
    /// Cache static assets, no-cache for everything else.
    EnforceCacheAssets,
}

/// Access control of an app (or of a value set / pod patch).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AccessControl {
    /// Groups that may access the app.
    pub groups: Vec<String>,
    /// Users that may access the app.
    pub users: Vec<String>,
    /// Expression that grants access when it evaluates to true.
    pub expression: Option<String>,
    /// Expression that must always evaluate to true.
    pub strict_expression: Option<String>,
}

impl AccessControl {
    /// Whether groups are configured.
    pub fn has_group_access(&self) -> bool {
        !self.groups.is_empty()
    }

    /// Whether users are configured.
    pub fn has_user_access(&self) -> bool {
        !self.users.is_empty()
    }

    /// Whether an access expression is configured.
    pub fn has_expression_access(&self) -> bool {
        self.expression
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Whether a strict access expression is configured.
    pub fn has_strict_expression_access(&self) -> bool {
        self.strict_expression
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Whether no access control at all is configured (everyone has access).
    pub fn is_open(&self) -> bool {
        !self.has_group_access() && !self.has_user_access() && !self.has_expression_access()
    }
}

/// A port of the container that is exposed through ShinyProxy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PortMapping {
    /// Name of the mapping; `default` is the app itself, other names become sub-paths.
    pub name: String,
    /// Port inside the container.
    pub port: Option<i64>,
    /// Path inside the container the mapping points to.
    pub target_path: SpelString,
}

impl PortMapping {
    /// The default mapping of an app (`default`, port 3838 for Shiny apps).
    pub fn default_mapping(port: i64) -> Self {
        PortMapping {
            name: "default".to_string(),
            port: Some(port),
            target_path: SpelString::empty(),
        }
    }

    /// Resolves the expressions of this mapping.
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        Ok(PortMapping {
            target_path: self.target_path.resolve(resolver)?,
            ..self.clone()
        })
    }
}

/// A Docker Swarm secret mounted into the container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DockerSwarmSecret {
    pub name: Option<String>,
    pub target: Option<String>,
    pub uid: Option<String>,
    pub gid: Option<String>,
    pub mode: Option<String>,
}

/// A device request (GPUs) for the Docker backend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DockerDeviceRequest {
    pub driver: Option<String>,
    pub count: Option<i64>,
    pub device_ids: Vec<String>,
    pub capabilities: Vec<Vec<String>>,
    pub options: BTreeMap<String, String>,
}

/// Parameters of an app: users choose values before the app starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Parameters {
    /// Definitions of the parameters, in the order they are shown.
    pub definitions: Vec<ParameterDefinition>,
    /// Allowed combinations of values, optionally restricted to certain users.
    pub value_sets: Vec<ParameterValueSet>,
    /// Optional Thymeleaf/MiniJinja template rendering the parameter form.
    pub template: Option<String>,
}

impl Parameters {
    /// Ids of the parameters, in definition order.
    pub fn ids(&self) -> Vec<String> {
        self.definitions
            .iter()
            .map(|definition| definition.id.clone())
            .collect()
    }
}

/// One parameter of an app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ParameterDefinition {
    /// Identifier used in the backend.
    pub id: String,
    /// Name shown to the user.
    pub display_name: Option<String>,
    /// Description shown to the user (sanitised HTML).
    pub description: Option<String>,
    /// Value selected by default.
    pub default_value: Option<String>,
    /// Human friendly names of the values.
    pub value_names: Vec<ParameterValueName>,
}

impl ParameterDefinition {
    /// Display name, falling back to the id (`getDisplayNameOrId`).
    pub fn display_name_or_id(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }

    /// Human friendly name of a backend value.
    pub fn name_of_value(&self, value: &str) -> Option<&str> {
        self.value_names
            .iter()
            .find(|mapping| mapping.value.as_deref() == Some(value))
            .and_then(|mapping| mapping.name.as_deref())
    }

    /// Backend value of a human friendly name.
    pub fn value_of_name(&self, name: &str) -> Option<&str> {
        self.value_names
            .iter()
            .find(|mapping| mapping.name.as_deref() == Some(name))
            .and_then(|mapping| mapping.value.as_deref())
    }
}

/// Mapping of a backend value to the name shown to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ParameterValueName {
    pub value: Option<String>,
    pub name: Option<String>,
}

/// A set of allowed parameter values, optionally restricted by access control.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ParameterValueSet {
    /// Optional name (used in log messages).
    pub name: Option<String>,
    /// Who may use this value set.
    pub access_control: Option<AccessControl>,
    /// Allowed values per parameter id.
    pub values: BTreeMap<String, Vec<String>>,
}

impl ParameterValueSet {
    /// Whether this value set defines values for the given parameter.
    pub fn contains_parameter(&self, parameter_id: &str) -> bool {
        self.values.contains_key(parameter_id)
    }
}

/// Extra, backend or application specific parts of an app definition (`ISpecExtension`).
///
/// The engine keeps them as JSON documents so that applications and backends can interpret their own
/// fields without the engine knowing about them; the ShinyProxy crate and the backends deserialize the
/// documents they own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpecExtensions {
    extensions: BTreeMap<String, Value>,
}

impl SpecExtensions {
    /// An empty set of extensions.
    pub fn new() -> Self {
        SpecExtensions::default()
    }

    /// Stores an extension document under the given name.
    pub fn insert(&mut self, name: impl Into<String>, value: Value) {
        self.extensions.insert(name.into(), value);
    }

    /// The raw document of an extension.
    pub fn raw(&self, name: &str) -> Option<&Value> {
        self.extensions.get(name)
    }

    /// Deserializes an extension into its type, returning the default when it is absent.
    pub fn get<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> T {
        match self.extensions.get(name) {
            Some(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
            None => T::default(),
        }
    }

    /// Names of the stored extensions.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.extensions.keys()
    }
}

/// The container part of an app definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContainerSpec {
    /// Index in the list of container specs of the proxy spec.
    pub index: i64,
    pub image: SpelString,
    pub cmd: SpelStringList,
    pub env: SpelStringMap,
    pub env_file: SpelString,
    pub network: SpelString,
    pub network_connections: SpelStringList,
    pub dns: SpelStringList,
    pub volumes: SpelStringList,
    pub port_mapping: Vec<PortMapping>,
    pub privileged: bool,
    pub memory_request: SpelString,
    pub memory_limit: SpelString,
    pub cpu_request: SpelString,
    pub cpu_limit: SpelString,
    pub labels: SpelStringMap,
    pub docker_swarm_secrets: Vec<DockerSwarmSecret>,
    pub docker_registry_domain: Option<String>,
    pub docker_registry_username: Option<String>,
    pub docker_registry_password: Option<String>,
    pub docker_runtime: SpelString,
    pub docker_user: SpelString,
    pub docker_ipc: SpelString,
    pub docker_group_add: SpelStringList,
    pub docker_device_requests: Vec<DockerDeviceRequest>,
    pub resource_name: SpelString,
}

impl ContainerSpec {
    /// First resolution phase (before the containers exist).
    pub fn first_resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        let mut port_mapping = Vec::with_capacity(self.port_mapping.len());
        for mapping in &self.port_mapping {
            port_mapping.push(mapping.resolve(resolver)?);
        }
        Ok(ContainerSpec {
            image: self.image.resolve(resolver)?,
            cmd: self.cmd.resolve(resolver)?,
            env_file: self.env_file.resolve(resolver)?,
            network: self.network.resolve(resolver)?,
            network_connections: self.network_connections.resolve(resolver)?,
            dns: self.dns.resolve(resolver)?,
            volumes: self.volumes.resolve(resolver)?,
            memory_request: self.memory_request.resolve(resolver)?,
            memory_limit: self.memory_limit.resolve(resolver)?,
            cpu_request: self.cpu_request.resolve(resolver)?,
            cpu_limit: self.cpu_limit.resolve(resolver)?,
            port_mapping,
            docker_runtime: self.docker_runtime.resolve(resolver)?,
            docker_user: self.docker_user.resolve(resolver)?,
            docker_ipc: self.docker_ipc.resolve(resolver)?,
            docker_group_add: self.docker_group_add.resolve(resolver)?,
            ..self.clone()
        })
    }

    /// Final resolution phase (once the runtime values of the proxy are known).
    pub fn final_resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        Ok(ContainerSpec {
            env: self.env.resolve(resolver)?,
            labels: self.labels.resolve(resolver)?,
            resource_name: self.resource_name.resolve(resolver)?,
            ..self.clone()
        })
    }

    /// The port mapping with the given name.
    pub fn port_mapping(&self, name: &str) -> Option<&PortMapping> {
        self.port_mapping
            .iter()
            .find(|mapping| mapping.name == name)
    }
}

/// An app definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxySpec {
    /// Identifier of the app, used in URLs.
    pub id: String,
    /// Name shown in the UI.
    pub display_name: Option<String>,
    /// Description shown in the UI (may contain limited HTML).
    pub description: Option<String>,
    /// Logo shown in the UI.
    pub logo_url: Option<String>,
    pub logo_width: Option<String>,
    pub logo_height: Option<String>,
    pub logo_style: Option<String>,
    pub logo_classes: Option<String>,
    /// Favicon of the app page.
    pub favicon_path: Option<String>,
    /// Who may use this app.
    pub access_control: AccessControl,
    /// Containers of the app (ShinyProxy always defines exactly one).
    pub container_specs: Vec<ContainerSpec>,
    /// Parameters the user chooses before starting the app.
    pub parameters: Option<Parameters>,
    /// Maximum lifetime in minutes (`-1` for unlimited).
    pub max_lifetime: SpelLong,
    /// Whether the app is stopped when the user logs out.
    pub stop_on_logout: Option<bool>,
    /// Heartbeat timeout in milliseconds.
    pub heartbeat_timeout: SpelLong,
    /// Extra headers sent to the app.
    pub http_headers: SpelStringMap,
    /// Whether `X-SP-UserId`/`X-SP-UserGroups` are sent to the app (default true).
    pub add_default_http_headers: Option<bool>,
    /// How cache headers are handled.
    pub cache_headers_mode: Option<CacheHeadersMode>,
    /// Maximum number of instances of this app across all users (`-1` for unlimited).
    pub max_total_instances: i64,
    /// Application and backend specific extensions.
    pub spec_extensions: SpecExtensions,
}

impl ProxySpec {
    /// A spec with the given id.
    pub fn new(id: impl Into<String>) -> Self {
        ProxySpec {
            id: id.into(),
            max_total_instances: -1,
            ..Default::default()
        }
    }

    /// Assigns the index of every container spec (`setContainerIndex`).
    pub fn set_container_index(&mut self) {
        for (index, container) in self.container_specs.iter_mut().enumerate() {
            container.index = index as i64;
        }
    }

    /// The first (and for ShinyProxy: only) container spec.
    pub fn container_spec(&self) -> Option<&ContainerSpec> {
        self.container_specs.first()
    }

    /// Display name, falling back to the id.
    pub fn display_name_or_id(&self) -> &str {
        match &self.display_name {
            Some(name) if !name.is_empty() => name,
            _ => &self.id,
        }
    }

    /// First resolution phase.
    pub fn first_resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        let mut container_specs = Vec::with_capacity(self.container_specs.len());
        for container in &self.container_specs {
            container_specs.push(container.first_resolve(resolver)?);
        }
        Ok(ProxySpec {
            heartbeat_timeout: self.heartbeat_timeout.resolve(resolver)?,
            max_lifetime: self.max_lifetime.resolve(resolver)?,
            container_specs,
            ..self.clone()
        })
    }

    /// Final resolution phase.
    pub fn final_resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        let mut container_specs = Vec::with_capacity(self.container_specs.len());
        for container in &self.container_specs {
            container_specs.push(container.final_resolve(resolver)?);
        }
        Ok(ProxySpec {
            http_headers: self.http_headers.resolve(resolver)?,
            container_specs,
            ..self.clone()
        })
    }

    /// JSON representation for the API.
    ///
    /// With `hide_details` (the default, `proxy.api-security.hide-spec-details`) only the fields that
    /// the UI needs are returned; otherwise the full definition is returned, which may contain secrets.
    pub fn api_json(&self, hide_details: bool) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("id".into(), Value::String(self.id.clone()));
        object.insert(
            "displayName".into(),
            self.display_name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "description".into(),
            self.description
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "logoWidth".into(),
            self.logo_width
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "logoHeight".into(),
            self.logo_height
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "logoStyle".into(),
            self.logo_style
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "logoClasses".into(),
            self.logo_classes
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if !hide_details {
            object.insert(
                "logoURL".into(),
                self.logo_url
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            object.insert(
                "faviconPath".into(),
                self.favicon_path
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            object.insert(
                "containerSpecs".into(),
                serde_json::to_value(&self.container_specs).unwrap_or(Value::Null),
            );
            object.insert(
                "accessControl".into(),
                serde_json::to_value(&self.access_control).unwrap_or(Value::Null),
            );
            object.insert(
                "parameters".into(),
                serde_json::to_value(&self.parameters).unwrap_or(Value::Null),
            );
            object.insert(
                "maxLifeTime".into(),
                serde_json::to_value(&self.max_lifetime).unwrap_or(Value::Null),
            );
            object.insert(
                "heartbeatTimeout".into(),
                serde_json::to_value(&self.heartbeat_timeout).unwrap_or(Value::Null),
            );
            object.insert(
                "stopOnLogout".into(),
                serde_json::to_value(self.stop_on_logout).unwrap_or(Value::Null),
            );
            object.insert(
                "httpHeaders".into(),
                serde_json::to_value(&self.http_headers).unwrap_or(Value::Null),
            );
            object.insert(
                "addDefaultHttpHeaders".into(),
                serde_json::to_value(self.add_default_http_headers).unwrap_or(Value::Null),
            );
            object.insert(
                "cacheHeadersMode".into(),
                serde_json::to_value(self.cache_headers_mode).unwrap_or(Value::Null),
            );
            object.insert(
                "maxTotalInstances".into(),
                Value::from(self.max_total_instances),
            );
        }
        Value::Object(object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PassthroughResolver;

    impl SpecResolver for PassthroughResolver {
        fn string(&self, raw: &str) -> Result<String, ResolveError> {
            Ok(raw.replace("#{userId}", "jack"))
        }

        fn integer(&self, raw: &str) -> Result<i64, ResolveError> {
            self.string(raw)?
                .parse()
                .map_err(|_| ResolveError::new(raw, "not a number"))
        }

        fn boolean(&self, raw: &str) -> Result<bool, ResolveError> {
            Ok(self.string(raw)?.eq_ignore_ascii_case("true"))
        }
    }

    fn spec() -> ProxySpec {
        let mut spec = ProxySpec::new("01_hello");
        spec.display_name = Some("Hello Application".into());
        spec.max_lifetime = SpelLong::raw("120".into());
        spec.heartbeat_timeout = SpelLong::raw("60000".into());
        spec.http_headers = SpelStringMap::raw(BTreeMap::from([(
            "X-User".to_string(),
            "#{userId}".to_string(),
        )]));
        spec.container_specs = vec![ContainerSpec {
            image: SpelString::raw("openanalytics/shinyproxy-demo".into()),
            cmd: SpelStringList::raw(vec!["R".into(), "-e".into()]),
            env: SpelStringMap::raw(BTreeMap::from([(
                "USER".to_string(),
                "#{userId}".to_string(),
            )])),
            port_mapping: vec![PortMapping::default_mapping(3838)],
            ..Default::default()
        }];
        spec.set_container_index();
        spec
    }

    #[test]
    fn resolves_in_two_phases() {
        let spec = spec();
        let first = spec
            .first_resolve(&PassthroughResolver)
            .expect("first resolve");
        assert_eq!(first.max_lifetime.value(), Some(&120));
        assert_eq!(first.heartbeat_timeout.value(), Some(&60000));
        assert_eq!(
            first.container_specs[0].image.as_str(),
            Some("openanalytics/shinyproxy-demo")
        );
        // env/labels/headers are only resolved in the final phase
        assert!(!first.container_specs[0].env.is_resolved());
        assert!(!first.http_headers.is_resolved());

        let final_spec = first
            .final_resolve(&PassthroughResolver)
            .expect("final resolve");
        assert_eq!(
            final_spec.container_specs[0]
                .env
                .value()
                .and_then(|env| env.get("USER"))
                .map(String::as_str),
            Some("jack")
        );
        assert_eq!(
            final_spec
                .http_headers
                .value()
                .and_then(|headers| headers.get("X-User"))
                .map(String::as_str),
            Some("jack")
        );
        // values resolved in the first phase survive
        assert_eq!(final_spec.max_lifetime.value(), Some(&120));
    }

    #[test]
    fn access_control_helpers() {
        let mut access = AccessControl::default();
        assert!(access.is_open());
        access.groups = vec!["scientists".into()];
        assert!(access.has_group_access());
        assert!(!access.is_open());

        let mut access = AccessControl {
            expression: Some(" ".into()),
            ..Default::default()
        };
        assert!(
            !access.has_expression_access(),
            "blank expressions do not count"
        );
        access.expression = Some("#{true}".into());
        assert!(access.has_expression_access());
    }

    #[test]
    fn container_indexes_are_assigned() {
        let mut spec = ProxySpec::new("multi");
        spec.container_specs = vec![ContainerSpec::default(), ContainerSpec::default()];
        spec.set_container_index();
        assert_eq!(spec.container_specs[0].index, 0);
        assert_eq!(spec.container_specs[1].index, 1);
    }

    #[test]
    fn api_json_hides_details_by_default() {
        let spec = spec();
        let hidden = spec.api_json(true);
        assert_eq!(hidden["id"], serde_json::json!("01_hello"));
        assert_eq!(
            hidden["displayName"],
            serde_json::json!("Hello Application")
        );
        assert!(hidden.get("containerSpecs").is_none());
        assert!(hidden.get("accessControl").is_none());

        let full = spec.api_json(false);
        assert_eq!(
            full["containerSpecs"][0]["image"],
            serde_json::json!("openanalytics/shinyproxy-demo")
        );
        assert_eq!(full["maxTotalInstances"], serde_json::json!(-1));
    }

    #[test]
    fn parameter_definitions_map_values_to_names() {
        let definition = ParameterDefinition {
            id: "resources".into(),
            display_name: Some("Resources".into()),
            value_names: vec![
                ParameterValueName {
                    value: Some("1-2".into()),
                    name: Some("1 CPU core".into()),
                },
                ParameterValueName {
                    value: Some("2-8".into()),
                    name: Some("2 CPU cores".into()),
                },
            ],
            ..Default::default()
        };
        assert_eq!(definition.display_name_or_id(), "Resources");
        assert_eq!(definition.name_of_value("2-8"), Some("2 CPU cores"));
        assert_eq!(definition.value_of_name("1 CPU core"), Some("1-2"));
        assert_eq!(definition.name_of_value("unknown"), None);

        let definition = ParameterDefinition {
            id: "dataset".into(),
            ..Default::default()
        };
        assert_eq!(definition.display_name_or_id(), "dataset");
    }

    #[test]
    fn spec_extensions_are_typed_on_demand() {
        #[derive(Debug, Default, Deserialize, PartialEq)]
        #[serde(default, rename_all = "kebab-case")]
        struct Example {
            external_url: Option<String>,
        }

        let mut extensions = SpecExtensions::new();
        extensions.insert(
            "external",
            serde_json::json!({"external-url": "https://example.com"}),
        );
        let example: Example = extensions.get("external");
        assert_eq!(example.external_url.as_deref(), Some("https://example.com"));
        let missing: Example = extensions.get("nope");
        assert_eq!(missing, Example::default());
    }
}
