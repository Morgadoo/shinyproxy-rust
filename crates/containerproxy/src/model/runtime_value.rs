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

//! Runtime values: the metadata attached to a running proxy.
//!
//! Every runtime value has a key that decides
//!
//! * the container **label**/**annotation** it is stored in (`openanalytics.eu/sp-*`), which is how app
//!   recovery reconstructs proxies after a restart;
//! * the **environment variable** it is exposed as inside the container (`SHINYPROXY_*`);
//! * whether it may be returned by the **API** (security sensitive values may not);
//! * whether it belongs to the proxy or to a single container.
//!
//! The definitions below mirror `eu.openanalytics.containerproxy.model.runtime.runtimevalues` exactly;
//! the flags are asserted against the Java sources in the tests.

use std::collections::BTreeMap;

use serde_json::Value;

/// How the value of a runtime value is (de)serialized when stored as a string (label/annotation/Redis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Plain string.
    Str,
    /// Signed integer.
    Integer,
    /// Boolean (`true`/`false`).
    Bool,
    /// JSON document.
    Json,
    /// A backend container name (`namespace/name`), stored as its plain value.
    BackendContainerName,
}

/// Metadata of a runtime value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeValueKey {
    /// Label/annotation used on containers, e.g. `openanalytics.eu/sp-proxy-id`.
    pub label: &'static str,
    /// Environment variable name, e.g. `SHINYPROXY_PROXY_ID`.
    pub env_var: &'static str,
    /// Whether the value is added as a container label.
    pub include_as_label: bool,
    /// Whether the value is added as a container annotation (Kubernetes) / label (Docker).
    pub include_as_annotation: bool,
    /// Whether the value is injected into the container as an environment variable.
    pub include_as_env: bool,
    /// Whether the value may be returned by the API.
    pub include_in_api: bool,
    /// Whether the value must be present (used by app recovery).
    pub required: bool,
    /// Whether the value belongs to a container instead of the proxy.
    pub container_specific: bool,
    /// Type of the value.
    pub kind: ValueKind,
}

impl RuntimeValueKey {
    /// Parses a value of this key from its string representation.
    pub fn parse(&self, raw: &str) -> Result<RuntimeValueData, RuntimeValueError> {
        match self.kind {
            ValueKind::Str => Ok(RuntimeValueData::Str(raw.to_string())),
            ValueKind::Integer => raw
                .trim()
                .parse::<i64>()
                .map(RuntimeValueData::Int)
                .map_err(|_| RuntimeValueError::InvalidValue {
                    key: self.env_var,
                    value: raw.to_string(),
                    expected: "an integer",
                }),
            ValueKind::Bool => Ok(RuntimeValueData::Bool(
                raw.trim().eq_ignore_ascii_case("true"),
            )),
            ValueKind::Json => serde_json::from_str(raw)
                .map(RuntimeValueData::Json)
                .map_err(|_| RuntimeValueError::InvalidValue {
                    key: self.env_var,
                    value: raw.to_string(),
                    expected: "a JSON document",
                }),
            ValueKind::BackendContainerName => Ok(RuntimeValueData::Json(
                serde_json::to_value(BackendContainerName::new(raw)).expect("serializable"),
            )),
        }
    }
}

/// Errors related to runtime values.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeValueError {
    #[error("value '{value}' of runtime value {key} is invalid, expected {expected}")]
    InvalidValue {
        key: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("unknown runtime value '{0}'")]
    UnknownKey(String),
}

/// The value of a runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValueData {
    /// A string.
    Str(String),
    /// An integer.
    Int(i64),
    /// A boolean.
    Bool(bool),
    /// A structured value (port mappings, http headers, parameters, ...).
    Json(Value),
}

impl RuntimeValueData {
    /// JSON representation, as returned by the API and stored in Redis.
    pub fn to_json(&self) -> Value {
        match self {
            RuntimeValueData::Str(value) => Value::String(value.clone()),
            RuntimeValueData::Int(value) => Value::Number((*value).into()),
            RuntimeValueData::Bool(value) => Value::Bool(*value),
            RuntimeValueData::Json(value) => value.clone(),
        }
    }

    /// The value as a string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            RuntimeValueData::Str(value) => Some(value),
            _ => None,
        }
    }

    /// The value as an integer, if it is one.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            RuntimeValueData::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            RuntimeValueData::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Deserializes a structured value.
    pub fn parse_json<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.to_json()).ok()
    }
}

