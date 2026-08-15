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

//! OpenID Connect authentication, against a fake provider that runs in the test.
//!
//! The provider signs its id tokens with a generated RSA key and publishes the matching JWKS, so the whole
//! flow (authorization request, code exchange, id token verification, user info, group mapping) runs
//! exactly as it does against a real provider. Replaces the Java `TestOpenIdParseClaimRoles` and the
//! OpenID integration tests.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use common::TestInstance;

/// The state of the fake provider.
#[derive(Clone)]
struct Provider {
    /// The key the id tokens are signed with.
    encoding_key: Arc<jsonwebtoken::EncodingKey>,
    /// The public key in the JWKS format.
    jwks: Arc<serde_json::Value>,
    /// The claims the provider puts in its id tokens.
    claims: Arc<serde_json::Value>,
    /// The claims the user info endpoint answers with.
    userinfo: Arc<serde_json::Value>,
    /// The authorization requests the provider received.
    requests: Arc<std::sync::Mutex<Vec<HashMap<String, String>>>>,
}

/// Starts the fake provider and returns its address.
async fn start_provider(
    claims: serde_json::Value,
    userinfo: serde_json::Value,
) -> (String, Provider, tokio::task::JoinHandle<()>) {
    // a fresh RSA key per test run
    let key = rsa::RsaPrivateKey::new(&mut rand_old::thread_rng(), 2048).expect("rsa key");
    let public = rsa::RsaPublicKey::from(&key);
    let pem = rsa::pkcs1::EncodeRsaPrivateKey::to_pkcs1_pem(&key, rsa::pkcs8::LineEnding::LF)
        .expect("pem");
    let encoding_key =
        jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");

    use base64::Engine;
    use rsa::traits::PublicKeyParts;
    let modulus = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let exponent =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    let jwks = serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": "test-key",
            "n": modulus,
            "e": exponent,
        }]
    });

    let provider = Provider {
        encoding_key: Arc::new(encoding_key),
        jwks: Arc::new(jwks),
        claims: Arc::new(claims),
        userinfo: Arc::new(userinfo),
        requests: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address").to_string();

    let app = Router::new()
        .route(
            "/authorize",
            get(
                |State(provider): State<Provider>, Query(query): Query<HashMap<String, String>>| async move {
                    provider
                        .requests
                        .lock()
                        .expect("lock")
                        .push(query.clone());
                    // the provider sends the user back with a code and the state it received
                    let redirect_uri = query.get("redirect_uri").cloned().unwrap_or_default();
                    let state = query.get("state").cloned().unwrap_or_default();
                    axum::response::Redirect::to(&format!(
                        "{redirect_uri}?code=the-code&state={state}"
                    ))
                },
            ),
        )
        .route(
            "/token",
            post(
                |State(provider): State<Provider>, Form(form): Form<HashMap<String, String>>| async move {
                    if form.get("code").map(String::as_str) != Some("the-code") {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({"error": "invalid_grant"})),
                        );
                    }
                    let nonce = provider
                        .requests
                        .lock()
                        .expect("lock")
                        .last()
                        .and_then(|request| request.get("nonce").cloned())
                        .unwrap_or_default();

                    let mut claims = provider.claims.as_object().cloned().unwrap_or_default();
                    claims.insert("nonce".to_string(), serde_json::json!(nonce));
                    claims.insert("aud".to_string(), serde_json::json!("shinyproxy-client"));
                    claims.insert("iss".to_string(), serde_json::json!("https://idp.test"));
                    claims.insert(
                        "exp".to_string(),
                        serde_json::json!(
                            (std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("time")
                                .as_secs()
                                + 600) as i64
                        ),
                    );

                    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
                    header.kid = Some("test-key".to_string());
                    let id_token = jsonwebtoken::encode(
                        &header,
                        &serde_json::Value::Object(claims),
                        &provider.encoding_key,
                    )
                    .expect("id token");

                    (
                        axum::http::StatusCode::OK,
                        Json(serde_json::json!({
                            "access_token": "the-access-token",
                            "refresh_token": "the-refresh-token",
                            "token_type": "Bearer",
                            "expires_in": 3600,
                            "id_token": id_token,
                        })),
                    )
                },
            ),
        )
        .route(
            "/jwks",
            get(|State(provider): State<Provider>| async move {
                Json((*provider.jwks).clone())
            }),
        )
        .route(
            "/userinfo",
            get(|State(provider): State<Provider>| async move {
                Json((*provider.userinfo).clone())
            }),
        )
        .with_state(provider.clone());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (address, provider, handle)
}

