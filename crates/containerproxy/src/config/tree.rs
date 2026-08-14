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

//! Property tree helpers.
//!
//! Configuration is represented as a `serde_json::Value` tree (the parsed `application.yml`), on top of
//! which command line arguments and environment variables are applied. Paths use the Spring notation:
//! `proxy.docker.port-range-start`, `proxy.specs[0].container-image`.
//!
//! Property *name* matching follows Spring's relaxed binding rules: `-`/`_` are ignored and comparison is
//! case-insensitive, so `port-range-start`, `portRangeStart` and `PORT_RANGE_START` all refer to the same
//! property.

use serde_json::{Map, Value};

/// A single element of a property path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A map key.
    Key(String),
    /// An array index.
    Index(usize),
}

/// Parses a dotted property path into segments.
///
/// ```text
/// proxy.specs[0].container-image -> [Key("proxy"), Key("specs"), Index(0), Key("container-image")]
/// ```
pub fn parse_path(path: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    for raw in path.split('.') {
        if raw.is_empty() {
            continue;
        }
        let mut rest = raw;
        if let Some(bracket) = rest.find('[') {
            let (name, indexes) = rest.split_at(bracket);
            if !name.is_empty() {
                segments.push(Segment::Key(name.to_string()));
            }
            rest = indexes;
            for part in rest.split('[').filter(|part| !part.is_empty()) {
                let digits = part.trim_end_matches(']');
                match digits.parse::<usize>() {
                    Ok(index) => segments.push(Segment::Index(index)),
                    // Not an index after all (e.g. a weird key): keep it as a key.
                    Err(_) => segments.push(Segment::Key(format!("[{part}"))),
                }
            }
        } else {
            segments.push(Segment::Key(rest.to_string()));
        }
    }
    segments
}

