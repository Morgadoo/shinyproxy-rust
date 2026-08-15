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

//! The `ecs` container backend: apps run as Fargate tasks.
//!
//! Port of `EcsBackend`. One task per container: a task definition is registered for the proxy
//! (`sp-task-definition-{proxyId}`), the task runs in the configured subnets and security groups, and the app
//! is reached on the private IP of its network interface. Stopping an app stops the task and removes the task
//! definition.
//!
//! The requests that go to AWS are built by [`task`], which is pure and covered by unit tests; a real
//! cluster is needed to validate the backend end to end, which is documented in `docs/COMPATIBILITY.md`.

pub mod config;
pub mod task;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_ecs::types::{
    AwsVpcConfiguration, LaunchType, NetworkConfiguration, PropagateTags, Task,
};
use aws_sdk_ecs::Client;

pub use config::EcsConfig;

use super::target::{compute_target_path, mapping_key_to_path, target_url};
use super::{
    BackendError, ContainerBackend, ExistingContainerInfo, StartContext, StartedContainer,
};
use crate::model::proxy::Proxy;
use crate::model::runtime_value::{
    BackendContainerName, RuntimeValue, RuntimeValueRegistry, RuntimeValues,
    BACKEND_CONTAINER_NAME, CONTAINER_IMAGE,
};
use crate::model::spec::ProxySpec;

/// Name of the backend.
pub const NAME: &str = "ecs";

/// States a task goes through while it starts.
const STARTING_STATES: [&str; 3] = ["PROVISIONING", "PENDING", "ACTIVATING"];

/// States a task goes through while it stops.
const STOPPING_STATES: [&str; 5] = [
    "DEACTIVATING",
    "STOPPING",
    "DEPROVISIONING",
    "STOPPED",
    "DELETED",
];

/// The ECS fields of an app definition (`EcsSpecExtension`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EcsSpecExtension {
    /// Role the containers of the task assume.
    pub task_role: Option<String>,
    /// Role the ECS agent uses to start the task.
    pub execution_role: Option<String>,
    /// Architecture of the task (`X86_64`, `ARM64`).
    pub cpu_architecture: Option<String>,
    /// Operating system of the task (`LINUX`, ...).
    #[serde(alias = "operation-system-family")]
    pub operating_system_family: Option<String>,
    /// Whether the root file system of the container is read only.
    pub readonly_root_filesystem: Option<bool>,
    /// Whether `aws ecs execute-command` may attach to the container.
    pub enable_execute_command: Option<bool>,
    /// Size of the ephemeral storage in GiB (21 by default).
    pub ephemeral_storage_size: Option<i64>,
    /// Parameter with the credentials of the image registry.
    pub repository_credentials_parameter: Option<String>,
    /// EFS volumes of the task.
    pub efs_volumes: Vec<EcsEfsVolume>,
    /// Volumes that are bound from the host.
    pub bind_volumes: Vec<String>,
    /// Secrets that become environment variables.
    pub managed_secrets: Vec<EcsManagedSecret>,
}

impl EcsSpecExtension {
    /// The ECS fields of an app definition.
    pub fn of(spec: &ProxySpec) -> Self {
        spec.spec_extensions.get("ecs")
    }
}

/// An EFS volume of a task (`EcsEfsVolume`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EcsEfsVolume {
    /// Name the app refers to in its `container-volumes`.
    pub name: Option<String>,
    /// The EFS file system.
    pub file_system_id: Option<String>,
    /// Directory of the file system that is mounted.
    pub root_directory: Option<String>,
    /// Whether the traffic to EFS is encrypted.
    pub transit_encryption: Option<bool>,
    /// Port used for the encrypted traffic.
    pub transit_encryption_port: Option<i64>,
    /// Access point of the file system.
    pub access_point_id: Option<String>,
    /// Whether IAM authorization is used.
    pub enable_iam: Option<bool>,
}

/// A secret that becomes an environment variable (`EcsManagedSecret`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EcsManagedSecret {
    /// Name of the environment variable.
    pub name: Option<String>,
    /// Where the value comes from (an SSM parameter or a Secrets Manager secret).
    pub value_from: Option<String>,
}

/// Runs apps as Fargate tasks.
pub struct EcsBackend {
    client: Client,
    config: EcsConfig,
    #[allow(dead_code)]
    registry: Arc<RuntimeValueRegistry>,
}

impl std::fmt::Debug for EcsBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EcsBackend")
            .field("config", &self.config)
            .finish()
    }
}

