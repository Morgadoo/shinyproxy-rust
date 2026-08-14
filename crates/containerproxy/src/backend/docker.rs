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

//! The `docker` container backend: apps run as Docker containers.
//!
//! Port of `AbstractDockerBackend` and `DockerEngineBackend`. The container that is created is byte for
//! byte the same request as the Java implementation sends: same image, command, environment, labels, port
//! bindings, resource limits, network settings, log configuration and container name
//! (`sp-container-{proxyId}-{index}`), so that a Rust server can take over the containers of a Java
//! server (app recovery) and the other way around.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, InspectContainerOptions,
    ListContainersOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::secret::{
    ContainerCreateBody, DeviceRequest, HostConfig, HostConfigLogConfig, PortBinding,
};
use bollard::Docker;
use futures::StreamExt;

use super::ports::PortAllocator;
use super::target::{compute_target_path, mapping_key_to_path, target_url};
use super::{
    BackendError, ContainerBackend, ExistingContainerInfo, StartContext, StartedContainer,
};
use crate::config::Settings;
use crate::model::proxy::Proxy;
use crate::model::runtime_value::{
    BackendContainerName, RuntimeValue, RuntimeValueRegistry, RuntimeValues,
    BACKEND_CONTAINER_NAME, CONTAINER_IMAGE,
};
use crate::model::spec::ProxySpec;

/// Name of the backend.
pub const NAME: &str = "docker";

/// The CPU period Docker uses when a CPU limit is configured (Java uses the same constant).
const CPU_PERIOD: i64 = 100_000;

/// When to pull an image (`proxy.docker.image-pull-policy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePullPolicy {
    /// Never pull, the image must exist on the host.
    Never,
    /// Always pull before starting a container.
    Always,
    /// Pull when the image is not present (the default).
    IfNotPresent,
}

impl ImagePullPolicy {
    /// Parses the value of `proxy.docker.image-pull-policy` (Java uses a case sensitive enum, but Spring
    /// relaxed binding accepts other spellings as well).
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") => Ok(ImagePullPolicy::IfNotPresent),
            Some(value) => match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
                "never" => Ok(ImagePullPolicy::Never),
                "always" => Ok(ImagePullPolicy::Always),
                "ifnotpresent" => Ok(ImagePullPolicy::IfNotPresent),
                _ => Err(format!(
                    "invalid value '{value}' for proxy.docker.image-pull-policy \
                     (expected Never, Always or IfNotPresent)"
                )),
            },
        }
    }
}

/// Everything the backend needs to know about the Docker daemon and how to reach containers.
#[derive(Debug, Clone)]
pub struct DockerConfig {
    /// The value of `proxy.docker.url` (or `DOCKER_HOST`).
    pub url: Option<String>,
    /// Directory with client certificates (`proxy.docker.cert-path`).
    pub cert_path: Option<String>,
    /// Whether containers are reached over the internal container network.
    pub internal_networking: bool,
    /// The network containers are attached to (`proxy.docker.default-container-network`).
    pub container_network: Option<String>,
    /// Whether containers run privileged (`proxy.docker.privileged`).
    pub privileged: bool,
    /// When to pull images.
    pub image_pull_policy: ImagePullPolicy,
    /// Loki endpoint for the container log driver (`proxy.docker.loki-url`).
    pub loki_url: Option<String>,
    /// Host interface the container ports are published on (`proxy.docker.target-bind-ip`).
    pub target_bind_ip: String,
    /// Protocol used to reach containers.
    pub target_protocol: String,
    /// Host used to reach containers when they are not on an internal network.
    pub target_host: String,
    /// Realm of this server, used in the Loki labels.
    pub realm_id: Option<String>,
}

impl DockerConfig {
    /// Reads the configuration from the settings.
    pub fn from_settings(settings: &Settings, realm_id: Option<String>) -> Result<Self, String> {
        let docker = &settings.proxy.docker;
        let target_url = docker
            .target_url
            .clone()
            .unwrap_or_else(|| "http://localhost".to_string());
        let parsed = url::Url::parse(&target_url)
            .map_err(|error| format!("invalid proxy.docker.target-url '{target_url}': {error}"))?;
        let target_protocol = docker
            .container_protocol
            .clone()
            .unwrap_or_else(|| parsed.scheme().to_string());
        let target_host = parsed.host_str().unwrap_or("localhost").to_string();

        Ok(DockerConfig {
            url: docker.url.clone(),
            cert_path: docker.cert_path.clone(),
            internal_networking: docker.internal_networking(),
            container_network: docker.default_container_network.clone(),
            privileged: docker.privileged.map(|value| value.0).unwrap_or(false),
            image_pull_policy: ImagePullPolicy::parse(docker.image_pull_policy.as_deref())?,
            loki_url: docker.loki_url.clone(),
            target_bind_ip: docker.target_bind_ip().to_string(),
            target_protocol,
            target_host,
            realm_id,
        })
    }
}

/// Runs apps as Docker containers.
pub struct DockerBackend {
    client: Docker,
    config: DockerConfig,
    port_allocator: Arc<PortAllocator>,
    registry: Arc<RuntimeValueRegistry>,
}

