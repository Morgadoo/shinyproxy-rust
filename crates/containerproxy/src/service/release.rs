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

//! Releasing apps that are no longer used (`ActiveProxiesService`, `ProxyMaxLifetimeService`,
//! `DefaultProxyLogoutStrategy`).
//!
//! Three things release an app:
//!
//! * **silence**: no heartbeat for longer than the heartbeat timeout of the app. Checked every
//!   `2 × proxy.heartbeat-rate`, exactly like the Java timer.
//! * **age**: the app is older than its `max-lifetime`. Checked every five minutes.
//! * **logout**: the user logged out and the app has `stop-on-logout` (or
//!   `proxy.default-stop-proxy-on-logout`, which is true by default).
//!
//! Releasing means stopping the app, unless `proxy.container-wait-time`... no: unless the app is released
//! by pausing, which the Java implementation does when the configured release strategy is
//! `PauseProxyReleaseStrategy`. ShinyProxy always stops apps, so [`ReleaseStrategy::Stop`] is the default.

use std::sync::Arc;
use std::time::Duration;

use crate::model::proxy::{now_millis, Proxy, ProxyStatus, ProxyStopReason};
use crate::model::runtime_value::{HEARTBEAT_TIMEOUT, MAX_LIFETIME};
use crate::service::ProxyService;
use crate::store::HeartbeatStore;

/// How an app is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReleaseStrategy {
    /// Stop the app (`StopProxyReleaseStrategy`, the default).
    #[default]
    Stop,
    /// Pause the app (`PauseProxyReleaseStrategy`).
    Pause,
}

/// Releases apps that are no longer used.
#[derive(Debug, Clone)]
pub struct ReleaseService {
    proxies: Arc<ProxyService>,
    heartbeats: Arc<dyn HeartbeatStore>,
    strategy: ReleaseStrategy,
    /// `2 × proxy.heartbeat-rate`, the interval of the silence check.
    cleanup_interval: Duration,
    /// Interval of the max lifetime check (five minutes in Java).
    lifetime_interval: Duration,
}

impl ReleaseService {
    /// Creates the service from the configuration.
    pub fn new(
        settings: &crate::config::Settings,
        proxies: Arc<ProxyService>,
        heartbeats: Arc<dyn HeartbeatStore>,
    ) -> Self {
        let rate = settings.proxy.heartbeat_rate_ms().max(1) as u64;
        ReleaseService {
            proxies,
            heartbeats,
            strategy: ReleaseStrategy::default(),
            cleanup_interval: Duration::from_millis(rate * 2),
            lifetime_interval: Duration::from_secs(5 * 60),
        }
    }