/// A runtime value: a key and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeValue {
    /// Metadata of the value.
    pub key: &'static RuntimeValueKey,
    /// The value itself.
    pub data: RuntimeValueData,
}

impl RuntimeValue {
    /// Creates a string valued runtime value.
    pub fn string(key: &'static RuntimeValueKey, value: impl Into<String>) -> Self {
        RuntimeValue {
            key,
            data: RuntimeValueData::Str(value.into()),
        }
    }

    /// Creates an integer valued runtime value.
    pub fn integer(key: &'static RuntimeValueKey, value: i64) -> Self {
        RuntimeValue {
            key,
            data: RuntimeValueData::Int(value),
        }
    }

    /// Creates a boolean valued runtime value.
    pub fn boolean(key: &'static RuntimeValueKey, value: bool) -> Self {
        RuntimeValue {
            key,
            data: RuntimeValueData::Bool(value),
        }
    }

    /// Creates a structured runtime value.
    pub fn json(key: &'static RuntimeValueKey, value: impl serde::Serialize) -> Self {
        RuntimeValue {
            key,
            data: RuntimeValueData::Json(serde_json::to_value(value).expect("serializable value")),
        }
    }

    /// Parses a runtime value from its string representation.
    pub fn parse(key: &'static RuntimeValueKey, raw: &str) -> Result<Self, RuntimeValueError> {
        Ok(RuntimeValue {
            key,
            data: key.parse(raw)?,
        })
    }

    /// String representation, used for labels, annotations, environment variables and Redis.
    ///
    /// Equivalent to `RuntimeValueKey#serializeToString` combined with `RuntimeValue#toString`.
    pub fn to_value_string(&self) -> String {
        match (&self.data, self.key.kind) {
            (RuntimeValueData::Str(value), _) => value.clone(),
            (RuntimeValueData::Int(value), _) => value.to_string(),
            (RuntimeValueData::Bool(value), _) => value.to_string(),
            (RuntimeValueData::Json(value), ValueKind::BackendContainerName) => value
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            (RuntimeValueData::Json(value), _) => {
                serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
            }
        }
    }
}

/// A collection of runtime values, keyed by their environment variable name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeValues {
    values: BTreeMap<&'static str, RuntimeValue>,
}

impl RuntimeValues {
    /// An empty collection.
    pub fn new() -> Self {
        RuntimeValues::default()
    }

    /// Adds a value; existing values are only replaced when `override_existing` is set (this mirrors
    /// the `addRuntimeValue(value, override)` semantics of the Java builders).
    pub fn add(&mut self, value: RuntimeValue, override_existing: bool) {
        let key = value.key.env_var;
        if override_existing || !self.values.contains_key(key) {
            self.values.insert(key, value);
        }
    }

    /// Adds all values without overriding existing ones.
    pub fn add_all(&mut self, values: impl IntoIterator<Item = RuntimeValue>) {
        for value in values {
            self.add(value, false);
        }
    }

    /// Looks up a value by key.
    pub fn get(&self, key: &RuntimeValueKey) -> Option<&RuntimeValue> {
        self.values.get(key.env_var)
    }

    /// Looks up a value by environment variable name (used by SpEL expressions).
    pub fn get_by_env_var(&self, env_var: &str) -> Option<&RuntimeValue> {
        self.values.get(env_var)
    }

    /// String representation of a value, if present.
    pub fn value_string(&self, key: &RuntimeValueKey) -> Option<String> {
        self.get(key).map(|value| value.to_value_string())
    }

    /// Number of values.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether there are no values.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Iterates over all values.
    pub fn iter(&self) -> impl Iterator<Item = &RuntimeValue> {
        self.values.values()
    }

    /// The values that may be returned by the API, keyed by environment variable name.
    pub fn api_json(&self) -> BTreeMap<String, Value> {
        self.values
            .values()
            .filter(|value| value.key.include_in_api)
            .map(|value| (value.key.env_var.to_string(), value.data.to_json()))
            .collect()
    }

    /// All values as strings, keyed by environment variable name (internal representation used for
    /// Redis and app recovery, `_runtimeValues` in the Java API).
    pub fn internal_json(&self) -> BTreeMap<String, String> {
        self.values
            .values()
            .map(|value| (value.key.env_var.to_string(), value.to_value_string()))
            .collect()
    }

    /// Labels/annotations for the backend, i.e. the values with `include_as_label` or
    /// `include_as_annotation` set.
    pub fn labels(&self) -> BTreeMap<String, String> {
        self.values
            .values()
            .filter(|value| value.key.include_as_label || value.key.include_as_annotation)
            .map(|value| (value.key.label.to_string(), value.to_value_string()))
            .collect()
    }

