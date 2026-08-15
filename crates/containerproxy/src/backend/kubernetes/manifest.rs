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

//! The manifests the Kubernetes backend creates.
//!
//! Everything in this module is a pure function of the configuration and the app definition, so the
//! manifests can be asserted in unit tests without a cluster — which is how the parity with the Java
//! implementation is checked (`proxy.kubernetes.debug-patches` prints the same documents there).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::config::KubernetesConfig;
use crate::backend::StartContext;
use crate::model::runtime_value::RuntimeValues;

/// Name of the headless service that gives the pods a stable DNS name.
pub const HEADLESS_SERVICE_NAME: &str = "sp-headless-service";

/// Label that selects the pods of the headless service.
pub const HEADLESS_SERVICE_LABEL: &str = "openanalytics.eu/sp-headless-service";

/// Annotation that decides what happens to an additional manifest that already exists.
pub const MANIFEST_POLICY_ANNOTATION: &str = "openanalytics.eu/sp-additional-manifest-policy";

/// Label of the resources that come from `kubernetes-additional-manifests`.
pub const ADDITIONAL_MANIFEST_LABEL: &str = "openanalytics.eu/sp-additional-manifest";

/// Label that marks a manifest as persistent (it survives the app).
pub const PERSISTENT_MANIFEST_LABEL: &str = "openanalytics.eu/sp-persistent-manifest";

/// Label that groups the manifests of one app of one user.
pub const MANIFEST_ID_LABEL: &str = "openanalytics.eu/sp-manifest-id";

/// Prefix of a value that references a key of a secret (`secretKeyRef:name:key`).
const SECRET_KEY_REF: &str = "secretkeyref";

/// The name of the pod of a container (`sp-pod-{proxyId}-{index}`, at most 63 characters).
pub fn pod_name(proxy_id: &str, index: i64, resource_name: Option<&str>) -> String {
    let name = match resource_name.filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => format!("sp-pod-{proxy_id}-{index}"),
    };
    name.chars().take(63).collect()
}

/// The name of the service of a container (`sp-service-{proxyId}-{index}`).
pub fn service_name(proxy_id: &str, index: i64) -> String {
    format!("sp-service-{proxy_id}-{index}")
}

/// The id that groups the additional manifests of one app of one user.
///
/// `KubernetesManifestsRemover.getManifestId` hashes the spec and the user, so that the label is a valid
/// label value for every user name.
pub fn manifest_id(spec_id: &str, user_id: &str) -> String {
    use sha1::Digest;
    let digest = sha1::Sha1::digest(format!("{spec_id}/{user_id}").as_bytes());
    hex::encode(digest)
}

/// Normalises a CPU quantity like `parseCpuQuantity` does (`500M` → `500m`).
pub fn cpu_quantity(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let normalised = match value.strip_suffix('M') {
        Some(number) => format!("{number}m"),
        None => value.to_string(),
    };
    // only a plain number or a number with the `m` suffix is a valid CPU quantity
    let number = normalised.strip_suffix('m').unwrap_or(&normalised);
    if number.is_empty() || number.parse::<f64>().is_err() {
        return Err(format!("Invalid format for CPU resources: {value}"));
    }
    Ok(Some(normalised))
}

/// Normalises a memory quantity like `parseMemoryQuantity` does (`2g` → `2G`, `512mi` → `512Mi`).
pub fn memory_quantity(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let mut normalised = value.to_string();
    for suffix in ["p", "t", "g", "m", "k"] {
        if let Some(number) = value.strip_suffix(suffix) {
            normalised = format!("{number}{}", suffix.to_uppercase());
            break;
        }
        if let Some(number) = value.strip_suffix(&format!("{suffix}i")) {
            normalised = format!("{number}{}i", suffix.to_uppercase());
            break;
        }
    }

    // the number in front of the suffix has to be a number
    let number: String = normalised
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    if number.is_empty() || number.parse::<f64>().is_err() {
        return Err(format!("Invalid format for memory resources: {value}"));
    }
    Ok(Some(normalised))
}