/// Starts the flow and follows the redirect to the provider (the test client does not follow redirects).
async fn follow_to_provider(client: &common::TestClient, instance: &TestInstance) {
    let response = client
        .get(instance.url("/oauth2/authorization/shinyproxy"))
        .send()
        .await
        .expect("request");
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("redirect to the provider")
        .to_string();
    client
        .get(&location)
        .send()
        .await
        .expect("provider request");
}

/// The configuration of a ShinyProxy that uses the fake provider.
fn config(address: &str, extra: &str) -> String {
    format!(
        r##"
proxy:
  title: OpenID Test
  authentication: openid
  admin-groups: admins
  container-backend: local
  openid:
    auth-url: http://{address}/authorize
    token-url: http://{address}/token
    jwks-url: http://{address}/jwks
    userinfo-url: http://{address}/userinfo
    client-id: shinyproxy-client
    client-secret: shinyproxy-secret
    roles-claim: groups
{extra}
  specs:
    - id: 01_hello
      display-name: Hello Application
      container-image: sp-testapp
      container-cmd: [ "sp-testapp" ]
      access-groups: scientists
    - id: 02_admin_only
      display-name: Admin Application
      container-image: sp-testapp
      access-groups: admins
"##
    )
}

#[tokio::test]
async fn the_whole_login_flow() {
    let (address, provider, provider_task) = start_provider(
        serde_json::json!({"email": "jack@example.com", "sub": "jack", "groups": ["scientists"]}),
        serde_json::json!({"email": "jack@example.com", "name": "Jack", "groups": ["ROLE_users"]}),
    )
    .await;

    let instance = TestInstance::start(&config(&address, "")).await;
    let client = instance.client();

    // an unauthenticated request goes to the login page, which redirects to the provider
    let response = client
        .get(instance.url("/"))
        .header("accept", "text/html")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );

    let response = client
        .get(instance.url("/login"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/oauth2/authorization/shinyproxy")
    );

    // starting the flow sends the user to the provider with the expected parameters
    let response = client
        .get(instance.url("/oauth2/authorization/shinyproxy"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 303);
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("redirect")
        .to_string();
    assert!(
        location.starts_with(&format!("http://{address}/authorize?")),
        "{location}"
    );
    assert!(
        location.contains("client_id=shinyproxy-client"),
        "{location}"
    );
    assert!(location.contains("scope=openid%20email"), "{location}");
    assert!(location.contains("state="), "{location}");
    assert!(location.contains("nonce="), "{location}");

    // the browser follows the redirect to the provider, which sends it back to the callback; the test
    // does the two steps itself so that the session cookie of the client is used all the way through
    let response = client
        .get(&location)
        .send()
        .await
        .expect("provider request");
    assert_eq!(response.status(), 303, "the provider redirects back");

    let request = provider
        .requests
        .lock()
        .expect("lock")
        .last()
        .cloned()
        .expect("the provider received the authorization request");
    let redirect_uri = request.get("redirect_uri").cloned().expect("redirect uri");
    let state = request.get("state").cloned().expect("state");
    assert!(
        redirect_uri.ends_with("/login/oauth2/code/shinyproxy"),
        "{redirect_uri}"
    );

    let response = client
        .get(format!("{redirect_uri}?code=the-code&state={state}"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(response.status(), 303);
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("redirect")
        .to_string();
    assert!(
        location.starts_with("/auth-success?continue="),
        "{location}"
    );

    // the user is logged in, with the groups of the id token and of the user info
    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Hello Application"), "{body}");
    assert!(!body.contains("Admin Application"), "{body}");
    assert!(body.contains("jack@example.com"), "{body}");

    // an app receives the access token of the user
    let started: serde_json::Value = client
        .post(instance.url("/app_i/01_hello/_"))
        .send()
        .await
        .expect("start request")
        .json()
        .await
        .expect("json");
    let proxy_id = started["data"]["id"].as_str().expect("id").to_string();
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

    let environment: std::collections::BTreeMap<String, String> = client
        .get(instance.url(&format!("/app_proxy/{proxy_id}/env")))
        .send()
        .await
        .expect("env request")
        .json()
        .await
        .expect("json");
    assert_eq!(
        environment
            .get("SHINYPROXY_OIDC_ACCESS_TOKEN")
            .map(String::as_str),
        Some("the-access-token")
    );
    assert_eq!(
        environment.get("SHINYPROXY_USERNAME").map(String::as_str),
        Some("jack@example.com")
    );
    assert_eq!(
        environment.get("SHINYPROXY_USERGROUPS").map(String::as_str),
        Some("SCIENTISTS,USERS")
    );

    instance.stop();
    provider_task.abort();
}

#[tokio::test]
async fn a_wrong_state_is_refused() {
    let (address, _provider, provider_task) = start_provider(
        serde_json::json!({"email": "jack@example.com", "sub": "jack"}),
        serde_json::json!({}),
    )
    .await;
    let instance = TestInstance::start(&config(&address, "")).await;
    let client = instance.client();

    // start the flow so that the session has a request
    client
        .get(instance.url("/oauth2/authorization/shinyproxy"))
        .send()
        .await
        .expect("request");

    let response = client
        .get(instance.url("/login/oauth2/code/shinyproxy?code=the-code&state=not-the-state"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/auth-error")
    );

    // a callback without a request in the session ends up on the error page as well
    let fresh = instance.client();
    let response = fresh
        .get(instance.url("/login/oauth2/code/shinyproxy?code=the-code&state=whatever"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/auth-error")
    );

    instance.stop();
    provider_task.abort();
}

#[tokio::test]
async fn roles_claims_of_every_shape_are_understood() {
    // a provider that puts a JSON list in a string claim, as some providers do (#25549)
    let (address, provider, provider_task) = start_provider(
        serde_json::json!({
            "email": "root@example.com",
            "sub": "root",
            "groups": "[\"admins\", \"scientists\"]"
        }),
        serde_json::json!({}),
    )
    .await;

    let instance = TestInstance::start(&config(&address, "")).await;
    let client = instance.client();
    follow_to_provider(&client, &instance).await;
    let request = provider
        .requests
        .lock()
        .expect("lock")
        .last()
        .cloned()
        .expect("request");
    let redirect_uri = request.get("redirect_uri").cloned().expect("redirect uri");
    let state = request.get("state").cloned().expect("state");
    client
        .get(format!("{redirect_uri}?code=the-code&state={state}"))
        .send()
        .await
        .expect("callback request");

    // the user is an administrator, so the admin app and the admin page are available
    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("Admin Application"), "{body}");
    assert!(body.contains("href=\"/admin\""), "{body}");

    instance.stop();
    provider_task.abort();
}

#[tokio::test]
async fn pkce_is_used_when_configured() {
    let (address, provider, provider_task) = start_provider(
        serde_json::json!({"email": "jack@example.com", "sub": "jack", "groups": ["scientists"]}),
        serde_json::json!({}),
    )
    .await;
    let instance = TestInstance::start(&config(&address, "    with-pkce: true\n")).await;
    let client = instance.client();

    follow_to_provider(&client, &instance).await;
    let request = provider
        .requests
        .lock()
        .expect("lock")
        .last()
        .cloned()
        .expect("request");
    assert!(
        request.contains_key("code_challenge"),
        "the authorization request must carry the challenge: {request:?}"
    );
    assert_eq!(
        request.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );

    // the flow still completes (the verifier is sent to the token endpoint)
    let redirect_uri = request.get("redirect_uri").cloned().expect("redirect uri");
    let state = request.get("state").cloned().expect("state");
    let response = client
        .get(format!("{redirect_uri}?code=the-code&state={state}"))
        .send()
        .await
        .expect("callback request");
    assert_eq!(response.status(), 303);
    let body = client
        .get(instance.url("/"))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    assert!(body.contains("jack@example.com"), "{body}");

    instance.stop();
    provider_task.abort();
}
