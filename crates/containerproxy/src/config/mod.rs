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

//! Configuration subsystem.
//!
//! ShinyProxy is configured through `application.yml` with Spring Boot semantics (profiles, relaxed
//! binding, environment variable and command line overrides, `${...}` placeholders). This module
//! reproduces those semantics and binds the result onto typed settings.

pub mod flex;
pub mod loader;
pub mod schema;
pub mod settings;
pub mod tree;
pub mod warnings;

pub use flex::{FlexBool, FlexI64, FlexString, StringList, StringMap};
pub use loader::{load, ConfigError, LoadOptions, RawConfig, CONFIG_FILENAME, DEMO_PROFILE};
pub use schema::{KeyDef, KeyKind, Schema, Support};
pub use settings::{HikariSettings, ProxySettings, ServerSettings, Settings, SpringSettings};
pub use warnings::{validate, Diagnostic, Severity};
