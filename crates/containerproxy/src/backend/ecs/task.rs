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

//! The task definitions the ECS backend registers.
//!
//! Everything here is a pure function of the configuration and the app definition, so the requests that go
//! to AWS are asserted by unit tests (the plan: the ECS backend is covered by unit tests only, because a
//! real cluster cannot be part of the test suite).

use std::collections::BTreeMap;

use aws_sdk_ecs::types::{
    ContainerDefinition, EfsAuthorizationConfig, EfsAuthorizationConfigIam, EfsTransitEncryption,
    EfsVolumeConfiguration, EphemeralStorage, KeyValuePair, LogConfiguration, LogDriver,
    MountPoint, RepositoryCredentials, RuntimePlatform, Secret, Tag, Volume,
};

use super::config::EcsConfig;
use super::EcsSpecExtension;
use crate::backend::StartContext;
use crate::model::runtime_value::{RuntimeValues, PORT_MAPPINGS, USER_GROUPS};

/// The tag that marks a task definition for deletion.
pub const TO_DELETE_TAG: (&str, &str) = ("openanalytics.eu/sp-to-delete", "true");

/// Characters that are allowed in the value of an ECS tag.
fn is_valid_tag_value(value: &str) -> bool {
    value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || " +-=._:/@".contains(character))
}

/// Removes the characters CloudWatch does not accept in a log group name.
fn sanitize_log_group(spec_id: &str) -> String {
    spec_id
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || ['_', '-', '.', '#'].contains(character)
        })
        .collect()
}

/// The name of the task definition family of a proxy.
pub fn task_definition_family(proxy_id: &str) -> String {
    format!("sp-task-definition-{proxy_id}")
}

/// The tags of a task: the runtime values that are labels or annotations, plus the labels of the app.
///
/// The port mappings and the groups of the user are left out (they are too long, or contain characters ECS
/// refuses), and a value ECS would refuse is skipped with a warning, exactly as in Java.
pub fn tags(context: &StartContext<'_>) -> Vec<Tag> {
    let mut tags = Vec::new();
    let mut add = |key: &str, value: &str| {
        if is_valid_tag_value(value) {
            tags.push(
                Tag::builder()
                    .key(key.to_string())
                    .value(value.to_string())
                    .build(),
            );
        } else {
            tracing::warn!(
                "Skipping tag {key} because its value contains characters ECS does not accept \
                 [proxyId: {}]",
                context.proxy.id
            );
        }
    };

    for values in [
        &context.proxy.runtime_values,
        &context.container.runtime_values,
    ] {
        for value in values.iter() {
            if value.key.env_var == PORT_MAPPINGS.env_var
                || value.key.env_var == USER_GROUPS.env_var
            {
                continue;
            }
            if value.key.include_as_label || value.key.include_as_annotation {
                add(value.key.label, &value.to_value_string());
            }
        }
    }
    if let Some(labels) = context.container_spec.labels.value() {
        for (key, value) in labels {
            add(key, value);
        }
    }
    tags
}

/// The docker labels of the container definition: the labels of the app plus the runtime values.
fn docker_labels(context: &StartContext<'_>) -> BTreeMap<String, String> {
    let mut labels = context
        .container_spec
        .labels
        .value()
        .cloned()
        .unwrap_or_default();
    let mut add = |values: &RuntimeValues| {
        for value in values.iter() {
            if value.key.include_as_label || value.key.include_as_annotation {
                labels.insert(value.key.label.to_string(), value.to_value_string());
            }
        }
    };
    add(&context.proxy.runtime_values);
    add(&context.container.runtime_values);
    labels
}

