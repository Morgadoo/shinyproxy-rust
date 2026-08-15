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

//! Keeps enough seats available (`ProxySharingScaler`).
//!
//! The scaler runs on the leader of the realm. Every ten seconds it compares the number of seats that can be
//! claimed with `minimum-seats-available` and starts or stops delegate proxies; every twenty seconds it
//! removes the containers that are marked for removal and whose seats are all free.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::Mutex;

use super::store::{DelegateProxyStore, SeatStore};
use super::{DelegateProxy, DelegateProxyStatus, ProxySharingSpecExtension, Seat};
use crate::backend::{ContainerBackend, StartContext};
use crate::config::Settings;
use crate::model::proxy::{now_millis, Container, Proxy, ProxyStatus};
use crate::model::runtime_value::{
    RuntimeValue, CREATED_TIMESTAMP, DELEGATE_PROXY, INSTANCE_ID, PROXY_ID, PROXY_SPEC_ID,
    PUBLIC_PATH, REALM_ID, TARGET_ID,
};
use crate::model::spec::ProxySpec;
use crate::service::identifier::Identifiers;
use crate::service::runtime_values::RuntimeValueService;
use crate::service::LeaderService;
use crate::spec::expression::ExpressionContextBuilder;
use crate::spec::expression::SpelResolver;

/// How often the scaler checks whether it has to scale (`scheduleReconcile`).
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(10);

/// How often the scaler removes the containers that are marked for removal (`scheduleCleanup`).
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(20);

/// What the scaler did the last time it looked (`ReconcileStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileStatus {
    /// Nothing to do.
    Stable,
    /// Containers are being created.
    ScaleUp,
    /// Containers are being removed.
    ScaleDown,
}

/// Creates and removes the pre-started containers of one app definition.
pub struct ProxySharingScaler {
    spec: ProxySpec,
    extension: ProxySharingSpecExtension,
    /// Hash of the app definition, so containers of another configuration are recognised.
    spec_hash: String,
    seats: Arc<dyn SeatStore>,
    delegates: Arc<dyn DelegateProxyStore>,
    backend: Arc<dyn ContainerBackend>,
    runtime_values: RuntimeValueService,
    identifiers: Identifiers,
    leader: Arc<dyn LeaderService>,
    settings: Arc<Settings>,
    /// Where the seats of the app are published (`/api/route/`).
    public_path_prefix: String,
    /// How many users are waiting for a seat, reported by the dispatcher.
    pending_users: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Seats of containers that are still being created.
    pending_seats: AtomicI64,
    status: Mutex<ReconcileStatus>,
    last_scale_up: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for ProxySharingScaler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxySharingScaler")
            .field("specId", &self.spec.id)
            .field(
                "minimumSeatsAvailable",
                &self.extension.minimum_seats_available(),
            )
            .field("seatsPerContainer", &self.extension.seats_per_container())
            .finish()
    }
}

