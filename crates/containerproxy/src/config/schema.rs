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

//! The configuration schema: the set of property paths ShinyProxy understands.
//!
//! Spring resolves properties with a *pull* model: it asks every property source for
//! `proxy.docker.port-range-start`, and the environment source translates that name into
//! `PROXY_DOCKER_PORT_RANGE_START`. Because this implementation builds a configuration *tree* instead,
//! it needs to know which property paths exist in order to
//!
//! * place environment variables at the right position in the tree,
//! * rewrite relaxed property names (`portRangeStart`, `PORT_RANGE_START`) to their canonical spelling
//!   before the tree is deserialized into typed settings,
//! * report properties that ShinyProxy does not know about.
//!
//! Array elements are written as `[]`, e.g. `proxy.specs[].container-image`.
//!
//! The table doubles as the source of truth for `docs/CONFIGURATION.md`.

use std::collections::BTreeSet;

use super::tree::{canonical_name, parse_path, Segment};

/// Shape of a configuration property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// A single value (string, number, boolean, enum).
    Scalar,
    /// A list of scalars. Accepts a single value, a comma separated string, or a YAML list.
    ScalarList,
    /// A free form map: arbitrary keys below this path (`container-env`, `logging.level`, ...).
    /// Keys below a map are never rewritten or validated.
    Map,
}

/// One known configuration property.
#[derive(Debug, Clone, Copy)]
pub struct KeyDef {
    /// Dotted path, with `[]` marking array elements, e.g. `proxy.specs[].container-image`.
    pub path: &'static str,
    /// Shape of the value.
    pub kind: KeyKind,
    /// Phase in which support is (or will be) implemented; used by `docs/CONFIGURATION.md`.
    pub support: Support,
}

/// Implementation status of a property in this rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Understood and honoured.
    Supported,
    /// Accepted by the parser, behaviour lands in the given phase.
    Planned(&'static str),
    /// Deliberately not supported (documented in `docs/COMPATIBILITY.md`).
    Unsupported(&'static str),
}

impl Support {
    /// Short label used in the generated documentation.
    pub fn label(&self) -> String {
        match self {
            Support::Supported => "supported".to_string(),
            Support::Planned(phase) => format!("planned ({phase})"),
            Support::Unsupported(reason) => format!("unsupported: {reason}"),
        }
    }
}

/// Declares a scalar property.
pub const fn scalar(path: &'static str, support: Support) -> KeyDef {
    KeyDef {
        path,
        kind: KeyKind::Scalar,
        support,
    }
}

/// Declares a list-of-scalars property.
pub const fn list(path: &'static str, support: Support) -> KeyDef {
    KeyDef {
        path,
        kind: KeyKind::ScalarList,
        support,
    }
}

/// Declares a free form map property.
pub const fn map(path: &'static str, support: Support) -> KeyDef {
    KeyDef {
        path,
        kind: KeyKind::Map,
        support,
    }
}

use Support::{Planned, Supported, Unsupported};

/// Properties of the engine (the Java `containerproxy` library).
pub fn engine_keys() -> &'static [KeyDef] {
    ENGINE_KEYS
}

