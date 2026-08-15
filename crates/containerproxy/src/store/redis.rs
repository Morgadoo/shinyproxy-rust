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

    /// The session store of these stores.
    pub fn session_store(
        &self,
        realm_id: Option<&str>,
        version: &str,
        session_timeout: std::time::Duration,
    ) -> RedisSessionStore {
        RedisSessionStore::new(self.clone(), realm_id, version, session_timeout)
    }

    /// The seat store of an app definition.
    pub fn seat_store(&self, spec_id: &str) -> RedisSeatStore {
        RedisSeatStore::new(self.clone(), spec_id)
    }

    /// The delegate proxy store of an app definition.
    pub fn delegate_proxy_store(&self, spec_id: &str) -> RedisDelegateProxyStore {
        RedisDelegateProxyStore::new(self.clone(), spec_id)
    }

    /// The version store of these stores.
    pub fn version_store(&self) -> RedisVersionStore {
        RedisVersionStore::new(self.clone())
    }

    /// The leader lock of these stores.
    pub fn leader_lock(&self) -> RedisLock {
        RedisLock::leader(self.clone())
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

/// Keeps the HTTP sessions in Redis (`RedisSessionConfig` + Spring Session).
///
/// With `spring.session.store-type: redis` the sessions of the servers of a realm are shared, so a user
/// reaches their app through any of them. The keys live under
/// `shinyproxy__{realmId}__{version}__spring:session:sessions:{id}`, like the namespace the Java
/// implementation configures, but the value is the JSON of this implementation: Spring Session writes a
/// Redis hash with Java serialised attributes, which cannot be read here (documented in
/// `docs/COMPATIBILITY.md` — sessions are lost when migrating from the Java implementation).
#[derive(Debug, Clone)]
pub struct RedisSessionStore {
    stores: RedisStores,
    namespace: String,
    /// How long a session lives without being used; needed to derive the last use from the expiry.
    session_timeout: time::Duration,
}

impl RedisSessionStore {
    /// Creates the store for a realm and a version of ShinyProxy.
    ///
    /// The namespace is the one `RedisSessionConfig` builds: the realm and the version of the server, with
    /// Spring Session's default namespace behind it.
    pub fn new(
        stores: RedisStores,
        realm_id: Option<&str>,
        version: &str,
        session_timeout: std::time::Duration,
    ) -> Self {
        let version = version.replace(['.', '-'], "_");
        let namespace = match realm_id.filter(|realm| !realm.is_empty()) {
            Some(realm) => {
                format!("shinyproxy__{realm}__{version}__spring:session:sessions")
            }
            None => format!("shinyproxy__{version}__spring:session:sessions"),
        };
        RedisSessionStore {
            stores,
            namespace,
            session_timeout: time::Duration::try_from(session_timeout)
                .unwrap_or(time::Duration::hours(1)),
        }
    }

    /// The key of a session.
    fn key(&self, id: &tower_sessions::session::Id) -> String {
        format!("{}:{}", self.namespace, id)
    }

    /// The namespace of the sessions (used by tests).
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

#[async_trait::async_trait]
impl tower_sessions::SessionStore for RedisSessionStore {
    async fn create(
        &self,
        record: &mut tower_sessions::session::Record,
    ) -> tower_sessions::session_store::Result<()> {
        // a new id until one is free, so that two servers never share a session by accident
        loop {
            let key = self.key(&record.id);
            let document = serde_json::to_string(record)
                .map_err(|error| tower_sessions::session_store::Error::Encode(error.to_string()))?;
            let mut connection = self.connection().await?;
            let created: bool = redis::cmd("SET")
                .arg(&key)
                .arg(document)
                .arg("NX")
                .arg("EXAT")
                .arg(record.expiry_date.unix_timestamp())
                .query_async(&mut connection)
                .await
                .map_err(|error| {
                    tower_sessions::session_store::Error::Backend(error.to_string())
                })?;
            if created {
                return Ok(());
            }
            record.id = tower_sessions::session::Id::default();
        }
    }

    async fn save(
        &self,
        record: &tower_sessions::session::Record,
    ) -> tower_sessions::session_store::Result<()> {
        let document = serde_json::to_string(record)
            .map_err(|error| tower_sessions::session_store::Error::Encode(error.to_string()))?;
        let mut connection = self.connection().await?;
        let _: () = redis::cmd("SET")
            .arg(self.key(&record.id))
            .arg(document)
            .arg("EXAT")
            .arg(record.expiry_date.unix_timestamp())
            .query_async(&mut connection)
            .await
            .map_err(|error| tower_sessions::session_store::Error::Backend(error.to_string()))?;
        Ok(())
    }

    async fn load(
        &self,
        id: &tower_sessions::session::Id,
    ) -> tower_sessions::session_store::Result<Option<tower_sessions::session::Record>> {
        let mut connection = self.connection().await?;
        let document: Option<String> = redis::cmd("GET")
            .arg(self.key(id))
            .query_async(&mut connection)
            .await
            .map_err(|error| tower_sessions::session_store::Error::Backend(error.to_string()))?;
        let Some(document) = document else {
            return Ok(None);
        };
        match serde_json::from_str(&document) {
            Ok(record) => Ok(Some(record)),
            Err(error) => {
                // a session this server cannot read (e.g. written by another implementation) is
                // treated as absent, so the user simply logs in again
                tracing::debug!("ignoring an unreadable session in Redis: {error}");
                Ok(None)
            }
        }
    }

    async fn delete(
        &self,
        id: &tower_sessions::session::Id,
    ) -> tower_sessions::session_store::Result<()> {
        let mut connection = self.connection().await?;
        let _: () = redis::cmd("DEL")
            .arg(self.key(id))
            .query_async(&mut connection)
            .await
            .map_err(|error| tower_sessions::session_store::Error::Backend(error.to_string()))?;
        Ok(())
    }
}

impl RedisSessionStore {
    /// Counts the users of the sessions of this realm (`updateCachedUsersLoggedInCount`).
    ///
    /// Returns the number of users that are logged in and the number that used their session inside
    /// `active_window`. The last use of a session is derived from its expiry: `tower-sessions` moves the
    /// expiry forward on every request, so `expiry - timeout` is the time it was last used.
    pub async fn count_users(
        &self,
        active_window: std::time::Duration,
    ) -> Result<(i64, i64), String> {
        let mut connection = self
            .stores
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| error.to_string())?;

        // SCAN instead of KEYS, which the Java implementation warns about
        let pattern = format!("{}:*", self.namespace);
        let mut cursor: u64 = 0;
        let mut logged_in: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
        let now = time::OffsetDateTime::now_utc();

        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut connection)
                .await
                .map_err(|error| error.to_string())?;

            for key in keys {
                let document: Option<String> = redis::cmd("GET")
                    .arg(&key)
                    .query_async(&mut connection)
                    .await
                    .map_err(|error| error.to_string())?;
                let Some(document) = document else { continue };
                let Ok(record) = serde_json::from_str::<tower_sessions::session::Record>(&document)
                else {
                    continue;
                };

                // the user of the session, when it is authenticated
                let data = record
                    .data
                    .get("shinyproxy")
                    .and_then(|value| {
                        serde_json::from_value::<crate::web::session::SessionData>(value.clone())
                            .ok()
                    })
                    .unwrap_or_default();
                let Some(user) = data.user.map(|user| user.id) else {
                    continue;
                };

                logged_in.insert(user.clone());
                // the remaining life of the session tells when it was last used
                let remaining = record.expiry_date - now;
                if remaining > self.session_timeout - active_window {
                    active.insert(user);
                }
            }

            cursor = next;
            if cursor == 0 {
                break;
            }
        }

        Ok((logged_in.len() as i64, active.len() as i64))
    }

    /// Moves the expiry of a session forward (`reActivateSession`).
    pub async fn extend(&self, session_id: &str) -> Result<(), String> {
        let mut connection = self
            .stores
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| error.to_string())?;
        let key = format!("{}:{session_id}", self.namespace);
        // the session document keeps its own expiry date, which the next request refreshes; extending the
        // key is enough to keep the session from disappearing while the app is being used
        let _: () = redis::cmd("EXPIRE")
            .arg(&key)
            .arg(self.session_timeout.whole_seconds().max(1))
            .arg("XX")
            .query_async(&mut connection)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// An asynchronous connection to Redis.
    async fn connection(
        &self,
    ) -> tower_sessions::session_store::Result<redis::aio::MultiplexedConnection> {
        self.stores
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| tower_sessions::session_store::Error::Backend(error.to_string()))
    }
}