    /// Uses another release strategy (pausing instead of stopping).
    pub fn with_strategy(mut self, strategy: ReleaseStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Uses another interval for the max lifetime check (used by tests).
    pub fn with_lifetime_interval(mut self, interval: Duration) -> Self {
        self.lifetime_interval = interval;
        self
    }

    /// The interval of the silence check.
    pub fn cleanup_interval(&self) -> Duration {
        self.cleanup_interval
    }

    /// Starts the timers; they run until the process ends.
    pub fn spawn(self: Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        let silence = {
            let service = self.clone();
            tokio::spawn(async move {
                let mut timer = tokio::time::interval(service.cleanup_interval);
                // the first tick fires immediately, and the Java timer waits one interval first
                timer.tick().await;
                loop {
                    timer.tick().await;
                    service.release_inactive_proxies().await;
                }
            })
        };

        let lifetime = {
            let service = self.clone();
            tokio::spawn(async move {
                let mut timer = tokio::time::interval(service.lifetime_interval);
                timer.tick().await;
                loop {
                    timer.tick().await;
                    service.release_expired_proxies().await;
                }
            })
        };

        vec![silence, lifetime]
    }

    /// Releases the apps that have been silent for too long (`performCleanup`).
    pub async fn release_inactive_proxies(&self) {
        let now = now_millis();
        for proxy in self.proxies.all_proxies() {
            if let Some(silence) = self.silence_of(&proxy, now) {
                tracing::info!(
                    "Releasing inactive proxy [silence: {silence}ms] [proxyId: {}] [userId: {}]",
                    proxy.id,
                    proxy.user_id.clone().unwrap_or_default()
                );
                self.release(&proxy, ProxyStopReason::Inactivity).await;
            }
        }
    }

    /// How long an app has been silent, when it must be released for it.
    fn silence_of(&self, proxy: &Proxy, now: i64) -> Option<i64> {
        if proxy.status != ProxyStatus::Up {
            return None;
        }
        let timeout = proxy
            .runtime_values
            .get(&HEARTBEAT_TIMEOUT)
            .and_then(|value| value.data.as_int())
            .unwrap_or_default();
        if timeout <= 0 {
            // heartbeats are disabled for this app (or globally)
            return None;
        }
        // an app that never sent a heartbeat is measured from its startup
        let last = self
            .heartbeats
            .get(&proxy.id)
            .unwrap_or(proxy.startup_timestamp);
        let silence = now - last;
        (silence > timeout).then_some(silence)
    }

    /// Releases the apps that reached their max lifetime (`ProxyMaxLifetimeService`).
    pub async fn release_expired_proxies(&self) {
        let now = now_millis();
        for proxy in self.proxies.all_proxies() {
            if must_be_released_for_age(&proxy, now) {
                tracing::info!(
                    "Forcefully releasing proxy because it reached the max lifetime \
                     [uptime: {}] [proxyId: {}] [userId: {}]",
                    format_uptime(now - proxy.startup_timestamp),
                    proxy.id,
                    proxy.user_id.clone().unwrap_or_default()
                );
                self.release(&proxy, ProxyStopReason::ExceededMaxLifetime)
                    .await;
            }
        }
    }

    /// Stops the apps of a user that logged out (`DefaultProxyLogoutStrategy.onLogout`).
    ///
    /// `stop_on_logout` answers whether an app must be stopped, which needs the app definition (and is
    /// therefore provided by the caller).
    pub async fn on_logout(
        &self,
        user_id: &str,
        stop_on_logout: &(dyn Fn(&Proxy) -> bool + Send + Sync),
    ) -> Vec<String> {
        let mut stopped = Vec::new();
        for proxy in self.proxies.user_proxies(user_id) {
            if !stop_on_logout(&proxy) {
                continue;
            }
            tracing::info!(
                "Stopping proxy because the user logged out [proxyId: {}] [userId: {user_id}]",
                proxy.id
            );
            if let Err(error) = self
                .proxies
                .stop_proxy(&proxy, ProxyStopReason::Logout)
                .await
            {
                tracing::warn!("cannot stop proxy {}: {error}", proxy.id);
                continue;
            }
            stopped.push(proxy.id.clone());
        }
        stopped
    }

    /// Releases one app with the configured strategy.
    async fn release(&self, proxy: &Proxy, reason: ProxyStopReason) {
        match self.strategy {
            ReleaseStrategy::Stop => {
                if let Err(error) = self.proxies.stop_proxy(proxy, reason).await {
                    tracing::warn!("cannot stop proxy {}: {error}", proxy.id);
                }
            }
            ReleaseStrategy::Pause => {
                if let Err(error) = self.proxies.pause_proxy(proxy).await {
                    tracing::warn!("cannot pause proxy {}: {error}", proxy.id);
                }
            }
        }
    }
}

/// Whether an app is older than its max lifetime.
pub fn must_be_released_for_age(proxy: &Proxy, now: i64) -> bool {
    if proxy.status != ProxyStatus::Up {
        return false;
    }
    let max_lifetime_minutes = proxy
        .runtime_values
        .get(&MAX_LIFETIME)
        .and_then(|value| value.data.as_int())
        .unwrap_or(-1);
    if max_lifetime_minutes <= 0 {
        return false;
    }
    let deadline = now - max_lifetime_minutes * 60_000;
    proxy.startup_timestamp < deadline
}

/// Formats a duration like Java's `DurationFormatUtils.formatDurationWords(millis, true, false)`.
///
/// Used in the log line of an app that reached its max lifetime, so that log parsers keep working.
pub fn format_uptime(millis: i64) -> String {
    let millis = millis.max(0);
    let total_seconds = millis / 1000;
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    let mut parts = Vec::new();
    let mut push = |value: i64, singular: &str| {
        if value != 0 {
            parts.push(format!(
                "{value} {}",
                if value == 1 {
                    singular.to_string()
                } else {
                    format!("{singular}s")
                }
            ));
        }
    };
    push(days, "day");
    push(hours, "hour");
    push(minutes, "minute");
    push(seconds, "second");

    if parts.is_empty() {
        // Java keeps the seconds when everything is zero
        return "0 seconds".to_string();
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::runtime_value::RuntimeValue;

    fn proxy(status: ProxyStatus, timeout: i64, max_lifetime: i64, startup: i64) -> Proxy {
        let mut proxy = Proxy::new("proxy-1", status);
        proxy.startup_timestamp = startup;
        proxy.add_runtime_value(RuntimeValue::integer(&HEARTBEAT_TIMEOUT, timeout), true);
        proxy.add_runtime_value(RuntimeValue::integer(&MAX_LIFETIME, max_lifetime), true);
        proxy
    }

    #[test]
    fn releases_apps_that_reached_their_max_lifetime() {
        let now = 10 * 60_000; // ten minutes after the epoch
                               // an app with a lifetime of five minutes, started at the epoch
        assert!(must_be_released_for_age(
            &proxy(ProxyStatus::Up, 60_000, 5, 0),
            now
        ));
        // ... but not when it started one minute ago
        assert!(!must_be_released_for_age(
            &proxy(ProxyStatus::Up, 60_000, 5, now - 60_000),
            now
        ));
        // no max lifetime
        assert!(!must_be_released_for_age(
            &proxy(ProxyStatus::Up, 60_000, -1, 0),
            now
        ));
        // apps that are not up are left alone
        assert!(!must_be_released_for_age(
            &proxy(ProxyStatus::Stopping, 60_000, 5, 0),
            now
        ));
    }

    #[test]
    fn formats_uptime_like_java() {
        assert_eq!(format_uptime(0), "0 seconds");
        assert_eq!(format_uptime(1_000), "1 second");
        assert_eq!(format_uptime(59_000), "59 seconds");
        assert_eq!(format_uptime(60_000), "1 minute");
        assert_eq!(format_uptime(61_000), "1 minute 1 second");
        assert_eq!(format_uptime(3_600_000), "1 hour");
        assert_eq!(format_uptime(3_661_000), "1 hour 1 minute 1 second");
        assert_eq!(format_uptime(90_061_000), "1 day 1 hour 1 minute 1 second");
        assert_eq!(format_uptime(-5), "0 seconds");
    }

    #[test]
    fn derives_the_cleanup_interval_from_the_heartbeat_rate() {
        let settings: crate::config::Settings =
            serde_yaml_ng::from_str("proxy:\n  heartbeat-rate: 5000\n").expect("settings");
        let rate = settings.proxy.heartbeat_rate_ms();
        assert_eq!(rate, 5000);
        assert_eq!(
            Duration::from_millis(rate as u64 * 2),
            Duration::from_secs(10)
        );
    }
}
