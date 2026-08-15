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
}

impl RedisLeaderService {
    /// Creates the service; the lock is taken by [`RedisLeaderService::spawn`].
    pub fn new(lock: crate::store::redis::RedisLock, runtime_id: impl Into<String>) -> Self {
        RedisLeaderService {
            lock,
            runtime_id: runtime_id.into(),
            leader: AtomicBool::new(false),
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

    /// One round of the election: renew the lock, or try to take it.
    pub fn elect(&self) -> bool {
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