/// The seats of one app definition, shared by the servers of a realm (`RedisSeatStore`).
///
/// The seats live in the hash `shinyproxy_{realmId}__seats_{specId}` and the ids of the seats that can be
/// claimed in the set `shinyproxy_{realmId}__unclaimed_seat_ids_{specId}`, exactly as the Java
/// implementation stores them. Claiming a seat pops an id from the set, which is atomic, so two servers can
/// never hand out the same seat.
#[derive(Debug, Clone)]
pub struct RedisSeatStore {
    stores: RedisStores,
    seats_key: String,
    unclaimed_key: String,
}

impl RedisSeatStore {
    /// The seat store of an app definition.
    pub fn new(stores: RedisStores, spec_id: &str) -> Self {
        let seats_key = format!("{}__seats_{spec_id}", stores.prefix);
        let unclaimed_key = format!("{}__unclaimed_seat_ids_{spec_id}", stores.prefix);
        RedisSeatStore {
            stores,
            seats_key,
            unclaimed_key,
        }
    }

    /// The key of the hash with the seats (used by tests).
    pub fn seats_key(&self) -> &str {
        &self.seats_key
    }

    /// The key of the set with the seats that can be claimed (used by tests).
    pub fn unclaimed_key(&self) -> &str {
        &self.unclaimed_key
    }