    /// Environment variables to inject into the container.
    pub fn environment(&self) -> BTreeMap<String, String> {
        self.values
            .values()
            .filter(|value| value.key.include_as_env)
            .map(|value| (value.key.env_var.to_string(), value.to_value_string()))
            .collect()
    }
}

/// Registry of known runtime value keys, used to interpret labels and stored proxies.
#[derive(Debug, Clone)]
pub struct RuntimeValueRegistry {
    keys: Vec<&'static RuntimeValueKey>,
}

impl RuntimeValueRegistry {
    /// Registry containing the engine keys.
    pub fn engine() -> Self {
        RuntimeValueRegistry {
            keys: ENGINE_KEYS.to_vec(),
        }
    }

    /// Adds application specific keys (ShinyProxy contributes six of them).
    pub fn with_keys(mut self, keys: &[&'static RuntimeValueKey]) -> Self {
        self.keys.extend_from_slice(keys);
        self
    }

    /// All registered keys.
    pub fn keys(&self) -> &[&'static RuntimeValueKey] {
        &self.keys
    }

    /// Looks up a key by environment variable name.
    pub fn by_env_var(&self, env_var: &str) -> Option<&'static RuntimeValueKey> {
        self.keys.iter().copied().find(|key| key.env_var == env_var)
    }

    /// Looks up a key by label/annotation.
    pub fn by_label(&self, label: &str) -> Option<&'static RuntimeValueKey> {
        self.keys.iter().copied().find(|key| key.label == label)
    }

    /// Parses container labels into runtime values, ignoring labels that are not runtime values.
    ///
    /// Returns `None` when a required value is missing, which is how the Java implementation decides
    /// that a container is not a (recoverable) ShinyProxy container.
    pub fn parse_labels<'a>(
        &self,
        labels: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Option<RuntimeValues> {
        let mut values = RuntimeValues::new();
        for (label, raw) in labels {
            if let Some(key) = self.by_label(label) {
                match RuntimeValue::parse(key, raw) {
                    Ok(value) => values.add(value, true),
                    Err(error) => {
                        tracing::warn!("ignoring invalid runtime value on container: {error}");
                        return None;
                    }
                }
            }
        }
        // Java checks the keys that are stored as a label or annotation, including the container
        // specific ones, and logs which label it missed
        for key in &self.keys {
            if (key.include_as_label || key.include_as_annotation)
                && key.required
                && values.get(key).is_none()
            {
                tracing::warn!(
                    "Ignoring container because no label named {} is found",
                    key.label
                );
                return None;
            }
        }
        Some(values)
    }
}

/// A backend container name, optionally namespaced (`namespace/name`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackendContainerName {
    /// Full value as stored on the container.
    pub value: String,
    /// Name part.
    pub name: String,
    /// Namespace part (`default` when the value is not namespaced).
    pub namespace: String,
}

impl BackendContainerName {
    /// Parses a value of the form `name` or `namespace/name`.
    pub fn new(value: &str) -> Self {
        match value.split_once('/') {
            Some((namespace, name)) => BackendContainerName {
                value: value.to_string(),
                name: name.to_string(),
                namespace: namespace.to_string(),
            },
            None => BackendContainerName {
                value: value.to_string(),
                name: value.to_string(),
                namespace: "default".to_string(),
            },
        }
    }

    /// Builds a namespaced name.
    pub fn namespaced(namespace: &str, name: &str) -> Self {
        BackendContainerName {
            value: format!("{namespace}/{name}"),
            name: name.to_string(),
            namespace: namespace.to_string(),
        }
    }
}

macro_rules! runtime_value_key {
    (
        $ident:ident,
        $label:literal,
        $env:literal,
        label: $as_label:literal,
        annotation: $as_annotation:literal,
        env: $as_env:literal,
        api: $in_api:literal,
        required: $required:literal,
        container: $container:literal,
        kind: $kind:expr
    ) => {
        /// See the module documentation.
        pub static $ident: RuntimeValueKey = RuntimeValueKey {
            label: $label,
            env_var: $env,
            include_as_label: $as_label,
            include_as_annotation: $as_annotation,
            include_as_env: $as_env,
            include_in_api: $in_api,
            required: $required,
            container_specific: $container,
            kind: $kind,
        };
    };
}

