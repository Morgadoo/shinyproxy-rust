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

//! Template expressions: `#{...}` embedded in ordinary text.
//!
//! ShinyProxy evaluates most configuration values as *templates* (Spring's `TemplateParserContext`
//! with the `#{`/`}` delimiters), so `my-app-#{userId}` is a valid value and a value without `#{` is
//! returned unchanged.

use crate::error::SpelError;

/// A part of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Literal text.
    Text(String),
    /// An expression (without the `#{` and `}` delimiters).
    Expression(String),
}

/// Whether the value contains a template expression.
pub fn contains_expression(value: &str) -> bool {
    value.contains("#{")
}

/// Splits a template into its parts.
///
/// Braces inside string literals do not end an expression, so `#{'}'}` is a single expression.
pub fn split(template: &str) -> Result<Vec<Part>, SpelError> {
    let mut parts = Vec::new();
    let mut rest = template;

    while let Some(start) = rest.find("#{") {
        if start > 0 {
            parts.push(Part::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let end = find_closing_brace(after).ok_or_else(|| {
            SpelError::syntax(
                template,
                template.len() - after.len(),
                "unterminated expression: no matching '}' for '#{'",
            )
        })?;
        parts.push(Part::Expression(after[..end].to_string()));
        rest = &after[end + 1..];
    }

    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(parts)
}

/// Finds the `}` that closes an expression, skipping string literals and nested braces.
fn find_closing_brace(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (offset, character) in text.char_indices() {
        match (quote, character) {
            (Some(active), current) if current == active => {
                quote = None;
            }
            (Some(_), _) => {}
            (None, '\'') | (None, '"') => quote = Some(character),
            (None, '{') => depth += 1,
            (None, '}') => {
                if depth == 0 {
                    return Some(offset);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_expressions() {
        assert!(contains_expression("a #{b} c"));
        assert!(!contains_expression("a b c"));
    }

    #[test]
    fn splits_templates() {
        assert_eq!(
            split("hello").unwrap(),
            vec![Part::Text("hello".to_string())]
        );
        assert_eq!(
            split("#{userId}").unwrap(),
            vec![Part::Expression("userId".to_string())]
        );
        assert_eq!(
            split("app-#{userId}-suffix").unwrap(),
            vec![
                Part::Text("app-".to_string()),
                Part::Expression("userId".to_string()),
                Part::Text("-suffix".to_string())
            ]
        );
        assert_eq!(
            split("#{a}#{b}").unwrap(),
            vec![
                Part::Expression("a".to_string()),
                Part::Expression("b".to_string())
            ]
        );
    }

    #[test]
    fn handles_braces_in_expressions_and_strings() {
        assert_eq!(
            split("#{ {1, 2} }").unwrap(),
            vec![Part::Expression(" {1, 2} ".to_string())]
        );
        assert_eq!(
            split("#{'}'}").unwrap(),
            vec![Part::Expression("'}'".to_string())]
        );
        assert_eq!(
            split("#{groups.?[#this matches '^a.*$']}").unwrap(),
            vec![Part::Expression(
                "groups.?[#this matches '^a.*$']".to_string()
            )]
        );
    }

    #[test]
    fn reports_unterminated_expressions() {
        let error = split("#{userId").unwrap_err();
        assert!(error.message.contains("unterminated expression"), "{error}");
    }
}