/// Normalises a property name for relaxed comparison (lowercase, without `-` and `_`).
pub fn canonical_name(name: &str) -> String {
    name.chars()
        .filter(|character| *character != '-' && *character != '_')
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// Looks up a key in a map using relaxed name matching, returning the actual key that matched.
fn relaxed_key<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a String> {
    if map.contains_key(key) {
        return map.get_key_value(key).map(|(name, _)| name);
    }
    let wanted = canonical_name(key);
    map.keys()
        .find(|candidate| canonical_name(candidate) == wanted)
}

/// Returns the value at the given path, using relaxed name matching.
pub fn get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in parse_path(path) {
        current = match (segment, current) {
            (Segment::Key(key), Value::Object(map)) => {
                let actual = relaxed_key(map, &key)?;
                map.get(actual)?
            }
            (Segment::Index(index), Value::Array(items)) => items.get(index)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Returns the value at the given path as a string, mirroring `Environment#getProperty`.
///
/// Numbers and booleans are stringified the way Spring does when converting a property to a `String`.
pub fn get_string(root: &Value, path: &str) -> Option<String> {
    match get(root, path)? {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        other => Some(other.to_string()),
    }
}

/// Sets a value at the given path, creating intermediate maps/arrays as needed.
///
/// Existing keys are matched with relaxed name matching so that `PROXY_PORT_RANGE_START` overrides
/// `port-range-start` from the YAML file instead of adding a second entry.
pub fn set(root: &mut Value, path: &str, value: Value) {
    let segments = parse_path(path);
    if segments.is_empty() {
        *root = value;
        return;
    }
    let mut current = root;
    for (position, segment) in segments.iter().enumerate() {
        let last = position + 1 == segments.len();
        match segment {
            Segment::Key(key) => {
                if !current.is_object() {
                    *current = Value::Object(Map::new());
                }
                let map = current.as_object_mut().expect("object");
                let actual = relaxed_key(map, key)
                    .cloned()
                    .unwrap_or_else(|| key.clone());
                if last {
                    map.insert(actual, value);
                    return;
                }
                current = map.entry(actual).or_insert(Value::Null);
            }
            Segment::Index(index) => {
                if !current.is_array() {
                    *current = Value::Array(Vec::new());
                }
                let items = current.as_array_mut().expect("array");
                while items.len() <= *index {
                    items.push(Value::Null);
                }
                if last {
                    items[*index] = value;
                    return;
                }
                current = &mut items[*index];
            }
        }
    }
}

/// Deep merges `overlay` into `base`: maps are merged recursively, everything else is replaced.
pub fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let actual = relaxed_key(base_map, &key).cloned().unwrap_or(key);
                match base_map.get_mut(&actual) {
                    Some(base_value) => merge(base_value, overlay_value),
                    None => {
                        base_map.insert(actual, overlay_value);
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value,
    }
}

/// Flattens the tree into `path -> scalar` entries (used for diagnostics and unknown-key reporting).
pub fn flatten(root: &Value) -> Vec<(String, Value)> {
    let mut result = Vec::new();
    flatten_into(root, String::new(), &mut result);
    result
}

fn flatten_into(value: &Value, prefix: String, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_into(child, path, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                flatten_into(child, format!("{prefix}[{index}]"), out);
            }
        }
        scalar => out.push((prefix, scalar.clone())),
    }
}

/// Resolves a scalar into the string/bool/number best matching the textual input, like Spring does when
/// binding a property source value (everything arrives as a string from the environment).
pub fn scalar_from_str(raw: &str) -> Value {
    match raw {
        "true" | "TRUE" | "True" => return Value::Bool(true),
        "false" | "FALSE" | "False" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(number) = raw.parse::<i64>() {
        return Value::Number(number.into());
    }
    if let Ok(number) = raw.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return Value::Number(number);
        }
    }
    Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_paths() {
        assert_eq!(
            parse_path("proxy.specs[0].container-image"),
            vec![
                Segment::Key("proxy".into()),
                Segment::Key("specs".into()),
                Segment::Index(0),
                Segment::Key("container-image".into())
            ]
        );
        assert_eq!(
            parse_path("proxy.admin-groups[1]"),
            vec![
                Segment::Key("proxy".into()),
                Segment::Key("admin-groups".into()),
                Segment::Index(1)
            ]
        );
    }

    #[test]
    fn relaxed_lookup() {
        let tree = json!({"proxy": {"port-range-start": 20000, "heartbeatRate": 5000}});
        assert_eq!(
            get_string(&tree, "proxy.port-range-start").as_deref(),
            Some("20000")
        );
        assert_eq!(
            get_string(&tree, "proxy.PORT_RANGE_START").as_deref(),
            Some("20000")
        );
        assert_eq!(
            get_string(&tree, "proxy.portRangeStart").as_deref(),
            Some("20000")
        );
        assert_eq!(
            get_string(&tree, "proxy.heartbeat-rate").as_deref(),
            Some("5000")
        );
        assert_eq!(get_string(&tree, "proxy.unknown"), None);
    }

    #[test]
    fn sets_and_overrides_values() {
        let mut tree = json!({"proxy": {"port": 8080, "docker": {"port-range-start": 20000}}});
        set(&mut tree, "proxy.port", json!(9090));
        set(&mut tree, "proxy.docker.PORT_RANGE_START", json!(30000));
        set(&mut tree, "proxy.admin-groups[1]", json!("admins"));
        set(&mut tree, "spring.session.store-type", json!("redis"));

        assert_eq!(tree["proxy"]["port"], json!(9090));
        assert_eq!(tree["proxy"]["docker"]["port-range-start"], json!(30000));
        assert_eq!(tree["proxy"]["admin-groups"], json!([null, "admins"]));
        assert_eq!(tree["spring"]["session"]["store-type"], json!("redis"));
    }

    #[test]
    fn merges_trees() {
        let mut base = json!({"proxy": {"title": "a", "users": [{"name": "jack"}]}});
        merge(&mut base, json!({"proxy": {"title": "b", "port": 8081}}));
        assert_eq!(base["proxy"]["title"], json!("b"));
        assert_eq!(base["proxy"]["port"], json!(8081));
        assert_eq!(base["proxy"]["users"][0]["name"], json!("jack"));
    }

    #[test]
    fn flattens_tree() {
        let tree = json!({"proxy": {"specs": [{"id": "01_hello"}], "port": 8080}});
        let flat = flatten(&tree);
        assert!(flat.contains(&("proxy.port".to_string(), json!(8080))));
        assert!(flat.contains(&("proxy.specs[0].id".to_string(), json!("01_hello"))));
    }

    #[test]
    fn converts_scalars() {
        assert_eq!(scalar_from_str("true"), json!(true));
        assert_eq!(scalar_from_str("8080"), json!(8080));
        assert_eq!(scalar_from_str("1.5"), json!(1.5));
        assert_eq!(scalar_from_str("hello"), json!("hello"));
    }
}
