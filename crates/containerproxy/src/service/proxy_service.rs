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

//! The proxy lifecycle (`ProxyService`).
//!
//! Starting an app is a two step operation, exactly like in the Java implementation: the *blocking*
//! part validates the request and registers the proxy (so that the UI immediately sees a proxy in state
//! `New`), the *asynchronous* part resolves the expressions, starts the containers, waits until the app
//! responds and finally marks the proxy as `Up`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use crate::backend::{BackendError, ContainerBackend, StartContext};
use crate::config::Settings;
use crate::events::{Event, EventBus};
use crate::model::proxy::{now_millis, Container, Proxy, ProxyStatus, ProxyStopReason};
use crate::model::runtime_value::{RuntimeValue, TARGET_ID};
use crate::model::spec::ProxySpec;
use crate::service::identifier::Identifiers;
use crate::service::runtime_values::RuntimeValueService;
use crate::spec::expression::{ExpressionContextBuilder, SpelResolver, UserContext};
use crate::store::{HeartbeatStore, ProxyStore};

/// Why a proxy could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The user may not use this app.
    #[error("Cannot start proxy {0}: access denied")]
    AccessDenied(String),
    /// The server or the app reached its capacity.
    #[error("{0}")]
    Validation(String),
    /// The app definition could not be resolved (an expression failed).
    #[error("Container failed to start: {0}")]
    Resolve(String),
    /// The backend could not start the containers.
    #[error("Container failed to start: {0}")]
    Backend(String),
    /// The app did not respond in time.
    #[error("Container did not respond in time")]
    Timeout,
    /// The proxy or spec does not exist.
    #[error("{0}")]
    NotFound(String),
}

/// Runs and stops proxies.
pub struct ProxyService {
    settings: Arc<Settings>,
    store: Arc<dyn ProxyStore>,
    heartbeats: Arc<dyn HeartbeatStore>,
    backend: Arc<dyn ContainerBackend>,
    runtime_values: RuntimeValueService,
    events: EventBus,
    /// Proxies that currently have a long running action, mirroring `actionsInProgress`.
    actions_in_progress: DashMap<String, ()>,
    /// When the last proxy was stopped, used by `is_busy` (`/actuator/recyclable`).
    last_stop: DashMap<(), i64>,
    shutting_down: AtomicBool,
}

impl std::fmt::Debug for ProxyService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyService")
            .field("backend", &self.backend.name())
            .field("proxies", &self.store.count())
            .finish()
    }
}

impl ProxyService {
    /// Creates the service.
    pub fn new(
        settings: Arc<Settings>,
        identifiers: &Identifiers,
        store: Arc<dyn ProxyStore>,
        heartbeats: Arc<dyn HeartbeatStore>,
        backend: Arc<dyn ContainerBackend>,
        events: EventBus,
    ) -> Self {
        let runtime_values = RuntimeValueService::new(&settings, identifiers);
        ProxyService {
            settings,
            store,
            heartbeats,
            backend,
            runtime_values,
            events,
            actions_in_progress: DashMap::new(),
            last_stop: DashMap::new(),
            shutting_down: AtomicBool::new(false),
        }
    }

    /// The event bus, so that callers can subscribe.
    pub fn events(&self) -> &EventBus {
        &self.events
    }

    /// The container backend.
    pub fn backend(&self) -> &Arc<dyn ContainerBackend> {
        &self.backend
    }

    /// All proxies.
    pub fn all_proxies(&self) -> Vec<Proxy> {
        self.store.all_proxies()
    }

    /// All proxies in state `Up`.
    pub fn all_up_proxies(&self) -> Vec<Proxy> {
        self.store
            .all_proxies()
            .into_iter()
            .filter(|proxy| proxy.status == ProxyStatus::Up)
            .collect()
    }

    /// The proxy with the given id, without any access check.
    pub fn proxy(&self, proxy_id: &str) -> Option<Proxy> {
        self.store.proxy(proxy_id)
    }

    /// The proxies of a user.
    pub fn user_proxies(&self, user_id: &str) -> Vec<Proxy> {
        self.store.user_proxies(user_id)
    }

    /// The proxies of a user for one app.
    pub fn user_proxies_by_spec(&self, user_id: &str, spec_id: &str) -> Vec<Proxy> {
        self.store
            .user_proxies(user_id)
            .into_iter()
            .filter(|proxy| proxy.spec_id.as_deref() == Some(spec_id))
            .collect()
    }

