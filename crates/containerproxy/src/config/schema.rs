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
    /// Accepted by the parser, behaviour lands in the given phase (nothing is left in this state; kept so a
    /// property that is added before its behaviour can be marked honestly).
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

use Support::{Supported, Unsupported};

/// Properties of the engine (the Java `containerproxy` library).
pub fn engine_keys() -> &'static [KeyDef] {
    ENGINE_KEYS
}

static ENGINE_KEYS: &[KeyDef] = {
    &[
        // --- server ---
        scalar("proxy.port", Supported),
        scalar("proxy.bind-address", Supported),
        scalar("proxy.same-site-cookie", Supported),
        scalar("server.servlet.context-path", Supported),
        scalar("server.secure-cookies", Supported),
        scalar("server.frame-options", Supported),
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
        scalar("spring.session.store-type", Supported),
        scalar("spring.session.redis.flush-mode", Unsupported("Spring Session specific; sessions are written immediately")),
        scalar("spring.session.redis.repository-type", Unsupported("Spring Session specific; there is one Redis session store")),
        scalar("spring.session.timeout", Supported),
        scalar("spring.data.redis.host", Supported),
        scalar("spring.data.redis.port", Supported),
        scalar("spring.data.redis.password", Supported),
        scalar("spring.data.redis.username", Supported),
        scalar("spring.data.redis.database", Supported),
        scalar("spring.data.redis.sentinel.master", Supported),
        list("spring.data.redis.sentinel.nodes", Supported),
        scalar("spring.data.redis.sentinel.password", Supported),
        scalar("spring.mail.host", Supported),
        scalar("spring.mail.port", Supported),
        scalar("spring.mail.username", Supported),
        scalar("spring.mail.password", Supported),
        map("spring.mail.properties", Supported),
        scalar("springdoc.api-docs.enabled", Supported),
        scalar("springdoc.swagger-ui.enabled", Supported),
        scalar("management.server.port", Supported),
        scalar("management.endpoints.web.exposure.include", Unsupported("health, prometheus and recyclable are always exposed")),
        scalar("management.endpoint.health.probes.enabled", Unsupported("the liveness and readiness probes are always available")),
        scalar(
            "management.endpoint.health.group.readiness.include",
            Unsupported("the readiness probe of this implementation always reports app recovery"),
        ),
        scalar("management.health.ldap.enabled", Unsupported("the health endpoint does not check the directory")),
        scalar("management.health.redis.enabled", Unsupported("the health endpoint does not check Redis")),
        scalar("management.defaults.metrics.export.enabled", Unsupported("the metrics of this implementation are always collected")),
        // --- logging ---
        scalar("proxy.log-as-json", Supported),
        scalar("logging.file.name", Supported),
        map("logging.level", Supported),
        scalar("logging.include-application-name", Unsupported("Logback specific; the log format of this implementation always names the module")),
        scalar("logging.requestdump", Unsupported("request dumping is not implemented; use logging.level.* instead")),
        // --- ui ---
        scalar("proxy.title", Supported),
        scalar("proxy.logo-url", Supported),
        scalar("proxy.logo-width", Supported),
        scalar("proxy.logo-height", Supported),
        scalar("proxy.logo-style", Supported),
        scalar("proxy.favicon-path", Supported),
        scalar("proxy.landing-page", Supported),
        scalar("proxy.hide-navbar", Supported),
        list("proxy.body-classes", Supported),
        scalar("proxy.notification-message", Supported),
        scalar("proxy.template-path", Supported),
        scalar("proxy.my-apps-mode", Supported),
        scalar("proxy.default-app-logo-url", Supported),
        scalar("proxy.default-app-logo-width", Supported),
        scalar("proxy.default-app-logo-height", Supported),
        scalar("proxy.default-app-logo-style", Supported),
        scalar("proxy.default-app-logo-classes", Supported),
        scalar("proxy.support.mail-to-address", Supported),
        scalar("proxy.support.mail-from-address", Supported),
        scalar("proxy.support.mail-subject", Supported),
        scalar("proxy.monitoring.grafana-url", Supported),
        // --- authentication & authorisation ---
        scalar("proxy.authentication", Supported),
        list("proxy.admin-groups", Supported),
        list("proxy.admin-users", Supported),
        scalar("proxy.username-case-sensitive", Supported),
        scalar("proxy.users[].name", Supported),
        scalar("proxy.users[].password", Supported),
        list("proxy.users[].groups", Supported),
        scalar("proxy.allow-transfer-app", Supported),
        scalar("proxy.api-security.hide-spec-details", Supported),
        scalar("proxy.api-security.disable-no-sniff-header", Supported),
        scalar("proxy.api-security.disable-hsts-header", Supported),
        scalar("proxy.api-security.disable-xss-protection-header", Supported),
        list("proxy.api-security.cors-allowed-origins", Supported),
        scalar("proxy.api-security.custom-headers[].name", Supported),
        scalar("proxy.api-security.custom-headers[].value", Supported),
        scalar("proxy.oauth2.resource-id", Supported),
        scalar("proxy.oauth2.jwks-url", Supported),
        scalar("proxy.oauth2.roles-claim", Supported),
        scalar("proxy.oauth2.username-attribute", Supported),
        scalar("proxy.openid.auth-url", Supported),
        scalar("proxy.openid.token-url", Supported),
        scalar("proxy.openid.jwks-url", Supported),
        scalar("proxy.openid.userinfo-url", Supported),
        scalar("proxy.openid.logout-url", Supported),
        scalar("proxy.openid.client-id", Supported),
        scalar("proxy.openid.client-secret", Supported),
        scalar("proxy.openid.client-authentication-method", Supported),
        list("proxy.openid.scopes", Supported),
        scalar("proxy.openid.username-attribute", Supported),
        scalar("proxy.openid.roles-claim", Supported),
        scalar("proxy.openid.with-pkce", Supported),
        scalar("proxy.openid.include-default-scopes", Supported),
        scalar("proxy.openid.enforce-https-redirect-uri", Supported),
        scalar("proxy.openid.ignore-session-expire", Supported),
        scalar("proxy.openid.jwks-signature-algorithm", Supported),
        // LDAP can be configured with a single provider (`proxy.ldap.url`) or with a list of providers
        // (`proxy.ldap[0].url`); both notations use the same property names.
        scalar("proxy.ldap[].url", Supported),
        scalar("proxy.ldap[].starttls", Supported),
        scalar("proxy.ldap[].user-dn-pattern", Supported),
        scalar("proxy.ldap[].user-search-base", Supported),
        scalar("proxy.ldap[].user-search-filter", Supported),
        scalar("proxy.ldap[].group-search-base", Supported),
        scalar("proxy.ldap[].group-search-filter", Supported),
        scalar("proxy.ldap[].manager-dn", Supported),
        scalar("proxy.ldap[].manager-password", Supported),
        scalar("proxy.webservice.authentication-url", Supported),
        scalar("proxy.webservice.authentication-request-body", Supported),
        scalar("proxy.webservice.groups-expression", Supported),
        scalar("proxy.custom-header.username-header-name", Supported),
        scalar("proxy.custom-header.groups-header-name", Supported),
        scalar("proxy.ms-graph.client-id", Supported),
        scalar("proxy.ms-graph.client-secret", Supported),
        scalar("proxy.ms-graph.tenant-id", Supported),
        scalar("proxy.ms-graph.api-url", Supported),
        scalar("proxy.ms-graph.token-url", Supported),
        list("proxy.ms-graph.scopes", Supported),
        scalar("proxy.saml.app-entity-id", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.app-base-url", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.idp-metadata-url", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.keystore", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.keystore-password", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.encryption-cert-name", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.encryption-cert-password", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.name-attribute", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.roles-attribute", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.force-authn", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.log-attributes", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.logout-url", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        scalar("proxy.saml.logout-method", Unsupported("SAML authentication is not implemented; the server refuses to start with it (use openid)")),
        // --- proxy behaviour ---
        scalar("proxy.heartbeat-rate", Supported),
        scalar("proxy.heartbeat-timeout", Supported),
        scalar("proxy.container-wait-time", Supported),
        scalar("proxy.container-wait-timeout", Supported),
        scalar("proxy.max-total-instances", Supported),
        scalar("proxy.default-max-instances", Supported),
        scalar("proxy.default-proxy-max-lifetime", Supported),
        scalar("proxy.default-cache-headers-mode", Supported),
        scalar("proxy.default-stop-proxy-on-logout", Supported),
        scalar("proxy.default-always-switch-instance", Supported),
        scalar("proxy.default-websocket-reconnection-mode", Supported),
        scalar("proxy.default-track-app-url", Supported),
        scalar("proxy.stop-proxies-on-shutdown", Supported),
        scalar("proxy.recover-running-proxies", Supported),
        scalar("proxy.recover-running-proxies-from-different-config", Supported),
        scalar("proxy.store-mode", Supported),
        scalar("proxy.seat-wait-time", Supported),
        scalar("proxy.realm-id", Supported),
        scalar("proxy.version", Supported),
        scalar("proxy.container-backend", Supported),
        scalar("proxy.container-log-path", Supported),
        scalar("proxy.container-log-s3-access-key", Unsupported("S3 log storage is not implemented; ship the log files instead")),
        scalar("proxy.container-log-s3-access-secret", Unsupported("S3 log storage is not implemented; ship the log files instead")),
        scalar("proxy.container-log-s3-endpoint", Unsupported("S3 log storage is not implemented; ship the log files instead")),
        scalar("proxy.container-log-s3-sse", Unsupported("S3 log storage is not implemented; ship the log files instead")),
        // --- usage statistics ---
        scalar("proxy.usage-stats-url", Supported),
        scalar("proxy.usage-stats-username", Supported),
        scalar("proxy.usage-stats-password", Supported),
        scalar("proxy.usage-stats-table-name", Supported),
        scalar("proxy.usage-stats-attributes[].name", Supported),
        scalar("proxy.usage-stats-attributes[].expression", Supported),
        scalar("proxy.usage-stats[].url", Supported),
        scalar("proxy.usage-stats[].username", Supported),
        scalar("proxy.usage-stats[].password", Supported),
        scalar("proxy.usage-stats[].table-name", Supported),
        scalar("proxy.usage-stats[].attributes[].name", Supported),
        scalar("proxy.usage-stats[].attributes[].expression", Supported),
        scalar("proxy.usage-stats-micrometer-prefix", Supported),
        scalar("proxy.usage-stats-hikari.connection-timeout", Supported),
        scalar("proxy.usage-stats-hikari.idle-timeout", Supported),
        scalar("proxy.usage-stats-hikari.max-lifetime", Supported),
        scalar("proxy.usage-stats-hikari.minimum-idle", Supported),
        scalar("proxy.usage-stats-hikari.maximum-pool-size", Supported),
        // --- docker backend ---
        scalar("proxy.docker.url", Supported),
        scalar("proxy.docker.cert-path", Supported),
        scalar("proxy.docker.port-range-start", Supported),
        scalar("proxy.docker.port-range-max", Supported),
        scalar("proxy.docker.target-url", Supported),
        scalar("proxy.docker.target-bind-ip", Supported),
        scalar("proxy.docker.default-container-network", Supported),
        scalar("proxy.docker.internal-networking", Supported),
        scalar("proxy.docker.container-protocol", Supported),
        scalar("proxy.docker.privileged", Supported),
        scalar("proxy.docker.image-pull-policy", Supported),
        scalar("proxy.docker.loki-url", Supported),
        scalar("proxy.docker.service-wait-time", Supported),
        // --- kubernetes backend ---
        scalar("proxy.kubernetes.url", Supported),
        scalar("proxy.kubernetes.cert-path", Supported),
        scalar("proxy.kubernetes.namespace", Supported),
        scalar("proxy.kubernetes.api-version", Supported),
        scalar("proxy.kubernetes.image-pull-policy", Supported),
        list("proxy.kubernetes.image-pull-secrets", Supported),
        scalar("proxy.kubernetes.image-pull-secret", Supported),
        scalar("proxy.kubernetes.node-selector", Supported),
        map("proxy.kubernetes.node-selector", Supported),
        list("proxy.kubernetes.app-namespaces", Supported),
        scalar("proxy.kubernetes.cluster-domain", Supported),
        scalar("proxy.kubernetes.internal-networking", Supported),
        scalar("proxy.kubernetes.container-protocol", Supported),
        scalar("proxy.kubernetes.privileged", Supported),
        scalar("proxy.kubernetes.pod-wait-time", Supported),
        scalar("proxy.kubernetes.debug-patches", Supported),
        list("proxy.kubernetes.authorized-pod-patches", Supported),
        list("proxy.kubernetes.authorized-additional-manifests", Supported),
        list("proxy.kubernetes.authorized-additional-persistent-manifests", Supported),
        // --- ecs backend ---
        scalar("proxy.ecs.name", Supported),
        scalar("proxy.ecs.region", Supported),
        scalar("proxy.ecs.service-wait-time", Supported),
        list("proxy.ecs.subnets", Supported),
        list("proxy.ecs.security-groups", Supported),
        // note: `proxy.ecs.enable-cloudwatch` is an accepted alias (Java `EnvironmentUtils.getProperty`
        // fallback); both spellings canonicalise identically.
        scalar("proxy.ecs.enable-cloud-watch", Supported),
        scalar("proxy.ecs.cloud-watch-group-prefix", Supported),
        scalar("proxy.ecs.cloud-watch-region", Supported),
        scalar("proxy.ecs.cloud-watch-stream-prefix", Supported),
        scalar("proxy.ecs.default-repository-credentials-parameter", Supported),
        scalar("proxy.ecs.privileged", Supported),
        scalar("proxy.ecs.internal-networking", Unsupported("ECS tasks are always reached on the private address of their network interface")),
        scalar("proxy.ecs.container-protocol", Supported),
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
