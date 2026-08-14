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

//! ShinyProxy server entry point.

#![forbid(unsafe_code)]

use axum::http::StatusCode;
use axum::routing::any;
use axum::Router;
use containerproxy::config::{warnings, LoadOptions};
use containerproxy::service::Identifiers;
use shinyproxy::VERSION;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "shinyproxy {VERSION} (Rust implementation, compatible with ShinyProxy {})",
            containerproxy::COMPATIBLE_WITH_JAVA_VERSION
        );
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (raw, settings) = shinyproxy::load_config(LoadOptions::from_process())?;
    match &raw.path {
        Some(path) => tracing::info!("Using configuration file {}", path.display()),
        None => {
            tracing::warn!("WARNING: Did not found configuration, using fallback configuration!")
        }
    }

    let diagnostics = warnings::validate(&settings, &raw.unknown_properties);
    if let Some(fatal) = warnings::report(&diagnostics) {
        anyhow::bail!(fatal);
    }

    let identifiers =
        Identifiers::from_config(&raw, std::env::var("SP_KUBE_POD_NAME").ok().as_deref());
    identifiers.log();

    // Placeholder router: replaced by the real UI/API routers in phases P4 onwards.
    let app = Router::new().fallback(any(|| async {
        (
            StatusCode::NOT_IMPLEMENTED,
            "ShinyProxy (Rust) is being implemented; no routes are registered yet.\n",
        )
    }));

    let address = format!(
        "{}:{}",
        settings.proxy.bind_address(),
        settings.proxy.port()
    );
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(
        "ShinyProxy {VERSION} listening on http://{}{}",
        listener.local_addr()?,
        settings.server.context_path()
    );
    axum::serve(listener, app).await?;
    Ok(())
}