    /// Whether a long running action is in progress, or a proxy was stopped less than a minute ago.
    ///
    /// This is what `/actuator/recyclable` reports, so that a deployment does not replace a server that
    /// is still busy.
    pub fn is_busy(&self) -> bool {
        if !self.actions_in_progress.is_empty() {
            return true;
        }
        match self.last_stop.get(&()) {
            Some(entry) => now_millis() - *entry.value() <= 60_000,
            None => false,
        }
    }

    /// Registers a proxy in state `New` and returns it (the blocking part of `startProxy`).
    ///
    /// The caller is responsible for the access check, exactly like in the Java implementation.
    pub fn create_proxy(
        &self,
        proxy_id: &str,
        user: &UserContext,
        spec: &ProxySpec,
        runtime_values: Vec<RuntimeValue>,
    ) -> Result<Proxy, StartError> {
        self.validate_capacity(spec)?;

        let mut proxy = Proxy::new(proxy_id, ProxyStatus::New);
        proxy.user_id = Some(user.user_id.clone());
        proxy.spec_id = Some(spec.id.clone());
        proxy.created_timestamp = now_millis();
        proxy.display_name = Some(spec.display_name_or_id().to_string());
        proxy.add_runtime_value(RuntimeValue::string(&TARGET_ID, proxy_id), false);
        for value in runtime_values {
            proxy.add_runtime_value(value, false);
        }

        self.actions_in_progress.insert(proxy.id.clone(), ());
        self.store.add_proxy(&proxy);
        Ok(proxy)
    }

    /// Starts the containers of a proxy (the asynchronous part of `startProxy`).
    pub async fn start_proxy(
        &self,
        proxy: Proxy,
        spec: &ProxySpec,
        user: &UserContext,
    ) -> Result<Proxy, StartError> {
        let proxy_id = proxy.id.clone();
        let result = self.start_proxy_inner(proxy, spec, user).await;
        self.finish_action(&proxy_id);
        result
    }

    async fn start_proxy_inner(
        &self,
        proxy: Proxy,
        spec: &ProxySpec,
        user: &UserContext,
    ) -> Result<Proxy, StartError> {
        let started_at = now_millis();
        let (mut proxy, resolved_spec) = self.prepare_for_start(proxy, spec, user)?;

        tracing::info!(
            "Starting proxy [user: {}] [proxyId: {}]",
            user.user_id,
            proxy.id
        );

        for container_spec in &resolved_spec.container_specs {
            let container = proxy
                .container(container_spec.index)
                .cloned()
                .unwrap_or_else(|| Container::new(container_spec.index));

            let environment = self.container_environment(&proxy, container_spec);
            let labels = self.container_labels(&proxy, &container, container_spec);

            let started = self
                .backend
                .start_container(StartContext {
                    user: Some(user),
                    proxy: &proxy,
                    spec: &resolved_spec,
                    container_spec,
                    container: &container,
                    environment,
                    labels,
                })
                .await;

            match started {
                Ok(started) => {
                    let container = proxy.container_mut(container_spec.index);
                    container.id = started.id;
                    for value in started.runtime_values.iter() {
                        container.add_runtime_value(value.clone(), true);
                    }
                    for (mapping, target) in started.targets {
                        proxy.targets.insert(mapping, target);
                    }
                }
                Err(error) => {
                    self.fail_start(&proxy, &format!("{error}")).await;
                    return Err(StartError::Backend(error.to_string()));
                }
            }
        }

        // the app has to answer before the proxy is considered up
        if !self.wait_until_reachable(&proxy).await {
            self.fail_start(&proxy, "Container did not respond in time")
                .await;
            return Err(StartError::Timeout);
        }

        // the user may have stopped the app while it was starting
        if self.cleanup_if_stopped(&proxy).await {
            return Err(StartError::NotFound(format!(
                "Proxy {} was stopped while starting",
                proxy.id
            )));
        }

        proxy.status = ProxyStatus::Up;
        proxy.startup_timestamp = now_millis();
        self.store.update_proxy(&proxy);
        self.heartbeats.update(&proxy.id, now_millis());

        tracing::info!(
            "Proxy activated [user: {}] [proxyId: {}]",
            user.user_id,
            proxy.id
        );
        self.events.publish(Event::ProxyStarted {
            proxy: Box::new(proxy.clone()),
            startup_time_ms: Some(now_millis() - started_at),
        });

        Ok(proxy)
    }

