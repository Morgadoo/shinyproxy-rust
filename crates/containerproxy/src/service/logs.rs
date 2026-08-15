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

//! Collecting the logs of the containers (`LogService`, `FileLogStorage`).
//!
//! With `proxy.container-log-path` set, the output of every app is written to two files in that
//! directory, named exactly like the Java implementation does:
//!
//! ```text
//! {specId}_{proxyId}_{dd_MMM_yyyy_kk_mm_ss}_stdout.log
//! {specId}_{proxyId}_{dd_MMM_yyyy_kk_mm_ss}_stderr.log
//! ```
//!
//! The service follows the events of the server: it attaches when an app starts or resumes, and detaches
//! when it stops or pauses.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::backend::ContainerBackend;
use crate::events::{Event, EventBus};
use crate::model::proxy::Proxy;

/// Where the logs of one app are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPaths {
    /// File with the standard output.
    pub stdout: PathBuf,
    /// File with the standard error.
    pub stderr: PathBuf,
}

/// Writes the output of the apps to files.
#[derive(Debug)]
pub struct LogService {
    /// Directory the logs are written to (`proxy.container-log-path`).
    directory: Option<PathBuf>,
    /// The paths of every app, so that the same files are used for the lifetime of the app.
    paths: DashMap<String, LogPaths>,
    /// The task that copies the output of an app, so that it can be stopped.
    tasks: DashMap<String, tokio::task::JoinHandle<()>>,
}

impl LogService {
    /// Creates the service from the configuration.
    pub fn new(settings: &crate::config::Settings) -> Self {
        LogService {
            directory: settings
                .proxy
                .container_log_path
                .as_deref()
                .filter(|path| !path.trim().is_empty())
                .map(PathBuf::from),
            paths: DashMap::new(),
            tasks: DashMap::new(),
        }
    }

    /// Whether the logs of the containers are collected.
    pub fn is_enabled(&self) -> bool {
        self.directory.is_some()
    }

    /// Creates the log directory, as `FileLogStorage.initialize` does.
    pub fn initialize(&self) {
        let Some(directory) = &self.directory else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(directory) {
            tracing::error!(
                "Failed to initialize container log storage ({}): {error}",
                directory.display()
            );
        }
    }

    /// The log files of an app; the names are fixed the first time they are asked for.
    pub fn log_paths(&self, proxy: &Proxy) -> Option<LogPaths> {
        let directory = self.directory.clone()?;
        if let Some(paths) = self.paths.get(&proxy.id) {
            return Some(paths.clone());
        }
        let timestamp = timestamp();
        let base = format!(
            "{}_{}_{timestamp}",
            proxy.spec_id.clone().unwrap_or_default(),
            proxy.id
        );
        let paths = LogPaths {
            stdout: directory.join(format!("{base}_stdout.log")),
            stderr: directory.join(format!("{base}_stderr.log")),
        };
        self.paths.insert(proxy.id.clone(), paths.clone());
        Some(paths)
    }

    /// Starts writing the output of an app to its files.
    pub async fn attach(&self, proxy: &Proxy, backend: &Arc<dyn ContainerBackend>) {
        let Some(paths) = self.log_paths(proxy) else {
            return;
        };
        if self.tasks.contains_key(&proxy.id) {
            return; // already attached
        }

        let stream = match backend.container_logs(proxy, true).await {
            Ok(Some(stream)) => stream,
            Ok(None) => {
                tracing::warn!(
                    "Failed to attach logging of proxy: no output streams defined [proxyId: {}]",
                    proxy.id
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to attach logging of proxy [proxyId: {}]: {error}",
                    proxy.id
                );
                return;
            }
        };

        let proxy_id = proxy.id.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = write_stream(stream, paths).await {
                tracing::warn!("Error while writing the logs of proxy {proxy_id}: {error}");
            }
        });
        self.tasks.insert(proxy.id.clone(), task);
        tracing::info!("Container logging enabled [proxyId: {}]", proxy.id);
    }

    /// Stops writing the output of an app.
    pub fn detach(&self, proxy_id: &str) {
        if let Some((_, task)) = self.tasks.remove(proxy_id) {
            task.abort();
            tracing::debug!("Container logging stopped [proxyId: {proxy_id}]");
        }
        self.paths.remove(proxy_id);
    }

    /// Follows the events of the server: attach on start/resume, detach on stop/pause.
    pub fn subscribe(
        self: &Arc<Self>,
        events: &EventBus,
        backend: Arc<dyn ContainerBackend>,
        leader: Arc<dyn crate::service::LeaderService>,
    ) {
        if !self.is_enabled() {
            return;
        }
        let service = self.clone();
        let mut receiver = events.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = receiver.recv().await {
                match &event {
                    // only the leader collects the logs of a container (as in Java)
                    Event::ProxyStarted { proxy, .. } | Event::ProxyResumed { proxy }
                        if leader.is_leader() =>
                    {
                        service.attach(proxy, &backend).await;
                    }
                    Event::ProxyStopped { proxy, .. } | Event::ProxyPaused { proxy } => {
                        service.detach(&proxy.id);
                    }
                    _ => {}
                }
            }
        });
    }
}