    /// Writes a seat.
    fn put(&self, connection: &mut redis::Connection, seat: &crate::service::sharing::Seat) {
        let Ok(document) = serde_json::to_string(seat) else {
            return;
        };
        let _: Result<(), _> = redis::cmd("HSET")
            .arg(&self.seats_key)
            .arg(&seat.id)
            .arg(document)
            .query(connection);
    }
}

impl crate::service::sharing::SeatStore for RedisSeatStore {
    fn add_seat(&self, seat: crate::service::sharing::Seat) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        self.put(&mut connection, &seat);
        if seat.delegating_proxy_id.is_none() {
            let _: Result<(), _> = redis::cmd("SADD")
                .arg(&self.unclaimed_key)
                .arg(&seat.id)
                .query(&mut connection);
        }
    }

    fn seat(&self, seat_id: &str) -> Option<crate::service::sharing::Seat> {
        let mut connection = self.stores.connection()?;
        let document: Option<String> = redis::cmd("HGET")
            .arg(&self.seats_key)
            .arg(seat_id)
            .query(&mut connection)
            .unwrap_or_default();
        document.and_then(|document| serde_json::from_str(&document).ok())
    }

    fn claim_seat(&self, claiming_proxy_id: &str) -> Option<crate::service::sharing::Seat> {
        let mut connection = self.stores.connection()?;
        // SPOP is atomic: only one server gets the seat
        let seat_id: Option<String> = redis::cmd("SPOP")
            .arg(&self.unclaimed_key)
            .query(&mut connection)
            .unwrap_or_default();
        let seat_id = seat_id?;

        let document: Option<String> = redis::cmd("HGET")
            .arg(&self.seats_key)
            .arg(&seat_id)
            .query(&mut connection)
            .unwrap_or_default();
        let mut seat: crate::service::sharing::Seat =
            document.and_then(|document| serde_json::from_str(&document).ok())?;
        seat.delegating_proxy_id = Some(claiming_proxy_id.to_string());
        self.put(&mut connection, &seat);
        Some(seat)
    }

    fn release_seat(&self, seat_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let document: Option<String> = redis::cmd("HGET")
            .arg(&self.seats_key)
            .arg(seat_id)
            .query(&mut connection)
            .unwrap_or_default();
        let Some(mut seat) = document.and_then(|document| {
            serde_json::from_str::<crate::service::sharing::Seat>(&document).ok()
        }) else {
            return;
        };
        seat.delegating_proxy_id = None;
        self.put(&mut connection, &seat);
    }

    fn add_to_unclaimed_seats(&self, seat_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = redis::cmd("SADD")
            .arg(&self.unclaimed_key)
            .arg(seat_id)
            .query(&mut connection);
    }

    fn remove_seats_if_unclaimed(
        &self,
        seat_ids: &[String],
    ) -> Result<bool, crate::service::sharing::SeatClaimedDuringRemoval> {
        let Some(mut connection) = self.stores.connection() else {
            return Ok(false);
        };
        if seat_ids.is_empty() {
            return Ok(true);
        }

        // WATCH the set, check that every seat can be claimed, then remove them in one transaction: a seat
        // that is claimed in the meantime aborts the transaction (`UnclaimedSeatRemover`)
        let _: Result<(), _> = redis::cmd("WATCH")
            .arg(&self.unclaimed_key)
            .query(&mut connection);
        for seat_id in seat_ids {
            let member: bool = redis::cmd("SISMEMBER")
                .arg(&self.unclaimed_key)
                .arg(seat_id)
                .query(&mut connection)
                .unwrap_or(false);
            if !member {
                let _: Result<(), _> = redis::cmd("UNWATCH").query(&mut connection);
                return Ok(false);
            }
        }

        let mut pipeline = redis::pipe();
        pipeline.atomic();
        let mut remove = pipeline.cmd("SREM");
        remove = remove.arg(&self.unclaimed_key);
        for seat_id in seat_ids {
            remove = remove.arg(seat_id);
        }
        let answer: Option<Vec<redis::Value>> = remove.query(&mut connection).ok();
        match answer {
            Some(answers) if !answers.is_empty() => {
                let mut delete = redis::cmd("HDEL");
                let mut delete = delete.arg(&self.seats_key);
                for seat_id in seat_ids {
                    delete = delete.arg(seat_id);
                }
                let _: Result<(), _> = delete.query(&mut connection);
                Ok(true)
            }
            // the transaction was aborted: somebody claimed one of the seats
            _ => Err(crate::service::sharing::SeatClaimedDuringRemoval),
        }
    }

    fn remove_seat_info(&self, seat_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = redis::cmd("HDEL")
            .arg(&self.seats_key)
            .arg(seat_id)
            .query(&mut connection);
        let _: Result<(), _> = redis::cmd("SREM")
            .arg(&self.unclaimed_key)
            .arg(seat_id)
            .query(&mut connection);
    }

    fn unclaimed_count(&self) -> i64 {
        let Some(mut connection) = self.stores.connection() else {
            return 0;
        };
        redis::cmd("SCARD")
            .arg(&self.unclaimed_key)
            .query(&mut connection)
            .unwrap_or(0)
    }

    fn count(&self) -> i64 {
        let Some(mut connection) = self.stores.connection() else {
            return 0;
        };
        redis::cmd("HLEN")
            .arg(&self.seats_key)
            .query(&mut connection)
            .unwrap_or(0)
    }
}