impl EcsBackend {
    /// Connects to ECS with the credentials of the environment.
    pub async fn connect(
        config: EcsConfig,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Result<Self, BackendError> {
        let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(config.region.clone()))
            .load()
            .await;
        Ok(EcsBackend {
            client: Client::new(&aws_config),
            config,
            registry,
        })
    }

    /// Creates a backend around an existing client (used by tests).
    pub fn with_client(
        client: Client,
        config: EcsConfig,
        registry: Arc<RuntimeValueRegistry>,
    ) -> Self {
        EcsBackend {
            client,
            config,
            registry,
        }
    }

    /// The configuration of this backend.
    pub fn config(&self) -> &EcsConfig {
        &self.config
    }

    /// The task of a container, when it has one.
    async fn task_of(&self, container: &crate::model::proxy::Container) -> Option<Task> {
        let arn = container
            .runtime_values
            .get(&BACKEND_CONTAINER_NAME)
            .and_then(|value| value.data.parse_json::<BackendContainerName>())
            .map(|name| name.name)?;
        let response = self
            .client
            .describe_tasks()
            .cluster(&self.config.cluster)
            .tasks(arn)
            .send()
            .await
            .ok()?;
        response.tasks().first().cloned()
    }

    /// Registers the task definition of a proxy and returns its ARN.
    ///
    /// An app whose image is a task definition ARN uses that one instead (`arn:aws:ecs:`).
    async fn register_task_definition(
        &self,
        context: &StartContext<'_>,
        extension: &EcsSpecExtension,
    ) -> Result<String, BackendError> {
        if let Some(image) = context
            .container_spec
            .image
            .as_str()
            .filter(|image| image.starts_with("arn:aws:ecs:"))
        {
            return Ok(image.to_string());
        }

        let request = task::task_definition(&self.config, context, extension)
            .map_err(BackendError::FailedToStart)?;
        let response = self
            .client
            .register_task_definition()
            .family(request.family.clone())
            .container_definitions(request.container.clone())
            .network_mode(aws_sdk_ecs::types::NetworkMode::Awsvpc)
            .requires_compatibilities(aws_sdk_ecs::types::Compatibility::Fargate)
            .cpu(request.cpu.clone())
            .memory(request.memory.clone())
            .set_task_role_arn(request.task_role.clone())
            .set_execution_role_arn(request.execution_role.clone())
            .runtime_platform(request.runtime_platform.clone())
            .ephemeral_storage(request.ephemeral_storage.clone())
            .set_volumes(Some(request.volumes.clone()))
            .set_tags(Some(request.tags.clone()))
            .send()
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot register the task definition: {error}"))
            })?;

        response
            .task_definition()
            .and_then(|definition| definition.task_definition_arn())
            .map(str::to_string)
            .ok_or_else(|| {
                BackendError::FailedToStart(
                    "the answer of ECS has no task definition ARN".to_string(),
                )
            })
    }

    /// The private IP of a task, from its network interface.
    fn task_ip(task: &Task) -> Option<String> {
        for attachment in task.attachments() {
            for detail in attachment.details() {
                let name = detail.name().unwrap_or_default();
                if name == "privateIPv4Address" || name == "privateIPv6Address" {
                    if let Some(value) = detail.value().filter(|value| !value.is_empty()) {
                        return Some(value.to_string());
                    }
                }
            }
        }
        None
    }

    /// The message of a task that is not running (used in the log, as in Java).
    fn failure_message(task: &Task) -> String {
        format!(
            "ECS container failed: task not running, stopCode: '{}', stoppingAt: '{}', \
             stoppedAt: '{}', stoppedReason: '{}'",
            task.stop_code()
                .map(|code| code.as_str().to_string())
                .unwrap_or_default(),
            task.stopping_at()
                .map(|time| time.to_string())
                .unwrap_or_default(),
            task.stopped_at()
                .map(|time| time.to_string())
                .unwrap_or_default(),
            task.stopped_reason().unwrap_or_default()
        )
    }
}

