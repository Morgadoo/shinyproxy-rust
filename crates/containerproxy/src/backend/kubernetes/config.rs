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

//! The configuration of the Kubernetes backend (`proxy.kubernetes.*`).

use std::collections::BTreeMap;
use std::time::Duration;

use crate::config::Settings;

/// Everything the backend needs to know about the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesConfig {
    /// Namespace the pods are created in (`default`).
    pub namespace: String,
    /// Namespaces that are scanned for existing pods (`app-namespaces` plus `namespace`).
    pub app_namespaces: Vec<String>,
    /// API version of the manifests (`v1`).
    pub api_version: String,
    /// Image pull policy of the containers, when configured.
    pub image_pull_policy: Option<String>,
    /// Secrets used to pull the images (`image-pull-secret` and `image-pull-secrets`).
    pub image_pull_secrets: Vec<String>,
    /// Node selector of the pods, parsed from `a=b,c=d`.
    pub node_selector: BTreeMap<String, String>,
    /// DNS domain of the cluster (`cluster.local`).
    pub cluster_domain: String,
    /// How long a pod may take to become ready (`pod-wait-time`, 60 seconds).
    pub pod_wait_time: Duration,
    /// Whether ShinyProxy runs inside the cluster and reaches the pods directly.
    pub internal_networking: bool,
    /// Whether the containers run privileged.
    pub privileged: bool,
    /// Whether the generated manifests are logged (`debug-patches`).
    pub debug_manifests: bool,
    /// Protocol used to reach the apps.
    pub target_protocol: String,
    /// URL of the API server (`proxy.kubernetes.url`), when it is not read from the environment.
    pub url: Option<String>,
    /// Directory with `ca.pem`, `cert.pem` and `key.pem` (`proxy.kubernetes.cert-path`).
    pub cert_path: Option<String>,
}

impl KubernetesConfig {
    /// Reads the configuration.
    pub fn from_settings(settings: &Settings) -> Result<Self, String> {
        let kubernetes = &settings.proxy.kubernetes;

        let namespace = kubernetes.namespace().to_string();
        let mut app_namespaces: Vec<String> = kubernetes
            .app_namespaces
            .values()
            .iter()
            .filter(|namespace| !namespace.trim().is_empty())
            .cloned()
            .collect();
        if !app_namespaces.contains(&namespace) {
            app_namespaces.push(namespace.clone());
        }

        let mut image_pull_secrets: Vec<String> = Vec::new();
        if let Some(secret) = kubernetes
            .image_pull_secret
            .clone()
            .filter(|secret| !secret.trim().is_empty())
        {
            image_pull_secrets.push(secret);
        }
        for secret in kubernetes.image_pull_secrets.values() {
            if !secret.trim().is_empty() && !image_pull_secrets.contains(secret) {
                image_pull_secrets.push(secret.clone());
            }
        }

        // the node selector is a `key=value,key=value` string in Java; a map is accepted as well
        let node_selector = match &kubernetes.node_selector {
            Some(selector) => selector.pairs()?,
            None => BTreeMap::new(),
        };

        let target_protocol = kubernetes
            .container_protocol
            .clone()
            .filter(|protocol| !protocol.trim().is_empty())
            .unwrap_or_else(|| "http".to_string());

        Ok(KubernetesConfig {
            namespace,
            app_namespaces,
            api_version: kubernetes
                .api_version
                .clone()
                .filter(|version| !version.trim().is_empty())
                .unwrap_or_else(|| "v1".to_string()),
            image_pull_policy: kubernetes
                .image_pull_policy
                .clone()
                .filter(|policy| !policy.trim().is_empty()),
            image_pull_secrets,
            node_selector,
            cluster_domain: kubernetes.cluster_domain().to_string(),
            pod_wait_time: Duration::from_millis(
                kubernetes
                    .pod_wait_time
                    .map(|value| value.0)
                    .filter(|value| *value > 0)
                    .unwrap_or(60_000) as u64,
            ),
            internal_networking: kubernetes
                .internal_networking
                .map(|value| value.0)
                .unwrap_or(false),
            privileged: kubernetes.privileged.map(|value| value.0).unwrap_or(false),
            debug_manifests: kubernetes
                .debug_patches
                .map(|value| value.0)
                .unwrap_or(false),
            target_protocol,
            url: kubernetes.url.clone().filter(|url| !url.trim().is_empty()),
            cert_path: kubernetes
                .cert_path
                .clone()
                .filter(|path| !path.trim().is_empty()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_java_defaults() {
        let config = KubernetesConfig::from_settings(&Settings::default()).expect("config");
        assert_eq!(config.namespace, "default");
        assert_eq!(config.app_namespaces, vec!["default"]);
        assert_eq!(config.api_version, "v1");
        assert_eq!(config.cluster_domain, "cluster.local");
        assert_eq!(config.pod_wait_time, Duration::from_secs(60));
        assert!(config.image_pull_policy.is_none());
        assert!(config.image_pull_secrets.is_empty());
        assert!(config.node_selector.is_empty());
        assert!(!config.internal_networking);
        assert!(!config.privileged);
        assert!(!config.debug_manifests);
        assert_eq!(config.target_protocol, "http");
    }

    #[test]
    fn reads_the_configuration() {
        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  kubernetes:\n    namespace: shiny\n    app-namespaces: [ other, shiny ]\n    \
             api-version: v1\n    image-pull-policy: IfNotPresent\n    image-pull-secret: first\n    \
             image-pull-secrets: [ second, first ]\n    node-selector: disk=ssd, zone = a\n    \
             cluster-domain: my.cluster\n    pod-wait-time: 30000\n    internal-networking: true\n    \
             privileged: true\n    debug-patches: true\n    container-protocol: https\n    \
             url: https://api.cluster:6443\n    cert-path: /etc/certs\n",
        )
        .expect("settings");
        let config = KubernetesConfig::from_settings(&settings).expect("config");

        assert_eq!(config.namespace, "shiny");
        assert_eq!(config.app_namespaces, vec!["other", "shiny"]);
        assert_eq!(config.image_pull_policy.as_deref(), Some("IfNotPresent"));
        assert_eq!(config.image_pull_secrets, vec!["first", "second"]);
        assert_eq!(
            config.node_selector,
            BTreeMap::from([
                ("disk".to_string(), "ssd".to_string()),
                ("zone".to_string(), "a".to_string())
            ])
        );
        assert_eq!(config.cluster_domain, "my.cluster");
        assert_eq!(config.pod_wait_time, Duration::from_secs(30));
        assert!(config.internal_networking);
        assert!(config.privileged);
        assert!(config.debug_manifests);
        assert_eq!(config.target_protocol, "https");
        assert_eq!(config.url.as_deref(), Some("https://api.cluster:6443"));
        assert_eq!(config.cert_path.as_deref(), Some("/etc/certs"));
    }

    #[test]
    fn refuses_an_invalid_node_selector() {
        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  kubernetes:\n    node-selector: nonsense\n")
                .expect("settings");
        let error = KubernetesConfig::from_settings(&settings).unwrap_err();
        assert!(error.contains("node-selector"), "{error}");
    }
}
