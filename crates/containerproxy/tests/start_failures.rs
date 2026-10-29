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

//! What happens when a start goes wrong, with a backend that fails on purpose: the container the backend
//! created is removed, and the reachability check waits for a slow app as `ShinyProxyTestStrategy` does.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use containerproxy::backend::{BackendError, ContainerBackend, StartContext, StartedContainer};
use containerproxy::config::{load, LoadOptions, Schema, Settings};
use containerproxy::events::EventBus;
use containerproxy::model::proxy::{Proxy, ProxyStatus};
use containerproxy::model::spec::{ContainerSpec, ProxySpec};
use containerproxy::service::identifier::Identifiers;
use containerproxy::service::proxy_service::wait_until_reachable;
use containerproxy::service::ProxyService;
use containerproxy::spec::expression::UserContext;
use containerproxy::store::{MemoryHeartbeatStore, MemoryProxyStore};

/// A backend whose containers are created but never start.
#[derive(Debug, Default)]
struct FailingBackend {
    /// The container ids `stop_proxy` was asked to remove.
    stopped: Mutex<Vec<Option<String>>>,
}

#[async_trait]
impl ContainerBackend for FailingBackend {
    fn name(&self) -> &'static str {
        "failing"
    }

    async fn start_container(
        &self,
        _context: StartContext<'_>,
    ) -> Result<StartedContainer, BackendError> {
        let created = StartedContainer {
            id: Some("created-container".into()),
            ..StartedContainer::default()
        };
        Err(BackendError::FailedToStart("cannot start container".into()).after_create(&created))
    }

    async fn stop_proxy(&self, proxy: &Proxy) -> Result<(), BackendError> {
        let mut stopped = self.stopped.lock().unwrap();
        stopped.extend(
            proxy
                .containers
                .iter()
                .map(|container| container.id.clone()),
        );
        Ok(())
    }
}

fn service(backend: Arc<FailingBackend>) -> ProxyService {
    let settings: Settings = serde_yaml_ng::from_str("proxy: {}").expect("settings");
    let directory = tempfile::tempdir().expect("temp dir");
    let options = LoadOptions {
        working_dir: Some(directory.path().to_path_buf()),
        ..LoadOptions::default()
    };
    let raw = load(&Schema::engine(), &options).expect("config");
    ProxyService::new(
        Arc::new(settings),
        &Identifiers::from_config(&raw, None),
        Arc::new(MemoryProxyStore::new(true)),
        Arc::new(MemoryHeartbeatStore::new()),
        backend,
        EventBus::new(),
    )
}

#[tokio::test]
async fn a_container_that_fails_after_its_creation_is_removed() {
    let backend = Arc::new(FailingBackend::default());
    let service = service(backend.clone());
    let mut spec = ProxySpec::new("app");
    spec.container_specs = vec![ContainerSpec::default()];
    spec.set_container_index();
    let user = UserContext::new("jack", vec![]);

    let proxy = service
        .create_proxy("proxy-1", &user, &spec, vec![])
        .expect("proxy is created");
    let result = service.start_proxy(proxy, &spec, &user).await;

    assert!(result.is_err());
    assert_eq!(
        *backend.stopped.lock().unwrap(),
        vec![Some("created-container".to_string())]
    );
    assert!(service.proxy("proxy-1").is_none());
}

/// An app that takes `delay` before it sends its answer.
async fn slow_app(delay: Duration) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                tokio::time::sleep(delay).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    format!("http://{address}")
}

#[tokio::test]
async fn an_app_that_answers_slowly_is_reachable() {
    // slower than the 200 ms of the first attempts, faster than proxy.container-wait-timeout
    let target = slow_app(Duration::from_millis(1_000)).await;
    let mut proxy = Proxy::new("proxy-1", ProxyStatus::New);
    proxy.targets = BTreeMap::from([(String::new(), target)]);

    let backend = FailingBackend::default();
    assert!(
        wait_until_reachable(
            &proxy,
            &backend,
            Duration::from_secs(10),
            Duration::from_secs(5)
        )
        .await
    );
    assert!(
        !wait_until_reachable(
            &proxy,
            &backend,
            Duration::from_secs(3),
            Duration::from_millis(500)
        )
        .await,
        "a request timeout below the answer time of the app never succeeds"
    );
}
