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

//! Security headers and route classification.
//!
//! The headers and their configuration switches mirror the Java `WebSecurityConfig`.

use axum::http::header::{HeaderName, HeaderValue};
use axum::http::HeaderMap;

use crate::config::Settings;

/// The security headers that are added to every response.
#[derive(Debug, Clone, Default)]
pub struct SecurityHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SecurityHeaders {
    /// Builds the headers from the configuration.
    pub fn from_settings(settings: &Settings) -> Self {
        let api_security = &settings.proxy.api_security;
        let mut headers = Vec::new();

        if !api_security
            .disable_no_sniff_header
            .map(|value| value.0)
            .unwrap_or(false)
        {
            headers.push((
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ));
        }
        if !api_security
            .disable_xss_protection_header
            .map(|value| value.0)
            .unwrap_or(false)
        {
            headers.push((
                HeaderName::from_static("x-xss-protection"),
                HeaderValue::from_static("0"),
            ));
        }
        if !api_security
            .disable_hsts_header
            .map(|value| value.0)
            .unwrap_or(false)
        {
            headers.push((
                HeaderName::from_static("strict-transport-security"),
                HeaderValue::from_static("max-age=31536000 ; includeSubDomains"),
            ));
        }

        match settings
            .server
            .frame_options()
            .to_ascii_uppercase()
            .as_str()
        {
            "DISABLE" => {}
            "DENY" => headers.push((
                HeaderName::from_static("x-frame-options"),
                HeaderValue::from_static("DENY"),
            )),
            "SAMEORIGIN" => headers.push((
                HeaderName::from_static("x-frame-options"),
                HeaderValue::from_static("SAMEORIGIN"),
            )),
            other if other.starts_with("ALLOW-FROM") => {
                if let Ok(value) = HeaderValue::from_str(settings.server.frame_options()) {
                    headers.push((HeaderName::from_static("x-frame-options"), value));
                }
            }
            _ => {}
        }

        // custom headers may override the ones above (Java: OverridingHeaderWriter)
        for custom in &api_security.custom_headers {
            let (Some(name), Some(value)) = (&custom.name, &custom.value) else {
                tracing::warn!("Missing header value for header {:?}", custom.name);
                continue;
            };
            match (
                HeaderName::try_from(name.as_str()),
                HeaderValue::from_str(value),
            ) {
                (Ok(name), Ok(value)) => {
                    headers.retain(|(existing, _)| existing != &name);
                    headers.push((name, value));
                }
                _ => tracing::warn!("Ignoring invalid custom header {name}"),
            }
        }

        SecurityHeaders { headers }
    }

    /// The headers as a map.
    pub fn header_map(&self) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in &self.headers {
            map.insert(name.clone(), value.clone());
        }
        map
    }

    /// The configured headers.
    pub fn headers(&self) -> &[(HeaderName, HeaderValue)] {
        &self.headers
    }
}

/// Cache headers that the Java implementation adds to its own (non proxied) responses.
pub fn no_cache_headers() -> [(HeaderName, HeaderValue); 3] {
    [
        (
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache, no-store, max-age=0, must-revalidate"),
        ),
        (
            HeaderName::from_static("pragma"),
            HeaderValue::from_static("no-cache"),
        ),
        (
            HeaderName::from_static("expires"),
            HeaderValue::from_static("0"),
        ),
    ]
}

/// Paths that are reachable without authentication (the `permitAll` matchers of the Java
/// configuration).
pub fn is_public_path(path: &str, instance_id: &str) -> bool {
    const PUBLIC: &[&str] = &[
        "/login",
        "/auth-error",
        "/error",
        "/app-access-denied",
        "/logout-success",
        "/favicon.ico",
        "/saml/metadata",
    ];
    const PUBLIC_PREFIXES: &[&str] = &["/signin/", "/actuator/"];

    if PUBLIC.contains(&path) {
        return true;
    }
    if PUBLIC_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return true;
    }
    // assets, both with and without the instance id prefix
    let unprefixed = path
        .strip_prefix(&format!("/{instance_id}"))
        .unwrap_or(path);
    if unprefixed == "/favicon" {
        return true;
    }
    super::assets::ASSET_PREFIXES
        .iter()
        .any(|prefix| unprefixed.starts_with(&format!("/{prefix}/")))
}

/// Whether a path requires administrator rights.
pub fn is_admin_path(path: &str) -> bool {
    path == "/admin" || path.starts_with("/admin/") || path.starts_with("/grafana/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    #[test]
    fn adds_the_default_security_headers() {
        let headers = SecurityHeaders::from_settings(&Settings::default()).header_map();
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert!(headers.contains_key("x-xss-protection"));
        assert!(headers.contains_key("strict-transport-security"));
        // frame options default to `disable`, so no header
        assert!(!headers.contains_key("x-frame-options"));
    }

    #[test]
    fn honours_the_disable_switches_and_frame_options() {
        let headers = SecurityHeaders::from_settings(&settings(
            "proxy:\n  api-security:\n    disable-no-sniff-header: true\n    disable-hsts-header: true\n    disable-xss-protection-header: true\nserver:\n  frame-options: sameorigin\n",
        ))
        .header_map();
        assert!(!headers.contains_key("x-content-type-options"));
        assert!(!headers.contains_key("strict-transport-security"));
        assert!(!headers.contains_key("x-xss-protection"));
        assert_eq!(headers.get("x-frame-options").unwrap(), "SAMEORIGIN");
    }

    #[test]
    fn custom_headers_override_defaults() {
        let headers = SecurityHeaders::from_settings(&settings(
            "proxy:\n  api-security:\n    custom-headers:\n      - name: X-Content-Type-Options\n        value: custom\n      - name: X-Custom\n        value: value\n",
        ))
        .header_map();
        assert_eq!(headers.get("x-content-type-options").unwrap(), "custom");
        assert_eq!(headers.get("x-custom").unwrap(), "value");
    }

    #[test]
    fn classifies_public_and_admin_paths() {
        let instance = "abc123";
        for path in [
            "/login",
            "/error",
            "/auth-error",
            "/logout-success",
            "/app-access-denied",
            "/favicon.ico",
            "/css/default.css",
            "/js/shiny.common.js",
            "/webjars/jquery/3.7.1/jquery.min.js",
            "/abc123/js/shiny.common.js",
            "/abc123/favicon",
            "/actuator/health",
        ] {
            assert!(is_public_path(path, instance), "{path} must be public");
        }
        for path in ["/", "/app/01_hello", "/api/proxy", "/admin", "/heartbeat/x"] {
            assert!(!is_public_path(path, instance), "{path} must not be public");
        }

        assert!(is_admin_path("/admin"));
        assert!(is_admin_path("/admin/data"));
        assert!(is_admin_path("/grafana/dashboard"));
        assert!(!is_admin_path("/administration"));
        assert!(!is_admin_path("/"));
    }
}