static ENGINE_KEYS: &[KeyDef] = {
    &[
        // --- server ---
        scalar("proxy.port", Supported),
        scalar("proxy.bind-address", Supported),
        scalar("proxy.same-site-cookie", Planned("P4")),
        scalar("server.servlet.context-path", Planned("P4")),
        scalar("server.secure-cookies", Planned("P4")),
        scalar("server.frame-options", Planned("P4")),
        scalar(
            "server.use-forward-headers",
            Unsupported("removed in ShinyProxy 3.x"),
        ),
        scalar(
            "server.undertow.max-http-post-size",
            Unsupported("Undertow specific"),
        ),
        scalar("spring.application.name", Supported),
        scalar("spring.config.location", Supported),
        scalar("spring.profiles.active", Supported),
        scalar(
            "spring.servlet.multipart.enabled",
            Unsupported("bodies are never buffered"),
        ),
        scalar("spring.session.store-type", Planned("P12")),
        scalar("spring.session.redis.flush-mode", Planned("P12")),
        scalar("spring.session.redis.repository-type", Planned("P12")),
        scalar("spring.session.timeout", Planned("P4")),
        scalar("spring.data.redis.host", Planned("P12")),
        scalar("spring.data.redis.port", Planned("P12")),
        scalar("spring.data.redis.password", Planned("P12")),
        scalar("spring.data.redis.username", Planned("P12")),
        scalar("spring.data.redis.database", Planned("P12")),
        scalar("spring.data.redis.sentinel.master", Planned("P12")),
        list("spring.data.redis.sentinel.nodes", Planned("P12")),
        scalar("spring.data.redis.sentinel.password", Planned("P12")),
        scalar("spring.mail.host", Planned("P7")),
        scalar("spring.mail.port", Planned("P7")),
        scalar("spring.mail.username", Planned("P7")),
        scalar("spring.mail.password", Planned("P7")),
        map("spring.mail.properties", Planned("P7")),
        scalar("springdoc.api-docs.enabled", Planned("P7")),
        scalar("springdoc.swagger-ui.enabled", Planned("P7")),
        scalar("management.server.port", Planned("P10")),
        scalar("management.endpoints.web.exposure.include", Planned("P10")),
        scalar("management.endpoint.health.probes.enabled", Planned("P10")),
        scalar(
            "management.endpoint.health.group.readiness.include",
            Planned("P10"),
        ),
        scalar("management.health.ldap.enabled", Planned("P10")),
        scalar("management.health.redis.enabled", Planned("P10")),
        scalar("management.defaults.metrics.export.enabled", Planned("P10")),
        // --- logging ---
        scalar("proxy.log-as-json", Planned("P10")),
        scalar("logging.file.name", Planned("P10")),
        map("logging.level", Planned("P10")),
        scalar("logging.include-application-name", Planned("P10")),
        scalar("logging.requestdump", Planned("P10")),
        // --- ui ---
        scalar("proxy.title", Planned("P4")),
        scalar("proxy.logo-url", Planned("P4")),
        scalar("proxy.logo-width", Planned("P4")),
        scalar("proxy.logo-height", Planned("P4")),
        scalar("proxy.logo-style", Planned("P4")),
        scalar("proxy.favicon-path", Planned("P4")),
        scalar("proxy.landing-page", Planned("P4")),
        scalar("proxy.hide-navbar", Planned("P4")),
        list("proxy.body-classes", Planned("P4")),
        scalar("proxy.notification-message", Planned("P4")),
        scalar("proxy.template-path", Planned("P9")),
        scalar("proxy.my-apps-mode", Planned("P9")),
        scalar("proxy.default-app-logo-url", Planned("P4")),
        scalar("proxy.default-app-logo-width", Planned("P4")),
        scalar("proxy.default-app-logo-height", Planned("P4")),
        scalar("proxy.default-app-logo-style", Planned("P4")),
        scalar("proxy.default-app-logo-classes", Planned("P4")),
        scalar("proxy.support.mail-to-address", Planned("P7")),
        scalar("proxy.support.mail-from-address", Planned("P7")),
        scalar("proxy.support.mail-subject", Planned("P7")),
        scalar("proxy.monitoring.grafana-url", Planned("P9")),
        // --- authentication & authorisation ---
        scalar("proxy.authentication", Planned("P4")),
        list("proxy.admin-groups", Planned("P4")),
        list("proxy.admin-users", Planned("P4")),
        scalar("proxy.username-case-sensitive", Planned("P4")),
        scalar("proxy.users[].name", Planned("P4")),
        scalar("proxy.users[].password", Planned("P4")),
        list("proxy.users[].groups", Planned("P4")),
        scalar("proxy.allow-transfer-app", Planned("P7")),
        scalar("proxy.api-security.hide-spec-details", Planned("P7")),
        scalar("proxy.api-security.disable-no-sniff-header", Planned("P4")),
        scalar("proxy.api-security.disable-hsts-header", Planned("P4")),
        scalar(
            "proxy.api-security.disable-xss-protection-header",
            Planned("P4"),
        ),
        list("proxy.api-security.cors-allowed-origins", Planned("P4")),
        scalar("proxy.api-security.custom-headers[].name", Planned("P4")),
        scalar("proxy.api-security.custom-headers[].value", Planned("P4")),
        scalar("proxy.oauth2.resource-id", Planned("P11")),
        scalar("proxy.oauth2.jwks-url", Planned("P11")),
        scalar("proxy.oauth2.roles-claim", Planned("P11")),
        scalar("proxy.oauth2.username-attribute", Planned("P11")),
        scalar("proxy.openid.auth-url", Planned("P11")),
        scalar("proxy.openid.token-url", Planned("P11")),
        scalar("proxy.openid.jwks-url", Planned("P11")),
        scalar("proxy.openid.userinfo-url", Planned("P11")),
        scalar("proxy.openid.logout-url", Planned("P11")),
        scalar("proxy.openid.client-id", Planned("P11")),
        scalar("proxy.openid.client-secret", Planned("P11")),
        scalar("proxy.openid.client-authentication-method", Planned("P11")),
        list("proxy.openid.scopes", Planned("P11")),
        scalar("proxy.openid.username-attribute", Planned("P11")),
        scalar("proxy.openid.roles-claim", Planned("P11")),
        scalar("proxy.openid.with-pkce", Planned("P11")),
        scalar("proxy.openid.include-default-scopes", Planned("P11")),
        scalar("proxy.openid.enforce-https-redirect-uri", Planned("P11")),
        scalar("proxy.openid.ignore-session-expire", Planned("P11")),
        scalar("proxy.openid.jwks-signature-algorithm", Planned("P11")),
        // LDAP can be configured with a single provider (`proxy.ldap.url`) or with a list of providers
        // (`proxy.ldap[0].url`); both notations use the same property names.
        scalar("proxy.ldap[].url", Planned("P11")),
        scalar("proxy.ldap[].starttls", Planned("P11")),
        scalar("proxy.ldap[].user-dn-pattern", Planned("P11")),
        scalar("proxy.ldap[].user-search-base", Planned("P11")),
        scalar("proxy.ldap[].user-search-filter", Planned("P11")),
        scalar("proxy.ldap[].group-search-base", Planned("P11")),
        scalar("proxy.ldap[].group-search-filter", Planned("P11")),
        scalar("proxy.ldap[].manager-dn", Planned("P11")),
        scalar("proxy.ldap[].manager-password", Planned("P11")),
        scalar("proxy.webservice.authentication-url", Planned("P11")),
        scalar(
            "proxy.webservice.authentication-request-body",
            Planned("P11"),
        ),
        scalar("proxy.webservice.groups-expression", Planned("P11")),
        scalar("proxy.custom-header.username-header-name", Planned("P11")),
        scalar("proxy.custom-header.groups-header-name", Planned("P11")),
        scalar("proxy.ms-graph.client-id", Planned("P11")),
        scalar("proxy.ms-graph.client-secret", Planned("P11")),
        scalar("proxy.ms-graph.tenant-id", Planned("P11")),
        scalar("proxy.ms-graph.api-url", Planned("P11")),
        scalar("proxy.ms-graph.token-url", Planned("P11")),
        list("proxy.ms-graph.scopes", Planned("P11")),
        scalar("proxy.saml.app-entity-id", Planned("P11")),
        scalar("proxy.saml.app-base-url", Planned("P11")),
        scalar("proxy.saml.idp-metadata-url", Planned("P11")),
        scalar("proxy.saml.keystore", Planned("P11")),
        scalar("proxy.saml.keystore-password", Planned("P11")),
        scalar("proxy.saml.encryption-cert-name", Planned("P11")),
        scalar("proxy.saml.encryption-cert-password", Planned("P11")),
        scalar("proxy.saml.name-attribute", Planned("P11")),
        scalar("proxy.saml.roles-attribute", Planned("P11")),
        scalar("proxy.saml.force-authn", Planned("P11")),
        scalar("proxy.saml.log-attributes", Planned("P11")),
        scalar("proxy.saml.logout-url", Planned("P11")),
        scalar("proxy.saml.logout-method", Planned("P11")),
        // --- proxy behaviour ---
        scalar("proxy.heartbeat-rate", Planned("P6")),
        scalar("proxy.heartbeat-timeout", Planned("P6")),
        scalar("proxy.container-wait-time", Planned("P5")),
        scalar("proxy.container-wait-timeout", Planned("P5")),
        scalar("proxy.max-total-instances", Planned("P5")),
        scalar("proxy.default-max-instances", Planned("P5")),
        scalar("proxy.default-proxy-max-lifetime", Planned("P5")),
        scalar("proxy.default-cache-headers-mode", Planned("P6")),
        scalar("proxy.default-stop-proxy-on-logout", Planned("P10")),
        scalar("proxy.default-always-switch-instance", Planned("P9")),
        scalar("proxy.default-websocket-reconnection-mode", Planned("P6")),
        scalar("proxy.default-track-app-url", Planned("P9")),
        scalar("proxy.stop-proxies-on-shutdown", Planned("P5")),
        scalar("proxy.recover-running-proxies", Planned("P8")),
        scalar(
            "proxy.recover-running-proxies-from-different-config",
            Planned("P8"),
        ),
        scalar("proxy.store-mode", Planned("P12")),
        scalar("proxy.seat-wait-time", Planned("P12")),
        scalar("proxy.realm-id", Supported),
        scalar("proxy.version", Supported),
        scalar("proxy.container-backend", Planned("P5")),
        scalar("proxy.container-log-path", Planned("P10")),
        scalar("proxy.container-log-s3-access-key", Planned("P10")),
        scalar("proxy.container-log-s3-access-secret", Planned("P10")),
        scalar("proxy.container-log-s3-endpoint", Planned("P10")),
        scalar("proxy.container-log-s3-sse", Planned("P10")),
        // --- usage statistics ---
        scalar("proxy.usage-stats-url", Planned("P10")),
        scalar("proxy.usage-stats-username", Planned("P10")),
        scalar("proxy.usage-stats-password", Planned("P10")),
        scalar("proxy.usage-stats-table-name", Planned("P10")),
        scalar("proxy.usage-stats-attributes[].name", Planned("P10")),
        scalar("proxy.usage-stats-attributes[].expression", Planned("P10")),
        scalar("proxy.usage-stats[].url", Planned("P10")),
        scalar("proxy.usage-stats[].username", Planned("P10")),
        scalar("proxy.usage-stats[].password", Planned("P10")),
        scalar("proxy.usage-stats[].table-name", Planned("P10")),
        scalar("proxy.usage-stats[].attributes[].name", Planned("P10")),
        scalar(
            "proxy.usage-stats[].attributes[].expression",
            Planned("P10"),
        ),
        scalar("proxy.usage-stats-micrometer-prefix", Planned("P10")),
        scalar(
            "proxy.usage-stats-hikari.connection-timeout",
            Planned("P10"),
        ),
        scalar("proxy.usage-stats-hikari.idle-timeout", Planned("P10")),
        scalar("proxy.usage-stats-hikari.max-lifetime", Planned("P10")),
        scalar("proxy.usage-stats-hikari.minimum-idle", Planned("P10")),
        scalar("proxy.usage-stats-hikari.maximum-pool-size", Planned("P10")),
        // --- docker backend ---
        scalar("proxy.docker.url", Planned("P8")),
        scalar("proxy.docker.cert-path", Planned("P8")),
        scalar("proxy.docker.port-range-start", Planned("P5")),
        scalar("proxy.docker.port-range-max", Planned("P5")),
        scalar("proxy.docker.target-url", Planned("P8")),
        scalar("proxy.docker.target-bind-ip", Planned("P8")),
        scalar("proxy.docker.default-container-network", Planned("P8")),
        scalar("proxy.docker.internal-networking", Planned("P8")),
        scalar("proxy.docker.container-protocol", Planned("P8")),
        scalar("proxy.docker.privileged", Planned("P8")),
        scalar("proxy.docker.image-pull-policy", Planned("P8")),
        scalar("proxy.docker.loki-url", Planned("P8")),
        scalar("proxy.docker.service-wait-time", Planned("P8")),
        // --- kubernetes backend ---
        scalar("proxy.kubernetes.url", Planned("P12")),
        scalar("proxy.kubernetes.cert-path", Planned("P12")),
        scalar("proxy.kubernetes.namespace", Planned("P12")),
        scalar("proxy.kubernetes.api-version", Planned("P12")),
        scalar("proxy.kubernetes.image-pull-policy", Planned("P12")),
        list("proxy.kubernetes.image-pull-secrets", Planned("P12")),
        scalar("proxy.kubernetes.image-pull-secret", Planned("P12")),
        map("proxy.kubernetes.node-selector", Planned("P12")),
        scalar("proxy.kubernetes.cluster-domain", Planned("P12")),
        scalar("proxy.kubernetes.internal-networking", Planned("P12")),
        scalar("proxy.kubernetes.container-protocol", Planned("P12")),
        scalar("proxy.kubernetes.privileged", Planned("P12")),
        scalar("proxy.kubernetes.pod-wait-time", Planned("P12")),
        scalar("proxy.kubernetes.debug-patches", Planned("P12")),
        list("proxy.kubernetes.authorized-pod-patches", Planned("P12")),
        list(
            "proxy.kubernetes.authorized-additional-manifests",
            Planned("P12"),
        ),
        list(
            "proxy.kubernetes.authorized-additional-persistent-manifests",
            Planned("P12"),
        ),
        // --- ecs backend ---
        scalar("proxy.ecs.name", Planned("P12")),
        scalar("proxy.ecs.region", Planned("P12")),
        scalar("proxy.ecs.service-wait-time", Planned("P12")),
        list("proxy.ecs.subnets", Planned("P12")),
        list("proxy.ecs.security-groups", Planned("P12")),
        // note: `proxy.ecs.enable-cloudwatch` is an accepted alias (Java `EnvironmentUtils.getProperty`
        // fallback); both spellings canonicalise identically.
        scalar("proxy.ecs.enable-cloud-watch", Planned("P12")),
        scalar("proxy.ecs.cloud-watch-group-prefix", Planned("P12")),
        scalar("proxy.ecs.cloud-watch-region", Planned("P12")),
        scalar("proxy.ecs.cloud-watch-stream-prefix", Planned("P12")),
        scalar(
            "proxy.ecs.default-repository-credentials-parameter",
            Planned("P12"),
        ),
        scalar("proxy.ecs.privileged", Planned("P12")),
        scalar("proxy.ecs.internal-networking", Planned("P12")),
    ]
};

