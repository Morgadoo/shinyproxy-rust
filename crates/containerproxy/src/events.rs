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

//! Application events.
//!
//! The Java implementation publishes Spring application events for everything that happens to a proxy
//! or a user; the usage-statistics collectors, the metrics, the status watchers of
//! `/api/proxy/{id}/status?watch=true` and the heartbeat bookkeeping all listen to them. This module
//! provides the same events on top of a `tokio::sync::broadcast` channel.

use tokio::sync::broadcast;

use crate::model::proxy::{Proxy, ProxyStopReason};

/// Capacity of the event channel; slow subscribers lose the oldest events (and log a warning).
const CHANNEL_CAPACITY: usize = 1024;

/// An event that happened in the engine.
#[derive(Debug, Clone)]
pub enum Event {
    /// A proxy is about to be created on the backend (`NewProxyEvent`).
    NewProxy { proxy: Box<Proxy> },
    /// A proxy became available (`ProxyStartEvent`).
    ProxyStarted {
        proxy: Box<Proxy>,
        startup_time_ms: Option<i64>,
    },
    /// A proxy failed to start (`ProxyStartFailedEvent`).
    ProxyStartFailed { proxy: Box<Proxy> },
    /// A proxy was stopped (`ProxyStopEvent`).
    ProxyStopped {
        proxy: Box<Proxy>,
        reason: ProxyStopReason,
    },
    /// A proxy was paused (`ProxyPauseEvent`).
    ProxyPaused { proxy: Box<Proxy> },
    /// A proxy was resumed (`ProxyResumeEvent`).
    ProxyResumed { proxy: Box<Proxy> },
    /// A user logged in (`UserLoginEvent`).
    UserLoggedIn { user_id: String },
    /// A user logged out (`UserLogoutEvent`); `expired` marks a session that timed out.
    UserLoggedOut { user_id: String, expired: bool },
    /// An authentication attempt failed (`AuthFailedEvent`).
    AuthenticationFailed { user_id: String },
}

impl Event {
    /// The proxy this event is about, when it is about a proxy.
    pub fn proxy(&self) -> Option<&Proxy> {
        match self {
            Event::NewProxy { proxy }
            | Event::ProxyStarted { proxy, .. }
            | Event::ProxyStartFailed { proxy }
            | Event::ProxyStopped { proxy, .. }
            | Event::ProxyPaused { proxy }
            | Event::ProxyResumed { proxy } => Some(proxy),
            _ => None,
        }
    }

    /// Short name of the event, used in log messages and metrics.
    pub fn name(&self) -> &'static str {
        match self {
            Event::NewProxy { .. } => "NewProxy",
            Event::ProxyStarted { .. } => "ProxyStart",
            Event::ProxyStartFailed { .. } => "ProxyStartFailed",
            Event::ProxyStopped { .. } => "ProxyStop",
            Event::ProxyPaused { .. } => "ProxyPause",
            Event::ProxyResumed { .. } => "ProxyResume",
            Event::UserLoggedIn { .. } => "UserLogin",
            Event::UserLoggedOut { .. } => "UserLogout",
            Event::AuthenticationFailed { .. } => "AuthFailed",
        }
    }
}

/// Publishes events to all subscribers.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        EventBus::new()
    }
}

impl EventBus {
    /// Creates a bus.
    pub fn new() -> Self {
        let (sender, _receiver) = broadcast::channel(CHANNEL_CAPACITY);
        EventBus { sender }
    }

    /// Publishes an event; events are dropped when nobody listens (as in Spring).
    pub fn publish(&self, event: Event) {
        let name = event.name();
        match self.sender.send(event) {
            Ok(subscribers) => tracing::trace!("published {name} to {subscribers} subscriber(s)"),
            Err(_) => tracing::trace!("published {name}, no subscribers"),
        }
    }

    /// Subscribes to all future events.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::ProxyStatus;

    fn proxy() -> Proxy {
        Proxy::new("proxy-1", ProxyStatus::Up)
    }

    #[tokio::test]
    async fn delivers_events_to_subscribers() {
        let bus = EventBus::new();
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        bus.publish(Event::ProxyStarted {
            proxy: Box::new(proxy()),
            startup_time_ms: Some(1234),
        });

        for receiver in [&mut first, &mut second] {
            let event = receiver.recv().await.expect("event");
            assert_eq!(event.name(), "ProxyStart");
            assert_eq!(
                event.proxy().map(|proxy| proxy.id.clone()),
                Some("proxy-1".to_string())
            );
        }
    }

    #[tokio::test]
    async fn events_without_subscribers_are_dropped() {
        let bus = EventBus::new();
        bus.publish(Event::UserLoggedIn {
            user_id: "jack".into(),
        });
        // subscribing afterwards does not replay events
        let mut receiver = bus.subscribe();
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn exposes_the_proxy_of_proxy_events() {
        let stopped = Event::ProxyStopped {
            proxy: Box::new(proxy()),
            reason: ProxyStopReason::ByUser,
        };
        assert!(stopped.proxy().is_some());
        assert!(Event::UserLoggedIn {
            user_id: "jack".into()
        }
        .proxy()
        .is_none());
    }
}