/// The environment of a container, with `secretKeyRef:name:key` values turned into a reference.
fn environment(context: &StartContext<'_>) -> Vec<Value> {
    let mut variables = Vec::new();
    for (name, value) in &context.environment {
        if value.to_lowercase().starts_with(SECRET_KEY_REF) {
            let parts: Vec<&str> = value.split(':').collect();
            if parts.len() != 3 {
                tracing::warn!(
                    "Invalid secret key reference: {name}={value}. Expected format: \
                     'secretKeyRef:<name>:<key>' [proxyId: {}]",
                    context.proxy.id
                );
                continue;
            }
            variables.push(json!({
                "name": name,
                "valueFrom": {"secretKeyRef": {"name": parts[1], "key": parts[2]}},
            }));
        } else {
            variables.push(json!({"name": name, "value": value}));
        }
    }
    variables
}

/// The labels and annotations of the runtime values of a proxy and its container.
///
/// Kubernetes gets the values with `include_as_label` as labels and those with `include_as_annotation` as
/// annotations (unlike Docker, which puts both in labels).
fn runtime_labels_and_annotations(
    proxy_values: &RuntimeValues,
    container_values: &RuntimeValues,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut labels = BTreeMap::new();
    let mut annotations = BTreeMap::new();
    for values in [proxy_values, container_values] {
        for value in values.iter() {
            if value.key.include_as_label {
                labels.insert(value.key.label.to_string(), value.to_value_string());
            }
            if value.key.include_as_annotation {
                annotations.insert(value.key.label.to_string(), value.to_value_string());
            }
        }
    }
    (labels, annotations)
}

/// Builds the pod of a container (`KubernetesBackend.startContainer`).
///
/// `container_id` is the identifier of the container inside ShinyProxy, which becomes the `app` label and
/// the selector of the service.
pub fn build_pod(
    config: &KubernetesConfig,
    context: &StartContext<'_>,
    container_id: &str,
) -> Result<Value, String> {
    let spec = context.container_spec;
    let name = pod_name(
        &context.proxy.id,
        context.container.index,
        spec.resource_name.as_str(),
    );

    // volumes are `hostPath:mountPath` pairs, named after their position
    let volume_strings: Vec<String> = spec.volumes.value().cloned().unwrap_or_default();
    let mut volumes = Vec::new();
    let mut volume_mounts = Vec::new();
    for (index, volume) in volume_strings.iter().enumerate() {
        let mut parts = volume.splitn(2, ':');
        let Some(host_path) = parts.next() else {
            continue;
        };
        let Some(mount_path) = parts.next() else {
            continue;
        };
        let volume_name = format!("shinyproxy-volume-{index}");
        volumes.push(json!({
            "name": volume_name,
            "hostPath": {"path": host_path, "type": ""},
        }));
        volume_mounts.push(json!({"name": volume_name, "mountPath": mount_path}));
    }

    let mut requests = BTreeMap::new();
    let mut limits = BTreeMap::new();
    if let Some(cpu) = cpu_quantity(spec.cpu_request.as_str())
        .map_err(|error| format!("Invalid container-cpu-request: {error}"))?
    {
        requests.insert("cpu".to_string(), cpu);
    }
    if let Some(cpu) = cpu_quantity(spec.cpu_limit.as_str())
        .map_err(|error| format!("Invalid container-cpu-limit: {error}"))?
    {
        limits.insert("cpu".to_string(), cpu);
    }
    if let Some(memory) = memory_quantity(spec.memory_request.as_str())
        .map_err(|error| format!("Invalid container-memory-request: {error}"))?
    {
        requests.insert("memory".to_string(), memory);
    }
    if let Some(memory) = memory_quantity(spec.memory_limit.as_str())
        .map_err(|error| format!("Invalid container-memory-limit: {error}"))?
    {
        limits.insert("memory".to_string(), memory);
    }
    let mut resources = serde_json::Map::new();
    if !requests.is_empty() {
        resources.insert("requests".to_string(), json!(requests));
    }
    if !limits.is_empty() {
        resources.insert("limits".to_string(), json!(limits));
    }

    let ports: Vec<Value> = spec
        .port_mapping
        .iter()
        .filter_map(|mapping| mapping.port)
        .map(|port| json!({"containerPort": port}))
        .collect();

    let mut container = serde_json::Map::new();
    container.insert(
        "name".to_string(),
        json!(format!("sp-container-{}", context.container.index)),
    );
    container.insert(
        "image".to_string(),
        json!(spec.image.as_str().unwrap_or_default()),
    );
    if let Some(command) = spec.cmd.value().filter(|command| !command.is_empty()) {
        container.insert("command".to_string(), json!(command));
    }
    container.insert("ports".to_string(), json!(ports));
    container.insert("volumeMounts".to_string(), json!(volume_mounts));
    container.insert(
        "securityContext".to_string(),
        json!({"privileged": config.privileged || spec.privileged}),
    );
    container.insert("resources".to_string(), Value::Object(resources));
    container.insert("env".to_string(), json!(environment(context)));
    container.insert(
        "terminationMessagePolicy".to_string(),
        json!("FallbackToLogsOnError"),
    );
    if let Some(policy) = &config.image_pull_policy {
        container.insert("imagePullPolicy".to_string(), json!(policy));
    }

    // the labels and annotations of the runtime values, plus the labels of the app definition
    let (mut labels, annotations) = runtime_labels_and_annotations(
        &context.proxy.runtime_values,
        &context.container.runtime_values,
    );
    labels.insert("app".to_string(), container_id.to_string());
    labels.insert(HEADLESS_SERVICE_LABEL.to_string(), "true".to_string());
    if let Some(configured) = spec.labels.value() {
        for (key, value) in configured {
            labels.insert(key.clone(), value.clone());
        }
    }

    let mut pod_spec = serde_json::Map::new();
    pod_spec.insert("containers".to_string(), json!([Value::Object(container)]));
    if let Some(dns) = spec.dns.value().filter(|servers| !servers.is_empty()) {
        pod_spec.insert("dnsConfig".to_string(), json!({"nameservers": dns}));
    }
    pod_spec.insert("volumes".to_string(), json!(volumes));
    pod_spec.insert(
        "imagePullSecrets".to_string(),
        json!(config
            .image_pull_secrets
            .iter()
            .map(|secret| json!({"name": secret}))
            .collect::<Vec<_>>()),
    );
    pod_spec.insert("restartPolicy".to_string(), json!("Never"));
    pod_spec.insert("hostname".to_string(), json!(name));
    pod_spec.insert("subdomain".to_string(), json!(HEADLESS_SERVICE_NAME));
    if !config.node_selector.is_empty() {
        pod_spec.insert("nodeSelector".to_string(), json!(config.node_selector));
    }

    Ok(json!({
        "apiVersion": config.api_version,
        "kind": "Pod",
        "metadata": {
            "name": name,
            "namespace": config.namespace,
            "labels": labels,
            "annotations": annotations,
        },
        "spec": Value::Object(pod_spec),
    }))
}

