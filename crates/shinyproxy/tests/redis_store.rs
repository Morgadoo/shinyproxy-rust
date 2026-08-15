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

//! Two servers that share their state through Redis (`proxy.store-mode: Redis`).
//!
//! The test needs a Redis (`redis-server --port 6379`, or `SP_TEST_REDIS_URL`) and is skipped unless
//! `SP_TEST_REDIS=1` is set. Replaces the Java `TestRedisStore`/HA integration tests.

mod common;

use common::{TestClient, TestInstance};
use containerproxy::service::LeaderService;

/// Whether the Redis tests are enabled.
fn enabled() -> bool {
    std::env::var("SP_TEST_REDIS").as_deref() == Ok("1")
}

/// The configuration of one server of the realm.
fn config(realm: &str) -> String {
    format!(
        r##"
proxy:
  title: HA Test
  authentication: simple
  container-backend: local
  container-wait-timeout: 15000
  heartbeat-rate: 5000
  heartbeat-timeout: -1
  store-mode: Redis
  realm-id: {realm}
  users:
    - name: jack
      password: password
      groups: scientists
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: scientists
      max-instances: 3
"##
    )
}

/// Removes the keys of a realm, so that a test starts from an empty store.
fn clear_realm(realm: &str) {
    let url =
        std::env::var("SP_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let client = redis::Client::open(url).expect("client");
    let mut connection = client.get_connection().expect("connection");
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("shinyproxy_{realm}*"))
        .query(&mut connection)
        .unwrap_or_default();
    for key in keys {
        let _: Result<(), _> = redis::cmd("DEL").arg(key).query(&mut connection);
    }
}

/// Starts an app on one server and waits until it is up.
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

