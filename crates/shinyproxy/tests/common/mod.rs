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

//! Test harness: runs a ShinyProxy server in-process on an ephemeral port.

#![allow(dead_code)]

use std::sync::Arc;

use containerproxy::config::LoadOptions;
use shinyproxy::web::AppState;
use tempfile::TempDir;

/// A running ShinyProxy instance.
pub struct TestInstance {
    /// Base URL of the server, e.g. `http://127.0.0.1:34567`.
    pub base_url: String,
    /// The server state (useful for assertions on the configuration).
    pub state: Arc<AppState>,
    _directory: TempDir,
    handle: tokio::task::JoinHandle<()>,
}

/// A port range that no other test instance uses.
///
/// The base is derived from the process id (test binaries run in parallel) and a counter (tests within a
/// binary run in parallel as well).
fn unique_port_range() -> (u16, u16) {
    // the kernel hands out a free port, which becomes the first port of the range; asking it is more
    // reliable than a fixed mapping, because several test binaries run at the same time
    let mut ports = Vec::new();
    for _ in 0..PORTS_PER_INSTANCE {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        ports.push(listener.local_addr().expect("address").port());
    }
    // the listeners are closed here, so the ports are free again for the apps of this instance
    let start = *ports.iter().min().expect("one port");
    let end = *ports.iter().max().expect("one port");
    (start, end)
}

/// How many host ports one test instance may publish.
const PORTS_PER_INSTANCE: usize = 8;

/// Sends the log output of the server to the test output, so that `cargo test -- --nocapture` (and the
/// output of a failing test) shows what the server logged. Enable with `RUST_LOG=info`.
fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

impl TestInstance {
    /// Starts a server with the given configuration.
    pub async fn start(yaml: &str) -> TestInstance {
        init_logging();
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("application.yml");
        std::fs::write(&path, yaml).expect("write configuration");

        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (raw, mut settings) = shinyproxy::load_config(options).expect("configuration loads");
        // the test environment has no container runtime: apps run as local processes
        if settings.proxy.container_backend.is_none() {
            settings.proxy.container_backend = Some("local".to_string());
        }
        // every instance gets its own port range, so that tests (which run in parallel, in several test
        // binaries) never hand out the same host port twice
        let (range_start, range_max) = unique_port_range();
        settings.proxy.docker.port_range_start =
            Some(containerproxy::config::FlexI64(range_start as i64));
        settings.proxy.docker.port_range_max =
            Some(containerproxy::config::FlexI64(range_max as i64));
        let state = Arc::new(AppState::new(raw, settings).expect("state"));
        // the same startup sequence as `main`: recovery runs before requests are served
        state
            .spawn_startup_tasks()
            .await
            .expect("startup tasks finish");
        let app = shinyproxy::web::server::build(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                eprintln!("test server stopped: {error}");
            }
        });

        TestInstance {
            base_url: format!("http://{address}"),
            state,
            _directory: directory,
            handle,
        }
    }

    /// A client that does not follow redirects and keeps cookies.
    ///
    /// The cookie jar is kept alongside the client so that tests can read the session cookie back,
    /// which is needed to open a WebSocket connection (the WebSocket client is a different library).
    pub fn client(&self) -> TestClient {
        let jar = Arc::new(reqwest::cookie::Jar::default());
        let client = reqwest::Client::builder()
            .cookie_provider(jar.clone())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        TestClient {
            client,
            jar,
            base_url: self.base_url.clone(),
        }
    }

    /// A client that follows redirects and keeps cookies.
    pub fn following_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("client")
    }

    /// Full URL of a path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Logs a user in and returns the client that holds the session.
    pub async fn login(&self, username: &str, password: &str) -> TestClient {
        let client = self.client();
        let token = self.csrf_token(&client).await;
        let response = client
            .post(self.url("/login"))
            .form(&[
                ("username", username),
                ("password", password),
                ("_csrf", token.as_str()),
            ])
            .send()
            .await
            .expect("login request");
        assert_eq!(response.status(), 303, "login must redirect");
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            location.contains("auth-success"),
            "login must succeed, got redirect to {location}"
        );
        client
    }

    /// The session cookie of a client, formatted for a `Cookie` header.
    pub fn session_cookie(&self, client: &TestClient) -> Option<String> {
        client.cookie_header()
    }

    /// Reads the CSRF token from the login page.
    pub async fn csrf_token(&self, client: &TestClient) -> String {
        let body = client
            .get(self.url("/login"))
            .send()
            .await
            .expect("login page")
            .text()
            .await
            .expect("body");
        extract_csrf_token(&body).expect("login page must contain a csrf token")
    }

    /// Stops the server.
    pub fn stop(self) {
        self.handle.abort();
    }
}

/// A client with an accessible cookie jar.
pub struct TestClient {
    client: reqwest::Client,
    jar: Arc<reqwest::cookie::Jar>,
    base_url: String,
}

impl TestClient {
    /// The value for a `Cookie` header carrying the current session.
    pub fn cookie_header(&self) -> Option<String> {
        use reqwest::cookie::CookieStore;
        let url = self.base_url.parse().expect("base url");
        self.jar
            .cookies(&url)
            .and_then(|value| value.to_str().ok().map(str::to_string))
    }
}

impl std::ops::Deref for TestClient {
    type Target = reqwest::Client;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

/// Extracts the CSRF token from a rendered login page.
pub fn extract_csrf_token(html: &str) -> Option<String> {
    let marker = "name=\"_csrf\" value=\"";
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
