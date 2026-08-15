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

//! The `kubernetes` container backend: apps run as pods.
//!
//! Port of `KubernetesBackend`. One pod per container, named `sp-pod-{proxyId}-{index}`, with the labels
//! and annotations of the runtime values, and — when ShinyProxy runs outside the cluster — a `NodePort`
//! service named `sp-service-{proxyId}-{index}` that publishes the ports. Inside the cluster the pods are
//! reached directly through the `sp-headless-service` DNS name.
//!
//! The manifests are built by [`manifest`], which is pure and therefore covered by unit tests; this module
//! talks to the API server.

pub mod config;
pub mod manifest;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams};
use kube::core::{DynamicObject, GroupVersionKind};
use kube::discovery::ApiResource;
use kube::{Client, Resource, ResourceExt};
use serde_json::Value;

pub use config::KubernetesConfig;

use super::target::{compute_target_path, mapping_key_to_path, target_url};
use super::{
    BackendError, ContainerBackend, ExistingContainerInfo, LogChunk, LogStream, StartContext,
    StartedContainer,
};
use crate::model::proxy::Proxy;
use crate::model::runtime_value::{
    BackendContainerName, RuntimeValue, RuntimeValueRegistry, RuntimeValues,
    BACKEND_CONTAINER_NAME, CONTAINER_IMAGE, PROXIED_APP,
};
use crate::model::spec::ProxySpec;

/// Name of the backend.
pub const NAME: &str = "kubernetes";

/// The Kubernetes fields of an app definition (`KubernetesSpecExtension`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KubernetesSpecExtension {
    /// JSON patch (written in YAML) applied to the pod.
    pub pod_patches: Option<String>,
    /// Patches that are only applied to users that pass their access control.
    pub authorized_pod_patches: Vec<AuthorizedPatches>,
    /// Manifests created next to the pod, removed when the app stops.
    pub additional_manifests: Vec<String>,
    /// Manifests created next to the pod that survive the app.
    pub additional_persistent_manifests: Vec<String>,
    /// Manifests that are only created for users that pass their access control.
    pub authorized_additional_manifests: Vec<AuthorizedManifests>,
    pub authorized_additional_persistent_manifests: Vec<AuthorizedManifests>,
}

/// Pod patches behind an access control.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AuthorizedPatches {
    /// Who may use these patches.
    pub access_control: Option<crate::model::spec::AccessControl>,
    /// The patches themselves.
    pub patches: Option<String>,
}

/// Additional manifests behind an access control.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AuthorizedManifests {
    /// Who gets these manifests.
    pub access_control: Option<crate::model::spec::AccessControl>,
    /// The manifests themselves.
    pub manifests: Vec<String>,
}

impl KubernetesSpecExtension {
    /// The Kubernetes fields of an app definition.
    pub fn of(spec: &ProxySpec) -> Self {
        spec.spec_extensions.get("kubernetes")
    }
}

/// Decides whether a user may use an authorized patch or manifest.
pub type AccessCheck =
    Arc<dyn Fn(Option<&crate::model::spec::AccessControl>) -> bool + Send + Sync>;

/// Runs apps as pods in a Kubernetes cluster.
pub struct KubernetesBackend {
    client: Client,
    config: KubernetesConfig,
    registry: Arc<RuntimeValueRegistry>,
    /// Evaluates the access control of authorized patches and manifests.
    access_check: AccessCheck,
}

impl std::fmt::Debug for KubernetesBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KubernetesBackend")
            .field("config", &self.config)
            .finish()
    }
}

