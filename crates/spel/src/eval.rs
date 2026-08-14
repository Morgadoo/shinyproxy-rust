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

//! Evaluation of the syntax tree.

use std::collections::BTreeMap;
use std::sync::Arc;

use regex::Regex;

use crate::ast::{BinaryOperator, Node, UnaryOperator};
use crate::error::{SpelError, SpelErrorKind};
use crate::value::{SpelObject, Value};

/// Maximum evaluation depth, guarding against pathological expressions.
const MAX_DEPTH: usize = 64;

/// The evaluation context: the root object, variables, beans and allowed java types.
#[derive(Debug, Clone, Default)]
pub struct Context {
    root: BTreeMap<String, Value>,
    variables: BTreeMap<String, Value>,
    beans: BTreeMap<String, Value>,
    environment: BTreeMap<String, String>,
}

impl Context {
    /// An empty context.
    pub fn new() -> Self {
        Context::default()
    }

    /// Adds a property of the root object (`proxy`, `userId`, `groups`, ...).
    pub fn with_root(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.root.insert(name.into(), value.into());
        self
    }

    /// Adds a property of the root object that is backed by an object with methods.
    pub fn with_object(mut self, name: impl Into<String>, object: Arc<dyn SpelObject>) -> Self {
        self.root.insert(name.into(), Value::Object(object));
        self
    }

    /// Adds a variable (`#name`).
    pub fn with_variable(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.variables.insert(name.into(), value.into());
        self
    }

    /// Adds a bean (`@name`).
    pub fn with_bean(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.beans.insert(name.into(), value.into());
        self
    }

    /// Adds a bean backed by an object with methods.
    pub fn with_bean_object(
        mut self,
        name: impl Into<String>,
        object: Arc<dyn SpelObject>,
    ) -> Self {
        self.beans.insert(name.into(), Value::Object(object));
        self
    }

    /// Sets the environment visible to `T(java.lang.System).getenv(...)`.
    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = environment;
        self
    }

    /// Looks up a property of the root object.
    pub fn root(&self, name: &str) -> Option<&Value> {
        self.root.get(name)
    }

    /// Names of the root properties, used in error messages.
    pub fn root_names(&self) -> Vec<&str> {
        self.root.keys().map(String::as_str).collect()
    }
}

/// Evaluates a syntax tree.
pub fn evaluate(node: &Node, context: &Context, expression: &str) -> Result<Value, SpelError> {
    Evaluator {
        context,
        expression,
        this: None,
    }
    .evaluate(node, 0)
}

struct Evaluator<'a> {
    context: &'a Context,
    expression: &'a str,
    /// Current item inside a selection or projection (`#this`).
    this: Option<&'a Value>,
}

