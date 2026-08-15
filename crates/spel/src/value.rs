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

//! Values an expression can produce, with Java-compatible conversions.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::{SpelError, SpelErrorKind};

/// An object of the evaluation context that exposes properties and methods.
///
/// The engine implements this for proxies, app definitions and users, so that expressions such as
/// `proxy.getRuntimeValue('SHINYPROXY_USERNAME')` behave like in the Java implementation.
pub trait SpelObject: fmt::Debug + Send + Sync {
    /// Name of the type, used in error messages.
    fn type_name(&self) -> &'static str;

    /// Value of a property (`object.property`).
    fn property(&self, name: &str) -> Option<Value>;

    /// Result of a method call (`object.method(args)`).
    ///
    /// Returning `None` means "no such method", which produces an `unknown property or method` error.
    fn call(&self, _name: &str, _arguments: &[Value]) -> Option<Result<Value, SpelError>> {
        None
    }

    /// String representation (`toString()`).
    fn to_display(&self) -> String {
        self.type_name().to_string()
    }
}

/// A value produced by an expression.
#[derive(Debug, Clone, Default)]
pub enum Value {
    /// `null`
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer (Java `int`/`long`).
    Int(i64),
    /// A floating point number (Java `double`).
    Float(f64),
    /// A string.
    Str(String),
    /// A list.
    List(Vec<Value>),
    /// A map with string keys.
    Map(BTreeMap<String, Value>),
    /// An object of the context.
    Object(Arc<dyn SpelObject>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(left), Value::Bool(right)) => left == right,
            (Value::Int(left), Value::Int(right)) => left == right,
            (Value::Float(left), Value::Float(right)) => left == right,
            (Value::Int(left), Value::Float(right)) | (Value::Float(right), Value::Int(left)) => {
                (*left as f64) == *right
            }
            (Value::Str(left), Value::Str(right)) => left == right,
            (Value::List(left), Value::List(right)) => left == right,
            (Value::Map(left), Value::Map(right)) => left == right,
            (Value::Object(left), Value::Object(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Value {
    /// Name of the type, used in error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Object(object) => object.type_name(),
        }
    }

    /// Whether the value is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// String representation, following Java's `String.valueOf`.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => format_double(*value),
            Value::Str(value) => value.clone(),
            Value::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Value::to_display_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::Map(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, value)| format!("{key}={}", value.to_display_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::Object(object) => object.to_display(),
        }
    }

    /// Converts to a string, failing only for objects that have no representation.
    pub fn as_string(&self, expression: &str) -> Result<String, SpelError> {
        match self {
            Value::Null => Ok(String::new()),
            other => Ok(other.to_display_string()),
        }
        .map_err(|error: SpelError| error.with_expression(expression))
    }

    /// Converts to a boolean, following Java: only booleans and the strings `true`/`false` are valid.
    pub fn as_bool(&self, expression: &str) -> Result<bool, SpelError> {
        match self {
            Value::Bool(value) => Ok(*value),
            Value::Str(value) if value.eq_ignore_ascii_case("true") => Ok(true),
            Value::Str(value) if value.eq_ignore_ascii_case("false") => Ok(false),
            Value::Null => Ok(false),
            other => Err(SpelError::new(
                SpelErrorKind::Evaluation,
                expression,
                None,
                format!(
                    "expected a boolean but got {} ({})",
                    other.type_name(),
                    other.to_display_string()
                ),
            )),
        }
    }

    /// Converts to an integer.
    pub fn as_int(&self, expression: &str) -> Result<i64, SpelError> {
        match self {
            Value::Int(value) => Ok(*value),
            Value::Float(value) => Ok(*value as i64),
            Value::Bool(value) => Ok(i64::from(*value)),
            Value::Str(value) => value.trim().parse::<i64>().or_else(|_| {
                value
                    .trim()
                    .parse::<f64>()
                    .map(|value| value as i64)
                    .map_err(|_| {
                        SpelError::new(
                            SpelErrorKind::Evaluation,
                            expression,
                            None,
                            format!("expected a number but got '{value}'"),
                        )
                    })
            }),
            other => Err(SpelError::new(
                SpelErrorKind::Evaluation,
                expression,
                None,
                format!("expected a number but got {}", other.type_name()),
            )),
        }
    }

