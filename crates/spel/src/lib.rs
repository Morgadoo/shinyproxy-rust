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
//! ShinyProxy configuration files may contain `#{ ... }` expressions in most string valued
//! properties (container environment variables, access control expressions, titles, ...).
//! This crate implements the subset of SpEL that is used by those configurations; see
//! `docs/COMPATIBILITY.md` for the exact supported grammar.

#![forbid(unsafe_code)]

/// Placeholder until the parser/evaluator lands in phase P3.
///
/// Returns `true` when the given string contains a SpEL template expression, mirroring the
/// `value.contains("#{")` fast-path checks used by the Java implementation.
pub fn contains_expression(value: &str) -> bool {
    value.contains("#{")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_expressions() {
        assert!(contains_expression("hello #{userId}"));
        assert!(!contains_expression("hello world"));
    }
}