/// The configuration schema: engine properties plus the properties contributed by the application
/// (ShinyProxy adds `proxy.specs[]` and `proxy.template-groups[]`).
#[derive(Debug, Clone, Default)]
pub struct Schema {
    keys: Vec<KeyDef>,
}

impl Schema {
    /// Schema containing only the engine properties.
    pub fn engine() -> Self {
        Schema {
            keys: engine_keys().to_vec(),
        }
    }

    /// Adds application specific properties.
    pub fn with_keys(mut self, keys: &[KeyDef]) -> Self {
        self.keys.extend_from_slice(keys);
        self
    }

    /// All known keys.
    pub fn keys(&self) -> &[KeyDef] {
        &self.keys
    }

    /// Returns the definition of the given path (indexes are ignored during matching).
    pub fn find(&self, path: &str) -> Option<&KeyDef> {
        let wanted = canonical_path(path);
        self.keys
            .iter()
            .find(|key| canonical_path(key.path) == wanted)
    }

    /// Returns the definition of the map property that contains the given path, if any.
    pub fn enclosing_map(&self, path: &str) -> Option<&KeyDef> {
        let wanted = canonical_path(path);
        self.keys.iter().find(|key| {
            key.kind == KeyKind::Map
                && wanted.starts_with(&format!("{}.", canonical_path(key.path)))
        })
    }

