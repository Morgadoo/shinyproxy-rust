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

//! End-to-end tests of the proxy lifecycle with the `local` backend and the `sp-testapp` fixture.
//!
//! This is the Rust counterpart of the Java lifecycle tests that need a Docker daemon: a real app
//! process is started, the proxy becomes reachable, the runtime values arrive in the app as environment
//! variables, and stopping the proxy cleans everything up.

use std::collections::BTreeMap;
use std::sync::Arc;

use containerproxy::backend::{self, PortAllocator};
use containerproxy::config::{load, LoadOptions, Schema, Settings};
use containerproxy::events::{Event, EventBus};
use containerproxy::model::proxy::{ProxyStatus, ProxyStopReason};
use containerproxy::model::runtime_value::{
    RuntimeValue, RuntimeValueRegistry, DISPLAY_NAME, PUBLIC_PATH, USER_GROUPS, USER_ID,
};
use containerproxy::model::spec::{ContainerSpec, PortMapping, ProxySpec};
use containerproxy::model::spel_field::{SpelString, SpelStringList, SpelStringMap};
use containerproxy::service::identifier::Identifiers;
use containerproxy::service::ProxyService;
use containerproxy::spec::expression::UserContext;
use containerproxy::store::{MemoryHeartbeatStore, MemoryProxyStore};

/// Builds a proxy service with the `local` backend on a dedicated port range.
fn build_service(yaml: &str, port_range_start: u16) -> (Arc<ProxyService>, Arc<PortAllocator>) {
    let settings: Settings = serde_yaml_ng::from_str(yaml).expect("settings");
    let settings = Arc::new(settings);

    let directory = tempfile::tempdir().expect("temp dir");
    let options = LoadOptions {
        working_dir: Some(directory.path().to_path_buf()),
        ..LoadOptions::default()
    };
    let raw = load(&Schema::engine(), &options).expect("config");
    let identifiers = Identifiers::from_config(&raw, None);

    let allocator = Arc::new(PortAllocator::new(port_range_start, None));
    let backend = backend::create(
        &settings,
        backend::BackendContext {
            port_allocator: allocator.clone(),
            registry: Arc::new(RuntimeValueRegistry::engine()),
            realm_id: None,
            access_check: None,
        },
    )
    .expect("backend");

    let service = ProxyService::new(
        settings,
        &identifiers,
        Arc::new(MemoryProxyStore::new(true)),
        Arc::new(MemoryHeartbeatStore::new()),
        backend,
        EventBus::new(),
    );
    (Arc::new(service), allocator)
}

const CONFIG: &str = r#"
proxy:
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-timeout: 60000
"#;

/// A spec that runs the test fixture app.
fn testapp_spec(id: &str) -> ProxySpec {
    let container = ContainerSpec {
        image: SpelString::raw("sp-testapp".into()),
        cmd: SpelStringList::raw(vec!["sp-testapp".into()]),
        env: SpelStringMap::raw(BTreeMap::from([
            ("MY_APP_VAR".to_string(), "static".to_string()),
            ("USER_FROM_EXPRESSION".to_string(), "#{userId}".to_string()),
        ])),
        port_mapping: vec![PortMapping {
            name: "default".into(),
            port: Some(3838),
            target_path: SpelString::empty(),
        }],
        ..Default::default()
    };
    let mut spec = ProxySpec::new(id);
    spec.display_name = Some("Test App".into());
    spec.container_specs = vec![container];
    spec.set_container_index();
    spec
}

fn user() -> UserContext {
    UserContext::new("jack", vec!["SCIENTISTS".into()])
}