/// The log configuration of a container (`awslogs`, only with CloudWatch enabled).
pub fn log_configuration(config: &EcsConfig, spec_id: &str) -> Option<LogConfiguration> {
    if !config.enable_cloud_watch {
        return None;
    }
    let options = std::collections::HashMap::from([
        (
            "awslogs-group".to_string(),
            format!(
                "{}sp-{}",
                config.cloud_watch_group_prefix,
                sanitize_log_group(spec_id)
            ),
        ),
        (
            "awslogs-region".to_string(),
            config.cloud_watch_region.clone(),
        ),
        (
            "awslogs-stream-prefix".to_string(),
            config.cloud_watch_stream_prefix.clone(),
        ),
        ("awslogs-create-group".to_string(), "true".to_string()),
    ]);
    LogConfiguration::builder()
        .log_driver(LogDriver::Awslogs)
        .set_options(Some(options))
        .build()
        .ok()
}

/// The volumes and the mount points of a task (`getVolumes`).
pub fn volumes(
    context: &StartContext<'_>,
    extension: &EcsSpecExtension,
) -> Result<(Vec<Volume>, Vec<MountPoint>), String> {
    let mut volume_names: Vec<String> = Vec::new();
    let mut volumes: Vec<Volume> = Vec::new();

    for efs in &extension.efs_volumes {
        let name = efs.name.clone().unwrap_or_default();
        let mut configuration = EfsVolumeConfiguration::builder()
            .file_system_id(efs.file_system_id.clone().unwrap_or_default());
        if let Some(root) = efs.root_directory.clone() {
            configuration = configuration.root_directory(root);
        }
        if efs.transit_encryption.unwrap_or(false) {
            configuration = configuration.transit_encryption(EfsTransitEncryption::Enabled);
        }
        if let Some(port) = efs.transit_encryption_port {
            configuration = configuration.transit_encryption_port(port as i32);
        }
        let mut authorization = EfsAuthorizationConfig::builder();
        if let Some(access_point) = efs.access_point_id.clone() {
            authorization = authorization.access_point_id(access_point);
        }
        if efs.enable_iam.unwrap_or(false) {
            authorization = authorization.iam(EfsAuthorizationConfigIam::Enabled);
        }
        configuration = configuration.authorization_config(authorization.build());

        volumes.push(
            Volume::builder()
                .name(name.clone())
                .efs_volume_configuration(
                    configuration
                        .build()
                        .map_err(|error| format!("invalid EFS volume {name}: {error}"))?,
                )
                .build(),
        );
        volume_names.push(name);
    }

    for name in &extension.bind_volumes {
        volumes.push(Volume::builder().name(name.clone()).build());
        volume_names.push(name.clone());
    }

    let mut mount_points = Vec::new();
    if let Some(configured) = context.container_spec.volumes.value() {
        for volume in configured {
            let components: Vec<&str> = volume.split(':').collect();
            if components.len() != 2 && components.len() != 3 {
                return Err(format!(
                    "Invalid volume configuration: {volume}, did not found correct components \
                     (e.g. 'myname:/mnt' or 'myname:/mnt:readonly')"
                ));
            }
            let name = components[0];
            let container_path = components[1];
            if !volume_names.iter().any(|known| known == name) {
                return Err(format!(
                    "Invalid volume configuration: {volume}, no corresponding (EFS or bind) volume \
                     definition found"
                ));
            }
            let mut mount_point = MountPoint::builder()
                .source_volume(name.to_string())
                .container_path(container_path.to_string());
            if components.len() == 3 {
                if components[2] != "readonly" {
                    return Err(format!(
                        "Invalid volume configuration: {volume}, third component must be equal to \
                         'readonly' (or removed)"
                    ));
                }
                mount_point = mount_point.read_only(true);
            }
            mount_points.push(mount_point.build());
        }
    }

    // a read-only root file system still needs a writable /tmp
    if extension.readonly_root_filesystem.unwrap_or(false) {
        volumes.push(Volume::builder().name("tmp").build());
        mount_points.push(
            MountPoint::builder()
                .source_volume("tmp")
                .container_path("/tmp")
                .build(),
        );
    }

    Ok((volumes, mount_points))
}

/// The secrets of a container (`getSecrets`).
pub fn secrets(extension: &EcsSpecExtension) -> Result<Vec<Secret>, String> {
    let mut secrets = Vec::new();
    for managed in &extension.managed_secrets {
        let name = managed.name.clone().unwrap_or_default();
        secrets.push(
            Secret::builder()
                .name(name.clone())
                .value_from(managed.value_from.clone().unwrap_or_default())
                .build()
                .map_err(|error| format!("invalid managed secret {name}: {error}"))?,
        );
    }
    Ok(secrets)
}

