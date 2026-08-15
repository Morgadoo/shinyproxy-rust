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

//! Counts the users that are logged in (`ISessionService`).
//!
//! The Java implementation has two implementations of this service: one that walks the sessions of
//! Undertow, and one that scans the session keys in Redis. Both feed the `absolute_users_logged_in` and
//! `absolute_users_active` gauges, where "active" means the session was used in the last minute.

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;

/// A session is active when it was used in the last minute (`RedisSessionService`).
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(60);

/// Counts the users of the sessions of this realm.
#[async_trait]
pub trait SessionService: Send + Sync + std::fmt::Debug {
    /// Number of users that are logged in, or `None` when it is not known yet.
    async fn logged_in_users(&self) -> Option<i64>;

    /// Number of users that used their session in the last minute.
    async fn active_users(&self) -> Option<i64>;

    /// Remembers that a user used their session (only the in-memory implementation needs this).
    fn touch(&self, _session_id: &str, _user_id: &str) {}

    /// Forgets a session (used when a user signs out).
    fn forget(&self, _session_id: &str) {}

    /// Whether the expiry of this session should be written again.
    ///
    /// Spring Session writes the last access time of a session on every request. Writing the session on every
    /// request costs a serialisation (and a Redis round trip in a high availability setup), so the expiry is
    /// only moved when a quarter of the timeout has passed since the last time — the session still never
    /// expires while it is used, and a request costs nothing extra.
    fn should_refresh_expiry(&self, _session_id: &str, _timeout: Duration) -> bool {
        true
    }

    /// Keeps a session alive while its app is used (`SessionReActivatorService`).
    ///
    /// The heartbeats of an app come from the browser through a WebSocket, so a user that only looks at
    /// their app — without loading a page of ShinyProxy — would otherwise have their session expire.
    fn reactivate(&self, _session_id: &str, _user_id: &str) {}
}

/// Counts the sessions of this server, for the in-memory session store.
#[derive(Debug)]
pub struct MemorySessionService {
    /// Session id to the user and the time it was last used (epoch millis).
    sessions: DashMap<String, (String, i64)>,
    /// When the expiry of a session was last written (epoch millis), for [`should_refresh_expiry`].
    refreshed: DashMap<String, i64>,
    /// How long a session lives without being used.
    timeout: Duration,
}

impl MemorySessionService {
    /// Creates the service; sessions that are not used within `timeout` are forgotten.
    pub fn new(timeout: Duration) -> Self {
        MemorySessionService {
            sessions: DashMap::new(),
            refreshed: DashMap::new(),
            timeout,
        }
    }

    /// Removes the sessions that timed out.
    fn expire(&self) {
        let deadline = crate::model::proxy::now_millis() - self.timeout.as_millis() as i64;
        self.sessions
            .retain(|_, (_, last_used)| *last_used > deadline);
        self.refreshed.retain(|_, refreshed| *refreshed > deadline);
    }

    /// Remembers that the expiry of a session was written, and answers whether it was due.
    fn due_for_refresh(&self, session_id: &str, timeout: Duration) -> bool {
        let now = crate::model::proxy::now_millis();
        let interval = (timeout.as_millis() as i64 / 4).max(1);
        match self.refreshed.get(session_id).map(|entry| *entry.value()) {
            Some(last) if now - last < interval => false,
            _ => {
                self.refreshed.insert(session_id.to_string(), now);
                true
            }
        }
    }
}

#[async_trait]
impl SessionService for MemorySessionService {
    async fn logged_in_users(&self) -> Option<i64> {
        self.expire();
        let users: HashSet<String> = self
            .sessions
            .iter()
            .map(|entry| entry.value().0.clone())
            .collect();
        Some(users.len() as i64)
    }

    async fn active_users(&self) -> Option<i64> {
        self.expire();
        let deadline = crate::model::proxy::now_millis() - ACTIVE_WINDOW.as_millis() as i64;
        let users: HashSet<String> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().1 > deadline)
            .map(|entry| entry.value().0.clone())
            .collect();
        Some(users.len() as i64)
    }

    fn touch(&self, session_id: &str, user_id: &str) {
        self.sessions.insert(
            session_id.to_string(),
            (user_id.to_string(), crate::model::proxy::now_millis()),
        );
    }

    fn forget(&self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    fn reactivate(&self, session_id: &str, user_id: &str) {
        self.touch(session_id, user_id);
    }

    fn should_refresh_expiry(&self, session_id: &str, timeout: Duration) -> bool {
        self.due_for_refresh(session_id, timeout)
    }
}

