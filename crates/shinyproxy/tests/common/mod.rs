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

impl TestInstance {
    /// Starts a server with the given configuration.
    pub async fn start(yaml: &str) -> TestInstance {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("application.yml");
        std::fs::write(&path, yaml).expect("write configuration");

        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (raw, settings) = shinyproxy::load_config(options).expect("configuration loads");
        let state = Arc::new(AppState::new(raw, settings).expect("state"));
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
    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client")
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
    pub async fn login(&self, username: &str, password: &str) -> reqwest::Client {
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

    /// Reads the CSRF token from the login page.
    pub async fn csrf_token(&self, client: &reqwest::Client) -> String {
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

/// Extracts the CSRF token from a rendered login page.
pub fn extract_csrf_token(html: &str) -> Option<String> {
    let marker = "name=\"_csrf\" value=\"";
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
