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

//! The data plane: proxying requests to the apps.
//!
//! In the Java implementation this is Undertow's `ProxyHandler` behind a `PathHandler` that Spring
//! dispatches into (`ProxyMappingManager`). Here the request handlers call [`ProxyRouter::forward`]
//! directly, which removes the need for the internal `/proxy_endpoint` paths and the 403 guard that
//! protects them.
//!
//! What has to behave exactly like Java:
//!
//! * request bodies and response bodies are streamed, never buffered (file uploads, downloads);
//! * WebSocket upgrades are tunnelled, with ShinyProxy injecting ping frames to detect activity;
//! * `X-Forwarded-Host` contains the host *including* a non-standard port;
//! * the app receives the configured `http-headers` plus `X-SP-UserId`/`X-SP-UserGroups`;
//! * cache headers follow `CacheHeadersMode`;
//! * when the app is gone, the JSON bodies `app_crashed` / `app_stopped_or_non_existent` are returned.

pub mod cache_headers;
pub mod http;
pub mod inject;
pub mod ws;

use std::collections::BTreeMap;

/// Marker put in the extensions of a response that came from an app.
///
/// The answers of the server itself carry the cache headers of Spring Security; the answers of an app keep
/// the headers the app chose (and whatever `proxy.default-cache-headers-mode` adds).
#[derive(Debug, Clone, Copy)]
pub struct AppAnswer;
use std::sync::Arc;

use dashmap::DashMap;

use crate::model::proxy::Proxy;

pub use http::{ForwardError, ForwardOptions};

/// Keeps track of where a proxy points to.
#[derive(Debug, Default)]
pub struct ProxyRouter {
    /// Proxy id to its targets (mapping name, `""` for the default mapping).
    targets: DashMap<String, BTreeMap<String, String>>,
}

impl ProxyRouter {
    /// An empty router.
    pub fn new() -> Self {
        ProxyRouter::default()
    }

    /// Registers the targets of a proxy (`ProxyMappingManager.addMappings`).
    pub fn add_mappings(&self, proxy: &Proxy) {
        if proxy.targets.is_empty() {
            return;
        }
        self.targets.insert(proxy.id.clone(), proxy.targets.clone());
    }

    /// Removes the targets of a proxy (`ProxyMappingManager.removeMappings`).
    pub fn remove_mappings(&self, proxy_id: &str) {
        self.targets.remove(proxy_id);
    }

    /// The targets of a proxy.
    pub fn targets(&self, proxy_id: &str) -> Option<BTreeMap<String, String>> {
        self.targets
            .get(proxy_id)
            .map(|entry| entry.value().clone())
    }

    /// Number of proxies with registered targets.
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// Whether no proxy has registered targets.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Resolves the target of a request.
    ///
    /// The sub path may start with the name of a port mapping, in which case the request goes to that
    /// mapping and the name is removed from the path; otherwise the default mapping is used.
    pub fn resolve<'a>(&self, proxy: &'a Proxy, sub_path: &'a str) -> Option<ResolvedTarget> {
        let targets = self
            .targets(&proxy.id)
            .or_else(|| Some(proxy.targets.clone()))?;
        if targets.is_empty() {
            return None;
        }

        let trimmed = sub_path.trim_start_matches('/');
        let (mapping, rest) = match trimmed.split_once('/') {
            Some((first, rest)) if targets.contains_key(first) => {
                (first.to_string(), rest.to_string())
            }
            _ => {
                // the whole sub path may be a mapping without a trailing slash
                if !trimmed.is_empty() && targets.contains_key(trimmed) {
                    (trimmed.to_string(), String::new())
                } else {
                    (String::new(), trimmed.to_string())
                }
            }
        };

        let target = targets.get(&mapping)?.clone();
        Some(ResolvedTarget {
            target,
            mapping,
            path: rest,
        })
    }
}

/// The target a request is proxied to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// Base URL of the app (including the target path of the mapping).
    pub target: String,
    /// Name of the mapping (`""` for the default mapping).
    pub mapping: String,
    /// Remaining path inside the app, without a leading slash.
    pub path: String,
}

impl ResolvedTarget {
    /// The URL of the request, including the query string.
    pub fn url(&self, query: Option<&str>) -> String {
        let mut url = format!("{}/{}", self.target.trim_end_matches('/'), self.path);
        if let Some(query) = query {
            if !query.is_empty() {
                url.push('?');
                url.push_str(query);
            }
        }
        url
    }
}

