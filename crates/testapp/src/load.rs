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

//! The load generator of `scripts/load-test.sh` and `scripts/benchmark.sh`.
//!
//! It logs in, starts an app, opens a number of WebSocket connections through the proxy and keeps them alive
//! while it hammers a path with a number of connections. At the end it reports the throughput, the latency
//! distribution and the number of errors, so the run can be compared with the Java implementation on the
//! same machine.
//!
//! Three targets can be measured (`--target`):
//!
//! * `app` — the reverse proxy path (`/app_proxy/{id}/`), which is what a user of an app exercises,
//! * `index` — a page the server renders itself (`/`), which measures the templating and the session layer,
//! * `api` — the JSON API (`/api/proxy`), which measures the store and the serialisation.
//!
//! `--measure-start-cycles N` measures something else entirely: how long the server needs to start an app and
//! to stop it again, N times, which is the number a user feels when opening an app.
//!
//! For the `index` and `api` targets every connection logs in as its own user session, because a servlet
//! container serialises the requests of *one* session (Undertow locks the session per request), which would
//! measure the session lock instead of the page. The `app` target necessarily shares the session of the user
//! that owns the app.

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
    /// What is measured: the app behind the proxy, a page of the server, or the API.
    pub target: Target,
    /// When set, the run measures how long starting and stopping an app takes, this many times.
    pub start_cycles: usize,
    /// Prints the report as `key: value` lines that a script can read.
    pub machine_readable: bool,
}

/// What the load is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The reverse proxy path of a running app.
    App,
    /// The index page, which the server renders itself.
    Index,
    /// The JSON API.
    Api,
    /// A 64 KB body streamed through the proxy (measures the data plane, not the routing).
    Big,
    /// A 64 KB body posted through the proxy (measures the request body path).
    Upload,
    /// WebSocket churn: connect, exchange one message, close (measures the handshake path).
    WsChurn,
}

impl Target {
    /// The target of a name (`app` by default).
    fn parse(value: &str) -> Target {
        match value.to_ascii_lowercase().as_str() {
            "index" | "page" => Target::Index,
            "api" => Target::Api,
            "big" | "download" => Target::Big,
            "upload" => Target::Upload,
            "ws-churn" | "wschurn" => Target::WsChurn,
            _ => Target::App,
        }
    }

    /// The name used in the report.
    fn name(self) -> &'static str {
        match self {
            Target::App => "app",
            Target::Index => "index",
            Target::Api => "api",
            Target::Big => "big",
            Target::Upload => "upload",
            Target::WsChurn => "ws_churn",
        }
    }
}

/// How large the bodies of the `big` and `upload` targets are.
pub const BODY_BYTES: usize = 64 * 1024;

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
            target: Target::parse(&value("--target", "app")),
            start_cycles: value("--measure-start-cycles", "0").parse().unwrap_or(0),
            machine_readable: args.iter().any(|arg| arg == "--machine-readable"),
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
/// A client that is logged in, together with its session cookie.
async fn logged_in_client(options: &Options) -> anyhow::Result<(reqwest::Client, String)> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let client = reqwest::Client::builder()
        .cookie_provider(jar.clone())
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(options.connections.max(1) * 2)
        .build()?;

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

    let cookie = {
        use reqwest::cookie::CookieStore;
        let url: reqwest::Url = options.base_url.parse()?;
        jar.cookies(&url)
            .and_then(|value| value.to_str().ok().map(str::to_string))
            .unwrap_or_default()
    };
    Ok((client, cookie))
}

