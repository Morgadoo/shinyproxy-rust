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

//! Services: the stateful components of the engine.

pub mod identifier;
pub mod leader;
pub mod logs;
pub mod parameters;
pub mod proxy_service;
pub mod recovery;
pub mod release;
pub mod runtime_values;
pub mod sessions;

pub use identifier::Identifiers;
pub use leader::{LeaderService, MemoryLeaderService, RedisLeaderService};
pub use logs::{LogPaths, LogService};
pub use parameters::{
    allowed_parameters_for_user, parse_and_validate_request, AllowedParametersForUser,
    InvalidParameters, ParameterName, ParameterNames, ParameterValues,
};
pub use proxy_service::{ProxyService, StartError};
pub use recovery::AppRecoveryService;
pub use release::{ReleaseService, ReleaseStrategy};
pub use runtime_values::{PortMappings, RuntimeValueService};
pub use sessions::{MemorySessionService, RedisSessionService, SessionService, ACTIVE_WINDOW};