impl std::fmt::Debug for DockerBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DockerBackend")
            .field("config", &self.config)
            .finish()
    }
}

impl DockerBackend {
    /// Connects to the Docker daemon.
    ///
    /// The connection is made the same way as the Java implementation: `proxy.docker.url` wins, then the
    /// `DOCKER_*` environment variables, then the local socket. Certificates from
    /// `proxy.docker.cert-path` are used for TLS connections.
    pub fn new(
        config: DockerConfig,
        port_allocator: Arc<PortAllocator>,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Result<Self, BackendError> {
        let client = connect(&config)?;
        Ok(DockerBackend {
            client,
            config,
            port_allocator,
            registry,
        })
    }

    /// Creates a backend around an existing client (used by tests and by the Swarm backend).
    pub fn with_client(
        client: Docker,
        config: DockerConfig,
        port_allocator: Arc<PortAllocator>,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Self {
        DockerBackend {
            client,
            config,
            port_allocator,
            registry,
        }
    }

    /// The Docker client, so that other backends can reuse the connection.
    pub fn client(&self) -> &Docker {
        &self.client
    }

    /// The configuration of this backend.
    pub fn config(&self) -> &DockerConfig {
        &self.config
    }

    /// The port allocator, shared with the Swarm backend.
    pub fn port_allocator(&self) -> &Arc<PortAllocator> {
        &self.port_allocator
    }

    /// The runtime value keys, shared with the Swarm backend.
    pub fn registry(&self) -> &Arc<RuntimeValueRegistry> {
        &self.registry
    }

    /// Checks that the daemon answers, so that a misconfigured daemon is reported at startup instead of
    /// when the first app is started (parity with the Java client, which connects eagerly).
    pub async fn check_connection(&self) -> Result<String, BackendError> {
        let version = self.client.version().await.map_err(|error| {
            BackendError::Backend(format!("cannot reach Docker daemon: {error}"))
        })?;
        Ok(version.version.unwrap_or_else(|| "unknown".to_string()))
    }

    /// Pulls the image when the pull policy says so.
    async fn pull_image_if_needed(&self, context: &StartContext<'_>) -> Result<(), BackendError> {
        let image = context
            .container_spec
            .image
            .as_str()
            .unwrap_or_default()
            .to_string();
        if image.is_empty() {
            return Err(BackendError::FailedToStart(
                "container-image is required".to_string(),
            ));
        }

        let present = self.client.inspect_image(&image).await.is_ok();
        let pull = match self.config.image_pull_policy {
            ImagePullPolicy::Always => true,
            ImagePullPolicy::IfNotPresent => !present,
            ImagePullPolicy::Never => false,
        };
        if !pull {
            return Ok(());
        }

        tracing::info!("Pulling image {image} [proxyId: {}]", context.proxy.id);
        let spec = context.container_spec;
        let credentials = match (
            spec.docker_registry_domain.as_ref(),
            spec.docker_registry_username.as_ref(),
            spec.docker_registry_password.as_ref(),
        ) {
            (Some(domain), Some(username), Some(password)) => {
                Some(bollard::auth::DockerCredentials {
                    serveraddress: Some(domain.clone()),
                    username: Some(username.clone()),
                    password: Some(password.clone()),
                    ..Default::default()
                })
            }
            _ => None,
        };

        let options = CreateImageOptionsBuilder::new().from_image(&image).build();
        let mut stream = self.client.create_image(Some(options), None, credentials);
        while let Some(message) = stream.next().await {
            match message {
                Ok(_) => {}
                Err(error) => {
                    return Err(BackendError::FailedToStart(format!(
                        "cannot pull image {image}: {error}"
                    )))
                }
            }
        }
        Ok(())
    }
}

/// Connects to the daemon.
fn connect(config: &DockerConfig) -> Result<Docker, BackendError> {
    let map_error = |error: bollard::errors::Error| {
        BackendError::Backend(format!("cannot connect to Docker: {error}"))
    };

    if let Some(url) = &config.url {
        let timeout = 0; // no timeout, the Java client uses none either (needed for pulls and logs)
        let version = bollard::API_DEFAULT_VERSION;
        if let Some(certificates) = &config.cert_path {
            let directory = std::path::Path::new(certificates);
            return Docker::connect_with_ssl(
                url,
                &directory.join("key.pem"),
                &directory.join("cert.pem"),
                &directory.join("ca.pem"),
                timeout,
                version,
            )
            .map_err(map_error);
        }
        if url.starts_with("unix://") {
            return Docker::connect_with_socket(
                url.trim_start_matches("unix://"),
                timeout,
                version,
            )
            .map_err(map_error);
        }
        return Docker::connect_with_http(url, timeout, version).map_err(map_error);
    }

    Docker::connect_with_defaults().map_err(map_error)
}

/// Converts a memory string (`2g`, `512m`, `1024`) into bytes, like `memoryToBytes` in Java.
pub fn memory_to_bytes(memory: Option<&str>) -> Result<Option<i64>, String> {
    let Some(memory) = memory.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if memory.contains(',') {
        return Err(format!(
            "Invalid memory argument: {memory}, no ',' allowed in number"
        ));
    }
    let lower = memory.to_ascii_lowercase();
    // (\d+\.?\d*)([bkmg]?)i?
    let (number, rest) = lower.split_at(
        lower
            .find(|character: char| !character.is_ascii_digit() && character != '.')
            .unwrap_or(lower.len()),
    );
    let invalid = || format!("Invalid memory argument: {memory}");
    if number.is_empty()
        || number.matches('.').count() > 1
        || number.ends_with('.') && rest.is_empty()
    {
        return Err(invalid());
    }
    let value: f64 = number.parse().map_err(|_| invalid())?;
    let (unit, suffix) = match rest.strip_suffix('i') {
        Some(unit) => (unit, true),
        None => (rest, false),
    };
    let factor = match unit {
        "k" => 1024.0,
        "m" => 1024.0 * 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        // "b" and "" match the Java regex but fall into the default branch, which throws
        "" | "b" => return Err(invalid()),
        _ => return Err(invalid()),
    };
    if suffix && unit.is_empty() {
        return Err(invalid());
    }
    Ok(Some((value * factor) as i64))
}

/// Converts a CPU limit (`2`, `500m`) into a Docker CPU quota, like `getCpuQuota` in Java.
pub fn cpu_quota(period: i64, cpu: &str) -> Result<i64, String> {
    let converted = match cpu.strip_suffix('m') {
        Some(value) => {
            value
                .parse::<f64>()
                .map_err(|_| format!("Invalid cpu argument: {cpu}"))?
                / 1000.0
        }
        None => cpu
            .parse::<f64>()
            .map_err(|_| format!("Invalid cpu argument: {cpu}"))?,
    };
    Ok((period as f64 * converted) as i64)
}

/// The name of a container, as the Java implementation builds it.
pub fn container_name(proxy_id: &str, index: i64, resource_name: Option<&str>) -> String {
    match resource_name.filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => format!("sp-container-{proxy_id}-{index}"),
    }
}

/// The body of the container create request.
///
/// Separated from the request itself so that it can be asserted in unit tests without a daemon.
pub fn build_container_create_body(
    config: &DockerConfig,
    context: &StartContext<'_>,
    port_bindings: &BTreeMap<i64, u16>,
) -> Result<ContainerCreateBody, BackendError> {
    let spec = context.container_spec;
    let failed = BackendError::FailedToStart;

    // published ports, bound to the configured interface (skipped in internal networking mode)
    let mut docker_port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    for (container_port, host_port) in port_bindings {
        docker_port_bindings.insert(
            container_port.to_string(),
            Some(vec![PortBinding {
                host_ip: Some(config.target_bind_ip.clone()),
                host_port: Some(host_port.to_string()),
            }]),
        );
    }

    let mut host_config = HostConfig {
        port_bindings: Some(docker_port_bindings.clone()),
        memory_reservation: memory_to_bytes(spec.memory_request.as_str()).map_err(failed)?,
        memory: memory_to_bytes(spec.memory_limit.as_str()).map_err(failed)?,
        privileged: Some(config.privileged || spec.privileged),
        ..Default::default()
    };

    if let Some(cpu) = spec.cpu_limit.as_str().filter(|value| !value.is_empty()) {
        host_config.cpu_period = Some(CPU_PERIOD);
        host_config.cpu_quota = Some(cpu_quota(CPU_PERIOD, cpu).map_err(failed)?);
    }

    // `container-network` of the app wins over `proxy.docker.default-container-network`
    let network = spec
        .network
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| config.container_network.clone());
    host_config.network_mode = network;