#[tokio::test]
async fn two_servers_share_their_apps() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 (and run a Redis) to run the Redis tests");
        return;
    }

    let realm = format!("test-{}", std::process::id());
    clear_realm(&realm);

    let first = TestInstance::start(&config(&realm)).await;
    let second = TestInstance::start(&config(&realm)).await;

    // an app started on the first server
    let jack_on_first = first.login("jack", "password").await;
    let proxy_id = start_app(&first, &jack_on_first).await;

    // ... is visible on the second server, with the same id, user and status
    let jack_on_second = second.login("jack", "password").await;
    let proxies: serde_json::Value = jack_on_second
        .get(second.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    let entries = proxies["data"].as_array().expect("array");
    assert_eq!(entries.len(), 1, "the app is shared: {proxies}");
    assert_eq!(entries[0]["id"], proxy_id.as_str());
    assert_eq!(entries[0]["userId"], "jack");
    assert_eq!(entries[0]["status"], "Up");
    assert_eq!(entries[0]["specId"], "01_hello");

    // the runtime values survive the round trip through Redis
    assert_eq!(entries[0]["runtimeValues"]["SHINYPROXY_APP_INSTANCE"], "_");
    assert!(entries[0]["runtimeValues"]["SHINYPROXY_PUBLIC_PATH"]
        .as_str()
        .is_some_and(|path| path.starts_with("/app_proxy/")));

    // the second server can also read the status endpoint of the app
    let status: serde_json::Value = jack_on_second
        .get(second.url(&format!("/api/proxy/{proxy_id}/status")))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up");

    // and stopping it on the second server removes it everywhere
    let response = jack_on_second
        .put(second.url(&format!("/api/proxy/{proxy_id}/status")))
        .json(&serde_json::json!({"status": "Stopping"}))
        .send()
        .await
        .expect("stop request");
    assert_eq!(response.status(), 200);

    for _ in 0..25 {
        let proxies: serde_json::Value = jack_on_first
            .get(first.url("/api/proxy"))
            .send()
            .await
            .expect("api request")
            .json()
            .await
            .expect("json");
        if proxies["data"].as_array().map(Vec::len) == Some(0) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let proxies: serde_json::Value = jack_on_first
        .get(first.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        proxies["data"].as_array().map(Vec::len),
        Some(0),
        "the app must be gone on both servers: {proxies}"
    );

    first.stop();
    second.stop();
    clear_realm(&realm);
}

#[tokio::test]
async fn two_servers_never_publish_the_same_port() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 to run the Redis tests");
        return;
    }

    let realm = format!("ports-{}", std::process::id());
    clear_realm(&realm);

    // both servers publish from the same range, which is the point of the shared allocator
    let range = (24800, 24809);
    let first = TestInstance::start_sharing_ports(&config(&realm), range).await;
    let second = TestInstance::start_sharing_ports(&config(&realm), range).await;

    // with the Redis registry they never hand out the same port
    let jack_on_first = first.login("jack", "password").await;
    let jack_on_second = second.login("jack", "password").await;
    let first_id = start_app(&first, &jack_on_first).await;

    // the second app is started through the other server (a second instance of the same app)
    let started: serde_json::Value = jack_on_second
        .post(second.url("/app_i/01_hello/second"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let second_id = started["data"]["id"].as_str().expect("id").to_string();
    let status: serde_json::Value = jack_on_second
        .get(second.url(&format!(
            "/api/proxy/{second_id}/status?watch=true&timeout=15"
        )))
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("json");
    assert_eq!(status["data"]["status"], "Up", "{status}");

    // the two apps listen on different host ports (the targets are part of the stored proxies)
    let target_of = |proxies: &serde_json::Value, id: &str| -> String {
        proxies["data"]
            .as_array()
            .expect("array")
            .iter()
            .find(|proxy| proxy["id"] == id)
            .and_then(|proxy| proxy["runtimeValues"]["SHINYPROXY_PUBLIC_PATH"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    let proxies: serde_json::Value = jack_on_first
        .get(first.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(proxies["data"].as_array().map(Vec::len), Some(2));
    assert_ne!(
        target_of(&proxies, &first_id),
        target_of(&proxies, &second_id)
    );

    // the ports of both servers are in the shared hash, under their proxy ids
    let url =
        std::env::var("SP_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let client = redis::Client::open(url).expect("client");
    let mut connection = client.get_connection().expect("connection");
    let entries: std::collections::BTreeMap<String, String> = redis::cmd("HGETALL")
        .arg(format!("shinyproxy_{realm}__ports"))
        .query(&mut connection)
        .expect("ports");
    assert!(entries.contains_key(&first_id), "{entries:?}");
    assert!(entries.contains_key(&second_id), "{entries:?}");
    let ports: Vec<u16> = entries
        .values()
        .flat_map(|value| serde_json::from_str::<Vec<u16>>(value).unwrap_or_default())
        .collect();
    assert_eq!(ports.len(), 2, "one port per app: {entries:?}");
    assert_ne!(ports[0], ports[1], "the ports must differ: {entries:?}");

    first.stop();
    second.stop();
    clear_realm(&realm);
}

#[tokio::test]
async fn heartbeats_are_shared() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 to run the Redis tests");
        return;
    }

    let realm = format!("heartbeat-{}", std::process::id());
    clear_realm(&realm);

    let first = TestInstance::start(&config(&realm)).await;
    let second = TestInstance::start(&config(&realm)).await;

    let jack = first.login("jack", "password").await;
    let proxy_id = start_app(&first, &jack).await;

    // the heartbeat of the second server is visible to the first one
    let jack_on_second = second.login("jack", "password").await;
    let response = jack_on_second
        .post(second.url(&format!("/heartbeat/{proxy_id}")))
        .send()
        .await
        .expect("heartbeat request");
    assert_eq!(response.status(), 200);

    let info: serde_json::Value = jack
        .get(first.url(&format!("/heartbeat/{proxy_id}")))
        .send()
        .await
        .expect("heartbeat request")
        .json()
        .await
        .expect("json");
    assert!(
        info["data"]["lastHeartbeat"].as_i64().unwrap_or_default() > 0,
        "the heartbeat is shared: {info}"
    );

    // the store of the realm is only used by this realm
    let other_realm = format!("other-{}", std::process::id());
    clear_realm(&other_realm);
    let third = TestInstance::start(&config(&other_realm)).await;
    let jack_on_third = third.login("jack", "password").await;
    let proxies: serde_json::Value = jack_on_third
        .get(third.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        proxies["data"].as_array().map(Vec::len),
        Some(0),
        "another realm has its own store: {proxies}"
    );

    first.stop();
    second.stop();
    third.stop();
    clear_realm(&realm);
    clear_realm(&other_realm);
}

#[tokio::test]
async fn only_one_server_of_a_realm_is_the_leader() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 to run the Redis tests");
        return;
    }

    let realm = format!("leader-{}", std::process::id());
    clear_realm(&realm);

    let first = TestInstance::start(&config(&realm)).await;
    let second = TestInstance::start(&config(&realm)).await;

    // exactly one of the two servers holds the lock
    let leaders = [&first, &second]
        .iter()
        .filter(|instance| instance.state.leader.is_leader())
        .count();
    assert_eq!(leaders, 1, "exactly one server must be the leader");

    // the lock holds the runtime id of that server
    let url =
        std::env::var("SP_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let client = redis::Client::open(url).expect("client");
    let mut connection = client.get_connection().expect("connection");
    let holder: Option<String> = redis::cmd("GET")
        .arg(format!("shinyproxy_{realm}__leader"))
        .query(&mut connection)
        .expect("lock");
    let holder = holder.expect("the lock is taken");
    let expected = if first.state.leader.is_leader() {
        first.state.identifiers.runtime_id.clone()
    } else {
        second.state.identifiers.runtime_id.clone()
    };
    assert_eq!(holder, expected);

    // the lock expires, so a server that disappears does not block the realm forever
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("shinyproxy_{realm}__leader"))
        .query(&mut connection)
        .expect("ttl");
    assert!(ttl > 0 && ttl <= 30, "the lock must expire: {ttl}");

    // a single server (no Redis) is always the leader
    let alone = TestInstance::start(
        r##"
proxy:
  authentication: none
  container-backend: local
  specs: []
"##,
    )
    .await;
    assert!(alone.state.leader.is_leader());

    first.stop();
    second.stop();
    alone.stop();
    clear_realm(&realm);
}

#[tokio::test]
async fn two_servers_share_their_sessions() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 (and run a Redis) to run the Redis tests");
        return;
    }

    let realm = format!("session-{}", std::process::id());
    clear_realm(&realm);

    // `spring.session.store-type: redis` puts the sessions in Redis, so a user that logged in on one
    // server is known by the other one (and the cookie is named SESSION, like Spring Session's default)
    let configuration = format!(
        "{}\nspring:\n  session:\n    store-type: redis\n    timeout: 30m\n",
        config(&realm)
    );
    let first = TestInstance::start(&configuration).await;
    let second = TestInstance::start(&configuration).await;

    let jack = first.login("jack", "password").await;
    let cookie = first.session_cookie(&jack).expect("session cookie");
    assert!(
        cookie.starts_with("SESSION="),
        "Redis sessions use the SESSION cookie: {cookie}"
    );

    // the same session works on the second server: its index page shows the user, no login redirect
    let response = jack
        .get(second.url("/"))
        .send()
        .await
        .expect("index request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("Hello Application"),
        "the second server must accept the session: {body}"
    );
    assert!(body.contains("href=\"/logout\""), "{body}");

    // an app started through the second server belongs to the same user
    let proxy_id = start_app(&second, &jack).await;
    let proxies: serde_json::Value = jack
        .get(first.url("/api/proxy"))
        .send()
        .await
        .expect("api request")
        .json()
        .await
        .expect("json");
    let entries = proxies["data"].as_array().expect("array");
    assert_eq!(entries.len(), 1, "{proxies}");
    assert_eq!(entries[0]["id"], proxy_id.as_str());
    assert_eq!(entries[0]["userId"], "jack");

    // the session keys live in the namespace of the realm, like RedisSessionConfig builds it
    let store = first
        .state
        .session_store
        .clone()
        .expect("the session store is Redis backed");
    assert!(
        store
            .namespace()
            .starts_with(&format!("shinyproxy__{realm}__")),
        "unexpected namespace: {}",
        store.namespace()
    );
    assert!(store.namespace().ends_with("spring:session:sessions"));

    // and the session service counts the user of the realm (this is what feeds the gauges)
    let (logged_in, active) = store
        .count_users(std::time::Duration::from_secs(60))
        .await
        .expect("counts");
    assert_eq!(logged_in, 1, "one user is logged in");
    assert_eq!(active, 1, "and used the session just now");

    // signing out on one server ends the session on both
    let response = jack.get(first.url("/logout")).send().await.expect("logout");
    assert_eq!(response.status(), 303);
    let response = jack
        .get(second.url("/api/proxy"))
        .header("Accept", "application/json")
        .send()
        .await
        .expect("api request");
    assert_eq!(
        response.status(),
        401,
        "the session must be gone on the second server too"
    );

    first.stop();
    second.stop();
    clear_realm(&realm);
}