impl<'a> Evaluator<'a> {
    fn evaluate(&self, node: &Node, depth: usize) -> Result<Value, SpelError> {
        if depth > MAX_DEPTH {
            return Err(SpelError::evaluation(
                self.expression,
                "expression is nested too deeply",
            ));
        }
        match node {
            Node::Null => Ok(Value::Null),
            Node::Bool(value) => Ok(Value::Bool(*value)),
            Node::Int(value) => Ok(Value::Int(*value)),
            Node::Float(value) => Ok(Value::Float(*value)),
            Node::Str(value) => Ok(Value::Str(value.clone())),
            Node::Root(name) => match self.context.root(name) {
                Some(value) => Ok(value.clone()),
                None => Err(SpelError::unknown(
                    self.expression,
                    format!(
                        "'{name}' is not available; available values: {}",
                        self.context.root_names().join(", ")
                    ),
                )),
            },
            Node::Variable(name) => match name.as_str() {
                "this" | "root" => match self.this.or_else(|| self.context.variables.get(name)) {
                    Some(value) => Ok(value.clone()),
                    None => Err(SpelError::unknown(
                        self.expression,
                        format!("#{name} is only available inside a selection or projection"),
                    )),
                },
                _ => self.context.variables.get(name).cloned().ok_or_else(|| {
                    SpelError::unknown(self.expression, format!("unknown variable #{name}"))
                }),
            },
            Node::Bean(name) => self.context.beans.get(name).cloned().ok_or_else(|| {
                SpelError::unknown(self.expression, format!("unknown bean @{name}"))
            }),
            Node::Type(name) => Ok(Value::Str(format!("T({name})"))),
            Node::Property { target, name, safe } => {
                let target = self.evaluate(target, depth + 1)?;
                if target.is_null() {
                    if *safe {
                        return Ok(Value::Null);
                    }
                    return Err(SpelError::evaluation(
                        self.expression,
                        format!(
                            "cannot read property '{name}' of null (use ?. for safe navigation)"
                        ),
                    ));
                }
                self.property(&target, name)
            }
            Node::Index { target, index } => {
                let target = self.evaluate(target, depth + 1)?;
                let index = self.evaluate(index, depth + 1)?;
                self.index(&target, &index)
            }
            Node::Call {
                target,
                name,
                arguments,
                safe,
            } => {
                let mut evaluated = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    evaluated.push(self.evaluate(argument, depth + 1)?);
                }
                match target {
                    None => self.call_helper(name, &evaluated),
                    Some(target) => {
                        // `T(java.lang.System).getenv('X')` and friends
                        if let Node::Type(type_name) = target.as_ref() {
                            return self.call_type(type_name, name, &evaluated);
                        }
                        let target = self.evaluate(target, depth + 1)?;
                        if target.is_null() {
                            if *safe {
                                return Ok(Value::Null);
                            }
                            return Err(SpelError::evaluation(
                                self.expression,
                                format!("cannot call '{name}' on null"),
                            ));
                        }
                        self.call_method(&target, name, &evaluated)
                    }
                }
            }
            Node::Projection { target, body } => {
                let target = self.evaluate(target, depth + 1)?;
                let items = target.as_list();
                let mut result = Vec::with_capacity(items.len());
                for item in &items {
                    result.push(self.with_this(item).evaluate(body, depth + 1)?);
                }
                Ok(Value::List(result))
            }
            Node::Selection { target, body } => {
                let target = self.evaluate(target, depth + 1)?;
                let items = target.as_list();
                let mut result = Vec::new();
                for item in &items {
                    if self.with_this(item).evaluate(body, depth + 1)?.is_truthy() {
                        result.push(item.clone());
                    }
                }
                Ok(Value::List(result))
            }
            Node::Unary { operator, operand } => {
                let value = self.evaluate(operand, depth + 1)?;
                match operator {
                    UnaryOperator::Not => Ok(Value::Bool(!value.as_bool(self.expression)?)),
                    UnaryOperator::Negate => match value {
                        Value::Int(value) => Ok(Value::Int(-value)),
                        Value::Float(value) => Ok(Value::Float(-value)),
                        other => Err(SpelError::evaluation(
                            self.expression,
                            format!("cannot negate {}", other.type_name()),
                        )),
                    },
                }
            }
            Node::Binary {
                operator,
                left,
                right,
            } => self.binary(*operator, left, right, depth),
            Node::Ternary {
                condition,
                then,
                otherwise,
            } => {
                if self
                    .evaluate(condition, depth + 1)?
                    .as_bool(self.expression)?
                {
                    self.evaluate(then, depth + 1)
                } else {
                    self.evaluate(otherwise, depth + 1)
                }
            }
            Node::Elvis { value, fallback } => {
                let value = self.evaluate(value, depth + 1)?;
                if value.is_null() {
                    self.evaluate(fallback, depth + 1)
                } else {
                    Ok(value)
                }
            }
            Node::ListLiteral(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.evaluate(item, depth + 1)?);
                }
                Ok(Value::List(values))
            }
            Node::MapLiteral(entries) => {
                let mut values = BTreeMap::new();
                for (key, value) in entries {
                    values.insert(key.clone(), self.evaluate(value, depth + 1)?);
                }
                Ok(Value::Map(values))
            }
        }
    }

    /// Selections and projections evaluate their body with `#this` bound to the current item.
    fn with_this<'b>(&'b self, value: &'b Value) -> Evaluator<'b> {
        Evaluator {
            context: self.context,
            expression: self.expression,
            this: Some(value),
        }
    }

    fn binary(
        &self,
        operator: BinaryOperator,
        left: &Node,
        right: &Node,
        depth: usize,
    ) -> Result<Value, SpelError> {
        // short circuit evaluation for and/or, as in Java
        match operator {
            BinaryOperator::And => {
                let left = self.evaluate(left, depth + 1)?.as_bool(self.expression)?;
                if !left {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(
                    self.evaluate(right, depth + 1)?.as_bool(self.expression)?,
                ));
            }
            BinaryOperator::Or => {
                let left = self.evaluate(left, depth + 1)?.as_bool(self.expression)?;
                if left {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(
                    self.evaluate(right, depth + 1)?.as_bool(self.expression)?,
                ));
            }
            _ => {}
        }

        let left = self.evaluate(left, depth + 1)?;
        let right = self.evaluate(right, depth + 1)?;

        match operator {
            BinaryOperator::Equal => Ok(Value::Bool(equals(&left, &right))),
            BinaryOperator::NotEqual => Ok(Value::Bool(!equals(&left, &right))),
            BinaryOperator::Matches => {
                let value = left.to_display_string();
                let pattern = right.to_display_string();
                let regex = Regex::new(&pattern).map_err(|error| {
                    SpelError::evaluation(
                        self.expression,
                        format!("invalid regular expression '{pattern}': {error}"),
                    )
                })?;
                // Java's `matches` requires the whole string to match.
                let anchored = format!("^(?:{pattern})$");
                let regex = Regex::new(&anchored).unwrap_or(regex);
                Ok(Value::Bool(regex.is_match(&value)))
            }
            BinaryOperator::Add => match (&left, &right) {
                (Value::Str(_), _) | (_, Value::Str(_)) => Ok(Value::Str(format!(
                    "{}{}",
                    left.to_display_string(),
                    right.to_display_string()
                ))),
                (Value::Float(_), _) | (_, Value::Float(_)) => Ok(Value::Float(
                    as_float(&left, self.expression)? + as_float(&right, self.expression)?,
                )),
                _ => Ok(Value::Int(
                    left.as_int(self.expression)? + right.as_int(self.expression)?,
                )),
            },
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo => {
                let use_float = matches!(left, Value::Float(_)) || matches!(right, Value::Float(_));
                if use_float {
                    let left = as_float(&left, self.expression)?;
                    let right = as_float(&right, self.expression)?;
                    let value = match operator {
                        BinaryOperator::Subtract => left - right,
                        BinaryOperator::Multiply => left * right,
                        BinaryOperator::Divide => left / right,
                        _ => left % right,
                    };
                    Ok(Value::Float(value))
                } else {
                    let left = left.as_int(self.expression)?;
                    let right = right.as_int(self.expression)?;
                    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Modulo)
                        && right == 0
                    {
                        return Err(SpelError::evaluation(self.expression, "division by zero"));
                    }
                    let value = match operator {
                        BinaryOperator::Subtract => left - right,
                        BinaryOperator::Multiply => left * right,
                        BinaryOperator::Divide => left / right,
                        _ => left % right,
                    };
                    Ok(Value::Int(value))
                }
            }
            BinaryOperator::Less
            | BinaryOperator::LessOrEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterOrEqual => {
                let ordering = compare(&left, &right, self.expression)?;
                let result = match operator {
                    BinaryOperator::Less => ordering.is_lt(),
                    BinaryOperator::LessOrEqual => ordering.is_le(),
                    BinaryOperator::Greater => ordering.is_gt(),
                    _ => ordering.is_ge(),
                };
                Ok(Value::Bool(result))
            }
            BinaryOperator::And | BinaryOperator::Or => unreachable!("handled above"),
        }
    }

    fn property(&self, target: &Value, name: &str) -> Result<Value, SpelError> {
        match target {
            Value::Map(entries) => Ok(entries.get(name).cloned().unwrap_or(Value::Null)),
            Value::Object(object) => object.property(name).ok_or_else(|| {
                SpelError::unknown(
                    self.expression,
                    format!("{} has no property '{name}'", object.type_name()),
                )
            }),
            // Java beans expose `length`/`size` style properties on strings and collections
            Value::Str(value) => match name {
                "length" => Ok(Value::Int(value.chars().count() as i64)),
                "empty" => Ok(Value::Bool(value.is_empty())),
                _ => Err(SpelError::unknown(
                    self.expression,
                    format!("a string has no property '{name}'"),
                )),
            },
            Value::List(items) => match name {
                "length" | "size" => Ok(Value::Int(items.len() as i64)),
                "empty" => Ok(Value::Bool(items.is_empty())),
                _ => Err(SpelError::unknown(
                    self.expression,
                    format!("a list has no property '{name}'"),
                )),
            },
            other => Err(SpelError::unknown(
                self.expression,
                format!("cannot read property '{name}' of {}", other.type_name()),
            )),
        }
    }

    fn index(&self, target: &Value, index: &Value) -> Result<Value, SpelError> {
        match target {
            Value::Map(entries) => Ok(entries
                .get(&index.to_display_string())
                .cloned()
                .unwrap_or(Value::Null)),
            Value::List(items) => {
                let position = index.as_int(self.expression)?;
                if position < 0 {
                    return Err(SpelError::evaluation(
                        self.expression,
                        format!("index {position} is negative"),
                    ));
                }
                Ok(items.get(position as usize).cloned().unwrap_or(Value::Null))
            }
            Value::Str(value) => {
                let position = index.as_int(self.expression)?;
                Ok(value
                    .chars()
                    .nth(position.max(0) as usize)
                    .map(|character| Value::Str(character.to_string()))
                    .unwrap_or(Value::Null))
            }
            Value::Object(object) => Ok(object
                .property(&index.to_display_string())
                .unwrap_or(Value::Null)),
            Value::Null => Ok(Value::Null),
            other => Err(SpelError::evaluation(
                self.expression,
                format!("cannot index {}", other.type_name()),
            )),
        }
    }

    /// Methods of the root object: the helpers of `SpecExpressionContext`.
    fn call_helper(&self, name: &str, arguments: &[Value]) -> Result<Value, SpelError> {
        match (name, arguments.len()) {
            ("toList", 1) => Ok(split_to_list(&arguments[0], ",", false)),
            ("toList", 2) => Ok(split_to_list(
                &arguments[0],
                &arguments[1].to_display_string(),
                false,
            )),
            ("toLowerCaseList", 1) => Ok(split_to_list(&arguments[0], ",", true)),
            ("toLowerCaseList", 2) => Ok(split_to_list(
                &arguments[0],
                &arguments[1].to_display_string(),
                true,
            )),
            ("isOneOf", _) if !arguments.is_empty() => Ok(Value::Bool(is_one_of(arguments, false))),
            ("isOneOfIgnoreCase", _) if !arguments.is_empty() => {
                Ok(Value::Bool(is_one_of(arguments, true)))
            }
            _ => Err(SpelError::unknown(
                self.expression,
                format!(
                    "unknown function '{name}' with {} argument(s); supported: toList, toList(regex), \
                     toLowerCaseList, toLowerCaseList(regex), isOneOf, isOneOfIgnoreCase",
                    arguments.len()
                ),
            )),
        }
    }

    /// Methods of a value (strings, lists, maps and context objects).
    fn call_method(
        &self,
        target: &Value,
        name: &str,
        arguments: &[Value],
    ) -> Result<Value, SpelError> {
        if let Value::Object(object) = target {
            if let Some(result) = object.call(name, arguments) {
                return result.map_err(|error| error.with_expression(self.expression));
            }
            // fall back to properties, so `object.getX()` style access also works via `object.x`
            if arguments.is_empty() {
                if let Some(value) = object.property(name) {
                    return Ok(value);
                }
            }
            return Err(SpelError::unknown(
                self.expression,
                format!("{} has no method '{name}'", object.type_name()),
            ));
        }

        match target {
            Value::Str(value) => self.call_string_method(value, name, arguments),
            Value::List(items) => self.call_list_method(items, name, arguments),
            Value::Map(entries) => self.call_map_method(entries, name, arguments),
            Value::Int(_) | Value::Float(_) | Value::Bool(_) => match name {
                "toString" => Ok(Value::Str(target.to_display_string())),
                "equals" if arguments.len() == 1 => Ok(Value::Bool(equals(target, &arguments[0]))),
                _ => Err(SpelError::unknown(
                    self.expression,
                    format!("{} has no method '{name}'", target.type_name()),
                )),
            },
            other => Err(SpelError::unknown(
                self.expression,
                format!("cannot call '{name}' on {}", other.type_name()),
            )),
        }
    }

    fn call_string_method(
        &self,
        value: &str,
        name: &str,
        arguments: &[Value],
    ) -> Result<Value, SpelError> {
        let argument = |index: usize| arguments[index].to_display_string();
        match (name, arguments.len()) {
            ("toString", 0) => Ok(Value::Str(value.to_string())),
            ("toLowerCase", 0) => Ok(Value::Str(value.to_lowercase())),
            ("toUpperCase", 0) => Ok(Value::Str(value.to_uppercase())),
            ("trim", 0) | ("strip", 0) => Ok(Value::Str(value.trim().to_string())),
            ("length", 0) => Ok(Value::Int(value.chars().count() as i64)),
            ("isEmpty", 0) => Ok(Value::Bool(value.is_empty())),
            ("isBlank", 0) => Ok(Value::Bool(value.trim().is_empty())),
            ("hashCode", 0) => Ok(Value::Int(java_string_hash(value))),
            ("contains", 1) => Ok(Value::Bool(value.contains(&argument(0)))),
            ("startsWith", 1) => Ok(Value::Bool(value.starts_with(&argument(0)))),
            ("endsWith", 1) => Ok(Value::Bool(value.ends_with(&argument(0)))),
            ("equals", 1) => Ok(Value::Bool(value == argument(0))),
            ("equalsIgnoreCase", 1) => Ok(Value::Bool(value.eq_ignore_ascii_case(&argument(0)))),
            ("concat", 1) => Ok(Value::Str(format!("{value}{}", argument(0)))),
            ("indexOf", 1) => Ok(Value::Int(
                value
                    .find(&argument(0))
                    .map(|index| index as i64)
                    .unwrap_or(-1),
            )),
            ("replace", 2) => Ok(Value::Str(value.replace(&argument(0), &argument(1)))),
            ("replaceAll", 2) => {
                let pattern = argument(0);
                let regex = Regex::new(&pattern).map_err(|error| {
                    SpelError::evaluation(
                        self.expression,
                        format!("invalid regular expression '{pattern}': {error}"),
                    )
                })?;
                Ok(Value::Str(
                    regex.replace_all(value, argument(1).as_str()).to_string(),
                ))
            }
            ("matches", 1) => {
                let pattern = format!("^(?:{})$", argument(0));
                let regex = Regex::new(&pattern).map_err(|error| {
                    SpelError::evaluation(
                        self.expression,
                        format!("invalid regular expression '{pattern}': {error}"),
                    )
                })?;
                Ok(Value::Bool(regex.is_match(value)))
            }
            ("split", 1) => {
                let pattern = argument(0);
                let regex = Regex::new(&pattern).map_err(|error| {
                    SpelError::evaluation(
                        self.expression,
                        format!("invalid regular expression '{pattern}': {error}"),
                    )
                })?;
                Ok(Value::List(
                    regex
                        .split(value)
                        .map(|part| Value::Str(part.to_string()))
                        .collect(),
                ))
            }
            ("substring", 1) => {
                let start = arguments[0].as_int(self.expression)?.max(0) as usize;
                Ok(Value::Str(value.chars().skip(start).collect()))
            }
            ("substring", 2) => {
                let start = arguments[0].as_int(self.expression)?.max(0) as usize;
                let end = arguments[1].as_int(self.expression)?.max(0) as usize;
                Ok(Value::Str(
                    value
                        .chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect(),
                ))
            }
            _ => Err(SpelError::unknown(
                self.expression,
                format!(
                    "a string has no method '{name}' with {} argument(s)",
                    arguments.len()
                ),
            )),
        }
    }

    fn call_list_method(
        &self,
        items: &[Value],
        name: &str,
        arguments: &[Value],
    ) -> Result<Value, SpelError> {
        match (name, arguments.len()) {
            ("size", 0) | ("length", 0) => Ok(Value::Int(items.len() as i64)),
            ("isEmpty", 0) => Ok(Value::Bool(items.is_empty())),
            ("contains", 1) => Ok(Value::Bool(
                items.iter().any(|item| equals(item, &arguments[0])),
            )),
            ("containsAll", 1) => {
                let expected = arguments[0].as_list();
                Ok(Value::Bool(expected.iter().all(|expected| {
                    items.iter().any(|item| equals(item, expected))
                })))
            }
            ("get", 1) => {
                let index = arguments[0].as_int(self.expression)?;
                Ok(items
                    .get(index.max(0) as usize)
                    .cloned()
                    .unwrap_or(Value::Null))
            }
            ("indexOf", 1) => Ok(Value::Int(
                items
                    .iter()
                    .position(|item| equals(item, &arguments[0]))
                    .map(|index| index as i64)
                    .unwrap_or(-1),
            )),
            ("stream", 0) => Ok(Value::List(items.to_vec())),
            ("toString", 0) => Ok(Value::Str(Value::List(items.to_vec()).to_display_string())),
            _ => Err(SpelError::unknown(
                self.expression,
                format!(
                    "a list has no method '{name}' with {} argument(s)",
                    arguments.len()
                ),
            )),
        }
    }

    fn call_map_method(
        &self,
        entries: &BTreeMap<String, Value>,
        name: &str,
        arguments: &[Value],
    ) -> Result<Value, SpelError> {
        match (name, arguments.len()) {
            ("size", 0) => Ok(Value::Int(entries.len() as i64)),
            ("isEmpty", 0) => Ok(Value::Bool(entries.is_empty())),
            ("get", 1) => Ok(entries
                .get(&arguments[0].to_display_string())
                .cloned()
                .unwrap_or(Value::Null)),
            ("getOrDefault", 2) => Ok(entries
                .get(&arguments[0].to_display_string())
                .cloned()
                .unwrap_or_else(|| arguments[1].clone())),
            ("containsKey", 1) => Ok(Value::Bool(
                entries.contains_key(&arguments[0].to_display_string()),
            )),
            ("containsValue", 1) => Ok(Value::Bool(
                entries.values().any(|value| equals(value, &arguments[0])),
            )),
            ("keySet", 0) => Ok(Value::List(
                entries.keys().map(|key| Value::Str(key.clone())).collect(),
            )),
            ("values", 0) => Ok(Value::List(entries.values().cloned().collect())),
            _ => Err(SpelError::unknown(
                self.expression,
                format!(
                    "a map has no method '{name}' with {} argument(s)",
                    arguments.len()
                ),
            )),
        }
    }

    /// Static methods of the allow-listed java types.
    fn call_type(
        &self,
        type_name: &str,
        method: &str,
        arguments: &[Value],
    ) -> Result<Value, SpelError> {
        match (type_name, method, arguments.len()) {
            ("java.lang.System", "getenv", 1) => {
                let name = arguments[0].to_display_string();
                Ok(self
                    .context
                    .environment
                    .get(&name)
                    .cloned()
                    .map(Value::Str)
                    .unwrap_or(Value::Null))
            }
            ("java.lang.System", "getProperty", 1) => Ok(Value::Null),
            ("java.lang.String", "valueOf", 1) => Ok(Value::Str(arguments[0].to_display_string())),
            ("java.lang.String", "join", _) if arguments.len() >= 2 => {
                let separator = arguments[0].to_display_string();
                let parts: Vec<String> = arguments[1..]
                    .iter()
                    .flat_map(|argument| match argument {
                        Value::List(items) => {
                            items.iter().map(Value::to_display_string).collect::<Vec<_>>()
                        }
                        other => vec![other.to_display_string()],
                    })
                    .collect();
                Ok(Value::Str(parts.join(&separator)))
            }
            ("java.lang.Math", "max", 2) => Ok(Value::Int(
                arguments[0]
                    .as_int(self.expression)?
                    .max(arguments[1].as_int(self.expression)?),
            )),
            ("java.lang.Math", "min", 2) => Ok(Value::Int(
                arguments[0]
                    .as_int(self.expression)?
                    .min(arguments[1].as_int(self.expression)?),
            )),
            ("java.lang.Math", "abs", 1) => Ok(Value::Int(arguments[0].as_int(self.expression)?.abs())),
            ("java.lang.Integer", "parseInt", 1) | ("java.lang.Long", "parseLong", 1) => {
                Ok(Value::Int(arguments[0].as_int(self.expression)?))
            }
            ("java.lang.Boolean", "parseBoolean", 1) => Ok(Value::Bool(
                arguments[0].to_display_string().eq_ignore_ascii_case("true"),
            )),
            _ => Err(SpelError::new(
                SpelErrorKind::Unsupported,
                self.expression,
                None,
                format!(
                    "T({type_name}).{method}(...) is not supported; supported types: java.lang.System \
                     (getenv), java.lang.String (valueOf, join), java.lang.Math (min, max, abs), \
                     java.lang.Integer/Long (parse), java.lang.Boolean (parseBoolean)"
                ),
            )),
        }
    }
}