/// Builds the `NodePort` service of a container (used when ShinyProxy runs outside the cluster).
pub fn build_service(
    config: &KubernetesConfig,
    context: &StartContext<'_>,
    container_id: &str,
    namespace: &str,
) -> Value {
    let (labels, _) = runtime_labels_and_annotations(
        &context.proxy.runtime_values,
        &context.container.runtime_values,
    );
    let ports: Vec<Value> = context
        .container_spec
        .port_mapping
        .iter()
        .filter_map(|mapping| mapping.port)
        .map(|port| json!({"port": port}))
        .collect();

    json!({
        "apiVersion": config.api_version,
        "kind": "Service",
        "metadata": {
            "name": service_name(&context.proxy.id, context.container.index),
            "namespace": namespace,
            "labels": labels,
        },
        "spec": {
            "selector": {"app": container_id},
            "type": "NodePort",
            "ports": ports,
        },
    })
}

/// Builds the headless service that gives the pods their DNS names.
pub fn build_headless_service(config: &KubernetesConfig, namespace: &str) -> Value {
    json!({
        "apiVersion": config.api_version,
        "kind": "Service",
        "metadata": {"name": HEADLESS_SERVICE_NAME, "namespace": namespace},
        "spec": {
            "selector": {HEADLESS_SERVICE_LABEL: "true"},
            "clusterIP": "None",
        },
    })
}

/// The fully qualified name of a pod inside the cluster.
pub fn pod_fqdn(hostname: &str, namespace: &str, cluster_domain: &str) -> String {
    format!("{hostname}.{HEADLESS_SERVICE_NAME}.{namespace}.svc.{cluster_domain}")
}

