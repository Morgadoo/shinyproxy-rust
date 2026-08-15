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

//! Generates `docs/CONFIGURATION.md` from the configuration schema.
//!
//! Run with:
//!
//! ```text
//! cargo run -q -p shinyproxy --example config-docs > docs/CONFIGURATION.md
//! ```
//!
//! A test (`tests/configuration_docs.rs`) verifies that the checked-in file is up to date.

fn main() {
    print!("{}", shinyproxy::config_schema::markdown());
}