impl KubernetesBackend {
    /// Connects to the cluster.
    ///
    /// The connection is made the way the Java client does it: `proxy.kubernetes.url` wins, then the
    /// in-cluster service account, then the kubeconfig of the user.
    pub async fn connect(
        config: KubernetesConfig,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Result<Self, BackendError> {
        let client_config = match &config.url {
            Some(url) => {
                let mut client_config = kube::Config::new(url.parse().map_err(|error| {
                    BackendError::Backend(format!("invalid proxy.kubernetes.url '{url}': {error}"))
                })?);
                if let Some(cert_path) = &config.cert_path {
                    let directory = std::path::Path::new(cert_path);
                    client_config.root_cert = std::fs::read(directory.join("ca.pem"))
                        .ok()
                        .map(|certificate| vec![certificate]);
                    if let (Ok(certificate), Ok(key)) = (
                        std::fs::read(directory.join("cert.pem")),
                        std::fs::read(directory.join("key.pem")),
                    ) {
                        client_config.auth_info.client_certificate_data =
                            Some(base64_encode(&certificate));
                        client_config.auth_info.client_key_data = Some(base64_encode(&key).into());
                    }
                }
                client_config
            }
            None => kube::Config::infer().await.map_err(|error| {
                BackendError::Backend(format!("cannot read the Kubernetes configuration: {error}"))
            })?,
        };

        let client = Client::try_from(client_config).map_err(|error| {
            BackendError::Backend(format!("cannot connect to Kubernetes: {error}"))
        })?;
        Ok(KubernetesBackend {
            client,
            config,
            registry,
            access_check: Arc::new(|_| true),
        })
    }

    /// Creates a backend around an existing client (used by tests).
    pub fn with_client(
        client: Client,
        config: KubernetesConfig,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Self {
        KubernetesBackend {
            client,
            config,
            registry,
            access_check: Arc::new(|_| true),
        }
    }

    /// Uses the given closure to evaluate the access control of authorized patches and manifests.
    pub fn with_access_check(mut self, access_check: AccessCheck) -> Self {
        self.access_check = access_check;
        self
    }

    /// The configuration of this backend.
    pub fn config(&self) -> &KubernetesConfig {
        &self.config
    }

    /// The pod of a container, when it has one.
    async fn pod_of(&self, container: &crate::model::proxy::Container) -> Option<Pod> {
        let name = backend_container_name(container)?;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &name.namespace);
        pods.get_opt(&name.name).await.ok().flatten()
    }

    /// The patched pod of a container.
    fn patched_pod(
        &self,
        context: &StartContext<'_>,
        container_id: &str,
    ) -> Result<Value, BackendError> {
        let extension = KubernetesSpecExtension::of(context.spec);
        let pod = manifest::build_pod(&self.config, context, container_id)
            .map_err(BackendError::FailedToStart)?;

        let mut patched = manifest::apply_patch(&pod, extension.pod_patches.as_deref())
            .map_err(BackendError::FailedToStart)?;
        for authorized in &extension.authorized_pod_patches {
            if (self.access_check)(authorized.access_control.as_ref()) {
                patched = manifest::apply_patch(&patched, authorized.patches.as_deref())
                    .map_err(BackendError::FailedToStart)?;
            }
        }

        if self.config.debug_manifests {
            tracing::info!(
                "Creating pod [proxyId: {}]:\n{}",
                context.proxy.id,
                serde_yaml_ng::to_string(&patched).unwrap_or_default()
            );
        }
        Ok(patched)
    }

    /// Creates the additional manifests of an app definition.
    async fn create_additional_manifests(
        &self,
        context: &StartContext<'_>,
        namespace: &str,
    ) -> Result<(), BackendError> {
        let extension = KubernetesSpecExtension::of(context.spec);
        let manifest_id = manifest::manifest_id(
            context.proxy.spec_id.as_deref().unwrap_or_default(),
            context.proxy.user_id.as_deref().unwrap_or_default(),
        );

        let mut manifests: Vec<(String, bool)> = Vec::new();
        for document in &extension.additional_manifests {
            manifests.push((document.clone(), false));
        }
        for authorized in &extension.authorized_additional_manifests {
            if (self.access_check)(authorized.access_control.as_ref()) {
                for document in &authorized.manifests {
                    manifests.push((document.clone(), false));
                }
            }
        }
        for document in &extension.additional_persistent_manifests {
            manifests.push((document.clone(), true));
        }
        for authorized in &extension.authorized_additional_persistent_manifests {
            if (self.access_check)(authorized.access_control.as_ref()) {
                for document in &authorized.manifests {
                    manifests.push((document.clone(), true));
                }
            }
        }

        for (document, persistent) in manifests {
            let prepared = manifest::prepare_additional_manifest(
                &document,
                namespace,
                persistent,
                &manifest_id,
            )
            .map_err(BackendError::FailedToStart)?;
            if self.config.debug_manifests {
                tracing::info!(
                    "Creating additional manifest [proxyId: {}]:\n{}",
                    context.proxy.id,
                    serde_yaml_ng::to_string(&prepared).unwrap_or_default()
                );
            }
            self.apply_additional_manifest(&prepared).await?;
        }
        Ok(())
    }

    /// Applies one additional manifest, following its manifest policy.
    async fn apply_additional_manifest(&self, document: &Value) -> Result<(), BackendError> {
        let policy = manifest::ManifestPolicy::of(document).map_err(BackendError::FailedToStart)?;
        let (api, name) = self.dynamic_api(document)?;
        let object: DynamicObject = serde_json::from_value(document.clone())
            .map_err(|error| BackendError::FailedToStart(format!("invalid manifest: {error}")))?;

        let exists = api
            .get_opt(&name)
            .await
            .map_err(|error| BackendError::Backend(format!("cannot read {name}: {error}")))?
            .is_some();

        match policy {
            manifest::ManifestPolicy::CreateOnce => {
                if !exists {
                    api.create(&PostParams::default(), &object)
                        .await
                        .map_err(|error| {
                            BackendError::FailedToStart(format!("cannot create {name}: {error}"))
                        })?;
                }
            }
            manifest::ManifestPolicy::Patch => {
                if exists {
                    api.patch(&name, &PatchParams::default(), &Patch::Merge(document))
                        .await
                        .map_err(|error| {
                            BackendError::FailedToStart(format!("cannot patch {name}: {error}"))
                        })?;
                } else {
                    api.create(&PostParams::default(), &object)
                        .await
                        .map_err(|error| {
                            BackendError::FailedToStart(format!("cannot create {name}: {error}"))
                        })?;
                }
            }
            manifest::ManifestPolicy::Delete => {
                if exists {
                    let _ = api
                        .delete(&name, &DeleteParams::default().grace_period(0))
                        .await;
                }
            }
            manifest::ManifestPolicy::Replace => {
                if exists {
                    let _ = api
                        .delete(&name, &DeleteParams::default().grace_period(0))
                        .await;
                }
                api.create(&PostParams::default(), &object)
                    .await
                    .map_err(|error| {
                        BackendError::FailedToStart(format!("cannot create {name}: {error}"))
                    })?;
            }
        }
        Ok(())
    }

    /// The dynamic API and the name of a manifest.
    fn dynamic_api(&self, document: &Value) -> Result<(Api<DynamicObject>, String), BackendError> {
        let api_version = document
            .get("apiVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BackendError::FailedToStart("a manifest needs an apiVersion".to_string())
            })?;
        let kind = document
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| BackendError::FailedToStart("a manifest needs a kind".to_string()))?;
        let name = document
            .get("metadata")
            .and_then(|metadata| metadata.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BackendError::FailedToStart("a manifest needs a metadata.name".to_string())
            })?
            .to_string();
        let namespace = document
            .get("metadata")
            .and_then(|metadata| metadata.get("namespace"))
            .and_then(Value::as_str)
            .unwrap_or(&self.config.namespace)
            .to_string();

        let gvk = parse_api_version(api_version, kind);
        let resource = ApiResource::from_gvk(&gvk);
        Ok((
            Api::namespaced_with(self.client.clone(), &namespace, &resource),
            name,
        ))
    }

    /// Creates the headless service that gives the pods their DNS names.
    async fn ensure_headless_service(&self, namespace: &str) -> Result<(), BackendError> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), namespace);
        if services
            .get_opt(manifest::HEADLESS_SERVICE_NAME)
            .await
            .map_err(|error| {
                BackendError::Backend(format!("cannot read the headless service: {error}"))
            })?
            .is_some()
        {
            return Ok(());
        }
        let document = manifest::build_headless_service(&self.config, namespace);
        let service: Service = serde_json::from_value(document).map_err(|error| {
            BackendError::FailedToStart(format!("invalid headless service: {error}"))
        })?;
        services
            .create(&PostParams::default(), &service)
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot create the headless service: {error}"))
            })?;
        Ok(())
    }

    /// The port bindings of a service (container port to node port).
    fn port_bindings(service: &Service) -> BTreeMap<i64, u16> {
        service
            .spec
            .as_ref()
            .and_then(|spec| spec.ports.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|port| {
                port.node_port
                    .and_then(|node_port| u16::try_from(node_port).ok())
                    .map(|node_port| (port.port as i64, node_port))
            })
            .collect()
    }

    /// The failure of a pod, when it has one (`getContainerFailure`).
    fn pod_failure(pod: Option<&Pod>) -> Option<String> {
        let Some(pod) = pod else {
            return Some("Kubernetes container failed, pod does not exist".to_string());
        };
        let status = pod.status.as_ref();
        let node = pod
            .spec
            .as_ref()
            .and_then(|spec| spec.node_name.clone())
            .unwrap_or_default();

        let container_status = status
            .and_then(|status| status.container_statuses.as_ref())
            .and_then(|statuses| statuses.first());
        let state = container_status
            .and_then(|status| status.state.clone().or_else(|| status.last_state.clone()));
        if let Some(terminated) = state.and_then(|state| state.terminated) {
            let message = status
                .and_then(|status| status.message.clone())
                .unwrap_or_default();
            return Some(format!(
                "Kubernetes pod failed, reason: '{}', exitCode: '{}', node: '{node}', message:\n{message}\nlogs:\n{}\n",
                terminated.reason.unwrap_or_default(),
                terminated.exit_code,
                terminated.message.unwrap_or_default(),
            ));
        }
        if pod.meta().deletion_timestamp.is_some() {
            return Some(format!(
                "Kubernetes pod is being terminated, node: '{node}'"
            ));
        }
        None
    }

    /// Whether a pod is ready (every container ready and the pod in phase `Running`).
    fn is_ready(pod: &Pod) -> bool {
        let Some(status) = &pod.status else {
            return false;
        };
        if status.phase.as_deref() != Some("Running") {
            return false;
        }
        status
            .conditions
            .as_ref()
            .map(|conditions| {
                conditions
                    .iter()
                    .any(|condition| condition.type_ == "Ready" && condition.status == "True")
            })
            .unwrap_or(false)
    }
}

