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

//! Running proxies and their containers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::runtime_value::{
    RuntimeValue, RuntimeValueKey, RuntimeValueRegistry, RuntimeValues, CONTAINER_INDEX,
};

/// Lifecycle state of a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyStatus {
    /// Being created.
    New,
    /// Running and reachable.
    Up,
    /// Being stopped.
    Stopping,
    /// Being paused.
    Pausing,
    /// Paused (containers stopped but kept).
    Paused,
    /// Being resumed.
    Resuming,
    /// Stopped and removed.
    Stopped,
}

impl ProxyStatus {
    /// Whether requests can currently *not* be proxied to the app.
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            ProxyStatus::Stopping
                | ProxyStatus::Stopped
                | ProxyStatus::Pausing
                | ProxyStatus::Paused
        )
    }

    /// Name as used in the API and in log messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyStatus::New => "New",
            ProxyStatus::Up => "Up",
            ProxyStatus::Stopping => "Stopping",
            ProxyStatus::Pausing => "Pausing",
            ProxyStatus::Paused => "Paused",
            ProxyStatus::Resuming => "Resuming",
            ProxyStatus::Stopped => "Stopped",
        }
    }
}

impl std::fmt::Display for ProxyStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a proxy was stopped (used for events and usage statistics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyStopReason {
    /// Reason not known/relevant.
    Unknown,
    /// The user asked for it.
    ByUser,
    /// The app did not respond to heartbeats.
    Inactivity,
    /// The maximum lifetime was exceeded.
    ExceededMaxLifetime,
    /// The app crashed.
    Crashed,
    /// The user logged out.
    Logout,
    /// ShinyProxy is shutting down.
    Shutdown,
}

/// One container of a proxy.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Container {
    /// Index in the list of container specs of the proxy spec.
    pub index: i64,
    /// Backend specific id (Docker container id, Kubernetes pod name, ...).
    pub id: Option<String>,
    /// Runtime values of this container.
    pub runtime_values: RuntimeValues,
}

impl Container {
    /// A container with the given index.
    pub fn new(index: i64) -> Self {
        Container {
            index,
            ..Default::default()
        }
    }

    /// Adds a runtime value.
    pub fn add_runtime_value(&mut self, value: RuntimeValue, override_existing: bool) {
        self.runtime_values.add(value, override_existing);
    }

    /// JSON representation for the user facing API (`Views.UserApi`).
    pub fn api_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("index".into(), Value::from(self.index));
        object.insert(
            "id".into(),
            self.id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        object.insert(
            "runtimeValues".into(),
            Value::Object(self.runtime_values.api_json().into_iter().collect()),
        );
        Value::Object(object)
    }