    /// Converts to a list: lists stay as they are, `null` becomes empty, everything else is wrapped.
    pub fn as_list(&self) -> Vec<Value> {
        match self {
            Value::List(items) => items.clone(),
            Value::Null => Vec::new(),
            other => vec![other.clone()],
        }
    }

    /// Truthiness used by `?:` and by `if`-like constructs: `null` and `false` are falsy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Str(value) => !value.is_empty(),
            Value::Int(value) => *value != 0,
            Value::Float(value) => *value != 0.0,
            Value::List(items) => !items.is_empty(),
            Value::Map(entries) => !entries.is_empty(),
            Value::Object(_) => true,
        }
    }
}

/// Formats a double the way Java's `Double.toString` does for the common cases.
fn format_double(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e7 {
        format!("{value:.1}")
    } else {
        let formatted = format!("{value}");
        if formatted.contains(['e', 'E']) {
            // Java uses E notation with an explicit exponent sign for large/small numbers.
            formatted.replace('e', "E")
        } else {
            formatted
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_string())
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(values: Vec<T>) -> Self {
        Value::List(values.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Value::Null,
        }
    }
}

impl From<BTreeMap<String, String>> for Value {
    fn from(values: BTreeMap<String, String>) -> Self {
        Value::Map(
            values
                .into_iter()
                .map(|(key, value)| (key, Value::Str(value)))
                .collect(),
        )
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(value) => Value::Bool(value),
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(value) => Value::Int(value),
                None => Value::Float(number.as_f64().unwrap_or_default()),
            },
            serde_json::Value::String(value) => Value::Str(value),
            serde_json::Value::Array(items) => {
                Value::List(items.into_iter().map(Value::from).collect())
            }
            serde_json::Value::Object(entries) => Value::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, Value::from(value)))
                    .collect(),
            ),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_display_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_strings_like_java() {
        assert_eq!(Value::Str("a".into()).to_display_string(), "a");
        assert_eq!(Value::Int(5).to_display_string(), "5");
        assert_eq!(Value::Float(1.5).to_display_string(), "1.5");
        assert_eq!(Value::Float(1000.0).to_display_string(), "1000.0");
        assert_eq!(Value::Bool(true).to_display_string(), "true");
        assert_eq!(Value::Null.to_display_string(), "null");
        assert_eq!(
            Value::List(vec![Value::Int(1), Value::Str("a".into())]).to_display_string(),
            "[1, a]"
        );
    }

    #[test]
    fn converts_to_booleans_and_numbers() {
        assert!(Value::Bool(true).as_bool("").unwrap());
        assert!(Value::Str("TRUE".into()).as_bool("").unwrap());
        assert!(!Value::Str("false".into()).as_bool("").unwrap());
        assert!(!Value::Null.as_bool("").unwrap());
        assert!(Value::Int(1).as_bool("").is_err());

        assert_eq!(Value::Int(5).as_int("").unwrap(), 5);
        assert_eq!(Value::Str("42".into()).as_int("").unwrap(), 42);
        assert_eq!(Value::Float(2.7).as_int("").unwrap(), 2);
        assert!(Value::Str("abc".into()).as_int("").is_err());
    }

    #[test]
    fn equality_across_numeric_types() {
        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_ne!(Value::Int(1), Value::Str("1".into()));
        assert_eq!(
            Value::List(vec![Value::Int(1)]),
            Value::List(vec![Value::Int(1)])
        );
    }

    #[test]
    fn converts_from_json() {
        let json = serde_json::json!({"a": 1, "b": ["x", true], "c": null});
        let value = Value::from(json);
        match value {
            Value::Map(entries) => {
                assert_eq!(entries.get("a"), Some(&Value::Int(1)));
                assert_eq!(
                    entries.get("b"),
                    Some(&Value::List(vec![
                        Value::Str("x".into()),
                        Value::Bool(true)
                    ]))
                );
                assert_eq!(entries.get("c"), Some(&Value::Null));
            }
            other => panic!("expected a map, got {other:?}"),
        }
    }
}
