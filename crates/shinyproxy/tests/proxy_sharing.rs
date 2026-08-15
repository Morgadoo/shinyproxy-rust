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

//! End-to-end tests of the pre-initialized, shared containers (`minimum-seats-available`).
//!
//! The apps run with the `local` backend, so the containers are real processes of the `sp-testapp` fixture:
//! the scaler starts them before anybody logs in, the users claim their seats, and the containers are
//! recycled or removed as the configuration says.

mod common;

use std::time::Duration;

use common::{TestClient, TestInstance};
use containerproxy::service::DelegateProxyStatus;

/// A configuration with one shared app definition.
fn config(sharing: &str) -> String {
    format!(
        r##"
proxy:
  title: Sharing Test
  authentication: simple
  admin-groups: admins
  container-backend: local
  container-wait-timeout: 20000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  seat-wait-time: 6000
  users:
    - name: jack
      password: password
      groups: scientists
    - name: jeff
      password: password
      groups: scientists
    - name: root
      password: rootpw
      groups: admins
  specs:
    - id: 01_hello
      display-name: Shared Application
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: [ scientists, admins ]
      max-instances: 2
{sharing}
"##
    )
}

/// Waits until the scaler created the seats of the app definition.
async fn wait_for_seats(instance: &TestInstance, expected: i64) {
    for _ in 0..200 {
        let scaler = &instance.state.sharing_scalers[0];
        if scaler.seats().unclaimed_count() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let scaler = &instance.state.sharing_scalers[0];
    panic!(
        "the scaler did not create {expected} seats (unclaimed: {}, delegate proxies: {})",
        scaler.seats().unclaimed_count(),
        scaler.delegate_proxies().len()
    );
}

/// Starts the app for a user and returns its proxy id.
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
        .unwrap_or_else(|| panic!("the app must start: {started}"))
        .to_string();
    let status: serde_json::Value = client
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=20"
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

#[tokio::test]
async fn users_share_pre_initialized_containers() {
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 2\n      seats-per-container: 1\n",
    ))
    .await;

    // the containers exist before anybody logged in
    wait_for_seats(&instance, 2).await;
    let scaler = instance.state.sharing_scalers[0].clone();
    assert_eq!(scaler.delegate_proxies().len(), 2);
    assert!(scaler
        .delegate_proxies()
        .iter()
        .all(|delegate| delegate.status == DelegateProxyStatus::Available));
    assert_eq!(scaler.seats().count(), 2);
    assert_eq!(scaler.seats().claimed_count(), 0);

    // starting the app claims a seat instead of starting a container
    let jack = instance.login("jack", "password").await;
    let started_at = std::time::Instant::now();
    let proxy_id = start_app(&instance, &jack).await;
    assert!(
        started_at.elapsed() < Duration::from_secs(5),
        "claiming a seat must be quick, took {:?}",
        started_at.elapsed()
    );
    assert_eq!(scaler.seats().claimed_count(), 1);

    // the app of the user points at the container of the delegate proxy
    let proxy = instance.state.proxies.proxy(&proxy_id).expect("the proxy");
    let delegate = scaler
        .delegate_proxies()
        .into_iter()
        .find(|delegate| Some(delegate.proxy.id.clone()) == proxy.target_id)
        .expect("the app uses one of the pre-initialized containers");
    assert_eq!(proxy.targets, delegate.proxy.targets);
    assert_eq!(
        proxy
            .runtime_values
            .value_string(&containerproxy::model::runtime_value::PUBLIC_PATH),
        delegate
            .proxy
            .runtime_values
            .value_string(&containerproxy::model::runtime_value::PUBLIC_PATH)
    );
    assert!(proxy
        .runtime_values
        .value_string(&containerproxy::model::runtime_value::SEAT_ID)
        .is_some());

    // the browser reaches the app through the public path of the delegate proxy (`/api/route/{id}/`),
    // which is what the dispatcher copies into the proxy of the user
    let public_path = proxy
        .runtime_values
        .value_string(&containerproxy::model::runtime_value::PUBLIC_PATH)
        .expect("the public path");
    assert_eq!(
        public_path,
        format!("/api/route/{}/", delegate.proxy.id),
        "the app of the user is served under the path of the container"
    );
    let body = jack
        .get(instance.url(&public_path))
        .send()
        .await
        .expect("app request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("sp-testapp"), "{body}");

    // a second user gets the other seat, pointing at the other container
    let jeff = instance.login("jeff", "password").await;
    let other_id = start_app(&instance, &jeff).await;
    let other = instance.state.proxies.proxy(&other_id).expect("the proxy");
    assert_ne!(
        other.target_id, proxy.target_id,
        "the two users are on different containers"
    );
    assert_eq!(scaler.seats().claimed_count(), 2);

    // the scaler creates a new container, because the seats ran out
    wait_for_seats(&instance, 1).await;
    assert!(scaler.delegate_proxies().len() >= 3);

    // when a user stops their app the seat is handed back and the container is re-used
    let response = jack
        .put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    for _ in 0..100 {
        if scaler.seats().claimed_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(scaler.seats().claimed_count(), 1);
    assert!(
        scaler
            .delegate_proxies()
            .iter()
            .any(|delegate| Some(delegate.proxy.id.clone()) == proxy.target_id),
        "the container of the first user stays for the next one"
    );

    // and the seat can be claimed again by another user
    let again = start_app(&instance, &jack).await;
    let again = instance.state.proxies.proxy(&again).expect("the proxy");
    assert!(again.target_id.is_some());

    instance.stop();
}

#[tokio::test]
async fn several_users_share_one_container() {
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 2\n      seats-per-container: 2\n",
    ))
    .await;

    // one container serves two seats, so a single container is enough
    wait_for_seats(&instance, 2).await;
    let scaler = instance.state.sharing_scalers[0].clone();
    assert_eq!(scaler.delegate_proxies().len(), 1);
    assert_eq!(scaler.seats().count(), 2);

    let jack = instance.login("jack", "password").await;
    let jeff = instance.login("jeff", "password").await;
    let first = start_app(&instance, &jack).await;
    let second = start_app(&instance, &jeff).await;

    let first = instance.state.proxies.proxy(&first).expect("the proxy");
    let second = instance.state.proxies.proxy(&second).expect("the proxy");
    assert_eq!(
        first.target_id, second.target_id,
        "both users are served by the same container"
    );
    assert_eq!(scaler.seats().claimed_count(), 2);

    instance.stop();
}

