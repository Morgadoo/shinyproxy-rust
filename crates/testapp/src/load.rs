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

//! The load generator of `scripts/load-test.sh`.
//!
//! It logs in, starts an app, opens a number of WebSocket connections through the proxy and keeps them alive
//! while it hammers the HTTP path of the same app with a number of connections. At the end it reports the
//! throughput, the latency distribution and the number of errors, so the run can be compared with the Java
//! implementation on the same machine.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What to run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Where ShinyProxy serves.
    pub base_url: String,
    /// The user that starts the app.
    pub username: String,
    pub password: String,
    /// The app definition to start.
    pub spec: String,
    /// How many WebSocket connections are held open.
    pub websockets: usize,
    /// How many connections hammer the HTTP path.
    pub connections: usize,
    /// How long the load runs.
    pub duration: Duration,
}

impl Options {
    /// Reads the options from the command line.
    pub fn from_args(args: &[String]) -> Options {
        let value = |name: &str, fallback: &str| -> String {
            args.iter()
                .position(|arg| arg == name)
                .and_then(|index| args.get(index + 1))
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        Options {
            base_url: value("--base-url", "http://127.0.0.1:8080"),
            username: value("--username", "jack"),
            password: value("--password", "password"),
            spec: value("--spec", "01_hello"),
            websockets: value("--websockets", "200").parse().unwrap_or(200),
            connections: value("--connections", "32").parse().unwrap_or(32),
            duration: Duration::from_secs(value("--seconds", "60").parse().unwrap_or(60)),
        }
    }
}

/// The counters of a run.
#[derive(Debug, Default)]
struct Counters {
    requests: AtomicU64,
    errors: AtomicU64,
    websocket_messages: AtomicU64,
    websocket_errors: AtomicU64,
}

/// Runs the load and prints the report.
pub async fn run(options: Options) -> anyhow::Result<()> {
    // the cookie jar is kept, because the WebSocket client needs the session cookie
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let client = reqwest::Client::builder()
        .cookie_provider(jar.clone())
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(options.connections * 2)
        .build()?;

    // log in
    let login = client
        .get(format!("{}/login", options.base_url))
        .send()
        .await?
        .text()
        .await?;
    let token = login
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string();
    client
        .post(format!("{}/login", options.base_url))
        .form(&[
            ("username", options.username.as_str()),
            ("password", options.password.as_str()),
            ("_csrf", token.as_str()),
        ])
        .send()
        .await?;

    // start the app and wait until it is up
    let started: serde_json::Value = client
        .post(format!("{}/app_i/{}/_", options.base_url, options.spec))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await?
        .json()
        .await?;
    let proxy_id = started["data"]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the app did not start: {started}"))?
        .to_string();
    let status: serde_json::Value = client
        .get(format!(
            "{}/api/proxy/{proxy_id}/status?watch=true&timeout=60",
            options.base_url
        ))
        .send()
        .await?
        .json()
        .await?;
    if status["data"]["status"] != "Up" {
        anyhow::bail!("the app did not become available: {status}");
    }
    println!("app {proxy_id} is up, starting the load");

    let cookie = {
        use reqwest::cookie::CookieStore;
        let url: reqwest::Url = options.base_url.parse()?;
        jar.cookies(&url)
            .and_then(|value| value.to_str().ok().map(str::to_string))
            .unwrap_or_default()
    };
    let counters = Arc::new(Counters::default());
    let deadline = Instant::now() + options.duration;

    // the WebSocket connections, which the proxy keeps alive with its heartbeat pings
    let mut tasks = Vec::new();
    for _ in 0..options.websockets {
        let url = format!(
            "{}/app_proxy/{proxy_id}/ws",
            options.base_url.replace("http://", "ws://")
        );
        let counters = counters.clone();
        let cookie = cookie.clone();
        tasks.push(tokio::spawn(async move {
            websocket_session(url, cookie, deadline, counters).await;
        }));
    }

    // the HTTP load, with a latency sample per request
    let latencies = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    for _ in 0..options.connections {
        let client = client.clone();
        let url = format!("{}/app_proxy/{proxy_id}/", options.base_url);
        let counters = counters.clone();
        let latencies = latencies.clone();
        tasks.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let started = Instant::now();
                match client.get(&url).send().await {
                    Ok(response) => {
                        let ok = response.status().is_success();
                        // the body has to be read, otherwise the connection cannot be reused
                        let _ = response.bytes().await;
                        if ok {
                            counters.requests.fetch_add(1, Ordering::Relaxed);
                            let micros = started.elapsed().as_micros() as u64;
                            latencies
                                .lock()
                                .expect("the latency list is not poisoned")
                                .push(micros);
                        } else {
                            counters.errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        counters.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // progress, so a long run shows that it is alive
    let progress = counters.clone();
    let ticker = tokio::spawn(async move {
        let mut previous = 0;
        let mut timer = tokio::time::interval(Duration::from_secs(10));
        timer.tick().await;
        while Instant::now() < deadline {
            timer.tick().await;
            let now = progress.requests.load(Ordering::Relaxed);
            println!(
                "  {:>8} requests total ({:>6}/s), {} errors, {} websocket messages",
                now,
                (now - previous) / 10,
                progress.errors.load(Ordering::Relaxed),
                progress.websocket_messages.load(Ordering::Relaxed)
            );
            previous = now;
        }
    });

    for task in tasks {
        let _ = task.await;
    }
    ticker.abort();

    let mut samples = latencies
        .lock()
        .expect("the latency list is not poisoned")
        .clone();
    samples.sort_unstable();
    let percentile = |fraction: f64| -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let index = ((samples.len() as f64 - 1.0) * fraction).round() as usize;
        samples[index] as f64 / 1000.0
    };

    let requests = counters.requests.load(Ordering::Relaxed);
    println!();
    println!("requests: {requests}");
    println!(
        "requests_per_second: {:.0}",
        requests as f64 / options.duration.as_secs_f64()
    );
    println!("errors: {}", counters.errors.load(Ordering::Relaxed));
    println!(
        "websockets: {} open, {} messages, {} errors",
        options.websockets,
        counters.websocket_messages.load(Ordering::Relaxed),
        counters.websocket_errors.load(Ordering::Relaxed)
    );
    println!(
        "latency_ms: p50 {:.1} p90 {:.1} p99 {:.1} max {:.1}",
        percentile(0.50),
        percentile(0.90),
        percentile(0.99),
        percentile(1.0)
    );

    Ok(())
}

/// Holds one WebSocket connection open, echoing a message every second.
async fn websocket_session(
    url: String,
    cookie: String,
    deadline: Instant,
    counters: Arc<Counters>,
) {
    use futures::{SinkExt, StreamExt};

    let request = match tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&url)
        .header(
            "Host",
            url.trim_start_matches("ws://")
                .split('/')
                .next()
                .unwrap_or("localhost"),
        )
        .header("Cookie", cookie)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
    {
        Ok(request) => request,
        Err(_) => {
            counters.websocket_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    let mut socket = match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => socket,
        Err(_) => {
            counters.websocket_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    while Instant::now() < deadline {
        if socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "load".to_string().into(),
            ))
            .await
            .is_err()
        {
            counters.websocket_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // the answer of the app, or a ping of the proxy
        match socket.next().await {
            Some(Ok(_)) => {
                counters.websocket_messages.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                counters.websocket_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = socket.close(None).await;
}
