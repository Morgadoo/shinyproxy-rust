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

//! Where running proxies are kept.
//!
//! With `proxy.store-mode: None` (the default) proxies live in memory; with `Redis` they are shared
//! between ShinyProxy servers (P12). The traits mirror `IProxyStore` and `IHeartbeatStore`.

pub mod memory;

use crate::model::proxy::Proxy;

pub mod redis;

pub use memory::{MemoryHeartbeatStore, MemoryProxyStore};
pub use redis::{
    RedisDelegateProxyStore, RedisHeartbeatStore, RedisLock, RedisPortRegistry, RedisProxyStore,
    RedisSeatStore, RedisSessionStore, RedisStores, RedisVersionStore,
};

/// Keeps track of the running proxies.
pub trait ProxyStore: Send + Sync + std::fmt::Debug {
    /// All proxies, in no particular order.
    fn all_proxies(&self) -> Vec<Proxy>;

    /// Adds a proxy.
    fn add_proxy(&self, proxy: &Proxy);

    /// Replaces an existing proxy (same id).
    fn update_proxy(&self, proxy: &Proxy);

    /// Removes a proxy.
    fn remove_proxy(&self, proxy: &Proxy);

    /// The proxy with the given id.
    fn proxy(&self, proxy_id: &str) -> Option<Proxy>;

    /// The proxy with the given id, shared instead of copied.
    ///
    /// The data plane looks a proxy up on *every* request; copying one means cloning its runtime values
    /// (two maps full of JSON documents), which showed up as roughly a tenth of the CPU of the proxy path.
    /// The in-memory store hands out its `Arc` directly; other stores fall back to a copy.
    fn proxy_ref(&self, proxy_id: &str) -> Option<std::sync::Arc<Proxy>> {
        self.proxy(proxy_id).map(std::sync::Arc::new)
    }

    /// The proxy of a user whose target id matches, shared instead of copied (the data plane's lookup).
    fn find_user_proxy_by_target(
        &self,
        user_id: &str,
        target_id: &str,
    ) -> Option<std::sync::Arc<Proxy>> {
        self.user_proxies(user_id)
            .into_iter()
            .find(|proxy| proxy.target_id() == target_id)
            .map(std::sync::Arc::new)
    }

    /// The proxies owned by the given user.
    fn user_proxies(&self, user_id: &str) -> Vec<Proxy>;

    /// Number of proxies.
    fn count(&self) -> usize {
        self.all_proxies().len()
    }

    /// Number of proxies of a spec.
    fn count_by_spec(&self, spec_id: &str) -> usize {
        self.all_proxies()
            .iter()
            .filter(|proxy| proxy.spec_id.as_deref() == Some(spec_id))
            .count()
    }
}

/// Keeps track of the last heartbeat of every proxy.
pub trait HeartbeatStore: Send + Sync + std::fmt::Debug {
    /// Records a heartbeat (epoch millis).
    fn update(&self, proxy_id: &str, timestamp: i64);

    /// The last heartbeat of a proxy.
    fn get(&self, proxy_id: &str) -> Option<i64>;

    /// Forgets a proxy.
    fn remove(&self, proxy_id: &str);
}
