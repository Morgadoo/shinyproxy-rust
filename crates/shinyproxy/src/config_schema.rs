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

use containerproxy::config::schema::{list, map, scalar, KeyDef, Schema, Support::Supported};

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
        scalar("proxy.template-groups[].id", Supported),
        map("proxy.template-groups[].properties", Supported),
        // --- app definition: identity and presentation ---
        scalar("proxy.specs[].id", Supported),
        scalar("proxy.specs[].display-name", Supported),
        scalar("proxy.specs[].description", Supported),
        scalar("proxy.specs[].logo-url", Supported),
        scalar("proxy.specs[].logo-width", Supported),
        scalar("proxy.specs[].logo-height", Supported),
        scalar("proxy.specs[].logo-style", Supported),
        scalar("proxy.specs[].logo-classes", Supported),
        scalar("proxy.specs[].favicon-path", Supported),
        // --- app definition: container ---
        scalar("proxy.specs[].container-image", Supported),
        list("proxy.specs[].container-cmd", Supported),
        map("proxy.specs[].container-env", Supported),
        scalar("proxy.specs[].container-env-file", Supported),
        scalar("proxy.specs[].container-network", Supported),
        list("proxy.specs[].container-network-connections", Supported),
        list("proxy.specs[].container-dns", Supported),
        list("proxy.specs[].container-volumes", Supported),
        scalar("proxy.specs[].container-memory-request", Supported),
        scalar("proxy.specs[].container-memory-limit", Supported),
        scalar("proxy.specs[].container-cpu-request", Supported),
        scalar("proxy.specs[].container-cpu-limit", Supported),
        scalar("proxy.specs[].container-privileged", Supported),
        scalar("proxy.specs[].container-resource-name", Supported),
        map("proxy.specs[].labels", Supported),
        scalar("proxy.specs[].port", Supported),
        scalar("proxy.specs[].target-path", Supported),
        scalar("proxy.specs[].additional-port-mappings[].name", Supported),
        scalar("proxy.specs[].additional-port-mappings[].port", Supported),
        scalar("proxy.specs[].additional-port-mappings[].target-path", Supported),
        // --- app definition: docker specific ---
        scalar("proxy.specs[].docker-registry-domain", Supported),
        scalar("proxy.specs[].docker-registry-username", Supported),
        scalar("proxy.specs[].docker-registry-password", Supported),
        scalar("proxy.specs[].docker-runtime", Supported),
        scalar("proxy.specs[].docker-user", Supported),
        scalar("proxy.specs[].docker-ipc", Supported),
        list("proxy.specs[].docker-group-add", Supported),
        scalar("proxy.specs[].docker-swarm-secrets[].name", Supported),
        scalar("proxy.specs[].docker-swarm-secrets[].target", Supported),
        scalar("proxy.specs[].docker-swarm-secrets[].uid", Supported),
        scalar("proxy.specs[].docker-swarm-secrets[].gid", Supported),
        scalar("proxy.specs[].docker-swarm-secrets[].mode", Supported),
        scalar("proxy.specs[].docker-device-requests[].driver", Supported),
        scalar("proxy.specs[].docker-device-requests[].count", Supported),
        list("proxy.specs[].docker-device-requests[].device-ids", Supported),
        list("proxy.specs[].docker-device-requests[].capabilities", Supported),
        map("proxy.specs[].docker-device-requests[].options", Supported),
        // --- app definition: access control ---
        list("proxy.specs[].access-groups", Supported),
        list("proxy.specs[].access-users", Supported),
        scalar("proxy.specs[].access-expression", Supported),
        scalar("proxy.specs[].access-strict-expression", Supported),
        // --- app definition: lifecycle ---
        scalar("proxy.specs[].max-lifetime", Supported),
        scalar("proxy.specs[].heartbeat-timeout", Supported),
        scalar("proxy.specs[].stop-on-logout", Supported),
        scalar("proxy.specs[].max-total-instances", Supported),
        scalar("proxy.specs[].max-instances", Supported),
        // --- app definition: http behaviour ---
        scalar("proxy.specs[].add-default-http-headers", Supported),
        map("proxy.specs[].http-headers", Supported),
        scalar("proxy.specs[].cache-headers-mode", Supported),
        scalar("proxy.specs[].websocket-reconnection-mode", Supported),
        scalar("proxy.specs[].shiny-force-full-reload", Supported),
        scalar("proxy.specs[].track-app-url", Supported),
        // --- app definition: ui ---
        scalar("proxy.specs[].hide-navbar-on-main-page-link", Supported),
        scalar("proxy.specs[].always-show-switch-instance", Supported),
        scalar("proxy.specs[].template-group", Supported),
        map("proxy.specs[].template-properties", Supported),
        scalar("proxy.specs[].support-mail-to-address", Supported),
        scalar("proxy.specs[].support-mail-subject", Supported),
        scalar("proxy.specs[].custom-app-details[].name", Supported),
        scalar("proxy.specs[].custom-app-details[].description", Supported),
        scalar("proxy.specs[].custom-app-details[].value", Supported),
        scalar("proxy.specs[].external-url", Supported),
        // --- app definition: parameters ---
        scalar("proxy.specs[].parameters.template", Supported),
        scalar("proxy.specs[].parameters.definitions[].id", Supported),
        scalar("proxy.specs[].parameters.definitions[].display-name", Supported),
        scalar("proxy.specs[].parameters.definitions[].description", Supported),
        scalar("proxy.specs[].parameters.definitions[].default-value", Supported),
        scalar("proxy.specs[].parameters.definitions[].value-names[].value", Supported),
        scalar("proxy.specs[].parameters.definitions[].value-names[].name", Supported),
        scalar("proxy.specs[].parameters.value-sets[].name", Supported),
        map("proxy.specs[].parameters.value-sets[].values", Supported),
        list("proxy.specs[].parameters.value-sets[].access-control.groups", Supported),
        list("proxy.specs[].parameters.value-sets[].access-control.users", Supported),
        scalar("proxy.specs[].parameters.value-sets[].access-control.expression", Supported),
        // --- app definition: container pre-initialization / sharing ---
        scalar("proxy.specs[].minimum-seats-available", Supported),
        scalar("proxy.specs[].allow-container-re-use", Supported),
        scalar("proxy.specs[].scale-down-delay", Supported),
        scalar("proxy.specs[].seats-per-container", Supported),
        // --- app definition: kubernetes specific ---
        scalar("proxy.specs[].kubernetes-pod-patches", Supported),
        list("proxy.specs[].kubernetes-additional-manifests", Supported),
        list("proxy.specs[].kubernetes-additional-persistent-manifests", Supported),
        scalar("proxy.specs[].kubernetes-authorized-pod-patches[].patches", Supported),
        list("proxy.specs[].kubernetes-authorized-pod-patches[].access-control.groups", Supported),
        list("proxy.specs[].kubernetes-authorized-pod-patches[].access-control.users", Supported),
        scalar("proxy.specs[].kubernetes-authorized-pod-patches[].access-control.expression", Supported),
        list("proxy.specs[].kubernetes-authorized-additional-manifests[].manifests", Supported),
        list("proxy.specs[].kubernetes-authorized-additional-manifests[].access-control.groups", Supported),
        list("proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].manifests", Supported),
        list("proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].access-control.groups", Supported),
        // --- app definition: ecs specific ---
        scalar("proxy.specs[].ecs-task-role", Supported),
        scalar("proxy.specs[].ecs-execution-role", Supported),
        scalar("proxy.specs[].ecs-cpu-architecture", Supported),
        scalar("proxy.specs[].ecs-operation-system-family", Supported),
        scalar("proxy.specs[].ecs-ephemeral-storage-size", Supported),
        list("proxy.specs[].ecs-bind-volumes", Supported),
        scalar("proxy.specs[].ecs-enable-execute-command", Supported),
        scalar("proxy.specs[].ecs-readonly-root-filesystem", Supported),
        scalar("proxy.specs[].ecs-repository-credentials-parameter", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].name", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].file-system-id", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].root-directory", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].transit-encryption", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].transit-encryption-port", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].access-point-id", Supported),
        scalar("proxy.specs[].ecs-efs-volumes[].enable-iam", Supported),
        scalar("proxy.specs[].ecs-managed-secrets[].name", Supported),
        scalar("proxy.specs[].ecs-managed-secrets[].value-from", Supported),
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
