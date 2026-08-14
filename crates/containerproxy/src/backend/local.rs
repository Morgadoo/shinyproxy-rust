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

//! The `local` container backend: apps run as local processes.
//!
//! **This backend is an addition of this implementation and exists for testing only** (the startup
//! warning says so as well). It makes the complete proxy lifecycle and the reverse proxy data plane
//! testable without a container runtime, which is what the test suite of this repository uses.
//!
//! The app command is taken from `container-cmd`; when it is absent, `container-image` is interpreted as
//! the name of an executable. The allocated host port is passed to the process as `PORT` and as
//! `--port <port>`, which is what the `sp-testapp` fixture (and most web frameworks) understand.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::process::{Child, Command};

use super::ports::PortAllocator;
use super::target::{compute_target_path, mapping_key_to_path, target_url};
use super::{BackendError, ContainerBackend, StartContext, StartedContainer};
use crate::config::Settings;
use crate::model::proxy::Proxy;
use crate::model::runtime_value::{
    BackendContainerName, RuntimeValue, RuntimeValues, BACKEND_CONTAINER_NAME,
};

/// Name of the backend.
pub const NAME: &str = "local";

/// Runs apps as local processes.
#[derive(Debug)]
pub struct LocalBackend {
    processes: DashMap<String, Vec<Child>>,
    port_allocator: Arc<PortAllocator>,
    host: String,
    protocol: String,
}

impl LocalBackend {
    /// Creates the backend.
    pub fn new(settings: &Settings, port_allocator: Arc<PortAllocator>) -> Self {
        LocalBackend {
            processes: DashMap::new(),
            port_allocator,
            host: settings.proxy.docker.target_bind_ip().to_string(),
            protocol: settings
                .proxy
                .docker
                .container_protocol
                .clone()
                .unwrap_or_else(|| "http".to_string()),
        }
    }

    /// Number of proxies with running processes (used in tests).
    pub fn running_proxies(&self) -> usize {
        self.processes.len()
    }

