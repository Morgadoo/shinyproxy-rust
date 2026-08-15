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

//! Container sharing and pre-initialization (`backend/dispatcher/proxysharing`).
//!
//! An app definition with `minimum-seats-available` does not start a container per user. Instead the server
//! keeps *delegate proxies* running, each of which offers `seats-per-container` seats; a user that opens the
//! app claims a free seat and is proxied to the delegate proxy, which makes the app open instantly.
//!
//! * [`Seat`] and [`DelegateProxy`] are the model, kept in a [`store::SeatStore`] and a
//!   [`store::DelegateProxyStore`] (in memory, or in Redis so the servers of a realm share the seats).
//! * [`scaler::ProxySharingScaler`] keeps enough seats available: it creates delegate proxies when seats
//!   run low and removes them again after `scale-down-delay` minutes. Only the leader of the realm scales.
//! * [`ProxySharingDispatcher`] claims and releases the seats when a user starts or stops the app.

pub mod scaler;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::proxy::Proxy;
use crate::model::spec::ProxySpec;

pub use scaler::ProxySharingScaler;
pub use store::{
    DelegateProxyStore, MemoryDelegateProxyStore, MemorySeatStore, SeatClaimedDuringRemoval,
    SeatStore,
};

/// A seat of a delegate proxy: one user of a shared container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    /// Identifier of the seat.
    pub id: String,
    /// The delegate proxy that offers this seat.
    #[serde(rename = "delegateProxyId")]
    pub delegate_proxy_id: String,
    /// The proxy of the user that claimed this seat, when it is claimed.
    #[serde(rename = "delegatingProxyId", default)]
    pub delegating_proxy_id: Option<String>,
}

impl Seat {
    /// A new, unclaimed seat of a delegate proxy.
    pub fn new(delegate_proxy_id: impl Into<String>) -> Self {
        Seat {
            id: uuid::Uuid::new_v4().to_string(),
            delegate_proxy_id: delegate_proxy_id.into(),
            delegating_proxy_id: None,
        }
    }

    /// Whether a user holds this seat.
    pub fn is_claimed(&self) -> bool {
        self.delegating_proxy_id.is_some()
    }
}

/// The state of a delegate proxy (`DelegateProxyStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelegateProxyStatus {
    /// The container is being created.
    Pending,
    /// The container runs and its seats can be claimed.
    Available,
    /// The container has to be removed as soon as its seats are free.
    ToRemove,
}

/// A pre-started container that serves the seats of an app definition.
///
/// The `Proxy` inside is serialised with its internal JSON view when the delegate proxies are shared
/// through Redis (see `store::redis`), which is why this type has no derived `Serialize`.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegateProxy {
    /// The proxy of the container itself.
    pub proxy: Proxy,
    /// The seats this container offers.
    pub seat_ids: Vec<String>,
    /// What has to happen to this container.
    pub status: DelegateProxyStatus,
    /// Hash of the app definition it was created for; a server with another configuration removes it.
    pub proxy_spec_hash: String,
}

impl DelegateProxy {
    /// A delegate proxy that is being created.
    pub fn pending(proxy: Proxy, proxy_spec_hash: impl Into<String>) -> Self {
        DelegateProxy {
            proxy,
            seat_ids: Vec::new(),
            status: DelegateProxyStatus::Pending,
            proxy_spec_hash: proxy_spec_hash.into(),
        }
    }
}

/// The sharing fields of an app definition (`ProxySharingSpecExtension`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ProxySharingSpecExtension {
    /// How many seats are kept available; sharing is off when this is absent.
    pub minimum_seats_available: Option<crate::config::FlexI64>,
    /// Whether a container may serve another user after the first one left.
    pub allow_container_re_use: crate::config::FlexBool,
    /// How many minutes after the last scale up a container may be removed.
    pub scale_down_delay: crate::config::FlexI64,
    /// How many users share one container.
    pub seats_per_container: crate::config::FlexI64,
}

impl Default for ProxySharingSpecExtension {
    fn default() -> Self {
        ProxySharingSpecExtension {
            minimum_seats_available: None,
            allow_container_re_use: crate::config::FlexBool(true),
            scale_down_delay: crate::config::FlexI64(2),
            seats_per_container: crate::config::FlexI64(1),
        }
    }
}