    if let Some(dns) = spec.dns.value().filter(|value| !value.is_empty()) {
        host_config.dns = Some(dns.clone());
    }
    if let Some(volumes) = spec.volumes.value().filter(|value| !value.is_empty()) {
        host_config.binds = Some(volumes.clone());
    }
    if let Some(ipc) = spec.docker_ipc.as_str().filter(|value| !value.is_empty()) {
        host_config.ipc_mode = Some(ipc.to_string());
    }
    if let Some(runtime) = spec
        .docker_runtime
        .as_str()
        .filter(|value| !value.is_empty())
    {
        host_config.runtime = Some(runtime.to_string());
    }
    if let Some(group_add) = spec
        .docker_group_add
        .value()
        .filter(|value| !value.is_empty())
    {
        host_config.group_add = Some(group_add.clone());
    }

    // GPUs and other devices
    let device_requests: Vec<DeviceRequest> = spec
        .docker_device_requests
        .iter()
        .map(|request| DeviceRequest {
            driver: request.driver.clone(),
            count: request.count,
            device_ids: if request.device_ids.is_empty() {
                None
            } else {
                Some(request.device_ids.clone())
            },
            capabilities: if request.capabilities.is_empty() {
                None
            } else {
                Some(request.capabilities.clone())
            },
            options: if request.options.is_empty() {
                None
            } else {
                Some(request.options.clone().into_iter().collect())
            },
        })
        .collect();
    host_config.device_requests = Some(device_requests);