    /// Process ids of a proxy (used in tests).
    pub fn process_ids(&self, proxy_id: &str) -> Vec<u32> {
        self.processes
            .get(proxy_id)
            .map(|children| children.iter().filter_map(Child::id).collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl ContainerBackend for LocalBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supports_health_check(&self) -> bool {
        true
    }

    async fn start_container(
        &self,
        context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        let spec = context.container_spec;

        // the command: `container-cmd`, or `container-image` interpreted as an executable
        let mut command_line: Vec<String> = spec
            .cmd
            .value()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        if command_line.is_empty() {
            let image = spec.image.as_str().unwrap_or_default();
            if image.is_empty() {
                return Err(BackendError::FailedToStart(
                    "the local backend needs container-cmd or container-image".to_string(),
                ));
            }
            command_line.push(image.to_string());
        }

        // one host port per port mapping
        let mut targets = BTreeMap::new();
        let mut first_port = None;
        for mapping in &spec.port_mapping {
            let port = self
                .port_allocator
                .allocate(&context.proxy.id)
                .map_err(|error| BackendError::FailedToStart(error.to_string()))?;
            if first_port.is_none() {
                first_port = Some(port);
            }
            let target_path = compute_target_path(mapping.target_path.as_str());
            targets.insert(
                mapping_key_to_path(&mapping.name),
                target_url(&self.protocol, &self.host, port, &target_path),
            );
        }
        let port = first_port.unwrap_or_else(|| {
            // no port mappings at all: still allocate one so that the process has a port
            self.port_allocator
                .allocate(&context.proxy.id)
                .unwrap_or_default()
        });

        let program = resolve_program(&command_line[0]);
        let mut command = Command::new(&program);
        command.args(&command_line[1..]);
        command.arg("--port");
        command.arg(port.to_string());
        command.env_clear();
        // keep the variables a process needs to run at all
        for name in ["PATH", "HOME", "LANG", "TMPDIR", "USER"] {
            if let Ok(value) = std::env::var(name) {
                command.env(name, value);
            }
        }
        for (name, value) in &context.environment {
            command.env(name, value);
        }
        command.env("PORT", port.to_string());
        command.kill_on_drop(false);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());

        let child = command.spawn().map_err(|error| {
            BackendError::FailedToStart(format!("cannot start '{program}': {error}"))
        })?;
        let pid = child.id().unwrap_or_default();
        tracing::info!(
            "[local backend] started process {pid} for proxy {} on port {port}",
            context.proxy.id
        );

        self.processes
            .entry(context.proxy.id.clone())
            .or_default()
            .push(child);

        let mut runtime_values = RuntimeValues::new();
        runtime_values.add(
            RuntimeValue::json(
                &BACKEND_CONTAINER_NAME,
                BackendContainerName::new(&format!("local/{pid}")),
            ),
            true,
        );

        Ok(StartedContainer {
            id: Some(pid.to_string()),
            runtime_values,
            targets,
        })
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        if let Some((_, children)) = self.processes.remove(&proxy.id) {
            for mut child in children {
                let pid = child.id();
                // ask the process to stop, then make sure it is gone
                if let Err(error) = child.start_kill() {
                    tracing::debug!("[local backend] cannot signal process {pid:?}: {error}");
                }
                match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(status)) => {
                        tracing::info!("[local backend] process {pid:?} stopped ({status})")
                    }
                    Ok(Err(error)) => {
                        tracing::warn!("[local backend] cannot wait for process {pid:?}: {error}")
                    }
                    Err(_) => {
                        tracing::warn!("[local backend] process {pid:?} did not stop in time");
                        let _ = child.kill().await;
                    }
                }
            }
        }
        self.port_allocator.release(&proxy.id);
        Ok(())
    }

    async fn is_proxy_healthy(&self, proxy: &Proxy) -> Result<bool, BackendError> {
        let Some(mut children) = self.processes.get_mut(&proxy.id) else {
            return Ok(false);
        };
        for child in children.iter_mut() {
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    tracing::warn!(
                        "[local backend] process of proxy {} exited ({status})",
                        proxy.id
                    );
                    return Ok(false);
                }
                Err(error) => {
                    return Err(BackendError::Backend(format!(
                        "cannot check process of proxy {}: {error}",
                        proxy.id
                    )))
                }
            }
        }
        Ok(true)
    }
}

