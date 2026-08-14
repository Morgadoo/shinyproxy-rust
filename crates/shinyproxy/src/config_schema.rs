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

//! Configuration properties contributed by ShinyProxy: `proxy.specs[]` (the "ShinyProxy notation" for
//! app definitions, including the fields of all spec extensions) and `proxy.template-groups[]`.

use containerproxy::config::schema::{list, map, scalar, KeyDef, Schema, Support::Planned};

/// The full ShinyProxy schema: engine properties plus the ShinyProxy specific ones.
pub fn schema() -> Schema {
    Schema::engine().with_keys(spec_keys())
}

/// ShinyProxy specific properties.
pub fn spec_keys() -> &'static [KeyDef] {
    SPEC_KEYS
}

static SPEC_KEYS: &[KeyDef] = {
    &[
        // --- app template groups ---
        scalar("proxy.template-groups[].id", Planned("P9")),
        map("proxy.template-groups[].properties", Planned("P9")),
        // --- app definition: identity and presentation ---
        scalar("proxy.specs[].id", Planned("P2")),
        scalar("proxy.specs[].display-name", Planned("P2")),
        scalar("proxy.specs[].description", Planned("P2")),
        scalar("proxy.specs[].logo-url", Planned("P2")),
        scalar("proxy.specs[].logo-width", Planned("P2")),
        scalar("proxy.specs[].logo-height", Planned("P2")),
        scalar("proxy.specs[].logo-style", Planned("P2")),
        scalar("proxy.specs[].logo-classes", Planned("P2")),
        scalar("proxy.specs[].favicon-path", Planned("P2")),
        // --- app definition: container ---
        scalar("proxy.specs[].container-image", Planned("P2")),
        list("proxy.specs[].container-cmd", Planned("P2")),
        map("proxy.specs[].container-env", Planned("P2")),
        scalar("proxy.specs[].container-env-file", Planned("P2")),
        scalar("proxy.specs[].container-network", Planned("P2")),
        list("proxy.specs[].container-network-connections", Planned("P2")),
        list("proxy.specs[].container-dns", Planned("P2")),
        list("proxy.specs[].container-volumes", Planned("P2")),
        scalar("proxy.specs[].container-memory-request", Planned("P2")),
        scalar("proxy.specs[].container-memory-limit", Planned("P2")),
        scalar("proxy.specs[].container-cpu-request", Planned("P2")),
        scalar("proxy.specs[].container-cpu-limit", Planned("P2")),
        scalar("proxy.specs[].container-privileged", Planned("P2")),
        scalar("proxy.specs[].container-resource-name", Planned("P2")),
        map("proxy.specs[].labels", Planned("P2")),
        scalar("proxy.specs[].port", Planned("P2")),
        scalar("proxy.specs[].target-path", Planned("P2")),
        scalar("proxy.specs[].additional-port-mappings[].name", Planned("P2")),
        scalar("proxy.specs[].additional-port-mappings[].port", Planned("P2")),
        scalar("proxy.specs[].additional-port-mappings[].target-path", Planned("P2")),
        // --- app definition: docker specific ---
        scalar("proxy.specs[].docker-registry-domain", Planned("P8")),
        scalar("proxy.specs[].docker-registry-username", Planned("P8")),
        scalar("proxy.specs[].docker-registry-password", Planned("P8")),
        scalar("proxy.specs[].docker-runtime", Planned("P8")),
        scalar("proxy.specs[].docker-user", Planned("P8")),
        scalar("proxy.specs[].docker-ipc", Planned("P8")),
        list("proxy.specs[].docker-group-add", Planned("P8")),
        scalar("proxy.specs[].docker-swarm-secrets[].name", Planned("P8")),
        scalar("proxy.specs[].docker-swarm-secrets[].target", Planned("P8")),
        scalar("proxy.specs[].docker-swarm-secrets[].uid", Planned("P8")),
        scalar("proxy.specs[].docker-swarm-secrets[].gid", Planned("P8")),
        scalar("proxy.specs[].docker-swarm-secrets[].mode", Planned("P8")),
        scalar("proxy.specs[].docker-device-requests[].driver", Planned("P8")),
        scalar("proxy.specs[].docker-device-requests[].count", Planned("P8")),
        list("proxy.specs[].docker-device-requests[].device-ids", Planned("P8")),
        list("proxy.specs[].docker-device-requests[].capabilities", Planned("P8")),
        map("proxy.specs[].docker-device-requests[].options", Planned("P8")),
        // --- app definition: access control ---
        list("proxy.specs[].access-groups", Planned("P2")),
        list("proxy.specs[].access-users", Planned("P2")),
        scalar("proxy.specs[].access-expression", Planned("P4")),
        scalar("proxy.specs[].access-strict-expression", Planned("P4")),
        // --- app definition: lifecycle ---
        scalar("proxy.specs[].max-lifetime", Planned("P5")),
        scalar("proxy.specs[].heartbeat-timeout", Planned("P6")),
        scalar("proxy.specs[].stop-on-logout", Planned("P10")),
        scalar("proxy.specs[].max-total-instances", Planned("P5")),
        scalar("proxy.specs[].max-instances", Planned("P5")),
        // --- app definition: http behaviour ---
        scalar("proxy.specs[].add-default-http-headers", Planned("P6")),
        map("proxy.specs[].http-headers", Planned("P6")),
        scalar("proxy.specs[].cache-headers-mode", Planned("P6")),
        scalar("proxy.specs[].websocket-reconnection-mode", Planned("P6")),
        scalar("proxy.specs[].shiny-force-full-reload", Planned("P9")),
        scalar("proxy.specs[].track-app-url", Planned("P9")),
        // --- app definition: ui ---
        scalar("proxy.specs[].hide-navbar-on-main-page-link", Planned("P9")),
        scalar("proxy.specs[].always-show-switch-instance", Planned("P9")),
        scalar("proxy.specs[].template-group", Planned("P9")),
        map("proxy.specs[].template-properties", Planned("P9")),
        scalar("proxy.specs[].support-mail-to-address", Planned("P9")),
        scalar("proxy.specs[].support-mail-subject", Planned("P9")),
        scalar("proxy.specs[].custom-app-details[].name", Planned("P9")),
        scalar("proxy.specs[].custom-app-details[].description", Planned("P9")),
        scalar("proxy.specs[].custom-app-details[].value", Planned("P9")),
        scalar("proxy.specs[].external-url", Planned("P9")),
        // --- app definition: parameters ---
        scalar("proxy.specs[].parameters.template", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].id", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].display-name", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].description", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].default-value", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].value-names[].value", Planned("P9")),
        scalar("proxy.specs[].parameters.definitions[].value-names[].name", Planned("P9")),
        scalar("proxy.specs[].parameters.value-sets[].name", Planned("P9")),
        map("proxy.specs[].parameters.value-sets[].values", Planned("P9")),
        list("proxy.specs[].parameters.value-sets[].access-control.groups", Planned("P9")),
        list("proxy.specs[].parameters.value-sets[].access-control.users", Planned("P9")),
        scalar("proxy.specs[].parameters.value-sets[].access-control.expression", Planned("P9")),
        // --- app definition: container pre-initialization / sharing ---
        scalar("proxy.specs[].minimum-seats-available", Planned("P12")),
        scalar("proxy.specs[].allow-container-re-use", Planned("P12")),
        scalar("proxy.specs[].scale-down-delay", Planned("P12")),
        scalar("proxy.specs[].seats-per-container", Planned("P12")),
        // --- app definition: kubernetes specific ---
        scalar("proxy.specs[].kubernetes-pod-patches", Planned("P12")),
        list("proxy.specs[].kubernetes-additional-manifests", Planned("P12")),
        list("proxy.specs[].kubernetes-additional-persistent-manifests", Planned("P12")),
        scalar("proxy.specs[].kubernetes-authorized-pod-patches[].patches", Planned("P12")),
        list(
            "proxy.specs[].kubernetes-authorized-pod-patches[].access-control.groups",
            Planned("P12"),
        ),
        list(
            "proxy.specs[].kubernetes-authorized-pod-patches[].access-control.users",
            Planned("P12"),
        ),
        scalar(
            "proxy.specs[].kubernetes-authorized-pod-patches[].access-control.expression",
            Planned("P12"),
        ),
        list(
            "proxy.specs[].kubernetes-authorized-additional-manifests[].manifests",
            Planned("P12"),
        ),
        list(
            "proxy.specs[].kubernetes-authorized-additional-manifests[].access-control.groups",
            Planned("P12"),
        ),
        list(
            "proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].manifests",
            Planned("P12"),
        ),
        list(
            "proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].access-control.groups",
            Planned("P12"),
        ),
        // --- app definition: ecs specific ---
        scalar("proxy.specs[].ecs-task-role", Planned("P12")),
        scalar("proxy.specs[].ecs-execution-role", Planned("P12")),
        scalar("proxy.specs[].ecs-cpu-architecture", Planned("P12")),
        scalar("proxy.specs[].ecs-operation-system-family", Planned("P12")),
        scalar("proxy.specs[].ecs-ephemeral-storage-size", Planned("P12")),
        list("proxy.specs[].ecs-bind-volumes", Planned("P12")),
        scalar("proxy.specs[].ecs-enable-execute-command", Planned("P12")),
        scalar("proxy.specs[].ecs-readonly-root-filesystem", Planned("P12")),
        scalar("proxy.specs[].ecs-repository-credentials-parameter", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].name", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].file-system-id", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].root-directory", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].transit-encryption", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].transit-encryption-port", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].access-point-id", Planned("P12")),
        scalar("proxy.specs[].ecs-efs-volumes[].enable-iam", Planned("P12")),
        scalar("proxy.specs[].ecs-managed-secrets[].name", Planned("P12")),
        scalar("proxy.specs[].ecs-managed-secrets[].value-from", Planned("P12")),
    ]
};