    // ship the logs of the container to Loki when configured
    if let Some(loki_url) = &config.loki_url {
        let mut options = HashMap::new();
        options.insert("loki-url".to_string(), loki_url.clone());
        options.insert("mode".to_string(), "non-blocking".to_string());
        options.insert(
            "loki-external-labels".to_string(),
            format!(
                "sp_realm_id={},namespace=default,sp_proxy_id={}",
                config.realm_id.clone().unwrap_or_default(),
                context.proxy.id
            ),
        );
        host_config.log_config = Some(HostConfigLogConfig {
            typ: Some("loki".to_string()),
            config: Some(options),
        });
    }

    // labels: those of the app definition plus the runtime values that are labels/annotations
    let mut labels: HashMap<String, String> = spec
        .labels
        .value()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    for (name, value) in &context.labels {
        labels.insert(name.clone(), value.clone());
    }

    let environment: Vec<String> = context
        .environment
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();

    let exposed_ports: HashMap<String, HashMap<(), ()>> = docker_port_bindings
        .keys()
        .map(|port| (port.clone(), HashMap::new()))
        .collect();

    Ok(ContainerCreateBody {
        image: Some(spec.image.as_str().unwrap_or_default().to_string()),
        cmd: spec.cmd.value().cloned().filter(|cmd| !cmd.is_empty()),
        env: Some(environment),
        labels: Some(labels),
        exposed_ports: Some(exposed_ports),
        user: spec
            .docker_user
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        host_config: Some(host_config),
        ..Default::default()
    })
}

