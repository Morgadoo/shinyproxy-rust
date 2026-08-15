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

//! Usage statistics (`stat/`).
//!
//! The metrics are exposed on `/actuator/prometheus` of the management server with the same names and
//! labels as the Micrometer registry of the Java implementation, so that existing dashboards and alerts
//! keep working: counters get the `_total` suffix, tag keys use `_` instead of `.`, every metric carries
//! the `shinyproxy_instance` and `shinyproxy_realm` labels, and `proxy.usage-stats-micrometer-prefix`
//! prefixes every name.

pub mod collectors;
pub mod prometheus;

use std::collections::BTreeMap;
use std::sync::Arc;

use dashmap::DashMap;

use crate::events::{Event, EventBus};
use crate::model::proxy::{Proxy, ProxyStatus, ProxyStopReason};
use crate::service::identifier::Identifiers;

/// The value of the `appInfo` gauge per status, as in the Java `PROXY_STATUS_TO_INTEGER`.
pub fn status_value(status: ProxyStatus) -> i64 {
    match status {
        ProxyStatus::New => 1,
        ProxyStatus::Up => 10,
        ProxyStatus::Pausing | ProxyStatus::Paused => 20,
        ProxyStatus::Resuming => 30,
        ProxyStatus::Stopping | ProxyStatus::Stopped => 40,
    }
}

/// The value of the `appInfo` gauge of a crashed app.
pub const STATUS_CRASHED: i64 = 50;
/// The value of the `appInfo` gauge of an app that failed to start.
pub const STATUS_FAILED_TO_START: i64 = 100;

/// A metric identified by its name and its labels.
type Key = (String, BTreeMap<String, String>);

/// All metrics at a point in time: counters, gauges and timers.
pub(crate) type Snapshot = (Vec<(Key, u64)>, Vec<(Key, f64)>, Vec<(Key, (u64, f64))>);

/// Collects the metrics of this server.
#[derive(Debug)]
pub struct Metrics {
    /// Counters (exposed with the `_total` suffix).
    counters: DashMap<Key, u64>,
    /// Gauges.
    gauges: DashMap<Key, f64>,
    /// Timers: total number of recordings and the total duration in seconds.
    timers: DashMap<Key, (u64, f64)>,
    /// Labels added to every metric.
    common_labels: BTreeMap<String, String>,
    /// Prefix of every metric name (`proxy.usage-stats-micrometer-prefix`).
    prefix: String,
}