#[tokio::test]
async fn starts_an_app_and_makes_it_reachable() {
    let (service, allocator) = build_service(CONFIG, 22000);
    let mut events = service.events().subscribe();
    let spec = testapp_spec("01_hello");
    let user = user();

    let proxy = service
        .create_proxy(
            "proxy-1",
            &user,
            &spec,
            vec![RuntimeValue::string(&PUBLIC_PATH, "/app_proxy/proxy-1/")],
        )
        .expect("proxy is created");
    assert_eq!(proxy.status, ProxyStatus::New);
    assert_eq!(service.all_proxies().len(), 1);
    assert!(service.is_busy(), "starting an app makes the server busy");

    let proxy = service
        .start_proxy(proxy, &spec, &user)
        .await
        .expect("app starts");

    assert_eq!(proxy.status, ProxyStatus::Up);
    assert!(proxy.startup_timestamp > 0);
    assert_eq!(proxy.containers.len(), 1);
    assert!(
        proxy.containers[0].id.is_some(),
        "the process id is recorded"
    );

    // the app is reachable on its target
    let target = proxy.default_target().expect("target").to_string();
    assert!(target.starts_with("http://127.0.0.1:22000"), "{target}");
    let body = reqwest::get(format!("{target}/"))
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("sp-testapp"), "{body}");

    // the runtime values reached the app as environment variables
    let environment: BTreeMap<String, String> = reqwest::get(format!("{target}/env"))
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        environment.get("SHINYPROXY_USERNAME").map(String::as_str),
        Some("jack")
    );
    assert_eq!(
        environment.get("SHINYPROXY_USERGROUPS").map(String::as_str),
        Some("SCIENTISTS")
    );
    assert_eq!(
        environment
            .get("SHINYPROXY_PUBLIC_PATH")
            .map(String::as_str),
        Some("/app_proxy/proxy-1/")
    );
    assert_eq!(
        environment.get("MY_APP_VAR").map(String::as_str),
        Some("static")
    );
    assert_eq!(
        environment.get("USER_FROM_EXPRESSION").map(String::as_str),
        Some("jack"),
        "expressions in container-env are resolved"
    );
    // values that Java does not inject must not be present
    assert!(!environment.contains_key("SHINYPROXY_INSTANCE"));

    // the proxy carries the expected runtime values
    assert_eq!(
        proxy.runtime_value(&DISPLAY_NAME).as_deref(),
        Some("Test App")
    );
    assert_eq!(proxy.runtime_value(&USER_ID).as_deref(), Some("jack"));
    assert_eq!(
        proxy.runtime_value(&USER_GROUPS).as_deref(),
        Some("SCIENTISTS")
    );

    // a start event was published
    let event = events.recv().await.expect("event");
    assert_eq!(event.name(), "ProxyStart");

    // health checks pass while the app runs
    assert!(service.is_proxy_healthy(&proxy).await);

    // stopping releases the process and the port
    service
        .stop_proxy(&proxy, ProxyStopReason::ByUser)
        .await
        .expect("stops");
    assert!(service.all_proxies().is_empty());
    assert!(allocator.owned_ports("proxy-1").is_empty());
    assert!(
        reqwest::get(format!("{target}/")).await.is_err(),
        "the app process must be gone"
    );

    let event = events.recv().await.expect("event");
    assert_eq!(event.name(), "ProxyStop");
}

#[tokio::test]
async fn app_that_never_answers_fails_to_start_and_is_cleaned_up() {
    // `sleep` never listens on a port, so the readiness probe times out
    let (service, allocator) = build_service(
        "proxy:\n  container-backend: local\n  container-wait-timeout: 1000\n",
        22100,
    );
    let mut events = service.events().subscribe();

    let mut spec = testapp_spec("broken");
    spec.container_specs[0].cmd = SpelStringList::raw(vec!["sleep".into(), "30".into()]);

    let user = user();
    let proxy = service
        .create_proxy("proxy-broken", &user, &spec, vec![])
        .expect("proxy is created");
    let error = service
        .start_proxy(proxy, &spec, &user)
        .await
        .expect_err("must fail");
    assert!(
        error.to_string().contains("did not respond in time"),
        "{error}"
    );

    // everything is cleaned up
    assert!(service.all_proxies().is_empty());
    assert!(allocator.owned_ports("proxy-broken").is_empty());
    let event = events.recv().await.expect("event");
    assert_eq!(event.name(), "ProxyStartFailed");
}

#[tokio::test]
async fn respects_max_total_instances() {
    let (service, _allocator) = build_service(
        "proxy:\n  container-backend: local\n  container-wait-timeout: 15000\n  max-total-instances: 1\n",
        22200,
    );
    let spec = testapp_spec("01_hello");
    let user = user();

    let first = service
        .create_proxy("proxy-a", &user, &spec, vec![])
        .expect("first proxy");
    let error = service
        .create_proxy("proxy-b", &user, &spec, vec![])
        .expect_err("second proxy must be rejected");
    assert!(
        error.to_string().contains("does not have enough capacity"),
        "{error}"
    );

    // the same check exists per app
    let (service, _allocator) = build_service(CONFIG, 22300);
    let mut spec = testapp_spec("limited");
    spec.max_total_instances = 1;
    service
        .create_proxy("proxy-c", &user, &spec, vec![])
        .expect("first proxy");
    let error = service
        .create_proxy("proxy-d", &user, &spec, vec![])
        .expect_err("second proxy must be rejected");
    assert!(
        error.to_string().contains("does not have enough capacity"),
        "{error}"
    );

    drop(first);
}