#[async_trait]
impl ContainerBackend for DockerBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports_pause(&self) -> bool {
        true
    }

    fn supports_health_check(&self) -> bool {
        true
    }

    async fn initialize(&self) -> Result<(), BackendError> {
        // the Java client does not ping the daemon at startup either, so an unreachable daemon is only
        // a warning here; starting an app then fails with the same message as in Java
        match self.check_connection().await {
            Ok(version) => tracing::info!("Using Docker daemon (API version {version})"),
            Err(error) => tracing::warn!("Cannot reach the Docker daemon: {error}"),
        }
        Ok(())
    }

    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        self.pull_image_if_needed(&context).await?;

        let spec = context.container_spec;
        let proxy_id = context.proxy.id.clone();

        // allocate a host port per port mapping (not needed on an internal network)
        let mut port_bindings: BTreeMap<i64, u16> = BTreeMap::new();
        if !self.config.internal_networking {
            for mapping in &spec.port_mapping {
                let Some(container_port) = mapping.port else {
                    continue;
                };
                let host_port = self
                    .port_allocator
                    .allocate(&proxy_id)
                    .map_err(|error| BackendError::FailedToStart(error.to_string()))?;
                port_bindings.insert(container_port, host_port);
            }
        }

        let body = build_container_create_body(&self.config, &context, &port_bindings)?;
        let name = container_name(
            &proxy_id,
            context.container.index,
            spec.resource_name.as_str(),
        );

        let created = self
            .client
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&name).build()),
                body,
            )
            .await
            .map_err(|error| {
                self.port_allocator.release(&proxy_id);
                BackendError::FailedToStart(format!("cannot create container {name}: {error}"))
            })?;

        // additional networks the container must join
        if let Some(networks) = spec
            .network_connections
            .value()
            .filter(|value| !value.is_empty())
        {
            for network in networks {
                self.client
                    .connect_network(
                        network,
                        bollard::models::NetworkConnectRequest {
                            container: Some(created.id.clone()),
                            endpoint_config: None,
                        },
                    )
                    .await
                    .map_err(|error| {
                        BackendError::FailedToStart(format!(
                            "cannot connect container {name} to network {network}: {error}"
                        ))
                    })?;
            }
        }

        self.client
            .start_container(
                &created.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot start container {name}: {error}"))
            })?;

        // where the proxy sends the requests to
        let mut targets = BTreeMap::new();
        let hostname = if self.config.internal_networking {
            self.internal_hostname(&created.id).await?
        } else {
            self.config.target_host.clone()
        };
        for mapping in &spec.port_mapping {
            let Some(container_port) = mapping.port else {
                continue;
            };
            let port = if self.config.internal_networking {
                container_port as u16
            } else {
                *port_bindings.get(&container_port).ok_or_else(|| {
                    BackendError::FailedToStart(format!(
                        "no host port was allocated for container port {container_port}"
                    ))
                })?
            };
            let target_path = compute_target_path(mapping.target_path.as_str());
            targets.insert(
                mapping_key_to_path(&mapping.name),
                target_url(&self.config.target_protocol, &hostname, port, &target_path),
            );
        }

        let mut runtime_values = RuntimeValues::new();
        runtime_values.add(
            RuntimeValue::json(&BACKEND_CONTAINER_NAME, BackendContainerName::new(&name)),
            true,
        );

        Ok(StartedContainer {
            id: Some(created.id),
            runtime_values,
            targets,
        })
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        if proxy.containers.is_empty() {
            // the containers were not created yet, nothing to clean up (see Java issue #33102)
            return Ok(());
        }
        for container in &proxy.containers {
            let Some(id) = container.id.as_deref() else {
                continue;
            };

            // leave the networks first, so that the container does not keep addresses (Java does the same)
            match self
                .client
                .inspect_container(id, None::<InspectContainerOptions>)
                .await
            {
                Ok(info) => {
                    let networks = info
                        .network_settings
                        .and_then(|settings| settings.networks)
                        .unwrap_or_default();
                    for (name, network) in networks {
                        let network_id = network.network_id.unwrap_or(name);
                        if let Err(error) = self
                            .client
                            .disconnect_network(
                                &network_id,
                                bollard::models::NetworkDisconnectRequest {
                                    container: Some(id.to_string()),
                                    force: Some(true),
                                },
                            )
                            .await
                        {
                            // already disconnected, which is not a problem
                            tracing::debug!(
                                "cannot disconnect container {id} from network {network_id}: {error}"
                            );
                        }
                    }
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    // container is already gone
                    self.port_allocator.release(&proxy.id);
                    continue;
                }
                Err(error) => {
                    tracing::debug!("cannot inspect container {id} while stopping: {error}");
                }
            }

            let options = RemoveContainerOptionsBuilder::new().force(true).build();
            match self.client.remove_container(id, Some(options)).await {
                Ok(()) => self.port_allocator.release(&proxy.id),
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    // already removed
                    self.port_allocator.release(&proxy.id);
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 409, ..
                }) => {
                    // the container is being removed; the port is released by the removal that runs
                }
                Err(error) => {
                    return Err(BackendError::Backend(format!(
                        "cannot remove container {id}: {error}"
                    )))
                }
            }
        }
        Ok(())
    }

    async fn pause_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        for container in &proxy.containers {
            let Some(id) = container.id.as_deref() else {
                continue;
            };
            self.client
                .stop_container(id, None::<bollard::query_parameters::StopContainerOptions>)
                .await
                .map_err(|error| {
                    BackendError::Backend(format!("cannot pause container {id}: {error}"))
                })?;
        }
        self.port_allocator.release(&proxy.id);
        Ok(())
    }

    async fn resume_proxy(
        &self,
        proxy: &Proxy,
        _spec: &ProxySpec,
    ) -> Result<StartedContainer, BackendError> {
        let mut targets = BTreeMap::new();
        for container in &proxy.containers {
            let Some(id) = container.id.as_deref() else {
                continue;
            };
            self.client
                .start_container(id, None::<bollard::query_parameters::StartContainerOptions>)
                .await
                .map_err(|error| {
                    BackendError::Backend(format!("cannot resume container {id}: {error}"))
                })?;

            // the host ports of a restarted container are the ones of its configuration, but they were
            // released while the app was paused, so they are claimed again and the targets are rebuilt
            let info = self
                .client
                .inspect_container(id, None::<InspectContainerOptions>)
                .await
                .map_err(|error| {
                    BackendError::Backend(format!("cannot inspect container {id}: {error}"))
                })?;
            let mut host_ports: BTreeMap<i64, u16> = BTreeMap::new();
            for (port, binding) in info
                .network_settings
                .clone()
                .and_then(|settings| settings.ports)
                .unwrap_or_default()
            {
                let Some(host_port) = binding
                    .and_then(|bindings| bindings.first().cloned())
                    .and_then(|binding| binding.host_port)
                    .and_then(|port| port.parse::<u16>().ok())
                else {
                    continue;
                };
                let Some(container_port) =
                    port.split('/').next().and_then(|port| port.parse().ok())
                else {
                    continue;
                };
                self.port_allocator.add_existing_port(&proxy.id, host_port);
                host_ports.insert(container_port, host_port);
            }

            // the names and paths of the mappings are stored on the container, so the targets look
            // exactly like they did before the app was paused
            let mappings: crate::service::runtime_values::PortMappings = container
                .runtime_values
                .get(&crate::model::runtime_value::PORT_MAPPINGS)
                .and_then(|value| value.data.parse_json())
                .unwrap_or_default();
            let hostname = if self.config.internal_networking {
                self.internal_hostname(id).await?
            } else {
                self.config.target_host.clone()
            };
            for mapping in &mappings.port_mappings {
                let port = if self.config.internal_networking {
                    mapping.port as u16
                } else {
                    match host_ports.get(&mapping.port) {
                        Some(host_port) => *host_port,
                        None => continue,
                    }
                };
                targets.insert(
                    mapping_key_to_path(&mapping.name),
                    target_url(
                        &self.config.target_protocol,
                        &hostname,
                        port,
                        &mapping.target_path,
                    ),
                );
            }
        }
        Ok(StartedContainer {
            id: None,
            runtime_values: RuntimeValues::new(),
            targets,
        })
    }

    async fn is_proxy_healthy(&self, proxy: &Proxy) -> Result<bool, BackendError> {
        for container in &proxy.containers {
            let Some(id) = container.id.as_deref() else {
                continue;
            };
            match self
                .client
                .inspect_container(id, None::<InspectContainerOptions>)
                .await
            {
                Ok(info) => {
                    let state = info.state.unwrap_or_default();
                    let running = state.running.unwrap_or(false)
                        && state
                            .status
                            .map(|status| status.to_string() == "running")
                            .unwrap_or(false);
                    if !running {
                        tracing::warn!(
                            "Docker container failed: container not running [proxyId: {}]",
                            proxy.id
                        );
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    tracing::warn!(
                        "Docker container failed: container does not exist [proxyId: {}]",
                        proxy.id
                    );
                    return Ok(false);
                }
                Err(error) => {
                    return Err(BackendError::Backend(format!(
                        "cannot inspect container {id}: {error}"
                    )))
                }
            }
        }
        Ok(true)
    }

    async fn scan_existing_containers(&self) -> Result<Vec<ExistingContainerInfo>, BackendError> {
        let options = ListContainersOptionsBuilder::new().all(true).build();
        let containers = self
            .client
            .list_containers(Some(options))
            .await
            .map_err(|error| BackendError::Backend(format!("cannot list containers: {error}")))?;

        let mut existing = Vec::new();
        for container in containers {
            let id = container.id.clone().unwrap_or_default();
            let state = container
                .state
                .map(|state| state.to_string())
                .unwrap_or_default();
            if !state.eq_ignore_ascii_case("running") {
                tracing::warn!("Ignoring container {id} because it is not running, {state}");
                continue;
            }

            let labels: BTreeMap<String, String> =
                container.labels.unwrap_or_default().into_iter().collect();
            let Some(mut runtime_values) = self.registry.parse_labels(
                labels
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            ) else {
                tracing::warn!("Ignoring container {id} because it has no ShinyProxy labels");
                continue;
            };
            if let Some(image) = &container.image {
                runtime_values.add(RuntimeValue::string(&CONTAINER_IMAGE, image.clone()), true);
            }
            runtime_values.add(
                RuntimeValue::json(&BACKEND_CONTAINER_NAME, BackendContainerName::new(&id)),
                true,
            );

            // the ports of existing containers are registered even when the app is not recovered, so
            // that they are never handed out twice
            let mut port_bindings = BTreeMap::new();
            for port in container.ports.unwrap_or_default() {
                let Some(public) = port.public_port else {
                    continue;
                };
                self.port_allocator.add_existing_port(&id, public);
                port_bindings.insert(port.private_port, public);
            }

            existing.push(ExistingContainerInfo {
                id,
                runtime_values,
                image: container.image,
                port_bindings,
            });
        }
        Ok(existing)
    }
}

