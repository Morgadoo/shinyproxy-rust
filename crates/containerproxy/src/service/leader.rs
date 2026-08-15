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

//! Leader election (`ILeaderService`, `RedisLeaderService`, `MemoryLeaderService`).
//!
//! Work that must happen exactly once in a realm — releasing inactive and expired apps, collecting the
//! logs of the containers — only runs on the leader. A single server is always the leader
//! (`MemoryLeaderService`); with `proxy.store-mode: Redis` the servers of the realm take a lock in Redis and
//! whoever holds it is the leader, exactly as the Java implementation does with its `RedisLockRegistry`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Which server does the work of the realm.
pub trait LeaderService: Send + Sync + std::fmt::Debug {
    /// Whether this server is the leader right now.
    fn is_leader(&self) -> bool;
}

/// A single server, which is always the leader.
#[derive(Debug, Default)]
pub struct MemoryLeaderService {
    leader: AtomicBool,
}

impl MemoryLeaderService {
    /// Creates the service (leader from the start, as in Java).
    pub fn new() -> Self {
        MemoryLeaderService {
            leader: AtomicBool::new(true),
        }
    }

    /// Gives up leadership (when the server shuts down).
    pub fn resign(&self) {
        self.leader.store(false, Ordering::SeqCst);
    }
}

impl LeaderService for MemoryLeaderService {
    fn is_leader(&self) -> bool {
        self.leader.load(Ordering::SeqCst)
    }
}

/// How long the lock is held before it has to be renewed.
pub const LOCK_TTL: Duration = Duration::from_secs(30);

/// How often the leader renews the lock (and a follower tries to take it).
pub const LOCK_RENEWAL: Duration = Duration::from_secs(10);

/// Elects a leader among the servers of a realm with a lock in Redis.
#[derive(Debug)]
pub struct RedisLeaderService {
    lock: crate::store::redis::RedisLock,
    /// Identifies this server (its runtime id).
    runtime_id: String,
    leader: AtomicBool,
    /// Set while this server may take part in the election; a server that runs an older configuration
    /// steps out (`RedisCheckLatestConfigService`).
    participating: AtomicBool,
}

impl RedisLeaderService {
    /// Creates the service; the lock is taken by [`RedisLeaderService::spawn`].
    pub fn new(lock: crate::store::redis::RedisLock, runtime_id: impl Into<String>) -> Self {
        RedisLeaderService {
            lock,
            runtime_id: runtime_id.into(),
            leader: AtomicBool::new(false),
            participating: AtomicBool::new(true),
        }
    }

    /// Takes part in the election until the process ends.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(LOCK_RENEWAL);
            loop {
                timer.tick().await;
                self.elect();
            }
        })
    }

    /// Stops taking part in the election, giving up leadership.
    ///
    /// Used by [`LatestConfigService`] when a newer server appeared: the old server keeps serving its apps
    /// but no longer does the work of the realm.
    pub fn withdraw(&self) {
        if self.participating.swap(false, Ordering::SeqCst) {
            self.resign();
        }
    }

    /// Whether this server takes part in the election.
    pub fn participating(&self) -> bool {
        self.participating.load(Ordering::SeqCst)
    }

    /// One round of the election: renew the lock, or try to take it.
    pub fn elect(&self) -> bool {
        if !self.participating.load(Ordering::SeqCst) {
            return false;
        }
        let was_leader = self.leader.load(Ordering::SeqCst);
        let is_leader = self.lock.acquire(&self.runtime_id, LOCK_TTL);
        self.leader.store(is_leader, Ordering::SeqCst);

        if is_leader && !was_leader {
            tracing::info!(
                "This server (runtimeId: {}) is now the leader.",
                self.runtime_id
            );
        } else if !is_leader && was_leader {
            tracing::info!(
                "This server (runtimeId: {}) is no longer the leader.",
                self.runtime_id
            );
        }
        is_leader
    }

    /// Gives up the lock (when the server shuts down), so that another server takes over immediately.
    pub fn resign(&self) {
        if self.leader.swap(false, Ordering::SeqCst) {
            self.lock.release(&self.runtime_id);
            tracing::info!(
                "This server (runtimeId: {}) is no longer the leader.",
                self.runtime_id
            );
        }
    }
}

impl LeaderService for RedisLeaderService {
    fn is_leader(&self) -> bool {
        self.leader.load(Ordering::SeqCst)
    }
}

/// How long the leader waits before giving up leadership after a newer server appeared.
///
/// The Java implementation waits 25 seconds, so that every other server had a chance to notice that it is
/// not running the latest configuration either (they check every 20 seconds).
pub const RESIGN_DELAY: Duration = Duration::from_secs(25);

/// How often a server checks whether it still runs the latest configuration.
pub const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(20);

/// Keeps a server out of the leader election when it runs an older configuration.
///
/// Port of `RedisCheckLatestConfigService`: the newest `proxy.version` of the realm is kept in Redis. A
/// server without `proxy.version` always takes part (there is nothing to compare), and a server whose
/// version is older stops taking part, which is how a rolling update moves the work of the realm to the new
/// servers.
#[derive(Debug)]
pub struct LatestConfigService {
    store: crate::store::RedisVersionStore,
    election: Arc<RedisLeaderService>,
    /// The version of this server (`proxy.version`), when it has one.
    version: Option<i64>,
    instance_id: String,
    latest: AtomicBool,
}

