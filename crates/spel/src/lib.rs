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

//! A Spring-Expression-Language (SpEL) compatible expression engine.
//!
//! ShinyProxy configuration files may contain `#{ ... }` expressions in most string valued properties
//! (container environment variables, access control expressions, titles, memory limits, ...). This
//! crate implements the subset of SpEL that those configurations use:
//!
//! * literals (`'text'`, `42`, `1.5`, `true`, `null`), string concatenation and arithmetic;
//! * property navigation (`proxy.userId`), safe navigation (`user?.email`), indexing
//!   (`attributes['dept']`, `groups[0]`);
//! * method calls on strings, lists, maps and context objects (`proxy.getRuntimeValue('...')`);
//! * the helper functions of `SpecExpressionContext`: `toList`, `toLowerCaseList`, `isOneOf`,
//!   `isOneOfIgnoreCase`;
//! * comparisons (`==`, `!=`, `<`, `matches`, ...), logical operators (`and`, `or`, `!`), the ternary
//!   and elvis operators, inline lists/maps, projection (`.![...]`) and selection (`.?[...]`);
//! * a small allow list of static java methods (`T(java.lang.System).getenv('HOME')`).
//!
//! Anything outside that subset produces an error that names the offending construct, so that
//! configuration mistakes surface at startup instead of silently changing behaviour.
//!
//! ```
//! use spel::{Context, Expression};
//!
//! let context = Context::new().with_root("userId", "jack");
//! let value = Expression::parse("userId.toUpperCase()").unwrap().evaluate(&context).unwrap();
//! assert_eq!(value.to_display_string(), "JACK");
//!
//! // template expressions are the usual form in application.yml
//! assert_eq!(spel::evaluate_template("app-#{userId}", &context).unwrap(), "app-jack");
//! ```

#![forbid(unsafe_code)]

pub mod ast;
pub mod error;
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod template;
pub mod value;

pub use error::{SpelError, SpelErrorKind};
pub use eval::Context;
pub use template::contains_expression;
pub use value::{SpelObject, Value};

/// A parsed expression, ready to be evaluated (possibly many times).
#[derive(Debug, Clone)]
pub struct Expression {
    source: String,
    node: ast::Node,
}

impl Expression {
    /// Parses an expression (without the `#{}` delimiters).
    pub fn parse(expression: &str) -> Result<Self, SpelError> {
        Ok(Expression {
            source: expression.to_string(),
            node: parser::parse(expression)?,
        })
    }

    /// The expression as written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Evaluates the expression.
    pub fn evaluate(&self, context: &Context) -> Result<Value, SpelError> {
        eval::evaluate(&self.node, context, &self.source)
    }
}

/// Evaluates a template (text with embedded `#{...}` expressions) into a string.
///
/// Values without `#{` are returned unchanged, which is both faster and exactly what the Java
/// implementation does.
pub fn evaluate_template(template: &str, context: &Context) -> Result<String, SpelError> {
    if !contains_expression(template) {
        return Ok(template.to_string());
    }
    let parts = template::split(template)?;
    let mut result = String::with_capacity(template.len());
    for part in parts {
        match part {
            template::Part::Text(text) => result.push_str(&text),
            template::Part::Expression(expression) => {
                let value = evaluate_part(&expression, template, context)?;
                result.push_str(&value.to_display_string());
            }
        }
    }
    Ok(result)
}

/// Evaluates a template into a value.
///
/// When the template consists of a single expression, its value is returned as-is (so that
/// `#{1 + 1}` yields the number 2 instead of the string "2"); otherwise the parts are concatenated.
pub fn evaluate_template_to_value(template: &str, context: &Context) -> Result<Value, SpelError> {
    if !contains_expression(template) {
        return Ok(Value::Str(template.to_string()));
    }
    let parts = template::split(template)?;
    if let [template::Part::Expression(expression)] = parts.as_slice() {
        return evaluate_part(expression, template, context);
    }
    evaluate_template(template, context).map(Value::Str)
}

/// Evaluates one `#{...}` part of a template, reporting errors against the whole template so that the
/// message points at the configuration value the user wrote.
fn evaluate_part(expression: &str, template: &str, context: &Context) -> Result<Value, SpelError> {
    let attach = |mut error: SpelError| {
        if error.expression != template {
            error.message = format!("{} (in expression '{}')", error.message, error.expression);
            error.expression = template.to_string();
        }
        error
    };
    let parsed = Expression::parse(expression).map_err(attach)?;
    parsed.evaluate(context).map_err(attach)
}

/// Evaluates a template and converts the result into a string.
pub fn evaluate_to_string(template: &str, context: &Context) -> Result<String, SpelError> {
    evaluate_template(template, context)
}

/// Evaluates a template and converts the result into an integer.
pub fn evaluate_to_integer(template: &str, context: &Context) -> Result<i64, SpelError> {
    evaluate_template_to_value(template, context)?.as_int(template)
}

/// Evaluates a template and converts the result into a boolean.
pub fn evaluate_to_boolean(template: &str, context: &Context) -> Result<bool, SpelError> {
    evaluate_template_to_value(template, context)?.as_bool(template)
}

/// Evaluates a template and converts the result into a list of strings.
pub fn evaluate_to_list(template: &str, context: &Context) -> Result<Vec<String>, SpelError> {
    let value = evaluate_template_to_value(template, context)?;
    Ok(value
        .as_list()
        .into_iter()
        .map(|item| item.to_display_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context::new()
            .with_root("userId", "jack")
            .with_root("groups", vec!["scientists"])
    }

    #[test]
    fn evaluates_templates() {
        let context = context();
        assert_eq!(
            evaluate_template("plain text", &context).unwrap(),
            "plain text"
        );
        assert_eq!(evaluate_template("#{userId}", &context).unwrap(), "jack");
        assert_eq!(
            evaluate_template("/home/#{userId}/data", &context).unwrap(),
            "/home/jack/data"
        );
        assert_eq!(
            evaluate_template("#{userId}-#{groups[0]}", &context).unwrap(),
            "jack-scientists"
        );
    }

    #[test]
    fn converts_results() {
        let context = context();
        assert_eq!(evaluate_to_integer("#{1 + 1}", &context).unwrap(), 2);
        assert_eq!(evaluate_to_integer("120", &context).unwrap(), 120);
        assert!(evaluate_to_boolean("#{userId == 'jack'}", &context).unwrap());
        assert!(evaluate_to_boolean("true", &context).unwrap());
        assert_eq!(
            evaluate_to_list("#{groups}", &context).unwrap(),
            vec!["scientists".to_string()]
        );
        assert_eq!(
            evaluate_to_list("literal", &context).unwrap(),
            vec!["literal".to_string()]
        );
    }

    #[test]
    fn reports_errors_with_context() {
        let context = context();
        let error = evaluate_template("#{unknown}", &context).unwrap_err();
        assert_eq!(error.kind, SpelErrorKind::Unknown);
        assert!(error.to_string().contains("unknown"), "{error}");

        let error = evaluate_template("#{1 +}", &context).unwrap_err();
        assert_eq!(error.kind, SpelErrorKind::Syntax);
    }
}