/// Encodes bytes for a kubeconfig field.
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Splits an `apiVersion` into the group and the version of a `GroupVersionKind`.
fn parse_api_version(api_version: &str, kind: &str) -> GroupVersionKind {
    match api_version.split_once('/') {
        Some((group, version)) => GroupVersionKind::gvk(group, version, kind),
        None => GroupVersionKind::gvk("", api_version, kind),
    }
}

/// The pod of a container, from its runtime values.
fn backend_container_name(
    container: &crate::model::proxy::Container,
) -> Option<BackendContainerName> {
    container
        .runtime_values
        .get(&BACKEND_CONTAINER_NAME)
        .and_then(|value| value.data.parse_json())
}

#[async_trait]
impl ContainerBackend for KubernetesBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports_health_check(&self) -> bool {
        true
    }

    async fn initialize(&self) -> Result<(), BackendError> {
        // a cluster that cannot be reached is a configuration problem worth reporting at startup
        match self.client.apiserver_version().await {
            Ok(version) => tracing::info!(
                "Using Kubernetes {}.{} (namespace {})",
                version.major,
                version.minor,
                self.config.namespace
            ),
            Err(error) => tracing::warn!("Cannot reach the Kubernetes API server: {error}"),
        }
        Ok(())
    }

    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        let container_id = uuid::Uuid::new_v4().to_string();
        let document = self.patched_pod(&context, &container_id)?;

        // the namespace of the patched pod wins, so a patch may move the app to another namespace
        let namespace = document
            .get("metadata")
            .and_then(|metadata| metadata.get("namespace"))
            .and_then(Value::as_str)
            .unwrap_or(&self.config.namespace)
            .to_string();
        let name = document
            .get("metadata")
            .and_then(|metadata| metadata.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let mut runtime_values = RuntimeValues::new();
        runtime_values.add(
            RuntimeValue::json(
                &BACKEND_CONTAINER_NAME,
                BackendContainerName::new(&format!("{namespace}/{name}")),
            ),
            true,
        );

        // the manifests of the app definition are created before the pod, as in Java
        self.create_additional_manifests(&context, &namespace)
            .await?;

        let pod: Pod = serde_json::from_value(document)
            .map_err(|error| BackendError::FailedToStart(format!("invalid pod: {error}")))?;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &namespace);
        pods.create(&PostParams::default(), &pod)
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot create pod {name}: {error}"))
            })?;

        // wait until the pod is ready (or failed)
        let deadline = std::time::Instant::now() + self.config.pod_wait_time;
        let interval = self.config.pod_wait_time / 10;
        let mut ready = false;
        while std::time::Instant::now() < deadline {
            let current = pods.get_opt(&name).await.ok().flatten();
            if let Some(failure) = KubernetesBackend::pod_failure(current.as_ref()) {
                tracing::warn!("{failure} [proxyId: {}]", context.proxy.id);
                break;
            }
            if current.as_ref().is_some_and(KubernetesBackend::is_ready) {
                ready = true;
                break;
            }
            tokio::time::sleep(interval).await;
        }
        if !ready {
            // a final check, as the Java implementation does
            let current = pods.get_opt(&name).await.ok().flatten();
            if !current.as_ref().is_some_and(KubernetesBackend::is_ready) {
                return Err(BackendError::FailedToStart(
                    "Kubernetes Pod did not start in time".to_string(),
                ));
            }
        }

        let pod = pods.get(&name).await.map_err(|error| {
            BackendError::FailedToStart(format!("cannot read pod {name}: {error}"))
        })?;

        // where the proxy sends the requests to
        let mut targets = BTreeMap::new();
        if self.config.internal_networking {
            self.ensure_headless_service(&namespace).await?;
            let hostname = pod
                .spec
                .as_ref()
                .and_then(|spec| spec.hostname.clone())
                .unwrap_or_else(|| name.clone());
            let host = manifest::pod_fqdn(&hostname, &namespace, &self.config.cluster_domain);
            for mapping in &context.container_spec.port_mapping {
                let Some(port) = mapping.port else { continue };
                targets.insert(
                    mapping_key_to_path(&mapping.name),
                    target_url(
                        &self.config.target_protocol,
                        &host,
                        port as u16,
                        &compute_target_path(mapping.target_path.as_str()),
                    ),
                );
            }
        } else {
            let document =
                manifest::build_service(&self.config, &context, &container_id, &namespace);
            let service: Service = serde_json::from_value(document).map_err(|error| {
                BackendError::FailedToStart(format!("invalid service: {error}"))
            })?;
            let services: Api<Service> = Api::namespaced(self.client.clone(), &namespace);
            let service_name = manifest::service_name(&context.proxy.id, context.container.index);
            let created = services
                .create(&PostParams::default(), &service)
                .await
                .map_err(|error| {
                    BackendError::FailedToStart(format!(
                        "cannot create service {service_name}: {error}"
                    ))
                })?;

            // the node ports are assigned by the cluster, so the service is read back
            let mut bindings = KubernetesBackend::port_bindings(&created);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while bindings.is_empty() && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if let Ok(Some(current)) = services.get_opt(&service_name).await {
                    bindings = KubernetesBackend::port_bindings(&current);
                }
            }

            let host = pod
                .status
                .as_ref()
                .and_then(|status| status.host_ip.clone())
                .unwrap_or_else(|| "127.0.0.1".to_string());
            for mapping in &context.container_spec.port_mapping {
                let Some(port) = mapping.port else { continue };
                let Some(node_port) = bindings.get(&port) else {
                    return Err(BackendError::FailedToStart(format!(
                        "the service has no node port for container port {port}"
                    )));
                };
                targets.insert(
                    mapping_key_to_path(&mapping.name),
                    target_url(
                        &self.config.target_protocol,
                        &host,
                        *node_port,
                        &compute_target_path(mapping.target_path.as_str()),
                    ),
                );
            }
        }

        Ok(StartedContainer {
            id: Some(container_id),
            runtime_values,
            targets,
        })
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        for container in &proxy.containers {
            let Some(name) = backend_container_name(container) else {
                // the pod was not created yet
                continue;
            };
            let pods: Api<Pod> = Api::namespaced(self.client.clone(), &name.namespace);
            let _ = pods
                .delete(&name.name, &DeleteParams::default().grace_period(0))
                .await;

            if !self.config.internal_networking {
                let services: Api<Service> = Api::namespaced(self.client.clone(), &name.namespace);
                let service_name = manifest::service_name(&proxy.id, container.index);
                let _ = services
                    .delete(&service_name, &DeleteParams::default().grace_period(0))
                    .await;
            }

            // the manifests of this app, except the persistent ones
            let manifest_id = manifest::manifest_id(
                proxy.spec_id.as_deref().unwrap_or_default(),
                proxy.user_id.as_deref().unwrap_or_default(),
            );
            self.delete_additional_manifests(&name.namespace, &manifest_id)
                .await;
        }
        Ok(())
    }

    async fn is_proxy_healthy(&self, proxy: &Proxy) -> Result<bool, BackendError> {
        for container in &proxy.containers {
            if backend_container_name(container).is_none() {
                continue;
            }
            let pod = self.pod_of(container).await;
            if let Some(failure) = KubernetesBackend::pod_failure(pod.as_ref()) {
                tracing::warn!("{failure} [proxyId: {}]", proxy.id);
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn container_logs(
        &self,
        proxy: &Proxy,
        follow: bool,
    ) -> Result<Option<LogStream>, BackendError> {
        let Some(container) = proxy.containers.first() else {
            return Ok(None);
        };
        let Some(name) = backend_container_name(container) else {
            return Ok(None);
        };

        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &name.namespace);
        let parameters = LogParams {
            follow,
            ..LogParams::default()
        };
        let reader = pods
            .log_stream(&name.name, &parameters)
            .await
            .map_err(|error| {
                BackendError::Backend(format!("cannot read the logs of {}: {error}", name.name))
            })?;

        // Kubernetes does not separate stdout and stderr, so everything lands in the stdout file (as in
        // Java, which copies the log stream to stdout)
        let chunks = futures::stream::unfold(
            (reader, vec![0u8; 8192]),
            |(mut reader, mut buffer)| async move {
                use futures::AsyncReadExt;
                match reader.read(&mut buffer).await {
                    Ok(0) => None,
                    Ok(read) => {
                        let chunk = LogChunk {
                            stderr: false,
                            data: buffer[..read].to_vec(),
                        };
                        Some((Ok(chunk), (reader, buffer)))
                    }
                    Err(error) => Some((
                        Err(BackendError::Backend(format!(
                            "cannot read the logs of the pod: {error}"
                        ))),
                        (reader, buffer),
                    )),
                }
            },
        );
        Ok(Some(Box::pin(chunks)))
    }

    async fn scan_existing_containers(&self) -> Result<Vec<ExistingContainerInfo>, BackendError> {
        let mut existing = Vec::new();
        for namespace in &self.config.app_namespaces {
            let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
            let parameters = ListParams::default().labels(&format!("{}=true", PROXIED_APP.label));
            let list = pods.list(&parameters).await.map_err(|error| {
                BackendError::Backend(format!("cannot list pods in {namespace}: {error}"))
            })?;

            for pod in list.items {
                let labels = pod.labels().clone();
                let annotations = pod.annotations().clone();
                let Some(container_id) = labels.get("app").cloned() else {
                    // not a pod of ShinyProxy
                    continue;
                };

                let Some(mut runtime_values) = self.registry.parse_labels_and_annotations(
                    labels
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str())),
                    annotations
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str())),
                ) else {
                    tracing::warn!(
                        "Ignoring container {container_id} because a required label or annotation is \
                         missing"
                    );
                    continue;
                };

                let image = pod
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.containers.first())
                    .and_then(|container| container.image.clone());
                if let Some(image) = &image {
                    runtime_values.add(RuntimeValue::string(&CONTAINER_IMAGE, image.clone()), true);
                }
                let pod_namespace = pod.namespace().unwrap_or_else(|| namespace.clone());
                runtime_values.add(
                    RuntimeValue::json(
                        &BACKEND_CONTAINER_NAME,
                        BackendContainerName::new(&format!("{pod_namespace}/{}", pod.name_any())),
                    ),
                    true,
                );

                // the ports come from the service of the app (unless the pods are reached directly)
                let mut port_bindings = BTreeMap::new();
                if !self.config.internal_networking {
                    let proxy_id = runtime_values
                        .value_string(&crate::model::runtime_value::PROXY_ID)
                        .unwrap_or_default();
                    let index = runtime_values
                        .get(&crate::model::runtime_value::CONTAINER_INDEX)
                        .and_then(|value| value.data.as_int())
                        .unwrap_or_default();
                    let services: Api<Service> =
                        Api::namespaced(self.client.clone(), &pod_namespace);
                    let service_name = manifest::service_name(&proxy_id, index);
                    match services.get_opt(&service_name).await {
                        Ok(Some(service)) => {
                            for (port, node_port) in KubernetesBackend::port_bindings(&service) {
                                if let Ok(port) = u16::try_from(port) {
                                    port_bindings.insert(port, node_port);
                                }
                            }
                        }
                        _ => {
                            tracing::warn!(
                                "Ignoring container {container_id} because it has no associated \
                                 service"
                            );
                            continue;
                        }
                    }
                }

                existing.push(ExistingContainerInfo {
                    id: container_id,
                    runtime_values,
                    image,
                    port_bindings,
                });
            }
        }
        Ok(existing)
    }
}