/// Applies a JSON patch (`kubernetes-pod-patches`, written in YAML) to a document.
pub fn apply_patch(document: &Value, patch_yaml: Option<&str>) -> Result<Value, String> {
    let Some(patch_yaml) = patch_yaml.map(str::trim).filter(|patch| !patch.is_empty()) else {
        return Ok(document.clone());
    };
    // the patches are written in YAML in the configuration, and are JSON patches (RFC 6902)
    let patch_value: Value = serde_yaml_ng::from_str(patch_yaml)
        .map_err(|error| format!("cannot read the pod patch: {error}"))?;
    let patch: json_patch::Patch = serde_json::from_value(patch_value)
        .map_err(|error| format!("cannot read the pod patch: {error}"))?;

    let mut patched = document.clone();
    json_patch::patch(&mut patched, &patch)
        .map_err(|error| format!("cannot apply the pod patch: {error}"))?;
    Ok(patched)
}

/// Prepares an additional manifest: fills in the namespace and adds the labels of ShinyProxy.
pub fn prepare_additional_manifest(
    manifest_yaml: &str,
    namespace: &str,
    persistent: bool,
    manifest_id: &str,
) -> Result<Value, String> {
    let mut manifest: Value = serde_yaml_ng::from_str(manifest_yaml)
        .map_err(|error| format!("cannot read the additional manifest: {error}"))?;
    if !manifest.is_object() {
        return Err("an additional manifest must be a YAML document".to_string());
    }

    let metadata = manifest
        .as_object_mut()
        .expect("object")
        .entry("metadata")
        .or_insert_with(|| json!({}));
    let metadata = metadata
        .as_object_mut()
        .ok_or_else(|| "the metadata of an additional manifest must be an object".to_string())?;
    if !metadata.contains_key("namespace") {
        metadata.insert("namespace".to_string(), json!(namespace));
    }
    let labels = metadata
        .entry("labels")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "the labels of an additional manifest must be an object".to_string())?;
    labels.insert(ADDITIONAL_MANIFEST_LABEL.to_string(), json!("true"));
    labels.insert(
        PERSISTENT_MANIFEST_LABEL.to_string(),
        json!(persistent.to_string()),
    );
    labels.insert(MANIFEST_ID_LABEL.to_string(), json!(manifest_id));

    Ok(manifest)
}

/// What happens to an additional manifest that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestPolicy {
    /// Create it when it does not exist (the default).
    CreateOnce,
    /// Create it, or patch the existing resource.
    Patch,
    /// Delete it when it exists.
    Delete,
    /// Delete and create it again.
    Replace,
}

impl ManifestPolicy {
    /// The policy of a manifest, from its annotation.
    pub fn of(manifest: &Value) -> Result<Self, String> {
        let annotation = manifest
            .get("metadata")
            .and_then(|metadata| metadata.get("annotations"))
            .and_then(|annotations| annotations.get(MANIFEST_POLICY_ANNOTATION))
            .and_then(Value::as_str)
            .unwrap_or("CreateOnce");
        match annotation.to_ascii_lowercase().as_str() {
            "createonce" => Ok(ManifestPolicy::CreateOnce),
            "patch" => Ok(ManifestPolicy::Patch),
            "delete" => Ok(ManifestPolicy::Delete),
            "replace" => Ok(ManifestPolicy::Replace),
            other => Err(format!("Unknown manifest-policy: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Container, Proxy, ProxyStatus};
    use crate::model::runtime_value::{RuntimeValue, INSTANCE_ID, PROXIED_APP, PROXY_ID, USER_ID};
    use crate::model::spec::{ContainerSpec, PortMapping, ProxySpec};
    use crate::model::spel_field::{SpelString, SpelStringList, SpelStringMap};

    fn config() -> KubernetesConfig {
        KubernetesConfig::from_settings(&crate::config::Settings::default()).expect("config")
    }

    fn container_spec() -> ContainerSpec {
        ContainerSpec {
            image: SpelString::resolved("x".into(), "openanalytics/shinyproxy-demo".into()),
            cmd: SpelStringList::resolved(
                vec!["R".into(), "-e".into()],
                vec!["R".into(), "-e".into()],
            ),
            port_mapping: vec![PortMapping {
                name: "default".to_string(),
                port: Some(3838),
                target_path: SpelString::resolved(String::new(), String::new()),
            }],
            memory_request: SpelString::resolved("1g".into(), "1g".into()),
            memory_limit: SpelString::resolved("2Gi".into(), "2Gi".into()),
            cpu_request: SpelString::resolved("500M".into(), "500M".into()),
            cpu_limit: SpelString::resolved("2".into(), "2".into()),
            volumes: SpelStringList::resolved(
                vec!["/host/data:/data".into()],
                vec!["/host/data:/data".into()],
            ),
            dns: SpelStringList::resolved(vec!["8.8.8.8".into()], vec!["8.8.8.8".into()]),
            labels: SpelStringMap::resolved(
                BTreeMap::new(),
                BTreeMap::from([("my.label".to_string(), "value".to_string())]),
            ),
            ..Default::default()
        }
    }

    fn proxy() -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::New);
        proxy.spec_id = Some("01_hello".to_string());
        proxy.user_id = Some("jack".to_string());
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_ID, "proxy-1"), true);
        proxy.add_runtime_value(RuntimeValue::string(&USER_ID, "jack"), true);
        proxy.add_runtime_value(RuntimeValue::string(&INSTANCE_ID, "instance-1"), true);
        proxy.add_runtime_value(RuntimeValue::string(&PROXIED_APP, "true"), true);
        proxy
    }