impl ProxySharingSpecExtension {
    /// The sharing fields of an app definition.
    pub fn of(spec: &ProxySpec) -> Self {
        spec.spec_extensions.get("proxy-sharing")
    }

    /// Whether this app definition uses shared containers (`ProxySharingDispatcher.supportSpec`).
    pub fn enabled(&self) -> bool {
        self.minimum_seats_available.is_some()
    }

    /// How many seats are kept available.
    pub fn minimum_seats_available(&self) -> i64 {
        self.minimum_seats_available
            .map(|value| value.0)
            .unwrap_or(0)
    }

    /// How many users share one container.
    pub fn seats_per_container(&self) -> i64 {
        self.seats_per_container.0.max(1)
    }

    /// Whether a container may serve another user.
    pub fn allow_container_re_use(&self) -> bool {
        self.allow_container_re_use.0
    }

    /// The delay before a container may be removed.
    pub fn scale_down_delay(&self) -> Duration {
        Duration::from_secs((self.scale_down_delay.0.max(0) * 60) as u64)
    }

    /// Refuses configurations the Java implementation refuses as well.
    pub fn validate(&self, spec_id: &str) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        if !self.allow_container_re_use() && self.seats_per_container() != 1 {
            return Err(format!(
                "Spec {spec_id} is invalid: when allow-container-re-use is disabled, \
                 seatsPerContainer must be exactly 1"
            ));
        }
        Ok(())
    }
}

/// How long a user waits for a seat by default (`proxy.seat-wait-time`).
pub const DEFAULT_SEAT_WAIT_TIME: Duration = Duration::from_millis(300_000);

/// How often a waiting user checks for a free seat (the Java implementation waits in steps of 3 seconds).
pub const SEAT_WAIT_STEP: Duration = Duration::from_millis(3_000);

/// Hands out the seats of an app definition (`ProxySharingDispatcher`).
#[derive(Debug)]
pub struct ProxySharingDispatcher {
    /// The app definition this dispatcher serves.
    spec_id: String,
    seats: Arc<dyn SeatStore>,
    delegates: Arc<dyn DelegateProxyStore>,
    /// How long a user waits for a seat.
    seat_wait_time: Duration,
    /// The users that are waiting for a seat, in the order they arrived.
    pending: std::sync::Mutex<Vec<String>>,
}

