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

use std::sync::Arc;

use containerproxy::config::{warnings, LoadOptions};
use containerproxy::spec::SpecProvider;
use shinyproxy::web::AppState;
use shinyproxy::VERSION;

/// The reverse proxy is allocation heavy (per request: headers, URLs, session data); a profile showed almost
/// half of the CPU of the proxy path inside glibc's allocator. mimalloc cuts that substantially.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "shinyproxy {VERSION} (Rust implementation, compatible with ShinyProxy {})\n\
             commit {}, built {}",
            containerproxy::COMPATIBLE_WITH_JAVA_VERSION,
            env!("SHINYPROXY_GIT_COMMIT"),
            env!("SHINYPROXY_BUILD_TIMESTAMP"),
        );
        return Ok(());
    }

    // the configuration decides how the server logs (`logging.*`, `proxy.log-as-json`), so it is loaded
    // before logging starts; problems while loading it are reported on stderr
    let (raw, settings) = match shinyproxy::load_config(LoadOptions::from_process()) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("Configuration error: {error}");
            std::process::exit(1);
        }
    };
    let _log_guard = shinyproxy::logging::init(&settings);
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

    let bind_address = settings.proxy.bind_address().to_string();
    let port = settings.proxy.port();
    let state = Arc::new(AppState::new(raw, settings).await?);
    state.identifiers.log();
    tracing::info!(
        "Serving {} app(s), authentication: {}",
        state.specs.specs().len(),
        state.auth.name()
    );

    // the usage statistics collectors (`proxy.usage-stats-url`)
    match containerproxy::stat::collectors::create_collectors(&state.settings).await {
        Ok(collectors) => {
            let service = std::sync::Arc::new(
                containerproxy::stat::collectors::UsageStatsService::new(collectors),
            );
            service.subscribe(state.proxies.events());
        }
        Err(error) => {
            tracing::error!("Configuration error: {error}");
            anyhow::bail!(error.to_string());
        }
    }

    // the backend is checked before the server starts: an unusable backend (e.g. a daemon that is not
    // part of a swarm) is a fatal error, exactly as in the Java implementation
    state.backend.initialize().await?;

    // apps that are still running are taken over first; until recovery finished the server answers 503
    // with the startup page (as the Java AppRecoveryFilter does)
    state.spawn_startup_tasks();

    // the management server (actuator) listens on its own port, as Spring Boot does
    let management_port = state.settings.management.port();
    let management_address = format!("{bind_address}:{management_port}");
    match tokio::net::TcpListener::bind(&management_address).await {
        Ok(listener) => {
            tracing::info!(
                "Management endpoints available on http://{management_address}/actuator"
            );
            let management = shinyproxy::web::management::router(state.clone());
            tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, management).await {
                    tracing::warn!("management server stopped: {error}");
                }
            });
        }
        Err(error) => {
            tracing::warn!("cannot start the management server on {management_address}: {error}")
        }
    }

    let app = shinyproxy::web::server::build(state.clone());

    let address = format!("{bind_address}:{port}");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(
        "ShinyProxy {VERSION} listening on http://{}{}",
        listener.local_addr()?,
        state.context_path()
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("ShinyProxy stopped");
    Ok(())
}

/// Waits for `SIGTERM`/`SIGINT` so that the server can shut down gracefully.
async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::warn!("cannot listen for SIGTERM: {error}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => tracing::info!("Received SIGINT, shutting down"),
        _ = terminate => tracing::info!("Received SIGTERM, shutting down"),
    }
}
