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

//! Where app definitions come from.
//!
//! The engine only needs to *read* specs; how they are configured is up to the application. ShinyProxy
//! implements this trait on top of the compact `proxy.specs` notation.

pub mod expression;

pub use expression::{ExpressionContextBuilder, SpelResolver, UserContext, UserKind};

use crate::model::spec::ProxySpec;

/// Provides the app definitions of this ShinyProxy instance.
pub trait SpecProvider: Send + Sync {
    /// All specs, in configuration order.
    fn specs(&self) -> &[ProxySpec];

    /// The spec with the given id.
    fn spec(&self, id: &str) -> Option<&ProxySpec> {
        if id.is_empty() {
            return None;
        }
        self.specs().iter().find(|spec| spec.id == id)
    }

    /// Whether a spec with the given id exists.
    fn contains(&self, id: &str) -> bool {
        self.spec(id).is_some()
    }
}

/// A provider backed by a fixed list of specs (used in tests and by simple deployments).
#[derive(Debug, Clone, Default)]
pub struct StaticSpecProvider {
    specs: Vec<ProxySpec>,
}

impl StaticSpecProvider {
    /// Creates a provider for the given specs.
    pub fn new(specs: Vec<ProxySpec>) -> Self {
        StaticSpecProvider { specs }
    }
}

impl SpecProvider for StaticSpecProvider {
    fn specs(&self) -> &[ProxySpec] {
        &self.specs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_specs_by_id() {
        let provider = StaticSpecProvider::new(vec![ProxySpec::new("a"), ProxySpec::new("b")]);
        assert_eq!(provider.specs().len(), 2);
        assert_eq!(provider.spec("b").map(|spec| spec.id.as_str()), Some("b"));
        assert!(provider.spec("c").is_none());
        assert!(provider.spec("").is_none());
        assert!(provider.contains("a"));
    }
}
