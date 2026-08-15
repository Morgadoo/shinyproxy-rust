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

//! In-memory stores (`proxy.store-mode: None`).

use dashmap::DashMap;

use super::{HeartbeatStore, ProxyStore};
use crate::model::proxy::Proxy;

/// Keeps the running proxies in memory.
#[derive(Debug, Default)]
pub struct MemoryProxyStore {
    /// The proxies are shared, so the per-request lookups of the data plane never copy them.
    proxies: DashMap<String, std::sync::Arc<Proxy>>,
    /// Whether user names are compared case sensitively (`proxy.username-case-sensitive`).
    case_sensitive_users: bool,
}

impl MemoryProxyStore {
    /// Creates a store.
    pub fn new(case_sensitive_users: bool) -> Self {
        MemoryProxyStore {
            proxies: DashMap::new(),
            case_sensitive_users,
        }
    }

    fn owns(&self, proxy: &Proxy, user_id: &str) -> bool {
        match &proxy.user_id {
            Some(owner) if self.case_sensitive_users => owner == user_id,
            Some(owner) => owner.eq_ignore_ascii_case(user_id),
            None => false,
        }
    }
}

impl ProxyStore for MemoryProxyStore {
    fn all_proxies(&self) -> Vec<Proxy> {
        self.proxies
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect()
    }

    fn add_proxy(&self, proxy: &Proxy) {
        self.proxies
            .insert(proxy.id.clone(), std::sync::Arc::new(proxy.clone()));
    }

    fn update_proxy(&self, proxy: &Proxy) {
        self.proxies
            .insert(proxy.id.clone(), std::sync::Arc::new(proxy.clone()));
    }

    fn remove_proxy(&self, proxy: &Proxy) {
        self.proxies.remove(&proxy.id);
    }

    fn proxy(&self, proxy_id: &str) -> Option<Proxy> {
        self.proxies
            .get(proxy_id)
            .map(|entry| (**entry.value()).clone())
    }

    fn proxy_ref(&self, proxy_id: &str) -> Option<std::sync::Arc<Proxy>> {
        self.proxies
            .get(proxy_id)
            .map(|entry| entry.value().clone())
    }

    fn find_user_proxy_by_target(
        &self,
        user_id: &str,
        target_id: &str,
    ) -> Option<std::sync::Arc<Proxy>> {
        self.proxies
            .iter()
            .find(|entry| {
                entry.value().target_id() == target_id && self.owns(entry.value(), user_id)
            })
            .map(|entry| entry.value().clone())
    }

    fn user_proxies(&self, user_id: &str) -> Vec<Proxy> {
        self.proxies
            .iter()
            .filter(|entry| self.owns(entry.value(), user_id))
            .map(|entry| (**entry.value()).clone())
            .collect()
    }
}

/// Keeps the last heartbeat of every proxy in memory.
#[derive(Debug, Default)]
pub struct MemoryHeartbeatStore {
    heartbeats: DashMap<String, i64>,
}

impl MemoryHeartbeatStore {
    /// Creates a store.
    pub fn new() -> Self {
        MemoryHeartbeatStore::default()
    }
}

impl HeartbeatStore for MemoryHeartbeatStore {
    fn update(&self, proxy_id: &str, timestamp: i64) {
        self.heartbeats.insert(proxy_id.to_string(), timestamp);
    }

    fn get(&self, proxy_id: &str) -> Option<i64> {
        self.heartbeats.get(proxy_id).map(|entry| *entry.value())
    }

    fn remove(&self, proxy_id: &str) {
        self.heartbeats.remove(proxy_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::ProxyStatus;

    fn proxy(id: &str, user: &str, spec: &str) -> Proxy {
        let mut proxy = Proxy::new(id, ProxyStatus::Up);
        proxy.user_id = Some(user.to_string());
        proxy.spec_id = Some(spec.to_string());
        proxy
    }

    #[test]
    fn stores_and_finds_proxies() {
        let store = MemoryProxyStore::new(true);
        store.add_proxy(&proxy("a", "jack", "01_hello"));
        store.add_proxy(&proxy("b", "jack", "02_other"));
        store.add_proxy(&proxy("c", "jeff", "01_hello"));

        assert_eq!(store.count(), 3);
        assert_eq!(store.count_by_spec("01_hello"), 2);
        assert_eq!(
            store.proxy("a").map(|proxy| proxy.id),
            Some("a".to_string())
        );
        assert!(store.proxy("nope").is_none());

        let mut jack: Vec<String> = store
            .user_proxies("jack")
            .into_iter()
            .map(|proxy| proxy.id)
            .collect();
        jack.sort();
        assert_eq!(jack, ["a", "b"]);
        assert_eq!(store.user_proxies("jeff").len(), 1);
        assert!(store.user_proxies("unknown").is_empty());
    }

    #[test]
    fn updates_and_removes_proxies() {
        let store = MemoryProxyStore::new(true);
        let proxy = proxy("a", "jack", "01_hello");
        store.add_proxy(&proxy);

        let stopping = proxy.with_status(ProxyStatus::Stopping);
        store.update_proxy(&stopping);
        assert_eq!(
            store.proxy("a").map(|proxy| proxy.status),
            Some(ProxyStatus::Stopping)
        );

        store.remove_proxy(&stopping);
        assert!(store.proxy("a").is_none());
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn honours_case_insensitive_user_names() {
        let store = MemoryProxyStore::new(false);
        store.add_proxy(&proxy("a", "Jack", "01_hello"));
        assert_eq!(store.user_proxies("jack").len(), 1);

        let store = MemoryProxyStore::new(true);
        store.add_proxy(&proxy("a", "Jack", "01_hello"));
        assert!(store.user_proxies("jack").is_empty());
    }

    #[test]
    fn stores_heartbeats() {
        let store = MemoryHeartbeatStore::new();
        assert!(store.get("a").is_none());
        store.update("a", 1000);
        assert_eq!(store.get("a"), Some(1000));
        store.update("a", 2000);
        assert_eq!(store.get("a"), Some(2000));
        store.remove("a");
        assert!(store.get("a").is_none());
    }
}
