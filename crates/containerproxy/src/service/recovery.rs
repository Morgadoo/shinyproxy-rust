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

//! App recovery (`AppRecoveryService`).
//!
//! When `proxy.recover-running-proxies` is enabled, the containers that are already running when the
//! server starts are turned back into proxies: the labels of the containers hold every runtime value the
//! engine needs, so the apps keep working over a restart of ShinyProxy (a rolling update, a crash, or an
//! upgrade from the Java implementation to this one).
//!
//! `proxy.recover-running-proxies-from-different-config` decides whether containers of another
//! configuration (a different `instanceId`) are recovered as well.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::backend::ContainerBackend;
use crate::config::Settings;
use crate::model::proxy::{Container, Proxy, ProxyStatus};
use crate::model::runtime_value::{
    RuntimeValues, CONTAINER_INDEX, CREATED_TIMESTAMP, DISPLAY_NAME, INSTANCE_ID, PROXY_ID,
    PROXY_SPEC_ID, TARGET_ID, USER_ID,
};
use crate::service::identifier::Identifiers;
use crate::store::{HeartbeatStore, ProxyStore};

/// Recovers the apps that are still running.
#[derive(Debug)]
pub struct AppRecoveryService {
    /// Whether recovery is enabled (`proxy.recover-running-proxies`).
    enabled: bool,
    /// Whether apps of another configuration are recovered
    /// (`proxy.recover-running-proxies-from-different-config`).
    from_different_config: bool,
    /// The instance id of this server.
    instance_id: String,
    /// Set once recovery finished; the readiness probe and the recovery filter use it.
    ready: AtomicBool,
}

impl AppRecoveryService {
    /// Creates the service from the settings.
    pub fn new(settings: &Settings, identifiers: &Identifiers) -> Self {
        AppRecoveryService {
            enabled: settings.proxy.recover_running_proxies(),
            from_different_config: settings
                .proxy
                .recover_running_proxies_from_different_config
                .map(|value| value.0)
                .unwrap_or(false),
            instance_id: identifiers.instance_id.clone(),
            ready: AtomicBool::new(false),
        }
    }

    /// Whether recovery is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether recovery finished (readiness probe, recovery filter).
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Whether a container with this instance id may be recovered (`canRecoverProxy`).
    pub fn can_recover(&self, container_instance_id: Option<&str>) -> bool {
        let Some(instance_id) = container_instance_id else {
            // sanity check, as in Java
            return false;
        };
        if self.from_different_config {
            return true;
        }
        instance_id == self.instance_id
    }

