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

//! Container backends: what actually runs an app.
//!
//! `proxy.container-backend` selects the backend (`docker` by default, `docker-swarm`, `kubernetes`,
//! `ecs`, and the test-only `local` backend of this implementation). The trait mirrors
//! `IContainerBackend`.

pub mod docker;
pub mod ecs;
pub mod kubernetes;
pub mod local;
pub mod ports;
pub mod swarm;
pub mod target;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::Settings;
use crate::model::proxy::{Container, Proxy};
use crate::model::runtime_value::RuntimeValues;
use crate::model::spec::{ContainerSpec, ProxySpec};
use crate::spec::expression::UserContext;

pub use ports::PortAllocator;
pub use target::{compute_target_path, mapping_key_to_path, DEFAULT_MAPPING_KEY};

/// Everything a backend needs to start a container.
pub struct StartContext<'a> {
    /// The user the app is started for.
    pub user: Option<&'a UserContext>,
    /// The proxy the container belongs to.
    pub proxy: &'a Proxy,
    /// The app definition (with expressions resolved).
    pub spec: &'a ProxySpec,
    /// The definition of this container.
    pub container_spec: &'a ContainerSpec,
    /// The container that is being started (index and runtime values are already set).
    pub container: &'a Container,
    /// Environment variables to inject (runtime values + `container-env` + env file).
    pub environment: BTreeMap<String, String>,
    /// Labels/annotations to set (runtime values + `labels`).
    pub labels: BTreeMap<String, String>,
}

/// The result of starting a container.
#[derive(Debug, Clone, Default)]
pub struct StartedContainer {
    /// Backend specific id of the container.
    pub id: Option<String>,
    /// Runtime values the backend added (e.g. the backend container name).
    pub runtime_values: RuntimeValues,
    /// Targets of the proxy: mapping name (`""` for the default mapping) to URL.
    pub targets: BTreeMap<String, String>,
}

/// Information about a container that already exists, used by app recovery.
#[derive(Debug, Clone)]
pub struct ExistingContainerInfo {
    /// Backend specific id.
    pub id: String,
    /// Runtime values parsed from the labels/annotations.
    pub runtime_values: RuntimeValues,
    /// Image of the container.
    pub image: Option<String>,
    /// Container port to host port mapping.
    pub port_bindings: BTreeMap<u16, u16>,
}

/// One chunk of the output of a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    /// Whether the chunk was written to the standard error.
    pub stderr: bool,
    /// The bytes the container wrote.
    pub data: Vec<u8>,
}

/// The output of the containers of a proxy.
pub type LogStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<LogChunk, BackendError>> + Send>>;

/// Errors of a container backend.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The container could not be created or started.
    #[error("{0}")]
    FailedToStart(String),
    /// The backend does not support this operation (e.g. pausing).
    #[error("this backend does not support {0}")]
    Unsupported(&'static str),
    /// Something went wrong while talking to the backend.
    #[error("backend error: {0}")]
    Backend(String),
}

/// What runs the apps.
#[async_trait]
pub trait ContainerBackend: Send + Sync + std::fmt::Debug {
    /// Name as used by `proxy.container-backend`.
    fn name(&self) -> &'static str;

    /// Whether apps can be paused and resumed (only the Docker backends can).
    fn supports_pause(&self) -> bool {
        false
    }

    /// Whether `is_proxy_healthy` gives a meaningful answer for this backend.
    fn supports_health_check(&self) -> bool {
        false
    }

    /// Checks whether the backend can be used, called once at startup.
    ///
    /// The Java implementation only fails at startup for Docker Swarm (which inspects the swarm); the
    /// other backends fail when the first app is started. This method keeps that behaviour: it returns an
    /// error only when the backend is certainly unusable.
    async fn initialize(&self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Starts one container of a proxy.
    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError>;

    /// Stops all containers of a proxy; must be idempotent.
    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError>;

    /// Pauses a proxy (keeping its containers).
    async fn pause_proxy(&self, _proxy: &Proxy) -> Result<(), BackendError> {
        Err(BackendError::Unsupported("pausing apps"))
    }

    /// Resumes a paused proxy.
    async fn resume_proxy(
        &self,
        _proxy: &Proxy,
        _spec: &ProxySpec,
    ) -> Result<StartedContainer, BackendError> {
        Err(BackendError::Unsupported("resuming apps"))
    }

    /// Whether the containers of the proxy are still running.
    async fn is_proxy_healthy(&self, _proxy: &Proxy) -> Result<bool, BackendError> {
        Ok(true)
    }

    /// The output of the containers of a proxy, used by the log service.
    ///
    /// `follow` keeps the stream open while the app runs. `None` means that this backend cannot provide
    /// the output (the Java implementation logs "no output streams defined" in that case).
    async fn container_logs(
        &self,
        _proxy: &Proxy,
        _follow: bool,
    ) -> Result<Option<LogStream>, BackendError> {
        Ok(None)
    }

    /// The targets of a container that already exists, used by app recovery.
    ///
    /// The backend decides how a container is reached (`setupPortMappingExistingProxy` in Java, which asks
    /// the backend for every mapping), because the answer differs per backend: a published host port for
    /// Docker, a node port or a pod name for Kubernetes.
    async fn existing_targets(
        &self,
        _container: &Container,
        _port_bindings: &BTreeMap<u16, u16>,
    ) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// Containers that already exist, used by app recovery.
    async fn scan_existing_containers(&self) -> Result<Vec<ExistingContainerInfo>, BackendError> {
        Ok(Vec::new())
    }
}

/// Creates the configured container backend, connecting to the cluster when needed.
pub async fn create_async(
    settings: &Settings,
    context: BackendContext,
) -> Result<Arc<dyn ContainerBackend>, CreateError> {
    let name = settings
        .proxy
        .container_backend()
        .to_ascii_lowercase()
        .replace(['-', '_'], "");
    match name.as_str() {
        "kubernetes" => {
            let config = kubernetes::KubernetesConfig::from_settings(settings)
                .map_err(CreateError::Configuration)?;
            let mut backend =
                kubernetes::KubernetesBackend::connect(config, context.registry.clone()).await?;
            if let Some(access_check) = context.access_check.clone() {
                backend = backend.with_access_check(access_check);
            }
            Ok(Arc::new(backend))
        }
        "ecs" => {
            let config =
                ecs::EcsConfig::from_settings(settings).map_err(CreateError::Configuration)?;
            let backend = ecs::EcsBackend::connect(config, context.registry.clone()).await?;
            Ok(Arc::new(backend))
        }
        _ => create(settings, context),
    }
}

/// The configured container backend is not implemented yet.
#[derive(Debug, thiserror::Error)]
#[error(
    "container backend '{name}' is not supported yet by this implementation \
     (supported: docker, docker-swarm, kubernetes, ecs, local); see docs/PROGRESS.md for the phase \
     that adds it"
)]
pub struct UnsupportedBackend {
    /// The configured value of `proxy.container-backend`.
    pub name: String,
}

