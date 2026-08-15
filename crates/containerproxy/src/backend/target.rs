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

//! How port mappings become proxy targets.
//!
//! The default mapping (`default`) is reachable at the root of the app (`/app_proxy/{targetId}/`), every
//! other mapping becomes a sub-path (`/app_proxy/{targetId}/{mapping}/`). This mirrors
//! `DefaultTargetMappingStrategy` and `AbstractContainerBackend.computeTargetPath`.

/// Name of the mapping that is served at the root of the app.
pub const DEFAULT_MAPPING_KEY: &str = "default";

/// The path of a mapping inside the app URL (empty for the default mapping).
pub fn mapping_key_to_path(mapping_key: &str) -> String {
    if mapping_key.eq_ignore_ascii_case(DEFAULT_MAPPING_KEY) {
        String::new()
    } else {
        mapping_key.to_string()
    }
}

/// Normalises the `target-path` of a port mapping.
///
/// * consecutive slashes are collapsed (they happen easily with expressions);
/// * the path starts with a slash and does not end with one;
/// * an empty path (or a single slash) becomes the empty string.
pub fn compute_target_path(target_path: Option<&str>) -> String {
    let Some(path) = target_path else {
        return String::new();
    };
    if path.is_empty() {
        return String::new();
    }

    let mut normalised = String::with_capacity(path.len() + 1);
    let mut previous_was_slash = false;
    for character in path.chars() {
        if character == '/' {
            if previous_was_slash {
                continue;
            }
            previous_was_slash = true;
        } else {
            previous_was_slash = false;
        }
        normalised.push(character);
    }

    if !normalised.starts_with('/') {
        normalised.insert(0, '/');
    }
    while normalised.ends_with('/') {
        normalised.pop();
    }
    normalised
}

/// Builds the target URL of a port mapping.
pub fn target_url(protocol: &str, host: &str, port: u16, target_path: &str) -> String {
    format!("{protocol}://{host}:{port}{target_path}")
}

/// The targets of a container that already exists, from the port mappings stored on it.
///
/// Used by the backends that publish host ports (Docker, Swarm and the local backend): the mapping names
/// and paths come from the `SHINYPROXY_PORT_MAPPINGS` runtime value, and the host and the port from the
/// arguments.
pub fn targets_from_stored_mappings(
    container: &crate::model::proxy::Container,
    port_bindings: &std::collections::BTreeMap<u16, u16>,
    protocol: &str,
    host: &str,
    internal_networking: bool,
) -> std::collections::BTreeMap<String, String> {
    let mappings: crate::service::runtime_values::PortMappings = container
        .runtime_values
        .get(&crate::model::runtime_value::PORT_MAPPINGS)
        .and_then(|value| value.data.parse_json())
        .unwrap_or_default();

    let mut targets = std::collections::BTreeMap::new();
    for mapping in &mappings.port_mappings {
        let container_port = mapping.port;
        let (host, port) = if internal_networking {
            // inside a container network the short container id is a resolvable name
            (
                container
                    .id
                    .clone()
                    .unwrap_or_default()
                    .chars()
                    .take(12)
                    .collect::<String>(),
                container_port as u16,
            )
        } else {
            match port_bindings.get(&(container_port as u16)) {
                Some(host_port) => (host.to_string(), *host_port),
                None => continue,
            }
        };
        targets.insert(
            mapping_key_to_path(&mapping.name),
            target_url(
                protocol,
                &host,
                port,
                &compute_target_path(Some(mapping.target_path.as_str())),
            ),
        );
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mapping_is_served_at_the_root() {
        assert_eq!(mapping_key_to_path("default"), "");
        assert_eq!(mapping_key_to_path("DEFAULT"), "");
        assert_eq!(mapping_key_to_path("dashboard"), "dashboard");
    }

    #[test]
    fn normalises_target_paths_like_java() {
        assert_eq!(compute_target_path(None), "");
        assert_eq!(compute_target_path(Some("")), "");
        assert_eq!(compute_target_path(Some("/")), "");
        assert_eq!(compute_target_path(Some("app")), "/app");
        assert_eq!(compute_target_path(Some("/app")), "/app");
        assert_eq!(compute_target_path(Some("/app/")), "/app");
        assert_eq!(compute_target_path(Some("//app//sub//")), "/app/sub");
        assert_eq!(compute_target_path(Some("app///")), "/app");
    }

    #[test]
    fn builds_target_urls() {
        assert_eq!(
            target_url("http", "127.0.0.1", 20000, ""),
            "http://127.0.0.1:20000"
        );
        assert_eq!(
            target_url("https", "container-host", 3838, "/app"),
            "https://container-host:3838/app"
        );
    }
}