/// Writes a log stream to the two files.
async fn write_stream(
    mut stream: crate::backend::LogStream,
    paths: LogPaths,
) -> std::io::Result<()> {
    let mut stdout = tokio::fs::File::create(&paths.stdout).await?;
    let mut stderr = tokio::fs::File::create(&paths.stderr).await?;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                tracing::debug!("container log stream ended: {error}");
                break;
            }
        };
        let file = if chunk.stderr {
            &mut stderr
        } else {
            &mut stdout
        };
        file.write_all(&chunk.data).await?;
        // apps write their logs slowly, so every chunk is flushed to make `tail -f` work
        file.flush().await?;
    }

    stdout.flush().await?;
    stderr.flush().await?;
    Ok(())
}

/// The timestamp in the file names, in the format of Java's `dd_MMM_yyyy_kk_mm_ss`.
///
/// `kk` is the hour of the day from 1 to 24, so midnight is written as 24 (a quirk of the Java format
/// that is reproduced here, because the file names must match).
fn timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let hour = if now.hour() == 0 { 24 } else { now.hour() };
    format!(
        "{:02}_{}_{}_{hour:02}_{:02}_{:02}",
        now.day(),
        month_abbreviation(now.month()),
        now.year(),
        now.minute(),
        now.second(),
    )
}

/// The English month abbreviation Java's `MMM` produces.
fn month_abbreviation(month: time::Month) -> &'static str {
    match month {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::ProxyStatus;

    fn settings(yaml: &str) -> crate::config::Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    fn proxy() -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".to_string());
        proxy
    }

    #[test]
    fn is_disabled_without_a_log_path() {
        let service = LogService::new(&settings("proxy:\n  authentication: none\n"));
        assert!(!service.is_enabled());
        assert_eq!(service.log_paths(&proxy()), None);
    }

    #[test]
    fn names_the_files_like_java() {
        let directory = tempfile::tempdir().expect("temp dir");
        let service = LogService::new(&settings(&format!(
            "proxy:\n  container-log-path: {}\n",
            directory.path().join("logs").display()
        )));
        assert!(service.is_enabled());
        service.initialize();
        assert!(
            directory.path().join("logs").is_dir(),
            "the directory is created"
        );

        let paths = service.log_paths(&proxy()).expect("paths");
        let stdout = paths
            .stdout
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let stderr = paths
            .stderr
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(stdout.starts_with("01_hello_proxy-1_"), "{stdout}");
        assert!(stdout.ends_with("_stdout.log"), "{stdout}");
        assert!(stderr.ends_with("_stderr.log"), "{stderr}");
        // dd_MMM_yyyy_kk_mm_ss: 01_Jan_2026_13_05_09
        let middle = stdout
            .trim_start_matches("01_hello_proxy-1_")
            .trim_end_matches("_stdout.log")
            .to_string();
        let parts: Vec<&str> = middle.split('_').collect();
        assert_eq!(parts.len(), 6, "timestamp parts of '{middle}'");
        assert_eq!(parts[0].len(), 2, "day of '{middle}'");
        assert_eq!(parts[2].len(), 4, "year of '{middle}'");

        // the same app keeps the same files
        assert_eq!(service.log_paths(&proxy()), Some(paths));
    }

    #[tokio::test]
    async fn writes_the_output_of_a_stream() {
        let directory = tempfile::tempdir().expect("temp dir");
        let paths = LogPaths {
            stdout: directory.path().join("out.log"),
            stderr: directory.path().join("err.log"),
        };
        let chunks = vec![
            Ok(crate::backend::LogChunk {
                stderr: false,
                data: b"hello ".to_vec(),
            }),
            Ok(crate::backend::LogChunk {
                stderr: false,
                data: b"world\n".to_vec(),
            }),
            Ok(crate::backend::LogChunk {
                stderr: true,
                data: b"oops\n".to_vec(),
            }),
        ];
        let stream: crate::backend::LogStream = Box::pin(futures::stream::iter(chunks));
        write_stream(stream, paths.clone()).await.expect("writes");

        assert_eq!(
            std::fs::read_to_string(&paths.stdout).expect("stdout"),
            "hello world\n"
        );
        assert_eq!(
            std::fs::read_to_string(&paths.stderr).expect("stderr"),
            "oops\n"
        );
    }
}
