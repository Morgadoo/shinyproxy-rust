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

//! Loading of the configuration file, profiles, environment variables and command line arguments.
//!
//! Precedence, highest first (mirroring Spring Boot's externalized configuration order for the sources
//! ShinyProxy actually uses):
//!
//! 1. command line arguments (`--proxy.port=8081`)
//! 2. environment variables (`PROXY_PORT=8081`)
//! 3. profile specific configuration file (`application-{profile}.yml`)
//! 4. configuration file (`application.yml`)
//! 5. built-in defaults

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::schema::{KeyKind, Schema};
use super::tree;

/// Name of the configuration file, as in the Java implementation.
pub const CONFIG_FILENAME: &str = "application.yml";
/// Alternative extension accepted by Spring Boot.
pub const CONFIG_FILENAME_YAML: &str = "application.yaml";
/// Profile activated when no configuration file can be found.
pub const DEMO_PROFILE: &str = "demo";

/// Environment variable prefixes that are scanned for overrides.
const ENV_PREFIXES: &[&str] = &[
    "PROXY_",
    "SERVER_",
    "SPRING_",
    "LOGGING_",
    "MANAGEMENT_",
    "SPRINGDOC_",
];

/// Errors produced while loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read configuration file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse configuration file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("configuration file {path} must contain a YAML mapping at the top level")]
    NotAMapping { path: PathBuf },
    #[error("cannot resolve placeholder ${{{name}}} used in property '{property}'")]
    UnresolvedPlaceholder { name: String, property: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// Raw configuration: the merged property tree plus provenance information.
#[derive(Debug, Clone)]
pub struct RawConfig {
    /// Merged tree (file + profiles + environment + command line, placeholders resolved).
    pub tree: Value,
    /// Tree of the configuration file only, without profiles or overrides and with placeholders left
    /// untouched. The instance id is a hash of this tree, exactly like in the Java implementation.
    pub file_tree: Option<Value>,
    /// Path of the configuration file that was loaded, if any.
    pub path: Option<PathBuf>,
    /// Active profiles.
    pub profiles: Vec<String>,
    /// Properties present in the tree that the schema does not know about.
    pub unknown_properties: Vec<String>,
}

impl RawConfig {
    /// Reads a property as a string, like `Environment#getProperty`.
    pub fn property(&self, path: &str) -> Option<String> {
        tree::get_string(&self.tree, path)
    }

    /// Reads a property, falling back to the given default.
    pub fn property_or(&self, path: &str, default: &str) -> String {
        self.property(path).unwrap_or_else(|| default.to_string())
    }

    /// Reads a boolean property (Spring semantics: only "true" is true).
    pub fn property_bool(&self, path: &str, default: bool) -> bool {
        match tree::get(&self.tree, path) {
            Some(Value::Bool(value)) => *value,
            Some(Value::String(value)) => value.eq_ignore_ascii_case("true"),
            Some(Value::Number(value)) => value.as_i64().is_some_and(|number| number != 0),
            _ => default,
        }
    }

    /// Reads a list property, accepting a single value, a comma separated string or a YAML list
    /// (the Java `EnvironmentUtils.readList` behaviour).
    pub fn property_list(&self, path: &str) -> Option<Vec<String>> {
        match tree::get(&self.tree, path)? {
            Value::Array(items) => {
                let values: Vec<String> = items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Null => None,
                        Value::String(value) => Some(value.clone()),
                        other => Some(other.to_string()),
                    })
                    .collect();
                if values.is_empty() {
                    None
                } else {
                    Some(values)
                }
            }
            Value::String(value) if value.trim().is_empty() => None,
            Value::String(value) => Some(
                value
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect(),
            ),
            Value::Null => None,
            other => Some(vec![other.to_string()]),
        }
    }
}

/// Options controlling configuration loading (used by tests to avoid touching the real environment).
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Command line arguments (without the program name).
    pub args: Vec<String>,
    /// Environment variables to consider.
    pub env: BTreeMap<String, String>,
    /// Working directory used to look for `application.yml`.
    pub working_dir: Option<PathBuf>,
    /// Configuration used when no file is found (the embedded demo configuration).
    pub fallback_config: Option<String>,
}

