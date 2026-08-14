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

//! Tokenizer for the supported subset of the Spring Expression Language.

use crate::error::SpelError;

/// A token of an expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// Identifier or keyword (`proxy`, `and`, `matches`, ...).
    Identifier(String),
    /// String literal (quotes removed, escapes applied).
    String(String),
    /// Integer literal.
    Int(i64),
    /// Floating point literal.
    Float(f64),
    /// `#name` (variable reference).
    Variable(String),
    /// `@name` (bean reference).
    Bean(String),
    /// Punctuation or operator.
    Symbol(&'static str),
}

/// A token together with its position in the expression.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    /// The token.
    pub token: Token,
    /// Byte offset of the token in the expression.
    pub position: usize,
}

/// Operators and punctuation, longest first so that `<=` wins over `<`.
const SYMBOLS: &[&str] = &[
    "?.", "?:", ".![", ".?[", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "(",
    ")", "[", "]", "{", "}", ",", ":", ".", "?", "!", "<", ">",
];

/// Splits an expression into tokens.
pub fn tokenize(expression: &str) -> Result<Vec<SpannedToken>, SpelError> {
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0usize;

    while position < bytes.len() {
        let character = bytes[position] as char;

        if character.is_whitespace() {
            position += 1;
            continue;
        }

        // string literals: 'single' or "double", '' escapes a quote inside single quotes
        if character == '\'' || character == '"' {
            let (value, next) = read_string(expression, position, character)?;
            tokens.push(SpannedToken {
                token: Token::String(value),
                position,
            });
            position = next;
            continue;
        }

        // numbers
        if character.is_ascii_digit() {
            let (token, next) = read_number(expression, position)?;
            tokens.push(SpannedToken { token, position });
            position = next;
            continue;
        }

        // variables and bean references
        if character == '#' || character == '@' {
            let start = position + 1;
            let (name, next) = read_identifier(expression, start);
            if name.is_empty() {
                return Err(SpelError::syntax(
                    expression,
                    position,
                    format!("expected a name after '{character}'"),
                ));
            }
            let token = if character == '#' {
                Token::Variable(name)
            } else {
                Token::Bean(name)
            };
            tokens.push(SpannedToken { token, position });
            position = next;
            continue;
        }

        // identifiers
        if character.is_alphabetic() || character == '_' || character == '$' {
            let (name, next) = read_identifier(expression, position);
            tokens.push(SpannedToken {
                token: Token::Identifier(name),
                position,
            });
            position = next;
            continue;
        }

        // symbols
        let rest = &expression[position..];
        match SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol)) {
            Some(symbol) => {
                tokens.push(SpannedToken {
                    token: Token::Symbol(symbol),
                    position,
                });
                position += symbol.len();
            }
            None => {
                return Err(SpelError::syntax(
                    expression,
                    position,
                    format!("unexpected character '{character}'"),
                ))
            }
        }
    }

    Ok(tokens)
}

fn read_identifier(expression: &str, start: usize) -> (String, usize) {
    let bytes = expression.as_bytes();
    let mut end = start;
    while end < bytes.len() {
        let character = bytes[end] as char;
        if character.is_alphanumeric() || character == '_' || character == '$' {
            end += 1;
        } else {
            break;
        }
    }
    (expression[start..end].to_string(), end)
}

fn read_number(expression: &str, start: usize) -> Result<(Token, usize), SpelError> {
    let bytes = expression.as_bytes();
    let mut end = start;
    let mut is_float = false;
    while end < bytes.len() {
        let character = bytes[end] as char;
        if character.is_ascii_digit() {
            end += 1;
        } else if character == '.'
            && !is_float
            && bytes.get(end + 1).is_some_and(|next| next.is_ascii_digit())
        {
            is_float = true;
            end += 1;
        } else if character == 'L' || character == 'l' {
            // Java long suffix
            let text = &expression[start..end];
            let value = text.parse::<i64>().map_err(|_| {
                SpelError::syntax(expression, start, format!("invalid number '{text}'"))
            })?;
            return Ok((Token::Int(value), end + 1));
        } else {
            break;
        }
    }
    let text = &expression[start..end];
    if is_float {
        let value = text.parse::<f64>().map_err(|_| {
            SpelError::syntax(expression, start, format!("invalid number '{text}'"))
        })?;
        Ok((Token::Float(value), end))
    } else {
        let value = text.parse::<i64>().map_err(|_| {
            SpelError::syntax(expression, start, format!("invalid number '{text}'"))
        })?;
        Ok((Token::Int(value), end))
    }
}