    fn context<'a>(
        proxy: &'a Proxy,
        spec: &'a ProxySpec,
        container_spec: &'a ContainerSpec,
        container: &'a Container,
    ) -> StartContext<'a> {
        StartContext {
            user: None,
            proxy,
            spec,
            container_spec,
            container,
            environment: BTreeMap::from([
                ("SHINYPROXY_USERNAME".to_string(), "jack".to_string()),
                (
                    "DB_PASSWORD".to_string(),
                    "secretKeyRef:my-secret:password".to_string(),
                ),
            ]),
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn builds_the_pod_like_java() {
        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let pod = build_pod(&config(), &context, "container-id").expect("pod");

        assert_eq!(pod["apiVersion"], "v1");
        assert_eq!(pod["kind"], "Pod");
        assert_eq!(pod["metadata"]["name"], "sp-pod-proxy-1-0");
        assert_eq!(pod["metadata"]["namespace"], "default");
        assert_eq!(pod["metadata"]["labels"]["app"], "container-id");
        assert_eq!(
            pod["metadata"]["labels"]["openanalytics.eu/sp-headless-service"],
            "true"
        );
        assert_eq!(pod["metadata"]["labels"]["my.label"], "value");
        // label backed runtime values become labels, the others annotations
        assert_eq!(
            pod["metadata"]["labels"]["openanalytics.eu/sp-proxied-app"],
            "true"
        );
        assert_eq!(
            pod["metadata"]["labels"]["openanalytics.eu/sp-instance"],
            "instance-1"
        );
        assert_eq!(
            pod["metadata"]["annotations"]["openanalytics.eu/sp-proxy-id"],
            "proxy-1"
        );
        assert_eq!(
            pod["metadata"]["annotations"]["openanalytics.eu/sp-user-id"],
            "jack"
        );
        assert!(pod["metadata"]["labels"]
            .get("openanalytics.eu/sp-proxy-id")
            .is_none());

        let pod_spec = &pod["spec"];
        assert_eq!(pod_spec["restartPolicy"], "Never");
        assert_eq!(pod_spec["hostname"], "sp-pod-proxy-1-0");
        assert_eq!(pod_spec["subdomain"], "sp-headless-service");
        assert_eq!(pod_spec["dnsConfig"]["nameservers"][0], "8.8.8.8");
        assert_eq!(pod_spec["volumes"][0]["name"], "shinyproxy-volume-0");
        assert_eq!(pod_spec["volumes"][0]["hostPath"]["path"], "/host/data");

        let container = &pod_spec["containers"][0];
        assert_eq!(container["name"], "sp-container-0");
        assert_eq!(container["image"], "openanalytics/shinyproxy-demo");
        assert_eq!(container["command"][0], "R");
        assert_eq!(container["ports"][0]["containerPort"], 3838);
        assert_eq!(container["volumeMounts"][0]["mountPath"], "/data");
        assert_eq!(container["securityContext"]["privileged"], false);
        assert_eq!(
            container["terminationMessagePolicy"],
            "FallbackToLogsOnError"
        );
        // the quantities are normalised like Java does
        assert_eq!(container["resources"]["requests"]["cpu"], "500m");
        assert_eq!(container["resources"]["limits"]["cpu"], "2");
        assert_eq!(container["resources"]["requests"]["memory"], "1G");
        assert_eq!(container["resources"]["limits"]["memory"], "2Gi");

        // the environment, with the secret reference
        let environment = container["env"].as_array().expect("env");
        let username = environment
            .iter()
            .find(|variable| variable["name"] == "SHINYPROXY_USERNAME")
            .expect("username");
        assert_eq!(username["value"], "jack");
        let password = environment
            .iter()
            .find(|variable| variable["name"] == "DB_PASSWORD")
            .expect("password");
        assert_eq!(
            password["valueFrom"]["secretKeyRef"],
            json!({"name": "my-secret", "key": "password"})
        );
        assert!(password.get("value").is_none());
    }

    #[test]
    fn uses_the_configured_namespace_and_pull_settings() {
        let settings: crate::config::Settings = serde_yaml_ng::from_str(
            "proxy:\n  container-backend: kubernetes\n  kubernetes:\n    namespace: shiny\n    \
             image-pull-policy: Always\n    image-pull-secret: my-secret\n    \
             image-pull-secrets: [ other-secret ]\n    node-selector: disk=ssd,zone=a\n    \
             privileged: true\n    api-version: v1\n",
        )
        .expect("settings");
        let config = KubernetesConfig::from_settings(&settings).expect("config");

        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let pod = build_pod(&config, &context, "container-id").expect("pod");
        assert_eq!(pod["metadata"]["namespace"], "shiny");
        assert_eq!(pod["spec"]["containers"][0]["imagePullPolicy"], "Always");
        assert_eq!(
            pod["spec"]["containers"][0]["securityContext"]["privileged"],
            true
        );
        assert_eq!(
            pod["spec"]["imagePullSecrets"],
            json!([{"name": "my-secret"}, {"name": "other-secret"}])
        );
        assert_eq!(
            pod["spec"]["nodeSelector"],
            json!({"disk": "ssd", "zone": "a"})
        );
    }

    #[test]
    fn builds_the_node_port_service() {
        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let service = build_service(&config(), &context, "container-id", "shiny");
        assert_eq!(service["kind"], "Service");
        assert_eq!(service["metadata"]["name"], "sp-service-proxy-1-0");
        assert_eq!(service["metadata"]["namespace"], "shiny");
        assert_eq!(
            service["metadata"]["labels"]["openanalytics.eu/sp-instance"],
            "instance-1"
        );
        assert_eq!(service["spec"]["type"], "NodePort");
        assert_eq!(service["spec"]["selector"]["app"], "container-id");
        assert_eq!(service["spec"]["ports"][0]["port"], 3838);

        let headless = build_headless_service(&config(), "shiny");
        assert_eq!(headless["metadata"]["name"], "sp-headless-service");
        assert_eq!(headless["spec"]["clusterIP"], "None");
        assert_eq!(
            headless["spec"]["selector"]["openanalytics.eu/sp-headless-service"],
            "true"
        );
    }

    #[test]
    fn names_resources_like_java() {
        assert_eq!(pod_name("abc", 0, None), "sp-pod-abc-0");
        assert_eq!(pod_name("abc", 1, Some("my-pod")), "my-pod");
        assert_eq!(
            pod_name(&"x".repeat(80), 0, None).len(),
            63,
            "the name is truncated to 63 characters"
        );
        assert_eq!(service_name("abc", 2), "sp-service-abc-2");
        assert_eq!(
            pod_fqdn("sp-pod-abc-0", "shiny", "cluster.local"),
            "sp-pod-abc-0.sp-headless-service.shiny.svc.cluster.local"
        );
        // the manifest id is stable and a valid label value
        let id = manifest_id("01_hello", "jack");
        assert_eq!(id, manifest_id("01_hello", "jack"));
        assert_ne!(id, manifest_id("01_hello", "jeff"));
        assert!(id
            .chars()
            .all(|character| character.is_ascii_alphanumeric()));
    }

    #[test]
    fn normalises_quantities_like_java() {
        assert_eq!(cpu_quantity(None).unwrap(), None);
        assert_eq!(cpu_quantity(Some("2")).unwrap().as_deref(), Some("2"));
        assert_eq!(cpu_quantity(Some("500m")).unwrap().as_deref(), Some("500m"));
        assert_eq!(cpu_quantity(Some("500M")).unwrap().as_deref(), Some("500m"));
        assert!(cpu_quantity(Some("2Gi")).is_err());

        assert_eq!(memory_quantity(None).unwrap(), None);
        assert_eq!(memory_quantity(Some("2g")).unwrap().as_deref(), Some("2G"));
        assert_eq!(
            memory_quantity(Some("512m")).unwrap().as_deref(),
            Some("512M")
        );
        assert_eq!(
            memory_quantity(Some("512mi")).unwrap().as_deref(),
            Some("512Mi")
        );
        assert_eq!(
            memory_quantity(Some("2Gi")).unwrap().as_deref(),
            Some("2Gi")
        );
        assert_eq!(
            memory_quantity(Some("1024")).unwrap().as_deref(),
            Some("1024")
        );
        assert!(memory_quantity(Some("lots")).is_err());
    }

    #[test]
    fn applies_pod_patches() {
        let pod = json!({
            "metadata": {"name": "sp-pod-1", "namespace": "default"},
            "spec": {"containers": [{"name": "sp-container-0", "env": []}]}
        });

        // no patch leaves the document alone
        assert_eq!(apply_patch(&pod, None).unwrap(), pod);
        assert_eq!(apply_patch(&pod, Some("  ")).unwrap(), pod);

        // the patches of the ShinyProxy documentation
        let patched = apply_patch(
            &pod,
            Some(
                "- op: add\n  path: /spec/containers/0/env/-\n  value:\n    name: CUSTOM\n    \
                 value: my-value\n- op: replace\n  path: /metadata/namespace\n  value: other\n",
            ),
        )
        .expect("patched");
        assert_eq!(patched["metadata"]["namespace"], "other");
        assert_eq!(patched["spec"]["containers"][0]["env"][0]["name"], "CUSTOM");

        // an invalid patch is an error, not a silent no-op
        assert!(apply_patch(&pod, Some("- op: nonsense\n  path: /x\n")).is_err());
        assert!(apply_patch(&pod, Some("- op: replace\n  path: /nope\n  value: 1\n")).is_err());
    }

    #[test]
    fn prepares_additional_manifests() {
        let manifest = prepare_additional_manifest(
            "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: home-dir\nspec:\n  \
             accessModes: [ ReadWriteOnce ]\n",
            "shiny",
            true,
            "the-id",
        )
        .expect("manifest");

        assert_eq!(manifest["kind"], "PersistentVolumeClaim");
        assert_eq!(manifest["metadata"]["namespace"], "shiny");
        assert_eq!(
            manifest["metadata"]["labels"]["openanalytics.eu/sp-additional-manifest"],
            "true"
        );
        assert_eq!(
            manifest["metadata"]["labels"]["openanalytics.eu/sp-persistent-manifest"],
            "true"
        );
        assert_eq!(
            manifest["metadata"]["labels"]["openanalytics.eu/sp-manifest-id"],
            "the-id"
        );

        // a manifest with its own namespace keeps it
        let manifest = prepare_additional_manifest(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: s\n  namespace: other\n",
            "shiny",
            false,
            "the-id",
        )
        .expect("manifest");
        assert_eq!(manifest["metadata"]["namespace"], "other");
        assert_eq!(
            manifest["metadata"]["labels"]["openanalytics.eu/sp-persistent-manifest"],
            "false"
        );
    }

    #[test]
    fn reads_the_manifest_policy() {
        let manifest = json!({"metadata": {"name": "x"}});
        assert_eq!(
            ManifestPolicy::of(&manifest).unwrap(),
            ManifestPolicy::CreateOnce
        );

        for (annotation, expected) in [
            ("CreateOnce", ManifestPolicy::CreateOnce),
            ("Patch", ManifestPolicy::Patch),
            ("delete", ManifestPolicy::Delete),
            ("Replace", ManifestPolicy::Replace),
        ] {
            let manifest = json!({
                "metadata": {
                    "annotations": {MANIFEST_POLICY_ANNOTATION: annotation}
                }
            });
            assert_eq!(ManifestPolicy::of(&manifest).unwrap(), expected);
        }

        let manifest = json!({
            "metadata": {"annotations": {MANIFEST_POLICY_ANNOTATION: "Nonsense"}}
        });
        assert!(ManifestPolicy::of(&manifest)
            .unwrap_err()
            .contains("Unknown manifest-policy"));
    }
}