impl LoadOptions {
    /// Options taken from the current process (real command line and environment).
    pub fn from_process() -> Self {
        LoadOptions {
            args: std::env::args().skip(1).collect(),
            env: std::env::vars().collect(),
            working_dir: std::env::current_dir().ok(),
            fallback_config: None,
        }
    }

    /// Sets the fallback (demo) configuration.
    pub fn with_fallback_config(mut self, config: impl Into<String>) -> Self {
        self.fallback_config = Some(config.into());
        self
    }
}

/// Loads the configuration.
pub fn load(schema: &Schema, options: &LoadOptions) -> Result<RawConfig, ConfigError> {
    let args = parse_args(&options.args);

    // 1. locate the configuration file
    let explicit_location = args
        .get("spring.config.location")
        .cloned()
        .or_else(|| options.env.get("SPRING_CONFIG_LOCATION").cloned());
    let working_dir = options
        .working_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let config_path = resolve_config_path(explicit_location.as_deref(), &working_dir);

    // 2. parse it (or fall back to the demo configuration)
    let mut profiles: Vec<String> = Vec::new();
    let (file_tree, path) = match &config_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            (Some(parse_yaml(path, &text)?), Some(path.clone()))
        }
        None => {
            profiles.push(DEMO_PROFILE.to_string());
            match &options.fallback_config {
                Some(text) => (
                    Some(parse_yaml(Path::new("application-demo.yml"), text)?),
                    None,
                ),
                None => (None, None),
            }
        }
    };

    let mut merged = file_tree
        .clone()
        .unwrap_or_else(|| Value::Object(Default::default()));

    // 3. additional profiles requested through the file, the environment or the command line
    for source in [
        tree::get_string(&merged, "spring.profiles.active"),
        options.env.get("SPRING_PROFILES_ACTIVE").cloned(),
        args.get("spring.profiles.active").cloned(),
    ]
    .into_iter()
    .flatten()
    {
        for profile in source.split(',') {
            let profile = profile.trim();
            if !profile.is_empty() && !profiles.iter().any(|existing| existing == profile) {
                profiles.push(profile.to_string());
            }
        }
    }

    // 4. profile specific files, next to the main configuration file
    if let Some(path) = &path {
        let directory = path.parent().unwrap_or(Path::new("."));
        for profile in &profiles {
            for extension in ["yml", "yaml"] {
                let candidate = directory.join(format!("application-{profile}.{extension}"));
                if candidate.is_file() {
                    let text = std::fs::read_to_string(&candidate).map_err(|source| {
                        ConfigError::Read {
                            path: candidate.clone(),
                            source,
                        }
                    })?;
                    let profile_tree = parse_yaml(&candidate, &text)?;
                    tree::merge(&mut merged, profile_tree);
                }
            }
        }
    }

    // 5. environment variables
    apply_env(schema, &options.env, &mut merged);

    // 6. command line arguments (highest precedence)
    for (key, value) in &args {
        tree::set(&mut merged, key, tree::scalar_from_str(value));
    }

    // 7. rewrite relaxed property names to their canonical spelling
    normalize_keys(schema, &mut merged, "");

    // 8. placeholder resolution
    resolve_placeholders(&mut merged, &options.env)?;

    let unknown_properties = unknown_properties(schema, &merged);

    Ok(RawConfig {
        tree: merged,
        file_tree,
        path,
        profiles,
        unknown_properties,
    })
}

/// Parses `--key=value` and `--key value` arguments into a map of properties.
fn parse_args(args: &[String]) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((key, value)) = rest.split_once('=') {
                result.insert(key.to_string(), value.to_string());
            } else if rest.contains('.') {
                // `--proxy.port 8081`
                if let Some(value) = args.get(index + 1) {
                    if !value.starts_with("--") {
                        result.insert(rest.to_string(), value.clone());
                        index += 1;
                    }
                }
            }
        }
        index += 1;
    }
    result
}

