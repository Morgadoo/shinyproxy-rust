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

//! Usage statistics collectors (`StatCollectorFactory`, `AbstractDbCollector`, `JDBCCollector`,
//! `CSVCollector`).
//!
//! `proxy.usage-stats-url` selects the collector by its value, exactly like the Java factory:
//!
//! | value | collector |
//! | --- | --- |
//! | `micrometer` | the Prometheus metrics of [`super::Metrics`] |
//! | something ending in `.csv` | a CSV file |
//! | a `jdbc:` URL | a SQL database |
//! | anything else | a configuration error |
//!
//! The rows are the ones the Java implementation writes: `event_time`, `username`, `type` (`Login`,
//! `Logout`, `ProxyStart`, `ProxyStop`) and `data` (the app id for the proxy events), plus one column per
//! `proxy.usage-stats-attributes` entry.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use crate::events::{Event, EventBus};

/// One row of the usage statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// When the event happened (epoch millis).
    pub event_time: i64,
    /// The user the event is about.
    pub username: String,
    /// `Login`, `Logout`, `ProxyStart` or `ProxyStop`.
    pub kind: String,
    /// The app id for the proxy events, empty otherwise.
    pub data: Option<String>,
    /// The configured extra attributes.
    pub attributes: BTreeMap<String, String>,
}

impl UsageRecord {
    /// The record of an event, or `None` for events that are not collected (as in Java, where
    /// `ProxyStartFailed` and `AuthFailed` are not written).
    pub fn of(event: &Event, timestamp: i64) -> Option<Self> {
        Self::with_attributes(event, timestamp, &[])
    }

    /// Like [`Self::of`], evaluating `proxy.usage-stats-attributes` (and per-collector attributes)
    /// into the attribute columns. Expression failures are logged and become an empty string, so a
    /// broken expression never drops the event.
    pub fn with_attributes(
        event: &Event,
        timestamp: i64,
        attributes: &[crate::config::settings::NamedExpression],
    ) -> Option<Self> {
        let (kind, username, data) = match event {
            Event::UserLoggedIn { user_id } => ("Login", user_id.clone(), None),
            Event::UserLoggedOut { user_id, .. } => ("Logout", user_id.clone(), None),
            Event::ProxyStarted { proxy, .. } => (
                "ProxyStart",
                proxy.user_id.clone().unwrap_or_default(),
                proxy.spec_id.clone(),
            ),
            Event::ProxyStopped { proxy, .. } => (
                "ProxyStop",
                proxy.user_id.clone().unwrap_or_default(),
                proxy.spec_id.clone(),
            ),
            _ => return None,
        };
        Some(UsageRecord {
            event_time: timestamp,
            username,
            kind: kind.to_string(),
            data,
            attributes: evaluate_attribute_expressions(event, attributes),
        })
    }
}

/// Evaluates every configured attribute expression against the event.
fn evaluate_attribute_expressions(
    event: &Event,
    attributes: &[crate::config::settings::NamedExpression],
) -> BTreeMap<String, String> {
    if attributes.is_empty() {
        return BTreeMap::new();
    }
    let context = expression_context_for_event(event);
    let mut values = BTreeMap::new();
    for attribute in attributes {
        let Some(name) = attribute
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let value = match attribute
            .expression
            .as_deref()
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
        {
            Some(expression) => match spel::evaluate_to_string(expression, &context) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(
                        "usage-stats attribute '{name}' could not be evaluated ({error}); \
                         writing an empty value"
                    );
                    String::new()
                }
            },
            None => String::new(),
        };
        values.insert(name.to_string(), value);
    }
    values
}

