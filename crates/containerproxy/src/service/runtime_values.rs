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

//! Which runtime values a proxy gets, and when (`RuntimeValueService`).
//!
//! Runtime values are added in three steps, because some of them may be used *inside* expressions while
//! others depend on the result of those expressions:
//!
//! 1. before the expressions of the app definition are resolved (ids, user, timestamps);
//! 2. after the first resolution round (heartbeat timeout and max lifetime, which may be expressions);
//! 3. after the final resolution round (the HTTP headers sent to the app, which may refer to runtime
//!    values).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backend::target::compute_target_path;
use crate::config::Settings;
use crate::model::proxy::{Container, Proxy};
use crate::model::runtime_value::{
    RuntimeValue, CACHE_HEADERS_MODE, CONTAINER_IMAGE, CONTAINER_INDEX, CREATED_TIMESTAMP,
    DISPLAY_NAME, HEARTBEAT_TIMEOUT, HTTP_HEADERS, INSTANCE_ID, MAX_LIFETIME, PORT_MAPPINGS,
    PROXIED_APP, PROXY_ID, PROXY_SPEC_ID, REALM_ID, USER_GROUPS, USER_ID,
};
use crate::model::spec::{CacheHeadersMode, ContainerSpec, ProxySpec};
use crate::service::identifier::Identifiers;
use crate::spec::expression::UserContext;

/// The port mappings of a container, as stored in the `SHINYPROXY_PORT_MAPPINGS` runtime value.
///
/// The Java class carries a `@JsonValue` list, so the value is a bare JSON array (not an object with a
/// `portMappings` field). Containers created by either implementation are therefore readable by both, which
/// app recovery depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortMappings {
    /// One entry per configured port mapping.
    pub port_mappings: Vec<PortMappingEntry>,
}

impl Serialize for PortMappings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.port_mappings.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PortMappings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // a bare array is what both implementations write; the object form is accepted as well, because
        // earlier builds of this implementation wrote it
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            List(Vec<PortMappingEntry>),
            Object {
                #[serde(rename = "portMappings", default)]
                port_mappings: Vec<PortMappingEntry>,
            },
        }
        Ok(match Either::deserialize(deserializer)? {
            Either::List(port_mappings) => PortMappings { port_mappings },
            Either::Object { port_mappings } => PortMappings { port_mappings },
        })
    }
}

/// One port mapping of a container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortMappingEntry {
    /// Name of the mapping (`default` for the app itself).
    pub name: String,
    /// Port inside the container.
    pub port: i64,
    /// Normalised path inside the container.
    pub target_path: String,
}

/// Adds the runtime values of a proxy.
#[derive(Debug, Clone)]
pub struct RuntimeValueService {
    default_heartbeat_timeout_ms: i64,
    default_max_lifetime_minutes: i64,
    default_cache_headers_mode: CacheHeadersMode,
    instance_id: String,
    realm_id: Option<String>,
}

impl RuntimeValueService {
    /// Creates the service from the configuration.
    pub fn new(settings: &Settings, identifiers: &Identifiers) -> Self {
        let default_cache_headers_mode = settings
            .proxy
            .default_cache_headers_mode
            .as_deref()
            .and_then(parse_cache_headers_mode)
            .unwrap_or_default();

        RuntimeValueService {
            default_heartbeat_timeout_ms: settings.proxy.heartbeat_timeout_ms(),
            default_max_lifetime_minutes: settings
                .proxy
                .default_proxy_max_lifetime
                .map(|value| value.0)
                .unwrap_or(-1),
            default_cache_headers_mode,
            instance_id: identifiers.instance_id.clone(),
            realm_id: identifiers.realm_id.clone(),
        }
    }