/// Renders the schema as the documentation page `docs/CONFIGURATION.md`.
pub fn markdown() -> String {
    use containerproxy::config::schema::KeyKind;

    let schema = schema();
    let mut keys: Vec<&KeyDef> = schema.keys().iter().collect();
    keys.sort_by_key(|key| key.path);

    let mut out = String::new();
    out.push_str("# Configuration reference\n\n");
    out.push_str(
        "ShinyProxy (Rust) reads the same `application.yml` as the Java implementation \
         (version 3.2.4). This page lists every property that is understood, its shape and whether the \
         behaviour behind it is already implemented in this rewrite.\n\n\
         *This file is generated:* run\n\n```\ncargo run -q -p shinyproxy --example config-docs > docs/CONFIGURATION.md\n```\n\n\
         after changing `crates/containerproxy/src/config/schema.rs` or \
         `crates/shinyproxy/src/config_schema.rs`.\n\n",
    );
    out.push_str("## How configuration is resolved\n\n");
    out.push_str(
        "1. `--key=value` command line arguments\n\
         2. environment variables (`PROXY_PORT`, `PROXY_DOCKER_PORT_RANGE_START`, \
         `PROXY_ADMIN_GROUPS_0`, `PROXY_SPECS_0_CONTAINER_IMAGE`, ...)\n\
         3. profile specific files (`application-{profile}.yml`) next to the configuration file\n\
         4. the configuration file: `application.yml` in the working directory, \
         `--spring.config.location=<file|dir>` or `SPRING_CONFIG_LOCATION`\n\
         5. built-in defaults; when no configuration file exists at all, the built-in demo \
         configuration is used (`demo` profile)\n\n\
         Property names are matched leniently: `port-range-start`, `portRangeStart` and \
         `PORT_RANGE_START` are the same property. `${VAR}` and `${other.property:default}` \
         placeholders are resolved against the environment and the configuration itself; \
         placeholders that cannot be resolved are left untouched (so Thymeleaf snippets such as \
         `${parameterDefinitions}` keep working).\n\n",
    );
    out.push_str("## Properties\n\n");
    out.push_str("| Property | Shape | Status |\n| --- | --- | --- |\n");
    for key in keys {
        let shape = match key.kind {
            KeyKind::Scalar => "value",
            KeyKind::ScalarList => "list of values",
            KeyKind::Map => "map (free form keys)",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            key.path,
            shape,
            key.support.label()
        ));
    }
    out.push_str(
        "\n## Notes\n\n\
         * `proxy.ldap` accepts a single provider (`proxy.ldap.url`) or a list of providers \
         (`proxy.ldap[0].url`).\n\
         * `proxy.ecs.enable-cloudwatch` is accepted as an alias of `proxy.ecs.enable-cloud-watch`.\n\
         * `proxy.container-backend: local` is an addition of this implementation: it starts apps as \
         local processes and exists for testing only.\n\
         * Deviations from the Java implementation are tracked in [COMPATIBILITY.md](COMPATIBILITY.md).\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_contains_every_property() {
        let markdown = markdown();
        for key in schema().keys() {
            assert!(
                markdown.contains(&format!("| `{}` |", key.path)),
                "{} is missing from the generated documentation",
                key.path
            );
        }
    }

    #[test]
    fn knows_shinyproxy_specific_properties() {
        let schema = schema();
        assert!(schema.is_known("proxy.specs[0].container-image"));
        assert!(schema.is_known("proxy.specs[1].containerImage"));
        assert!(schema.is_known("proxy.specs[0].container-env.MY_VAR"));
        assert!(schema.is_known("proxy.specs[0].parameters.definitions[0].value-names[1].name"));
        assert!(schema.is_known("proxy.template-groups[0].properties.color"));
        assert!(schema.is_known("proxy.port"), "engine keys are still known");
        assert!(!schema.is_known("proxy.specs[0].not-a-field"));
    }

    #[test]
    fn spec_keys_have_unique_canonical_paths() {
        let schema = schema();
        let mut seen = std::collections::HashMap::new();
        for key in schema.keys() {
            let canonical = containerproxy::config::schema::canonical_path(key.path);
            if let Some(previous) = seen.insert(canonical.clone(), key.path) {
                assert_eq!(previous, key.path, "duplicate canonical path {canonical}");
            }
        }
    }
}
