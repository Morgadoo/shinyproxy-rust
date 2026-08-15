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

//! Redis backed stores (`RedisProxyStore`, `RedisHeartbeatStore`, `RedisPortAllocator`).
//!
//! With `proxy.store-mode: Redis` several ShinyProxy servers share their state, so that a user reaches
//! their app through any of them. The keys are the ones the Java implementation uses:
//!
//! | key | contents |
//! | --- | --- |
//! | `shinyproxy_{realmId}__active_proxies` | hash of proxy id to the proxy |
//! | `shinyproxy_{realmId}_user_proxies_{userId}` | set of the proxy ids of a user |
//! | `shinyproxy_{realmId}__heartbeats` | hash of proxy id to the last heartbeat |
//! | `shinyproxy_{realmId}__ports` | hash of the published host port to its owner |
//!
//! The proxies are stored as the JSON of [`Proxy::internal_json`], which is the document the Java
//! implementation writes as well (`Views.Internal`), so both implementations can read each other's state.

use std::collections::BTreeMap;

use redis::Commands;

use super::{HeartbeatStore, ProxyStore};
use crate::model::proxy::Proxy;
use crate::model::runtime_value::RuntimeValueRegistry;

/// The stores of one realm, sharing one connection pool.
#[derive(Clone)]
pub struct RedisStores {
    client: redis::Client,
    /// `shinyproxy_{realmId}` (without a realm: `shinyproxy_`, as in Java, where the realm is null).
    prefix: String,
    /// The runtime value keys, needed to read a proxy back.
    registry: std::sync::Arc<RuntimeValueRegistry>,
}

impl std::fmt::Debug for RedisStores {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisStores")
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl RedisStores {
    /// Connects to Redis.
    pub fn connect(
        url: &str,
        realm_id: Option<&str>,
        registry: std::sync::Arc<RuntimeValueRegistry>,
    ) -> Result<Self, String> {
        let client = redis::Client::open(url)
            .map_err(|error| format!("cannot use the Redis URL '{url}': {error}"))?;
        // fail at startup when Redis is not reachable, as the Java implementation does
        let mut connection = client
            .get_connection()
            .map_err(|error| format!("cannot connect to Redis ({url}): {error}"))?;
        redis::cmd("PING")
            .query::<String>(&mut connection)
            .map_err(|error| format!("cannot talk to Redis ({url}): {error}"))?;

        Ok(RedisStores {
            client,
            prefix: format!("shinyproxy_{}", realm_id.unwrap_or_default()),
            registry,
        })
    }

    /// The URL of a `spring.data.redis.*` configuration.
    pub fn url_of(settings: &crate::config::Settings) -> String {
        let redis = &settings.spring.data.redis;
        let host = redis
            .host
            .clone()
            .unwrap_or_else(|| "localhost".to_string());
        let port = redis.port.map(|value| value.0).unwrap_or(6379);
        let database = redis.database.map(|value| value.0).unwrap_or(0);
        let password = redis.password.clone().unwrap_or_default();
        let username = redis.username.clone().unwrap_or_default();

        let credentials = match (username.is_empty(), password.is_empty()) {
            (true, true) => String::new(),
            (true, false) => format!(":{password}@"),
            (false, true) => format!("{username}@"),
            (false, false) => format!("{username}:{password}@"),
        };
        format!("redis://{credentials}{host}:{port}/{database}")
    }

    /// The key with all proxies.
    fn proxies_key(&self) -> String {
        format!("{}__active_proxies", self.prefix)
    }

    /// The key with the proxy ids of a user.
    fn user_key(&self, user_id: &str) -> String {
        format!("{}_user_proxies_{user_id}", self.prefix)
    }

    /// The key with the heartbeats.
    fn heartbeats_key(&self) -> String {
        format!("{}__heartbeats", self.prefix)
    }

    /// The key with the published ports.
    pub fn ports_key(&self) -> String {
        format!("{}__ports", self.prefix)
    }

    /// A connection, or `None` when Redis is unreachable (which is logged).
    fn connection(&self) -> Option<redis::Connection> {
        match self.client.get_connection() {
            Ok(connection) => Some(connection),
            Err(error) => {
                tracing::error!("cannot connect to Redis: {error}");
                None
            }
        }
    }

    /// The proxy store of these stores.
    pub fn proxy_store(&self) -> RedisProxyStore {
        RedisProxyStore {
            stores: self.clone(),
        }
    }

    /// The heartbeat store of these stores.
    pub fn heartbeat_store(&self) -> RedisHeartbeatStore {
        RedisHeartbeatStore {
            stores: self.clone(),
        }
    }
}

/// Keeps the proxies in Redis.
#[derive(Debug, Clone)]
pub struct RedisProxyStore {
    stores: RedisStores,
}

impl RedisProxyStore {
    /// Reads a proxy from its JSON document.
    fn parse(&self, document: &str) -> Option<Proxy> {
        match serde_json::from_str::<serde_json::Value>(document) {
            Ok(value) => match Proxy::from_internal_json(&self.stores.registry, &value) {
                Ok(proxy) => Some(proxy),
                Err(error) => {
                    tracing::warn!("ignoring an unreadable proxy in Redis ({error}): {document}");
                    None
                }
            },
            Err(error) => {
                tracing::warn!("ignoring an invalid proxy in Redis ({error}): {document}");
                None
            }
        }
    }
}

impl ProxyStore for RedisProxyStore {
    fn all_proxies(&self) -> Vec<Proxy> {
        let Some(mut connection) = self.stores.connection() else {
            return Vec::new();
        };
        let documents: Vec<String> = connection
            .hvals(self.stores.proxies_key())
            .unwrap_or_default();
        documents
            .iter()
            .filter_map(|document| self.parse(document))
            .collect()
    }