/// The pre-started containers of one app definition, shared by the servers of a realm
/// (`RedisDelegateProxyStore`).
///
/// They live in the hash `shinyproxy_{realmId}__delegate_proxies_{specId}`; the proxy inside is stored with
/// the internal JSON view, like the proxies of the realm.
#[derive(Debug, Clone)]
pub struct RedisDelegateProxyStore {
    stores: RedisStores,
    key: String,
}

impl RedisDelegateProxyStore {
    /// The delegate proxy store of an app definition.
    pub fn new(stores: RedisStores, spec_id: &str) -> Self {
        let key = format!("{}__delegate_proxies_{spec_id}", stores.prefix);
        RedisDelegateProxyStore { stores, key }
    }

    /// The key of the hash (used by tests).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The JSON document of a delegate proxy.
    fn document(delegate_proxy: &crate::service::sharing::DelegateProxy) -> serde_json::Value {
        serde_json::json!({
            "proxy": delegate_proxy.proxy.internal_json(),
            "seatIds": delegate_proxy.seat_ids,
            "delegateProxyStatus": match delegate_proxy.status {
                crate::service::sharing::DelegateProxyStatus::Pending => "Pending",
                crate::service::sharing::DelegateProxyStatus::Available => "Available",
                crate::service::sharing::DelegateProxyStatus::ToRemove => "ToRemove",
            },
            "proxySpecHash": delegate_proxy.proxy_spec_hash,
        })
    }