/// Shared handle to the router.
pub type SharedProxyRouter = Arc<ProxyRouter>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::ProxyStatus;

    fn proxy_with_targets(targets: &[(&str, &str)]) -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        for (mapping, target) in targets {
            proxy
                .targets
                .insert((*mapping).to_string(), (*target).to_string());
        }
        proxy
    }

    #[test]
    fn registers_and_removes_mappings() {
        let router = ProxyRouter::new();
        let proxy = proxy_with_targets(&[("", "http://127.0.0.1:20000")]);
        assert!(router.is_empty());

        router.add_mappings(&proxy);
        assert_eq!(router.len(), 1);
        assert_eq!(
            router
                .targets("proxy-1")
                .and_then(|targets| targets.get("").cloned()),
            Some("http://127.0.0.1:20000".to_string())
        );

        router.remove_mappings("proxy-1");
        assert!(router.is_empty());

        // proxies without targets are not registered
        router.add_mappings(&Proxy::new("empty", ProxyStatus::New));
        assert!(router.is_empty());
    }

    #[test]
    fn resolves_the_default_mapping() {
        let router = ProxyRouter::new();
        let proxy = proxy_with_targets(&[("", "http://127.0.0.1:20000")]);
        router.add_mappings(&proxy);

        let resolved = router.resolve(&proxy, "/").expect("resolved");
        assert_eq!(resolved.mapping, "");
        assert_eq!(resolved.path, "");
        assert_eq!(resolved.url(None), "http://127.0.0.1:20000/");

        let resolved = router.resolve(&proxy, "sub/page").expect("resolved");
        assert_eq!(resolved.path, "sub/page");
        assert_eq!(
            resolved.url(Some("a=1&b=2")),
            "http://127.0.0.1:20000/sub/page?a=1&b=2"
        );
    }

    #[test]
    fn resolves_named_mappings() {
        let router = ProxyRouter::new();
        let proxy = proxy_with_targets(&[
            ("", "http://127.0.0.1:20000"),
            ("dashboard", "http://127.0.0.1:20001/dash"),
        ]);
        router.add_mappings(&proxy);

        let resolved = router.resolve(&proxy, "dashboard/panel").expect("resolved");
        assert_eq!(resolved.mapping, "dashboard");
        assert_eq!(resolved.path, "panel");
        assert_eq!(resolved.url(None), "http://127.0.0.1:20001/dash/panel");

        // the mapping without a trailing slash
        let resolved = router.resolve(&proxy, "dashboard").expect("resolved");
        assert_eq!(resolved.mapping, "dashboard");
        assert_eq!(resolved.path, "");

        // unknown first segment goes to the app itself
        let resolved = router.resolve(&proxy, "dashboards/x").expect("resolved");
        assert_eq!(resolved.mapping, "");
        assert_eq!(resolved.path, "dashboards/x");
    }

    #[test]
    fn falls_back_to_the_targets_of_the_proxy() {
        // a proxy that was not registered (e.g. right after app recovery) still resolves
        let router = ProxyRouter::new();
        let proxy = proxy_with_targets(&[("", "http://127.0.0.1:20000")]);
        let resolved = router.resolve(&proxy, "/page").expect("resolved");
        assert_eq!(resolved.url(None), "http://127.0.0.1:20000/page");

        assert!(router
            .resolve(&Proxy::new("no-targets", ProxyStatus::New), "/")
            .is_none());
    }
}

/// Counts the open WebSocket tunnels (`WebSocketCounterService`).
///
/// `/actuator/recyclable` reports the count, so that a deployment does not replace a server that still
/// has users connected to an app.
#[derive(Debug, Default)]
pub struct WebSocketCounter {
    open: std::sync::atomic::AtomicUsize,
}

impl WebSocketCounter {
    /// A counter without connections.
    pub fn new() -> Self {
        WebSocketCounter::default()
    }

    /// A tunnel was opened.
    pub fn opened(&self) {
        self.open.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// A tunnel was closed.
    pub fn closed(&self) {
        // never go below zero, even when a close is reported twice
        let _ = self.open.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |current| Some(current.saturating_sub(1)),
        );
    }

    /// The number of open tunnels.
    pub fn count(&self) -> usize {
        self.open.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod counter_tests {
    use super::WebSocketCounter;

    #[test]
    fn counts_open_tunnels() {
        let counter = WebSocketCounter::new();
        assert_eq!(counter.count(), 0);
        counter.opened();
        counter.opened();
        assert_eq!(counter.count(), 2);
        counter.closed();
        assert_eq!(counter.count(), 1);
        counter.closed();
        counter.closed();
        assert_eq!(counter.count(), 0, "the count never goes below zero");
    }
}
