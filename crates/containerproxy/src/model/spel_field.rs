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

//! Fields of an app definition that may contain SpEL expressions (`SpelField` in the Java model).
//!
//! A field starts out *unresolved*, holding the value exactly as it appears in `application.yml`. When
//! a proxy is started, the expressions are evaluated against the current user/proxy/spec and the field
//! becomes *resolved*. Reading the value of an unresolved field is a programming error, which the Java
//! implementation signals with an exception and this implementation with `None`/`expect`.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

/// Evaluates the expressions of a spec against a context (implemented on top of the `spel` crate).
pub trait SpecResolver {
    /// Evaluates a value that may contain `#{...}` expressions into a string.
    fn string(&self, raw: &str) -> Result<String, ResolveError>;

    /// Evaluates into an integer.
    fn integer(&self, raw: &str) -> Result<i64, ResolveError>;

    /// Evaluates into a boolean.
    fn boolean(&self, raw: &str) -> Result<bool, ResolveError>;
}

/// Error while evaluating an expression of an app definition.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("cannot evaluate expression '{expression}': {message}")]
pub struct ResolveError {
    /// The expression that failed.
    pub expression: String,
    /// Why it failed.
    pub message: String,
}

impl ResolveError {
    /// Creates an error for the given expression.
    pub fn new(expression: impl Into<String>, message: impl Into<String>) -> Self {
        ResolveError {
            expression: expression.into(),
            message: message.into(),
        }
    }
}

/// A field whose value may contain expressions.
///
/// `O` is the type as written in the configuration file, `R` the type after evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spel<O, R> {
    original: Option<O>,
    value: Option<R>,
    resolved: bool,
}

impl<O, R> Default for Spel<O, R> {
    fn default() -> Self {
        Spel {
            original: None,
            value: None,
            resolved: false,
        }
    }
}

impl<O, R> Spel<O, R> {
    /// A field with the given original (unresolved) value.
    pub fn raw(original: O) -> Self {
        Spel {
            original: Some(original),
            value: None,
            resolved: false,
        }
    }

    /// An empty field.
    pub fn empty() -> Self {
        Spel::default()
    }

    /// A field that is already resolved (used in tests and for values that need no evaluation).
    pub fn resolved(original: O, value: R) -> Self {
        Spel {
            original: Some(original),
            value: Some(value),
            resolved: true,
        }
    }

    /// The value as written in the configuration file.
    pub fn original(&self) -> Option<&O> {
        self.original.as_ref()
    }

    /// Whether the field has been resolved.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }

    /// Whether the configuration file contains a value for this field.
    pub fn is_present(&self) -> bool {
        self.original.is_some()
    }

    /// The resolved value, if the field was resolved and has a value.
    pub fn value(&self) -> Option<&R> {
        debug_assert!(
            self.resolved || self.original.is_none(),
            "reading the value of an unresolved SpEL field"
        );
        self.value.as_ref()
    }

    /// The resolved value or the given default.
    pub fn value_or<'a>(&'a self, default: &'a R) -> &'a R {
        self.value().unwrap_or(default)
    }
}

impl<O, R: Clone> Spel<O, R> {
    /// The resolved value or the given default, cloned.
    pub fn value_or_default(&self, default: R) -> R {
        self.value().cloned().unwrap_or(default)
    }
}

/// A string field.
pub type SpelString = Spel<String, String>;
/// An integer field.
pub type SpelLong = Spel<String, i64>;
/// A boolean field.
pub type SpelBool = Spel<String, bool>;
/// A list-of-strings field.
pub type SpelStringList = Spel<Vec<String>, Vec<String>>;
/// A map-of-strings field.
pub type SpelStringMap = Spel<BTreeMap<String, String>, BTreeMap<String, String>>;

impl SpelString {
    /// Evaluates the field.
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        match &self.original {
            None => Ok(Spel {
                original: None,
                value: None,
                resolved: true,
            }),
            Some(original) => Ok(Spel {
                original: Some(original.clone()),
                value: Some(resolver.string(original)?),
                resolved: true,
            }),
        }
    }

    /// The resolved value as a `&str`.
    pub fn as_str(&self) -> Option<&str> {
        self.value().map(String::as_str)
    }
}

impl SpelLong {
    /// Evaluates the field.
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        match &self.original {
            None => Ok(Spel {
                original: None,
                value: None,
                resolved: true,
            }),
            Some(original) => Ok(Spel {
                original: Some(original.clone()),
                value: Some(resolver.integer(original)?),
                resolved: true,
            }),
        }
    }
}

impl SpelBool {
    /// Evaluates the field.
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        match &self.original {
            None => Ok(Spel {
                original: None,
                value: None,
                resolved: true,
            }),
            Some(original) => Ok(Spel {
                original: Some(original.clone()),
                value: Some(resolver.boolean(original)?),
                resolved: true,
            }),
        }
    }
}

impl SpelStringList {
    /// Evaluates every element of the list.
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        match &self.original {
            None => Ok(Spel {
                original: None,
                value: None,
                resolved: true,
            }),
            Some(original) => {
                let mut values = Vec::with_capacity(original.len());
                for item in original {
                    values.push(resolver.string(item)?);
                }
                Ok(Spel {
                    original: Some(original.clone()),
                    value: Some(values),
                    resolved: true,
                })
            }
        }
    }
}

impl SpelStringMap {
    /// Evaluates every value of the map (keys are never expressions, as in Java).
    pub fn resolve(&self, resolver: &dyn SpecResolver) -> Result<Self, ResolveError> {
        match &self.original {
            None => Ok(Spel {
                original: None,
                value: None,
                resolved: true,
            }),
            Some(original) => {
                let mut values = BTreeMap::new();
                for (key, value) in original {
                    values.insert(key.clone(), resolver.string(value)?);
                }
                Ok(Spel {
                    original: Some(original.clone()),
                    value: Some(values),
                    resolved: true,
                })
            }
        }
    }
}