runtime_value_key!(PROXY_ID, "openanalytics.eu/sp-proxy-id", "SHINYPROXY_PROXY_ID",
    label: false, annotation: true, env: false, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(PROXY_SPEC_ID, "openanalytics.eu/sp-spec-id", "SHINYPROXY_SPEC_ID",
    label: false, annotation: true, env: false, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(INSTANCE_ID, "openanalytics.eu/sp-instance", "SHINYPROXY_INSTANCE",
    label: true, annotation: false, env: false, api: true, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(REALM_ID, "openanalytics.eu/sp-realm-id", "SHINYPROXY_REALM_ID",
    label: false, annotation: true, env: true, api: false, required: false, container: false, kind: ValueKind::Str);
runtime_value_key!(USER_ID, "openanalytics.eu/sp-user-id", "SHINYPROXY_USERNAME",
    label: false, annotation: true, env: true, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(USER_GROUPS, "openanalytics.eu/sp-user-groups", "SHINYPROXY_USERGROUPS",
    label: false, annotation: true, env: true, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(DISPLAY_NAME, "openanalytics.eu/sp-display-name", "SHINYPROXY_DISPLAY_NAME",
    label: false, annotation: true, env: false, api: true, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(CREATED_TIMESTAMP, "openanalytics.eu/sp-proxy-created-timestamp", "SHINYPROXY_CREATED_TIMESTAMP",
    label: false, annotation: true, env: false, api: true, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(PROXIED_APP, "openanalytics.eu/sp-proxied-app", "SHINYPROXY_PROXIED_APP",
    label: true, annotation: false, env: false, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(PUBLIC_PATH, "openanalytics.eu/sp-public-path", "SHINYPROXY_PUBLIC_PATH",
    label: false, annotation: true, env: true, api: true, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(TARGET_ID, "openanalytics.eu/sp-target-id", "SHINYPROXY_TARGET_ID",
    label: false, annotation: true, env: false, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(HEARTBEAT_TIMEOUT, "openanalytics.eu/sp-heartbeat-timeout", "SHINYPROXY_HEARTBEAT_TIMEOUT",
    label: false, annotation: true, env: false, api: true, required: true, container: false, kind: ValueKind::Integer);
runtime_value_key!(MAX_LIFETIME, "openanalytics.eu/sp-max-lifetime", "SHINYPROXY_MAX_LIFETIME",
    label: false, annotation: true, env: false, api: true, required: true, container: false, kind: ValueKind::Integer);
runtime_value_key!(CACHE_HEADERS_MODE, "openanalytics.eu/sp-cache-headers-mode", "SHINYPROXY_CACHE_HEADERS_MODE",
    label: false, annotation: true, env: false, api: false, required: true, container: false, kind: ValueKind::Str);
runtime_value_key!(HTTP_HEADERS, "openanalytics.eu/sp-http-headers", "SHINYPROXY_HTTP_HEADERS",
    label: false, annotation: true, env: false, api: false, required: false, container: false, kind: ValueKind::Json);
runtime_value_key!(PARAMETER_NAMES, "openanalytics.eu/sp-parameters-names", "SHINYPROXY_PARAMETER_NAMES",
    label: false, annotation: true, env: false, api: true, required: false, container: false, kind: ValueKind::Json);
runtime_value_key!(PARAMETER_VALUES, "openanalytics.eu/sp-parameters", "SHINYPROXY_PARAMETERS",
    label: false, annotation: true, env: false, api: false, required: false, container: false, kind: ValueKind::Json);
runtime_value_key!(CONTAINER_INDEX, "openanalytics.eu/sp-container-index", "SHINYPROXY_CONTAINER_INDEX",
    label: false, annotation: true, env: false, api: true, required: true, container: true, kind: ValueKind::Integer);
runtime_value_key!(CONTAINER_IMAGE, "openanalytics.eu/sp-container-image", "SHINYPROXY_CONTAINER_IMAGE",
    label: false, annotation: false, env: false, api: false, required: false, container: true, kind: ValueKind::Str);
runtime_value_key!(PORT_MAPPINGS, "openanalytics.eu/sp-port-mappings", "SHINYPROXY_PORT_MAPPINGS",
    label: false, annotation: true, env: false, api: false, required: true, container: true, kind: ValueKind::Json);
runtime_value_key!(BACKEND_CONTAINER_NAME, "openanalytics.eu/sp-backend-container-name", "SHINYPROXY_BACKEND_CONTAINER_NAME",
    label: false, annotation: false, env: false, api: false, required: false, container: true, kind: ValueKind::BackendContainerName);
runtime_value_key!(SEAT_ID, "openanalytics.eu/sp-seat-id", "SHINYPROXY_SEAT_ID",
    label: false, annotation: true, env: false, api: false, required: false, container: false, kind: ValueKind::Str);

/// All runtime value keys of the engine.
pub static ENGINE_KEYS: &[&RuntimeValueKey] = &[
    &PROXY_ID,
    &PROXY_SPEC_ID,
    &INSTANCE_ID,
    &REALM_ID,
    &USER_ID,
    &USER_GROUPS,
    &DISPLAY_NAME,
    &CREATED_TIMESTAMP,
    &PROXIED_APP,
    &PUBLIC_PATH,
    &TARGET_ID,
    &HEARTBEAT_TIMEOUT,
    &MAX_LIFETIME,
    &CACHE_HEADERS_MODE,
    &HTTP_HEADERS,
    &PARAMETER_NAMES,
    &PARAMETER_VALUES,
    &CONTAINER_INDEX,
    &CONTAINER_IMAGE,
    &PORT_MAPPINGS,
    &BACKEND_CONTAINER_NAME,
    &SEAT_ID,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique() {
        let registry = RuntimeValueRegistry::engine();
        let mut labels = std::collections::HashSet::new();
        let mut env_vars = std::collections::HashSet::new();
        for key in registry.keys() {
            assert!(labels.insert(key.label), "duplicate label {}", key.label);
            assert!(
                env_vars.insert(key.env_var),
                "duplicate env var {}",
                key.env_var
            );
        }
    }

    #[test]
    fn only_the_documented_values_reach_the_container_environment() {
        let registry = RuntimeValueRegistry::engine();
        let mut env: Vec<&str> = registry
            .keys()
            .iter()
            .filter(|key| key.include_as_env)
            .map(|key| key.env_var)
            .collect();
        env.sort();
        // Matches the Java flags: only these engine values are injected into containers.
        assert_eq!(
            env,
            [
                "SHINYPROXY_PUBLIC_PATH",
                "SHINYPROXY_REALM_ID",
                "SHINYPROXY_USERGROUPS",
                "SHINYPROXY_USERNAME",
            ]
        );
    }

    #[test]
    fn security_sensitive_values_are_not_exposed_in_the_api() {
        for key in [
            &CONTAINER_IMAGE,
            &PORT_MAPPINGS,
            &BACKEND_CONTAINER_NAME,
            &HTTP_HEADERS,
            &PARAMETER_VALUES,
        ] {
            assert!(!key.include_in_api, "{} must not be exposed", key.env_var);
        }
        assert!(
            PARAMETER_NAMES.include_in_api,
            "parameter names may be exposed"
        );
    }

    #[test]
    fn serializes_and_parses_values() {
        let value = RuntimeValue::integer(&MAX_LIFETIME, -1);
        assert_eq!(value.to_value_string(), "-1");
        assert_eq!(
            RuntimeValue::parse(&MAX_LIFETIME, "-1").unwrap().data,
            RuntimeValueData::Int(-1)
        );

        let value = RuntimeValue::json(&HTTP_HEADERS, serde_json::json!({"X-SP-UserId": "jack"}));
        assert_eq!(value.to_value_string(), "{\"X-SP-UserId\":\"jack\"}");
        assert_eq!(
            RuntimeValue::parse(&HTTP_HEADERS, "{\"X-SP-UserId\":\"jack\"}")
                .unwrap()
                .data
                .to_json(),
            serde_json::json!({"X-SP-UserId": "jack"})
        );

        let value = RuntimeValue::parse(&BACKEND_CONTAINER_NAME, "my-namespace/pod-1").unwrap();
        assert_eq!(value.to_value_string(), "my-namespace/pod-1");
        let parsed: BackendContainerName = value.data.parse_json().expect("parses");
        assert_eq!(parsed.namespace, "my-namespace");
        assert_eq!(parsed.name, "pod-1");

        let value = RuntimeValue::parse(&BACKEND_CONTAINER_NAME, "container-id").unwrap();
        let parsed: BackendContainerName = value.data.parse_json().expect("parses");
        assert_eq!(parsed.namespace, "default");
        assert_eq!(parsed.name, "container-id");

        assert!(RuntimeValue::parse(&HEARTBEAT_TIMEOUT, "abc").is_err());
    }

    #[test]
    fn collection_respects_override_semantics() {
        let mut values = RuntimeValues::new();
        values.add(RuntimeValue::string(&DISPLAY_NAME, "first"), false);
        values.add(RuntimeValue::string(&DISPLAY_NAME, "second"), false);
        assert_eq!(values.value_string(&DISPLAY_NAME).as_deref(), Some("first"));
        values.add(RuntimeValue::string(&DISPLAY_NAME, "third"), true);
        assert_eq!(values.value_string(&DISPLAY_NAME).as_deref(), Some("third"));
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn projects_values_for_api_labels_and_environment() {
        let mut values = RuntimeValues::new();
        values.add(RuntimeValue::string(&USER_ID, "jack"), false);
        values.add(RuntimeValue::string(&DISPLAY_NAME, "Hello"), false);
        values.add(RuntimeValue::integer(&MAX_LIFETIME, 120), false);
        values.add(
            RuntimeValue::string(&CONTAINER_IMAGE, "secret/image"),
            false,
        );
        values.add(RuntimeValue::string(&INSTANCE_ID, "abc123"), false);

        let api = values.api_json();
        assert_eq!(
            api.get("SHINYPROXY_DISPLAY_NAME"),
            Some(&serde_json::json!("Hello"))
        );
        assert_eq!(
            api.get("SHINYPROXY_MAX_LIFETIME"),
            Some(&serde_json::json!(120))
        );
        assert_eq!(
            api.get("SHINYPROXY_INSTANCE"),
            Some(&serde_json::json!("abc123"))
        );
        assert!(!api.contains_key("SHINYPROXY_CONTAINER_IMAGE"));
        assert!(!api.contains_key("SHINYPROXY_USERNAME"));

        let environment = values.environment();
        assert_eq!(
            environment.get("SHINYPROXY_USERNAME").map(String::as_str),
            Some("jack")
        );
        assert!(!environment.contains_key("SHINYPROXY_INSTANCE"));

        let labels = values.labels();
        assert_eq!(
            labels
                .get("openanalytics.eu/sp-instance")
                .map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            labels
                .get("openanalytics.eu/sp-display-name")
                .map(String::as_str),
            Some("Hello")
        );
        assert!(!labels.contains_key("openanalytics.eu/sp-container-image"));

        let internal = values.internal_json();
        assert_eq!(
            internal
                .get("SHINYPROXY_CONTAINER_IMAGE")
                .map(String::as_str),
            Some("secret/image")
        );
        assert_eq!(internal.len(), 5);
    }

    #[test]
    fn parses_container_labels_and_rejects_incomplete_ones() {
        let registry = RuntimeValueRegistry::engine();
        let labels = [
            ("openanalytics.eu/sp-proxy-id", "abc"),
            ("openanalytics.eu/sp-spec-id", "01_hello"),
            ("openanalytics.eu/sp-instance", "instance"),
            ("openanalytics.eu/sp-user-id", "jack"),
            ("openanalytics.eu/sp-user-groups", "scientists"),
            ("openanalytics.eu/sp-display-name", "Hello"),
            (
                "openanalytics.eu/sp-proxy-created-timestamp",
                "1700000000000",
            ),
            ("openanalytics.eu/sp-proxied-app", "true"),
            ("openanalytics.eu/sp-public-path", "/app_proxy/abc/"),
            ("openanalytics.eu/sp-target-id", "abc"),
            ("openanalytics.eu/sp-heartbeat-timeout", "60000"),
            ("openanalytics.eu/sp-max-lifetime", "-1"),
            ("openanalytics.eu/sp-cache-headers-mode", "EnforceNoCache"),
            // container specific values are required as well (Java checks every label backed key)
            ("openanalytics.eu/sp-container-index", "0"),
            (
                "openanalytics.eu/sp-port-mappings",
                "{\"portMappings\":[{\"name\":\"default\",\"port\":3838,\"targetPath\":\"\"}]}",
            ),
            ("some.other/label", "ignored"),
        ];
        let values = registry.parse_labels(labels).expect("complete labels");
        assert_eq!(values.value_string(&PROXY_ID).as_deref(), Some("abc"));
        assert_eq!(
            values.get(&MAX_LIFETIME).unwrap().data,
            RuntimeValueData::Int(-1)
        );
        assert!(values.get_by_env_var("SHINYPROXY_INSTANCE").is_some());

        // a container without the required values is not a recoverable ShinyProxy container
        assert!(registry
            .parse_labels([("openanalytics.eu/sp-proxy-id", "abc")])
            .is_none());
    }
}
