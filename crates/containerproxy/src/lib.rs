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

//! The ContainerProxy engine.
//!
//! This crate is the Rust counterpart of the Java `eu.openanalytics:containerproxy` library: it owns
//! configuration loading, the domain model, container backends, authentication, the proxy lifecycle
//! and the reverse proxy data plane. The `shinyproxy` binary crate adds the ShinyProxy specific
//! configuration notation, controllers and templates on top of it.

#![forbid(unsafe_code)]

/// Chooses the cryptography provider of `rustls` for this process.
///
/// The dependency tree contains both `ring` (through the LDAP client) and `aws-lc-rs` (through the AWS
/// client), so `rustls` refuses to pick one by itself; `ring` is installed here, once, before any TLS
/// connection is made. Callers that build a TLS client should call this first (`AppState::new` does).
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // an error means another provider was already installed, which is just as good
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub mod auth;
pub mod backend;
pub mod config;
pub mod dataplane;
pub mod events;
pub mod model;
pub mod service;
pub mod spec;
pub mod stat;
pub mod store;
pub mod util;
pub mod web;

/// Version of the Java implementation this port aims to be compatible with.
pub const COMPATIBLE_WITH_JAVA_VERSION: &str = "3.2.4";

/// Version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_versions() {
        assert!(!VERSION.is_empty());
        assert_eq!(COMPATIBLE_WITH_JAVA_VERSION, "3.2.4");
    }
}
