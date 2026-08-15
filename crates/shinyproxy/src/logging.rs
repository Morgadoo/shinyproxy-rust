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

//! Logging setup (`LoggingConfigurer`, `logging.*` and `proxy.log-as-json`).
//!
//! * `logging.level.<logger>` sets levels, with `root` for the root logger, as in Spring Boot.
//! * `logging.file.name` writes the log to a file *in addition to* the console, like Spring Boot's file
//!   appender.
//! * `proxy.log-as-json` switches both to the JSON format of `logstash-logback-encoder`, so that log
//!   collectors configured for the Java implementation keep working: `@timestamp`, `@version`, `message`,
//!   `logger_name`, `thread_name`, `level` and `level_value`, plus the fields of the event.

use containerproxy::config::Settings;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Starts logging, according to the configuration.
///
/// Returns the guard of the file writer, which must be kept alive for as long as the process logs.
pub fn init(settings: &Settings) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = filter(settings);
    let json = settings.proxy.log_as_json();

    let file = settings
        .logging
        .file
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty());

    match file {
        Some(path) => {
            let (writer, guard) = file_writer(path);
            if json {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .event_format(LogstashFormat)
                            .with_writer(std::io::stdout),
                    )
                    .with(
                        tracing_subscriber::fmt::layer()
                            .event_format(LogstashFormat)
                            .with_writer(writer),
                    )
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .with_writer(writer),
                    )
                    .init();
            }
            Some(guard)
        }
        None => {
            if json {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .event_format(LogstashFormat)
                            .with_writer(std::io::stdout),
                    )
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
                    .init();
            }
            None
        }
    }
}

/// Opens the log file (`logging.file.name`), creating its directory when needed.
fn file_writer(
    path: &str,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
) {
    let path = std::path::Path::new(path);
    if let Some(directory) = path.parent() {
        if !directory.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(directory);
        }
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|error| {
            eprintln!("cannot open the log file {}: {error}", path.display());
            std::process::exit(1);
        });
    tracing_appender::non_blocking(file)
}

/// Builds the filter from `logging.level.*`, or from `RUST_LOG` when it is set.
///
/// The names of the loggers are the module paths of this implementation (`containerproxy::service`, ...);
/// `root` sets the level of everything, exactly like Spring Boot's `logging.level.root`.
pub fn filter(settings: &Settings) -> EnvFilter {
    // `RUST_LOG` wins, so that an operator can raise the level without touching the configuration
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return filter;
    }
    filter_from_settings(settings)
}

/// Builds the filter from `logging.level.*` only, ignoring the environment.
pub fn filter_from_settings(settings: &Settings) -> EnvFilter {
    let levels = &settings.logging.level;
    let root = levels
        .get("root")
        .map(|level| level.to_ascii_lowercase())
        .unwrap_or_else(|| "info".to_string());
    let mut filter = EnvFilter::new(normalise_level(&root));
    for (logger, level) in levels {
        if logger == "root" {
            continue;
        }
        let directive = format!("{}={}", logger.replace('.', "::"), normalise_level(level));
        match directive.parse() {
            Ok(directive) => filter = filter.add_directive(directive),
            Err(error) => eprintln!("ignoring logging.level.{logger}: {error}"),
        }
    }
    filter
}

/// Maps the Spring Boot level names onto the `tracing` names.
fn normalise_level(level: &str) -> &'static str {
    match level.trim().to_ascii_lowercase().as_str() {
        "off" | "none" => "off",
        "error" | "fatal" => "error",
        "warn" | "warning" => "warn",
        "debug" => "debug",
        "trace" | "all" => "trace",
        _ => "info",
    }
}

/// The numeric level of `logstash-logback-encoder` (the SLF4J values).
fn level_value(level: &tracing::Level) -> u32 {
    match *level {
        tracing::Level::ERROR => 40_000,
        tracing::Level::WARN => 30_000,
        tracing::Level::INFO => 20_000,
        tracing::Level::DEBUG => 10_000,
        tracing::Level::TRACE => 5_000,
    }
}

/// Formats events like `logstash-logback-encoder` does.
pub struct LogstashFormat;