    /// True when the given (possibly indexed) path is known, either directly or as a key below a
    /// free form map property.
    pub fn is_known(&self, path: &str) -> bool {
        self.find(path).is_some() || self.enclosing_map(path).is_some()
    }

    /// Canonical spelling of the last segment of a known path, used to normalise relaxed names.
    ///
    /// Works for leaves (`proxy.HEARTBEAT_RATE` -> `heartbeat-rate`) as well as for intermediate nodes
    /// (`proxy.DOCKER` -> `docker`), because the latter are needed to rebuild a tree that serde can
    /// deserialize.
    pub fn canonical_segment(&self, path: &str) -> Option<&'static str> {
        let canonical = canonical_path(path);
        if canonical.is_empty() {
            return None;
        }
        let depth = canonical.split('.').count();
        let prefix = format!("{canonical}.");
        for key in &self.keys {
            let key_canonical = canonical_path(key.path);
            if key_canonical == canonical || key_canonical.starts_with(&prefix) {
                let segments: Vec<&'static str> = key
                    .path
                    .split('.')
                    .map(|segment| segment.trim_end_matches("[]"))
                    .collect();
                if let Some(segment) = segments.get(depth - 1) {
                    return Some(segment);
                }
            }
        }
        None
    }

    /// All keys whose path contains no array, i.e. those bindable from a single environment variable.
    pub fn simple_keys(&self) -> impl Iterator<Item = &KeyDef> {
        self.keys.iter().filter(|key| !key.path.contains("[]"))
    }

    /// Keys grouped by their array root: `proxy.users` -> [`proxy.users[].name`, ...].
    pub fn array_groups(&self) -> Vec<(String, Vec<&KeyDef>)> {
        let mut roots: Vec<String> = self
            .keys
            .iter()
            .filter_map(|key| key.path.split_once("[]").map(|(root, _)| root.to_string()))
            .collect();
        roots.sort();
        roots.dedup();
        roots
            .into_iter()
            .map(|root| {
                let prefix = format!("{root}[]");
                let members = self
                    .keys
                    .iter()
                    .filter(|key| key.path.starts_with(&prefix))
                    .collect();
                (root, members)
            })
            .collect()
    }

    /// Sorted set of the documented paths, used by `docs/CONFIGURATION.md` tests.
    pub fn documented_paths(&self) -> BTreeSet<String> {
        self.keys.iter().map(|key| key.path.to_string()).collect()
    }
}

