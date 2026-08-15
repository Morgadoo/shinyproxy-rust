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

//! Where the seats and the delegate proxies of an app definition live.
//!
//! The in-memory stores serve a single server; the Redis stores (`store/redis.rs`) let the servers of a
//! realm share the pre-started containers, as `RedisSeatStore` and `RedisDelegateProxyStore` do in Java.

use std::collections::VecDeque;

use dashmap::DashMap;
use std::sync::Mutex;

use super::{DelegateProxy, Seat};

/// A seat was claimed while the store was removing it.
///
/// The scaler stops what it was doing when this happens, so that a user that just got a seat keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatClaimedDuringRemoval;

/// Keeps the seats of one app definition (`ISeatStore`).
pub trait SeatStore: Send + Sync + std::fmt::Debug {
    /// Adds a new, unclaimed seat.
    fn add_seat(&self, seat: Seat);

    /// The seat with the given id.
    fn seat(&self, seat_id: &str) -> Option<Seat>;

    /// Claims a free seat for a proxy, when there is one.
    fn claim_seat(&self, claiming_proxy_id: &str) -> Option<Seat>;

    /// Releases a seat; it is *not* offered again until [`SeatStore::add_to_unclaimed_seats`] is called,
    /// because the scaler first decides what happens to the container.
    fn release_seat(&self, seat_id: &str);

    /// Offers a released seat again.
    fn add_to_unclaimed_seats(&self, seat_id: &str);

    /// Removes the given seats, but only when none of them is claimed.
    fn remove_seats_if_unclaimed(
        &self,
        seat_ids: &[String],
    ) -> Result<bool, SeatClaimedDuringRemoval>;

    /// Forgets a seat completely.
    fn remove_seat_info(&self, seat_id: &str);

    /// How many seats can be claimed right now.
    fn unclaimed_count(&self) -> i64;

    /// How many seats exist (claimed and unclaimed).
    fn count(&self) -> i64;

    /// How many seats are claimed.
    fn claimed_count(&self) -> i64 {
        self.count() - self.unclaimed_count()
    }
}

/// Keeps the delegate proxies of one app definition (`IDelegateProxyStore`).
pub trait DelegateProxyStore: Send + Sync + std::fmt::Debug {
    /// Adds a delegate proxy.
    fn add_delegate_proxy(&self, delegate_proxy: DelegateProxy);

    /// Replaces a delegate proxy.
    fn update_delegate_proxy(&self, delegate_proxy: DelegateProxy);

    /// Forgets a delegate proxy.
    fn remove_delegate_proxy(&self, delegate_proxy_id: &str);

    /// The delegate proxy with the given id.
    fn delegate_proxy(&self, delegate_proxy_id: &str) -> Option<DelegateProxy>;

    /// Every delegate proxy of this app definition.
    fn all_delegate_proxies(&self) -> Vec<DelegateProxy>;
}

/// The seats of one server.
#[derive(Debug, Default)]
pub struct MemorySeatStore {
    seats: DashMap<String, Seat>,
    /// The seats that can be claimed, oldest first (as the Java implementation uses a queue).
    unclaimed: Mutex<VecDeque<String>>,
}

impl MemorySeatStore {
    /// An empty store.
    pub fn new() -> Self {
        MemorySeatStore::default()
    }
}

impl SeatStore for MemorySeatStore {
    fn add_seat(&self, seat: Seat) {
        let id = seat.id.clone();
        self.seats.insert(id.clone(), seat);
        self.unclaimed
            .lock()
            .expect("the seat queue is not poisoned")
            .push_back(id);
    }

    fn seat(&self, seat_id: &str) -> Option<Seat> {
        self.seats.get(seat_id).map(|entry| entry.value().clone())
    }

    fn claim_seat(&self, claiming_proxy_id: &str) -> Option<Seat> {
        let mut unclaimed = self
            .unclaimed
            .lock()
            .expect("the seat queue is not poisoned");
        while let Some(seat_id) = unclaimed.pop_front() {
            if let Some(mut entry) = self.seats.get_mut(&seat_id) {
                if entry.value().is_claimed() {
                    // it was claimed in the meantime, try the next one
                    continue;
                }
                entry.value_mut().delegating_proxy_id = Some(claiming_proxy_id.to_string());
                return Some(entry.value().clone());
            }
        }
        None
    }

    fn release_seat(&self, seat_id: &str) {
        if let Some(mut entry) = self.seats.get_mut(seat_id) {
            entry.value_mut().delegating_proxy_id = None;
        }
    }

    fn add_to_unclaimed_seats(&self, seat_id: &str) {
        if self.seats.contains_key(seat_id) {
            let mut unclaimed = self
                .unclaimed
                .lock()
                .expect("the seat queue is not poisoned");
            if !unclaimed.iter().any(|id| id == seat_id) {
                unclaimed.push_back(seat_id.to_string());
            }
        }
    }

    fn remove_seats_if_unclaimed(
        &self,
        seat_ids: &[String],
    ) -> Result<bool, SeatClaimedDuringRemoval> {
        let mut unclaimed = self
            .unclaimed
            .lock()
            .expect("the seat queue is not poisoned");
        // every seat has to be offered right now, otherwise nothing is removed: a seat that was released
        // but not offered again is still being processed by the scaler (`unClaimSeatIds.containsAll`)
        for seat_id in seat_ids {
            if !unclaimed.iter().any(|id| id == seat_id) {
                return Ok(false);
            }
        }
        for seat_id in seat_ids {
            self.seats.remove(seat_id);
            unclaimed.retain(|id| id != seat_id);
        }
        Ok(true)
    }