/// Finds the configuration file, following the Java lookup order.
fn resolve_config_path(explicit: Option<&str>, working_dir: &Path) -> Option<PathBuf> {
    if let Some(location) = explicit {
        // Spring accepts a comma separated list of files and/or directories.
        for entry in location.split(',') {
            let entry = entry.trim().trim_start_matches("file:");
            if entry.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(entry);
            if candidate.is_file() {
                return Some(candidate);
            }
            if candidate.is_dir() {
                for name in [CONFIG_FILENAME, CONFIG_FILENAME_YAML] {
                    let file = candidate.join(name);
                    if file.is_file() {
                        return Some(file);
                    }
                }
            }
        }
        return None;
    }

    for name in [CONFIG_FILENAME, CONFIG_FILENAME_YAML] {
        let candidate = working_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn parse_yaml(path: &Path, text: &str) -> Result<Value, ConfigError> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_yaml_ng::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    match value {
        Value::Object(_) => Ok(value),
        Value::Null => Ok(Value::Object(Default::default())),
        _ => Err(ConfigError::NotAMapping {
            path: path.to_path_buf(),
        }),
    }
}

/// Applies environment variables to the tree, for every property the schema knows about.
fn apply_env(schema: &Schema, env: &BTreeMap<String, String>, target: &mut Value) {
    if !env
        .keys()
        .any(|key| ENV_PREFIXES.iter().any(|prefix| key.starts_with(prefix)))
    {
        return;
    }

    for key in schema.simple_keys() {
        if key.kind == KeyKind::Map {
            // Free form maps cannot be bound from the environment (arbitrary keys).
            continue;
        }
        let base = env_name(key.path);
        if let Some(value) = env.get(&base) {
            match key.kind {
                KeyKind::ScalarList => tree::set(&mut *target, key.path, split_list(value)),
                _ => tree::set(&mut *target, key.path, tree::scalar_from_str(value)),
            }
            continue;
        }
        if key.kind == KeyKind::ScalarList {
            // Indexed form: PROXY_ADMIN_GROUPS_0 / PROXY_ADMIN_GROUPS_0_
            let mut values = Vec::new();
            for index in 0.. {
                let candidate = env
                    .get(&format!("{base}_{index}"))
                    .or_else(|| env.get(&format!("{base}_{index}_")));
                match candidate {
                    Some(value) => values.push(Value::String(value.clone())),
                    None => break,
                }
            }
            if !values.is_empty() {
                tree::set(&mut *target, key.path, Value::Array(values));
            }
        }
    }

    // Array properties: PROXY_USERS_0_NAME, PROXY_SPECS_1_CONTAINER_IMAGE, ...
    // Only one level of nesting is bound from the environment; deeper nesting (for example
    // `proxy.usage-stats[].attributes[].name`) has to be configured in the file, see
    // docs/COMPATIBILITY.md.
    for (root, members) in schema.array_groups() {
        let base = env_name(&root);
        for index in 0.. {
            let mut found = false;
            for member in &members {
                let Some((_, field)) = member.path.split_once("[].") else {
                    continue;
                };
                if field.contains("[]") || member.kind == KeyKind::Map {
                    continue;
                }
                let name = format!("{base}_{index}_{}", env_name(field));
                if let Some(value) = env.get(&name) {
                    let path = format!("{root}[{index}].{field}");
                    match member.kind {
                        KeyKind::ScalarList => tree::set(&mut *target, &path, split_list(value)),
                        _ => tree::set(&mut *target, &path, tree::scalar_from_str(value)),
                    }
                    found = true;
                }
            }
            if !found {
                break;
            }
        }
    }
}

/// Rewrites relaxed property names to their canonical spelling so that the tree can be deserialized
/// into typed settings by serde (which matches field names exactly).
///
/// Keys below free form map properties (`container-env`, `logging.level`, ...) are left untouched.
fn normalize_keys(schema: &Schema, value: &mut Value, path: &str) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let canonical = schema.canonical_segment(&child_path);
                let is_map = schema
                    .find(&child_path)
                    .is_some_and(|definition| definition.kind == KeyKind::Map);

                let mut child = map.remove(&key).unwrap_or(Value::Null);
                let target_key = canonical.map(str::to_string).unwrap_or_else(|| key.clone());
                if !is_map {
                    let normalized_path = if path.is_empty() {
                        target_key.clone()
                    } else {
                        format!("{path}.{target_key}")
                    };
                    normalize_keys(schema, &mut child, &normalized_path);
                }
                map.insert(target_key, child);
            }
        }
        Value::Array(items) => {
            let child_path = format!("{path}[]");
            for item in items.iter_mut() {
                normalize_keys(schema, item, &child_path);
            }
        }
        _ => {}
    }
}

