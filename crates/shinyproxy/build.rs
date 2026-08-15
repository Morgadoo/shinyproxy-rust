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

//! Build script: records build information that the admin page shows.

fn main() {
    let rustc =
        std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg("--version")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|version| version.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SHINYPROXY_RUSTC_VERSION={rustc}");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=SHINYPROXY_PROFILE={profile}");

    // the git commit: a release tarball or a container build has no git metadata, so the caller may pass it
    // in with SHINYPROXY_GIT_COMMIT
    let commit = std::env::var("SHINYPROXY_GIT_COMMIT")
        .ok()
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty() && commit != "unknown")
        .unwrap_or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|commit| commit.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });
    println!("cargo:rustc-env=SHINYPROXY_GIT_COMMIT={commit}");
    println!("cargo:rerun-if-env-changed=SHINYPROXY_GIT_COMMIT");

    // when the binary was built, in UTC (`SOURCE_DATE_EPOCH` keeps reproducible builds reproducible)
    let timestamp = match std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
    {
        Some(epoch) => epoch,
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default(),
    };
    let built = time::OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SHINYPROXY_BUILD_TIMESTAMP={built}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
