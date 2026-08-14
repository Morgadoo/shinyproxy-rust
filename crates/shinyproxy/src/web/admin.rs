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

//! The admin pages (`AdminController`).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use containerproxy::auth::AuthenticatedUser;
use containerproxy::web::security::no_cache_headers;
use serde_json::json;

use super::model::{prepare_model, Page};
use super::router::CurrentUser;
use super::state::AppState;

/// `GET /admin` — the table with all running apps.
pub async fn admin_page(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
) -> Response {
    render_admin(&state, user.as_ref(), "main")
}

/// `GET /admin/about` — information about this server.
pub async fn about_page(
    State(state): State<Arc<AppState>>,
    axum::Extension(CurrentUser(user)): axum::Extension<CurrentUser>,
) -> Response {
    render_admin(&state, user.as_ref(), "about")
}

fn render_admin(state: &AppState, user: Option<&AuthenticatedUser>, sub_page: &str) -> Response {
    let mut model = prepare_model(state, Page::Admin, user, false);
    model.insert("subPage".into(), json!(sub_page));

    if sub_page == "about" {
        model.insert("runtimeId".into(), json!(state.identifiers.runtime_id));
        model.insert("instanceId".into(), json!(state.identifiers.instance_id));
        model.insert("realmId".into(), json!(state.identifiers.realm_id));
        model.insert("shinyProxyVersion".into(), json!(crate::VERSION));
        // The Java page shows JVM details here; this implementation is a native binary, so it reports
        // the equivalent information (documented in docs/COMPATIBILITY.md).
        model.insert(
            "implementation".into(),
            json!(format!(
                "Rust implementation (compatible with ShinyProxy {})",
                containerproxy::COMPATIBLE_WITH_JAVA_VERSION
            )),
        );
        model.insert("buildInfo".into(), json!(build_info()));
        model.insert("processArguments".into(), json!(process_arguments()));
        model.insert("memoryUsage".into(), json!(memory_usage()));
        model.insert("containerBackend".into(), json!(state.backend.name()));
        model.insert("authentication".into(), json!(state.auth.name()));
    }

    match state.templates.render(
        "admin.html",
        minijinja::Value::from_serialize(serde_json::Value::Object(model)),
    ) {
        Ok(html) => (no_cache_headers(), Html(html)).into_response(),
        Err(error) => {
            tracing::error!("cannot render admin.html: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error\n").into_response()
        }
    }
}

/// Description of this build.
fn build_info() -> String {
    format!(
        "{} (rustc {}, {} profile)",
        env!("CARGO_PKG_VERSION"),
        RUSTC_VERSION,
        PROFILE
    )
}

/// Version of the compiler, filled in at build time.
const RUSTC_VERSION: &str = env!("SHINYPROXY_RUSTC_VERSION");
/// Build profile (`debug`/`release`).
const PROFILE: &str = env!("SHINYPROXY_PROFILE");

/// The command line of this process, one argument per line (as the Java page does for JVM arguments).
fn process_arguments() -> String {
    std::env::args().skip(1).collect::<Vec<_>>().join("\n")
}

/// Resident memory of this process, read from `/proc` on Linux.
fn memory_usage() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            let resident = status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok());
            if let Some(kilobytes) = resident {
                return format!("{} resident", format_bytes(kilobytes * 1024));
            }
        }
    }
    "unknown".to_string()
}

/// Formats a byte count the way the Java page does (`FileUtils.byteCountToDisplaySize`).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("GB", 1024 * 1024 * 1024),
        ("MB", 1024 * 1024),
        ("KB", 1024),
        ("bytes", 1),
    ];
    for (unit, size) in UNITS {
        if bytes >= size {
            let value = bytes / size;
            return format!("{value} {unit}");
        }
    }
    "0 bytes".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_counts_like_java() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3 GB");
    }

    #[test]
    fn reports_build_information() {
        let info = build_info();
        assert!(info.contains(env!("CARGO_PKG_VERSION")), "{info}");
        assert!(info.contains("rustc"), "{info}");
    }

    #[test]
    fn reports_memory_usage() {
        let usage = memory_usage();
        // on linux the value comes from /proc, everywhere else it is unknown
        assert!(usage.contains("resident") || usage == "unknown", "{usage}");
    }
}