/// Environment variable name of a property path, following Spring's rules.
fn env_name(path: &str) -> String {
    path.chars()
        .map(|character| match character {
            '.' | '-' => '_',
            other => other.to_ascii_uppercase(),
        })
        .collect()
}

fn split_list(value: &str) -> Value {
    Value::Array(
        value
            .split(',')
            .map(|part| Value::String(part.trim().to_string()))
            .filter(|part| part.as_str().is_some_and(|value| !value.is_empty()))
            .collect(),
    )
}

/// Replaces `${property}` / `${property:default}` placeholders in every string of the tree.
fn resolve_placeholders(
    target: &mut Value,
    env: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let snapshot = target.clone();
    let paths: Vec<String> = tree::flatten(&snapshot)
        .into_iter()
        .filter(|(_, value)| value.is_string())
        .map(|(path, _)| path)
        .collect();
    for path in paths {
        let Some(Value::String(raw)) = tree::get(&snapshot, &path).cloned() else {
            continue;
        };
        if !raw.contains("${") {
            continue;
        }
        let resolved = resolve_placeholders_in(&raw, &snapshot, env, &path, 0)?;
        tree::set(target, &path, Value::String(resolved));
    }
    Ok(())
}

fn resolve_placeholders_in(
    raw: &str,
    snapshot: &Value,
    env: &BTreeMap<String, String>,
    property: &str,
    depth: usize,
) -> Result<String, ConfigError> {
    if depth > 8 {
        return Err(ConfigError::Invalid(format!(
            "placeholder resolution of property '{property}' is too deeply nested"
        )));
    }
    let mut result = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // Unbalanced placeholder: keep the text as-is (Spring does the same).
            result.push_str(&rest[start..]);
            return Ok(result);
        };
        let expression = &after[..end];
        let (name, default) = match expression.split_once(':') {
            Some((name, default)) => (name.trim(), Some(default)),
            None => (expression.trim(), None),
        };
        let value = env
            .get(name)
            .cloned()
            .or_else(|| env.get(&env_name(name)).cloned())
            .or_else(|| tree::get_string(snapshot, name));
        let value = match (value, default) {
            (Some(value), _) => value,
            (None, Some(default)) => default.to_string(),
            (None, None) => {
                return Err(ConfigError::UnresolvedPlaceholder {
                    name: name.to_string(),
                    property: property.to_string(),
                })
            }
        };
        result.push_str(&resolve_placeholders_in(
            &value,
            snapshot,
            env,
            property,
            depth + 1,
        )?);
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

