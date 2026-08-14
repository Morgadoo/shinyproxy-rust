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

//! Canonical YAML rendering, byte compatible with Jackson's `YAMLMapper`.
//!
//! ShinyProxy derives its *instance id* from a SHA-1 hash of the configuration file rendered back to
//! YAML with sorted keys (`eu.openanalytics.containerproxy.util.Sha1#hash(Object)`), so that comments
//! and key order do not influence the id. To keep instance ids identical to the Java implementation,
//! this module reproduces the exact output of
//!
//! ```java
//! YAMLMapper.builder()
//!     .configure(SerializationFeature.ORDER_MAP_ENTRIES_BY_KEYS, true)
//!     .configure(MapperFeature.SORT_PROPERTIES_ALPHABETICALLY, true)
//!     .build()
//!     .writeValueAsString(parsedConfig);
//! ```
//!
//! The rules, derived from the reference implementation (see `docs/COMPATIBILITY.md`):
//!
//! * the document starts with `---`;
//! * mappings are indented by two spaces per level and their keys are sorted;
//! * block sequences are *not* indented relative to their key, and the content of an item is indented
//!   by two spaces relative to its `-`;
//! * string values are always double quoted, numbers/booleans/nulls never are;
//! * keys are plain unless they are ambiguous: keys that look like a number, boolean or null are double
//!   quoted, keys that are not plain-safe are single quoted, and the empty key uses the explicit
//!   `? ""` form;
//! * empty mappings and sequences are rendered inline as `{}` and `[]`.

use serde_json::{Map, Value};

/// Renders the value as canonical YAML (see the module documentation).
pub fn to_canonical_yaml(value: &Value) -> String {
    let mut out = String::from("---");
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(" {}\n"),
        Value::Object(map) => {
            out.push('\n');
            write_mapping(map, 0, &mut out);
        }
        Value::Array(items) if items.is_empty() => out.push_str(" []\n"),
        Value::Array(items) => {
            out.push('\n');
            write_sequence(items, 0, &mut out);
        }
        scalar => {
            out.push(' ');
            out.push_str(&scalar_value(scalar));
            out.push('\n');
        }
    }
    out
}

/// Sorted keys of a mapping.
///
/// Java sorts with `String.compareTo`, which compares UTF-16 code units; Rust compares UTF-8 bytes,
/// which is equivalent for every character in the Basic Multilingual Plane (i.e. for every character
/// that occurs in configuration keys).
fn sorted_keys(map: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys
}

fn write_mapping(map: &Map<String, Value>, level: usize, out: &mut String) {
    let indent = "  ".repeat(level);
    for key in sorted_keys(map) {
        let value = &map[key];
        if key.is_empty() {
            // Jackson/SnakeYAML render the empty key with the explicit key indicator.
            out.push_str(&indent);
            out.push_str("? \"\"\n");
            out.push_str(&indent);
            out.push(':');
            write_value_after_key(value, level, out);
            continue;
        }
        out.push_str(&indent);
        out.push_str(&format_key(key));
        out.push(':');
        write_value_after_key(value, level, out);
    }
}

/// Writes the value that follows a `key:` (or `:`) token, including the line break.
fn write_value_after_key(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(" {}\n"),
        Value::Object(map) => {
            out.push('\n');
            write_mapping(map, level + 1, out);
        }
        Value::Array(items) if items.is_empty() => out.push_str(" []\n"),
        Value::Array(items) => {
            out.push('\n');
            // Block sequences are rendered at the same indentation as their key.
            write_sequence(items, level, out);
        }
        scalar => {
            out.push(' ');
            out.push_str(&scalar_value(scalar));
            out.push('\n');
        }
    }
}

fn write_sequence(items: &[Value], level: usize, out: &mut String) {
    let indent = "  ".repeat(level);
    for item in items {
        out.push_str(&indent);
        out.push('-');
        match item {
            Value::Object(map) if map.is_empty() => out.push_str(" {}\n"),
            Value::Object(map) => {
                // First key on the `-` line, remaining keys indented by two spaces.
                let keys = sorted_keys(map);
                let mut first = true;
                for key in keys {
                    let value = &map[key];
                    if first {
                        out.push(' ');
                        first = false;
                    } else {
                        out.push_str(&indent);
                        out.push_str("  ");
                    }
                    if key.is_empty() {
                        out.push_str("? \"\"\n");
                        out.push_str(&indent);
                        out.push_str("  :");
                    } else {
                        out.push_str(&format_key(key));
                        out.push(':');
                    }
                    write_value_after_key(value, level + 1, out);
                }
            }
            Value::Array(nested) if nested.is_empty() => out.push_str(" []\n"),
            Value::Array(nested) => {
                // `- - "a"` for the first item, the rest indented by two spaces.
                out.push(' ');
                let mut first = true;
                for nested_item in nested {
                    if first {
                        first = false;
                    } else {
                        out.push_str(&indent);
                        out.push_str("  ");
                    }
                    match nested_item {
                        Value::Object(map) if !map.is_empty() => {
                            out.push_str("- ");
                            let keys = sorted_keys(map);
                            let mut inner_first = true;
                            for key in keys {
                                if inner_first {
                                    inner_first = false;
                                } else {
                                    out.push_str(&indent);
                                    out.push_str("    ");
                                }
                                out.push_str(&format_key(key));
                                out.push(':');
                                write_value_after_key(&map[key], level + 2, out);
                            }
                        }
                        Value::Array(_) | Value::Object(_) => {
                            out.push('-');
                            write_value_after_key(nested_item, level + 1, out);
                        }
                        scalar => {
                            out.push_str("- ");
                            out.push_str(&scalar_value(scalar));
                            out.push('\n');
                        }
                    }
                }
            }
            scalar => {
                out.push(' ');
                out.push_str(&scalar_value(scalar));
                out.push('\n');
            }
        }
    }
}

