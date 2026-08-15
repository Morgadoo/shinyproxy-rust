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

//! The configuration of the ECS backend (`proxy.ecs.*`) and its startup validations.

use std::time::Duration;

use crate::config::Settings;
use crate::model::spec::ProxySpec;

/// Everything the backend needs to know about the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcsConfig {
    /// Name of the ECS cluster (`proxy.ecs.name`).
    pub cluster: String,
    /// Region of the cluster (`proxy.ecs.region`).
    pub region: String,
    /// Subnets the tasks run in.
    pub subnets: Vec<String>,
    /// Security groups of the tasks.
    pub security_groups: Vec<String>,
    /// How long a task may take to start (`service-wait-time`, three minutes).
    pub service_wait_time: Duration,
    /// Whether the output of the containers goes to CloudWatch.
    pub enable_cloud_watch: bool,
    /// Prefix of the CloudWatch log group (`/ecs/`).
    pub cloud_watch_group_prefix: String,
    /// Region of the CloudWatch logs (the region of the cluster by default).
    pub cloud_watch_region: String,
    /// Prefix of the CloudWatch log streams (`ecs`).
    pub cloud_watch_stream_prefix: String,
    /// Parameter with the credentials of the image registry, used when an app has none.
    pub default_repository_credentials_parameter: Option<String>,
    /// Protocol used to reach the apps.
    pub target_protocol: String,
}

impl EcsConfig {
    /// Reads and validates the configuration, with the messages of the Java implementation.
    pub fn from_settings(settings: &Settings) -> Result<Self, String> {
        let ecs = &settings.proxy.ecs;

        let Some(region) = ecs
            .region
            .clone()
            .filter(|region| !region.trim().is_empty())
        else {
            return Err(
                "Error in configuration of ECS backend: proxy.ecs.region not set".to_string(),
            );
        };
        let Some(cluster) = ecs.name.clone().filter(|name| !name.trim().is_empty()) else {
            return Err(
                "Error in configuration of ECS backend: proxy.ecs.cluster not set to name of \
                 cluster"
                    .to_string(),
            );
        };

        let subnets: Vec<String> = ecs
            .subnets
            .values()
            .iter()
            .filter(|subnet| !subnet.trim().is_empty())
            .cloned()
            .collect();
        if subnets.is_empty() {
            return Err(
                "Error in configuration of ECS backend: need at least one subnet in \
                 proxy.ecs.subnets"
                    .to_string(),
            );
        }
        let security_groups: Vec<String> = ecs
            .security_groups
            .values()
            .iter()
            .filter(|group| !group.trim().is_empty())
            .cloned()
            .collect();
        if security_groups.is_empty() {
            return Err(
                "Error in configuration of ECS backend: need at least one security group in \
                 proxy.ecs.security-groups"
                    .to_string(),
            );
        }

        // Fargate has no privileged containers
        if ecs.privileged.map(|value| value.0).unwrap_or(false) {
            return Err(
                "Error in configuration of ECS backend: config has 'privileged: true' configured, \
                 this is not supported by ECS fargated"
                    .to_string(),
            );
        }

        Ok(EcsConfig {
            cluster,
            region: region.clone(),
            subnets,
            security_groups,
            service_wait_time: Duration::from_millis(
                ecs.service_wait_time
                    .map(|value| value.0)
                    .filter(|value| *value > 0)
                    .unwrap_or(180_000) as u64,
            ),
            enable_cloud_watch: ecs.enable_cloud_watch.map(|value| value.0).unwrap_or(false),
            cloud_watch_group_prefix: ecs
                .cloud_watch_group_prefix
                .clone()
                .unwrap_or_else(|| "/ecs/".to_string()),
            cloud_watch_region: ecs.cloud_watch_region.clone().unwrap_or(region),
            cloud_watch_stream_prefix: ecs
                .cloud_watch_stream_prefix
                .clone()
                .unwrap_or_else(|| "ecs".to_string()),
            default_repository_credentials_parameter: ecs
                .default_repository_credentials_parameter
                .clone()
                .filter(|value| !value.trim().is_empty()),
            target_protocol: ecs
                .container_protocol
                .clone()
                .filter(|protocol| !protocol.trim().is_empty())
                .unwrap_or_else(|| "http".to_string()),
        })
    }

