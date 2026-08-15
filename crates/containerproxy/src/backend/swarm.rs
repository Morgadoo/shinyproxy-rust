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

//! The `docker-swarm` container backend: apps run as Swarm services.
//!
//! Port of `DockerSwarmBackend`. One service with one task per container: the service is named
//! `sp-service-{proxyId}-{index}`, publishes the allocated host ports, has `restart-policy: none` and
//! carries the same labels as the Docker backend. The backend waits until the task of the service runs
//! (`proxy.docker.service-wait-time`, 60 seconds by default) and reads the container id from the task, so
//! that the rest of the engine works exactly as with plain Docker.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::query_parameters::{ListServicesOptionsBuilder, ListTasksOptionsBuilder};
use bollard::secret::{
    EndpointPortConfig, EndpointPortConfigPublishModeEnum, EndpointSpec, Limit, Mount,
    MountTypeEnum, NetworkAttachmentConfig, ResourceObject, ServiceSpec, TaskSpec,
    TaskSpecContainerSpec as SwarmContainerSpec, TaskSpecContainerSpecDnsConfig,
    TaskSpecContainerSpecFile as SecretFile, TaskSpecContainerSpecSecrets as ContainerSpecSecret,
    TaskSpecResources, TaskSpecRestartPolicy,
    TaskSpecRestartPolicyConditionEnum as RestartPolicyConditionEnum,
};
use bollard::Docker;

use super::docker::{cpu_quota, memory_to_bytes, DockerBackend, DockerConfig};
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

/// Name of the backend.
pub const NAME: &str = "docker-swarm";

/// One CPU in nano CPUs, the unit Swarm uses for reservations and limits.
const NANO_CPUS: i64 = 1_000_000_000;

/// Task states that mean "the task is on its way up" (same list as in Java).
const STARTING_STATES: &[&str] = &[
    "new",
    "pending",
    "assigned",
    "accepted",
    "ready",
    "preparing",
    "starting",
    "running",
];

/// Runs apps as Docker Swarm services.
#[derive(Debug)]
pub struct SwarmBackend {
    /// The Docker backend, which owns the client, the configuration and the port allocator.
    docker: DockerBackend,
    /// How long to wait for the task of a service to run (`proxy.docker.service-wait-time`).
    service_wait_time: Duration,
}

impl SwarmBackend {
    /// Connects to the Swarm manager.
    pub fn new(
        config: DockerConfig,
        settings: &Settings,
        port_allocator: Arc<PortAllocator>,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Result<Self, BackendError> {
        let docker = DockerBackend::new(config, port_allocator, registry)?;
        Ok(SwarmBackend {
            docker,
            service_wait_time: service_wait_time(settings),
        })
    }

    /// Creates the backend around an existing client (used by tests).
    pub fn with_client(
        client: Docker,
        config: DockerConfig,
        settings: &Settings,
        port_allocator: Arc<PortAllocator>,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Self {
        SwarmBackend {
            docker: DockerBackend::with_client(client, config, port_allocator, registry),
            service_wait_time: service_wait_time(settings),
        }
    }

    /// Checks that the daemon is part of a Swarm, as the Java implementation does at startup.
    pub async fn check_swarm(&self) -> Result<String, BackendError> {
        let swarm = self
            .docker
            .client()
            .inspect_swarm()
            .await
            .map_err(|error| {
                BackendError::Backend(format!("Backend is not a Docker Swarm: {error}"))
            })?;
        swarm
            .id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| BackendError::Backend("Backend is not a Docker Swarm".to_string()))
    }

    /// The name of the service of a container.
    fn service_name(&self, proxy: &Proxy, index: i64, resource_name: Option<&str>) -> String {
        match resource_name.filter(|name| !name.is_empty()) {
            Some(name) => name.to_string(),
            None => format!("sp-service-{}-{index}", proxy.id),
        }
    }

    /// Looks up the id of a secret by name.
    async fn secret_id(&self, name: &str) -> Result<String, BackendError> {
        let secrets = self
            .docker
            .client()
            .list_secrets(None::<bollard::query_parameters::ListSecretsOptions>)
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot list secrets: {error}"))
            })?;
        secrets
            .into_iter()
            .find(|secret| {
                secret
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.name.as_deref())
                    .map(|secret_name| secret_name == name)
                    .unwrap_or(false)
            })
            .and_then(|secret| secret.id)
            .ok_or_else(|| BackendError::FailedToStart("Secret not found!".to_string()))
    }

    /// The task of a service, if it has one.
    async fn service_task(
        &self,
        service_name: &str,
    ) -> Result<Option<bollard::secret::Task>, BackendError> {
        let mut filters = HashMap::new();
        filters.insert("service".to_string(), vec![service_name.to_string()]);
        let options = ListTasksOptionsBuilder::new().filters(&filters).build();
        let tasks = self
            .docker
            .client()
            .list_tasks(Some(options))
            .await
            .map_err(|error| {
                BackendError::Backend(format!("cannot list tasks of {service_name}: {error}"))
            })?;
        Ok(tasks.into_iter().next())
    }
}