#[tokio::test]
async fn a_container_that_may_not_be_re_used_is_removed_after_use() {
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 1\n      seats-per-container: 1\n      \
         allow-container-re-use: false\n",
    ))
    .await;

    wait_for_seats(&instance, 1).await;
    let scaler = instance.state.sharing_scalers[0].clone();
    let container_of_the_first_user = scaler.delegate_proxies()[0].proxy.id.clone();

    let jack = instance.login("jack", "password").await;
    let proxy_id = start_app(&instance, &jack).await;
    jack.put(instance.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");

    // the container of the user is marked for removal instead of being handed to the next user
    let mut marked = false;
    for _ in 0..100 {
        let delegates = scaler.delegate_proxies();
        let container = delegates
            .iter()
            .find(|delegate| delegate.proxy.id == container_of_the_first_user);
        match container {
            Some(delegate) if delegate.status == DelegateProxyStatus::ToRemove => {
                marked = true;
                break;
            }
            // it may already be gone, which is the same outcome
            None => {
                marked = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        marked,
        "the container must not be re-used: {:?}",
        scaler.delegate_proxies()
    );

    // and a fresh container is created for the next user
    wait_for_seats(&instance, 1).await;
    let seat = scaler
        .seats()
        .claim_seat("next-user")
        .expect("a seat of a fresh container");
    assert_ne!(seat.delegate_proxy_id, container_of_the_first_user);

    instance.stop();
}

#[tokio::test]
async fn a_user_waits_when_no_seat_is_available() {
    // one seat only: the second user has to wait for it
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 1\n      seats-per-container: 1\n",
    ))
    .await;

    wait_for_seats(&instance, 1).await;
    let jack = instance.login("jack", "password").await;
    let jeff = instance.login("jeff", "password").await;

    let first = start_app(&instance, &jack).await;
    assert!(!first.is_empty());

    // the second user has to wait; `seat-wait-time` is 6 seconds in this configuration, so the start fails
    let started: serde_json::Value = jeff
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"].as_str().expect("id").to_string();
    let status: serde_json::Value = jeff
        .get(instance.url(&format!(
            "/api/proxy/{proxy_id}/status?watch=true&timeout=20"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        status["data"]["status"], "Stopped",
        "the app of the second user must fail without a seat: {status}"
    );

    instance.stop();
}

#[tokio::test]
async fn an_administrator_removes_the_pre_initialized_containers() {
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 2\n      seats-per-container: 1\n",
    ))
    .await;

    wait_for_seats(&instance, 2).await;
    let scaler = instance.state.sharing_scalers[0].clone();
    assert_eq!(scaler.delegate_proxies().len(), 2);

    // a normal user may not remove them
    let jack = instance.login("jack", "password").await;
    let response = jack
        .delete(instance.url("/admin/delegate-proxy"))
        .header("Accept", "application/json")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);

    // an administrator may
    let root = instance.login("root", "rootpw").await;
    let response = root
        .delete(instance.url("/admin/delegate-proxy"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("json");
    assert_eq!(body["status"], "success");

    let removed: Vec<String> = scaler
        .delegate_proxies()
        .iter()
        .filter(|delegate| delegate.status == DelegateProxyStatus::ToRemove)
        .map(|delegate| delegate.proxy.id.clone())
        .collect();

    // the containers are marked and disappear as soon as the scaler is stable again (its timers run every
    // ten and twenty seconds; the test drives them so it does not have to wait for them)
    for _ in 0..40 {
        let current: Vec<String> = scaler
            .delegate_proxies()
            .iter()
            .map(|delegate| delegate.proxy.id.clone())
            .collect();
        if removed.iter().all(|id| !current.contains(id)) {
            break;
        }
        scaler.reconcile().await;
        scaler.cleanup().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let current: Vec<String> = scaler
        .delegate_proxies()
        .iter()
        .map(|delegate| delegate.proxy.id.clone())
        .collect();
    assert!(
        removed.iter().all(|id| !current.contains(id)),
        "the pre-initialized containers must be replaced: {removed:?} vs {current:?}"
    );

    instance.stop();
}

#[tokio::test]
async fn the_seats_are_reported_as_metrics() {
    let instance = TestInstance::start(&config(
        "      minimum-seats-available: 2\n      seats-per-container: 1\n",
    ))
    .await;
    wait_for_seats(&instance, 2).await;

    let body = instance
        .client()
        .get(instance.url("/actuator/prometheus"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let instance_id = instance.state.identifiers.instance_id.clone();
    let unclaimed = format!(
        "seats_unclaimed{{shinyproxy_instance=\"{instance_id}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 2"
    );
    assert!(body.contains(&unclaimed), "{body}");
    let claimed = format!(
        "seats_claimed{{shinyproxy_instance=\"{instance_id}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 0"
    );
    assert!(body.contains(&claimed), "{body}");

    // after a user claimed a seat the numbers move
    let jack = instance.login("jack", "password").await;
    start_app(&instance, &jack).await;
    let body = instance
        .client()
        .get(instance.url("/actuator/prometheus"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let claimed = format!(
        "seats_claimed{{shinyproxy_instance=\"{instance_id}\",shinyproxy_realm=\"\",spec_id=\"01_hello\"}} 1"
    );
    assert!(body.contains(&claimed), "{body}");

    instance.stop();
}

#[tokio::test]
async fn recovery_and_pre_initialized_containers_are_incompatible() {
    // the Java implementation refuses this combination at startup
    let error = common::start_and_expect_error(&format!(
        "{}  recover-running-proxies: true\n",
        config("      minimum-seats-available: 1\n")
    ))
    .await;
    assert!(error.contains("pre-initialized containers"), "{error}");
    assert!(error.contains("recover-running-proxies"), "{error}");

    // and a container that cannot be re-used must serve exactly one user
    let error = common::start_and_expect_error(&config(
        "      minimum-seats-available: 1\n      allow-container-re-use: false\n      \
         seats-per-container: 2\n",
    ))
    .await;
    assert!(
        error.contains("allow-container-re-use is disabled"),
        "{error}"
    );
}