    /// Checks that an app definition can run on Fargate, with the messages of the Java implementation.
    pub fn validate_spec(spec: &ProxySpec) -> Result<(), String> {
        let Some(container) = spec.container_specs.first() else {
            return Ok(());
        };
        if container.memory_request.original().is_none() {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has non 'memory-request' \
                 configured, this is required for running on ECS fargate",
                spec.id
            ));
        }
        if container.cpu_request.original().is_none() {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has non 'cpu-request' \
                 configured, this is required for running on ECS fargate",
                spec.id
            ));
        }
        if container.memory_limit.original().is_some() {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has 'memory-limit' configured, \
                 this is not supported by ECS fargate",
                spec.id
            ));
        }
        if container.cpu_limit.original().is_some() {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has 'cpu-limit' configured, \
                 this is not supported by ECS fargate",
                spec.id
            ));
        }
        if container.privileged {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has 'privileged: true' \
                 configured, this is not supported by ECS fargate",
                spec.id
            ));
        }
        if container.dns.original().is_some() {
            return Err(format!(
                "Error in configuration of specs: spec with id '{}' has 'dns' configured, this is \
                 not supported by ECS fargate",
                spec.id
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    const COMPLETE: &str = "proxy:\n  container-backend: ecs\n  ecs:\n    name: my-cluster\n    \
                            region: eu-west-1\n    subnets: [ subnet-1 ]\n    \
                            security-groups: [ sg-1 ]\n";

    #[test]
    fn reads_the_configuration() {
        let config = EcsConfig::from_settings(&settings(COMPLETE)).expect("config");
        assert_eq!(config.cluster, "my-cluster");
        assert_eq!(config.region, "eu-west-1");
        assert_eq!(config.subnets, vec!["subnet-1"]);
        assert_eq!(config.security_groups, vec!["sg-1"]);
        assert_eq!(config.service_wait_time, Duration::from_secs(180));
        assert!(!config.enable_cloud_watch);
        assert_eq!(config.cloud_watch_group_prefix, "/ecs/");
        assert_eq!(config.cloud_watch_region, "eu-west-1");
        assert_eq!(config.cloud_watch_stream_prefix, "ecs");
        assert_eq!(config.target_protocol, "http");

        let config = EcsConfig::from_settings(&settings(
            "proxy:\n  container-backend: ecs\n  ecs:\n    name: my-cluster\n    \
             region: eu-west-1\n    subnets: [ subnet-1, subnet-2 ]\n    \
             security-groups: [ sg-1 ]\n    service-wait-time: 60000\n    \
             enable-cloudwatch: true\n    cloud-watch-group-prefix: /shinyproxy/\n    \
             cloud-watch-region: eu-central-1\n    cloud-watch-stream-prefix: sp\n    \
             default-repository-credentials-parameter: arn:aws:secretsmanager:x\n    \
             container-protocol: https\n",
        ))
        .expect("config");
        assert_eq!(config.subnets, vec!["subnet-1", "subnet-2"]);
        assert_eq!(config.service_wait_time, Duration::from_secs(60));
        // `enable-cloudwatch` is the alias of `enable-cloud-watch`
        assert!(config.enable_cloud_watch);
        assert_eq!(config.cloud_watch_group_prefix, "/shinyproxy/");
        assert_eq!(config.cloud_watch_region, "eu-central-1");
        assert_eq!(config.cloud_watch_stream_prefix, "sp");
        assert_eq!(
            config.default_repository_credentials_parameter.as_deref(),
            Some("arn:aws:secretsmanager:x")
        );
        assert_eq!(config.target_protocol, "https");
    }

    #[test]
    fn refuses_an_incomplete_configuration() {
        for (yaml, expected) in [
            (
                "proxy:\n  container-backend: ecs\n",
                "proxy.ecs.region not set",
            ),
            (
                "proxy:\n  container-backend: ecs\n  ecs:\n    region: eu-west-1\n",
                "proxy.ecs.cluster not set to name of cluster",
            ),
            (
                "proxy:\n  container-backend: ecs\n  ecs:\n    region: eu-west-1\n    \
                 name: my-cluster\n",
                "need at least one subnet in proxy.ecs.subnets",
            ),
            (
                "proxy:\n  container-backend: ecs\n  ecs:\n    region: eu-west-1\n    \
                 name: my-cluster\n    subnets: [ subnet-1 ]\n",
                "need at least one security group in proxy.ecs.security-groups",
            ),
        ] {
            let error = EcsConfig::from_settings(&settings(yaml)).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }

        // Fargate has no privileged containers
        let yaml = format!("{COMPLETE}    privileged: true\n");
        let error = EcsConfig::from_settings(&settings(&yaml)).unwrap_err();
        assert!(error.contains("not supported by ECS fargated"), "{error}");
    }

    #[test]
    fn validates_the_app_definitions() {
        use crate::model::spec::ContainerSpec;
        use crate::model::spel_field::{SpelString, SpelStringList};

        let complete = || ContainerSpec {
            memory_request: SpelString::resolved("1Gi".into(), "1Gi".into()),
            cpu_request: SpelString::resolved("512".into(), "512".into()),
            ..Default::default()
        };

        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![complete()];
        assert!(EcsConfig::validate_spec(&spec).is_ok());

        // memory-request and cpu-request are required
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![ContainerSpec::default()];
        let error = EcsConfig::validate_spec(&spec).unwrap_err();
        assert!(error.contains("non 'memory-request' configured"), "{error}");

        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![ContainerSpec {
            memory_request: SpelString::resolved("1Gi".into(), "1Gi".into()),
            ..Default::default()
        }];
        let error = EcsConfig::validate_spec(&spec).unwrap_err();
        assert!(error.contains("non 'cpu-request' configured"), "{error}");

        // the limits, privileged containers and dns are not supported
        for (change, expected) in [
            (
                ContainerSpec {
                    memory_limit: SpelString::resolved("2Gi".into(), "2Gi".into()),
                    ..complete()
                },
                "'memory-limit' configured",
            ),
            (
                ContainerSpec {
                    cpu_limit: SpelString::resolved("1024".into(), "1024".into()),
                    ..complete()
                },
                "'cpu-limit' configured",
            ),
            (
                ContainerSpec {
                    privileged: true,
                    ..complete()
                },
                "'privileged: true' configured",
            ),
            (
                ContainerSpec {
                    dns: SpelStringList::resolved(vec!["8.8.8.8".into()], vec!["8.8.8.8".into()]),
                    ..complete()
                },
                "'dns' configured",
            ),
        ] {
            let mut spec = ProxySpec::new("01_hello");
            spec.container_specs = vec![change];
            let error = EcsConfig::validate_spec(&spec).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }
}