/// The name of the container of a task.
pub fn container_name(context: &StartContext<'_>) -> String {
    let name = match context
        .container_spec
        .resource_name
        .as_str()
        .filter(|name| !name.is_empty())
    {
        Some(name) => name.to_string(),
        None => format!(
            "sp-container-{}-{}",
            context.proxy.id, context.container.index
        ),
    };
    name.chars().take(255).collect()
}

/// The task definition of a container (`getTaskDefinition`).
///
/// Returns the fields of `RegisterTaskDefinitionInput`; the backend sends them to AWS.
pub fn task_definition(
    config: &EcsConfig,
    context: &StartContext<'_>,
    extension: &EcsSpecExtension,
) -> Result<TaskDefinitionRequest, String> {
    let spec = context.container_spec;

    let environment: Vec<KeyValuePair> = context
        .environment
        .iter()
        .map(|(name, value)| {
            KeyValuePair::builder()
                .name(name.clone())
                .value(value.clone())
                .build()
        })
        .collect();

    let (volumes, mount_points) = volumes(context, extension)?;

    let mut container = ContainerDefinition::builder()
        .name(container_name(context))
        .image(spec.image.as_str().unwrap_or_default().to_string())
        .set_environment(Some(environment))
        .stop_timeout(2)
        .set_docker_labels(Some(docker_labels(context).into_iter().collect()))
        .set_mount_points(Some(mount_points))
        .set_secrets(Some(secrets(extension)?))
        .readonly_root_filesystem(extension.readonly_root_filesystem.unwrap_or(false));
    if let Some(command) = spec.cmd.value().filter(|command| !command.is_empty()) {
        container = container.set_command(Some(command.clone()));
    }
    if let Some(log_configuration) =
        log_configuration(config, context.proxy.spec_id.as_deref().unwrap_or_default())
    {
        container = container.log_configuration(log_configuration);
    }
    let credentials = extension
        .repository_credentials_parameter
        .clone()
        .or_else(|| config.default_repository_credentials_parameter.clone())
        .filter(|value| !value.trim().is_empty());
    if let Some(credentials) = credentials {
        container = container.repository_credentials(
            RepositoryCredentials::builder()
                .credentials_parameter(credentials)
                .build()
                .map_err(|error| format!("invalid repository credentials: {error}"))?,
        );
    }

    let mut runtime_platform = RuntimePlatform::builder();
    if let Some(architecture) = &extension.cpu_architecture {
        runtime_platform = runtime_platform.cpu_architecture(architecture.as_str().into());
    }
    if let Some(family) = &extension.operating_system_family {
        runtime_platform = runtime_platform.operating_system_family(family.as_str().into());
    }

    Ok(TaskDefinitionRequest {
        family: task_definition_family(&context.proxy.id),
        container: container.build(),
        cpu: spec.cpu_request.as_str().unwrap_or_default().to_string(),
        memory: spec.memory_request.as_str().unwrap_or_default().to_string(),
        task_role: extension.task_role.clone(),
        execution_role: extension.execution_role.clone(),
        runtime_platform: runtime_platform.build(),
        ephemeral_storage: EphemeralStorage::builder()
            .size_in_gib(extension.ephemeral_storage_size.unwrap_or(21) as i32)
            .build(),
        volumes,
        tags: tags(context),
    })
}

