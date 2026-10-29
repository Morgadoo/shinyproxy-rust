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

use std::collections::BTreeMap;
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

/// String that also accepts numbers and booleans.
///
/// Spring converts property values on demand, so `default-max-instances: 2` and `logo-width: 100` are
/// valid even though the properties are strings (they may contain expressions).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FlexString(pub String);

impl FlexString {
    /// The value as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<FlexString> for String {
    fn from(value: FlexString) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for FlexString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlexStringVisitor;

        impl<'de> Visitor<'de> for FlexStringVisitor {
            type Value = FlexString;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string, number or boolean")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(FlexString(value.to_string()))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(FlexString(value.to_string()))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(FlexString(value.to_string()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(FlexString(value.to_string()))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(FlexString(value.to_string()))
            }
        }

        deserializer.deserialize_any(FlexStringVisitor)
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

/// `Option<String>` field that also accepts numbers and booleans, for `#[serde(deserialize_with)]`.
///
/// The Spring binder converts `uid: 1000` or `default-value: 2020` into the `String` fields of the Java
/// model, so the YAML of a Java configuration often leaves them unquoted.
pub fn opt_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<FlexString>::deserialize(deserializer).map(|value| value.map(String::from))
}

/// String map whose values may also be numbers and booleans, for `#[serde(deserialize_with)]`.
pub fn string_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    Option::<BTreeMap<String, Option<FlexString>>>::deserialize(deserializer).map(|map| {
        map.unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.map(String::from).unwrap_or_default()))
            .collect()
    })
}

/// `Option<bool>` field bound the way Spring's `StringToBooleanConverter` does, for
/// `#[serde(deserialize_with)]`: `true`/`on`/`yes`/`1` and `false`/`off`/`no`/`0`, in any case. A YAML
/// 1.1 parser (SnakeYAML) reads an unquoted `yes` as a boolean; here it arrives as a string, and a
/// placeholder such as `${TRACK_URL:true}` always does.
pub fn opt_spring_bool<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    match Option::<FlexString>::deserialize(deserializer)? {
        None => Ok(None),
        Some(FlexString(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "" => Ok(None),
            "true" | "on" | "yes" | "1" => Ok(Some(true)),
            "false" | "off" | "no" | "0" => Ok(Some(false)),
            _ => Err(de::Error::custom(format!(
                "invalid boolean value '{value}'"
            ))),
        },
    }
}

/// An enum that is bound the way Spring's lenient enum converter does: the exact name first, then the
/// name with only its letters and digits, ignoring case (`passthrough`, `PASSTHROUGH` and `Passthrough`
/// are the same, as are `ENFORCE_NO_CACHE` and `EnforceNoCache`).
pub trait LenientEnum: Sized + Copy + 'static {
    /// The names of the constants, as written in the Java enum.
    const VARIANTS: &'static [(&'static str, Self)];

    /// The constant with this name, if any.
    fn parse_lenient(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some((_, variant)) = Self::VARIANTS.iter().find(|(name, _)| *name == value) {
            return Some(*variant);
        }
        let canonical = canonical_enum_name(value);
        Self::VARIANTS
            .iter()
            .find(|(name, _)| canonical_enum_name(name) == canonical)
            .map(|(_, variant)| *variant)
    }

    /// The name of the constant, as written in the Java enum.
    fn name(&self) -> &'static str
    where
        Self: PartialEq,
    {
        Self::VARIANTS
            .iter()
            .find(|(_, variant)| variant == self)
            .map(|(name, _)| *name)
            .unwrap_or_default()
    }
}

/// Only the letters and digits of a name, in lower case (`getCanonicalName` of Spring's converter).
fn canonical_enum_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Deserializes a [`LenientEnum`], for its `Deserialize` implementation.
pub fn deserialize_lenient_enum<'de, D: Deserializer<'de>, T: LenientEnum>(
    deserializer: D,
) -> Result<T, D::Error> {
    let value = FlexString::deserialize(deserializer)?.0;
    T::parse_lenient(&value).ok_or_else(|| {
        let names: Vec<&str> = T::VARIANTS.iter().map(|(name, _)| *name).collect();
        de::Error::custom(format!(
            "invalid value '{value}', expected one of {}",
            names.join(", ")
        ))
    })
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
        text: Option<FlexString>,
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
    fn parses_lenient_strings() {
        assert_eq!(parse("text: abc").text, Some(FlexString("abc".into())));
        assert_eq!(parse("text: 2").text, Some(FlexString("2".into())));
        assert_eq!(parse("text: true").text, Some(FlexString("true".into())));
        assert_eq!(parse("text: 1.5").text, Some(FlexString("1.5".into())));
        assert_eq!(parse("number: 1").text, None);
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

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Coerced {
        #[serde(deserialize_with = "opt_string")]
        text: Option<String>,
        #[serde(deserialize_with = "string_map")]
        map: BTreeMap<String, String>,
        #[serde(deserialize_with = "opt_spring_bool")]
        flag: Option<bool>,
    }

    #[test]
    fn coerces_scalars_like_the_spring_binder() {
        let coerced: Coerced =
            serde_yaml_ng::from_str("text: 1000\nmap: {order: 1, on: true, empty: ~}\nflag: 'yes'")
                .expect("parses");
        assert_eq!(coerced.text.as_deref(), Some("1000"));
        assert_eq!(coerced.map.get("order").map(String::as_str), Some("1"));
        assert_eq!(coerced.map.get("on").map(String::as_str), Some("true"));
        assert_eq!(coerced.map.get("empty").map(String::as_str), Some(""));
        assert_eq!(coerced.flag, Some(true));

        let absent: Coerced = serde_yaml_ng::from_str("{}").expect("parses");
        assert_eq!(absent.text, None);
        assert_eq!(absent.flag, None);
        for (value, expected) in [("'OFF'", false), ("'1'", true), ("false", false)] {
            let parsed: Coerced =
                serde_yaml_ng::from_str(&format!("flag: {value}")).expect("parses");
            assert_eq!(parsed.flag, Some(expected), "{value}");
        }
        assert!(serde_yaml_ng::from_str::<Coerced>("flag: maybe").is_err());
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Mode {
        EnforceNoCache,
        Passthrough,
    }

    impl LenientEnum for Mode {
        const VARIANTS: &'static [(&'static str, Self)] = &[
            ("EnforceNoCache", Mode::EnforceNoCache),
            ("Passthrough", Mode::Passthrough),
        ];
    }

    #[test]
    fn matches_enum_names_leniently() {
        assert_eq!(Mode::parse_lenient("Passthrough"), Some(Mode::Passthrough));
        assert_eq!(Mode::parse_lenient("passthrough"), Some(Mode::Passthrough));
        assert_eq!(
            Mode::parse_lenient("ENFORCE_NO_CACHE"),
            Some(Mode::EnforceNoCache)
        );
        assert_eq!(
            Mode::parse_lenient("enforce-no-cache"),
            Some(Mode::EnforceNoCache)
        );
        assert_eq!(Mode::parse_lenient("Other"), None);
        assert_eq!(Mode::Passthrough.name(), "Passthrough");
    }
}