/// Why the container backend could not be created.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// The backend is not implemented (yet).
    #[error(transparent)]
    Unsupported(#[from] UnsupportedBackend),
    /// The configuration of the backend is invalid.
    #[error("invalid container backend configuration: {0}")]
    Configuration(String),
    /// The backend could not be reached.
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// Everything a backend may need besides the settings.
#[derive(Clone)]
pub struct BackendContext {
    /// Ports that can be published on the host.
    pub port_allocator: Arc<PortAllocator>,
    /// Runtime value keys, used to parse the labels of existing containers.
    pub registry: Arc<crate::model::runtime_value::RuntimeValueRegistry>,
    /// The realm of this server (used in the Loki labels).
    pub realm_id: Option<String>,
    /// Evaluates the access control of the authorized Kubernetes patches and manifests.
    pub access_check: Option<kubernetes::AccessCheck>,
}

impl std::fmt::Debug for BackendContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackendContext")
            .field("port_allocator", &self.port_allocator)
            .field("realm_id", &self.realm_id)
            .finish()
    }
}

/// Creates the configured container backend.
///
/// The Kubernetes backend has to connect to the cluster, which is asynchronous; [`create`] therefore
/// refuses it and [`create_async`] is used by the server.
pub fn create(
    settings: &Settings,
    context: BackendContext,
) -> Result<Arc<dyn ContainerBackend>, CreateError> {
    let BackendContext {
        port_allocator,
        registry,
        realm_id,
        access_check: _,
    } = context;
    match settings
        .proxy
        .container_backend()
        .to_ascii_lowercase()
        .as_str()
    {
        "docker" => {
            let config = docker::DockerConfig::from_settings(settings, realm_id)
                .map_err(CreateError::Configuration)?;
            Ok(Arc::new(docker::DockerBackend::new(
                config,
                port_allocator,
                registry,
            )?))
        }
        "docker-swarm" => {
            let config = docker::DockerConfig::from_settings(settings, realm_id)
                .map_err(CreateError::Configuration)?;
            Ok(Arc::new(swarm::SwarmBackend::new(
                config,
                settings,
                port_allocator,
                registry,
            )?))
        }
        "local" => Ok(Arc::new(local::LocalBackend::new(settings, port_allocator))),
        // these clients connect asynchronously; see `create_async`
        other @ ("kubernetes" | "ecs") => Err(CreateError::Configuration(format!(
            "the {other} backend must be created with backend::create_async"
        ))),
        other => Err(UnsupportedBackend {
            name: other.to_string(),
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> BackendContext {
        BackendContext {
            port_allocator: Arc::new(PortAllocator::new(20000, None)),
            registry: Arc::new(crate::model::runtime_value::RuntimeValueRegistry::engine()),
            realm_id: None,
            access_check: None,
        }
    }

    #[test]
    fn creates_the_local_backend() {
        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  container-backend: local\n").unwrap();
        let backend = create(&settings, context()).unwrap();
        assert_eq!(backend.name(), "local");
        assert!(!backend.supports_pause());
    }

    #[test]
    fn reports_unsupported_backends() {
        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  container-backend: nonsense\n").unwrap();
        let error = create(&settings, context()).unwrap_err();
        assert!(error.to_string().contains("nonsense"), "{error}");
        assert!(error.to_string().contains("not supported yet"), "{error}");
    }

    #[test]
    fn reports_invalid_backend_configuration() {
        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  container-backend: docker\n  docker:\n    image-pull-policy: sometimes\n",
        )
        .unwrap();
        let error = create(&settings, context()).unwrap_err();
        assert!(error.to_string().contains("image-pull-policy"), "{error}");
    }
}