/// Canonical form of a path: indexes and `[]` markers removed, names relaxed.
pub fn canonical_path(path: &str) -> String {
    parse_path(path)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Key(key) => {
                let key = key.trim_end_matches("[]");
                if key.is_empty() {
                    None
                } else {
                    Some(canonical_name(key))
                }
            }
            Segment::Index(_) => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalises_paths() {
        assert_eq!(
            canonical_path("proxy.docker.port-range-start"),
            "proxy.docker.portrangestart"
        );
        assert_eq!(
            canonical_path("proxy.specs[0].container-image"),
            "proxy.specs.containerimage"
        );
        assert_eq!(canonical_path("proxy.users[].name"), "proxy.users.name");
        assert_eq!(canonical_path("proxy.admin-groups[1]"), "proxy.admingroups");
    }

    #[test]
    fn knows_engine_properties() {
        let schema = Schema::engine();
        assert!(schema.is_known("proxy.port"));
        assert!(schema.is_known("proxy.docker.port-range-start"));
        assert!(schema.is_known("proxy.docker.portRangeStart"));
        assert!(schema.is_known("proxy.admin-groups[0]"));
        assert!(schema.is_known("proxy.users[2].password"));
        assert!(schema.is_known("logging.level.org.something"));
        assert!(schema.is_known("proxy.usage-stats[0].attributes[1].name"));
        assert!(!schema.is_known("proxy.does-not-exist"));
    }

    #[test]
    fn every_key_has_a_unique_canonical_path() {
        let schema = Schema::engine();
        let mut seen = std::collections::HashMap::new();
        for key in schema.keys() {
            let canonical = canonical_path(key.path);
            if let Some(previous) = seen.insert(canonical.clone(), key.path) {
                assert_eq!(previous, key.path, "duplicate canonical path {canonical}");
            }
        }
    }

    #[test]
    fn groups_array_keys() {
        let schema = Schema::engine();
        let groups = schema.array_groups();
        let users = groups
            .iter()
            .find(|(root, _)| root == "proxy.users")
            .expect("proxy.users group");
        assert_eq!(users.1.len(), 3);
        assert!(groups.iter().any(|(root, _)| root == "proxy.usage-stats"));
    }

    #[test]
    fn reports_canonical_spelling() {
        let schema = Schema::engine();
        assert_eq!(
            schema.canonical_segment("proxy.HEARTBEAT_RATE"),
            Some("heartbeat-rate")
        );
        assert_eq!(
            schema.canonical_segment("proxy.users[0].name"),
            Some("name")
        );
        assert_eq!(schema.canonical_segment("proxy.DOCKER"), Some("docker"));
        assert_eq!(schema.canonical_segment("PROXY"), Some("proxy"));
    }
}