#[async_trait]
impl ContainerBackend for EcsBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports_health_check(&self) -> bool {
        true
    }

    async fn initialize(&self) -> Result<(), BackendError> {
        match self
            .client
            .describe_clusters()
            .clusters(&self.config.cluster)
            .send()
            .await
        {
            Ok(response) => {
                let found = response
                    .clusters()
                    .iter()
                    .any(|cluster| cluster.cluster_name() == Some(self.config.cluster.as_str()));
                if found {
                    tracing::info!(
                        "Using the ECS cluster {} in {}",
                        self.config.cluster,
                        self.config.region
                    );
                } else {
                    tracing::warn!(
                        "The ECS cluster {} was not found in {}",
                        self.config.cluster,
                        self.config.region
                    );
                }
            }
            Err(error) => tracing::warn!("Cannot reach ECS: {error}"),
        }
        Ok(())
    }

    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        let container_id = uuid::Uuid::new_v4().to_string();
        let extension = EcsSpecExtension::of(context.spec);
        let task_definition = self.register_task_definition(&context, &extension).await?;

        let network = NetworkConfiguration::builder()
            .awsvpc_configuration(
                AwsVpcConfiguration::builder()
                    .set_subnets(Some(self.config.subnets.clone()))
                    .set_security_groups(Some(self.config.security_groups.clone()))
                    .build()
                    .map_err(|error| {
                        BackendError::FailedToStart(format!(
                            "invalid network configuration: {error}"
                        ))
                    })?,
            )
            .build();

        let response = self
            .client
            .run_task()
            .cluster(&self.config.cluster)
            .count(1)
            .task_definition(task_definition)
            .propagate_tags(PropagateTags::TaskDefinition)
            .network_configuration(network)
            .launch_type(LaunchType::Fargate)
            .enable_execute_command(extension.enable_execute_command.unwrap_or(false))
            .set_tags(Some(task::tags(&context)))
            .send()
            .await
            .map_err(|error| {
                BackendError::FailedToStart(format!("cannot run the ECS task: {error}"))
            })?;

        let Some(task) = response.tasks().first() else {
            return Err(BackendError::FailedToStart(
                "No task in taskResponse".to_string(),
            ));
        };
        let task_arn = task.task_arn().unwrap_or_default().to_string();

        let mut runtime_values = RuntimeValues::new();
        runtime_values.add(
            RuntimeValue::json(
                &BACKEND_CONTAINER_NAME,
                BackendContainerName::new(&task_arn),
            ),
            true,
        );

        // wait until the task runs
        let deadline = std::time::Instant::now() + self.config.service_wait_time;
        let interval = self.config.service_wait_time / 10;
        let mut running: Option<Task> = None;
        while std::time::Instant::now() < deadline {
            let response = self
                .client
                .describe_tasks()
                .cluster(&self.config.cluster)
                .tasks(&task_arn)
                .send()
                .await
                .map_err(|error| {
                    BackendError::FailedToStart(format!("cannot read the ECS task: {error}"))
                })?;
            if let Some(task) = response.tasks().first() {
                let last_status = task.last_status().unwrap_or_default();
                if last_status == "RUNNING" {
                    running = Some(task.clone());
                    break;
                }
                if !STARTING_STATES.contains(&last_status)
                    || task.desired_status().unwrap_or_default() != "RUNNING"
                {
                    tracing::warn!(
                        "{} [proxyId: {}]",
                        EcsBackend::failure_message(task),
                        context.proxy.id
                    );
                    break;
                }
            }
            tokio::time::sleep(interval).await;
        }

        let Some(task) = running else {
            return Err(BackendError::FailedToStart(
                "Service failed to start".to_string(),
            ));
        };

        if let Some(image) = task
            .containers()
            .first()
            .and_then(|container| container.image())
        {
            runtime_values.add(RuntimeValue::string(&CONTAINER_IMAGE, image), true);
        }

        // the app is reached on the private address of its network interface
        let Some(host) = EcsBackend::task_ip(&task) else {
            return Err(BackendError::FailedToStart(
                "Could not find ip in attachment".to_string(),
            ));
        };
        let mut targets = BTreeMap::new();
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

        Ok(StartedContainer {
            id: Some(container_id),
            runtime_values,
            targets,
        })
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        for container in &proxy.containers {
            let Some(arn) = container
                .runtime_values
                .get(&BACKEND_CONTAINER_NAME)
                .and_then(|value| value.data.parse_json::<BackendContainerName>())
                .map(|name| name.name)
            else {
                continue;
            };

            let _ = self
                .client
                .stop_task()
                .cluster(&self.config.cluster)
                .task(&arn)
                .send()
                .await;

            // the task definition of this proxy is not needed anymore
            let definition = format!("{}:1", task::task_definition_family(&proxy.id));
            let _ = self
                .client
                .deregister_task_definition()
                .task_definition(&definition)
                .send()
                .await;
            let _ = self
                .client
                .delete_task_definitions()
                .task_definitions(&definition)
                .send()
                .await;
        }

        // wait until the tasks are stopping, as the Java implementation does
        let deadline = std::time::Instant::now() + self.config.service_wait_time;
        while std::time::Instant::now() < deadline {
            let mut stopping = true;
            for container in &proxy.containers {
                if let Some(task) = self.task_of(container).await {
                    let desired = task.desired_status().unwrap_or_default();
                    if !STOPPING_STATES.contains(&desired) {
                        stopping = false;
                    }
                }
            }
            if stopping {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        tracing::warn!(
            "Container did not get into stopping state [proxyId: {}]",
            proxy.id
        );
        Ok(())
    }

    async fn is_proxy_healthy(&self, proxy: &Proxy) -> Result<bool, BackendError> {
        for container in &proxy.containers {
            let Some(task) = self.task_of(container).await else {
                tracing::warn!(
                    "ECS container failed: task not found [proxyId: {}]",
                    proxy.id
                );
                return Ok(false);
            };
            if task.last_status().unwrap_or_default() != "RUNNING"
                || task.desired_status().unwrap_or_default() != "RUNNING"
            {
                tracing::warn!(
                    "{} [proxyId: {}]",
                    EcsBackend::failure_message(&task),
                    proxy.id
                );
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn scan_existing_containers(&self) -> Result<Vec<ExistingContainerInfo>, BackendError> {
        // the Java implementation does not recover apps on ECS either
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ecs_fields_of_an_app() {
        let mut spec = ProxySpec::new("01_hello");
        spec.spec_extensions.insert(
            "ecs",
            serde_json::json!({
                "task-role": "arn:aws:iam::123:role/task",
                "enable-execute-command": true,
                "efs-volumes": [{"name": "home", "file-system-id": "fs-1"}],
                "bind-volumes": ["data"],
                "managed-secrets": [{"name": "TOKEN", "value-from": "arn:secret"}],
                "operation-system-family": "LINUX",
            }),
        );

        let extension = EcsSpecExtension::of(&spec);
        assert_eq!(
            extension.task_role.as_deref(),
            Some("arn:aws:iam::123:role/task")
        );
        assert_eq!(extension.enable_execute_command, Some(true));
        assert_eq!(extension.efs_volumes.len(), 1);
        assert_eq!(extension.efs_volumes[0].name.as_deref(), Some("home"));
        assert_eq!(extension.bind_volumes, vec!["data"]);
        assert_eq!(extension.managed_secrets[0].name.as_deref(), Some("TOKEN"));
        // the Java property is spelled `ecs-operation-system-family`
        assert_eq!(extension.operating_system_family.as_deref(), Some("LINUX"));

        let extension = EcsSpecExtension::of(&ProxySpec::new("other"));
        assert!(extension.task_role.is_none());
        assert!(extension.efs_volumes.is_empty());
    }

    #[test]
    fn reads_the_address_of_a_task() {
        // a task without attachments has no address
        let task = Task::builder().build();
        assert!(EcsBackend::task_ip(&task).is_none());

        let task = Task::builder()
            .attachments(
                aws_sdk_ecs::types::Attachment::builder()
                    .details(
                        aws_sdk_ecs::types::KeyValuePair::builder()
                            .name("networkInterfaceId")
                            .value("eni-1")
                            .build(),
                    )
                    .details(
                        aws_sdk_ecs::types::KeyValuePair::builder()
                            .name("privateIPv4Address")
                            .value("10.0.1.15")
                            .build(),
                    )
                    .build(),
            )
            .build();
        assert_eq!(EcsBackend::task_ip(&task).as_deref(), Some("10.0.1.15"));
    }

    #[test]
    fn describes_a_failed_task_like_java() {
        let task = Task::builder()
            .last_status("STOPPED")
            .desired_status("STOPPED")
            .stopped_reason("Essential container in task exited")
            .stop_code(aws_sdk_ecs::types::TaskStopCode::EssentialContainerExited)
            .build();
        let message = EcsBackend::failure_message(&task);
        assert!(
            message.contains("ECS container failed: task not running"),
            "{message}"
        );
        assert!(
            message.contains("stopCode: 'EssentialContainerExited'"),
            "{message}"
        );
        assert!(
            message.contains("stoppedReason: 'Essential container in task exited'"),
            "{message}"
        );
    }
}