    /// Reads a delegate proxy back.
    fn parse(&self, document: &str) -> Option<crate::service::sharing::DelegateProxy> {
        let value: serde_json::Value = serde_json::from_str(document).ok()?;
        let proxy = crate::model::proxy::Proxy::from_internal_json(
            &self.stores.registry,
            value.get("proxy")?,
        )
        .ok()?;
        let status = match value
            .get("delegateProxyStatus")
            .and_then(|value| value.as_str())
        {
            Some("Available") => crate::service::sharing::DelegateProxyStatus::Available,
            Some("ToRemove") => crate::service::sharing::DelegateProxyStatus::ToRemove,
            _ => crate::service::sharing::DelegateProxyStatus::Pending,
        };
        let seat_ids = value
            .get("seatIds")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Some(crate::service::sharing::DelegateProxy {
            proxy,
            seat_ids,
            status,
            proxy_spec_hash: value
                .get("proxySpecHash")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }
}

impl crate::service::sharing::DelegateProxyStore for RedisDelegateProxyStore {
    fn add_delegate_proxy(&self, delegate_proxy: crate::service::sharing::DelegateProxy) {
        self.update_delegate_proxy(delegate_proxy);
    }

    fn update_delegate_proxy(&self, delegate_proxy: crate::service::sharing::DelegateProxy) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let document = RedisDelegateProxyStore::document(&delegate_proxy).to_string();
        let _: Result<(), _> = redis::cmd("HSET")
            .arg(&self.key)
            .arg(&delegate_proxy.proxy.id)
            .arg(document)
            .query(&mut connection);
    }

    fn remove_delegate_proxy(&self, delegate_proxy_id: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let _: Result<(), _> = redis::cmd("HDEL")
            .arg(&self.key)
            .arg(delegate_proxy_id)
            .query(&mut connection);
    }

    fn delegate_proxy(
        &self,
        delegate_proxy_id: &str,
    ) -> Option<crate::service::sharing::DelegateProxy> {
        let mut connection = self.stores.connection()?;
        let document: Option<String> = redis::cmd("HGET")
            .arg(&self.key)
            .arg(delegate_proxy_id)
            .query(&mut connection)
            .unwrap_or_default();
        document.and_then(|document| self.parse(&document))
    }

    fn all_delegate_proxies(&self) -> Vec<crate::service::sharing::DelegateProxy> {
        let Some(mut connection) = self.stores.connection() else {
            return Vec::new();
        };
        let documents: Vec<String> = redis::cmd("HVALS")
            .arg(&self.key)
            .query(&mut connection)
            .unwrap_or_default();
        let mut all: Vec<crate::service::sharing::DelegateProxy> = documents
            .iter()
            .filter_map(|document| self.parse(document))
            .collect();
        all.sort_by(|left, right| left.proxy.id.cmp(&right.proxy.id));
        all
    }
}

/// Keeps the newest configuration version of a realm (`RedisCheckLatestConfigService`).
///
/// Every server publishes its `proxy.version`; a server whose version is older than the one in Redis is not
/// running the latest configuration and steps out of the leader election, so that a rolling update hands
/// the work of the realm to the new servers.
#[derive(Debug, Clone)]
pub struct RedisVersionStore {
    stores: RedisStores,
    key: String,
}

impl RedisVersionStore {
    /// The version key of a realm (`shinyproxy_{realmId}__version`).
    pub fn new(stores: RedisStores) -> Self {
        let key = format!("{}__version", stores.prefix);
        RedisVersionStore { stores, key }
    }