fn as_float(value: &Value, expression: &str) -> Result<f64, SpelError> {
    match value {
        Value::Float(value) => Ok(*value),
        Value::Int(value) => Ok(*value as f64),
        other => other.as_int(expression).map(|value| value as f64),
    }
}

fn equals(left: &Value, right: &Value) -> bool {
    match (left, right) {
        // Java compares strings with numbers as unequal, but SpEL coerces when one side is a number
        (Value::Str(left), Value::Int(right)) => left.trim().parse::<i64>().ok() == Some(*right),
        (Value::Int(left), Value::Str(right)) => right.trim().parse::<i64>().ok() == Some(*left),
        _ => left == right,
    }
}

fn compare(left: &Value, right: &Value, expression: &str) -> Result<std::cmp::Ordering, SpelError> {
    match (left, right) {
        (Value::Str(left), Value::Str(right)) => Ok(left.cmp(right)),
        _ => {
            let left = as_float(left, expression)?;
            let right = as_float(right, expression)?;
            left.partial_cmp(&right).ok_or_else(|| {
                SpelError::evaluation(expression, "cannot compare these values".to_string())
            })
        }
    }
}

fn split_to_list(value: &Value, separator: &str, lowercase: bool) -> Value {
    let text = match value {
        Value::Null => return Value::List(Vec::new()),
        Value::List(items) => {
            return Value::List(
                items
                    .iter()
                    .map(|item| {
                        let text = item.to_display_string();
                        Value::Str(if lowercase {
                            text.trim().to_lowercase()
                        } else {
                            text.trim().to_string()
                        })
                    })
                    .collect(),
            )
        }
        other => other.to_display_string(),
    };
    let parts: Vec<Value> = match Regex::new(separator) {
        Ok(regex) => regex
            .split(&text)
            .map(|part| {
                Value::Str(if lowercase {
                    part.trim().to_lowercase()
                } else {
                    part.trim().to_string()
                })
            })
            .collect(),
        Err(_) => text
            .split(separator)
            .map(|part| {
                Value::Str(if lowercase {
                    part.trim().to_lowercase()
                } else {
                    part.trim().to_string()
                })
            })
            .collect(),
    };
    Value::List(parts)
}