impl Metrics {
    /// Creates the registry.
    pub fn new(prefix: Option<&str>, identifiers: &Identifiers) -> Self {
        let prefix = prefix.map(str::trim).unwrap_or_default();
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}_")
        };

        let mut common_labels = BTreeMap::new();
        // `instance` is already a Prometheus label, hence the shinyproxy_ prefix (as in Java)
        common_labels.insert(
            "shinyproxy_instance".to_string(),
            identifiers.instance_id.clone(),
        );
        common_labels.insert(
            "shinyproxy_realm".to_string(),
            identifiers.realm_id.clone().unwrap_or_default(),
        );

        Metrics {
            counters: DashMap::new(),
            gauges: DashMap::new(),
            timers: DashMap::new(),
            common_labels,
            prefix,
        }
    }

    /// Registers the metrics of an app definition, so that they are exposed with value 0 before the first
    /// app starts (the Java implementation registers them at startup as well).
    pub fn register_spec(&self, spec_id: &str, container_indexes: &[i64]) {
        let spec = labels([("spec_id", spec_id)]);
        for name in ["appStarts", "appStops", "appCrashes", "startFailed"] {
            self.counters
                .entry((name.to_string(), spec.clone()))
                .or_insert(0);
        }
        self.gauges
            .entry(("absolute_apps_running".to_string(), spec.clone()))
            .or_insert(0.0);
        for name in ["startupTime", "applicationStartupTime", "usageTime"] {
            self.timers
                .entry((name.to_string(), spec.clone()))
                .or_insert((0, 0.0));
        }
        for index in container_indexes {
            let container = labels([("spec_id", spec_id), ("container_idx", &index.to_string())]);
            for name in [
                "imagePullTime",
                "containerScheduleTime",
                "containerStartupTime",
            ] {
                self.timers
                    .entry((name.to_string(), container.clone()))
                    .or_insert((0, 0.0));
            }
        }
    }

    /// Increments a counter.
    pub fn increment(&self, name: &str, labels: BTreeMap<String, String>) {
        *self.counters.entry((name.to_string(), labels)).or_insert(0) += 1;
    }

    /// Sets a gauge.
    pub fn set_gauge(&self, name: &str, labels: BTreeMap<String, String>, value: f64) {
        self.gauges.insert((name.to_string(), labels), value);
    }

    /// Removes a gauge (used when an app disappears).
    pub fn remove_gauge(&self, name: &str, predicate: &dyn Fn(&BTreeMap<String, String>) -> bool) {
        self.gauges
            .retain(|(metric, labels), _| metric != name || !predicate(labels));
    }

    /// Records a duration.
    pub fn record(&self, name: &str, labels: BTreeMap<String, String>, duration_ms: i64) {
        let mut entry = self
            .timers
            .entry((name.to_string(), labels))
            .or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += duration_ms.max(0) as f64 / 1000.0;
    }

    /// The value of a counter (used by tests).
    pub fn counter_value(&self, name: &str, labels: BTreeMap<String, String>) -> u64 {
        self.counters
            .get(&(name.to_string(), labels))
            .map(|value| *value)
            .unwrap_or_default()
    }

    /// The value of a gauge (used by tests).
    pub fn gauge_value(&self, name: &str, labels: BTreeMap<String, String>) -> Option<f64> {
        self.gauges
            .get(&(name.to_string(), labels))
            .map(|value| *value)
    }

    /// The metrics in the Prometheus text format.
    pub fn to_prometheus(&self) -> String {
        prometheus::render(self)
    }

    /// The prefix of every metric name.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The labels added to every metric.
    pub fn common_labels(&self) -> &BTreeMap<String, String> {
        &self.common_labels
    }

    /// Counters, gauges and timers, for the exposition format.
    pub(crate) fn snapshot(&self) -> Snapshot {
        let mut counters: Vec<(Key, u64)> = self
            .counters
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect();
        let mut gauges: Vec<(Key, f64)> = self
            .gauges
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect();
        let mut timers: Vec<(Key, (u64, f64))> = self
            .timers
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect();
        counters.sort_by(|left, right| left.0.cmp(&right.0));
        gauges.sort_by(|left, right| left.0.cmp(&right.0));
        timers.sort_by(|left, right| left.0.cmp(&right.0));
        (counters, gauges, timers)
    }

    /// Subscribes to the event bus and updates the metrics, like the Java `Micrometer` collector.
    pub fn subscribe(self: &Arc<Self>, events: &EventBus) {
        let metrics = self.clone();
        let mut receiver = events.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = receiver.recv().await {
                metrics.handle(&event);
            }
        });
    }

    /// Updates the metrics for one event.
    pub fn handle(&self, event: &Event) {
        match event {
            Event::UserLoggedIn { .. } => self.increment("userLogins", BTreeMap::new()),
            Event::UserLoggedOut { .. } => self.increment("userLogouts", BTreeMap::new()),
            Event::AuthenticationFailed { .. } => self.increment("authFailed", BTreeMap::new()),
            // the seats of the shared containers are reported by the scaler, not per event
            Event::SeatReleased { .. } => {}
            Event::NewProxy { proxy } => {
                self.set_app_info(proxy, status_value(ProxyStatus::New));
            }
            Event::ProxyStarted {
                proxy,
                startup_time_ms,
            } => {
                let spec = labels([("spec_id", spec_id_of(proxy))]);
                self.increment("appStarts", spec.clone());
                if let Some(duration) = startup_time_ms {
                    self.record("startupTime", spec, *duration);
                }
                self.set_app_info(proxy, status_value(ProxyStatus::Up));
            }
            Event::ProxyStartFailed { proxy } => {
                self.increment("startFailed", labels([("spec_id", spec_id_of(proxy))]));
                self.set_app_info(proxy, STATUS_FAILED_TO_START);
            }
            Event::ProxyStopped { proxy, reason } => {
                let spec = labels([("spec_id", spec_id_of(proxy))]);
                self.increment("appStops", spec.clone());
                if proxy.startup_timestamp > 0 {
                    let usage = crate::model::proxy::now_millis() - proxy.startup_timestamp;
                    self.record("usageTime", spec.clone(), usage);
                }
                if *reason == ProxyStopReason::Crashed {
                    self.increment("appCrashes", spec);
                    self.set_app_info(proxy, STATUS_CRASHED);
                } else {
                    self.set_app_info(proxy, status_value(ProxyStatus::Stopped));
                }
            }
            Event::ProxyPaused { proxy } => {
                self.set_app_info(proxy, status_value(ProxyStatus::Paused));
            }
            Event::ProxyResumed { proxy } => {
                self.set_app_info(proxy, status_value(ProxyStatus::Up));
            }
        }
    }

    /// Sets the `appInfo` gauge of a proxy, replacing the previous value.
    fn set_app_info(&self, proxy: &Proxy, value: i64) {
        let backend_name = proxy
            .containers
            .first()
            .and_then(|container| {
                container
                    .runtime_values
                    .get(&crate::model::runtime_value::BACKEND_CONTAINER_NAME)
            })
            .and_then(|value| {
                value
                    .data
                    .parse_json::<crate::model::runtime_value::BackendContainerName>()
            });

        let instance = proxy
            .runtime_values
            .value_string(&crate::model::runtime_value::INSTANCE_ID)
            .unwrap_or_default();
        let proxy_id = proxy.id.clone();
        let labels = labels([
            ("spec_id", spec_id_of(proxy)),
            ("user_id", proxy.user_id.as_deref().unwrap_or("")),
            ("proxy_instance", &instance),
            ("proxy_id", &proxy_id),
            (
                "proxy_created_timestamp",
                &proxy.created_timestamp.to_string(),
            ),
            (
                "resource_id",
                backend_name
                    .as_ref()
                    .map(|name| name.name.as_str())
                    .unwrap_or("NA"),
            ),
            (
                "proxy_namespace",
                backend_name
                    .as_ref()
                    .map(|name| name.namespace.as_str())
                    .unwrap_or("NA"),
            ),
        ]);

        // one gauge per proxy: the previous value of this proxy is replaced
        let id = proxy.id.clone();
        self.remove_gauge("appInfo", &move |existing| {
            existing.get("proxy_id").map(String::as_str) == Some(id.as_str())
        });
        self.set_gauge("appInfo", labels, value as f64);
    }

    /// Updates the gauges that count running apps (called by the timer of the caller).
    pub fn update_running_apps(&self, proxies: &[Proxy]) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for proxy in proxies {
            *counts.entry(spec_id_of(proxy).to_string()).or_insert(0) += 1;
        }
        // every known spec keeps its gauge, so that a spec without apps reports 0
        let known: Vec<Key> = self
            .gauges
            .iter()
            .filter(|entry| entry.key().0 == "absolute_apps_running")
            .map(|entry| entry.key().clone())
            .collect();
        for (name, labels) in known {
            let spec_id = labels.get("spec_id").cloned().unwrap_or_default();
            let count = counts.remove(&spec_id).unwrap_or(0);
            self.set_gauge(&name, labels, count as f64);
        }
        // apps of specs that are not configured (recovered apps of another configuration)
        for (spec_id, count) in counts {
            self.set_gauge(
                "absolute_apps_running",
                labels([("spec_id", &spec_id)]),
                count as f64,
            );
        }
    }
}

