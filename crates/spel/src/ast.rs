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

//! Abstract syntax tree of an expression.

/// A node of the syntax tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// Integer literal.
    Int(i64),
    /// Floating point literal.
    Float(f64),
    /// String literal.
    Str(String),
    /// A property of the root object (`proxy`, `userId`, ...).
    Root(String),
    /// A variable (`#this`, `#root`).
    Variable(String),
    /// A bean (`@identifierService`).
    Bean(String),
    /// A type reference (`T(java.lang.System)`).
    Type(String),
    /// Property access (`target.name`); `safe` marks `?.`.
    Property {
        target: Box<Node>,
        name: String,
        safe: bool,
    },
    /// Index access (`target[index]`).
    Index { target: Box<Node>, index: Box<Node> },
    /// Method call (`target.name(arguments)`); the target is `None` for helper functions of the root.
    Call {
        target: Option<Box<Node>>,
        name: String,
        arguments: Vec<Node>,
        safe: bool,
    },
    /// Projection (`list.![expression]`).
    Projection { target: Box<Node>, body: Box<Node> },
    /// Selection (`list.?[expression]`).
    Selection { target: Box<Node>, body: Box<Node> },
    /// Unary operator (`!`, `-`).
    Unary {
        operator: UnaryOperator,
        operand: Box<Node>,
    },
    /// Binary operator.
    Binary {
        operator: BinaryOperator,
        left: Box<Node>,
        right: Box<Node>,
    },
    /// Ternary operator (`condition ? then : otherwise`).
    Ternary {
        condition: Box<Node>,
        then: Box<Node>,
        otherwise: Box<Node>,
    },
    /// Elvis operator (`value ?: fallback`).
    Elvis {
        value: Box<Node>,
        fallback: Box<Node>,
    },
    /// List literal (`{1, 2, 3}`).
    ListLiteral(Vec<Node>),
    /// Map literal (`{key: value}`).
    MapLiteral(Vec<(String, Node)>),
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    /// `!expression`
    Not,
    /// `-expression`
    Negate,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Modulo,
    /// `==` / `eq`
    Equal,
    /// `!=` / `ne`
    NotEqual,
    /// `<` / `lt`
    Less,
    /// `<=` / `le`
    LessOrEqual,
    /// `>` / `gt`
    Greater,
    /// `>=` / `ge`
    GreaterOrEqual,
    /// `&&` / `and`
    And,
    /// `||` / `or`
    Or,
    /// `matches` (regular expression match)
    Matches,
}

impl BinaryOperator {
    /// Binding power, higher binds tighter.
    pub fn precedence(&self) -> u8 {
        match self {
            BinaryOperator::Or => 1,
            BinaryOperator::And => 2,
            BinaryOperator::Equal | BinaryOperator::NotEqual | BinaryOperator::Matches => 3,
            BinaryOperator::Less
            | BinaryOperator::LessOrEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterOrEqual => 4,
            BinaryOperator::Add | BinaryOperator::Subtract => 5,
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => 6,
        }
    }
}