    /// Stops a proxy and removes it from the store.
    pub async fn stop_proxy(
        &self,
        proxy: &Proxy,
        reason: ProxyStopReason,
    ) -> Result<(), StartError> {
        self.actions_in_progress.insert(proxy.id.clone(), ());

        let stopping = proxy.with_status(ProxyStatus::Stopping);
        self.store.update_proxy(&stopping);

        if let Err(error) = self.backend.stop_proxy(&stopping).await {
            tracing::warn!("Failed to stop proxy [proxyId: {}]: {error}", proxy.id);
        }

        let stopped = stopping.with_status(ProxyStatus::Stopped);
        self.store.remove_proxy(&stopped);
        self.heartbeats.remove(&stopped.id);
        tracing::info!("Proxy released [proxyId: {}]", stopped.id);

        self.events.publish(Event::ProxyStopped {
            proxy: Box::new(stopped),
            reason,
        });
        self.finish_action(&proxy.id);
        Ok(())
    }

    /// Stops every proxy, used when the server shuts down (`proxy.stop-proxies-on-shutdown`).
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if !self.settings.proxy.stop_proxies_on_shutdown() {
            tracing::info!("Leaving running apps alone (stop-proxies-on-shutdown is false)");
            return;
        }
        for proxy in self.store.all_proxies() {
            if let Err(error) = self.stop_proxy(&proxy, ProxyStopReason::Shutdown).await {
                tracing::error!("Error during shutdown: {error}");
            }
        }
    }

    /// Whether the server is shutting down.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Whether the app of a proxy is still healthy: the backend agrees and the app answers.
    pub async fn is_proxy_healthy(&self, proxy: &Proxy) -> bool {
        if self.backend.supports_health_check() {
            match self.backend.is_proxy_healthy(proxy).await {
                Ok(false) => return false,
                Err(error) => {
                    tracing::warn!("cannot check health of proxy {}: {error}", proxy.id);
                    return false;
                }
                Ok(true) => {}
            }
        }
        if proxy.targets.is_empty() {
            tracing::info!("Proxy failed: no targets available [proxyId: {}]", proxy.id);
            return false;
        }
        self.probe_target(proxy).await
    }

    /// Resolves the expressions of the app definition and adds the runtime values.
    fn prepare_for_start(
        &self,
        mut proxy: Proxy,
        spec: &ProxySpec,
        user: &UserContext,
    ) -> Result<(Proxy, ProxySpec), StartError> {
        self.runtime_values
            .add_before_expressions(&mut proxy, spec, Some(user));

        let resolver = self.resolver(&proxy, spec, user);
        let resolved = spec
            .first_resolve(&resolver)
            .map_err(|error| StartError::Resolve(error.to_string()))?;

        self.runtime_values
            .add_after_expressions(&mut proxy, &resolved);

        // create the container objects of a new proxy
        if proxy.containers.is_empty() {
            for container_spec in &resolved.container_specs {
                let container = proxy.container_mut(container_spec.index);
                let mut new_container = container.clone();
                self.runtime_values
                    .add_container_values(&mut new_container, container_spec);
                *container = new_container;
            }
        }

        let resolver = self.resolver(&proxy, &resolved, user);
        let resolved = resolved
            .final_resolve(&resolver)
            .map_err(|error| StartError::Resolve(error.to_string()))?;

        self.runtime_values
            .add_after_final_expressions(&mut proxy, &resolved);
        self.store.update_proxy(&proxy);

        Ok((proxy, resolved))
    }

    fn resolver(&self, proxy: &Proxy, spec: &ProxySpec, user: &UserContext) -> SpelResolver {
        SpelResolver::new(
            ExpressionContextBuilder::new()
                .process_environment()
                .user(user.clone())
                .proxy(proxy.clone())
                .spec(spec.clone())
                .build(),
        )
    }

    /// Environment variables of a container: runtime values, `container-env-file` and `container-env`.
    fn container_environment(
        &self,
        proxy: &Proxy,
        container_spec: &crate::model::spec::ContainerSpec,
    ) -> BTreeMap<String, String> {
        let mut environment = proxy.runtime_values.environment();

        if let Some(path) = container_spec.env_file.as_str() {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    for line in content.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        if let Some((name, value)) = line.split_once('=') {
                            environment.insert(name.trim().to_string(), value.trim().to_string());
                        }
                    }
                }
                Err(error) => tracing::warn!("cannot read container-env-file {path}: {error}"),
            }
        }

        if let Some(configured) = container_spec.env.value() {
            for (name, value) in configured {
                environment.insert(name.clone(), value.clone());
            }
        }

        environment
    }

    /// Labels of a container: runtime values of the proxy and the container, plus `labels`.
    fn container_labels(
        &self,
        proxy: &Proxy,
        container: &Container,
        container_spec: &crate::model::spec::ContainerSpec,
    ) -> BTreeMap<String, String> {
        let mut labels = container_spec.labels.value().cloned().unwrap_or_default();
        labels.extend(proxy.runtime_values.labels());
        labels.extend(container.runtime_values.labels());
        labels
    }

    /// Checks `proxy.max-total-instances` and the `max-total-instances` of the app.
    fn validate_capacity(&self, spec: &ProxySpec) -> Result<(), StartError> {
        const MESSAGE: &str =
            "The server does not have enough capacity to start this app, please try again later.";

        let max_total = self.settings.proxy.max_total_instances();
        if max_total >= 0 && self.store.count() as i64 >= max_total {
            return Err(StartError::Validation(MESSAGE.to_string()));
        }
        if spec.max_total_instances >= 0
            && self.store.count_by_spec(&spec.id) as i64 >= spec.max_total_instances
        {
            return Err(StartError::Validation(MESSAGE.to_string()));
        }
        Ok(())
    }

    /// Waits until the app answers on its default target.
    async fn wait_until_reachable(&self, proxy: &Proxy) -> bool {
        let timeout =
            Duration::from_millis(self.settings.proxy.container_wait_timeout_ms().max(0) as u64);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.probe_target(proxy).await {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Sends a request to the default target and checks the status code, as `isProxyHealthy` does.
    async fn probe_target(&self, proxy: &Proxy) -> bool {
        let Some(target) = proxy.default_target() else {
            return false;
        };
        let url = format!("{}/", target.trim_end_matches('/'));
        let timeout =
            Duration::from_millis(self.settings.proxy.container_wait_timeout_ms().max(0) as u64);
        let client = match reqwest::Client::builder()
            .timeout(timeout.max(Duration::from_millis(500)))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!("cannot create http client: {error}");
                return false;
            }
        };
        match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let acceptable = [200, 301, 302, 303, 307, 308].contains(&status);
                if !acceptable {
                    tracing::info!(
                        "Proxy failed: HTTP connection attempt returned invalid status: {status} [proxyId: {}]",
                        proxy.id
                    );
                }
                acceptable
            }
            Err(error) => {
                tracing::debug!("Proxy not (yet) reachable [proxyId: {}]: {error}", proxy.id);
                false
            }
        }
    }

    /// Cleans up after a failed start: stop what exists, remove the proxy and publish the event.
    async fn fail_start(&self, proxy: &Proxy, message: &str) {
        tracing::warn!("Proxy failed to start [proxyId: {}]: {message}", proxy.id);
        if let Err(error) = self.backend.stop_proxy(proxy).await {
            tracing::warn!(
                "Error while stopping failed proxy [proxyId: {}]: {error}",
                proxy.id
            );
        }
        self.store.remove_proxy(proxy);
        self.heartbeats.remove(&proxy.id);
        self.events.publish(Event::ProxyStartFailed {
            proxy: Box::new(proxy.clone()),
        });
    }

    /// Stops a proxy that was stopped by the user while it was starting.
    async fn cleanup_if_stopped(&self, starting: &Proxy) -> bool {
        match self.store.proxy(&starting.id) {
            Some(proxy)
                if proxy.status != ProxyStatus::Stopped
                    && proxy.status != ProxyStatus::Stopping =>
            {
                false
            }
            _ => {
                if let Err(error) = self.backend.stop_proxy(starting).await {
                    tracing::warn!("Error while cleaning up pending proxy: {error}");
                }
                tracing::info!("Pending proxy cleaned up [proxyId: {}]", starting.id);
                true
            }
        }
    }

    fn finish_action(&self, proxy_id: &str) {
        self.actions_in_progress.remove(proxy_id);
        self.last_stop.insert((), now_millis());
    }
}

/// Converts a backend error into a start error.
impl From<BackendError> for StartError {
    fn from(error: BackendError) -> Self {
        StartError::Backend(error.to_string())
    }
}