    fn add_proxy(&self, proxy: &Proxy) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let document = serde_json::to_string(&proxy.internal_json()).unwrap_or_default();
        let _: Result<(), _> = connection.hset(self.stores.proxies_key(), &proxy.id, document);
        if let Some(user_id) = &proxy.user_id {
            let _: Result<(), _> = connection.sadd(self.stores.user_key(user_id), &proxy.id);
        }
    }

    fn update_proxy(&self, proxy: &Proxy) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let document = serde_json::to_string(&proxy.internal_json()).unwrap_or_default();
        let _: Result<(), _> = connection.hset(self.stores.proxies_key(), &proxy.id, document);
    }

    fn remove_proxy(&self, proxy: &Proxy) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = connection.hdel(self.stores.proxies_key(), &proxy.id);
        if let Some(user_id) = &proxy.user_id {
            let _: Result<(), _> = connection.srem(self.stores.user_key(user_id), &proxy.id);
        }
    }

    fn proxy(&self, proxy_id: &str) -> Option<Proxy> {
        let mut connection = self.stores.connection()?;
        let document: Option<String> = connection
            .hget(self.stores.proxies_key(), proxy_id)
            .unwrap_or_default();
        document
            .as_deref()
            .and_then(|document| self.parse(document))
    }

    fn user_proxies(&self, user_id: &str) -> Vec<Proxy> {
        let Some(mut connection) = self.stores.connection() else {
            return Vec::new();
        };
        let ids: Vec<String> = connection
            .smembers(self.stores.user_key(user_id))
            .unwrap_or_default();
        if ids.is_empty() {
            return Vec::new();
        }
        // the ids of a user are a set, so the proxies are read in one call
        let documents: Vec<Option<String>> = connection
            .hget(self.stores.proxies_key(), ids)
            .unwrap_or_default();
        documents
            .into_iter()
            .flatten()
            .filter_map(|document| self.parse(&document))
            .collect()
    }
}

/// Keeps the heartbeats in Redis.
#[derive(Debug, Clone)]
pub struct RedisHeartbeatStore {
    stores: RedisStores,
}

impl HeartbeatStore for RedisHeartbeatStore {
    fn update(&self, proxy_id: &str, timestamp: i64) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = connection.hset(self.stores.heartbeats_key(), proxy_id, timestamp);
    }

    fn get(&self, proxy_id: &str) -> Option<i64> {
        let mut connection = self.stores.connection()?;
        connection
            .hget(self.stores.heartbeats_key(), proxy_id)
            .unwrap_or_default()
    }

    fn remove(&self, proxy_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = connection.hdel(self.stores.heartbeats_key(), proxy_id);
    }
}