    fn remove_seat_info(&self, seat_id: &str) {
        self.seats.remove(seat_id);
        self.unclaimed
            .lock()
            .expect("the seat queue is not poisoned")
            .retain(|id| id != seat_id);
    }

    fn unclaimed_count(&self) -> i64 {
        self.unclaimed
            .lock()
            .expect("the seat queue is not poisoned")
            .len() as i64
    }

    fn count(&self) -> i64 {
        self.seats.len() as i64
    }
}

/// The delegate proxies of one server.
#[derive(Debug, Default)]
pub struct MemoryDelegateProxyStore {
    delegate_proxies: DashMap<String, DelegateProxy>,
}

impl MemoryDelegateProxyStore {
    /// An empty store.
    pub fn new() -> Self {
        MemoryDelegateProxyStore::default()
    }
}

impl DelegateProxyStore for MemoryDelegateProxyStore {
    fn add_delegate_proxy(&self, delegate_proxy: DelegateProxy) {
        self.delegate_proxies
            .insert(delegate_proxy.proxy.id.clone(), delegate_proxy);
    }

    fn update_delegate_proxy(&self, delegate_proxy: DelegateProxy) {
        self.delegate_proxies
            .insert(delegate_proxy.proxy.id.clone(), delegate_proxy);
    }

    fn remove_delegate_proxy(&self, delegate_proxy_id: &str) {
        self.delegate_proxies.remove(delegate_proxy_id);
    }

    fn delegate_proxy(&self, delegate_proxy_id: &str) -> Option<DelegateProxy> {
        self.delegate_proxies
            .get(delegate_proxy_id)
            .map(|entry| entry.value().clone())
    }

    fn all_delegate_proxies(&self) -> Vec<DelegateProxy> {
        let mut all: Vec<DelegateProxy> = self
            .delegate_proxies
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        // a stable order keeps the scaler predictable (and the tests deterministic)
        all.sort_by(|left, right| left.proxy.id.cmp(&right.proxy.id));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Proxy, ProxyStatus};

    #[test]
    fn hands_out_seats_in_order() {
        let store = MemorySeatStore::new();
        let first = Seat::new("delegate-1");
        let second = Seat::new("delegate-1");
        store.add_seat(first.clone());
        store.add_seat(second.clone());

        assert_eq!(store.count(), 2);
        assert_eq!(store.unclaimed_count(), 2);
        assert_eq!(store.claimed_count(), 0);

        // the oldest seat is handed out first
        let claimed = store.claim_seat("proxy-1").expect("a seat");
        assert_eq!(claimed.id, first.id);
        assert_eq!(claimed.delegating_proxy_id.as_deref(), Some("proxy-1"));
        assert_eq!(store.unclaimed_count(), 1);
        assert_eq!(store.claimed_count(), 1);

        let claimed = store.claim_seat("proxy-2").expect("a seat");
        assert_eq!(claimed.id, second.id);
        assert!(store.claim_seat("proxy-3").is_none(), "no seats left");

        // a released seat is only offered again when the scaler decides so
        store.release_seat(&first.id);
        assert_eq!(store.unclaimed_count(), 0);
        assert!(store.claim_seat("proxy-3").is_none());
        store.add_to_unclaimed_seats(&first.id);
        assert_eq!(store.unclaimed_count(), 1);
        assert_eq!(
            store.claim_seat("proxy-3").map(|seat| seat.id),
            Some(first.id.clone())
        );
    }

    #[test]
    fn removes_seats_only_when_they_are_free() {
        let store = MemorySeatStore::new();
        let first = Seat::new("delegate-1");
        let second = Seat::new("delegate-1");
        store.add_seat(first.clone());
        store.add_seat(second.clone());

        store.claim_seat("proxy-1");
        let ids = vec![first.id.clone(), second.id.clone()];
        assert_eq!(
            store.remove_seats_if_unclaimed(&ids),
            Ok(false),
            "one of the seats is claimed, so nothing is removed"
        );
        assert_eq!(store.count(), 2);

        // a released seat that was not offered again is not removed either (it is still being processed)
        store.release_seat(&first.id);
        assert_eq!(store.remove_seats_if_unclaimed(&ids), Ok(false));
        store.add_to_unclaimed_seats(&first.id);
        assert_eq!(store.remove_seats_if_unclaimed(&ids), Ok(true));
        assert_eq!(store.count(), 0);
        assert_eq!(store.unclaimed_count(), 0);
    }

    #[test]
    fn keeps_the_delegate_proxies() {
        let store = MemoryDelegateProxyStore::new();
        let delegate = DelegateProxy::pending(Proxy::new("delegate-1", ProxyStatus::New), "hash");
        store.add_delegate_proxy(delegate.clone());

        assert_eq!(store.all_delegate_proxies().len(), 1);
        assert_eq!(
            store.delegate_proxy("delegate-1").map(|proxy| proxy.status),
            Some(super::super::DelegateProxyStatus::Pending)
        );

        let mut updated = delegate.clone();
        updated.status = super::super::DelegateProxyStatus::Available;
        updated.seat_ids.push("seat-1".to_string());
        store.update_delegate_proxy(updated);
        let stored = store.delegate_proxy("delegate-1").expect("the proxy");
        assert_eq!(stored.status, super::super::DelegateProxyStatus::Available);
        assert_eq!(stored.seat_ids, vec!["seat-1"]);

        store.remove_delegate_proxy("delegate-1");
        assert!(store.all_delegate_proxies().is_empty());
    }
}