/// Resolves the program to start: `sp-testapp` and other siblings of this executable are found next to
/// it, which is what the integration tests rely on.
fn resolve_program(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    if let Ok(current) = std::env::current_exe() {
        // tests run from target/debug/deps/<test binary>
        let directories = [current.parent(), current.parent().and_then(|parent| parent.parent())];
        for directory in directories.into_iter().flatten() {
            let candidate = directory.join(program);
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    program.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Container, ProxyStatus};
    use crate::model::spec::{ContainerSpec, PortMapping, ProxySpec};
    use crate::model::spel_field::{SpelString, SpelStringList};

    fn settings() -> Settings {
        serde_yaml_ng::from_str("proxy:\n  container-backend: local\n").expect("settings")
    }

    fn spec_with_command(command: Vec<&str>) -> (ProxySpec, ContainerSpec) {
        let container = ContainerSpec {
            image: SpelString::resolved("sp-testapp".into(), "sp-testapp".into()),
            cmd: SpelStringList::resolved(
                command.iter().map(|part| part.to_string()).collect(),
                command.iter().map(|part| part.to_string()).collect(),
            ),
            port_mapping: vec![PortMapping {
                name: "default".to_string(),
                port: Some(3838),
                target_path: SpelString::resolved(String::new(), String::new()),
            }],
            ..Default::default()
        };
        let mut spec = ProxySpec::new("test");
        spec.container_specs = vec![container.clone()];
        (spec, container)
    }

    #[tokio::test]
    async fn starts_and_stops_a_process() {
        let allocator = Arc::new(PortAllocator::new(21000, None));
        let backend = LocalBackend::new(&settings(), allocator.clone());
        let (spec, container_spec) = spec_with_command(vec!["sleep", "30"]);
        let proxy = Proxy::new("proxy-1", ProxyStatus::New);
        let container = Container::new(0);

        let started = backend
            .start_container(StartContext {
                user: None,
                proxy: &proxy,
                spec: &spec,
                container_spec: &container_spec,
                container: &container,
                environment: BTreeMap::from([("MY_VAR".to_string(), "value".to_string())]),
                labels: BTreeMap::new(),
            })
            .await
            .expect("starts");

        assert!(started.id.is_some());
        assert_eq!(
            started.targets.get(""),
            Some(&"http://127.0.0.1:21000".to_string())
        );
        assert_eq!(backend.running_proxies(), 1);
        assert_eq!(allocator.owned_ports("proxy-1").len(), 1);
        assert!(backend.is_proxy_healthy(&proxy).await.unwrap());

        let name: BackendContainerName = started
            .runtime_values
            .get(&BACKEND_CONTAINER_NAME)
            .expect("backend container name")
            .data
            .parse_json()
            .expect("parses");
        assert_eq!(name.namespace, "local");

        backend.stop_proxy(&proxy).await.expect("stops");
        assert_eq!(backend.running_proxies(), 0);
        assert!(allocator.owned_ports("proxy-1").is_empty());
        assert!(!backend.is_proxy_healthy(&proxy).await.unwrap());
    }

    #[tokio::test]
    async fn detects_processes_that_exited() {
        let backend = LocalBackend::new(&settings(), Arc::new(PortAllocator::new(21100, None)));
        let (spec, container_spec) = spec_with_command(vec!["true"]);
        let proxy = Proxy::new("proxy-2", ProxyStatus::New);
        let container = Container::new(0);

        backend
            .start_container(StartContext {
                user: None,
                proxy: &proxy,
                spec: &spec,
                container_spec: &container_spec,
                container: &container,
                environment: BTreeMap::new(),
                labels: BTreeMap::new(),
            })
            .await
            .expect("starts");

        // `true` exits immediately
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!backend.is_proxy_healthy(&proxy).await.unwrap());
        backend.stop_proxy(&proxy).await.expect("stops");
    }

    #[tokio::test]
    async fn reports_unknown_commands() {
        let backend = LocalBackend::new(&settings(), Arc::new(PortAllocator::new(21200, None)));
        let (spec, container_spec) = spec_with_command(vec!["definitely-not-a-command"]);
        let proxy = Proxy::new("proxy-3", ProxyStatus::New);
        let container = Container::new(0);

        let error = backend
            .start_container(StartContext {
                user: None,
                proxy: &proxy,
                spec: &spec,
                container_spec: &container_spec,
                container: &container,
                environment: BTreeMap::new(),
                labels: BTreeMap::new(),
            })
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("definitely-not-a-command"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn maps_additional_port_mappings_to_sub_paths() {
        let allocator = Arc::new(PortAllocator::new(21300, None));
        let backend = LocalBackend::new(&settings(), allocator.clone());
        let (mut spec, mut container_spec) = spec_with_command(vec!["sleep", "30"]);
        container_spec.port_mapping = vec![
            PortMapping {
                name: "dashboard".to_string(),
                port: Some(8080),
                target_path: SpelString::resolved("/dash//".into(), "/dash//".into()),
            },
            PortMapping {
                name: "default".to_string(),
                port: Some(3838),
                target_path: SpelString::resolved(String::new(), String::new()),
            },
        ];
        spec.container_specs = vec![container_spec.clone()];

        let proxy = Proxy::new("proxy-4", ProxyStatus::New);
        let container = Container::new(0);
        let started = backend
            .start_container(StartContext {
                user: None,
                proxy: &proxy,
                spec: &spec,
                container_spec: &container_spec,
                container: &container,
                environment: BTreeMap::new(),
                labels: BTreeMap::new(),
            })
            .await
            .expect("starts");

        assert_eq!(
            started.targets.get("dashboard"),
            Some(&"http://127.0.0.1:21300/dash".to_string()),
            "target paths are normalised"
        );
        assert_eq!(
            started.targets.get(""),
            Some(&"http://127.0.0.1:21301".to_string())
        );
        backend.stop_proxy(&proxy).await.expect("stops");
    }
}