/// Keeps the allocated host ports in Redis (`RedisPortAllocator`).
///
/// The hash `shinyproxy_{realmId}__ports` maps an owner (a proxy id) to the JSON array of its ports, which
/// is what the Java implementation stores (`PortList`). Claiming a port uses `WATCH`/`MULTI`/`EXEC`, so two
/// servers never hand out the same port.
#[derive(Debug, Clone)]
pub struct RedisPortRegistry {
    stores: RedisStores,
}

impl RedisPortRegistry {
    /// Creates the registry.
    pub fn new(stores: RedisStores) -> Self {
        RedisPortRegistry { stores }
    }
}

impl crate::backend::ports::PortRegistry for RedisPortRegistry {
    fn allocated(&self) -> BTreeMap<String, std::collections::BTreeSet<u16>> {
        let Some(mut connection) = self.stores.connection() else {
            return BTreeMap::new();
        };
        let entries: BTreeMap<String, String> = connection
            .hgetall(self.stores.ports_key())
            .unwrap_or_default();
        entries
            .into_iter()
            .map(|(owner, ports)| {
                let ports: std::collections::BTreeSet<u16> =
                    serde_json::from_str::<Vec<u16>>(&ports)
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                (owner, ports)
            })
            .collect()
    }

    fn add(&self, owner_id: &str, port: u16) -> bool {
        let Some(mut connection) = self.stores.connection() else {
            return false;
        };
        let key = self.stores.ports_key();
        let owner = owner_id.to_string();

        // the transaction re-reads the hash after WATCH, so a port another server claimed in the
        // meantime makes this attempt fail (the caller then retries with a fresh view)
        let result: redis::RedisResult<bool> =
            redis::transaction(&mut connection, &[key.as_str()], |connection, pipeline| {
                let entries: BTreeMap<String, String> = connection.hgetall(&key)?;
                let taken = entries.values().any(|ports| {
                    serde_json::from_str::<Vec<u16>>(ports)
                        .unwrap_or_default()
                        .contains(&port)
                });
                if taken {
                    // nothing to write: an empty transaction commits and reports the conflict
                    let _: Option<()> = pipeline.query(connection)?;
                    return Ok(Some(false));
                }
                let mut ports: Vec<u16> = entries
                    .get(&owner)
                    .and_then(|ports| serde_json::from_str(ports).ok())
                    .unwrap_or_default();
                if !ports.contains(&port) {
                    ports.push(port);
                }
                ports.sort_unstable();
                let document = serde_json::to_string(&ports).unwrap_or_default();
                let _: Option<()> = pipeline.hset(&key, &owner, document).query(connection)?;
                Ok(Some(true))
            });

        match result {
            Ok(claimed) => claimed,
            Err(error) => {
                tracing::warn!("cannot claim port {port} in Redis: {error}");
                false
            }
        }
    }

    fn release(&self, owner_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = connection.hdel(self.stores.ports_key(), owner_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    #[test]
    fn builds_the_url_of_a_spring_configuration() {
        let settings: Settings = serde_yaml_ng::from_str(
            "spring:\n  data:\n    redis:\n      host: redis\n      port: 6380\n      \
             database: 3\n      password: secret\n",
        )
        .expect("settings");
        assert_eq!(
            RedisStores::url_of(&settings),
            "redis://:secret@redis:6380/3"
        );

        assert_eq!(
            RedisStores::url_of(&Settings::default()),
            "redis://localhost:6379/0"
        );

        let settings: Settings = serde_yaml_ng::from_str(
            "spring:\n  data:\n    redis:\n      host: redis\n      username: sp\n      \
             password: secret\n",
        )
        .expect("settings");
        assert_eq!(
            RedisStores::url_of(&settings),
            "redis://sp:secret@redis:6379/0"
        );
    }

    #[test]
    fn uses_the_java_keys() {
        let stores = RedisStores {
            client: redis::Client::open("redis://127.0.0.1/").expect("client"),
            prefix: "shinyproxy_my-realm".to_string(),
            registry: std::sync::Arc::new(RuntimeValueRegistry::engine()),
        };
        assert_eq!(stores.proxies_key(), "shinyproxy_my-realm__active_proxies");
        assert_eq!(
            stores.user_key("jack"),
            "shinyproxy_my-realm_user_proxies_jack"
        );
        assert_eq!(stores.heartbeats_key(), "shinyproxy_my-realm__heartbeats");
        assert_eq!(stores.ports_key(), "shinyproxy_my-realm__ports");
    }
}