/// The fields of the task definition ShinyProxy registers.
#[derive(Debug, Clone)]
pub struct TaskDefinitionRequest {
    /// The family (`sp-task-definition-{proxyId}`).
    pub family: String,
    /// The single container of the task.
    pub container: ContainerDefinition,
    /// CPU units of the task (`container-cpu-request`, required by Fargate).
    pub cpu: String,
    /// Memory of the task (`container-memory-request`, required by Fargate).
    pub memory: String,
    /// Role of the task.
    pub task_role: Option<String>,
    /// Role the ECS agent uses to start the task.
    pub execution_role: Option<String>,
    /// Architecture and operating system of the task.
    pub runtime_platform: RuntimePlatform,
    /// Ephemeral storage of the task.
    pub ephemeral_storage: EphemeralStorage,
    /// Volumes of the task.
    pub volumes: Vec<Volume>,
    /// Tags of the task definition, which the task inherits.
    pub tags: Vec<Tag>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::model::proxy::{Container, Proxy, ProxyStatus};
    use crate::model::runtime_value::{RuntimeValue, INSTANCE_ID, PROXIED_APP, PROXY_ID, USER_ID};
    use crate::model::spec::{ContainerSpec, PortMapping, ProxySpec};
    use crate::model::spel_field::{SpelString, SpelStringList, SpelStringMap};

    fn config(extra: &str) -> EcsConfig {
        let yaml = format!(
            "proxy:\n  container-backend: ecs\n  ecs:\n    name: my-cluster\n    \
             region: eu-west-1\n    subnets: [ subnet-1 ]\n    security-groups: [ sg-1 ]\n{extra}"
        );
        let settings: Settings = serde_yaml_ng::from_str(&yaml).expect("settings");
        EcsConfig::from_settings(&settings).expect("config")
    }