/// Counts the sessions of the whole realm, by scanning the session keys in Redis.
///
/// Like the Java implementation, the counts are cached and refreshed by a timer, because scanning every
/// session is too expensive to do on every scrape.
#[derive(Debug)]
pub struct RedisSessionService {
    store: crate::store::RedisSessionStore,
    /// Cached counts (`-1` until the first refresh, as Java starts with `null`).
    logged_in: AtomicI64,
    active: AtomicI64,
    /// When the expiry of a session was last written by *this* server (epoch millis).
    refreshed: DashMap<String, i64>,
}

impl RedisSessionService {
    /// Creates the service around the session store.
    pub fn new(store: crate::store::RedisSessionStore) -> Self {
        RedisSessionService {
            store,
            logged_in: AtomicI64::new(-1),
            active: AtomicI64::new(-1),
            refreshed: DashMap::new(),
        }
    }

    /// Refreshes the cached counts.
    pub async fn refresh(&self) {
        match self.store.count_users(ACTIVE_WINDOW).await {
            Ok((logged_in, active)) => {
                self.logged_in.store(logged_in, Ordering::Relaxed);
                self.active.store(active, Ordering::Relaxed);
            }
            Err(error) => tracing::debug!("cannot count the sessions in Redis: {error}"),
        }
    }

    /// Refreshes the counts every 20 seconds (`CACHE_UPDATE_INTERVAL` in Java).
    pub fn spawn_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(20));
            loop {
                ticker.tick().await;
                self.refresh().await;
            }
        });
    }
}

#[async_trait]
impl SessionService for RedisSessionService {
    fn should_refresh_expiry(&self, session_id: &str, timeout: Duration) -> bool {
        let now = crate::model::proxy::now_millis();
        let interval = (timeout.as_millis() as i64 / 4).max(1);
        // the map is trimmed with the same window, so it cannot grow without bound
        self.refreshed
            .retain(|_, refreshed| now - *refreshed < timeout.as_millis() as i64);
        match self.refreshed.get(session_id).map(|entry| *entry.value()) {
            Some(last) if now - last < interval => false,
            _ => {
                self.refreshed.insert(session_id.to_string(), now);
                true
            }
        }
    }

    fn reactivate(&self, session_id: &str, _user_id: &str) {
        // the expiry of the session in Redis is moved forward; the call is asynchronous, so it happens in
        // the background of the heartbeat
        let store = self.store.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            if let Err(error) = store.extend(&session_id).await {
                tracing::debug!("cannot keep the session {session_id} alive: {error}");
            }
        });
    }

    async fn logged_in_users(&self) -> Option<i64> {
        match self.logged_in.load(Ordering::Relaxed) {
            -1 => None,
            value => Some(value),
        }
    }

    async fn active_users(&self) -> Option<i64> {
        match self.active.load(Ordering::Relaxed) {
            -1 => None,
            value => Some(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_the_users_of_the_sessions() {
        let service = MemorySessionService::new(Duration::from_secs(3600));
        assert_eq!(service.logged_in_users().await, Some(0));

        // two sessions of the same user count once
        service.touch("session-1", "jack");
        service.touch("session-2", "jack");
        service.touch("session-3", "jeff");
        assert_eq!(service.logged_in_users().await, Some(2));
        assert_eq!(service.active_users().await, Some(2));

        // signing out forgets the session
        service.forget("session-3");
        assert_eq!(service.logged_in_users().await, Some(1));
    }

    #[tokio::test]
    async fn only_counts_recently_used_sessions_as_active() {
        let service = MemorySessionService::new(Duration::from_secs(3600));
        service.touch("session-1", "jack");
        service.touch("session-2", "jeff");

        // jack used their session two minutes ago: logged in, but not active
        service.sessions.insert(
            "session-1".to_string(),
            (
                "jack".to_string(),
                crate::model::proxy::now_millis() - 120_000,
            ),
        );
        assert_eq!(service.logged_in_users().await, Some(2));
        assert_eq!(service.active_users().await, Some(1));
    }

    #[tokio::test]
    async fn a_heartbeat_keeps_a_session_alive() {
        let service = MemorySessionService::new(Duration::from_secs(60));
        // a session that is about to time out
        service.sessions.insert(
            "session-1".to_string(),
            (
                "jack".to_string(),
                crate::model::proxy::now_millis() - 50_000,
            ),
        );

        // the heartbeat of the app keeps it alive and active
        service.reactivate("session-1", "jack");
        assert_eq!(service.logged_in_users().await, Some(1));
        assert_eq!(service.active_users().await, Some(1));
    }

    #[tokio::test]
    async fn forgets_sessions_that_timed_out() {
        let service = MemorySessionService::new(Duration::from_secs(60));
        service.sessions.insert(
            "session-1".to_string(),
            (
                "jack".to_string(),
                crate::model::proxy::now_millis() - 120_000,
            ),
        );
        service.touch("session-2", "jeff");

        assert_eq!(service.logged_in_users().await, Some(1));
        assert_eq!(service.sessions.len(), 1, "the old session is removed");
    }
}