    /// Step 1: values that may be used inside expressions.
    pub fn add_before_expressions(
        &self,
        proxy: &mut Proxy,
        spec: &ProxySpec,
        user: Option<&UserContext>,
    ) {
        proxy.add_runtime_value(RuntimeValue::string(&PROXIED_APP, "true"), false);
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_ID, proxy.id.clone()), false);
        proxy.add_runtime_value(
            RuntimeValue::string(&INSTANCE_ID, self.instance_id.clone()),
            false,
        );
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_SPEC_ID, spec.id.clone()), false);
        proxy.add_runtime_value(
            RuntimeValue::string(&DISPLAY_NAME, spec.display_name_or_id()),
            true,
        );
        if let Some(realm_id) = &self.realm_id {
            proxy.add_runtime_value(RuntimeValue::string(&REALM_ID, realm_id.clone()), false);
        }
        proxy.add_runtime_value(
            RuntimeValue::string(&USER_ID, proxy.user_id.clone().unwrap_or_default()),
            false,
        );
        let groups = user.map(|user| user.groups.join(",")).unwrap_or_default();
        proxy.add_runtime_value(RuntimeValue::string(&USER_GROUPS, groups), true);
        proxy.add_runtime_value(
            RuntimeValue::string(&CREATED_TIMESTAMP, proxy.created_timestamp.to_string()),
            false,
        );

        let mode = spec
            .cache_headers_mode
            .unwrap_or(self.default_cache_headers_mode);
        proxy.add_runtime_value(
            RuntimeValue::string(&CACHE_HEADERS_MODE, cache_headers_mode_name(mode)),
            true,
        );
    }

    /// Step 2: values that are the result of the first resolution round.
    pub fn add_after_expressions(&self, proxy: &mut Proxy, spec: &ProxySpec) {
        let heartbeat_timeout = spec
            .heartbeat_timeout
            .value()
            .copied()
            .unwrap_or(self.default_heartbeat_timeout_ms);
        proxy.add_runtime_value(
            RuntimeValue::integer(&HEARTBEAT_TIMEOUT, heartbeat_timeout),
            true,
        );

        let max_lifetime = spec
            .max_lifetime
            .value()
            .copied()
            .unwrap_or(self.default_max_lifetime_minutes);
        proxy.add_runtime_value(RuntimeValue::integer(&MAX_LIFETIME, max_lifetime), true);
    }

    /// Step 3: the HTTP headers that are sent to the app.
    pub fn add_after_final_expressions(&self, proxy: &mut Proxy, spec: &ProxySpec) {
        let mut headers: BTreeMap<String, String> =
            spec.http_headers.value().cloned().unwrap_or_default();

        if spec.add_default_http_headers.unwrap_or(true) {
            headers.insert(
                "X-SP-UserId".to_string(),
                proxy.runtime_value(&USER_ID).unwrap_or_default(),
            );
            headers.insert(
                "X-SP-UserGroups".to_string(),
                proxy.runtime_value(&USER_GROUPS).unwrap_or_default(),
            );
        }

        proxy.add_runtime_value(RuntimeValue::json(&HTTP_HEADERS, headers), true);
    }

    /// The runtime values of a container (index, image and port mappings).
    pub fn add_container_values(&self, container: &mut Container, spec: &ContainerSpec) {
        container.add_runtime_value(
            RuntimeValue::integer(&CONTAINER_INDEX, container.index),
            false,
        );
        container.add_runtime_value(
            RuntimeValue::string(&CONTAINER_IMAGE, spec.image.as_str().unwrap_or_default()),
            false,
        );

        let port_mappings = PortMappings {
            port_mappings: spec
                .port_mapping
                .iter()
                .map(|mapping| PortMappingEntry {
                    name: mapping.name.clone(),
                    port: mapping.port.unwrap_or_default(),
                    target_path: compute_target_path(mapping.target_path.as_str()),
                })
                .collect(),
        };
        container.add_runtime_value(RuntimeValue::json(&PORT_MAPPINGS, port_mappings), false);
    }
}

/// Parses the value of `cache-headers-mode`.
pub fn parse_cache_headers_mode(value: &str) -> Option<CacheHeadersMode> {
    match value.to_ascii_lowercase().as_str() {
        "enforcenocache" => Some(CacheHeadersMode::EnforceNoCache),
        "passthrough" => Some(CacheHeadersMode::Passthrough),
        "enforcecacheassets" => Some(CacheHeadersMode::EnforceCacheAssets),
        _ => None,
    }
}