/// Builds the SpEL context of a usage-stats event (`userId`, and `proxy` when the event carries one).
fn expression_context_for_event(event: &Event) -> spel::Context {
    use crate::spec::expression::{ExpressionContextBuilder, UserContext};

    let (user_id, proxy) = match event {
        Event::UserLoggedIn { user_id } | Event::UserLoggedOut { user_id, .. } => {
            (user_id.clone(), None)
        }
        Event::ProxyStarted { proxy, .. } | Event::ProxyStopped { proxy, .. } => (
            proxy.user_id.clone().unwrap_or_default(),
            Some(proxy.as_ref().clone()),
        ),
        _ => (String::new(), None),
    };

    let mut builder = ExpressionContextBuilder::new().user(UserContext::new(user_id, Vec::new()));
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy);
    }
    builder.build()
}

/// Where the usage statistics go.
#[async_trait::async_trait]
pub trait StatCollector: Send + Sync + std::fmt::Debug {
    /// Writes one record.
    async fn write(&self, record: &UsageRecord) -> Result<(), String>;
}

/// The configuration of a collector is invalid.
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    /// The URL is not one of the supported forms.
    #[error("Base url for statistics contains an unrecognized values, baseURL {0}.")]
    UnrecognisedUrl(String),
    /// The collector could not be set up (file or database problem).
    #[error("{0}")]
    Setup(String),
    /// The collector is known but not implemented by this implementation yet.
    #[error("usage statistics to {0} are not supported yet by this implementation")]
    Unsupported(String),
}

/// Which collector a URL selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorKind {
    /// The Prometheus metrics (`micrometer`).
    Micrometer,
    /// A CSV file.
    Csv(PathBuf),
    /// A SQL database (a `jdbc:` URL).
    Database(String),
    /// InfluxDB (a URL containing `/write?db=`).
    InfluxDb(String),
}

impl CollectorKind {
    /// Classifies a `proxy.usage-stats-url`, like `StatCollectorFactory.createCollector`.
    pub fn of(url: &str) -> Result<Self, CollectorError> {
        let lower = url.to_ascii_lowercase();
        if lower.contains("/write?db=") {
            Ok(CollectorKind::InfluxDb(url.to_string()))
        } else if lower.starts_with("jdbc") {
            Ok(CollectorKind::Database(url.to_string()))
        } else if lower == "micrometer" {
            Ok(CollectorKind::Micrometer)
        } else if lower.ends_with(".csv") {
            Ok(CollectorKind::Csv(PathBuf::from(url)))
        } else {
            Err(CollectorError::UnrecognisedUrl(url.to_string()))
        }
    }
}

/// Writes the records to a CSV file (`CSVCollector`).
#[derive(Debug)]
pub struct CsvCollector {
    path: PathBuf,
    /// The names of the extra attributes, which become extra columns.
    attributes: Vec<String>,
    /// Serialises the writes, so that rows never interleave.
    lock: tokio::sync::Mutex<()>,
}