impl ProxySharingDispatcher {
    /// Creates the dispatcher of an app definition.
    pub fn new(
        spec_id: impl Into<String>,
        seats: Arc<dyn SeatStore>,
        delegates: Arc<dyn DelegateProxyStore>,
        seat_wait_time: Duration,
    ) -> Self {
        ProxySharingDispatcher {
            spec_id: spec_id.into(),
            seats,
            delegates,
            seat_wait_time,
            pending: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The app definition of this dispatcher.
    pub fn spec_id(&self) -> &str {
        &self.spec_id
    }

    /// The seats of this app definition.
    pub fn seats(&self) -> &Arc<dyn SeatStore> {
        &self.seats
    }

    /// The delegate proxies of this app definition.
    pub fn delegates(&self) -> &Arc<dyn DelegateProxyStore> {
        &self.delegates
    }

    /// The users that are waiting for a seat.
    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .expect("the pending list is not poisoned")
            .len()
    }

    /// Remembers that a user is waiting for a seat.
    fn add_pending(&self, proxy_id: &str) {
        let mut pending = self
            .pending
            .lock()
            .expect("the pending list is not poisoned");
        if !pending.iter().any(|id| id == proxy_id) {
            pending.push(proxy_id.to_string());
        }
    }

    /// Forgets a user that is no longer waiting.
    pub fn remove_pending(&self, proxy_id: &str) {
        self.pending
            .lock()
            .expect("the pending list is not poisoned")
            .retain(|id| id != proxy_id);
    }

    /// Gives a proxy a seat, waiting for one when they are all taken.
    ///
    /// The returned proxy points at the delegate proxy of the seat: same targets, same public path, and the
    /// seat id as a runtime value so that stopping the app releases the seat again.
    pub async fn start_proxy(&self, mut proxy: Proxy) -> Result<Proxy, String> {
        let started_at = std::time::Instant::now();
        let mut seat = self.seats.claim_seat(&proxy.id);

        if seat.is_none() {
            tracing::info!("Seat not immediately available [proxyId: {}]", proxy.id);
            self.add_pending(&proxy.id);
            let iterations = self
                .seat_wait_time
                .as_millis()
                .div_ceil(SEAT_WAIT_STEP.as_millis())
                .max(1);
            for attempt in 0..iterations {
                tokio::time::sleep(SEAT_WAIT_STEP).await;
                seat = self.seats.claim_seat(&proxy.id);
                if seat.is_some() {
                    tracing::info!("Seat available attempt: {attempt} [proxyId: {}]", proxy.id);
                    break;
                }
            }
        }

        let Some(seat) = seat else {
            self.remove_pending(&proxy.id);
            return Err("Could not claim a seat within the configured wait-time".to_string());
        };
        self.remove_pending(&proxy.id);
        tracing::info!(
            "Seat claimed [proxyId: {}] [seatId: {}] [delegateProxyId: {}]",
            proxy.id,
            seat.id,
            seat.delegate_proxy_id
        );

        let Some(delegate) = self.delegates.delegate_proxy(&seat.delegate_proxy_id) else {
            self.seats.release_seat(&seat.id);
            return Err(format!(
                "The delegate proxy {} of seat {} disappeared",
                seat.delegate_proxy_id, seat.id
            ));
        };

        // the app of the user is the delegate proxy: same targets, same public path
        proxy.target_id = Some(delegate.proxy.id.clone());
        for (mapping, target) in &delegate.proxy.targets {
            proxy.targets.insert(mapping.clone(), target.clone());
        }
        if let Some(public_path) = delegate
            .proxy
            .runtime_values
            .get(&crate::model::runtime_value::PUBLIC_PATH)
        {
            proxy.add_runtime_value(public_path.clone(), true);
        }
        proxy.add_runtime_value(
            crate::model::runtime_value::RuntimeValue::string(
                &crate::model::runtime_value::TARGET_ID,
                delegate.proxy.id.clone(),
            ),
            true,
        );
        proxy.add_runtime_value(
            crate::model::runtime_value::RuntimeValue::string(
                &crate::model::runtime_value::SEAT_ID,
                seat.id.clone(),
            ),
            true,
        );

        // the container of the user points at the container of the delegate proxy
        if let Some(delegate_container) = delegate.proxy.containers.first() {
            let container = proxy.container_mut(delegate_container.index);
            container.id = Some(uuid::Uuid::new_v4().to_string());
            if let Some(name) = delegate_container
                .runtime_values
                .get(&crate::model::runtime_value::BACKEND_CONTAINER_NAME)
            {
                container.add_runtime_value(name.clone(), true);
            }
        }

        tracing::debug!(
            "Seat claimed in {} ms [proxyId: {}]",
            started_at.elapsed().as_millis(),
            proxy.id
        );
        Ok(proxy)
    }

    /// Releases the seat of a proxy that stops.
    ///
    /// Returns the seat that was released, so the caller can tell the scaler what happened to it.
    pub fn stop_proxy(&self, proxy: &Proxy) -> Option<Seat> {
        self.remove_pending(&proxy.id);
        let seat_id = proxy
            .runtime_values
            .value_string(&crate::model::runtime_value::SEAT_ID)?;
        self.seats.release_seat(&seat_id);
        tracing::info!("Seat released [proxyId: {}] [seatId: {seat_id}]", proxy.id);
        self.seats.seat(&seat_id)
    }

    /// How long a user waits for a seat (`proxy.seat-wait-time`, at least 3 seconds).
    pub fn seat_wait_time(settings: &crate::config::Settings) -> Result<Duration, String> {
        let Some(value) = settings.proxy.seat_wait_time.map(|value| value.0) else {
            return Ok(DEFAULT_SEAT_WAIT_TIME);
        };
        if value < 3000 {
            return Err(
                "Invalid configuration: proxy.seat-wait-time must be larger than 3000 (3 seconds)."
                    .to_string(),
            );
        }
        Ok(Duration::from_millis(value as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::ProxyStatus;

    fn spec_with(fields: serde_json::Value) -> ProxySpec {
        let mut spec = ProxySpec::new("01_hello");
        spec.spec_extensions.insert("proxy-sharing", fields);
        spec
    }

    #[test]
    fn reads_the_sharing_fields_of_an_app() {
        // sharing is off unless minimum-seats-available is set
        let extension = ProxySharingSpecExtension::of(&ProxySpec::new("01_hello"));
        assert!(!extension.enabled());
        assert!(extension.allow_container_re_use());
        assert_eq!(extension.seats_per_container(), 1);
        assert_eq!(extension.scale_down_delay(), Duration::from_secs(120));

        let extension = ProxySharingSpecExtension::of(&spec_with(serde_json::json!({
            "minimum-seats-available": 3,
            "seats-per-container": 2,
            "allow-container-re-use": false,
            "scale-down-delay": 5,
        })));
        assert!(extension.enabled());
        assert_eq!(extension.minimum_seats_available(), 3);
        assert_eq!(extension.seats_per_container(), 2);
        assert!(!extension.allow_container_re_use());
        assert_eq!(extension.scale_down_delay(), Duration::from_secs(300));

        // strings are accepted as well, like everywhere else in the configuration
        let extension = ProxySharingSpecExtension::of(&spec_with(serde_json::json!({
            "minimum-seats-available": "2",
            "allow-container-re-use": "false",
        })));
        assert_eq!(extension.minimum_seats_available(), 2);
        assert!(!extension.allow_container_re_use());
    }

    #[test]
    fn refuses_invalid_sharing_configurations() {
        let extension = ProxySharingSpecExtension::of(&spec_with(serde_json::json!({
            "minimum-seats-available": 1,
            "allow-container-re-use": false,
            "seats-per-container": 2,
        })));
        let error = extension.validate("01_hello").unwrap_err();
        assert!(
            error.contains("allow-container-re-use is disabled"),
            "{error}"
        );
        assert!(
            error.contains("seatsPerContainer must be exactly 1"),
            "{error}"
        );

        // valid combinations pass
        assert!(ProxySharingSpecExtension::of(&spec_with(serde_json::json!({
            "minimum-seats-available": 1,
            "allow-container-re-use": false,
            "seats-per-container": 1,
        })))
        .validate("01_hello")
        .is_ok());
        assert!(ProxySharingSpecExtension::default()
            .validate("01_hello")
            .is_ok());
    }

    #[test]
    fn reads_the_seat_wait_time() {
        let settings = crate::config::Settings::default();
        assert_eq!(
            ProxySharingDispatcher::seat_wait_time(&settings).unwrap(),
            DEFAULT_SEAT_WAIT_TIME
        );

        let settings: crate::config::Settings =
            serde_yaml_ng::from_str("proxy:\n  seat-wait-time: 10000\n").expect("settings");
        assert_eq!(
            ProxySharingDispatcher::seat_wait_time(&settings).unwrap(),
            Duration::from_secs(10)
        );

        let settings: crate::config::Settings =
            serde_yaml_ng::from_str("proxy:\n  seat-wait-time: 2000\n").expect("settings");
        let error = ProxySharingDispatcher::seat_wait_time(&settings).unwrap_err();
        assert!(error.contains("must be larger than 3000"), "{error}");
    }

    /// A dispatcher with one delegate proxy that offers `seats` seats.
    fn dispatcher(seats: usize) -> (Arc<ProxySharingDispatcher>, Proxy) {
        let seat_store: Arc<dyn SeatStore> = Arc::new(MemorySeatStore::new());
        let delegate_store: Arc<dyn DelegateProxyStore> = Arc::new(MemoryDelegateProxyStore::new());

        let mut delegate_proxy = Proxy::new("delegate-1", ProxyStatus::Up);
        delegate_proxy.spec_id = Some("01_hello".to_string());
        delegate_proxy
            .targets
            .insert(String::new(), "http://127.0.0.1:20000".to_string());
        delegate_proxy.add_runtime_value(
            crate::model::runtime_value::RuntimeValue::string(
                &crate::model::runtime_value::PUBLIC_PATH,
                "/api/route/delegate-1/",
            ),
            true,
        );
        let mut container = crate::model::proxy::Container::new(0);
        container.id = Some("container-1".to_string());
        delegate_proxy.containers.push(container);

        let mut delegate = DelegateProxy::pending(delegate_proxy.clone(), "hash");
        delegate.status = DelegateProxyStatus::Available;
        for _ in 0..seats {
            let seat = Seat::new("delegate-1");
            delegate.seat_ids.push(seat.id.clone());
            seat_store.add_seat(seat);
        }
        delegate_store.add_delegate_proxy(delegate);

        (
            Arc::new(ProxySharingDispatcher::new(
                "01_hello",
                seat_store,
                delegate_store,
                Duration::from_millis(3000),
            )),
            delegate_proxy,
        )
    }

    #[tokio::test]
    async fn claims_a_seat_and_points_the_proxy_at_the_delegate() {
        let (dispatcher, delegate) = dispatcher(1);

        let proxy = Proxy::new("user-proxy", ProxyStatus::New);
        let started = dispatcher.start_proxy(proxy).await.expect("a seat");

        assert_eq!(started.target_id.as_deref(), Some("delegate-1"));
        assert_eq!(started.targets, delegate.targets);
        assert_eq!(
            started
                .runtime_values
                .value_string(&crate::model::runtime_value::PUBLIC_PATH)
                .as_deref(),
            Some("/api/route/delegate-1/")
        );
        let seat_id = started
            .runtime_values
            .value_string(&crate::model::runtime_value::SEAT_ID)
            .expect("the seat id is remembered");
        assert_eq!(dispatcher.seats().unclaimed_count(), 0);
        assert_eq!(
            dispatcher
                .seats()
                .seat(&seat_id)
                .and_then(|seat| seat.delegating_proxy_id),
            Some("user-proxy".to_string())
        );

        // stopping the app releases the seat; the scaler decides whether it is offered again, which is
        // what `add_to_unclaimed_seats` does here
        let released = dispatcher.stop_proxy(&started).expect("the seat");
        assert_eq!(released.id, seat_id);
        assert!(!released.is_claimed());
        assert_eq!(dispatcher.seats().unclaimed_count(), 0);
        dispatcher.seats().add_to_unclaimed_seats(&seat_id);
        assert_eq!(dispatcher.seats().unclaimed_count(), 1);
    }

    #[tokio::test]
    async fn waits_for_a_seat_and_gives_up_after_the_wait_time() {
        let (dispatcher, _) = dispatcher(1);

        // the only seat is taken
        let first = dispatcher
            .start_proxy(Proxy::new("first", ProxyStatus::New))
            .await
            .expect("a seat");

        // the second user waits (the wait time is 3 seconds in this test) and fails
        let start = std::time::Instant::now();
        let error = dispatcher
            .start_proxy(Proxy::new("second", ProxyStatus::New))
            .await
            .expect_err("no seat available");
        assert!(error.contains("Could not claim a seat"), "{error}");
        assert!(start.elapsed() >= Duration::from_millis(3000));
        assert_eq!(dispatcher.pending_count(), 0, "the user is not pending");

        // when the first user leaves, the seat is offered again
        let seat = dispatcher.stop_proxy(&first).expect("the seat");
        dispatcher.seats().add_to_unclaimed_seats(&seat.id);
        let second = dispatcher
            .start_proxy(Proxy::new("second", ProxyStatus::New))
            .await
            .expect("a seat");
        assert_eq!(second.target_id.as_deref(), Some("delegate-1"));
    }

    #[tokio::test]
    async fn a_seat_that_becomes_free_while_waiting_is_claimed() {
        let (dispatcher, _) = dispatcher(1);

        let first = dispatcher
            .start_proxy(Proxy::new("first", ProxyStatus::New))
            .await
            .expect("a seat");

        // the seat becomes free while the second user is waiting
        let releaser = dispatcher.clone();
        let released = first.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(seat) = releaser.stop_proxy(&released) {
                releaser.seats().add_to_unclaimed_seats(&seat.id);
            }
        });

        let second = dispatcher
            .start_proxy(Proxy::new("second", ProxyStatus::New))
            .await
            .expect("the seat that became free");
        assert_eq!(second.target_id.as_deref(), Some("delegate-1"));
    }
}