/// Name of a cache headers mode as used in runtime values and the API.
pub fn cache_headers_mode_name(mode: CacheHeadersMode) -> &'static str {
    match mode {
        CacheHeadersMode::EnforceNoCache => "EnforceNoCache",
        CacheHeadersMode::Passthrough => "Passthrough",
        CacheHeadersMode::EnforceCacheAssets => "EnforceCacheAssets",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load, LoadOptions, Schema};
    use crate::model::proxy::ProxyStatus;
    use crate::model::spec::PortMapping;
    use crate::model::spel_field::{SpelLong, SpelString, SpelStringMap};

    fn service(yaml: &str) -> RuntimeValueService {
        let settings: Settings = serde_yaml_ng::from_str(yaml).expect("settings");
        let directory = tempfile::tempdir().expect("temp dir");
        let options = LoadOptions {
            working_dir: Some(directory.path().to_path_buf()),
            ..LoadOptions::default()
        };
        let raw = load(&Schema::engine(), &options).expect("config");
        let mut identifiers = Identifiers::from_config(&raw, None);
        identifiers.instance_id = "instance-1".to_string();
        identifiers.realm_id = settings.proxy.realm_id.clone();
        RuntimeValueService::new(&settings, &identifiers)
    }

    fn proxy() -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::New);
        proxy.user_id = Some("jack".into());
        proxy.spec_id = Some("01_hello".into());
        proxy.created_timestamp = 1700000000000;
        proxy
    }

    fn user() -> UserContext {
        UserContext::new("jack", vec!["SCIENTISTS".into(), "ADMINS".into()])
    }

    #[test]
    fn adds_the_values_of_the_first_step() {
        let service = service("proxy:\n  realm-id: prod\n");
        let mut proxy = proxy();
        let mut spec = ProxySpec::new("01_hello");
        spec.display_name = Some("Hello Application".into());

        service.add_before_expressions(&mut proxy, &spec, Some(&user()));

        assert_eq!(proxy.runtime_value(&PROXIED_APP).as_deref(), Some("true"));
        assert_eq!(proxy.runtime_value(&PROXY_ID).as_deref(), Some("proxy-1"));
        assert_eq!(
            proxy.runtime_value(&INSTANCE_ID).as_deref(),
            Some("instance-1")
        );
        assert_eq!(
            proxy.runtime_value(&PROXY_SPEC_ID).as_deref(),
            Some("01_hello")
        );
        assert_eq!(
            proxy.runtime_value(&DISPLAY_NAME).as_deref(),
            Some("Hello Application")
        );
        assert_eq!(proxy.runtime_value(&REALM_ID).as_deref(), Some("prod"));
        assert_eq!(proxy.runtime_value(&USER_ID).as_deref(), Some("jack"));
        assert_eq!(
            proxy.runtime_value(&USER_GROUPS).as_deref(),
            Some("SCIENTISTS,ADMINS")
        );
        assert_eq!(
            proxy.runtime_value(&CREATED_TIMESTAMP).as_deref(),
            Some("1700000000000")
        );
        assert_eq!(
            proxy.runtime_value(&CACHE_HEADERS_MODE).as_deref(),
            Some("EnforceNoCache")
        );
    }

    #[test]
    fn display_name_falls_back_to_the_id_and_can_be_overridden() {
        let service = service("proxy: {}\n");
        let mut proxy = proxy();
        let spec = ProxySpec::new("01_hello");
        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        assert_eq!(
            proxy.runtime_value(&DISPLAY_NAME).as_deref(),
            Some("01_hello")
        );

        // the display name is added with override=true, so a second call updates it
        let mut spec = ProxySpec::new("01_hello");
        spec.display_name = Some("Renamed".into());
        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        assert_eq!(
            proxy.runtime_value(&DISPLAY_NAME).as_deref(),
            Some("Renamed")
        );
    }

    #[test]
    fn uses_the_configured_defaults_and_spec_values() {
        let service = service(
            "proxy:\n  heartbeat-timeout: 90000\n  default-proxy-max-lifetime: 120\n  default-cache-headers-mode: Passthrough\n",
        );
        let mut proxy = proxy();
        let spec = ProxySpec::new("01_hello");

        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        assert_eq!(
            proxy.runtime_value(&CACHE_HEADERS_MODE).as_deref(),
            Some("Passthrough")
        );

        service.add_after_expressions(&mut proxy, &spec);
        assert_eq!(
            proxy.runtime_value(&HEARTBEAT_TIMEOUT).as_deref(),
            Some("90000")
        );
        assert_eq!(proxy.runtime_value(&MAX_LIFETIME).as_deref(), Some("120"));

        // values of the app definition win
        let mut spec = ProxySpec::new("01_hello");
        spec.heartbeat_timeout = SpelLong::resolved("30000".into(), 30000);
        spec.max_lifetime = SpelLong::resolved("60".into(), 60);
        spec.cache_headers_mode = Some(CacheHeadersMode::EnforceCacheAssets);
        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        service.add_after_expressions(&mut proxy, &spec);
        assert_eq!(
            proxy.runtime_value(&HEARTBEAT_TIMEOUT).as_deref(),
            Some("30000")
        );
        assert_eq!(proxy.runtime_value(&MAX_LIFETIME).as_deref(), Some("60"));
        assert_eq!(
            proxy.runtime_value(&CACHE_HEADERS_MODE).as_deref(),
            Some("EnforceCacheAssets")
        );
    }

    #[test]
    fn adds_the_default_http_headers() {
        let service = service("proxy: {}\n");
        let mut proxy = proxy();
        let mut spec = ProxySpec::new("01_hello");
        spec.http_headers = SpelStringMap::resolved(
            BTreeMap::from([("X-Custom".to_string(), "value".to_string())]),
            BTreeMap::from([("X-Custom".to_string(), "value".to_string())]),
        );

        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        service.add_after_final_expressions(&mut proxy, &spec);

        let headers: BTreeMap<String, String> = proxy
            .runtime_values
            .get(&HTTP_HEADERS)
            .expect("headers")
            .data
            .parse_json()
            .expect("parses");
        assert_eq!(headers.get("X-Custom").map(String::as_str), Some("value"));
        assert_eq!(headers.get("X-SP-UserId").map(String::as_str), Some("jack"));
        assert_eq!(
            headers.get("X-SP-UserGroups").map(String::as_str),
            Some("SCIENTISTS,ADMINS")
        );
    }

    #[test]
    fn default_http_headers_can_be_disabled() {
        let service = service("proxy: {}\n");
        let mut proxy = proxy();
        let mut spec = ProxySpec::new("01_hello");
        spec.add_default_http_headers = Some(false);

        service.add_before_expressions(&mut proxy, &spec, Some(&user()));
        service.add_after_final_expressions(&mut proxy, &spec);

        let headers: BTreeMap<String, String> = proxy
            .runtime_values
            .get(&HTTP_HEADERS)
            .expect("headers")
            .data
            .parse_json()
            .expect("parses");
        assert!(headers.is_empty(), "{headers:?}");
    }

    #[test]
    fn adds_container_values_with_normalised_port_mappings() {
        let service = service("proxy: {}\n");
        let mut container = Container::new(0);
        let spec = ContainerSpec {
            image: SpelString::resolved(
                "openanalytics/shinyproxy-demo".into(),
                "openanalytics/shinyproxy-demo".into(),
            ),
            port_mapping: vec![
                PortMapping {
                    name: "default".into(),
                    port: Some(3838),
                    target_path: SpelString::resolved("//app//".into(), "//app//".into()),
                },
                PortMapping {
                    name: "dashboard".into(),
                    port: Some(8080),
                    target_path: SpelString::empty(),
                },
            ],
            ..Default::default()
        };

        service.add_container_values(&mut container, &spec);

        assert_eq!(
            container
                .runtime_values
                .value_string(&CONTAINER_INDEX)
                .as_deref(),
            Some("0")
        );
        assert_eq!(
            container
                .runtime_values
                .value_string(&CONTAINER_IMAGE)
                .as_deref(),
            Some("openanalytics/shinyproxy-demo")
        );
        let mappings: PortMappings = container
            .runtime_values
            .get(&PORT_MAPPINGS)
            .expect("port mappings")
            .data
            .parse_json()
            .expect("parses");
        assert_eq!(mappings.port_mappings.len(), 2);
        assert_eq!(mappings.port_mappings[0].name, "default");
        assert_eq!(mappings.port_mappings[0].port, 3838);
        assert_eq!(mappings.port_mappings[0].target_path, "/app");
        assert_eq!(mappings.port_mappings[1].target_path, "");
    }

    #[test]
    fn parses_cache_headers_modes() {
        assert_eq!(
            parse_cache_headers_mode("EnforceNoCache"),
            Some(CacheHeadersMode::EnforceNoCache)
        );
        assert_eq!(
            parse_cache_headers_mode("passthrough"),
            Some(CacheHeadersMode::Passthrough)
        );
        assert_eq!(parse_cache_headers_mode("nonsense"), None);
    }
}

#[cfg(test)]
mod port_mapping_tests {
    use super::*;

    #[test]
    fn port_mappings_are_a_bare_json_array_like_java() {
        let mappings = PortMappings {
            port_mappings: vec![PortMappingEntry {
                name: "default".to_string(),
                port: 3838,
                target_path: String::new(),
            }],
        };
        assert_eq!(
            serde_json::to_string(&mappings).expect("json"),
            r#"[{"name":"default","port":3838,"targetPath":""}]"#
        );

        // and both shapes are read back
        let from_array: PortMappings =
            serde_json::from_str(r#"[{"name":"default","port":3838,"targetPath":"/x"}]"#)
                .expect("array");
        assert_eq!(from_array.port_mappings.len(), 1);
        assert_eq!(from_array.port_mappings[0].target_path, "/x");

        let from_object: PortMappings = serde_json::from_str(
            r#"{"portMappings":[{"name":"default","port":3838,"targetPath":""}]}"#,
        )
        .expect("object");
        assert_eq!(from_object.port_mappings.len(), 1);
    }
}