    fn container_spec() -> ContainerSpec {
        ContainerSpec {
            image: SpelString::resolved("x".into(), "openanalytics/shinyproxy-demo".into()),
            cmd: SpelStringList::resolved(vec!["R".into()], vec!["R".into()]),
            port_mapping: vec![PortMapping {
                name: "default".to_string(),
                port: Some(3838),
                target_path: SpelString::resolved(String::new(), String::new()),
            }],
            memory_request: SpelString::resolved("2048".into(), "2048".into()),
            cpu_request: SpelString::resolved("1024".into(), "1024".into()),
            labels: SpelStringMap::resolved(
                std::collections::BTreeMap::new(),
                std::collections::BTreeMap::from([("my.label".to_string(), "value".to_string())]),
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
            environment: BTreeMap::from([("SHINYPROXY_USERNAME".to_string(), "jack".to_string())]),
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn builds_the_task_definition() {
        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let request =
            task_definition(&config(""), &context, &EcsSpecExtension::default()).expect("request");

        assert_eq!(request.family, "sp-task-definition-proxy-1");
        assert_eq!(request.cpu, "1024");
        assert_eq!(request.memory, "2048");
        assert_eq!(
            request.ephemeral_storage.size_in_gib(),
            21,
            "the Java default"
        );

        let definition = &request.container;
        assert_eq!(definition.name(), Some("sp-container-proxy-1-0"));
        assert_eq!(definition.image(), Some("openanalytics/shinyproxy-demo"));
        assert_eq!(definition.command(), ["R"]);
        assert_eq!(definition.stop_timeout(), Some(2));
        assert_eq!(definition.readonly_root_filesystem(), Some(false));
        let environment = definition.environment();
        assert!(
            environment
                .iter()
                .any(|pair| pair.name() == Some("SHINYPROXY_USERNAME")
                    && pair.value() == Some("jack"))
        );
        let labels = definition.docker_labels().expect("labels");
        assert_eq!(labels.get("my.label").map(String::as_str), Some("value"));
        assert_eq!(
            labels
                .get("openanalytics.eu/sp-proxied-app")
                .map(String::as_str),
            Some("true")
        );
        assert!(
            definition.log_configuration().is_none(),
            "CloudWatch is off"
        );

        // the tags carry the runtime values that are labels or annotations
        let tags: BTreeMap<String, String> = request
            .tags
            .iter()
            .map(|tag| {
                (
                    tag.key().unwrap_or_default().to_string(),
                    tag.value().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            tags.get("openanalytics.eu/sp-proxy-id").map(String::as_str),
            Some("proxy-1")
        );
        assert_eq!(
            tags.get("openanalytics.eu/sp-user-id").map(String::as_str),
            Some("jack")
        );
        assert_eq!(tags.get("my.label").map(String::as_str), Some("value"));
    }

    #[test]
    fn adds_the_cloud_watch_log_configuration() {
        let with_cloud_watch = config("    enable-cloud-watch: true\n");
        let configuration =
            log_configuration(&with_cloud_watch, "01_hello (demo)").expect("configuration");
        assert_eq!(configuration.log_driver(), &LogDriver::Awslogs);
        let options = configuration.options().expect("options");
        // the characters CloudWatch refuses are removed from the group name
        assert_eq!(
            options.get("awslogs-group").map(String::as_str),
            Some("/ecs/sp-01_hellodemo")
        );
        assert_eq!(
            options.get("awslogs-region").map(String::as_str),
            Some("eu-west-1")
        );
        assert_eq!(
            options.get("awslogs-stream-prefix").map(String::as_str),
            Some("ecs")
        );
        assert_eq!(
            options.get("awslogs-create-group").map(String::as_str),
            Some("true")
        );

        assert!(log_configuration(&config(""), "01_hello").is_none());
    }

    #[test]
    fn builds_the_volumes_and_mount_points() {
        let proxy = proxy();
        let container_spec = ContainerSpec {
            volumes: SpelStringList::resolved(
                vec!["home:/home/user".into(), "data:/data:readonly".into()],
                vec!["home:/home/user".into(), "data:/data:readonly".into()],
            ),
            ..container_spec()
        };
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let extension: EcsSpecExtension = serde_json::from_value(serde_json::json!({
            "efs-volumes": [{
                "name": "home",
                "file-system-id": "fs-123",
                "root-directory": "/home",
                "transit-encryption": true,
                "transit-encryption-port": 2049,
                "access-point-id": "fsap-1",
                "enable-iam": true,
            }],
            "bind-volumes": ["data"],
        }))
        .expect("extension");

        let (volumes, mount_points) = volumes(&context, &extension).expect("volumes");
        assert_eq!(volumes.len(), 2);
        let efs = volumes[0].efs_volume_configuration().expect("efs");
        assert_eq!(volumes[0].name(), Some("home"));
        assert_eq!(efs.file_system_id(), "fs-123");
        assert_eq!(efs.root_directory(), Some("/home"));
        assert_eq!(
            efs.transit_encryption(),
            Some(&EfsTransitEncryption::Enabled)
        );
        assert_eq!(efs.transit_encryption_port(), Some(2049));
        let authorization = efs.authorization_config().expect("authorization");
        assert_eq!(authorization.access_point_id(), Some("fsap-1"));
        assert_eq!(
            authorization.iam(),
            Some(&EfsAuthorizationConfigIam::Enabled)
        );
        assert_eq!(volumes[1].name(), Some("data"));
        assert!(volumes[1].efs_volume_configuration().is_none());

        assert_eq!(mount_points.len(), 2);
        assert_eq!(mount_points[0].source_volume(), Some("home"));
        assert_eq!(mount_points[0].container_path(), Some("/home/user"));
        assert_eq!(mount_points[0].read_only(), None);
        assert_eq!(mount_points[1].source_volume(), Some("data"));
        assert_eq!(mount_points[1].read_only(), Some(true));
    }

    #[test]
    fn refuses_invalid_volume_configurations() {
        let proxy = proxy();
        let make = |volume: &str| ContainerSpec {
            volumes: SpelStringList::resolved(vec![volume.into()], vec![volume.into()]),
            ..container_spec()
        };

        for (volume, expected) in [
            ("/only-one-component", "did not found correct components"),
            ("unknown:/mnt", "no corresponding (EFS or bind) volume"),
            (
                "data:/mnt:rw",
                "third component must be equal to 'readonly'",
            ),
        ] {
            let container_spec = make(volume);
            let mut spec = ProxySpec::new("01_hello");
            spec.container_specs = vec![container_spec.clone()];
            let container = Container::new(0);
            let context = context(&proxy, &spec, &container_spec, &container);
            let extension: EcsSpecExtension =
                serde_json::from_value(serde_json::json!({"bind-volumes": ["data"]}))
                    .expect("extension");
            let error = volumes(&context, &extension).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn a_read_only_root_file_system_gets_a_writable_tmp() {
        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let extension: EcsSpecExtension =
            serde_json::from_value(serde_json::json!({"readonly-root-filesystem": true}))
                .expect("extension");
        let (volumes, mount_points) = volumes(&context, &extension).expect("volumes");
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].name(), Some("tmp"));
        assert_eq!(mount_points[0].container_path(), Some("/tmp"));

        let request = task_definition(&config(""), &context, &extension).expect("request");
        assert_eq!(request.container.readonly_root_filesystem(), Some(true));
    }

    #[test]
    fn adds_the_roles_secrets_and_platform_of_the_app() {
        let proxy = proxy();
        let container_spec = container_spec();
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let extension: EcsSpecExtension = serde_json::from_value(serde_json::json!({
            "task-role": "arn:aws:iam::123:role/task",
            "execution-role": "arn:aws:iam::123:role/execution",
            "cpu-architecture": "ARM64",
            "operating-system-family": "LINUX",
            "ephemeral-storage-size": 30,
            "managed-secrets": [{"name": "DB_PASSWORD", "value-from": "arn:aws:ssm:secret"}],
            "repository-credentials-parameter": "arn:aws:secretsmanager:registry",
        }))
        .expect("extension");

        let request = task_definition(&config(""), &context, &extension).expect("request");
        assert_eq!(
            request.task_role.as_deref(),
            Some("arn:aws:iam::123:role/task")
        );
        assert_eq!(
            request.execution_role.as_deref(),
            Some("arn:aws:iam::123:role/execution")
        );
        assert_eq!(
            request
                .runtime_platform
                .cpu_architecture()
                .map(|value| value.as_str()),
            Some("ARM64")
        );
        assert_eq!(
            request
                .runtime_platform
                .operating_system_family()
                .map(|value| value.as_str()),
            Some("LINUX")
        );
        assert_eq!(request.ephemeral_storage.size_in_gib(), 30);

        let secrets = request.container.secrets();
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].name(), "DB_PASSWORD");
        assert_eq!(secrets[0].value_from(), "arn:aws:ssm:secret");
        assert_eq!(
            request
                .container
                .repository_credentials()
                .map(|credentials| credentials.credentials_parameter()),
            Some("arn:aws:secretsmanager:registry")
        );

        // the credentials of the configuration are used when the app has none
        let request = task_definition(
            &config("    default-repository-credentials-parameter: arn:aws:default\n"),
            &context,
            &EcsSpecExtension::default(),
        )
        .expect("request");
        assert_eq!(
            request
                .container
                .repository_credentials()
                .map(|credentials| credentials.credentials_parameter()),
            Some("arn:aws:default")
        );
    }

    #[test]
    fn skips_tags_ecs_would_refuse() {
        let mut proxy = proxy();
        // the groups of a user and the port mappings are never tags
        proxy.add_runtime_value(
            RuntimeValue::string(&USER_GROUPS, "scientists,mathematicians"),
            true,
        );
        let container_spec = ContainerSpec {
            labels: SpelStringMap::resolved(
                std::collections::BTreeMap::new(),
                std::collections::BTreeMap::from([
                    ("valid".to_string(), "a-value_1".to_string()),
                    ("invalid".to_string(), "no ünicode".to_string()),
                ]),
            ),
            ..container_spec()
        };
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![container_spec.clone()];
        let container = Container::new(0);
        let context = context(&proxy, &spec, &container_spec, &container);

        let keys: Vec<String> = tags(&context)
            .iter()
            .map(|tag| tag.key().unwrap_or_default().to_string())
            .collect();
        assert!(keys.contains(&"valid".to_string()));
        assert!(
            !keys.contains(&"invalid".to_string()),
            "a value ECS refuses is skipped: {keys:?}"
        );
        assert!(
            !keys.contains(&"openanalytics.eu/sp-user-groups".to_string()),
            "the groups of the user are never a tag: {keys:?}"
        );
    }
}