impl KubernetesBackend {
    /// Deletes the additional manifests of an app (`KubernetesManifestsRemover`).
    async fn delete_additional_manifests(&self, namespace: &str, manifest_id: &str) {
        // only the resources that are not persistent are removed
        let selector = format!(
            "{}={manifest_id},{}=false",
            manifest::MANIFEST_ID_LABEL,
            manifest::PERSISTENT_MANIFEST_LABEL
        );
        for kind in [
            ("v1", "Secret"),
            ("v1", "ConfigMap"),
            ("v1", "PersistentVolumeClaim"),
            ("v1", "Service"),
            ("v1", "ServiceAccount"),
        ] {
            let gvk = parse_api_version(kind.0, kind.1);
            let resource = ApiResource::from_gvk(&gvk);
            let api: Api<DynamicObject> =
                Api::namespaced_with(self.client.clone(), namespace, &resource);
            let parameters = ListParams::default().labels(&selector);
            if let Ok(list) = api.list(&parameters).await {
                for object in list.items {
                    let name = object.name_any();
                    let _ = api
                        .delete(&name, &DeleteParams::default().grace_period(0))
                        .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::spec::ProxySpec;

    #[test]
    fn reads_the_kubernetes_fields_of_an_app() {
        let mut spec = ProxySpec::new("01_hello");
        spec.spec_extensions.insert(
            "kubernetes",
            serde_json::json!({
                "pod-patches": "- op: add\n  path: /spec/nodeName\n  value: node-1\n",
                "additional-manifests": ["apiVersion: v1\nkind: Secret\n"],
                "additional-persistent-manifests": ["apiVersion: v1\nkind: PersistentVolumeClaim\n"],
                "authorized-pod-patches": [{
                    "access-control": {"groups": "scientists"},
                    "patches": "- op: add\n  path: /spec/nodeName\n  value: node-2\n"
                }],
            }),
        );

        let extension = KubernetesSpecExtension::of(&spec);
        assert!(extension.pod_patches.as_deref().unwrap().contains("node-1"));
        assert_eq!(extension.additional_manifests.len(), 1);
        assert_eq!(extension.additional_persistent_manifests.len(), 1);
        assert_eq!(extension.authorized_pod_patches.len(), 1);
        assert_eq!(
            extension.authorized_pod_patches[0]
                .access_control
                .as_ref()
                .map(|access| access.groups()),
            Some(["scientists".to_string()].as_slice())
        );

        // an app without Kubernetes fields yields the defaults
        let extension = KubernetesSpecExtension::of(&ProxySpec::new("other"));
        assert!(extension.pod_patches.is_none());
        assert!(extension.additional_manifests.is_empty());
    }

    #[test]
    fn splits_api_versions() {
        let gvk = parse_api_version("v1", "Pod");
        assert_eq!(gvk.group, "");
        assert_eq!(gvk.version, "v1");
        assert_eq!(gvk.kind, "Pod");

        let gvk = parse_api_version("apps/v1", "Deployment");
        assert_eq!(gvk.group, "apps");
        assert_eq!(gvk.version, "v1");
        assert_eq!(gvk.kind, "Deployment");
    }

    #[test]
    fn recognises_failed_and_ready_pods() {
        // no pod at all
        assert_eq!(
            KubernetesBackend::pod_failure(None).as_deref(),
            Some("Kubernetes container failed, pod does not exist")
        );

        let running: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": "sp-pod-1", "namespace": "default"},
            "spec": {"containers": [{"name": "sp-container-0"}]},
            "status": {
                "phase": "Running",
                "conditions": [{"type": "Ready", "status": "True"}],
                "containerStatuses": [{"name": "sp-container-0", "ready": true, "image": "img", "imageID": "", "restartCount": 0, "state": {"running": {}}}]
            }
        }))
        .expect("pod");
        assert!(KubernetesBackend::pod_failure(Some(&running)).is_none());
        assert!(KubernetesBackend::is_ready(&running));

        let pending: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": "sp-pod-1"},
            "status": {"phase": "Pending"}
        }))
        .expect("pod");
        assert!(KubernetesBackend::pod_failure(Some(&pending)).is_none());
        assert!(!KubernetesBackend::is_ready(&pending));

        let failed: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": "sp-pod-1"},
            "spec": {"containers": [{"name": "sp-container-0"}], "nodeName": "node-1"},
            "status": {
                "phase": "Failed",
                "message": "the pod failed",
                "containerStatuses": [{
                    "name": "sp-container-0", "ready": false, "image": "img", "imageID": "",
                    "restartCount": 0,
                    "state": {"terminated": {"exitCode": 1, "reason": "Error", "message": "boom"}}
                }]
            }
        }))
        .expect("pod");
        let failure = KubernetesBackend::pod_failure(Some(&failed)).expect("failure");
        assert!(failure.contains("reason: 'Error'"), "{failure}");
        assert!(failure.contains("exitCode: '1'"), "{failure}");
        assert!(failure.contains("node: 'node-1'"), "{failure}");
        assert!(failure.contains("boom"), "{failure}");
        assert!(!KubernetesBackend::is_ready(&failed));
    }

    #[test]
    fn reads_the_node_ports_of_a_service() {
        let service: Service = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": "sp-service-1-0"},
            "spec": {"type": "NodePort", "ports": [
                {"port": 3838, "nodePort": 32000},
                {"port": 8080, "nodePort": 32001},
                {"port": 9090}
            ]}
        }))
        .expect("service");
        let bindings = KubernetesBackend::port_bindings(&service);
        assert_eq!(bindings.get(&3838), Some(&32000));
        assert_eq!(bindings.get(&8080), Some(&32001));
        assert_eq!(
            bindings.get(&9090),
            None,
            "a port without a node port is skipped"
        );
    }
}