#[tokio::test]
async fn a_rolling_update_moves_the_leadership_to_the_new_configuration() {
    if !enabled() {
        eprintln!("skipping: set SP_TEST_REDIS=1 (and run a Redis) to run the Redis tests");
        return;
    }

    let realm = format!("version-{}", std::process::id());
    clear_realm(&realm);

    // `proxy.version` tells the servers of a realm which configuration is the newest one; only the
    // servers that run it take part in the leader election (`RedisCheckLatestConfigService`)
    let old = TestInstance::start(&format!("{}  version: 1\n", config(&realm))).await;
    let old_service = old
        .state
        .latest_config
        .clone()
        .expect("the version check is active");
    assert!(old_service.is_latest());
    assert!(
        old.state
            .redis_leader
            .as_ref()
            .expect("election")
            .is_leader(),
        "the only server of the realm is the leader"
    );

    // a server of the new configuration appears; it is the latest one and takes part in the election
    let new = TestInstance::start(&format!("{}  version: 2\n", config(&realm))).await;
    let new_service = new
        .state
        .latest_config
        .clone()
        .expect("the version check is active");
    assert!(new_service.is_latest());
    assert!(new
        .state
        .redis_leader
        .as_ref()
        .expect("election")
        .participating());

    // the old server notices on its next check and stops taking part; because it is the leader it waits
    // before releasing the lock, so that the other servers of its version notice as well
    assert!(
        !old_service.check(),
        "the old server is no longer the latest"
    );
    assert!(!old_service.is_latest());
    assert!(
        !old_service.check(),
        "a server that lost never becomes the latest again"
    );

    // and a server that starts with an old configuration never takes part at all
    let late = TestInstance::start(&format!("{}  version: 1\n", config(&realm))).await;
    let late_service = late
        .state
        .latest_config
        .clone()
        .expect("the version check is active");
    assert!(!late_service.is_latest());
    let late_election = late.state.redis_leader.as_ref().expect("election");
    assert!(!late_election.participating());
    assert!(!late_election.is_leader());
    assert!(!late_election.elect(), "it must not take the lock");

    old.stop();
    new.stop();
    late.stop();
    clear_realm(&realm);
}
