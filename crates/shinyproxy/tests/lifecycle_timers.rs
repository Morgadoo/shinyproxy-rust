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

//! Releasing apps: heartbeat timeout, max lifetime and logout.
//!
//! Replaces the Java `TestIntegrationHeartbeat`/`ProxyMaxLifetimeService` tests. The intervals are
//! configured in milliseconds so that the tests stay fast.

mod common;

use std::time::Duration;

use common::{TestClient, TestInstance};

/// Starts an app and waits until it is up.
async fn start_app(instance: &TestInstance, client: &TestClient) -> String {
    let started: serde_json::Value = client
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"]
        .as_str()
        .expect("proxy id")
        .to_string();
    let status: serde_json::Value = client
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=15"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");
    proxy_id
}

/// The apps of a user, from the API.
async fn running_apps(instance: &TestInstance, client: &TestClient) -> usize {
    let proxies: serde_json::Value = client
        .get(instance.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    proxies["data"].as_array().map(Vec::len).unwrap_or_default()
}

/// Waits until the app is gone, or gives up after `attempts` times 200ms.
async fn wait_until_gone(instance: &TestInstance, client: &TestClient, attempts: u32) -> bool {
    for _ in 0..attempts {
        if running_apps(instance, client).await == 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test]
async fn inactive_apps_are_released() {
    // heartbeat rate 200ms (so the cleanup runs every 400ms) and a timeout of 600ms
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 200
  heartbeat-timeout: 600
  docker:
    port-range-start: 27000
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;

    // while the user keeps using the app it stays up
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let response = jack
            .get(instance.url(&format!("/app_proxy/{proxy_id}/")))
            .send()
            .await
            .expect("app request");
        assert_eq!(response.status(), 200);
    }
    assert_eq!(
        running_apps(&instance, &jack).await,
        1,
        "the app must stay up while it is used"
    );

    // once the user stops using it, the app is released
    assert!(
        wait_until_gone(&instance, &jack, 25).await,
        "the inactive app must be released"
    );

    instance.stop();
}

#[tokio::test]
async fn apps_without_heartbeats_are_kept_when_the_timeout_is_disabled() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 200
  heartbeat-timeout: -1
  docker:
    port-range-start: 27100
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;
    start_app(&instance, &jack).await;

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        running_apps(&instance, &jack).await,
        1,
        "a negative heartbeat timeout disables the check"
    );

    instance.stop();
}

#[tokio::test]
async fn apps_are_released_when_they_reach_their_max_lifetime() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 200
  heartbeat-timeout: -1
  docker:
    port-range-start: 27200
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      max-lifetime: 1
"##,
    )
    .await;
    let jack = instance.login("jack", "password").await;
    start_app(&instance, &jack).await;

    // the timer of the max lifetime check runs every five minutes, which is too slow for a test; the
    // check itself is called directly (the timer interval is asserted in the unit tests of the service)
    assert_eq!(running_apps(&instance, &jack).await, 1);
    instance.state.release.release_expired_proxies().await;
    assert_eq!(
        running_apps(&instance, &jack).await,
        1,
        "an app that is one minute old is not released yet"
    );

    // make the app look older than its lifetime of one minute
    let mut proxy = instance
        .state
        .store
        .all_proxies()
        .into_iter()
        .next()
        .expect("one proxy");
    proxy.startup_timestamp -= 61_000;
    instance.state.store.update_proxy(&proxy);

    instance.state.release.release_expired_proxies().await;
    assert!(
        wait_until_gone(&instance, &jack, 25).await,
        "the app must be released when it reached its max lifetime"
    );

    instance.stop();
}

#[tokio::test]
async fn logging_out_stops_the_apps_of_the_user() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  docker:
    port-range-start: 27300
  users:
    - name: jack
      password: password
    - name: jeff
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
"##,
    )
    .await;

    let jack = instance.login("jack", "password").await;
    start_app(&instance, &jack).await;
    let jeff = instance.login("jeff", "password").await;
    start_app(&instance, &jeff).await;
    assert_eq!(instance.state.store.count(), 2);

    // jack logs out, which stops their app but leaves jeff's app alone
    let response = jack
        .get(instance.url("/logout"))
        .send()
        .await
        .expect("logout request");
    assert_eq!(response.status(), 303);

    for _ in 0..25 {
        if instance.state.store.count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        instance.state.store.count(),
        1,
        "only the app of the user that logged out is stopped"
    );
    assert_eq!(running_apps(&instance, &jeff).await, 1);

    instance.stop();
}

#[tokio::test]
async fn apps_survive_a_logout_when_configured() {
    let instance = TestInstance::start(
        r##"
proxy:
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  default-stop-proxy-on-logout: false
  docker:
    port-range-start: 27400
  users:
    - name: jack
      password: password
  specs:
    - id: 01_hello
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
    - id: 02_always_stopped
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      stop-on-logout: true
"##,
    )
    .await;

    let jack = instance.login("jack", "password").await;
    start_app(&instance, &jack).await;

    // and one app that always stops on logout
    let started: serde_json::Value = jack
        .post(instance.url("/app_i/02_always_stopped/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let second_id = started["data"]["id"].as_str().expect("id").to_string();
    let status: serde_json::Value = jack
        .get(instance.url(&format!(
            "/api/proxy/{second_id}/status?watch=true&timeout=15"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");
    assert_eq!(instance.state.store.count(), 2);

    jack.get(instance.url("/logout"))
        .send()
        .await
        .expect("logout request");

    for _ in 0..25 {
        if instance.state.store.count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let remaining = instance.state.store.all_proxies();
    assert_eq!(remaining.len(), 1, "only the app with stop-on-logout stops");
    assert_eq!(remaining[0].spec_id.as_deref(), Some("01_hello"));

    instance.stop();
}
