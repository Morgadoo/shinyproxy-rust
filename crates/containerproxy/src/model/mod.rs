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

//! The domain model: proxies, containers, specs and runtime values.

pub mod proxy;
pub mod runtime_value;
pub mod spel_field;

pub use proxy::{Container, Proxy, ProxyStartupLog, ProxyStatus, ProxyStopReason};
pub use runtime_value::{
    BackendContainerName, RuntimeValue, RuntimeValueData, RuntimeValueKey, RuntimeValueRegistry,
    RuntimeValues, ValueKind,
};
pub use spel_field::{
    ResolveError, SpecResolver, Spel, SpelBool, SpelLong, SpelString, SpelStringList, SpelStringMap,
};