impl<S, N> FormatEvent<S, N> for LogstashFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);

        let mut document = serde_json::Map::new();
        document.insert("@timestamp".into(), serde_json::json!(timestamp()));
        document.insert("@version".into(), serde_json::json!("1"));
        document.insert("message".into(), serde_json::json!(visitor.message));
        document.insert("logger_name".into(), serde_json::json!(metadata.target()));
        document.insert(
            "thread_name".into(),
            serde_json::json!(std::thread::current().name().unwrap_or("main").to_string()),
        );
        document.insert(
            "level".into(),
            serde_json::json!(metadata.level().to_string()),
        );
        document.insert(
            "level_value".into(),
            serde_json::json!(level_value(metadata.level())),
        );
        for (name, value) in visitor.fields {
            document.insert(name, value);
        }

        writeln!(writer, "{}", serde_json::Value::Object(document))
    }
}

/// Collects the message and the fields of an event.
#[derive(Default)]
struct JsonVisitor {
    message: String,
    fields: Vec<(String, serde_json::Value)>,
}

impl Visit for JsonVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .push((field.name().to_string(), serde_json::json!(value)));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .push((field.name().to_string(), serde_json::json!(value)));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .push((field.name().to_string(), serde_json::json!(value)));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .push((field.name().to_string(), serde_json::json!(value)));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields
                .push((field.name().to_string(), serde_json::json!(rendered)));
        }
    }
}

/// The current time in the ISO-8601 format the Logstash encoder uses.
fn timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let offset = now.offset();
    let sign = if offset.whole_hours() < 0 { '-' } else { '+' };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{sign}{:02}:{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond(),
        offset.whole_hours().abs(),
        offset.minutes_past_hour().abs(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    #[test]
    fn maps_spring_boot_levels() {
        assert_eq!(normalise_level("DEBUG"), "debug");
        assert_eq!(normalise_level("warn"), "warn");
        assert_eq!(normalise_level("WARNING"), "warn");
        assert_eq!(normalise_level("FATAL"), "error");
        assert_eq!(normalise_level("OFF"), "off");
        assert_eq!(normalise_level("nonsense"), "info");
    }

    #[test]
    fn builds_a_filter_from_the_configuration() {
        // the directives are visible in the string representation of the filter
        let filter = filter_from_settings(&settings(
            "logging:\n  level:\n    root: WARN\n    containerproxy.service: DEBUG\n",
        ));
        let rendered = filter.to_string();
        assert!(rendered.contains("warn"), "{rendered}");
        assert!(
            rendered.contains("containerproxy::service=debug"),
            "{rendered}"
        );
    }

    #[test]
    fn uses_the_logstash_level_values() {
        assert_eq!(level_value(&tracing::Level::ERROR), 40_000);
        assert_eq!(level_value(&tracing::Level::WARN), 30_000);
        assert_eq!(level_value(&tracing::Level::INFO), 20_000);
        assert_eq!(level_value(&tracing::Level::DEBUG), 10_000);
        assert_eq!(level_value(&tracing::Level::TRACE), 5_000);
    }

    #[test]
    fn formats_timestamps_like_the_logstash_encoder() {
        let timestamp = timestamp();
        // 2026-08-15T01:23:45.678+00:00
        assert_eq!(timestamp.len(), 29, "{timestamp}");
        assert_eq!(&timestamp[4..5], "-", "{timestamp}");
        assert_eq!(&timestamp[10..11], "T", "{timestamp}");
        assert_eq!(&timestamp[23..24], "+", "{timestamp}");
    }

    /// Captures what a subscriber writes.
    #[derive(Clone, Default)]
    struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Buffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().expect("lock").clone()).expect("utf-8")
        }
    }

    impl std::io::Write for Buffer {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn creates_the_log_file_and_its_directory() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("logs").join("shinyproxy.log");
        let (_writer, _guard) = file_writer(&path.display().to_string());
        assert!(path.is_file(), "the log file and its directory are created");
    }

    #[test]
    fn writes_json_events_like_the_logstash_encoder() {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(LogstashFormat)
                .with_writer(buffer.clone()),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(proxyId = "abc", "Proxy activated");
        });

        let line = buffer.contents();
        let document: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|error| panic!("{error}: {line}"));
        assert_eq!(document["@version"], "1");
        assert_eq!(document["message"], "Proxy activated");
        assert_eq!(document["level"], "INFO");
        assert_eq!(document["level_value"], 20_000);
        assert_eq!(document["logger_name"], "shinyproxy::logging::tests");
        assert_eq!(document["proxyId"], "abc", "fields become properties");
        assert!(document["@timestamp"].is_string());
        assert!(document["thread_name"].is_string());
    }
}