impl ProxySharingScaler {
    /// Creates the scaler of an app definition.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        spec: ProxySpec,
        seats: Arc<dyn SeatStore>,
        delegates: Arc<dyn DelegateProxyStore>,
        backend: Arc<dyn ContainerBackend>,
        settings: Arc<Settings>,
        identifiers: &Identifiers,
        leader: Arc<dyn LeaderService>,
        public_path_prefix: impl Into<String>,
        pending_users: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        let extension = ProxySharingSpecExtension::of(&spec);
        let spec_hash = spec_hash(&spec);
        ProxySharingScaler {
            spec,
            extension,
            spec_hash,
            seats,
            delegates,
            backend,
            runtime_values: RuntimeValueService::new(&settings, identifiers),
            identifiers: identifiers.clone(),
            leader,
            settings,
            public_path_prefix: public_path_prefix.into(),
            pending_users,
            pending_seats: AtomicI64::new(0),
            status: Mutex::new(ReconcileStatus::Stable),
            last_scale_up: Mutex::new(None),
        }
    }

    /// The app definition this scaler serves.
    pub fn spec(&self) -> &ProxySpec {
        &self.spec
    }

    /// What the scaler did the last time it looked.
    pub fn status(&self) -> ReconcileStatus {
        *self.status.lock().expect("the status is not poisoned")
    }

    /// Seats of containers that are still being created.
    pub fn pending_seats(&self) -> i64 {
        self.pending_seats.load(Ordering::SeqCst)
    }

    /// The seats of this app definition.
    pub fn seats(&self) -> &Arc<dyn SeatStore> {
        &self.seats
    }

    /// Every delegate proxy of this app definition.
    pub fn delegate_proxies(&self) -> Vec<DelegateProxy> {
        self.delegates.all_delegate_proxies()
    }

    /// Runs the reconcile and cleanup loops until the process ends.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let cleanup = self.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(CLEANUP_INTERVAL);
            loop {
                timer.tick().await;
                cleanup.cleanup().await;
            }
        });

        tokio::spawn(async move {
            // the first round happens immediately, so the seats exist shortly after the server starts
            let mut timer = tokio::time::interval(RECONCILE_INTERVAL);
            loop {
                timer.tick().await;
                self.reconcile().await;
            }
        })
    }

    /// Compares the available seats with the configuration and scales if needed.
    pub async fn reconcile(&self) {
        if !self.leader.is_leader() {
            return;
        }

        let unclaimed = self.seats.unclaimed_count();
        let pending_seats = self.pending_seats();
        let pending_users = (self.pending_users)();
        let available = unclaimed + pending_seats - pending_users;
        let minimum = self.extension.minimum_seats_available();
        let per_container = self.extension.seats_per_container();

        tracing::debug!(
            "Status: {:?}, Unclaimed: {unclaimed} + PendingDelegate: {pending_seats} - \
             PendingDelegating: {pending_users} = {available} -> minimum: {minimum} [specId: {}]",
            self.status(),
            self.spec.id
        );

        if available < minimum {
            // never start more containers than `max-total-instances` allows
            let maximum = self.spec.max_total_instances;
            if maximum > -1 && self.seats.count() >= maximum {
                tracing::warn!(
                    "Not scaling up: currently {} seats, scale up would create more than maximum \
                     number of instances: {maximum} [specId: {}]",
                    self.seats.count(),
                    self.spec.id
                );
                return;
            }
            *self.status.lock().expect("the status is not poisoned") = ReconcileStatus::ScaleUp;
            let missing = minimum - available;
            // ceiling division; `div_ceil` is not stable for signed integers
            let containers = (missing + per_container - 1) / per_container;
            self.scale_up(containers).await;
            *self
                .last_scale_up
                .lock()
                .expect("the timestamp is not poisoned") = Some(Instant::now());
        } else if pending_seats > 0 {
            *self.status.lock().expect("the status is not poisoned") = ReconcileStatus::ScaleUp;
            *self
                .last_scale_up
                .lock()
                .expect("the timestamp is not poisoned") = Some(Instant::now());
        } else if available - minimum >= per_container {
            let containers = (available - minimum) / per_container;
            if containers <= 0 {
                return;
            }
            // a container is only removed when the last scale up is long enough ago
            if let Some(last_scale_up) = *self
                .last_scale_up
                .lock()
                .expect("the timestamp is not poisoned")
            {
                let delay = self.extension.scale_down_delay();
                if last_scale_up.elapsed() < delay {
                    tracing::info!(
                        "Not scaling down because last scaleUp was {} minutes ago ({containers} \
                         proxies to remove, delay is {} minutes) [specId: {}]",
                        last_scale_up.elapsed().as_secs() / 60,
                        delay.as_secs() / 60,
                        self.spec.id
                    );
                    return;
                }
            }
            *self.status.lock().expect("the status is not poisoned") = ReconcileStatus::ScaleDown;
            self.scale_down(containers).await;
        } else {
            *self.status.lock().expect("the status is not poisoned") = ReconcileStatus::Stable;
            tracing::debug!("No scaling required [specId: {}]", self.spec.id);
        }
    }

    /// Creates `count` delegate proxies.
    async fn scale_up(&self, count: i64) {
        tracing::info!(
            "Scale up required, trying to create {count} DelegateProxies [specId: {}]",
            self.spec.id
        );
        for _ in 0..count {
            let id = uuid::Uuid::new_v4().to_string();
            let delegate =
                DelegateProxy::pending(Proxy::new(&id, ProxyStatus::New), self.spec_hash.clone());
            self.delegates.add_delegate_proxy(delegate.clone());
            self.pending_seats
                .fetch_add(self.extension.seats_per_container(), Ordering::SeqCst);

            match self.create_delegate_proxy(delegate).await {
                Ok(()) => {}
                Err(error) => tracing::error!(
                    "Failed to start DelegateProxy: {error} [specId: {}] [delegateProxyId: {id}]",
                    self.spec.id
                ),
            }
            self.pending_seats
                .fetch_sub(self.extension.seats_per_container(), Ordering::SeqCst);
        }
    }

    /// Starts one delegate proxy and publishes its seats.
    async fn create_delegate_proxy(&self, delegate: DelegateProxy) -> Result<(), String> {
        let id = delegate.proxy.id.clone();
        tracing::info!(
            "Preparing DelegateProxy [specId: {}] [delegateProxyId: {id}]",
            self.spec.id
        );

        // the proxy of a delegate proxy has no user; it carries the runtime values Java gives it
        let mut proxy = delegate.proxy.clone();
        proxy.status = ProxyStatus::New;
        proxy.spec_id = Some(self.spec.id.clone());
        proxy.target_id = Some(id.clone());
        proxy.created_timestamp = now_millis();
        proxy.add_runtime_value(RuntimeValue::boolean(&DELEGATE_PROXY, true), false);
        proxy.add_runtime_value(
            RuntimeValue::string(&PUBLIC_PATH, self.public_path(&id)),
            false,
        );
        proxy.add_runtime_value(
            RuntimeValue::string(&INSTANCE_ID, self.identifiers.instance_id.clone()),
            false,
        );
        proxy.add_runtime_value(
            RuntimeValue::string(&CREATED_TIMESTAMP, proxy.created_timestamp.to_string()),
            false,
        );
        proxy.add_runtime_value(RuntimeValue::string(&PROXY_ID, id.clone()), false);
        if let Some(realm_id) = &self.identifiers.realm_id {
            proxy.add_runtime_value(RuntimeValue::string(&REALM_ID, realm_id.clone()), false);
        }
        proxy.add_runtime_value(
            RuntimeValue::string(&PROXY_SPEC_ID, self.spec.id.clone()),
            false,
        );
        proxy.add_runtime_value(RuntimeValue::string(&TARGET_ID, id.clone()), false);

        // only `proxy`, `proxySpec` and `containerSpec` are available in the expressions of a shared app
        let resolved = self
            .resolve_spec(&proxy)
            .map_err(|error| format!("problem while resolving SpEL expressions: {error}"))?;

        // the containers of the proxy
        for container_spec in &resolved.container_specs {
            let container = proxy.container_mut(container_spec.index);
            let mut new_container = container.clone();
            self.runtime_values
                .add_container_values(&mut new_container, container_spec);
            *container = new_container;
        }
        self.runtime_values
            .add_after_final_expressions(&mut proxy, &resolved);

        let mut delegate = delegate;
        delegate.proxy = proxy.clone();
        self.delegates.update_delegate_proxy(delegate.clone());

        tracing::info!(
            "Starting DelegateProxy [specId: {}] [delegateProxyId: {id}]",
            self.spec.id
        );
        for container_spec in &resolved.container_specs {
            let container = proxy
                .container(container_spec.index)
                .cloned()
                .unwrap_or_else(|| Container::new(container_spec.index));
            let environment = self.container_environment(&proxy, container_spec);
            let labels = self.container_labels(&proxy, &container, container_spec);

            let started = self
                .backend
                .start_container(StartContext {
                    user: None,
                    proxy: &proxy,
                    spec: &resolved,
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
                    // the container may exist half way, so it is stopped before it is forgotten
                    let _ = self.backend.stop_proxy(&proxy).await;
                    self.delegates.remove_delegate_proxy(&id);
                    return Err(error.to_string());
                }
            }
        }

        delegate.proxy = proxy.clone();
        self.delegates.update_delegate_proxy(delegate.clone());

        // the container has to answer before its seats are handed out
        if !self.wait_until_reachable(&proxy).await {
            tracing::warn!(
                "Failed to start DelegateProxy: Container did not respond in time [specId: {}] \
                 [delegateProxyId: {id}]",
                self.spec.id
            );
            let _ = self.backend.stop_proxy(&proxy).await;
            self.delegates.remove_delegate_proxy(&id);
            return Ok(());
        }

        proxy.status = ProxyStatus::Up;
        proxy.startup_timestamp = now_millis();

        let mut seats = Vec::new();
        delegate.proxy = proxy.clone();
        delegate.status = DelegateProxyStatus::Available;
        for _ in 0..self.extension.seats_per_container() {
            let seat = Seat::new(&id);
            delegate.seat_ids.push(seat.id.clone());
            seats.push(seat);
        }
        self.delegates.update_delegate_proxy(delegate);
        for seat in seats {
            tracing::info!(
                "Created Seat [specId: {}] [delegateProxyId: {id}] [seatId: {}]",
                self.spec.id,
                seat.id
            );
            self.seats.add_seat(seat);
        }

        tracing::info!(
            "Started DelegateProxy [specId: {}] [delegateProxyId: {id}]",
            self.spec.id
        );
        Ok(())
    }

    /// Removes `count` delegate proxies whose seats are all free.
    async fn scale_down(&self, count: i64) {
        tracing::info!(
            "Scale down required, trying to remove {count} DelegateProxies [specId: {}]",
            self.spec.id
        );
        let mut to_remove = Vec::new();
        for delegate in self.delegates.all_delegate_proxies() {
            if delegate.status != DelegateProxyStatus::Available {
                continue;
            }
            match self.seats.remove_seats_if_unclaimed(&delegate.seat_ids) {
                Ok(true) => {
                    to_remove.push(delegate);
                    if to_remove.len() as i64 == count {
                        break;
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    tracing::info!(
                        "Stopping scale down because a seat was claimed [specId: {}]",
                        self.spec.id
                    );
                    break;
                }
            }
        }

        if to_remove.is_empty() {
            tracing::info!(
                "No proxy found to remove during scale-down. [specId: {}]",
                self.spec.id
            );
            return;
        }
        self.remove_delegate_proxies(to_remove).await;
    }

    /// Removes the containers that are marked for removal and whose seats are free.
    pub async fn cleanup(&self) {
        if !self.leader.is_leader() {
            return;
        }
        let status = self.status();
        if status != ReconcileStatus::Stable && status != ReconcileStatus::ScaleDown {
            return;
        }

        let mut to_remove = Vec::new();
        for delegate in self.delegates.all_delegate_proxies() {
            if delegate.status != DelegateProxyStatus::ToRemove {
                continue;
            }
            if delegate.seat_ids.is_empty() {
                to_remove.push(delegate);
                continue;
            }
            match self.seats.remove_seats_if_unclaimed(&delegate.seat_ids) {
                Ok(true) => to_remove.push(delegate),
                Ok(false) => tracing::debug!(
                    "DelegateProxy marked for removal but still has claimed seats [specId: {}] \
                     [delegateProxyId: {}]",
                    self.spec.id,
                    delegate.proxy.id
                ),
                Err(_) => {
                    tracing::debug!(
                        "Stopping cleanup because a seat was claimed [specId: {}]",
                        self.spec.id
                    );
                    break;
                }
            }
        }
        self.remove_delegate_proxies(to_remove).await;
    }

    /// Stops the containers of the given delegate proxies.
    async fn remove_delegate_proxies(&self, delegates: Vec<DelegateProxy>) {
        for delegate in delegates {
            tracing::info!(
                "Stopping DelegateProxy [specId: {}] [delegateProxyId: {}]",
                self.spec.id,
                delegate.proxy.id
            );
            if let Err(error) = self.backend.stop_proxy(&delegate.proxy).await {
                tracing::error!(
                    "Failed to stop delegateProxy: {error} [specId: {}] [delegateProxyId: {}]",
                    self.spec.id,
                    delegate.proxy.id
                );
            }
            self.delegates.remove_delegate_proxy(&delegate.proxy.id);
        }
    }

    /// Marks a container for removal; it goes away as soon as its seats are free.
    pub fn mark_for_removal(&self, delegate_proxy_id: &str) {
        let Some(delegate) = self.delegates.delegate_proxy(delegate_proxy_id) else {
            return;
        };
        let mut updated = delegate.clone();
        updated.status = DelegateProxyStatus::ToRemove;
        updated.seat_ids.clear();
        for seat_id in &delegate.seat_ids {
            let claimed = self
                .seats
                .seat(seat_id)
                .map(|seat| seat.is_claimed())
                .unwrap_or(false);
            if claimed {
                tracing::info!(
                    "Cannot yet remove seat, it is still claimed [specId: {}] \
                     [delegateProxyId: {delegate_proxy_id}] [seatId: {seat_id}]",
                    self.spec.id
                );
                updated.seat_ids.push(seat_id.clone());
            } else {
                self.seats.remove_seat_info(seat_id);
                tracing::info!(
                    "Removed seat [specId: {}] [delegateProxyId: {delegate_proxy_id}] \
                     [seatId: {seat_id}]",
                    self.spec.id
                );
            }
        }
        self.delegates.update_delegate_proxy(updated);
    }

    /// Marks every container of this app definition for removal (`/admin/delegate-proxy` without an id).
    pub fn mark_all_for_removal(&self) {
        tracing::info!(
            "Received external request to remove all DelegateProxies [specId: {}]",
            self.spec.id
        );
        for delegate in self.delegates.all_delegate_proxies() {
            self.mark_for_removal(&delegate.proxy.id);
        }
    }

    /// Handles the seat a user released (`processReleasedSeat`).
    ///
    /// A seat of a container that may be re-used is offered to the next user; otherwise the container is
    /// marked for removal, which is also what happens when the app of the user crashed.
    ///
    /// Unlike the Java implementation this runs on the server that released the seat instead of on the
    /// leader (Java bridges the event through Redis to the leader). Every step is a single operation on the
    /// seat store, which is shared and idempotent, so the outcome is the same; the scaling that follows is
    /// still leader-only, because `reconcile` checks that itself.
    pub async fn seat_released(&self, seat_id: &str, crashed: bool) {
        let Some(seat) = self.seats.seat(seat_id) else {
            tracing::warn!(
                "ProxySharing: Seat {seat_id} not found during processing of SeatReleasedEvent"
            );
            return;
        };
        let Some(delegate) = self.delegates.delegate_proxy(&seat.delegate_proxy_id) else {
            tracing::warn!(
                "ProxySharing: DelegateProxy {} not found during processing of SeatReleasedEvent \
                 with seatId: {seat_id}",
                seat.delegate_proxy_id
            );
            return;
        };

        if crashed {
            tracing::info!(
                "DelegateProxy crashed, marking for removal [specId: {}] [delegateProxyId: {}]",
                self.spec.id,
                delegate.proxy.id
            );
            self.remove_seat(&delegate, seat_id);
            self.mark_for_removal(&delegate.proxy.id);
            self.reconcile().await;
        } else if !self.extension.allow_container_re_use() {
            tracing::info!(
                "DelegateProxy cannot be re-used, marking for removal [specId: {}] \
                 [delegateProxyId: {}]",
                self.spec.id,
                delegate.proxy.id
            );
            self.remove_seat(&delegate, seat_id);
            self.mark_for_removal(&delegate.proxy.id);
            self.reconcile().await;
        } else if delegate.status == DelegateProxyStatus::Available {
            self.seats.add_to_unclaimed_seats(seat_id);
        } else if delegate.status == DelegateProxyStatus::ToRemove {
            self.remove_seat(&delegate, seat_id);
        }
    }

    /// Forgets a seat of a container.
    fn remove_seat(&self, delegate: &DelegateProxy, seat_id: &str) {
        self.seats.remove_seat_info(seat_id);
        let mut updated = delegate.clone();
        updated.seat_ids.retain(|id| id != seat_id);
        self.delegates.update_delegate_proxy(updated);
    }

    /// Takes over the containers of the realm when this server becomes the leader
    /// (`processOnLeaderGranted`).
    pub async fn on_leader_granted(&self) {
        for delegate in self.delegates.all_delegate_proxies() {
            if delegate.status == DelegateProxyStatus::Pending {
                tracing::info!(
                    "Pending DelegateProxy not created by this instance, marking for removal \
                     [specId: {}] [delegateProxyId: {}]",
                    self.spec.id,
                    delegate.proxy.id
                );
                self.mark_for_removal(&delegate.proxy.id);
            } else if delegate.proxy_spec_hash != self.spec_hash {
                tracing::info!(
                    "DelegateProxy not created by this config instance, marking for removal \
                     [specId: {}] [delegateProxyId: {}]",
                    self.spec.id,
                    delegate.proxy.id
                );
                self.mark_for_removal(&delegate.proxy.id);
            }
        }
        self.reconcile().await;
    }

    /// The public path of a delegate proxy (`/api/route/{id}/`).
    fn public_path(&self, delegate_proxy_id: &str) -> String {
        format!("{}{delegate_proxy_id}/", self.public_path_prefix)
    }

    /// Resolves the app definition for a delegate proxy.
    fn resolve_spec(&self, proxy: &Proxy) -> Result<ProxySpec, String> {
        let resolver = |proxy: &Proxy, spec: &ProxySpec| {
            SpelResolver::new(
                ExpressionContextBuilder::new()
                    .process_environment()
                    .proxy(proxy.clone())
                    .spec(spec.clone())
                    .build(),
            )
        };
        let first = self
            .spec
            .first_resolve(&resolver(proxy, &self.spec))
            .map_err(|error| error.to_string())?;
        first
            .final_resolve(&resolver(proxy, &first))
            .map_err(|error| error.to_string())
    }

    /// The environment of a container of a delegate proxy.
    fn container_environment(
        &self,
        proxy: &Proxy,
        container_spec: &crate::model::spec::ContainerSpec,
    ) -> std::collections::BTreeMap<String, String> {
        crate::service::proxy_service::container_environment(proxy, container_spec, None)
    }

    /// The labels of a container of a delegate proxy.
    fn container_labels(
        &self,
        proxy: &Proxy,
        container: &Container,
        container_spec: &crate::model::spec::ContainerSpec,
    ) -> std::collections::BTreeMap<String, String> {
        crate::service::proxy_service::container_labels(proxy, container, container_spec)
    }

    /// Waits until the container of a delegate proxy answers.
    async fn wait_until_reachable(&self, proxy: &Proxy) -> bool {
        let timeout =
            Duration::from_millis(self.settings.proxy.container_wait_timeout_ms().max(1) as u64);
        crate::service::proxy_service::wait_until_reachable(proxy, timeout).await
    }
}

/// The hash of an app definition, used to recognise containers of another configuration.
///
/// The Java implementation hashes the JSON of the app definition; this implementation hashes its debug
/// representation, which is just as stable for the only thing the hash is used for: telling whether a
/// container was created by a server with a different configuration.
pub fn spec_hash(spec: &ProxySpec) -> String {
    use sha1::Digest;
    // the HTTP headers are left out, exactly as the Java implementation does
    let mut copy = spec.clone();
    copy.http_headers = Default::default();
    hex::encode(sha1::Sha1::digest(format!("{copy:?}").as_bytes()))
}
