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

//! Lenient scalar types.
//!
//! Spring converts property values on demand, so `heartbeat-rate: "10000"`, `heartbeat-rate: 10000`
//! and `PROXY_HEARTBEAT_RATE=10000` all yield the same number, and `hide-navbar: "true"` is a boolean.
//! These wrapper types reproduce that leniency for serde.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

/// Boolean that also accepts the strings `true`/`false` (any case) and numbers (`0` is false).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FlexBool(pub bool);

impl From<FlexBool> for bool {
    fn from(value: FlexBool) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for FlexBool {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlexBoolVisitor;

        impl<'de> Visitor<'de> for FlexBoolVisitor {
            type Value = FlexBool;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a boolean, or a string/number that represents one")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(FlexBool(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(FlexBool(value != 0))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(FlexBool(value != 0))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                // Spring's `Boolean.parseBoolean`: everything that is not "true" is false.
                Ok(FlexBool(value.trim().eq_ignore_ascii_case("true")))
            }
        }

        deserializer.deserialize_any(FlexBoolVisitor)
    }
}

/// Signed 64 bit number that also accepts strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FlexI64(pub i64);

impl From<FlexI64> for i64 {
    fn from(value: FlexI64) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for FlexI64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlexI64Visitor;

        impl<'de> Visitor<'de> for FlexI64Visitor {
            type Value = FlexI64;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an integer, or a string that represents one")
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(FlexI64(value))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                i64::try_from(value)
                    .map(FlexI64)
                    .map_err(|_| E::custom(format!("number {value} is out of range")))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                if value.fract() == 0.0 {
                    Ok(FlexI64(value as i64))
                } else {
                    Err(E::custom(format!("expected an integer, got {value}")))
                }
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value
                    .trim()
                    .parse::<i64>()
                    .map(FlexI64)
                    .map_err(|_| E::custom(format!("expected an integer, got '{value}'")))
            }
        }

        deserializer.deserialize_any(FlexI64Visitor)
    }
}

/// List of strings that accepts a single scalar, a comma separated string, or a YAML list.
///
/// This mirrors the Java `EnvironmentUtils.readList` helper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    /// The contained values.
    pub fn values(&self) -> &[String] {
        &self.0
    }

    /// True when the list has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<StringList> for Vec<String> {
    fn from(value: StringList) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for StringList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringListVisitor;

        impl<'de> Visitor<'de> for StringListVisitor {
            type Value = StringList;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string, a comma separated string, or a list of strings")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StringList(
                    value
                        .split(',')
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                        .collect(),
                ))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StringList(vec![value.to_string()]))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StringList(vec![value.to_string()]))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StringList(vec![value.to_string()]))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StringList(Vec::new()))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<serde_json::Value>()? {
                    match value {
                        serde_json::Value::Null => {}
                        serde_json::Value::String(value) => values.push(value),
                        other => values.push(other.to_string()),
                    }
                }
                Ok(StringList(values))
            }
        }

        deserializer.deserialize_any(StringListVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Holder {
        flag: Option<FlexBool>,
        number: Option<FlexI64>,
        #[serde(default)]
        values: StringList,
    }

    fn parse(yaml: &str) -> Holder {
        serde_yaml_ng::from_str(yaml).expect("parses")
    }

    #[test]
    fn parses_lenient_booleans() {
        assert_eq!(parse("flag: true").flag, Some(FlexBool(true)));
        assert_eq!(parse("flag: 'true'").flag, Some(FlexBool(true)));
        assert_eq!(parse("flag: TRUE").flag, Some(FlexBool(true)));
        assert_eq!(parse("flag: 'yes'").flag, Some(FlexBool(false)));
        assert_eq!(parse("flag: 0").flag, Some(FlexBool(false)));
        assert_eq!(parse("number: 1").flag, None);
    }

    #[test]
    fn parses_lenient_numbers() {
        assert_eq!(parse("number: 10000").number, Some(FlexI64(10000)));
        assert_eq!(parse("number: '-1'").number, Some(FlexI64(-1)));
        assert_eq!(parse("number: 5.0").number, Some(FlexI64(5)));
    }

    #[test]
    fn parses_string_lists_in_all_notations() {
        assert_eq!(
            parse("values: scientists").values.0,
            vec!["scientists".to_string()]
        );
        assert_eq!(
            parse("values: 'a, b'").values.0,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            parse("values: [a, b]").values.0,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            parse("values:\n  - a\n  - b\n").values.0,
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(parse("number: 1").values.is_empty());
    }
}
