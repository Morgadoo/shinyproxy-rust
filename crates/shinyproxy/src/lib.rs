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

//! ShinyProxy: the application on top of the ContainerProxy engine.
//!
//! This crate contributes everything that is specific to ShinyProxy: the compact app definition
//! notation (`proxy.specs`), the user interface, and the ShinyProxy specific API endpoints.

#![forbid(unsafe_code)]

pub mod config_schema;
pub mod logging;
pub mod runtime_values;
pub mod spec_provider;
pub mod web;

/// Version of ShinyProxy (Rust implementation).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The fallback configuration used when no `application.yml` can be found, equivalent to the
/// `application-demo.yml` of the Java distribution.
pub const DEMO_CONFIG: &str = include_str!("../../../examples/application-demo.yml");

/// Loads the configuration the way the ShinyProxy binary does: the full schema (engine + ShinyProxy
/// properties), the demo configuration as fallback, and the process environment.
pub fn load_config(
    options: containerproxy::config::LoadOptions,
) -> Result<
    (
        containerproxy::config::RawConfig,
        containerproxy::config::Settings,
    ),
    containerproxy::config::ConfigError,
> {
    let schema = config_schema::schema();
    let options = match options.fallback_config {
        Some(_) => options,
        None => options.with_fallback_config(DEMO_CONFIG),
    };
    let raw = containerproxy::config::load(&schema, &options)?;
    let settings: containerproxy::config::Settings = serde_json::from_value(raw.tree.clone())
        .map_err(|error| {
            containerproxy::config::ConfigError::Invalid(format!(
                "cannot bind configuration: {error}"
            ))
        })?;
    Ok((raw, settings))
}
