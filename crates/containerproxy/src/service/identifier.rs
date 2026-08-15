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

//! Identifiers of this server and of its configuration (`IdentifierService` in the Java code).

use serde_json::Value;
use uuid::Uuid;

use crate::config::RawConfig;
use crate::util::sha1_of_value;

/// Instance id used when no configuration file is present (matches the Java implementation).
pub const UNKNOWN_INSTANCE_ID: &str = "unknown-instance-id";

/// Identifiers of the running server.
#[derive(Debug, Clone)]
pub struct Identifiers {
    /// Identifies this *run* of the server; unique for every start.
    ///
    /// In Kubernetes the last four characters of `SP_KUBE_POD_NAME` are used, so that the id matches
    /// the pod, exactly like the Java implementation.
    pub runtime_id: String,
    /// Identifies the *configuration*: a SHA-1 hash of the canonical form of `application.yml`.
    ///
    /// Used in asset URLs (so that clients reload assets when the configuration changes), in container
    /// labels (for app recovery) and to detect outdated servers in a high availability setup.
    pub instance_id: String,
    /// Realm this server operates in (`proxy.realm-id`).
    pub realm_id: Option<String>,
    /// Optional configuration version (`proxy.version`).
    pub version: Option<i64>,
}

impl Identifiers {
    /// Derives the identifiers from the loaded configuration.
    pub fn from_config(config: &RawConfig, pod_name: Option<&str>) -> Self {
        let runtime_id = match pod_name {
            Some(name) if name.len() > 4 => name[name.len() - 4..].to_string(),
            Some(name) if !name.is_empty() => name.to_string(),
            _ => Uuid::new_v4().to_string(),
        };

        let instance_id = match &config.file_tree {
            Some(tree) => instance_id_of(tree),
            // Only happens when no configuration file exists (e.g. the demo profile, or in tests).
            None => UNKNOWN_INSTANCE_ID.to_string(),
        };

        Identifiers {
            runtime_id,
            instance_id,
            realm_id: config.property("proxy.realm-id"),
            version: config
                .property("proxy.version")
                .and_then(|value| value.parse::<i64>().ok()),
        }
    }

    /// Logs the identifiers the way the Java implementation does at startup.
    pub fn log(&self) {
        tracing::info!(
            "ShinyProxy runtimeId:                   {}",
            self.runtime_id
        );
        tracing::info!(
            "ShinyProxy instanceID (hash of config): {}",
            self.instance_id
        );
        if let Some(realm_id) = &self.realm_id {
            tracing::info!("ShinyProxy realmId:                     {realm_id}");
        }
        if let Some(version) = self.version {
            tracing::info!("ShinyProxy version:                     {version}");
        }
    }
}

/// Instance id of a parsed configuration tree.
pub fn instance_id_of(tree: &Value) -> String {
    sha1_of_value(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load, LoadOptions, Schema};

    fn config_of(yaml: &str) -> RawConfig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("application.yml"), yaml).unwrap();
        let options = LoadOptions {
            working_dir: Some(dir.path().to_path_buf()),
            ..LoadOptions::default()
        };
        load(&Schema::engine(), &options).expect("config loads")
    }

    /// The expected hash was produced by running the Java implementation's `Sha1#hash(Object)` on the
    /// same file (see `docs/COMPATIBILITY.md` for the reference program).
    #[test]
    fn instance_id_matches_the_java_implementation() {
        let config = config_of(include_str!("../../../../examples/application-demo.yml"));
        let identifiers = Identifiers::from_config(&config, None);
        assert_eq!(
            identifiers.instance_id,
            "fa8f0913d4309dbe1fe44411fc59f5c6d6937837"
        );
    }

    #[test]
    fn instance_id_ignores_comments_and_key_order() {
        let first = config_of("proxy:\n  port: 8080\n  title: Test\n");
        let second = config_of("# a comment\nproxy:\n  title: Test\n\n  port: 8080\n");
        assert_eq!(
            Identifiers::from_config(&first, None).instance_id,
            Identifiers::from_config(&second, None).instance_id
        );
    }

    #[test]
    fn instance_id_changes_with_the_configuration() {
        let first = config_of("proxy:\n  port: 8080\n");
        let second = config_of("proxy:\n  port: 8081\n");
        assert_ne!(
            Identifiers::from_config(&first, None).instance_id,
            Identifiers::from_config(&second, None).instance_id
        );
    }

    #[test]
    fn runtime_id_uses_the_pod_name_suffix() {
        let config = config_of("proxy:\n  port: 8080\n");
        let identifiers = Identifiers::from_config(&config, Some("shinyproxy-6d4b8c7d9f-abcde"));
        assert_eq!(identifiers.runtime_id, "bcde");

        let identifiers = Identifiers::from_config(&config, None);
        assert_eq!(identifiers.runtime_id.len(), 36, "expected a uuid");
    }

    #[test]
    fn reads_realm_and_version() {
        let config = config_of("proxy:\n  realm-id: prod\n  version: 3\n");
        let identifiers = Identifiers::from_config(&config, None);
        assert_eq!(identifiers.realm_id.as_deref(), Some("prod"));
        assert_eq!(identifiers.version, Some(3));
    }

    #[test]
    fn instance_id_is_unknown_without_configuration_file() {
        let dir = tempfile::tempdir().unwrap();
        let options = LoadOptions {
            working_dir: Some(dir.path().to_path_buf()),
            ..LoadOptions::default()
        };
        let config = load(&Schema::engine(), &options).unwrap();
        let identifiers = Identifiers::from_config(&config, None);
        assert_eq!(identifiers.instance_id, UNKNOWN_INSTANCE_ID);
    }
}