    /// Scans the backend and adds the apps that are still running to the store.
    ///
    /// Returns the recovered proxies (in no particular order).
    pub async fn recover(
        &self,
        backend: &Arc<dyn ContainerBackend>,
        store: &Arc<dyn ProxyStore>,
        heartbeats: &Arc<dyn HeartbeatStore>,
    ) -> Vec<Proxy> {
        if !self.enabled {
            tracing::info!("Recovery of running apps disabled");
            self.ready.store(true, Ordering::SeqCst);
            return Vec::new();
        }

        if self.from_different_config {
            tracing::info!(
                "Recovery of running apps enabled (even apps started with a different config file)"
            );
        } else {
            tracing::info!(
                "Recovery of running apps enabled (but only apps started with the current config file)"
            );
        }

        let containers = match backend.scan_existing_containers().await {
            Ok(containers) => containers,
            Err(error) => {
                tracing::error!("Error while recovering running apps: {error}");
                self.ready.store(true, Ordering::SeqCst);
                return Vec::new();
            }
        };

        // one proxy can have several containers, so they are grouped by proxy id
        let mut proxies: BTreeMap<String, Proxy> = BTreeMap::new();
        for info in containers {
            let instance_id = info.runtime_values.value_string(&INSTANCE_ID);
            if !self.can_recover(instance_id.as_deref()) {
                tracing::warn!(
                    "Ignoring container {} because instanceId {} is not correct",
                    info.id,
                    instance_id.unwrap_or_default()
                );
                continue;
            }

            let Some(proxy_id) = info
                .runtime_values
                .get(&PROXY_ID)
                .map(|value| value.to_value_string())
            else {
                continue;
            };

            let proxy = proxies.entry(proxy_id.clone()).or_insert_with(|| {
                let created = info
                    .runtime_values
                    .get(&CREATED_TIMESTAMP)
                    .and_then(|value| value.to_value_string().parse::<i64>().ok())
                    .unwrap_or_default();
                let mut proxy = Proxy::new(&proxy_id, ProxyStatus::Up);
                proxy.spec_id = info.runtime_values.value_string(&PROXY_SPEC_ID);
                proxy.target_id = Some(
                    info.runtime_values
                        .value_string(&TARGET_ID)
                        .unwrap_or_else(|| proxy_id.clone()),
                );
                proxy.created_timestamp = created;
                // the startup timestamp is not stored on the container, so the creation time is used
                // (the difference only matters for the events, as the Java comment says)
                proxy.startup_timestamp = created;
                proxy.user_id = info
                    .runtime_values
                    .get(&USER_ID)
                    .map(|value| value.to_value_string());
                proxy.display_name = info
                    .runtime_values
                    .get(&DISPLAY_NAME)
                    .map(|value| value.to_value_string());
                // the values of the proxy itself, not those of its containers
                for value in info.runtime_values.iter() {
                    if !value.key.container_specific {
                        proxy.add_runtime_value(value.clone(), true);
                    }
                }
                proxy
            });

            let index = info
                .runtime_values
                .get(&CONTAINER_INDEX)
                .and_then(|value| value.to_value_string().parse::<i64>().ok())
                .unwrap_or_default();
            let mut container = Container::new(index);
            container.id = Some(info.id.clone());
            let mut container_values = RuntimeValues::new();
            for value in info.runtime_values.iter() {
                if value.key.container_specific {
                    container_values.add(value.clone(), true);
                }
            }
            container.runtime_values = container_values;

            // the backend knows how a container that already exists is reached
            for (name, url) in backend
                .existing_targets(&container, &info.port_bindings)
                .await
            {
                proxy.targets.insert(name, url);
            }
            proxy.containers.push(container);
        }

        let recovered: Vec<Proxy> = proxies.into_values().collect();
        for proxy in &recovered {
            tracing::info!(
                "Recovered running proxy [proxyId: {}] [specId: {}] [userId: {}]",
                proxy.id,
                proxy.spec_id.clone().unwrap_or_default(),
                proxy.user_id.clone().unwrap_or_default()
            );
            store.add_proxy(proxy);
            heartbeats.update(&proxy.id, crate::model::proxy::now_millis());
        }

        self.ready.store(true, Ordering::SeqCst);
        recovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{BackendError, ExistingContainerInfo, StartContext, StartedContainer};
    use crate::model::runtime_value::{RuntimeValue, PORT_MAPPINGS};
    use crate::service::runtime_values::PortMappingEntry;
    use crate::service::runtime_values::PortMappings;
    use crate::store::{MemoryHeartbeatStore, MemoryProxyStore};
    use async_trait::async_trait;

    /// A backend that answers with a fixed list of containers.
    #[derive(Debug)]
    struct ScanBackend {
        containers: Vec<ExistingContainerInfo>,
    }

    #[async_trait]
    impl ContainerBackend for ScanBackend {
        fn name(&self) -> &'static str {
            "scan"
        }

        async fn start_container(
            &self,
            _context: StartContext<'_>,
        ) -> Result<StartedContainer, BackendError> {
            unimplemented!("not used in these tests")
        }

        async fn stop_proxy(&self, _proxy: &Proxy) -> Result<(), BackendError> {
            Ok(())
        }

        async fn scan_existing_containers(
            &self,
        ) -> Result<Vec<ExistingContainerInfo>, BackendError> {
            Ok(self.containers.clone())
        }

        /// The targets of an existing container, like the Docker backend computes them.
        async fn existing_targets(
            &self,
            container: &crate::model::proxy::Container,
            port_bindings: &BTreeMap<u16, u16>,
        ) -> BTreeMap<String, String> {
            crate::backend::target::targets_from_stored_mappings(
                container,
                port_bindings,
                "http",
                "localhost",
                false,
            )
        }
    }

    fn container(instance_id: &str, proxy_id: &str, index: i64) -> ExistingContainerInfo {
        let mut values = RuntimeValues::new();
        for (key, value) in [
            (&PROXY_ID, proxy_id.to_string()),
            (&PROXY_SPEC_ID, "01_hello".to_string()),
            (&INSTANCE_ID, instance_id.to_string()),
            (&USER_ID, "jack".to_string()),
            (&DISPLAY_NAME, "Hello Application".to_string()),
            (&TARGET_ID, proxy_id.to_string()),
            (&CREATED_TIMESTAMP, "1700000000000".to_string()),
        ] {
            values.add(RuntimeValue::string(key, value), true);
        }
        values.add(RuntimeValue::integer(&CONTAINER_INDEX, index), true);
        values.add(
            RuntimeValue::json(
                &PORT_MAPPINGS,
                PortMappings {
                    port_mappings: vec![PortMappingEntry {
                        name: "default".to_string(),
                        port: 3838,
                        target_path: String::new(),
                    }],
                },
            ),
            true,
        );

        ExistingContainerInfo {
            id: format!("container-{proxy_id}-{index}"),
            runtime_values: values,
            image: Some("sp-testapp:test".to_string()),
            port_bindings: BTreeMap::from([(3838u16, 20000u16 + index as u16)]),
        }
    }