fn read_string(expression: &str, start: usize, quote: char) -> Result<(String, usize), SpelError> {
    let characters: Vec<(usize, char)> = expression.char_indices().collect();
    let mut index = characters
        .iter()
        .position(|(offset, _)| *offset == start)
        .expect("start is a character boundary")
        + 1;
    let mut value = String::new();

    while index < characters.len() {
        let (offset, character) = characters[index];
        if character == quote {
            // a doubled quote is an escaped quote (SpEL uses '' inside single quoted strings)
            if characters
                .get(index + 1)
                .is_some_and(|(_, next)| *next == quote)
            {
                value.push(quote);
                index += 2;
                continue;
            }
            return Ok((value, offset + character.len_utf8()));
        }
        value.push(character);
        index += 1;
    }

    Err(SpelError::syntax(
        expression,
        start,
        "unterminated string literal",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(expression: &str) -> Vec<Token> {
        tokenize(expression)
            .expect("tokenizes")
            .into_iter()
            .map(|spanned| spanned.token)
            .collect()
    }

    #[test]
    fn tokenizes_identifiers_and_navigation() {
        assert_eq!(
            tokens("proxy.userId"),
            vec![
                Token::Identifier("proxy".into()),
                Token::Symbol("."),
                Token::Identifier("userId".into())
            ]
        );
    }

    #[test]
    fn tokenizes_strings_with_escapes() {
        assert_eq!(tokens("'hello'"), vec![Token::String("hello".into())]);
        assert_eq!(tokens("\"hello\""), vec![Token::String("hello".into())]);
        assert_eq!(tokens("'it''s'"), vec![Token::String("it's".into())]);
        assert!(tokenize("'unterminated").is_err());
    }

    #[test]
    fn tokenizes_numbers() {
        assert_eq!(tokens("42"), vec![Token::Int(42)]);
        assert_eq!(tokens("1.5"), vec![Token::Float(1.5)]);
        assert_eq!(tokens("10L"), vec![Token::Int(10)]);
    }

    #[test]
    fn tokenizes_variables_beans_and_operators() {
        assert_eq!(tokens("#root"), vec![Token::Variable("root".into())]);
        assert_eq!(tokens("@bean"), vec![Token::Bean("bean".into())]);
        assert_eq!(
            tokens("a ?: b"),
            vec![
                Token::Identifier("a".into()),
                Token::Symbol("?:"),
                Token::Identifier("b".into())
            ]
        );
        assert_eq!(
            tokens("a?.b"),
            vec![
                Token::Identifier("a".into()),
                Token::Symbol("?."),
                Token::Identifier("b".into())
            ]
        );
        assert_eq!(
            tokens("list.![name]"),
            vec![
                Token::Identifier("list".into()),
                Token::Symbol(".!["),
                Token::Identifier("name".into()),
                Token::Symbol("]")
            ]
        );
        assert_eq!(
            tokens("list.?[#this > 1]"),
            vec![
                Token::Identifier("list".into()),
                Token::Symbol(".?["),
                Token::Variable("this".into()),
                Token::Symbol(">"),
                Token::Int(1),
                Token::Symbol("]")
            ]
        );
    }

    #[test]
    fn reports_unexpected_characters() {
        let error = tokenize("a ~ b").unwrap_err();
        assert_eq!(error.position, Some(2));
        assert!(error.message.contains('~'), "{error}");
    }
}