/// Renders a scalar value: strings are double quoted, everything else is plain.
fn scalar_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => double_quoted(text),
        other => other.to_string(),
    }
}

/// Renders a mapping key.
fn format_key(key: &str) -> String {
    if needs_double_quotes(key) {
        double_quoted(key)
    } else if is_plain_safe(key) {
        key.to_string()
    } else {
        single_quoted(key)
    }
}

/// Keys that would be read back as a boolean, null or number must be double quoted (Jackson's
/// `StringQuotingChecker`).
fn needs_double_quotes(key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    const BOOLEAN_LIKE: &[&str] = &[
        "true", "false", "yes", "no", "on", "off", "y", "n", "TRUE", "FALSE", "YES", "NO", "ON",
        "OFF", "Y", "N", "True", "False", "Yes", "No", "On", "Off",
    ];
    const NULL_LIKE: &[&str] = &["null", "Null", "NULL", "~"];
    if BOOLEAN_LIKE.contains(&key) || NULL_LIKE.contains(&key) {
        return true;
    }
    looks_like_number(key)
}

fn looks_like_number(text: &str) -> bool {
    let candidate = text.trim();
    if candidate.is_empty() {
        return false;
    }
    candidate.parse::<i64>().is_ok()
        || candidate.parse::<f64>().is_ok()
        || candidate.parse::<i128>().is_ok()
}

/// Whether the string can be written without quotes (a conservative subset of the SnakeYAML rules,
/// which is enough for configuration keys).
fn is_plain_safe(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let first = text.chars().next().expect("non empty");
    const INDICATORS: &[char] = &[
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ];
    if INDICATORS.contains(&first) {
        return false;
    }
    if text.starts_with(' ') || text.ends_with(' ') {
        return false;
    }
    if text.contains(": ") || text.ends_with(':') || text.contains(" #") {
        return false;
    }
    !text.chars().any(|character| {
        character.is_control() || character == '\n' || character == '\t' || character == '\u{85}'
    })
}

/// Double quoted style with the escapes SnakeYAML emits.
fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}' => out.push_str("\\0"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\u{1b}' => out.push_str("\\e"),
            '\u{85}' => out.push_str("\\N"),
            '\u{a0}' => out.push_str("\\_"),
            character if character.is_control() => {
                let code = character as u32;
                if code <= 0xFF {
                    out.push_str(&format!("\\x{code:02x}"));
                } else {
                    out.push_str(&format!("\\u{code:04x}"));
                }
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Single quoted style: the only escape is a doubled quote.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_scalars_and_nesting() {
        let value = json!({
            "deep": {"a": {"b": {"c": 1}}},
            "empties": {"list": [], "map": {}},
            "nulls": {"explicit": null},
        });
        let yaml = to_canonical_yaml(&value);
        assert_eq!(
            yaml,
            "---\ndeep:\n  a:\n    b:\n      c: 1\nempties:\n  list: []\n  map: {}\nnulls:\n  explicit: null\n"
        );
    }

    #[test]
    fn renders_sequences_at_the_indentation_of_their_key() {
        let value = json!({"proxy": {"specs": [{"id": "a", "cmd": ["R", "-e"]}]}});
        let yaml = to_canonical_yaml(&value);
        assert_eq!(
            yaml,
            "---\nproxy:\n  specs:\n  - cmd:\n    - \"R\"\n    - \"-e\"\n    id: \"a\"\n"
        );
    }

    #[test]
    fn renders_nested_sequences() {
        let value = json!({"list_of_lists": [["a", "b"], ["c"]]});
        assert_eq!(
            to_canonical_yaml(&value),
            "---\nlist_of_lists:\n- - \"a\"\n  - \"b\"\n- - \"c\"\n"
        );
    }

    #[test]
    fn quotes_keys_only_when_needed() {
        assert_eq!(
            format_key("kubernetes.io/hostname"),
            "kubernetes.io/hostname"
        );
        assert_eq!(format_key("with space"), "with space");
        assert_eq!(format_key("*star"), "'*star'");
        assert_eq!(format_key("on"), "\"on\"");
        assert_eq!(format_key("y"), "\"y\"");
        assert_eq!(format_key("1"), "\"1\"");
        assert_eq!(format_key("null"), "\"null\"");
        assert_eq!(format_key("plain"), "plain");
    }

    #[test]
    fn escapes_strings_like_snakeyaml() {
        assert_eq!(double_quoted("he said \"hi\""), "\"he said \\\"hi\\\"\"");
        assert_eq!(double_quoted("line1\nline2"), "\"line1\\nline2\"");
        assert_eq!(double_quoted("a\tb"), "\"a\\tb\"");
        assert_eq!(double_quoted("a\\b"), "\"a\\\\b\"");
        assert_eq!(double_quoted("héllo → ✓"), "\"héllo → ✓\"");
    }
}
