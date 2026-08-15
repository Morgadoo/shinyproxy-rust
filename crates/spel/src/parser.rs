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

//! Precedence climbing parser for the supported subset of SpEL.

use crate::ast::{BinaryOperator, Node, UnaryOperator};
use crate::error::SpelError;
use crate::lexer::{tokenize, SpannedToken, Token};

/// Parses an expression into a syntax tree.
pub fn parse(expression: &str) -> Result<Node, SpelError> {
    let tokens = tokenize(expression)?;
    let mut parser = Parser {
        expression,
        tokens,
        position: 0,
    };
    let node = parser.parse_expression(0)?;
    if parser.position < parser.tokens.len() {
        let token = &parser.tokens[parser.position];
        return Err(SpelError::syntax(
            expression,
            token.position,
            format!("unexpected trailing input: {:?}", token.token),
        ));
    }
    Ok(node)
}

struct Parser<'a> {
    expression: &'a str,
    tokens: Vec<SpannedToken>,
    position: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position).map(|spanned| &spanned.token)
    }

    fn peek_position(&self) -> usize {
        self.tokens
            .get(self.position)
            .map(|spanned| spanned.position)
            .unwrap_or(self.expression.len())
    }

    fn next(&mut self) -> Option<Token> {
        let token = self
            .tokens
            .get(self.position)
            .map(|spanned| spanned.token.clone());
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<(), SpelError> {
        match self.peek() {
            Some(Token::Symbol(found)) if *found == symbol => {
                self.position += 1;
                Ok(())
            }
            other => Err(SpelError::syntax(
                self.expression,
                self.peek_position(),
                format!("expected '{symbol}' but found {}", describe(other)),
            )),
        }
    }

    fn eat_symbol(&mut self, symbol: &str) -> bool {
        if let Some(Token::Symbol(found)) = self.peek() {
            if *found == symbol {
                self.position += 1;
                return true;
            }
        }
        false
    }

    /// Parses an expression with the given minimum binding power.
    fn parse_expression(&mut self, minimum_precedence: u8) -> Result<Node, SpelError> {
        let mut left = self.parse_unary()?;

        loop {
            // ternary and elvis bind loosest
            if minimum_precedence == 0 {
                if self.eat_symbol("?:") {
                    let fallback = self.parse_expression(0)?;
                    left = Node::Elvis {
                        value: Box::new(left),
                        fallback: Box::new(fallback),
                    };
                    continue;
                }
                if self.eat_symbol("?") {
                    let then = self.parse_expression(0)?;
                    self.expect_symbol(":")?;
                    let otherwise = self.parse_expression(0)?;
                    left = Node::Ternary {
                        condition: Box::new(left),
                        then: Box::new(then),
                        otherwise: Box::new(otherwise),
                    };
                    continue;
                }
            }

            let Some(operator) = self.peek_binary_operator() else {
                break;
            };
            if operator.precedence() < minimum_precedence.max(1) {
                break;
            }
            self.position += 1;
            let right = self.parse_expression(operator.precedence() + 1)?;
            left = Node::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn peek_binary_operator(&self) -> Option<BinaryOperator> {
        match self.peek()? {
            Token::Symbol("+") => Some(BinaryOperator::Add),
            Token::Symbol("-") => Some(BinaryOperator::Subtract),
            Token::Symbol("*") => Some(BinaryOperator::Multiply),
            Token::Symbol("/") => Some(BinaryOperator::Divide),
            Token::Symbol("%") => Some(BinaryOperator::Modulo),
            Token::Symbol("==") => Some(BinaryOperator::Equal),
            Token::Symbol("!=") => Some(BinaryOperator::NotEqual),
            Token::Symbol("<") => Some(BinaryOperator::Less),
            Token::Symbol("<=") => Some(BinaryOperator::LessOrEqual),
            Token::Symbol(">") => Some(BinaryOperator::Greater),
            Token::Symbol(">=") => Some(BinaryOperator::GreaterOrEqual),
            Token::Symbol("&&") => Some(BinaryOperator::And),
            Token::Symbol("||") => Some(BinaryOperator::Or),
            Token::Identifier(name) => match name.as_str() {
                "and" => Some(BinaryOperator::And),
                "or" => Some(BinaryOperator::Or),
                "eq" => Some(BinaryOperator::Equal),
                "ne" => Some(BinaryOperator::NotEqual),
                "lt" => Some(BinaryOperator::Less),
                "le" => Some(BinaryOperator::LessOrEqual),
                "gt" => Some(BinaryOperator::Greater),
                "ge" => Some(BinaryOperator::GreaterOrEqual),
                "matches" => Some(BinaryOperator::Matches),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_unary(&mut self) -> Result<Node, SpelError> {
        if self.eat_symbol("!") {
            let operand = self.parse_unary()?;
            return Ok(Node::Unary {
                operator: UnaryOperator::Not,
                operand: Box::new(operand),
            });
        }
        if self.eat_symbol("-") {
            let operand = self.parse_unary()?;
            return Ok(Node::Unary {
                operator: UnaryOperator::Negate,
                operand: Box::new(operand),
            });
        }
        if let Some(Token::Identifier(name)) = self.peek() {
            if name == "not" {
                self.position += 1;
                let operand = self.parse_unary()?;
                return Ok(Node::Unary {
                    operator: UnaryOperator::Not,
                    operand: Box::new(operand),
                });
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Node, SpelError> {
        let mut node = self.parse_primary()?;

        loop {
            if self.eat_symbol(".![") {
                let body = self.parse_expression(0)?;
                self.expect_symbol("]")?;
                node = Node::Projection {
                    target: Box::new(node),
                    body: Box::new(body),
                };
                continue;
            }
            if self.eat_symbol(".?[") {
                let body = self.parse_expression(0)?;
                self.expect_symbol("]")?;
                node = Node::Selection {
                    target: Box::new(node),
                    body: Box::new(body),
                };
                continue;
            }
            let safe = if self.eat_symbol("?.") {
                true
            } else if self.eat_symbol(".") {
                false
            } else if self.eat_symbol("[") {
                let index = self.parse_expression(0)?;
                self.expect_symbol("]")?;
                node = Node::Index {
                    target: Box::new(node),
                    index: Box::new(index),
                };
                continue;
            } else {
                break;
            };

            let position = self.peek_position();
            let name = match self.next() {
                Some(Token::Identifier(name)) => name,
                other => {
                    return Err(SpelError::syntax(
                        self.expression,
                        position,
                        format!(
                            "expected a property or method name but found {}",
                            describe(other.as_ref())
                        ),
                    ))
                }
            };

            if self.eat_symbol("(") {
                let arguments = self.parse_arguments()?;
                node = Node::Call {
                    target: Some(Box::new(node)),
                    name,
                    arguments,
                    safe,
                };
            } else {
                node = Node::Property {
                    target: Box::new(node),
                    name,
                    safe,
                };
            }
        }

        Ok(node)
    }

    fn parse_arguments(&mut self) -> Result<Vec<Node>, SpelError> {
        let mut arguments = Vec::new();
        if self.eat_symbol(")") {
            return Ok(arguments);
        }
        loop {
            arguments.push(self.parse_expression(0)?);
            if self.eat_symbol(",") {
                continue;
            }
            self.expect_symbol(")")?;
            return Ok(arguments);
        }
    }

    fn parse_primary(&mut self) -> Result<Node, SpelError> {
        let position = self.peek_position();
        match self.next() {
            Some(Token::Int(value)) => Ok(Node::Int(value)),
            Some(Token::Float(value)) => Ok(Node::Float(value)),
            Some(Token::String(value)) => Ok(Node::Str(value)),
            Some(Token::Variable(name)) => Ok(Node::Variable(name)),
            Some(Token::Bean(name)) => Ok(Node::Bean(name)),
            Some(Token::Symbol("(")) => {
                let node = self.parse_expression(0)?;
                self.expect_symbol(")")?;
                Ok(node)
            }
            Some(Token::Symbol("{")) => self.parse_inline_collection(position),
            Some(Token::Identifier(name)) => match name.as_str() {
                "true" => Ok(Node::Bool(true)),
                "false" => Ok(Node::Bool(false)),
                "null" => Ok(Node::Null),
                "new" => Err(SpelError::unsupported(
                    self.expression,
                    Some(position),
                    "constructor calls ('new') are not supported",
                )),
                "T" if matches!(self.peek(), Some(Token::Symbol("("))) => {
                    self.position += 1;
                    let type_name = self.parse_type_name(position)?;
                    self.expect_symbol(")")?;
                    Ok(Node::Type(type_name))
                }
                _ => {
                    if self.eat_symbol("(") {
                        let arguments = self.parse_arguments()?;
                        Ok(Node::Call {
                            target: None,
                            name,
                            arguments,
                            safe: false,
                        })
                    } else {
                        Ok(Node::Root(name))
                    }
                }
            },
            other => Err(SpelError::syntax(
                self.expression,
                position,
                format!("expected a value but found {}", describe(other.as_ref())),
            )),
        }
    }

    /// Parses a (possibly qualified) java type name inside `T(...)`.
    fn parse_type_name(&mut self, position: usize) -> Result<String, SpelError> {
        let mut name = String::new();
        loop {
            match self.next() {
                Some(Token::Identifier(part)) => name.push_str(&part),
                other => {
                    return Err(SpelError::syntax(
                        self.expression,
                        position,
                        format!(
                            "expected a type name but found {}",
                            describe(other.as_ref())
                        ),
                    ))
                }
            }
            if self.eat_symbol(".") {
                name.push('.');
                continue;
            }
            return Ok(name);
        }
    }

    /// Parses `{}`, `{1, 2}` (list) and `{key: value}` (map).
    fn parse_inline_collection(&mut self, position: usize) -> Result<Node, SpelError> {
        if self.eat_symbol("}") {
            return Ok(Node::ListLiteral(Vec::new()));
        }

        // decide between list and map by looking for `key:`
        let checkpoint = self.position;
        if let Some(key) = self.parse_map_key() {
            if self.eat_symbol(":") {
                let mut entries = Vec::new();
                let value = self.parse_expression(0)?;
                entries.push((key, value));
                while self.eat_symbol(",") {
                    let key = self.parse_map_key().ok_or_else(|| {
                        SpelError::syntax(
                            self.expression,
                            self.peek_position(),
                            "expected a map key",
                        )
                    })?;
                    self.expect_symbol(":")?;
                    let value = self.parse_expression(0)?;
                    entries.push((key, value));
                }
                self.expect_symbol("}")?;
                return Ok(Node::MapLiteral(entries));
            }
        }
        self.position = checkpoint;

        let mut items = Vec::new();
        loop {
            items.push(self.parse_expression(0)?);
            if self.eat_symbol(",") {
                continue;
            }
            self.expect_symbol("}").map_err(|_| {
                SpelError::syntax(
                    self.expression,
                    position,
                    "unterminated inline list or map literal",
                )
            })?;
            return Ok(Node::ListLiteral(items));
        }
    }

    fn parse_map_key(&mut self) -> Option<String> {
        match self.peek()?.clone() {
            Token::Identifier(name) => {
                self.position += 1;
                Some(name)
            }
            Token::String(value) => {
                self.position += 1;
                Some(value)
            }
            _ => None,
        }
    }
}

fn describe(token: Option<&Token>) -> String {
    match token {
        None => "the end of the expression".to_string(),
        Some(Token::Identifier(name)) => format!("'{name}'"),
        Some(Token::Symbol(symbol)) => format!("'{symbol}'"),
        Some(Token::String(value)) => format!("the string '{value}'"),
        Some(Token::Int(value)) => format!("the number {value}"),
        Some(Token::Float(value)) => format!("the number {value}"),
        Some(Token::Variable(name)) => format!("the variable #{name}"),
        Some(Token::Bean(name)) => format!("the bean @{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_property_navigation() {
        assert_eq!(
            parse("proxy.userId").unwrap(),
            Node::Property {
                target: Box::new(Node::Root("proxy".into())),
                name: "userId".into(),
                safe: false
            }
        );
        assert_eq!(
            parse("a?.b").unwrap(),
            Node::Property {
                target: Box::new(Node::Root("a".into())),
                name: "b".into(),
                safe: true
            }
        );
    }

    #[test]
    fn parses_method_calls() {
        assert_eq!(
            parse("toList(groups)").unwrap(),
            Node::Call {
                target: None,
                name: "toList".into(),
                arguments: vec![Node::Root("groups".into())],
                safe: false
            }
        );
        assert_eq!(
            parse("userId.toLowerCase()").unwrap(),
            Node::Call {
                target: Some(Box::new(Node::Root("userId".into()))),
                name: "toLowerCase".into(),
                arguments: vec![],
                safe: false
            }
        );
    }

    #[test]
    fn parses_operator_precedence() {
        // 1 + 2 * 3 == 7
        let node = parse("1 + 2 * 3 == 7").unwrap();
        match node {
            Node::Binary {
                operator: BinaryOperator::Equal,
                left,
                right,
            } => {
                assert_eq!(*right, Node::Int(7));
                match *left {
                    Node::Binary {
                        operator: BinaryOperator::Add,
                        right,
                        ..
                    } => {
                        assert!(matches!(
                            *right,
                            Node::Binary {
                                operator: BinaryOperator::Multiply,
                                ..
                            }
                        ));
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_ternary_and_elvis() {
        assert!(matches!(parse("a ? b : c").unwrap(), Node::Ternary { .. }));
        assert!(matches!(parse("a ?: b").unwrap(), Node::Elvis { .. }));
    }

    #[test]
    fn parses_collections_and_indexing() {
        assert_eq!(
            parse("{1, 2}").unwrap(),
            Node::ListLiteral(vec![Node::Int(1), Node::Int(2)])
        );
        assert_eq!(
            parse("{a: 1}").unwrap(),
            Node::MapLiteral(vec![("a".into(), Node::Int(1))])
        );
        assert_eq!(parse("{}").unwrap(), Node::ListLiteral(vec![]));
        assert!(matches!(parse("map['key']").unwrap(), Node::Index { .. }));
        assert!(matches!(parse("list[0]").unwrap(), Node::Index { .. }));
    }

    #[test]
    fn parses_projection_and_selection() {
        assert!(matches!(
            parse("list.![name]").unwrap(),
            Node::Projection { .. }
        ));
        assert!(matches!(
            parse("list.?[#this > 1]").unwrap(),
            Node::Selection { .. }
        ));
    }

    #[test]
    fn parses_type_references() {
        assert_eq!(
            parse("T(java.lang.System)").unwrap(),
            Node::Type("java.lang.System".into())
        );
        assert!(matches!(
            parse("T(java.lang.System).getenv('HOME')").unwrap(),
            Node::Call { .. }
        ));
    }

    #[test]
    fn rejects_unsupported_and_invalid_expressions() {
        let error = parse("new java.lang.String('a')").unwrap_err();
        assert_eq!(error.kind, crate::error::SpelErrorKind::Unsupported);

        let error = parse("proxy.").unwrap_err();
        assert!(
            error.message.contains("expected a property or method name"),
            "{error}"
        );

        let error = parse("1 + ").unwrap_err();
        assert!(error.message.contains("expected a value"), "{error}");

        let error = parse("a b").unwrap_err();
        assert!(error.message.contains("trailing input"), "{error}");
    }
}