impl LatestConfigService {
    /// Creates the service.
    pub fn new(
        store: crate::store::RedisVersionStore,
        election: Arc<RedisLeaderService>,
        version: Option<i64>,
        instance_id: impl Into<String>,
    ) -> Self {
        LatestConfigService {
            store,
            election,
            version,
            instance_id: instance_id.into(),
            latest: AtomicBool::new(false),
        }
    }

    /// Whether this server runs the latest configuration of the realm.
    pub fn is_latest(&self) -> bool {
        self.version.is_none() || self.latest.load(Ordering::SeqCst)
    }

    /// The first check, at startup: it decides whether this server takes part in the election at all.
    pub fn initialize(&self) {
        let Some(version) = self.version else {
            tracing::info!(
                "No proxy.version property found, assuming this server is running the latest \
                 configuration, taking part in leader election."
            );
            self.latest.store(true, Ordering::SeqCst);
            return;
        };

        match self.store.check_latest(version) {
            Some(true) => {
                self.latest.store(true, Ordering::SeqCst);
                tracing::info!(
                    "This server is running the latest configuration (instanceId: {}, version: \
                     {version}), taking part in leader election.",
                    self.instance_id
                );
            }
            Some(false) => {
                self.latest.store(false, Ordering::SeqCst);
                self.election.withdraw();
                tracing::info!(
                    "This server is not running the latest configuration (instanceId: {}, version: \
                     {version}), not taking part in leader election.",
                    self.instance_id
                );
            }
            None => {
                tracing::warn!(
                    "Failed to check whether this server is running the latest configuration"
                );
            }
        }
    }

    /// One check: `false` when this server no longer runs the latest configuration.
    pub fn check(&self) -> bool {
        let Some(version) = self.version else {
            return true;
        };
        if !self.latest.load(Ordering::SeqCst) {
            // a server that already lost never comes back, as in Java
            return false;
        }
        if let Some(false) = self.store.check_latest(version) {
            self.latest.store(false, Ordering::SeqCst);
            tracing::info!(
                "This server is no longer running the latest configuration (instanceId: {}, version: \
                 {version}), no longer taking part in leader election.",
                self.instance_id
            );
            self.stop_election();
            return false;
        }
        true
    }

    /// Leaves the election, after the delay that lets the other servers catch up.
    fn stop_election(&self) {
        if self.election.is_leader() {
            let election = self.election.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RESIGN_DELAY).await;
                election.withdraw();
            });
        } else {
            self.election.withdraw();
        }
    }

    /// Checks every 20 seconds, for the lifetime of the process.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(VERSION_CHECK_INTERVAL);
            loop {
                timer.tick().await;
                self.check();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_server_is_the_leader() {
        let service = MemoryLeaderService::new();
        assert!(service.is_leader());
        service.resign();
        assert!(!service.is_leader());
    }
}

#[cfg(test)]
mod latest_config_tests {
    use super::*;

    /// A service without a version always takes part in the election.
    #[tokio::test]
    async fn a_server_without_a_version_is_always_the_latest() {
        let Some(stores) = crate::store::redis::test_stores("latest-none") else {
            eprintln!("skipping: no Redis");
            return;
        };
        let election = Arc::new(RedisLeaderService::new(stores.leader_lock(), "runtime-1"));
        let service =
            LatestConfigService::new(stores.version_store(), election.clone(), None, "instance-1");

        service.initialize();
        assert!(service.is_latest());
        assert!(service.check());
        assert!(election.participating());
    }

    /// The newest version of the realm wins, older servers step out of the election.
    #[tokio::test]
    async fn only_the_newest_version_takes_part_in_the_election() {
        let Some(stores) = crate::store::redis::test_stores("latest-version") else {
            eprintln!("skipping: no Redis");
            return;
        };

        // the first server publishes version 1 and is the latest
        let first_election = Arc::new(RedisLeaderService::new(stores.leader_lock(), "runtime-1"));
        let first = LatestConfigService::new(
            stores.version_store(),
            first_election.clone(),
            Some(1),
            "instance-1",
        );
        first.initialize();
        assert!(first.is_latest());
        assert!(first_election.participating());

        // a second server with the same version is the latest as well (a restart of the same version)
        let same_election = Arc::new(RedisLeaderService::new(stores.leader_lock(), "runtime-2"));
        let same = LatestConfigService::new(
            stores.version_store(),
            same_election.clone(),
            Some(1),
            "instance-2",
        );
        same.initialize();
        assert!(same.is_latest());

        // a newer server takes over
        let new_election = Arc::new(RedisLeaderService::new(stores.leader_lock(), "runtime-3"));
        let new = LatestConfigService::new(
            stores.version_store(),
            new_election.clone(),
            Some(2),
            "instance-3",
        );
        new.initialize();
        assert!(new.is_latest());

        // the older server notices on its next check and leaves the election (it is not the leader here,
        // so it leaves immediately)
        assert!(!first.check());
        assert!(!first.is_latest());
        assert!(!first_election.participating());
        assert!(!first_election.elect(), "it must not take the lock anymore");

        // and an older server that starts later never takes part
        let old_election = Arc::new(RedisLeaderService::new(stores.leader_lock(), "runtime-4"));
        let old = LatestConfigService::new(
            stores.version_store(),
            old_election.clone(),
            Some(1),
            "instance-4",
        );
        old.initialize();
        assert!(!old.is_latest());
        assert!(!old_election.participating());

        stores.clear_for_tests();
    }
}