/// Serializes as the resolved value when available, else as the original value (`@JsonValue` on
/// `SpelField#toJson`).
impl<O: Serialize, R: Serialize> Serialize for Spel<O, R> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match (&self.value, &self.original) {
            (Some(value), _) => value.serialize(serializer),
            (None, Some(original)) => original.serialize(serializer),
            (None, None) => serializer.serialize_none(),
        }
    }
}

/// Accepts a string as well as numbers and booleans (`container-memory-limit: 2` is valid YAML).
impl<'de, R> Deserialize<'de> for Spel<String, R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LenientString<R>(std::marker::PhantomData<R>);

        impl<'de, R> Visitor<'de> for LenientString<R> {
            type Value = Spel<String, R>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string, number or boolean")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Spel::raw(value.to_string()))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(Spel::raw(value.to_string()))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(Spel::raw(value.to_string()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(Spel::raw(value.to_string()))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(Spel::raw(value.to_string()))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Spel::empty())
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Spel::empty())
            }
        }

        deserializer.deserialize_any(LenientString(std::marker::PhantomData))
    }
}

impl<'de> Deserialize<'de> for SpelStringList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = crate::config::flex::StringList::deserialize(deserializer)?;
        Ok(Spel::raw(values.0))
    }
}

impl<'de> Deserialize<'de> for SpelStringMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LenientMap;

        impl<'de> Visitor<'de> for LenientMap {
            type Value = SpelStringMap;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of strings")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Spel::empty())
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                    let value = match value {
                        serde_json::Value::String(value) => value,
                        serde_json::Value::Null => continue,
                        other => other.to_string(),
                    };
                    values.insert(key, value);
                }
                Ok(Spel::raw(values))
            }
        }

        deserializer.deserialize_any(LenientMap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolver that upper-cases `#{...}` expressions, enough to test the plumbing.
    struct FakeResolver;

    impl SpecResolver for FakeResolver {
        fn string(&self, raw: &str) -> Result<String, ResolveError> {
            if let Some(inner) = raw
                .strip_prefix("#{")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                if inner == "fail" {
                    return Err(ResolveError::new(raw, "no such property"));
                }
                return Ok(inner.to_uppercase());
            }
            Ok(raw.to_string())
        }

        fn integer(&self, raw: &str) -> Result<i64, ResolveError> {
            self.string(raw)?
                .parse()
                .map_err(|_| ResolveError::new(raw, "not a number"))
        }

        fn boolean(&self, raw: &str) -> Result<bool, ResolveError> {
            Ok(self.string(raw)?.eq_ignore_ascii_case("true"))
        }
    }

    #[test]
    fn resolves_strings_numbers_booleans_lists_and_maps() {
        let string: SpelString = serde_yaml_ng::from_str("'#{userId}'").unwrap();
        assert_eq!(
            string.resolve(&FakeResolver).unwrap().as_str(),
            Some("USERID")
        );

        let number: SpelLong = serde_yaml_ng::from_str("120").unwrap();
        assert_eq!(number.resolve(&FakeResolver).unwrap().value(), Some(&120));

        let boolean: SpelBool = serde_yaml_ng::from_str("true").unwrap();
        assert_eq!(boolean.resolve(&FakeResolver).unwrap().value(), Some(&true));

        let list: SpelStringList = serde_yaml_ng::from_str("['a', '#{b}']").unwrap();
        assert_eq!(
            list.resolve(&FakeResolver).unwrap().value(),
            Some(&vec!["a".to_string(), "B".to_string()])
        );

        let map: SpelStringMap = serde_yaml_ng::from_str("{KEY: '#{value}', OTHER: 3}").unwrap();
        let resolved = map.resolve(&FakeResolver).unwrap();
        assert_eq!(
            resolved.value().unwrap().get("KEY").map(String::as_str),
            Some("VALUE")
        );
        assert_eq!(
            resolved.value().unwrap().get("OTHER").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn empty_fields_stay_empty() {
        let field = SpelString::empty();
        let resolved = field.resolve(&FakeResolver).unwrap();
        assert!(resolved.is_resolved());
        assert!(!resolved.is_present());
        assert_eq!(resolved.value(), None);
        assert_eq!(
            resolved.value_or_default("fallback".to_string()),
            "fallback"
        );
    }

    #[test]
    fn reports_resolution_errors() {
        let field = SpelString::raw("#{fail}".to_string());
        let error = field.resolve(&FakeResolver).unwrap_err();
        assert_eq!(error.expression, "#{fail}");
    }

    #[test]
    fn serializes_resolved_value_or_original() {
        let field = SpelString::raw("#{userId}".to_string());
        assert_eq!(
            serde_json::to_value(&field).unwrap(),
            serde_json::json!("#{userId}")
        );
        let resolved = field.resolve(&FakeResolver).unwrap();
        assert_eq!(
            serde_json::to_value(&resolved).unwrap(),
            serde_json::json!("USERID")
        );
        assert_eq!(
            serde_json::to_value(SpelString::empty()).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn accepts_lenient_scalars() {
        let field: SpelString = serde_yaml_ng::from_str("2g").unwrap();
        assert_eq!(field.original().map(String::as_str), Some("2g"));
        let field: SpelString = serde_yaml_ng::from_str("1").unwrap();
        assert_eq!(field.original().map(String::as_str), Some("1"));
        let list: SpelStringList = serde_yaml_ng::from_str("'a, b'").unwrap();
        assert_eq!(
            list.original().unwrap(),
            &vec!["a".to_string(), "b".to_string()]
        );
    }
}