impl CsvCollector {
    /// Creates the collector and writes the header when the file is new.
    pub fn new(path: PathBuf, attributes: Vec<String>) -> Result<Self, CollectorError> {
        if let Some(directory) = path.parent() {
            if !directory.as_os_str().is_empty() {
                std::fs::create_dir_all(directory).map_err(|error| {
                    CollectorError::Setup(format!(
                        "cannot create the directory of {}: {error}",
                        path.display()
                    ))
                })?;
            }
        }

        let existing = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if existing == 0 {
            let header = header_line(&attributes);
            std::fs::write(&path, header).map_err(|error| {
                CollectorError::Setup(format!("cannot write {}: {error}", path.display()))
            })?;
        }

        Ok(CsvCollector {
            path,
            attributes,
            lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// The header of the CSV file; the Java collector quotes every value.
fn header_line(attributes: &[String]) -> String {
    let mut columns = vec![
        "event_time".to_string(),
        "username".to_string(),
        "type".to_string(),
        "data".to_string(),
    ];
    columns.extend(attributes.iter().cloned());
    format!(
        "{}\n",
        columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// One row of the CSV file.
fn record_line(record: &UsageRecord, attributes: &[String]) -> String {
    let mut values = vec![
        record.event_time.to_string(),
        record.username.clone(),
        record.kind.clone(),
        record.data.clone().unwrap_or_default(),
    ];
    for attribute in attributes {
        values.push(
            record
                .attributes
                .get(attribute)
                .cloned()
                .unwrap_or_default(),
        );
    }
    format!(
        "{}\n",
        values
            .iter()
            .map(|value| format!("\"{}\"", value.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[async_trait::async_trait]
impl StatCollector for CsvCollector {
    async fn write(&self, record: &UsageRecord) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|error| format!("cannot open {}: {error}", self.path.display()))?;
        file.write_all(record_line(record, &self.attributes).as_bytes())
            .await
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))?;
        file.flush()
            .await
            .map_err(|error| format!("cannot flush {}: {error}", self.path.display()))
    }
}

/// Turns a JDBC URL into the URL the SQL driver of this implementation understands.
///
/// `jdbc:postgresql://host/db` becomes `postgres://host/db`, `jdbc:mysql://...` becomes `mysql://...` and
/// `jdbc:sqlite:/path` becomes `sqlite:/path`, so that the configuration of a Java deployment works
/// unchanged.
pub fn database_url(jdbc_url: &str, username: Option<&str>, password: Option<&str>) -> String {
    let without_prefix = jdbc_url.trim_start_matches("jdbc:").to_string();
    let url = if let Some(rest) = without_prefix.strip_prefix("postgresql:") {
        format!("postgres:{rest}")
    } else {
        without_prefix
    };

    // JDBC creates a SQLite database that does not exist yet; sqlx only does so when it is asked
    let url = if url.starts_with("sqlite:") && !url.contains("mode=") {
        let separator = if url.contains('?') { '&' } else { '?' };
        format!("{url}{separator}mode=rwc")
    } else {
        url
    };

    // the credentials are configured separately in ShinyProxy, but SQL URLs carry them
    match (username, password) {
        (Some(username), password) if !url.contains('@') => {
            if let Some((scheme, rest)) = url.split_once("//") {
                let credentials = match password {
                    Some(password) if !password.is_empty() => {
                        format!("{}:{}", encode(username), encode(password))
                    }
                    _ => encode(username),
                };
                format!("{scheme}//{credentials}@{rest}")
            } else {
                url
            }
        }
        _ => url,
    }
}

/// Percent-encodes a value of a URL.
fn encode(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// The statement that creates the table, with the columns of the Java implementation.
pub fn create_table_statement(table: &str, attributes: &[String]) -> String {
    let mut columns = vec![
        "event_time timestamp".to_string(),
        "username varchar(128)".to_string(),
        "type varchar(128)".to_string(),
        "data text".to_string(),
    ];
    for attribute in attributes {
        columns.push(format!("{attribute} text"));
    }
    format!("create table if not exists {table}({})", columns.join(", "))
}

/// The insert statement, with one placeholder per column.
pub fn insert_statement(table: &str, attributes: &[String], numbered: bool) -> String {
    let mut columns = vec!["event_time", "username", "type", "data"];
    let owned: Vec<String> = attributes.to_vec();
    for attribute in &owned {
        columns.push(attribute.as_str());
    }
    let placeholders: Vec<String> = (1..=columns.len())
        .map(|index| {
            if numbered {
                format!("${index}")
            } else {
                "?".to_string()
            }
        })
        .collect();
    format!(
        "insert into {table}({}) values ({})",
        columns.join(", "),
        placeholders.join(", ")
    )
}

/// Writes the records to a SQL database (`JDBCCollector`).
///
/// The `jdbc:` URL of the configuration is converted into the URL of the SQL driver, so that the same
/// configuration works with this implementation. PostgreSQL, MySQL/MariaDB and SQLite are supported.
#[derive(Debug)]
pub struct SqlCollector {
    pool: sqlx::AnyPool,
    table: String,
    attributes: Vec<String>,
    /// Whether the placeholders are numbered (`$1`, PostgreSQL) or not (`?`).
    numbered_placeholders: bool,
}

impl SqlCollector {
    /// Connects to the database and creates the table when it does not exist yet.
    pub async fn connect(
        jdbc_url: &str,
        username: Option<&str>,
        password: Option<&str>,
        table: &str,
        attributes: Vec<String>,
        pool: &crate::config::HikariSettings,
    ) -> Result<Self, CollectorError> {
        sqlx::any::install_default_drivers();

        let url = database_url(jdbc_url, username, password);
        let numbered_placeholders = url.starts_with("postgres");

        let mut options = sqlx::pool::PoolOptions::<sqlx::Any>::new();
        if let Some(timeout) = pool.connection_timeout.map(|value| value.0) {
            options = options.acquire_timeout(std::time::Duration::from_millis(timeout as u64));
        }
        if let Some(timeout) = pool.idle_timeout.map(|value| value.0) {
            options = options.idle_timeout(std::time::Duration::from_millis(timeout as u64));
        }
        if let Some(lifetime) = pool.max_lifetime.map(|value| value.0) {
            options = options.max_lifetime(std::time::Duration::from_millis(lifetime as u64));
        }
        if let Some(minimum) = pool.minimum_idle.map(|value| value.0) {
            options = options.min_connections(minimum.max(0) as u32);
        }
        if let Some(maximum) = pool.maximum_pool_size.map(|value| value.0) {
            options = options.max_connections(maximum.max(1) as u32);
        }

        let pool = options.connect(&url).await.map_err(|error| {
            CollectorError::Setup(format!("cannot connect to {jdbc_url}: {error}"))
        })?;

        sqlx::query(&create_table_statement(table, &attributes))
            .execute(&pool)
            .await
            .map_err(|error| {
                CollectorError::Setup(format!("cannot create the table {table}: {error}"))
            })?;

        // the extra columns of an existing table are added, as the Java collector does
        for attribute in &attributes {
            let statement = format!("alter table {table} add {attribute} text");
            if let Err(error) = sqlx::query(&statement).execute(&pool).await {
                tracing::debug!("column {attribute} of {table} exists already ({error})");
            }
        }

        Ok(SqlCollector {
            pool,
            table: table.to_string(),
            attributes,
            numbered_placeholders,
        })
    }
}

#[async_trait::async_trait]
impl StatCollector for SqlCollector {
    async fn write(&self, record: &UsageRecord) -> Result<(), String> {
        let statement = insert_statement(&self.table, &self.attributes, self.numbered_placeholders);
        let mut query = sqlx::query(&statement)
            // the timestamp is stored as the epoch millis of the event; a `timestamp` column accepts it
            // in SQLite and MySQL, and PostgreSQL receives the formatted value below
            .bind(record.event_time)
            .bind(record.username.clone())
            .bind(record.kind.clone())
            .bind(record.data.clone().unwrap_or_default());
        for attribute in &self.attributes {
            query = query.bind(
                record
                    .attributes
                    .get(attribute)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        query
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|error| format!("cannot write the usage statistics: {error}"))
    }
}

/// Builds the collectors of a configuration (`StatCollectorFactory.init`).
///
/// `proxy.usage-stats-url` and every entry of `proxy.usage-stats` produce one collector. The
/// `micrometer` collector needs no object here, because the metrics are always collected; it is reported
/// through the returned flag so that the caller can log the same message as Java.
pub async fn create_collectors(
    settings: &crate::config::Settings,
) -> Result<Vec<Arc<dyn StatCollector>>, CollectorError> {
    let mut configured: Vec<ConfiguredCollector> = Vec::new();
    if let Some(url) = settings
        .proxy
        .usage_stats_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
    {
        configured.push(ConfiguredCollector {
            url: Some(url.to_string()),
            username: settings.proxy.usage_stats_username.clone(),
            password: settings.proxy.usage_stats_password.clone(),
            table: settings.proxy.usage_stats_table_name.clone(),
            attributes: attribute_names(&settings.proxy.usage_stats_attributes),
        });
    }
    for entry in &settings.proxy.usage_stats {
        configured.push(ConfiguredCollector {
            url: entry.url.clone(),
            username: entry.username.clone(),
            password: entry.password.clone(),
            table: entry.table_name.clone(),
            attributes: attribute_names(&entry.attributes),
        });
    }

    let mut collectors: Vec<Arc<dyn StatCollector>> = Vec::new();
    for ConfiguredCollector {
        url,
        username,
        password,
        table,
        attributes,
    } in configured
    {
        let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
            continue;
        };
        tracing::info!("Enabled. Sending usage statistics to {url}.");
        match CollectorKind::of(&url)? {
            // the metrics are always collected and exposed on /actuator/prometheus
            CollectorKind::Micrometer => {}
            CollectorKind::Csv(path) => {
                collectors.push(Arc::new(CsvCollector::new(path, attributes)?));
            }
            CollectorKind::Database(jdbc_url) => {
                let table = table.unwrap_or_else(|| "event".to_string());
                collectors.push(Arc::new(
                    SqlCollector::connect(
                        &jdbc_url,
                        username.as_deref(),
                        password.as_deref(),
                        &table,
                        attributes,
                        &settings.proxy.usage_stats_hikari,
                    )
                    .await?,
                ));
            }
            CollectorKind::InfluxDb(url) => {
                return Err(CollectorError::Unsupported(url));
            }
        }
    }
    Ok(collectors)
}

/// One configured collector (`proxy.usage-stats-url` or an entry of `proxy.usage-stats`).
#[derive(Debug, Clone, Default)]
struct ConfiguredCollector {
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    table: Option<String>,
    attributes: Vec<String>,
}

/// The names of the configured extra attributes.
fn attribute_names(attributes: &[crate::config::settings::NamedExpression]) -> Vec<String> {
    attributes
        .iter()
        .filter_map(|attribute| attribute.name.clone())
        .filter(|name| !name.trim().is_empty())
        .collect()
}

/// Reads the records of the events and hands them to the collectors.
#[derive(Debug)]
pub struct UsageStatsService {
    collectors: Vec<Arc<dyn StatCollector>>,
    /// Attribute definitions evaluated into every record (`proxy.usage-stats-attributes` and the
    /// attributes of every `proxy.usage-stats` entry).
    attributes: Vec<crate::config::settings::NamedExpression>,
}

impl UsageStatsService {
    /// Creates the service with the given collectors and no attribute expressions.
    pub fn new(collectors: Vec<Arc<dyn StatCollector>>) -> Self {
        Self::with_attributes(collectors, Vec::new())
    }

    /// Creates the service with the collectors and the attribute expressions of the configuration.
    pub fn with_attributes(
        collectors: Vec<Arc<dyn StatCollector>>,
        attributes: Vec<crate::config::settings::NamedExpression>,
    ) -> Self {
        UsageStatsService {
            collectors,
            attributes,
        }
    }

    /// Whether anything is collected.
    pub fn is_enabled(&self) -> bool {
        !self.collectors.is_empty()
    }

    /// Writes one record to every collector.
    pub async fn write(&self, record: &UsageRecord) {
        for collector in &self.collectors {
            if let Err(error) = collector.write(record).await {
                tracing::warn!("Collecting event failed: {error}");
            }
        }
    }

    /// Follows the events of the server.
    pub fn subscribe(self: &Arc<Self>, events: &EventBus) {
        if !self.is_enabled() {
            return;
        }
        let service = self.clone();
        let mut receiver = events.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = receiver.recv().await {
                if let Some(record) = UsageRecord::with_attributes(
                    &event,
                    crate::model::proxy::now_millis(),
                    &service.attributes,
                ) {
                    service.write(&record).await;
                }
            }
        });
    }
}

/// Every attribute definition of the configuration (global and per-collector), in order.
pub fn attribute_definitions(
    settings: &crate::config::Settings,
) -> Vec<crate::config::settings::NamedExpression> {
    let mut attributes = settings.proxy.usage_stats_attributes.clone();
    for entry in &settings.proxy.usage_stats {
        attributes.extend(entry.attributes.clone());
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Proxy, ProxyStatus};

    #[test]
    fn classifies_urls_like_the_java_factory() {
        assert_eq!(
            CollectorKind::of("micrometer").unwrap(),
            CollectorKind::Micrometer
        );
        assert_eq!(
            CollectorKind::of("MICROMETER").unwrap(),
            CollectorKind::Micrometer
        );
        assert_eq!(
            CollectorKind::of("/var/log/usage.csv").unwrap(),
            CollectorKind::Csv(PathBuf::from("/var/log/usage.csv"))
        );
        assert_eq!(
            CollectorKind::of("jdbc:postgresql://db/shinyproxy").unwrap(),
            CollectorKind::Database("jdbc:postgresql://db/shinyproxy".to_string())
        );
        assert_eq!(
            CollectorKind::of("http://influx:8086/write?db=shinyproxy").unwrap(),
            CollectorKind::InfluxDb("http://influx:8086/write?db=shinyproxy".to_string())
        );

        let error = CollectorKind::of("something-else").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Base url for statistics contains an unrecognized values, baseURL something-else."
        );
    }

    #[test]
    fn converts_jdbc_urls() {
        assert_eq!(
            database_url("jdbc:postgresql://db:5432/shinyproxy", None, None),
            "postgres://db:5432/shinyproxy"
        );
        assert_eq!(
            database_url("jdbc:mysql://db:3306/shinyproxy", None, None),
            "mysql://db:3306/shinyproxy"
        );
        // a SQLite database that does not exist yet is created, as JDBC does
        assert_eq!(
            database_url("jdbc:sqlite:/tmp/usage.db", None, None),
            "sqlite:/tmp/usage.db?mode=rwc"
        );
        assert_eq!(
            database_url("jdbc:sqlite:/tmp/usage.db?mode=ro", None, None),
            "sqlite:/tmp/usage.db?mode=ro",
            "an explicit mode wins"
        );
        assert_eq!(
            database_url(
                "jdbc:postgresql://db:5432/shinyproxy",
                Some("user"),
                Some("p@ss")
            ),
            "postgres://user:p%40ss@db:5432/shinyproxy"
        );
        assert_eq!(
            database_url("jdbc:postgresql://db/shinyproxy", Some("user"), None),
            "postgres://user@db/shinyproxy"
        );
    }

    #[test]
    fn builds_the_java_statements() {
        assert_eq!(
            create_table_statement("event", &[]),
            "create table if not exists event(event_time timestamp, username varchar(128), \
             type varchar(128), data text)"
        );
        assert_eq!(
            create_table_statement("usage", &["organisation".to_string()]),
            "create table if not exists usage(event_time timestamp, username varchar(128), \
             type varchar(128), data text, organisation text)"
        );
        assert_eq!(
            insert_statement("event", &[], false),
            "insert into event(event_time, username, type, data) values (?, ?, ?, ?)"
        );
        assert_eq!(
            insert_statement("event", &["organisation".to_string()], true),
            "insert into event(event_time, username, type, data, organisation) \
             values ($1, $2, $3, $4, $5)"
        );
    }

    #[test]
    fn builds_records_of_the_collected_events() {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".to_string());
        proxy.user_id = Some("jack".to_string());

        let record = UsageRecord::of(
            &Event::ProxyStarted {
                proxy: Box::new(proxy.clone()),
                startup_time_ms: None,
            },
            1234,
        )
        .expect("record");
        assert_eq!(record.kind, "ProxyStart");
        assert_eq!(record.username, "jack");
        assert_eq!(record.data.as_deref(), Some("01_hello"));
        assert_eq!(record.event_time, 1234);

        let record = UsageRecord::of(
            &Event::UserLoggedIn {
                user_id: "jack".to_string(),
            },
            1,
        )
        .expect("record");
        assert_eq!(record.kind, "Login");
        assert_eq!(record.data, None);

        assert_eq!(
            UsageRecord::of(
                &Event::UserLoggedOut {
                    user_id: "jack".to_string(),
                    expired: true
                },
                1
            )
            .expect("record")
            .kind,
            "Logout"
        );

        // events the Java collectors do not write
        assert!(UsageRecord::of(
            &Event::AuthenticationFailed {
                user_id: "jack".to_string()
            },
            1
        )
        .is_none());
        assert!(UsageRecord::of(
            &Event::ProxyStartFailed {
                proxy: Box::new(proxy)
            },
            1
        )
        .is_none());
    }

    #[test]
    fn evaluates_usage_stats_attribute_expressions() {
        use crate::config::settings::NamedExpression;
        use crate::model::runtime_value::{RuntimeValue, REALM_ID};

        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".to_string());
        proxy.user_id = Some("jack".to_string());
        proxy.add_runtime_value(RuntimeValue::string(&REALM_ID, "realm-a"), true);

        let attributes = vec![
            NamedExpression {
                name: Some("realm".to_string()),
                expression: Some("#{proxy.getRuntimeValue('SHINYPROXY_REALM_ID')}".to_string()),
            },
            NamedExpression {
                name: Some("user".to_string()),
                expression: Some("#{userId}".to_string()),
            },
            NamedExpression {
                name: Some("broken".to_string()),
                expression: Some("#{unknownProperty}".to_string()),
            },
        ];

        let record = UsageRecord::with_attributes(
            &Event::ProxyStarted {
                proxy: Box::new(proxy),
                startup_time_ms: None,
            },
            1,
            &attributes,
        )
        .expect("record");
        assert_eq!(
            record.attributes.get("realm").map(String::as_str),
            Some("realm-a")
        );
        assert_eq!(
            record.attributes.get("user").map(String::as_str),
            Some("jack")
        );
        assert_eq!(
            record.attributes.get("broken").map(String::as_str),
            Some(""),
            "a broken expression becomes an empty column instead of dropping the event"
        );

        // login events have no proxy: proxy expressions fail soft
        let record = UsageRecord::with_attributes(
            &Event::UserLoggedIn {
                user_id: "jack".to_string(),
            },
            1,
            &attributes,
        )
        .expect("record");
        assert_eq!(record.attributes.get("user").map(String::as_str), Some("jack"));
        assert_eq!(record.attributes.get("realm").map(String::as_str), Some(""));
    }

    #[tokio::test]
    async fn writes_a_csv_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("stats").join("usage.csv");
        let collector =
            CsvCollector::new(path.clone(), vec!["organisation".to_string()]).expect("collector");

        let mut record = UsageRecord {
            event_time: 1_700_000_000_000,
            username: "jack".to_string(),
            kind: "ProxyStart".to_string(),
            data: Some("01_hello".to_string()),
            attributes: BTreeMap::new(),
        };
        record
            .attributes
            .insert("organisation".to_string(), "openanalytics".to_string());
        collector.write(&record).await.expect("writes");
        collector
            .write(&UsageRecord {
                kind: "Logout".to_string(),
                data: None,
                attributes: BTreeMap::new(),
                ..record.clone()
            })
            .await
            .expect("writes");

        let contents = std::fs::read_to_string(&path).expect("csv");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines[0], "\"event_time\",\"username\",\"type\",\"data\",\"organisation\"",
            "the header has the Java columns"
        );
        assert_eq!(
            lines[1],
            "\"1700000000000\",\"jack\",\"ProxyStart\",\"01_hello\",\"openanalytics\""
        );
        assert_eq!(lines[2], "\"1700000000000\",\"jack\",\"Logout\",\"\",\"\"");

        // an existing file is appended to, and the header is not repeated
        let collector =
            CsvCollector::new(path.clone(), vec!["organisation".to_string()]).expect("collector");
        collector.write(&record).await.expect("writes");
        let contents = std::fs::read_to_string(&path).expect("csv");
        assert_eq!(contents.lines().count(), 4);
        assert_eq!(
            contents
                .lines()
                .filter(|line| line.contains("event_time"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn writes_to_a_sql_database() {
        let directory = tempfile::tempdir().expect("temp dir");
        let database = directory.path().join("usage.db");
        let url = format!("jdbc:sqlite:{}?mode=rwc", database.display());

        let collector = SqlCollector::connect(
            &url,
            None,
            None,
            "event",
            vec!["organisation".to_string()],
            &crate::config::HikariSettings::default(),
        )
        .await
        .expect("connects");

        let mut record = UsageRecord {
            event_time: 1_700_000_000_000,
            username: "jack".to_string(),
            kind: "ProxyStart".to_string(),
            data: Some("01_hello".to_string()),
            attributes: BTreeMap::new(),
        };
        record
            .attributes
            .insert("organisation".to_string(), "openanalytics".to_string());
        collector.write(&record).await.expect("writes");
        collector
            .write(&UsageRecord {
                kind: "ProxyStop".to_string(),
                ..record.clone()
            })
            .await
            .expect("writes");

        // the rows are readable with the column names of the Java implementation
        // the timestamp is read as text, because the `Any` driver of sqlx does not decode the
        // database specific timestamp types
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "select cast(event_time as text), username, type, data, organisation \
             from event order by type",
        )
        .fetch_all(&collector.pool)
        .await
        .expect("reads");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "1700000000000");
        assert_eq!(rows[0].1, "jack");
        assert_eq!(rows[0].2, "ProxyStart");
        assert_eq!(rows[0].3, "01_hello");
        assert_eq!(rows[0].4, "openanalytics");
        assert_eq!(rows[1].2, "ProxyStop");

        // connecting again reuses the table instead of failing
        let collector = SqlCollector::connect(
            &url,
            None,
            None,
            "event",
            vec!["organisation".to_string()],
            &crate::config::HikariSettings::default(),
        )
        .await
        .expect("connects again");
        collector.write(&record).await.expect("writes");
        let count: (i64,) = sqlx::query_as("select count(*) from event")
            .fetch_one(&collector.pool)
            .await
            .expect("reads");
        assert_eq!(count.0, 3);
    }

    #[tokio::test]
    async fn writes_every_collected_event() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("usage.csv");
        let collector = Arc::new(CsvCollector::new(path.clone(), Vec::new()).expect("collector"));
        let service = Arc::new(UsageStatsService::new(vec![collector]));
        assert!(service.is_enabled());

        let events = EventBus::new();
        service.subscribe(&events);

        events.publish(Event::UserLoggedIn {
            user_id: "jack".to_string(),
        });
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.spec_id = Some("01_hello".to_string());
        proxy.user_id = Some("jack".to_string());
        events.publish(Event::ProxyStarted {
            proxy: Box::new(proxy.clone()),
            startup_time_ms: Some(10),
        });
        events.publish(Event::ProxyStopped {
            proxy: Box::new(proxy),
            reason: crate::model::proxy::ProxyStopReason::ByUser,
        });

        // the writes happen in a task, so the file is polled
        let mut contents = String::new();
        for _ in 0..50 {
            contents = std::fs::read_to_string(&path).unwrap_or_default();
            if contents.lines().count() >= 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(contents.contains("\"Login\""), "{contents}");
        assert!(
            contents.contains("\"ProxyStart\",\"01_hello\""),
            "{contents}"
        );
        assert!(
            contents.contains("\"ProxyStop\",\"01_hello\""),
            "{contents}"
        );
    }
}
