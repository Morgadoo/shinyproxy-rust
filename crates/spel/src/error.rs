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

//! Errors produced while parsing or evaluating expressions.
//!
//! Configuration mistakes must be actionable, so every error carries the expression, the position in
//! it and a message that names the offending construct.

use std::fmt;

/// A parse or evaluation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpelError {
    /// What went wrong.
    pub kind: SpelErrorKind,
    /// The expression that failed.
    pub expression: String,
    /// Byte offset in the expression, when known.
    pub position: Option<usize>,
    /// Human readable message.
    pub message: String,
}

/// Category of a [`SpelError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpelErrorKind {
    /// The expression could not be parsed.
    Syntax,
    /// The expression uses a construct this implementation does not support.
    Unsupported,
    /// A property, method or variable does not exist.
    Unknown,
    /// The expression is valid but could not be evaluated (type mismatch, division by zero, ...).
    Evaluation,
}

impl SpelError {
    /// Creates an error.
    pub fn new(
        kind: SpelErrorKind,
        expression: impl Into<String>,
        position: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        SpelError {
            kind,
            expression: expression.into(),
            position,
            message: message.into(),
        }
    }

    /// A syntax error.
    pub fn syntax(expression: &str, position: usize, message: impl Into<String>) -> Self {
        SpelError::new(SpelErrorKind::Syntax, expression, Some(position), message)
    }

    /// An unsupported construct.
    pub fn unsupported(
        expression: &str,
        position: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        SpelError::new(SpelErrorKind::Unsupported, expression, position, message)
    }

    /// An unknown property, method or variable.
    pub fn unknown(expression: &str, message: impl Into<String>) -> Self {
        SpelError::new(SpelErrorKind::Unknown, expression, None, message)
    }

    /// An evaluation error.
    pub fn evaluation(expression: &str, message: impl Into<String>) -> Self {
        SpelError::new(SpelErrorKind::Evaluation, expression, None, message)
    }

    /// Adds the expression to an error that was created without it.
    pub fn with_expression(mut self, expression: &str) -> Self {
        if self.expression.is_empty() {
            self.expression = expression.to_string();
        }
        self
    }
}

impl fmt::Display for SpelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            SpelErrorKind::Syntax => "syntax error",
            SpelErrorKind::Unsupported => "unsupported expression",
            SpelErrorKind::Unknown => "unknown property or method",
            SpelErrorKind::Evaluation => "evaluation error",
        };
        write!(
            formatter,
            "{kind} in expression '{}': {}",
            self.expression, self.message
        )?;
        if let Some(position) = self.position {
            write!(formatter, " (at position {position})")?;
        }
        Ok(())
    }
}

impl std::error::Error for SpelError {}