/// The configured service wait time (default 60 seconds, like Java).
fn service_wait_time(settings: &Settings) -> Duration {
    let millis = settings
        .proxy
        .docker
        .service_wait_time
        .map(|value| value.0)
        .filter(|value| *value > 0)
        .unwrap_or(60_000);
    Duration::from_millis(millis as u64)
}

/// Builds the specification of the service of a container.
///
/// Separated from the request so that it can be asserted in unit tests without a Swarm.
pub fn build_service_spec(
    config: &DockerConfig,
    context: &StartContext<'_>,
    service_name: &str,
    published_ports: &BTreeMap<i64, u16>,
    secrets: Vec<ContainerSpecSecret>,
) -> Result<ServiceSpec, BackendError> {
    let spec = context.container_spec;
    let failed = BackendError::FailedToStart;

    // volumes become bind mounts (`source:target`)
    let mounts: Vec<Mount> = spec
        .volumes
        .value()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|volume| {
            let mut parts = volume.split(':');
            let source = parts.next()?.to_string();
            let target = parts.next()?.to_string();
            Some(Mount {
                source: Some(source),
                target: Some(target),
                typ: Some(MountTypeEnum::BIND),
                ..Default::default()
            })
        })
        .collect();

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

    let dns = spec.dns.value().cloned().unwrap_or_default();
    let container_spec = SwarmContainerSpec {
        image: Some(spec.image.as_str().unwrap_or_default().to_string()),
        labels: Some(labels),
        command: spec.cmd.value().cloned().filter(|cmd| !cmd.is_empty()),
        env: Some(environment),
        dns_config: Some(TaskSpecContainerSpecDnsConfig {
            nameservers: if dns.is_empty() { None } else { Some(dns) },
            ..Default::default()
        }),
        mounts: Some(mounts),
        secrets: Some(secrets),
        user: spec
            .docker_user
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        ..Default::default()
    };

    // the networks of the app plus the configured default network
    let mut networks: Vec<NetworkAttachmentConfig> = spec
        .network_connections
        .value()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|network| NetworkAttachmentConfig {
            target: Some(network),
            ..Default::default()
        })
        .collect();
    if let Some(network) = spec
        .network
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| config.container_network.clone())
    {
        networks.push(NetworkAttachmentConfig {
            target: Some(network),
            ..Default::default()
        });
    }

    // reservations are used by the scheduler, limits by the container runtime
    let mut reservations = ResourceObject::default();
    if let Some(cpu) = spec.cpu_request.as_str().filter(|value| !value.is_empty()) {
        reservations.nano_cpus = Some(cpu_quota(NANO_CPUS, cpu).map_err(failed)?);
    }
    if let Some(memory) = memory_to_bytes(spec.memory_request.as_str()).map_err(failed)? {
        reservations.memory_bytes = Some(memory);
    }
    let mut limits = Limit::default();
    if let Some(cpu) = spec.cpu_limit.as_str().filter(|value| !value.is_empty()) {
        limits.nano_cpus = Some(cpu_quota(NANO_CPUS, cpu).map_err(failed)?);
    }
    if let Some(memory) = memory_to_bytes(spec.memory_limit.as_str()).map_err(failed)? {
        limits.memory_bytes = Some(memory);
    }

    let mut service_spec = ServiceSpec {
        name: Some(service_name.to_string()),
        task_template: Some(TaskSpec {
            container_spec: Some(container_spec),
            restart_policy: Some(TaskSpecRestartPolicy {
                condition: Some(RestartPolicyConditionEnum::NONE),
                ..Default::default()
            }),
            networks: Some(networks),
            resources: Some(TaskSpecResources {
                limits: Some(limits),
                reservations: Some(reservations),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    if !published_ports.is_empty() {
        service_spec.endpoint_spec = Some(EndpointSpec {
            ports: Some(
                published_ports
                    .iter()
                    .map(|(target_port, published_port)| EndpointPortConfig {
                        target_port: Some(*target_port),
                        published_port: Some(*published_port as i64),
                        publish_mode: Some(EndpointPortConfigPublishModeEnum::INGRESS),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        });
    }

    Ok(service_spec)
}

/// Converts a `docker-swarm-secrets` entry into a secret bind, like `convertSecret` in Java.
pub fn build_secret(
    secret: &crate::model::spec::DockerSwarmSecret,
    secret_id: String,
) -> Result<ContainerSpecSecret, BackendError> {
    let Some(name) = secret.name.clone().filter(|name| !name.is_empty()) else {
        return Err(BackendError::FailedToStart(
            "No name for a Docker swarm secret provided".to_string(),
        ));
    };
    // the mode is an octal string, as in the Java implementation (default 444)
    let mode = i64::from_str_radix(secret.mode.as_deref().unwrap_or("444"), 8).map_err(|_| {
        BackendError::FailedToStart(format!(
            "invalid mode '{}' for Docker swarm secret {name}",
            secret.mode.clone().unwrap_or_default()
        ))
    })?;
    Ok(ContainerSpecSecret {
        secret_name: Some(name.clone()),
        secret_id: Some(secret_id),
        file: Some(SecretFile {
            name: Some(secret.target.clone().unwrap_or(name)),
            uid: Some(secret.uid.clone().unwrap_or_else(|| "0".to_string())),
            gid: Some(secret.gid.clone().unwrap_or_else(|| "0".to_string())),
            mode: Some(mode as u32),
        }),
    })
}

#[async_trait]
impl ContainerBackend for SwarmBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports_health_check(&self) -> bool {
        true
    }

    async fn initialize(&self) -> Result<(), BackendError> {
        // as in Java: a daemon that is not part of a swarm is a fatal configuration error
        let swarm_id = self.check_swarm().await?;
        tracing::info!("Using Docker Swarm {swarm_id}");
        Ok(())
    }

    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        let spec = context.container_spec;
        let config = self.docker.config().clone();
        let proxy_id = context.proxy.id.clone();
        let service_name = self.service_name(
            context.proxy,
            context.container.index,
            spec.resource_name.as_str(),
        );

        // published ports (not needed on an internal network)
        let mut published_ports: BTreeMap<i64, u16> = BTreeMap::new();
        if !config.internal_networking {
            for mapping in &spec.port_mapping {
                let Some(container_port) = mapping.port else {
                    continue;
                };
                let host_port = self
                    .docker
                    .port_allocator()
                    .allocate(&proxy_id)
                    .map_err(|error| BackendError::FailedToStart(error.to_string()))?;
                published_ports.insert(container_port, host_port);
            }
        }

        // secrets are referenced by id, so they are looked up first
        let mut secrets = Vec::new();
        for secret in &spec.docker_swarm_secrets {
            let name = secret.name.clone().unwrap_or_default();
            let id = self.secret_id(&name).await?;
            secrets.push(build_secret(secret, id)?);
        }

        let service_spec =
            build_service_spec(&config, &context, &service_name, &published_ports, secrets)?;

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

        let created = self
            .docker
            .client()
            .create_service(service_spec, credentials)
            .await
            .map_err(|error| {
                self.docker.port_allocator().release(&proxy_id);
                BackendError::FailedToStart(format!(
                    "cannot create service {service_name}: {error}"
                ))
            })?;
        let service_id = created.id.unwrap_or_else(|| service_name.clone());

        // wait until the task of the service runs, polling every 10th of the wait time (as Java does)
        let deadline = std::time::Instant::now() + self.service_wait_time;
        let interval = self.service_wait_time / 10;
        let mut container_id = None;
        let mut failure = None;
        while std::time::Instant::now() < deadline {
            match self.service_task(&service_name).await {
                Ok(Some(task)) => {
                    let status = task.status.clone().unwrap_or_default();
                    let state = status
                        .state
                        .map(|state| state.to_string())
                        .unwrap_or_default();
                    let id = status
                        .container_status
                        .and_then(|status| status.container_id);
                    if state == "running" && id.is_some() {
                        container_id = id;
                        break;
                    }
                    if !STARTING_STATES.contains(&state.as_str()) {
                        failure = Some(format!(
                            "Docker Swarm container failed: container not running, \
                             state reported by docker: {state}"
                        ));
                        break;
                    }
                }
                Ok(None) => {} // the service has no task yet
                Err(error) => tracing::debug!("cannot check the task of {service_name}: {error}"),
            }
            tokio::time::sleep(interval).await;
        }

        let Some(container_id) = container_id else {
            // clean up the service, so that a failed start leaves nothing behind
            let _ = self.docker.client().delete_service(&service_id).await;
            self.docker.port_allocator().release(&proxy_id);
            return Err(BackendError::FailedToStart(failure.unwrap_or_else(|| {
                "Swarm container did not start in time".to_string()
            })));
        };

        // where the proxy sends the requests to
        let mut targets = BTreeMap::new();
        for mapping in &spec.port_mapping {
            let Some(container_port) = mapping.port else {
                continue;
            };
            let (hostname, port) = if config.internal_networking {
                // the short container id is a resolvable name inside the overlay network
                (
                    container_id.chars().take(12).collect::<String>(),
                    container_port as u16,
                )
            } else {
                (
                    config.target_host.clone(),
                    *published_ports.get(&container_port).ok_or_else(|| {
                        BackendError::FailedToStart(format!(
                            "no host port was published for container port {container_port}"
                        ))
                    })?,
                )
            };
            let target_path = compute_target_path(mapping.target_path.as_str());
            targets.insert(
                mapping_key_to_path(&mapping.name),
                target_url(&config.target_protocol, &hostname, port, &target_path),
            );
        }

        let mut runtime_values = RuntimeValues::new();
        runtime_values.add(
            RuntimeValue::json(
                &BACKEND_CONTAINER_NAME,
                BackendContainerName::new(&service_name),
            ),
            true,
        );

        Ok(StartedContainer {
            id: Some(container_id),
            runtime_values,
            targets,
        })
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        for container in &proxy.containers {
            let Some(name) = backend_container_name(container) else {
                continue;
            };
            match self.docker.client().delete_service(&name).await {
                Ok(()) => {}
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    // the service is already removed
                }
                Err(error) => {
                    return Err(BackendError::Backend(format!(
                        "cannot remove service {name}: {error}"
                    )))
                }
            }
        }
        self.docker.port_allocator().release(&proxy.id);
        Ok(())
    }

    async fn is_proxy_healthy(&self, proxy: &Proxy) -> Result<bool, BackendError> {
        // the Java implementation answers based on the first container as well
        if let Some(container) = proxy.containers.first() {
            let Some(name) = backend_container_name(container) else {
                tracing::warn!(
                    "Docker Swarm container failed: no service id found [proxyId: {}]",
                    proxy.id
                );
                return Ok(false);
            };
            match self.service_task(&name).await {
                Ok(Some(task)) => {
                    let state = task
                        .status
                        .and_then(|status| status.state)
                        .map(|state| state.to_string())
                        .unwrap_or_default();
                    if state != "running" {
                        tracing::warn!(
                            "Docker Swarm container failed: container not running, \
                             state reported by docker: {state} [proxyId: {}]",
                            proxy.id
                        );
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Ok(None) => {
                    tracing::warn!(
                        "Docker Swarm container failed: service does not exist [proxyId: {}]",
                        proxy.id
                    );
                    return Ok(false);
                }
                Err(error) => {
                    tracing::warn!(
                        "Failed to check Docker Swarm container health [proxyId: {}]: {error}",
                        proxy.id
                    );
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    async fn scan_existing_containers(&self) -> Result<Vec<ExistingContainerInfo>, BackendError> {
        let services = self
            .docker
            .client()
            .list_services(None::<ListServicesOptionsBuilder>.map(|builder| builder.build()))
            .await
            .map_err(|error| BackendError::Backend(format!("cannot list services: {error}")))?;

        let mut existing = Vec::new();
        for service in services {
            let service_id = service.id.clone().unwrap_or_default();
            let Some(container_spec) = service
                .spec
                .as_ref()
                .and_then(|spec| spec.task_template.as_ref())
                .and_then(|template| template.container_spec.as_ref())
            else {
                continue;
            };

            // the container of the service, which carries the id the rest of the engine uses
            let mut filters = HashMap::new();
            filters.insert(
                "label".to_string(),
                vec![format!("com.docker.swarm.service.id={service_id}")],
            );
            let options = bollard::query_parameters::ListContainersOptionsBuilder::new()
                .filters(&filters)
                .build();
            let containers = self
                .docker
                .client()
                .list_containers(Some(options))
                .await
                .map_err(|error| {
                    BackendError::Backend(format!(
                        "cannot list containers of {service_id}: {error}"
                    ))
                })?;
            if containers.len() != 1 {
                tracing::warn!(
                    "Found not correct amount of containers for service {service_id}, \
                     therefore skipping this"
                );
                continue;
            }
            let container = &containers[0];
            let container_id = container.id.clone().unwrap_or_default();

            let labels: BTreeMap<String, String> = container_spec
                .labels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            let Some(mut runtime_values) = self.docker.registry().parse_labels(
                labels
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            ) else {
                continue;
            };
            if let Some(image) = &container.image {
                runtime_values.add(RuntimeValue::string(&CONTAINER_IMAGE, image.clone()), true);
            }
            runtime_values.add(
                RuntimeValue::json(
                    &BACKEND_CONTAINER_NAME,
                    BackendContainerName::new(&service_id),
                ),
                true,
            );

            let mut port_bindings = BTreeMap::new();
            for port in service
                .endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.ports.clone())
                .unwrap_or_default()
            {
                let (Some(published), Some(target)) = (port.published_port, port.target_port)
                else {
                    continue;
                };
                port_bindings.insert(target as u16, published as u16);
                self.docker
                    .port_allocator()
                    .add_existing_port(&container_id, published as u16);
            }

            existing.push(ExistingContainerInfo {
                id: container_id,
                runtime_values,
                image: container_spec.image.clone(),
                port_bindings,
            });
        }
        Ok(existing)
    }
}

/// The name of the service of a container, as stored in its runtime values.
fn backend_container_name(container: &crate::model::proxy::Container) -> Option<String> {
    container
        .runtime_values
        .get(&BACKEND_CONTAINER_NAME)
        .and_then(|value| value.data.parse_json::<BackendContainerName>())
        .map(|name| name.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Container, ProxyStatus};
    use crate::model::spec::{ContainerSpec, DockerSwarmSecret, PortMapping, ProxySpec};
    use crate::model::spel_field::{SpelString, SpelStringList, SpelStringMap};

    fn config() -> DockerConfig {
        DockerConfig::from_settings(&Settings::default(), Some("realm".into())).expect("config")
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
            cpu_request: SpelString::resolved("1".into(), "1".into()),
            cpu_limit: SpelString::resolved("2".into(), "2".into()),
            volumes: SpelStringList::resolved(
                vec!["/host/data:/data".to_string()],
                vec!["/host/data:/data".to_string()],
            ),
            dns: SpelStringList::resolved(vec!["8.8.8.8".to_string()], vec!["8.8.8.8".to_string()]),
            docker_user: SpelString::resolved("1000".into(), "1000".into()),
            labels: SpelStringMap::resolved(
                BTreeMap::new(),
                BTreeMap::from([("my.label".to_string(), "value".to_string())]),
            ),
            network_connections: SpelStringList::resolved(
                vec!["extra-net".to_string()],
                vec!["extra-net".to_string()],
            ),
            ..Default::default()
        }
    }

    #[test]
    fn builds_the_service_specification_like_java() {
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
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

        let mut config = config();
        config.container_network = Some("sp-net".to_string());
        let ports = BTreeMap::from([(3838, 20000u16)]);
        let service = build_service_spec(
            &config,
            &context,
            "sp-service-proxy-1-0",
            &ports,
            Vec::new(),
        )
        .expect("service spec");

        assert_eq!(service.name.as_deref(), Some("sp-service-proxy-1-0"));
        let template = service.task_template.expect("task template");
        assert_eq!(
            template.restart_policy.expect("restart policy").condition,
            Some(RestartPolicyConditionEnum::NONE),
            "swarm must not restart apps"
        );

        let networks: Vec<String> = template
            .networks
            .expect("networks")
            .into_iter()
            .filter_map(|network| network.target)
            .collect();
        assert_eq!(
            networks,
            vec!["extra-net".to_string(), "sp-net".to_string()],
            "network-connections first, then the container network"
        );

        let resources = template.resources.expect("resources");
        let reservations = resources.reservations.expect("reservations");
        assert_eq!(reservations.nano_cpus, Some(1_000_000_000));
        assert_eq!(reservations.memory_bytes, Some(1024 * 1024 * 1024));
        let limits = resources.limits.expect("limits");
        assert_eq!(limits.nano_cpus, Some(2_000_000_000));
        assert_eq!(limits.memory_bytes, Some(2 * 1024 * 1024 * 1024));

        let container_spec = template.container_spec.expect("container spec");
        assert_eq!(
            container_spec.image.as_deref(),
            Some("openanalytics/shinyproxy-demo")
        );
        assert_eq!(
            container_spec.command,
            Some(vec!["R".to_string(), "-e".to_string()])
        );
        assert_eq!(
            container_spec.env,
            Some(vec!["SHINYPROXY_USERNAME=jack".to_string()])
        );
        assert_eq!(container_spec.user.as_deref(), Some("1000"));
        let labels = container_spec.labels.expect("labels");
        assert_eq!(labels.get("my.label").map(String::as_str), Some("value"));
        assert_eq!(
            labels
                .get("openanalytics.eu/sp-proxy-id")
                .map(String::as_str),
            Some("proxy-1")
        );
        assert_eq!(
            container_spec.dns_config.expect("dns config").nameservers,
            Some(vec!["8.8.8.8".to_string()])
        );
        let mounts = container_spec.mounts.expect("mounts");
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].source.as_deref(), Some("/host/data"));
        assert_eq!(mounts[0].target.as_deref(), Some("/data"));
        assert_eq!(mounts[0].typ, Some(MountTypeEnum::BIND));

        let endpoint = service.endpoint_spec.expect("endpoint spec");
        let ports = endpoint.ports.expect("ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].target_port, Some(3838));
        assert_eq!(ports[0].published_port, Some(20000));
    }

    #[test]
    fn publishes_no_ports_on_an_internal_network() {
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
        let service = build_service_spec(
            &config(),
            &context,
            "sp-service-proxy-2-0",
            &BTreeMap::new(),
            Vec::new(),
        )
        .expect("service spec");
        assert!(service.endpoint_spec.is_none());
    }

    #[test]
    fn converts_secrets_like_java() {
        let secret = DockerSwarmSecret {
            name: Some("my-secret".to_string()),
            target: None,
            uid: None,
            gid: None,
            mode: None,
        };
        let bind = build_secret(&secret, "secret-id".to_string()).expect("secret");
        assert_eq!(bind.secret_name.as_deref(), Some("my-secret"));
        assert_eq!(bind.secret_id.as_deref(), Some("secret-id"));
        let file = bind.file.expect("file");
        assert_eq!(file.name.as_deref(), Some("my-secret"));
        assert_eq!(file.uid.as_deref(), Some("0"));
        assert_eq!(file.gid.as_deref(), Some("0"));
        assert_eq!(file.mode, Some(0o444));

        let secret = DockerSwarmSecret {
            name: Some("my-secret".to_string()),
            target: Some("/run/secrets/token".to_string()),
            uid: Some("1000".to_string()),
            gid: Some("1000".to_string()),
            mode: Some("600".to_string()),
        };
        let file = build_secret(&secret, "id".to_string())
            .expect("secret")
            .file
            .expect("file");
        assert_eq!(file.name.as_deref(), Some("/run/secrets/token"));
        assert_eq!(file.uid.as_deref(), Some("1000"));
        assert_eq!(file.mode, Some(0o600));

        // a secret without a name is an error, as in Java
        let secret = DockerSwarmSecret::default();
        assert!(build_secret(&secret, "id".to_string()).is_err());
    }

    #[test]
    fn names_services_like_java() {
        let settings = Settings::default();
        assert_eq!(service_wait_time(&settings), Duration::from_millis(60_000));
        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  docker:\n    service-wait-time: 30000\n")
                .expect("settings");
        assert_eq!(service_wait_time(&settings), Duration::from_millis(30_000));

        // the service name helper mirrors the container name helper of the docker backend
        let proxy = Proxy::new("abc", ProxyStatus::New);
        let backend = |resource: Option<&str>| match resource.filter(|name| !name.is_empty()) {
            Some(name) => name.to_string(),
            None => format!("sp-service-{}-0", proxy.id),
        };
        assert_eq!(backend(None), "sp-service-abc-0");
        assert_eq!(backend(Some("my-service")), "my-service");
    }
}
