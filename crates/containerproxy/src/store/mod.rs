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
    RedisHeartbeatStore, RedisLock, RedisPortRegistry, RedisProxyStore, RedisSessionStore,
    RedisStores,
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