fn is_one_of(arguments: &[Value], ignore_case: bool) -> bool {
    let Some(attribute) = arguments.first() else {
        return false;
    };
    if attribute.is_null() {
        return false;
    }
    let attribute = attribute.to_display_string();
    let attribute = attribute.trim();
    arguments[1..].iter().any(|allowed| {
        let allowed = allowed.to_display_string();
        let allowed = allowed.trim();
        if ignore_case {
            allowed.eq_ignore_ascii_case(attribute)
        } else {
            allowed == attribute
        }
    })
}

/// Java's `String.hashCode`, occasionally used in configurations to derive stable values.
fn java_string_hash(value: &str) -> i64 {
    let mut hash: i32 = 0;
    for character in value.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(character));
    }
    i64::from(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn evaluate_expression(expression: &str, context: &Context) -> Result<Value, SpelError> {
        let node = parse(expression)?;
        evaluate(&node, context, expression)
    }

    fn context() -> Context {
        Context::new()
            .with_root("userId", "jack")
            .with_root("groups", vec!["scientists", "mathematicians"])
            .with_root(
                "attributes",
                Value::Map(BTreeMap::from([
                    ("dept".to_string(), Value::Str("research".into())),
                    ("level".to_string(), Value::Int(3)),
                ])),
            )
            .with_environment(BTreeMap::from([(
                "HOME".to_string(),
                "/home/jack".to_string(),
            )]))
    }

    #[test]
    fn evaluates_literals_and_arithmetic() {
        let context = Context::new();
        assert_eq!(
            evaluate_expression("1 + 2", &context).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            evaluate_expression("7 / 2", &context).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            evaluate_expression("7.0 / 2", &context).unwrap(),
            Value::Float(3.5)
        );
        assert_eq!(
            evaluate_expression("7 % 3", &context).unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            evaluate_expression("'a' + 'b'", &context).unwrap(),
            Value::Str("ab".into())
        );
        assert_eq!(
            evaluate_expression("'count: ' + 3", &context).unwrap(),
            Value::Str("count: 3".into())
        );
        assert!(evaluate_expression("1 / 0", &context).is_err());
    }

    #[test]
    fn evaluates_comparisons_and_logic() {
        let context = context();
        assert_eq!(
            evaluate_expression("userId == 'jack'", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("userId != 'jill' and 1 < 2", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("!(userId == 'jack')", &context).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            evaluate_expression("userId matches 'ja.*'", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("userId matches 'ja'", &context).unwrap(),
            Value::Bool(false),
            "matches requires a full match, like in Java"
        );
    }

    #[test]
    fn navigates_properties_and_collections() {
        let context = context();
        assert_eq!(
            evaluate_expression("attributes['dept']", &context).unwrap(),
            Value::Str("research".into())
        );
        assert_eq!(
            evaluate_expression("attributes.dept", &context).unwrap(),
            Value::Str("research".into())
        );
        assert_eq!(
            evaluate_expression("groups[0]", &context).unwrap(),
            Value::Str("scientists".into())
        );
        assert_eq!(
            evaluate_expression("groups.size()", &context).unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            evaluate_expression("groups.contains('scientists')", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("attributes['missing']", &context).unwrap(),
            Value::Null
        );
        assert_eq!(
            evaluate_expression("attributes?.missing?.deeper", &context).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn evaluates_string_methods() {
        let context = context();
        assert_eq!(
            evaluate_expression("userId.toUpperCase()", &context).unwrap(),
            Value::Str("JACK".into())
        );
        assert_eq!(
            evaluate_expression("userId.substring(0, 2)", &context).unwrap(),
            Value::Str("ja".into())
        );
        assert_eq!(
            evaluate_expression("'a,b,c'.split(',').size()", &context).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            evaluate_expression("' x '.trim()", &context).unwrap(),
            Value::Str("x".into())
        );
        assert_eq!(
            evaluate_expression("userId.replace('ja', 'JA')", &context).unwrap(),
            Value::Str("JAck".into())
        );
    }

    #[test]
    fn evaluates_context_helpers() {
        let context = context();
        assert_eq!(
            evaluate_expression("toList('a, b')", &context).unwrap(),
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
        );
        assert_eq!(
            evaluate_expression("toLowerCaseList('A;B', ';')", &context).unwrap(),
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
        );
        assert_eq!(
            evaluate_expression("isOneOf(userId, 'jill', 'jack')", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("isOneOfIgnoreCase(userId, 'JACK')", &context).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate_expression("isOneOf(userId, 'jill')", &context).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn evaluates_ternary_elvis_projection_and_selection() {
        let context = context();
        assert_eq!(
            evaluate_expression("userId == 'jack' ? 'yes' : 'no'", &context).unwrap(),
            Value::Str("yes".into())
        );
        assert_eq!(
            evaluate_expression("attributes['missing'] ?: 'fallback'", &context).unwrap(),
            Value::Str("fallback".into())
        );
        assert_eq!(
            evaluate_expression("groups.![#this.toUpperCase()]", &context).unwrap(),
            Value::List(vec![
                Value::Str("SCIENTISTS".into()),
                Value::Str("MATHEMATICIANS".into())
            ])
        );
        assert_eq!(
            evaluate_expression("groups.?[#this == 'scientists']", &context).unwrap(),
            Value::List(vec![Value::Str("scientists".into())])
        );
    }

    #[test]
    fn evaluates_allow_listed_java_types() {
        let context = context();
        assert_eq!(
            evaluate_expression("T(java.lang.System).getenv('HOME')", &context).unwrap(),
            Value::Str("/home/jack".into())
        );
        assert_eq!(
            evaluate_expression("T(java.lang.System).getenv('MISSING')", &context).unwrap(),
            Value::Null
        );
        assert_eq!(
            evaluate_expression("T(java.lang.String).join('-', groups)", &context).unwrap(),
            Value::Str("scientists-mathematicians".into())
        );
        assert_eq!(
            evaluate_expression("T(java.lang.Math).max(2, 5)", &context).unwrap(),
            Value::Int(5)
        );

        let error =
            evaluate_expression("T(java.io.File).createTempFile('a', 'b')", &context).unwrap_err();
        assert_eq!(error.kind, SpelErrorKind::Unsupported);
        assert!(error.message.contains("not supported"), "{error}");
    }

    #[test]
    fn reports_unknown_values_with_available_alternatives() {
        let context = context();
        let error = evaluate_expression("nope", &context).unwrap_err();
        assert_eq!(error.kind, SpelErrorKind::Unknown);
        assert!(error.message.contains("available values"), "{error}");
        assert!(error.message.contains("userId"), "{error}");

        let error = evaluate_expression("userId.nope()", &context).unwrap_err();
        assert!(
            error.message.contains("a string has no method 'nope'"),
            "{error}"
        );
    }

    #[test]
    fn null_navigation_requires_safe_operator() {
        let context = Context::new().with_root("value", Value::Null);
        let error = evaluate_expression("value.name", &context).unwrap_err();
        assert!(error.message.contains("safe navigation"), "{error}");
        assert_eq!(
            evaluate_expression("value?.name", &context).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn computes_java_string_hash() {
        assert_eq!(java_string_hash("abc"), 96354);
        assert_eq!(java_string_hash(""), 0);
    }
}