pub async fn run(options: Options) -> anyhow::Result<()> {
    let (client, cookie) = logged_in_client(&options).await?;

    // how long the server needs to start an app and to stop it again
    if options.start_cycles > 0 {
        return measure_start_cycles(&client, &options).await;
    }

    // start the app and wait until it is up
    let (proxy_id, _) = start_app(&client, &options).await?;
    println!("app {proxy_id} is up, starting the load");

    let counters = Arc::new(Counters::default());
    let deadline = Instant::now() + options.duration;

    // WebSocket churn is its own loop: connect, one message, close
    if options.target == Target::WsChurn {
        return websocket_churn(&options, &proxy_id, &cookie, counters).await;
    }

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
    let target_url = match options.target {
        Target::App => format!("{}/app_proxy/{proxy_id}/", options.base_url),
        Target::Index => format!("{}/", options.base_url),
        Target::Api => format!("{}/api/proxy", options.base_url),
        Target::Big => format!("{}/app_proxy/{proxy_id}/big/{BODY_BYTES}", options.base_url),
        Target::Upload => format!("{}/app_proxy/{proxy_id}/upload", options.base_url),
        Target::WsChurn => unreachable!("handled above"),
    };
    let latencies = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let upload_body = bytes::Bytes::from(vec![b'x'; BODY_BYTES]);
    for connection in 0..options.connections {
        // a session of its own for the pages and the API (see the note at the top of this file)
        let client = match options.target {
            Target::App | Target::Big | Target::Upload | Target::WsChurn => client.clone(),
            Target::Index | Target::Api => {
                let (client, _) = logged_in_client(&options).await?;
                let _ = connection;
                client
            }
        };
        let upload = (options.target == Target::Upload).then(|| upload_body.clone());
        let url = target_url.clone();
        let counters = counters.clone();
        let latencies = latencies.clone();
        tasks.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                let started = Instant::now();
                let request = match &upload {
                    Some(body) => client.post(&url).body(body.clone()),
                    None => client.get(&url),
                };
                match request.send().await {
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
    println!("target: {}", options.target.name());
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
    // the lines a script reads (`scripts/benchmark.sh`)
    if options.machine_readable {
        println!(
            "METRIC requests_per_second {:.1}",
            requests as f64 / options.duration.as_secs_f64()
        );
        println!("METRIC errors {}", counters.errors.load(Ordering::Relaxed));
        println!("METRIC latency_p50_ms {:.2}", percentile(0.50));
        println!("METRIC latency_p90_ms {:.2}", percentile(0.90));
        println!("METRIC latency_p99_ms {:.2}", percentile(0.99));
        println!("METRIC latency_max_ms {:.2}", percentile(1.0));
        println!(
            "METRIC websocket_errors {}",
            counters.websocket_errors.load(Ordering::Relaxed)
        );
    }

    Ok(())
}

/// The app of the user that is already running, when there is one.
///
/// A benchmark run measures several phases one after the other; without this every phase would try to start
/// another app and run into `proxy.default-max-instances`.
async fn running_app(
    client: &reqwest::Client,
    options: &Options,
) -> anyhow::Result<Option<String>> {
    let proxies: serde_json::Value = client
        .get(format!("{}/api/proxy", options.base_url))
        .send()
        .await?
        .json()
        .await?;
    Ok(proxies["data"].as_array().and_then(|entries| {
        entries
            .iter()
            .find(|entry| {
                entry["specId"].as_str() == Some(options.spec.as_str())
                    && entry["status"].as_str() == Some("Up")
            })
            .and_then(|entry| entry["id"].as_str().map(str::to_string))
    }))
}

/// Starts an app and waits until it is up; returns its id and how long that took.
///
/// An app of this user that is already up is reused (the time is then zero).
async fn start_app(
    client: &reqwest::Client,
    options: &Options,
) -> anyhow::Result<(String, Duration)> {
    if let Some(proxy_id) = running_app(client, options).await? {
        return Ok((proxy_id, Duration::ZERO));
    }

    let started_at = Instant::now();
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
    Ok((proxy_id, started_at.elapsed()))
}

/// Stops an app and waits until it is gone; returns how long that took.
async fn stop_app(
    client: &reqwest::Client,
    options: &Options,
    proxy_id: &str,
) -> anyhow::Result<Duration> {
    let started_at = Instant::now();
    client
        .put(format!("{}/api/proxy/{proxy_id}/status", options.base_url))
        .header("Content-Type", "application/json")
        .body("{\"status\":\"Stopping\"}")
        .send()
        .await?;

    // wait until the app is no longer listed
    for _ in 0..600 {
        let proxies: serde_json::Value = client
            .get(format!("{}/api/proxy", options.base_url))
            .send()
            .await?
            .json()
            .await?;
        let listed = proxies["data"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["id"].as_str() == Some(proxy_id))
            })
            .unwrap_or(false);
        if !listed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(started_at.elapsed())
}

/// Measures how long the server needs to start an app and to stop it again.
async fn measure_start_cycles(client: &reqwest::Client, options: &Options) -> anyhow::Result<()> {
    // an app that is still running from an earlier phase would make the first cycle measure nothing
    if let Some(proxy_id) = running_app(client, options).await? {
        stop_app(client, options, &proxy_id).await?;
    }

    let mut start_times = Vec::new();
    let mut stop_times = Vec::new();
    for cycle in 1..=options.start_cycles {
        let (proxy_id, start) = start_app(client, options).await?;
        let stop = stop_app(client, options, &proxy_id).await?;
        println!(
            "  cycle {cycle}: start {:.0} ms, stop {:.0} ms",
            start.as_secs_f64() * 1000.0,
            stop.as_secs_f64() * 1000.0
        );
        start_times.push(start.as_secs_f64() * 1000.0);
        stop_times.push(stop.as_secs_f64() * 1000.0);
    }

    let median = |mut values: Vec<f64>| -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.sort_by(|left, right| left.partial_cmp(right).expect("no NaN"));
        values[values.len() / 2]
    };
    let start_median = median(start_times.clone());
    let stop_median = median(stop_times.clone());
    println!();
    println!("app_start_ms: median {start_median:.0}");
    println!("app_stop_ms: median {stop_median:.0}");
    if options.machine_readable {
        println!("METRIC app_start_ms {start_median:.1}");
        println!("METRIC app_stop_ms {stop_median:.1}");
    }
    Ok(())
}