impl DockerBackend {
    /// The hostname of a container, used when the proxy talks over the internal container network.
    async fn internal_hostname(&self, id: &str) -> Result<String, BackendError> {
        let info = self
            .client
            .inspect_container(id, None::<InspectContainerOptions>)
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot inspect container {id}: {error}"))
            })?;
        Ok(info
            .config
            .and_then(|config| config.hostname)
            .unwrap_or_else(|| id.to_string()))
    }

    /// The logs of a container, used by the log service.
    pub async fn container_logs(
        &self,
        id: &str,
        follow: bool,
    ) -> impl futures::Stream<Item = Result<bollard::container::LogOutput, bollard::errors::Error>>
    {
        let options = LogsOptionsBuilder::new()
            .follow(follow)
            .stdout(true)
            .stderr(true)
            .build();
        self.client.logs(id, Some(options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Container, ProxyStatus};
    use crate::model::spec::{ContainerSpec, DockerDeviceRequest, PortMapping};
    use crate::model::spel_field::{SpelString, SpelStringList, SpelStringMap};

    fn config() -> DockerConfig {
        DockerConfig {
            url: None,
            cert_path: None,
            internal_networking: false,
            container_network: None,
            privileged: false,
            image_pull_policy: ImagePullPolicy::IfNotPresent,
            loki_url: None,
            target_bind_ip: "127.0.0.1".to_string(),
            target_protocol: "http".to_string(),
            target_host: "localhost".to_string(),
            realm_id: None,
        }
    }

    #[test]
    fn converts_memory_like_java() {
        assert_eq!(memory_to_bytes(None).unwrap(), None);
        assert_eq!(memory_to_bytes(Some("")).unwrap(), None);
        assert_eq!(
            memory_to_bytes(Some("2g")).unwrap(),
            Some(2 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            memory_to_bytes(Some("2G")).unwrap(),
            Some(2 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            memory_to_bytes(Some("512m")).unwrap(),
            Some(512 * 1024 * 1024)
        );
        assert_eq!(memory_to_bytes(Some("1.5g")).unwrap(), Some(1_610_612_736));
        assert_eq!(memory_to_bytes(Some("100k")).unwrap(), Some(102_400));
        assert_eq!(
            memory_to_bytes(Some("2gi")).unwrap(),
            Some(2 * 1024 * 1024 * 1024)
        );
        // the Java implementation throws for these
        assert!(memory_to_bytes(Some("1024")).is_err());
        assert!(memory_to_bytes(Some("2b")).is_err());
        assert!(memory_to_bytes(Some("1,5g")).is_err());
        assert!(memory_to_bytes(Some("abc")).is_err());
    }

    #[test]
    fn converts_cpu_limits_like_java() {
        assert_eq!(cpu_quota(100_000, "2").unwrap(), 200_000);
        assert_eq!(cpu_quota(100_000, "0.5").unwrap(), 50_000);
        assert_eq!(cpu_quota(100_000, "500m").unwrap(), 50_000);
        assert!(cpu_quota(100_000, "many").is_err());
    }

    #[test]
    fn names_containers_like_java() {
        assert_eq!(
            container_name("abc", 0, None),
            "sp-container-abc-0".to_string()
        );
        assert_eq!(container_name("abc", 2, Some("")), "sp-container-abc-2");
        assert_eq!(container_name("abc", 0, Some("my-app")), "my-app");
    }

    #[test]
    fn parses_the_image_pull_policy() {
        assert_eq!(
            ImagePullPolicy::parse(None).unwrap(),
            ImagePullPolicy::IfNotPresent
        );
        assert_eq!(
            ImagePullPolicy::parse(Some("Always")).unwrap(),
            ImagePullPolicy::Always
        );
        assert_eq!(
            ImagePullPolicy::parse(Some("never")).unwrap(),
            ImagePullPolicy::Never
        );
        assert_eq!(
            ImagePullPolicy::parse(Some("if-not-present")).unwrap(),
            ImagePullPolicy::IfNotPresent
        );
        assert!(ImagePullPolicy::parse(Some("sometimes")).is_err());
    }

    fn container_spec() -> ContainerSpec {
        ContainerSpec {
            image: SpelString::resolved("x".into(), "openanalytics/shinyproxy-demo".into()),
            cmd: SpelStringList::resolved(
                vec!["R".to_string(), "-e".to_string()],
                vec!["R".to_string(), "-e".to_string()],
            ),
            port_mapping: vec![PortMapping {
                name: "default".to_string(),
                port: Some(3838),
                target_path: SpelString::resolved(String::new(), String::new()),
            }],
            memory_request: SpelString::resolved("1g".into(), "1g".into()),
            memory_limit: SpelString::resolved("2g".into(), "2g".into()),
            cpu_limit: SpelString::resolved("2".into(), "2".into()),
            volumes: SpelStringList::resolved(
                vec!["/tmp:/tmp".to_string()],
                vec!["/tmp:/tmp".to_string()],
            ),
            dns: SpelStringList::resolved(vec!["8.8.8.8".to_string()], vec!["8.8.8.8".to_string()]),
            docker_user: SpelString::resolved("1000:1000".into(), "1000:1000".into()),
            docker_ipc: SpelString::resolved("shareable".into(), "shareable".into()),
            docker_runtime: SpelString::resolved("nvidia".into(), "nvidia".into()),
            docker_group_add: SpelStringList::resolved(
                vec!["users".to_string()],
                vec!["users".to_string()],
            ),
            docker_device_requests: vec![DockerDeviceRequest {
                driver: Some("nvidia".to_string()),
                count: Some(1),
                device_ids: vec![],
                capabilities: vec![vec!["gpu".to_string()]],
                options: BTreeMap::new(),
            }],
            labels: SpelStringMap::resolved(
                BTreeMap::new(),
                BTreeMap::from([("my.label".to_string(), "value".to_string())]),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn builds_the_container_create_request_like_java() {
        let spec = {
            let mut spec = ProxySpec::new("01_hello");
            spec.container_specs = vec![container_spec()];
            spec
        };
        let container_spec = container_spec();
        let proxy = Proxy::new("proxy-1", ProxyStatus::New);
        let container = Container::new(0);
        let context = StartContext {
            user: None,
            proxy: &proxy,
            spec: &spec,
            container_spec: &container_spec,
            container: &container,
            environment: BTreeMap::from([("SHINYPROXY_USERNAME".to_string(), "jack".to_string())]),
            labels: BTreeMap::from([(
                "openanalytics.eu/sp-proxy-id".to_string(),
                "proxy-1".to_string(),
            )]),
        };
        let bindings = BTreeMap::from([(3838, 20000u16)]);

        let body = build_container_create_body(&config(), &context, &bindings).expect("body");
        assert_eq!(body.image.as_deref(), Some("openanalytics/shinyproxy-demo"));
        assert_eq!(
            body.cmd,
            Some(vec!["R".to_string(), "-e".to_string()]),
            "the command is passed as is"
        );
        assert_eq!(body.env, Some(vec!["SHINYPROXY_USERNAME=jack".to_string()]));
        assert_eq!(body.user.as_deref(), Some("1000:1000"));

        let labels = body.labels.expect("labels");
        assert_eq!(labels.get("my.label").map(String::as_str), Some("value"));
        assert_eq!(
            labels
                .get("openanalytics.eu/sp-proxy-id")
                .map(String::as_str),
            Some("proxy-1"),
            "runtime values are added as labels"
        );

        assert!(body.exposed_ports.expect("exposed").contains_key("3838"));

        let host = body.host_config.expect("host config");
        assert_eq!(
            host.port_bindings.as_ref().unwrap().get("3838"),
            Some(&Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("20000".to_string()),
            }]))
        );
        assert_eq!(host.memory_reservation, Some(1024 * 1024 * 1024));
        assert_eq!(host.memory, Some(2 * 1024 * 1024 * 1024));
        assert_eq!(host.cpu_period, Some(100_000));
        assert_eq!(host.cpu_quota, Some(200_000));
        assert_eq!(host.binds, Some(vec!["/tmp:/tmp".to_string()]));
        assert_eq!(host.dns, Some(vec!["8.8.8.8".to_string()]));
        assert_eq!(host.ipc_mode.as_deref(), Some("shareable"));
        assert_eq!(host.runtime.as_deref(), Some("nvidia"));
        assert_eq!(host.group_add, Some(vec!["users".to_string()]));
        assert_eq!(host.privileged, Some(false));
        let requests = host.device_requests.expect("device requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].driver.as_deref(), Some("nvidia"));
        assert_eq!(requests[0].count, Some(1));
        assert_eq!(
            requests[0].capabilities,
            Some(vec![vec!["gpu".to_string()]])
        );
        assert!(host.log_config.is_none(), "no loki url configured");
    }

    #[test]
    fn uses_the_configured_network_and_loki_endpoint() {
        let mut config = config();
        config.container_network = "sp-net".to_string().into();
        config.loki_url = Some("http://loki:3100/loki/api/v1/push".to_string());
        config.realm_id = Some("realm-1".to_string());
        config.privileged = true;

        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let proxy = Proxy::new("proxy-2", ProxyStatus::New);
        let container = Container::new(0);
        let context = StartContext {
            user: None,
            proxy: &proxy,
            spec: &spec,
            container_spec: &container_spec,
            container: &container,
            environment: BTreeMap::new(),
            labels: BTreeMap::new(),
        };

        let body = build_container_create_body(&config, &context, &BTreeMap::new()).expect("body");
        let host = body.host_config.expect("host config");
        assert_eq!(host.network_mode.as_deref(), Some("sp-net"));
        assert_eq!(host.privileged, Some(true));
        let log_config = host.log_config.expect("log config");
        assert_eq!(log_config.typ.as_deref(), Some("loki"));
        let options = log_config.config.expect("options");
        assert_eq!(
            options.get("loki-url").map(String::as_str),
            Some("http://loki:3100/loki/api/v1/push")
        );
        assert_eq!(
            options.get("mode").map(String::as_str),
            Some("non-blocking")
        );
        assert_eq!(
            options.get("loki-external-labels").map(String::as_str),
            Some("sp_realm_id=realm-1,namespace=default,sp_proxy_id=proxy-2")
        );
        assert!(
            host.port_bindings.unwrap().is_empty(),
            "no ports are published without allocations"
        );
    }

    #[test]
    fn the_app_network_wins_over_the_default_network() {
        let mut config = config();
        config.container_network = Some("default-net".to_string());
        let mut container_spec = container_spec();
        container_spec.network = SpelString::resolved("app-net".into(), "app-net".into());
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let proxy = Proxy::new("proxy-3", ProxyStatus::New);
        let container = Container::new(0);
        let context = StartContext {
            user: None,
            proxy: &proxy,
            spec: &spec,
            container_spec: &container_spec,
            container: &container,
            environment: BTreeMap::new(),
            labels: BTreeMap::new(),
        };

        let body = build_container_create_body(&config, &context, &BTreeMap::new()).expect("body");
        assert_eq!(
            body.host_config.unwrap().network_mode.as_deref(),
            Some("app-net")
        );
    }

    #[test]
    fn reads_the_configuration_from_the_settings() {
        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  docker:\n    url: tcp://docker:2375\n    internal-networking: true\n    \
             image-pull-policy: Always\n    target-url: https://docker-host\n    \
             default-container-network: sp-net\n    privileged: true\n",
        )
        .expect("settings");
        let config = DockerConfig::from_settings(&settings, Some("realm".into())).expect("config");
        assert_eq!(config.url.as_deref(), Some("tcp://docker:2375"));
        assert!(config.internal_networking);
        assert_eq!(config.image_pull_policy, ImagePullPolicy::Always);
        assert_eq!(config.target_protocol, "https");
        assert_eq!(config.target_host, "docker-host");
        assert_eq!(config.container_network.as_deref(), Some("sp-net"));
        assert!(config.privileged);

        // defaults
        let config = DockerConfig::from_settings(&Settings::default(), None).expect("config");
        assert_eq!(config.target_protocol, "http");
        assert_eq!(config.target_host, "localhost");
        assert_eq!(config.target_bind_ip, "127.0.0.1");
        assert_eq!(config.image_pull_policy, ImagePullPolicy::IfNotPresent);
        assert!(!config.internal_networking);
    }
}