#[tokio::test]
async fn stopping_a_starting_app_cleans_it_up() {
    let (service, allocator) = build_service(
        "proxy:\n  container-backend: local\n  container-wait-timeout: 3000\n",
        22400,
    );
    let spec = testapp_spec("01_hello");
    let user = user();

    let proxy = service
        .create_proxy("proxy-race", &user, &spec, vec![])
        .expect("proxy is created");

    // stop the proxy while it is starting: the store no longer has it in a startable state
    let service_for_stop = service.clone();
    let stopping = proxy.clone();
    let stop_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        service_for_stop
            .stop_proxy(&stopping, ProxyStopReason::ByUser)
            .await
            .expect("stops");
    });

    let result = service.start_proxy(proxy, &spec, &user).await;
    stop_task.await.expect("stop task");

    // either the start noticed the stop, or it finished before the stop removed it; in both cases
    // nothing is left behind
    if let Err(error) = &result {
        assert!(
            error.to_string().contains("stopped while starting")
                || error.to_string().contains("did not respond"),
            "{error}"
        );
    }
    assert!(service.all_proxies().is_empty());
    assert!(allocator.owned_ports("proxy-race").is_empty());
}

#[tokio::test]
async fn shutdown_stops_running_apps() {
    let (service, allocator) = build_service(CONFIG, 22500);
    let spec = testapp_spec("01_hello");
    let user = user();

    let proxy = service
        .create_proxy("proxy-shutdown", &user, &spec, vec![])
        .expect("proxy is created");
    let proxy = service
        .start_proxy(proxy, &spec, &user)
        .await
        .expect("app starts");
    let target = proxy.default_target().expect("target").to_string();

    service.shutdown().await;
    assert!(service.is_shutting_down());
    assert!(service.all_proxies().is_empty());
    assert!(allocator.owned_ports("proxy-shutdown").is_empty());
    assert!(reqwest::get(format!("{target}/")).await.is_err());
}

#[tokio::test]
async fn apps_are_left_running_when_configured() {
    let (service, allocator) = build_service(
        "proxy:\n  container-backend: local\n  container-wait-timeout: 15000\n  stop-proxies-on-shutdown: false\n",
        22600,
    );
    let spec = testapp_spec("01_hello");
    let user = user();

    let proxy = service
        .create_proxy("proxy-keep", &user, &spec, vec![])
        .expect("proxy is created");
    let proxy = service
        .start_proxy(proxy, &spec, &user)
        .await
        .expect("app starts");
    let target = proxy.default_target().expect("target").to_string();

    service.shutdown().await;
    assert_eq!(service.all_proxies().len(), 1, "the proxy is kept");
    assert!(
        reqwest::get(format!("{target}/")).await.is_ok(),
        "the app keeps running"
    );

    // clean up so that the test does not leak a process
    service
        .stop_proxy(&proxy, ProxyStopReason::Shutdown)
        .await
        .expect("stops");
    assert!(allocator.owned_ports("proxy-keep").is_empty());
}

#[tokio::test]
async fn user_proxies_are_tracked_per_user_and_app() {
    let (service, _allocator) = build_service(CONFIG, 22700);
    let spec_a = testapp_spec("app-a");
    let spec_b = testapp_spec("app-b");
    let jack = UserContext::new("jack", vec![]);
    let jeff = UserContext::new("jeff", vec![]);

    service
        .create_proxy("p1", &jack, &spec_a, vec![])
        .expect("p1");
    service
        .create_proxy("p2", &jack, &spec_b, vec![])
        .expect("p2");
    service
        .create_proxy("p3", &jeff, &spec_a, vec![])
        .expect("p3");

    assert_eq!(service.user_proxies("jack").len(), 2);
    assert_eq!(service.user_proxies("jeff").len(), 1);
    assert_eq!(service.user_proxies_by_spec("jack", "app-a").len(), 1);
    assert_eq!(service.user_proxies_by_spec("jack", "app-b").len(), 1);
    assert!(service.user_proxies_by_spec("jeff", "app-b").is_empty());
    assert_eq!(service.all_proxies().len(), 3);
    assert!(
        service.all_up_proxies().is_empty(),
        "none of them started yet"
    );
}

#[tokio::test]
async fn events_are_published_for_the_whole_lifecycle() {
    let (service, _allocator) = build_service(CONFIG, 22800);
    let mut events = service.events().subscribe();
    let spec = testapp_spec("01_hello");
    let user = user();

    let proxy = service
        .create_proxy("proxy-events", &user, &spec, vec![])
        .expect("proxy is created");
    let proxy = service
        .start_proxy(proxy, &spec, &user)
        .await
        .expect("app starts");
    service
        .stop_proxy(&proxy, ProxyStopReason::Inactivity)
        .await
        .expect("stops");

    let mut names = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let Event::ProxyStopped { reason, .. } = &event {
            assert_eq!(*reason, ProxyStopReason::Inactivity);
        }
        names.push(event.name());
    }
    assert_eq!(names, ["ProxyStart", "ProxyStop"]);
}