/// Opens and closes WebSocket connections as fast as it can (the handshake path of the tunnel).
async fn websocket_churn(
    options: &Options,
    proxy_id: &str,
    cookie: &str,
    counters: Arc<Counters>,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + options.duration;
    let latencies = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let url = format!(
        "{}/app_proxy/{proxy_id}/ws",
        options.base_url.replace("http://", "ws://")
    );

    let mut tasks = Vec::new();
    for _ in 0..options.connections {
        let url = url.clone();
        let cookie = cookie.to_string();
        let counters = counters.clone();
        let latencies = latencies.clone();
        tasks.push(tokio::spawn(async move {
            use futures::{SinkExt, StreamExt};
            while Instant::now() < deadline {
                let started = Instant::now();
                let request = match tokio_tungstenite::tungstenite::http::Request::builder()
                    .uri(&url)
                    .header(
                        "Host",
                        url.trim_start_matches("ws://")
                            .split('/')
                            .next()
                            .unwrap_or("localhost"),
                    )
                    .header("Cookie", cookie.clone())
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
                        counters.errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                match tokio_tungstenite::connect_async(request).await {
                    Ok((mut socket, _)) => {
                        let round_trip = socket
                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                "churn".into(),
                            ))
                            .await
                            .is_ok()
                            && matches!(socket.next().await, Some(Ok(_)));
                        let _ = socket.close(None).await;
                        if round_trip {
                            counters.requests.fetch_add(1, Ordering::Relaxed);
                            latencies
                                .lock()
                                .expect("the latency list is not poisoned")
                                .push(started.elapsed().as_micros() as u64);
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
    for task in tasks {
        let _ = task.await;
    }

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
    let handshakes = counters.requests.load(Ordering::Relaxed);
    println!();
    println!("target: ws_churn");
    println!(
        "websocket_handshakes_per_second: {:.0}",
        handshakes as f64 / options.duration.as_secs_f64()
    );
    println!("errors: {}", counters.errors.load(Ordering::Relaxed));
    println!(
        "handshake_ms: p50 {:.1} p99 {:.1}",
        percentile(0.50),
        percentile(0.99)
    );
    if options.machine_readable {
        println!(
            "METRIC requests_per_second {:.1}",
            handshakes as f64 / options.duration.as_secs_f64()
        );
        println!("METRIC errors {}", counters.errors.load(Ordering::Relaxed));
        println!("METRIC latency_p50_ms {:.2}", percentile(0.50));
        println!("METRIC latency_p99_ms {:.2}", percentile(0.99));
    }
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