    /// The key that holds the version (used by tests).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Publishes `version` and answers whether this server runs the latest configuration.
    ///
    /// `None` means the check could not be made (Redis is unreachable or another server changed the key
    /// while this one was writing), which the caller reports and retries, exactly like `VersionChecker`.
    pub fn check_latest(&self, version: i64) -> Option<bool> {
        let mut connection = self.stores.connection()?;

        // WATCH/MULTI/EXEC, so that two servers starting at the same time cannot both win
        let _: Result<(), _> = redis::cmd("WATCH").arg(&self.key).query(&mut connection);
        let current: Option<i64> = redis::cmd("GET")
            .arg(&self.key)
            .query(&mut connection)
            .unwrap_or_default();

        match current {
            Some(current) if version == current => {
                let _: Result<(), _> = redis::cmd("UNWATCH").query(&mut connection);
                Some(true)
            }
            Some(current) if version < current => {
                let _: Result<(), _> = redis::cmd("UNWATCH").query(&mut connection);
                Some(false)
            }
            // this server is newer (or the first one): it publishes its version
            _ => {
                let updated: Option<Vec<redis::Value>> = redis::pipe()
                    .atomic()
                    .cmd("SET")
                    .arg(&self.key)
                    .arg(version)
                    .query(&mut connection)
                    .ok();
                match updated {
                    Some(answers) if !answers.is_empty() => Some(true),
                    // the transaction was aborted because another server wrote the key
                    _ => None,
                }
            }
        }
    }
}

/// A lock in Redis, used to elect the leader of a realm (`RedisLockRegistry`).
///
/// The key `shinyproxy_{realmId}__leader` holds the runtime id of the leader and expires, so a server that
/// disappears loses the lock automatically.
#[derive(Debug, Clone)]
pub struct RedisLock {
    stores: RedisStores,
    key: String,
}

impl RedisLock {
    /// The leader lock of a realm.
    pub fn leader(stores: RedisStores) -> Self {
        let key = format!("{}__leader", stores.prefix);
        RedisLock { stores, key }
    }

    /// Takes the lock, or renews it when this owner already holds it.
    ///
    /// Returns whether the caller holds the lock afterwards.
    pub fn acquire(&self, owner: &str, ttl: std::time::Duration) -> bool {
        let Some(mut connection) = self.stores.connection() else {
            return false;
        };
        let seconds = ttl.as_secs().max(1) as i64;

        // take the lock when it is free
        let taken: bool = redis::cmd("SET")
            .arg(&self.key)
            .arg(owner)
            .arg("NX")
            .arg("EX")
            .arg(seconds)
            .query(&mut connection)
            .unwrap_or(false);
        if taken {
            return true;
        }

        // renew it when this server holds it
        let holder: Option<String> = redis::cmd("GET")
            .arg(&self.key)
            .query(&mut connection)
            .unwrap_or_default();
        if holder.as_deref() == Some(owner) {
            let _: Result<(), _> = redis::cmd("EXPIRE")
                .arg(&self.key)
                .arg(seconds)
                .query(&mut connection);
            return true;
        }
        false
    }

    /// Releases the lock when this owner holds it.
    pub fn release(&self, owner: &str) {
        let Some(mut connection) = self.stores.connection() else {
            return;
        };
        let holder: Option<String> = redis::cmd("GET")
            .arg(&self.key)
            .query(&mut connection)
            .unwrap_or_default();
        if holder.as_deref() == Some(owner) {
            let _: Result<(), _> = redis::cmd("DEL").arg(&self.key).query(&mut connection);
        }
    }

    /// The current holder of the lock (used by tests).
    pub fn holder(&self) -> Option<String> {
        let mut connection = self.stores.connection()?;
        redis::cmd("GET")
            .arg(&self.key)
            .query(&mut connection)
            .unwrap_or_default()
    }
}

/// Connects to the Redis of the test environment, or returns `None` when there is none.
///
/// The unit tests that need Redis are skipped when it is not running, exactly like the integration tests
/// that check `SP_TEST_REDIS`.
#[cfg(test)]
pub(crate) fn test_stores(realm: &str) -> Option<RedisStores> {
    if std::env::var("SP_TEST_REDIS").as_deref() != Ok("1") {
        return None;
    }
    let url =
        std::env::var("SP_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let realm = format!("{realm}-{}", std::process::id());
    let stores = RedisStores::connect(
        &url,
        Some(realm.as_str()),
        std::sync::Arc::new(crate::model::runtime_value::RuntimeValueRegistry::engine()),
    )
    .ok()?;
    stores.clear_for_tests();
    Some(stores)
}

impl RedisStores {
    /// Removes every key of this realm (tests only).
    #[cfg(test)]
    pub(crate) fn clear_for_tests(&self) {
        let Some(mut connection) = self.connection() else {
            return;
        };
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(format!("{}*", self.prefix))
            .query(&mut connection)
            .unwrap_or_default();
        for key in keys {
            let _: Result<(), _> = redis::cmd("DEL").arg(key).query(&mut connection);
        }
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