    fn build_service(yaml: &str) -> AppRecoveryService {
        let settings: Settings = serde_yaml_ng::from_str(yaml).expect("settings");
        let identifiers = Identifiers {
            runtime_id: "runtime".to_string(),
            instance_id: "instance-1".to_string(),
            realm_id: None,
            version: None,
        };
        AppRecoveryService::new(&settings, &identifiers)
    }

    async fn recover(
        service: &AppRecoveryService,
        containers: Vec<ExistingContainerInfo>,
    ) -> (Vec<Proxy>, Arc<dyn ProxyStore>) {
        let backend: Arc<dyn ContainerBackend> = Arc::new(ScanBackend { containers });
        let store: Arc<dyn ProxyStore> = Arc::new(MemoryProxyStore::new(false));
        let heartbeats: Arc<dyn HeartbeatStore> = Arc::new(MemoryHeartbeatStore::new());
        let recovered = service.recover(&backend, &store, &heartbeats).await;
        (recovered, store)
    }

    #[tokio::test]
    async fn does_nothing_when_recovery_is_disabled() {
        let service = build_service("proxy:\n  authentication: none\n");
        assert!(!service.enabled());
        let (recovered, store) =
            recover(&service, vec![container("instance-1", "proxy-1", 0)]).await;
        assert!(recovered.is_empty());
        assert_eq!(store.count(), 0);
        assert!(
            service.is_ready(),
            "the server is ready even without recovery"
        );
    }

    #[tokio::test]
    async fn recovers_containers_of_the_same_configuration() {
        let service = build_service("proxy:\n  recover-running-proxies: true\n");
        let (recovered, store) = recover(
            &service,
            vec![
                container("instance-1", "proxy-1", 0),
                container("other-instance", "proxy-2", 0),
            ],
        )
        .await;

        assert_eq!(recovered.len(), 1, "only the container of this instance");
        let proxy = &recovered[0];
        assert_eq!(proxy.id, "proxy-1");
        assert_eq!(proxy.status, ProxyStatus::Up);
        assert_eq!(proxy.spec_id.as_deref(), Some("01_hello"));
        assert_eq!(proxy.user_id.as_deref(), Some("jack"));
        assert_eq!(proxy.display_name.as_deref(), Some("Hello Application"));
        assert_eq!(proxy.created_timestamp, 1_700_000_000_000);
        assert_eq!(
            proxy.startup_timestamp, proxy.created_timestamp,
            "the startup timestamp is not stored on the container"
        );
        assert_eq!(proxy.containers.len(), 1);
        assert_eq!(
            proxy.containers[0].id.as_deref(),
            Some("container-proxy-1-0")
        );
        assert_eq!(
            proxy.targets.get(""),
            Some(&"http://localhost:20000".to_string()),
            "the target is rebuilt from the stored port mappings"
        );
        // the values of the proxy are separated from those of the container
        assert!(proxy.runtime_values.get(&USER_ID).is_some());
        assert!(proxy.runtime_values.get(&CONTAINER_INDEX).is_none());
        assert!(proxy.containers[0]
            .runtime_values
            .get(&CONTAINER_INDEX)
            .is_some());

        assert_eq!(store.count(), 1);
        assert!(service.is_ready());
    }

    #[tokio::test]
    async fn recovers_containers_of_other_configurations_when_allowed() {
        let service = build_service(
            "proxy:\n  recover-running-proxies: true\n  \
             recover-running-proxies-from-different-config: true\n",
        );
        let (recovered, _) = recover(
            &service,
            vec![
                container("instance-1", "proxy-1", 0),
                container("other-instance", "proxy-2", 0),
            ],
        )
        .await;
        assert_eq!(recovered.len(), 2);
    }

    #[tokio::test]
    async fn groups_the_containers_of_one_app() {
        let service = build_service("proxy:\n  recover-running-proxies: true\n");
        let (recovered, _) = recover(
            &service,
            vec![
                container("instance-1", "proxy-1", 0),
                container("instance-1", "proxy-1", 1),
            ],
        )
        .await;
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].containers.len(), 2);
    }

    #[test]
    fn decides_which_containers_may_be_recovered() {
        let service = build_service("proxy:\n  recover-running-proxies: true\n");
        assert!(service.can_recover(Some("instance-1")));
        assert!(!service.can_recover(Some("other")));
        assert!(!service.can_recover(None));

        let service = build_service(
            "proxy:\n  recover-running-proxies: true\n  \
             recover-running-proxies-from-different-config: true\n",
        );
        assert!(service.can_recover(Some("other")));
        assert!(
            !service.can_recover(None),
            "a missing instance id is never recovered"
        );
    }
}