/// The spec id of a proxy, or an empty string.
fn spec_id_of(proxy: &Proxy) -> &str {
    proxy.spec_id.as_deref().unwrap_or("")
}

/// Builds a label map.
pub fn labels<'a, const N: usize>(entries: [(&'a str, &'a str); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::runtime_value::{BackendContainerName, RuntimeValue, INSTANCE_ID};

    fn identifiers() -> Identifiers {
        Identifiers {
            runtime_id: "runtime".to_string(),
            instance_id: "instance-1".to_string(),
            realm_id: Some("realm-1".to_string()),
            version: None,
        }
    }

    fn proxy() -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".to_string());
        proxy.user_id = Some("jack".to_string());
        proxy.created_timestamp = 1_700_000_000_000;
        proxy.startup_timestamp = crate::model::proxy::now_millis() - 5_000;
        proxy.add_runtime_value(RuntimeValue::string(&INSTANCE_ID, "instance-1"), true);
        let mut container = crate::model::proxy::Container::new(0);
        container.add_runtime_value(
            RuntimeValue::json(
                &crate::model::runtime_value::BACKEND_CONTAINER_NAME,
                BackendContainerName::new("default/sp-container-proxy-1-0"),
            ),
            true,
        );
        proxy.containers.push(container);
        proxy
    }

    #[test]
    fn counts_events_like_the_java_collector() {
        let metrics = Metrics::new(None, &identifiers());
        metrics.register_spec("01_hello", &[0]);

        let spec = labels([("spec_id", "01_hello")]);
        assert_eq!(metrics.counter_value("appStarts", spec.clone()), 0);

        metrics.handle(&Event::ProxyStarted {
            proxy: Box::new(proxy()),
            startup_time_ms: Some(1500),
        });
        assert_eq!(metrics.counter_value("appStarts", spec.clone()), 1);

        metrics.handle(&Event::ProxyStopped {
            proxy: Box::new(proxy()),
            reason: ProxyStopReason::Crashed,
        });
        assert_eq!(metrics.counter_value("appStops", spec.clone()), 1);
        assert_eq!(metrics.counter_value("appCrashes", spec.clone()), 1);

        metrics.handle(&Event::ProxyStartFailed {
            proxy: Box::new(proxy()),
        });
        assert_eq!(metrics.counter_value("startFailed", spec), 1);

        metrics.handle(&Event::UserLoggedIn {
            user_id: "jack".to_string(),
        });
        metrics.handle(&Event::UserLoggedOut {
            user_id: "jack".to_string(),
            expired: false,
        });
        metrics.handle(&Event::AuthenticationFailed {
            user_id: "jack".to_string(),
        });
        assert_eq!(metrics.counter_value("userLogins", BTreeMap::new()), 1);
        assert_eq!(metrics.counter_value("userLogouts", BTreeMap::new()), 1);
        assert_eq!(metrics.counter_value("authFailed", BTreeMap::new()), 1);
    }

    #[test]
    fn keeps_one_app_info_gauge_per_proxy() {
        let metrics = Metrics::new(None, &identifiers());
        metrics.handle(&Event::NewProxy {
            proxy: Box::new(proxy()),
        });
        let gauges = metrics.snapshot().1;
        let app_info: Vec<_> = gauges
            .iter()
            .filter(|((name, _), _)| name == "appInfo")
            .collect();
        assert_eq!(app_info.len(), 1);
        assert_eq!(app_info[0].1, 1.0, "a new proxy has value 1");
        let labels = &app_info[0].0 .1;
        assert_eq!(labels.get("spec_id").map(String::as_str), Some("01_hello"));
        assert_eq!(labels.get("user_id").map(String::as_str), Some("jack"));
        assert_eq!(
            labels.get("proxy_created_timestamp").map(String::as_str),
            Some("1700000000000")
        );
        assert_eq!(
            labels.get("resource_id").map(String::as_str),
            Some("sp-container-proxy-1-0")
        );
        assert_eq!(
            labels.get("proxy_namespace").map(String::as_str),
            Some("default")
        );

        // starting the app replaces the gauge instead of adding one
        metrics.handle(&Event::ProxyStarted {
            proxy: Box::new(proxy()),
            startup_time_ms: None,
        });
        let gauges = metrics.snapshot().1;
        let app_info: Vec<_> = gauges
            .iter()
            .filter(|((name, _), _)| name == "appInfo")
            .collect();
        assert_eq!(app_info.len(), 1);
        assert_eq!(app_info[0].1, 10.0, "a running proxy has value 10");
    }

    #[test]
    fn counts_running_apps_per_spec() {
        let metrics = Metrics::new(None, &identifiers());
        metrics.register_spec("01_hello", &[0]);
        metrics.register_spec("02_other", &[0]);

        metrics.update_running_apps(&[proxy(), proxy()]);
        assert_eq!(
            metrics.gauge_value("absolute_apps_running", labels([("spec_id", "01_hello")])),
            Some(2.0)
        );
        assert_eq!(
            metrics.gauge_value("absolute_apps_running", labels([("spec_id", "02_other")])),
            Some(0.0),
            "a spec without apps reports 0"
        );

        metrics.update_running_apps(&[]);
        assert_eq!(
            metrics.gauge_value("absolute_apps_running", labels([("spec_id", "01_hello")])),
            Some(0.0)
        );
    }

    #[test]
    fn maps_statuses_like_java() {
        assert_eq!(status_value(ProxyStatus::New), 1);
        assert_eq!(status_value(ProxyStatus::Up), 10);
        assert_eq!(status_value(ProxyStatus::Pausing), 20);
        assert_eq!(status_value(ProxyStatus::Paused), 20);
        assert_eq!(status_value(ProxyStatus::Resuming), 30);
        assert_eq!(status_value(ProxyStatus::Stopping), 40);
        assert_eq!(status_value(ProxyStatus::Stopped), 40);
        assert_eq!(STATUS_CRASHED, 50);
        assert_eq!(STATUS_FAILED_TO_START, 100);
    }
}