    /// JSON representation used internally (Redis, app recovery): all runtime values as strings.
    pub fn internal_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("index".into(), Value::from(self.index));
        object.insert(
            "id".into(),
            self.id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        object.insert(
            "_runtimeValues".into(),
            Value::Object(
                self.runtime_values
                    .internal_json()
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
        Value::Object(object)
    }

    /// Rebuilds a container from its internal JSON representation.
    pub fn from_internal_json(
        registry: &RuntimeValueRegistry,
        value: &Value,
    ) -> Result<Self, ProxyDeserializeError> {
        let index = value
            .get("index")
            .and_then(Value::as_i64)
            .ok_or(ProxyDeserializeError::MissingField("index"))?;
        let id = value.get("id").and_then(Value::as_str).map(str::to_string);
        let runtime_values = parse_runtime_values(registry, value.get("_runtimeValues"))?;
        Ok(Container {
            index,
            id,
            runtime_values,
        })
    }
}

/// A running proxy: one app of one user.
#[derive(Debug, Clone, PartialEq)]
pub struct Proxy {
    /// Unique id (also the target id for non-shared proxies).
    pub id: String,
    /// Current status.
    pub status: ProxyStatus,
    /// When the app became reachable (epoch millis, 0 when not yet up).
    pub startup_timestamp: i64,
    /// When the proxy was created (epoch millis).
    pub created_timestamp: i64,
    /// Owner.
    pub user_id: Option<String>,
    /// Id of the spec this proxy was created from.
    pub spec_id: Option<String>,
    /// Display name of the app.
    pub display_name: Option<String>,
    /// Containers of this proxy.
    pub containers: Vec<Container>,
    /// Id used in the public path; equal to `id` unless the proxy is shared.
    pub target_id: Option<String>,
    /// Mapping name (`""` for the default mapping) to target URL.
    pub targets: BTreeMap<String, String>,
    /// Runtime values of the proxy.
    pub runtime_values: RuntimeValues,
}

impl Default for Proxy {
    fn default() -> Self {
        Proxy {
            id: String::new(),
            status: ProxyStatus::New,
            startup_timestamp: 0,
            created_timestamp: 0,
            user_id: None,
            spec_id: None,
            display_name: None,
            containers: Vec::new(),
            target_id: None,
            targets: BTreeMap::new(),
            runtime_values: RuntimeValues::new(),
        }
    }
}

impl Proxy {
    /// A new proxy with the given id and status.
    pub fn new(id: impl Into<String>, status: ProxyStatus) -> Self {
        let id = id.into();
        Proxy {
            target_id: Some(id.clone()),
            id,
            status,
            ..Default::default()
        }
    }

    /// Returns a copy with another status (`withStatus` in the Java model).
    pub fn with_status(&self, status: ProxyStatus) -> Self {
        Proxy {
            status,
            ..self.clone()
        }
    }

    /// The target id, falling back to the proxy id.
    pub fn target_id(&self) -> &str {
        self.target_id.as_deref().unwrap_or(&self.id)
    }

    /// The container with the given index.
    pub fn container(&self, index: i64) -> Option<&Container> {
        self.containers
            .iter()
            .find(|container| container.index == index)
    }

    /// Mutable access to the container with the given index, inserting it when missing.
    pub fn container_mut(&mut self, index: i64) -> &mut Container {
        if let Some(position) = self
            .containers
            .iter()
            .position(|container| container.index == index)
        {
            return &mut self.containers[position];
        }
        let mut container = Container::new(index);
        container.add_runtime_value(RuntimeValue::integer(&CONTAINER_INDEX, index), false);
        self.containers.push(container);
        self.containers
            .last_mut()
            .expect("container was just inserted")
    }

    /// Adds a runtime value to the proxy.
    pub fn add_runtime_value(&mut self, value: RuntimeValue, override_existing: bool) {
        self.runtime_values.add(value, override_existing);
    }

    /// String representation of a runtime value of this proxy (or of its first container).
    pub fn runtime_value(&self, key: &RuntimeValueKey) -> Option<String> {
        if key.container_specific {
            return self
                .containers
                .first()
                .and_then(|container| container.runtime_values.value_string(key));
        }
        self.runtime_values.value_string(key)
    }

    /// The default target URL, if the proxy has one.
    pub fn default_target(&self) -> Option<&str> {
        self.targets.get("").map(String::as_str)
    }

    /// JSON representation for the user facing API (`Views.UserApi`).
    pub fn api_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".into(), Value::String(self.id.clone()));
        object.insert("status".into(), Value::String(self.status.to_string()));
        object.insert(
            "startupTimestamp".into(),
            Value::from(self.startup_timestamp),
        );
        object.insert(
            "createdTimestamp".into(),
            Value::from(self.created_timestamp),
        );
        object.insert("userId".into(), optional_string(&self.user_id));
        object.insert("specId".into(), optional_string(&self.spec_id));
        object.insert("displayName".into(), optional_string(&self.display_name));
        object.insert(
            "containers".into(),
            Value::Array(self.containers.iter().map(Container::api_json).collect()),
        );
        object.insert(
            "runtimeValues".into(),
            Value::Object(self.runtime_values.api_json().into_iter().collect()),
        );
        Value::Object(object)
    }

    /// JSON representation used internally (Redis, app recovery).
    pub fn internal_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".into(), Value::String(self.id.clone()));
        object.insert("status".into(), Value::String(self.status.to_string()));
        object.insert(
            "startupTimestamp".into(),
            Value::from(self.startup_timestamp),
        );
        object.insert(
            "createdTimestamp".into(),
            Value::from(self.created_timestamp),
        );
        object.insert("userId".into(), optional_string(&self.user_id));
        object.insert("specId".into(), optional_string(&self.spec_id));
        object.insert("displayName".into(), optional_string(&self.display_name));
        object.insert(
            "containers".into(),
            Value::Array(
                self.containers
                    .iter()
                    .map(Container::internal_json)
                    .collect(),
            ),
        );
        object.insert("targetId".into(), optional_string(&self.target_id));
        object.insert(
            "targets".into(),
            Value::Object(
                self.targets
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
        object.insert(
            "_runtimeValues".into(),
            Value::Object(
                self.runtime_values
                    .internal_json()
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
        Value::Object(object)
    }

    /// Rebuilds a proxy from its internal JSON representation.
    pub fn from_internal_json(
        registry: &RuntimeValueRegistry,
        value: &Value,
    ) -> Result<Self, ProxyDeserializeError> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ProxyDeserializeError::MissingField("id"))?
            .to_string();
        let status: ProxyStatus = value
            .get("status")
            .and_then(Value::as_str)
            .and_then(|status| serde_json::from_value(Value::String(status.to_string())).ok())
            .ok_or(ProxyDeserializeError::MissingField("status"))?;
        let containers = match value.get("containers") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| Container::from_internal_json(registry, item))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        let targets = match value.get("targets") {
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        Ok(Proxy {
            id,
            status,
            startup_timestamp: value
                .get("startupTimestamp")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            created_timestamp: value
                .get("createdTimestamp")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            user_id: string_field(value, "userId"),
            spec_id: string_field(value, "specId"),
            display_name: string_field(value, "displayName"),
            containers,
            target_id: string_field(value, "targetId"),
            targets,
            runtime_values: parse_runtime_values(registry, value.get("_runtimeValues"))?,
        })
    }
}

/// Errors while rebuilding a proxy from its stored representation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProxyDeserializeError {
    #[error("stored proxy is missing the '{0}' field")]
    MissingField(&'static str),
    #[error("stored proxy contains an unknown runtime value '{0}'")]
    UnknownRuntimeValue(String),
    #[error("stored proxy contains an invalid runtime value: {0}")]
    InvalidRuntimeValue(#[from] super::runtime_value::RuntimeValueError),
}

fn optional_string(value: &Option<String>) -> Value {
    value.clone().map(Value::String).unwrap_or(Value::Null)
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn parse_runtime_values(
    registry: &RuntimeValueRegistry,
    value: Option<&Value>,
) -> Result<RuntimeValues, ProxyDeserializeError> {
    let mut values = RuntimeValues::new();
    if let Some(Value::Object(map)) = value {
        for (env_var, raw) in map {
            let Some(raw) = raw.as_str() else { continue };
            let key = registry
                .by_env_var(env_var)
                .ok_or_else(|| ProxyDeserializeError::UnknownRuntimeValue(env_var.clone()))?;
            values.add(RuntimeValue::parse(key, raw)?, true);
        }
    }
    Ok(values)
}

/// Records how long each step of the startup took (`ProxyStartupLog`).
#[derive(Debug, Clone, Default)]
pub struct ProxyStartupLog {
    steps: Vec<(String, i64)>,
}

impl ProxyStartupLog {
    /// Records the start of a step.
    pub fn step(&mut self, name: impl Into<String>) {
        self.steps.push((name.into(), now_millis()));
    }

    /// All recorded steps with their timestamps.
    pub fn steps(&self) -> &[(String, i64)] {
        &self.steps
    }
}

/// Current time in epoch milliseconds.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::runtime_value::{
        CONTAINER_IMAGE, CREATED_TIMESTAMP, DISPLAY_NAME, HEARTBEAT_TIMEOUT, INSTANCE_ID,
        MAX_LIFETIME, PORT_MAPPINGS, PROXY_ID, PROXY_SPEC_ID, PUBLIC_PATH, USER_GROUPS, USER_ID,
    };
    use serde_json::json;

    fn example_proxy() -> Proxy {
        let mut proxy = Proxy::new("5f39a7cf-c9ff-4a85-9313-d561ec79cca9", ProxyStatus::Up);
        proxy.startup_timestamp = 1234;
        proxy.created_timestamp = 1234;
        proxy.user_id = Some("jack".into());
        proxy.spec_id = Some("01_hello".into());
        proxy.display_name = Some("01_hello".into());
        proxy
            .targets
            .insert(String::new(), "http://localhost:20000".into());
        proxy.add_runtime_value(RuntimeValue::string(&DISPLAY_NAME, "01_hello"), false);
        proxy.add_runtime_value(RuntimeValue::integer(&MAX_LIFETIME, -1), false);
        proxy.add_runtime_value(RuntimeValue::string(&CREATED_TIMESTAMP, "1234"), false);
        proxy.add_runtime_value(
            RuntimeValue::string(&INSTANCE_ID, "03bc19d7d1970f737815c2d27ece37496ddee6f0"),
            false,
        );
        proxy.add_runtime_value(RuntimeValue::integer(&HEARTBEAT_TIMEOUT, -1), false);
        proxy.add_runtime_value(
            RuntimeValue::string(
                &PUBLIC_PATH,
                "/app_proxy/5f39a7cf-c9ff-4a85-9313-d561ec79cca9/",
            ),
            false,
        );
        proxy.add_runtime_value(RuntimeValue::string(&USER_ID, "jack"), false);
        proxy.add_runtime_value(RuntimeValue::string(&USER_GROUPS, "scientists"), false);
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_ID, proxy.id.clone()), false);
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_SPEC_ID, "01_hello"), false);

        let container = proxy.container_mut(0);
        container.id =
            Some("96a9e43437e356a8bbd6abb5bd4aa9f1436db49d95b3de8abcf03bccb15e2254".into());
        container.add_runtime_value(
            RuntimeValue::string(&CONTAINER_IMAGE, "openanalytics/shinyproxy-demo"),
            false,
        );
        container.add_runtime_value(
            RuntimeValue::json(
                &PORT_MAPPINGS,
                json!({"portMappings": [{"name": "default", "port": 3838, "targetPath": ""}]}),
            ),
            false,
        );
        proxy
    }

    /// The expected document is taken from the Swagger example of the Java `ProxyStatusController`.
    #[test]
    fn api_json_matches_the_java_user_api_view() {
        let proxy = example_proxy();
        assert_eq!(
            proxy.api_json(),
            json!({
                "id": "5f39a7cf-c9ff-4a85-9313-d561ec79cca9",
                "status": "Up",
                "startupTimestamp": 1234,
                "createdTimestamp": 1234,
                "userId": "jack",
                "specId": "01_hello",
                "displayName": "01_hello",
                "containers": [{
                    "index": 0,
                    "id": "96a9e43437e356a8bbd6abb5bd4aa9f1436db49d95b3de8abcf03bccb15e2254",
                    "runtimeValues": {"SHINYPROXY_CONTAINER_INDEX": 0}
                }],
                "runtimeValues": {
                    "SHINYPROXY_DISPLAY_NAME": "01_hello",
                    "SHINYPROXY_MAX_LIFETIME": -1,
                    "SHINYPROXY_CREATED_TIMESTAMP": "1234",
                    "SHINYPROXY_INSTANCE": "03bc19d7d1970f737815c2d27ece37496ddee6f0",
                    "SHINYPROXY_HEARTBEAT_TIMEOUT": -1,
                    "SHINYPROXY_PUBLIC_PATH": "/app_proxy/5f39a7cf-c9ff-4a85-9313-d561ec79cca9/"
                }
            })
        );
    }

    #[test]
    fn internal_json_round_trip() {
        let registry = RuntimeValueRegistry::engine();
        let proxy = example_proxy();
        let json = proxy.internal_json();

        assert_eq!(
            json["targetId"],
            json!("5f39a7cf-c9ff-4a85-9313-d561ec79cca9")
        );
        assert_eq!(json["targets"][""], json!("http://localhost:20000"));
        assert_eq!(json["_runtimeValues"]["SHINYPROXY_USERNAME"], json!("jack"));
        assert_eq!(
            json["containers"][0]["_runtimeValues"]["SHINYPROXY_CONTAINER_IMAGE"],
            json!("openanalytics/shinyproxy-demo")
        );

        let restored = Proxy::from_internal_json(&registry, &json).expect("round trip");
        assert_eq!(restored, proxy);
    }

    #[test]
    fn stopped_statuses_are_unavailable() {
        assert!(ProxyStatus::Stopped.is_unavailable());
        assert!(ProxyStatus::Stopping.is_unavailable());
        assert!(ProxyStatus::Paused.is_unavailable());
        assert!(ProxyStatus::Pausing.is_unavailable());
        assert!(!ProxyStatus::Up.is_unavailable());
        assert!(!ProxyStatus::New.is_unavailable());
        assert!(!ProxyStatus::Resuming.is_unavailable());
    }

    #[test]
    fn container_index_runtime_value_is_added_automatically() {
        let mut proxy = Proxy::new("id", ProxyStatus::New);
        let container = proxy.container_mut(0);
        assert_eq!(
            container
                .runtime_values
                .value_string(&CONTAINER_INDEX)
                .as_deref(),
            Some("0")
        );
        assert_eq!(proxy.containers.len(), 1);
        proxy.container_mut(0).id = Some("abc".into());
        assert_eq!(proxy.containers.len(), 1);
        assert_eq!(
            proxy.container(0).and_then(|c| c.id.clone()).as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn rejects_unknown_runtime_values_when_restoring() {
        let registry = RuntimeValueRegistry::engine();
        let json = json!({
            "id": "abc",
            "status": "Up",
            "_runtimeValues": {"SHINYPROXY_APP_INSTANCE": "default"}
        });
        let error = Proxy::from_internal_json(&registry, &json).unwrap_err();
        assert_eq!(
            error,
            ProxyDeserializeError::UnknownRuntimeValue("SHINYPROXY_APP_INSTANCE".to_string())
        );
    }
}