/// Properties present in the tree that the schema does not know.
fn unknown_properties(schema: &Schema, target: &Value) -> Vec<String> {
    let mut unknown: Vec<String> = tree::flatten(target)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| !schema.is_known(path))
        .collect();
    unknown.sort();
    unknown.dedup();
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Schema {
        Schema::engine()
    }

    fn options(yaml: &str) -> (tempfile::TempDir, LoadOptions) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILENAME), yaml).unwrap();
        let options = LoadOptions {
            args: vec![],
            env: BTreeMap::new(),
            working_dir: Some(dir.path().to_path_buf()),
            fallback_config: None,
        };
        (dir, options)
    }

    #[test]
    fn loads_configuration_file_from_working_directory() {
        let (_dir, options) = options("proxy:\n  port: 8081\n  title: Test\n");
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.property("proxy.port").as_deref(), Some("8081"));
        assert_eq!(config.property("proxy.title").as_deref(), Some("Test"));
        assert!(config.path.is_some());
        assert!(config.profiles.is_empty());
        assert!(
            config.unknown_properties.is_empty(),
            "{:?}",
            config.unknown_properties
        );
    }

    #[test]
    fn falls_back_to_demo_profile_without_configuration_file() {
        let dir = tempfile::tempdir().unwrap();
        let options = LoadOptions {
            working_dir: Some(dir.path().to_path_buf()),
            ..LoadOptions::default()
        }
        .with_fallback_config("proxy:\n  port: 8080\n  authentication: simple\n");
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.profiles, vec!["demo".to_string()]);
        assert_eq!(
            config.property("proxy.authentication").as_deref(),
            Some("simple")
        );
        assert!(config.path.is_none());
    }

    #[test]
    fn command_line_overrides_environment_and_file() {
        let (dir, mut options) = options("proxy:\n  port: 8080\n");
        options.env.insert("PROXY_PORT".into(), "8081".into());
        options.args = vec!["--proxy.port=8082".into()];
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.property("proxy.port").as_deref(), Some("8082"));

        options.args.clear();
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.property("proxy.port").as_deref(), Some("8081"));
        drop(dir);
    }

    #[test]
    fn binds_environment_variables_with_relaxed_names() {
        let (_dir, mut options) = options("proxy:\n  docker:\n    port-range-start: 20000\n");
        options
            .env
            .insert("PROXY_DOCKER_PORT_RANGE_START".into(), "30000".into());
        options
            .env
            .insert("PROXY_ADMIN_GROUPS_0".into(), "scientists".into());
        options
            .env
            .insert("PROXY_ADMIN_GROUPS_1".into(), "admins".into());
        options
            .env
            .insert("PROXY_USERS_0_NAME".into(), "jack".into());
        options
            .env
            .insert("PROXY_USERS_0_PASSWORD".into(), "secret".into());
        options
            .env
            .insert("SPRING_SESSION_STORE_TYPE".into(), "redis".into());
        let config = load(&schema(), &options).unwrap();

        assert_eq!(
            config.property("proxy.docker.port-range-start").as_deref(),
            Some("30000")
        );
        assert_eq!(
            config.property_list("proxy.admin-groups"),
            Some(vec!["scientists".to_string(), "admins".to_string()])
        );
        assert_eq!(
            config.property("proxy.users[0].name").as_deref(),
            Some("jack")
        );
        assert_eq!(
            config.property("spring.session.store-type").as_deref(),
            Some("redis")
        );
    }

    #[test]
    fn reads_lists_in_all_supported_notations() {
        let (_dir, options) = options(
            "proxy:\n  admin-groups: scientists\n  body-classes: [a, b]\n  api-security:\n    cors-allowed-origins: 'http://a, http://b'\n",
        );
        let config = load(&schema(), &options).unwrap();
        assert_eq!(
            config.property_list("proxy.admin-groups"),
            Some(vec!["scientists".into()])
        );
        assert_eq!(
            config.property_list("proxy.body-classes"),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            config.property_list("proxy.api-security.cors-allowed-origins"),
            Some(vec!["http://a".into(), "http://b".into()])
        );
        assert_eq!(config.property_list("proxy.admin-users"), None);
    }

    #[test]
    fn resolves_placeholders_from_environment_and_properties() {
        let (_dir, mut options) = options(
            "proxy:\n  title: 'Hello ${DEPLOY_ENV}'\n  realm-id: ${proxy.title}\n  logo-url: ${MISSING:https://example.com/logo.png}\n",
        );
        options.env.insert("DEPLOY_ENV".into(), "prod".into());
        let config = load(&schema(), &options).unwrap();
        assert_eq!(
            config.property("proxy.title").as_deref(),
            Some("Hello prod")
        );
        assert_eq!(
            config.property("proxy.realm-id").as_deref(),
            Some("Hello prod")
        );
        assert_eq!(
            config.property("proxy.logo-url").as_deref(),
            Some("https://example.com/logo.png")
        );
    }

    #[test]
    fn fails_on_unresolvable_placeholder() {
        let (_dir, options) = options("proxy:\n  title: ${NOPE}\n");
        let error = load(&schema(), &options).unwrap_err();
        assert!(
            matches!(error, ConfigError::UnresolvedPlaceholder { .. }),
            "{error}"
        );
    }

    #[test]
    fn reports_unknown_properties() {
        let (_dir, options) = options("proxy:\n  port: 8080\n  not-a-property: 1\n");
        let config = load(&schema(), &options).unwrap();
        assert_eq!(
            config.unknown_properties,
            vec!["proxy.not-a-property".to_string()]
        );
    }

    #[test]
    fn loads_profile_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILENAME),
            "spring:\n  profiles:\n    active: test\nproxy:\n  port: 8080\n  title: base\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("application-test.yml"),
            "proxy:\n  title: from-profile\n",
        )
        .unwrap();
        let options = LoadOptions {
            working_dir: Some(dir.path().to_path_buf()),
            ..LoadOptions::default()
        };
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.profiles, vec!["test".to_string()]);
        assert_eq!(
            config.property("proxy.title").as_deref(),
            Some("from-profile")
        );
        assert_eq!(config.property("proxy.port").as_deref(), Some("8080"));
    }

    #[test]
    fn honours_explicit_config_location() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.yml");
        std::fs::write(&path, "proxy:\n  port: 9000\n").unwrap();

        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.property("proxy.port").as_deref(), Some("9000"));
        assert_eq!(config.path.as_deref(), Some(path.as_path()));

        let options = LoadOptions {
            env: BTreeMap::from([(
                "SPRING_CONFIG_LOCATION".to_string(),
                dir.path().display().to_string(),
            )]),
            ..LoadOptions::default()
        };
        // directory without application.yml -> nothing found
        assert!(load(&schema(), &options).unwrap().path.is_none());
    }

    #[test]
    fn keeps_file_tree_free_of_overrides() {
        let (_dir, mut options) = options("proxy:\n  port: 8080\n");
        options.env.insert("PROXY_PORT".into(), "9999".into());
        let config = load(&schema(), &options).unwrap();
        assert_eq!(config.property("proxy.port").as_deref(), Some("9999"));
        assert_eq!(config.file_tree.unwrap(), json!({"proxy": {"port": 8080}}));
    }

    #[test]
    fn normalizes_relaxed_property_names() {
        let (_dir, options) = options(
            "proxy:\n  heartbeatRate: 5000\n  DOCKER:\n    PORT_RANGE_START: 20000\n  users:\n    - NAME: jack\n      Password: secret\n",
        );
        let config = load(&schema(), &options).unwrap();
        assert_eq!(
            config.tree["proxy"]["heartbeat-rate"],
            json!(5000),
            "tree was {:#}",
            config.tree
        );
        assert_eq!(
            config.tree["proxy"]["docker"]["port-range-start"],
            json!(20000)
        );
        assert_eq!(config.tree["proxy"]["users"][0]["name"], json!("jack"));
        assert_eq!(
            config.tree["proxy"]["users"][0]["password"],
            json!("secret")
        );
        assert!(
            config.unknown_properties.is_empty(),
            "{:?}",
            config.unknown_properties
        );
    }

    #[test]
    fn keeps_keys_of_free_form_maps_untouched() {
        let (_dir, options) = options(
            "logging:\n  level:\n    org.springframework.WEB: DEBUG\nproxy:\n  kubernetes:\n    node-selector:\n      kubernetes.io/hostName: node-1\n",
        );
        let config = load(&schema(), &options).unwrap();
        assert_eq!(
            config.tree["logging"]["level"]["org.springframework.WEB"],
            json!("DEBUG")
        );
        assert_eq!(
            config.tree["proxy"]["kubernetes"]["node-selector"]["kubernetes.io/hostName"],
            json!("node-1")
        );
        assert!(
            config.unknown_properties.is_empty(),
            "{:?}",
            config.unknown_properties
        );
    }

    #[test]
    fn boolean_properties_follow_spring_semantics() {
        let (_dir, options) = options("proxy:\n  hide-navbar: true\n  log-as-json: 'false'\n");
        let config = load(&schema(), &options).unwrap();
        assert!(config.property_bool("proxy.hide-navbar", false));
        assert!(!config.property_bool("proxy.log-as-json", true));
        assert!(config.property_bool("proxy.username-case-sensitive", true));
    }
}
