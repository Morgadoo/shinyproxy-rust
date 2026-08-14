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

//! Configuration validation.
//!
//! The Java implementation logs a set of warnings for contradictory configurations and refuses to start
//! for a few combinations that cannot work. Those checks are reproduced here so that operators see the
//! same diagnostics.

use super::settings::Settings;

/// A configuration problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity of the problem.
    pub severity: Severity,
    /// Message, kept close to the wording of the Java implementation.
    pub message: String,
}

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Logged as a warning; startup continues.
    Warning,
    /// Startup is aborted.
    Fatal,
}

impl Diagnostic {
    fn warning(message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Fatal,
            message: message.into(),
        }
    }
}

/// Validates the configuration, returning all diagnostics (warnings and fatal errors).
pub fn validate(settings: &Settings, unknown_properties: &[String]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let proxy = &settings.proxy;
    let redis_store = proxy.store_mode().eq_ignore_ascii_case("Redis");
    let redis_sessions = settings.spring.session.is_redis();
    let recovery = proxy.recover_running_proxies();

    if settings.server.use_forward_headers.is_some() {
        diagnostics.push(Diagnostic::warning(
            "WARNING: Using server.use-forward-headers will not work in this ShinyProxy release, you need to \
             change your configuration to use another property. See \
             https://shinyproxy.io/documentation/security/#forward-headers on how to change your configuration.",
        ));
    }

    if proxy.same_site_cookie().eq_ignore_ascii_case("none") && !settings.server.secure_cookies() {
        diagnostics.push(Diagnostic::warning(
            "WARNING: Invalid configuration detected: same-site-cookie policy is set to None, but \
             secure-cookies are not enabled. Secure cookies must be enabled when using None as \
             same-site-cookie policy ",
        ));
    }

    if redis_store {
        if !redis_sessions {
            diagnostics.push(Diagnostic::warning(
                "WARNING: Invalid configuration detected: store-mode is set to Redis (i.e. \
                 High-Availability mode), but you are not using Redis for user sessions!",
            ));
        }
        if proxy.stop_proxies_on_shutdown() {
            diagnostics.push(Diagnostic::warning(
                "WARNING: Invalid configuration detected: store-mode is set to Redis (i.e. \
                 High-Availability mode), but proxies are stopped at shutdown of server!",
            ));
        }
        if recovery {
            diagnostics.push(Diagnostic::warning(
                "WARNING: Invalid configuration detected: cannot use store-mode with Redis (i.e. \
                 High-Availability mode) and app recovery at the same time. Disable app recovery!",
            ));
        }
    }

    if redis_sessions {
        if !redis_store {
            diagnostics.push(Diagnostic::warning(
                "WARNING: Invalid configuration detected: user sessions are stored in Redis, but \
                 store-more is not set to Redis. Change store-mode so that app sessions are stored in Redis!",
            ));
        }
        if recovery {
            diagnostics.push(Diagnostic::warning(
                "WARNING: Invalid configuration detected: user sessions are stored in Redis and App \
                 Recovery is enabled. Instead of using App Recovery, change store-mode so that app \
                 sessions are stored in Redis!",
            ));
        }
    }

    if !settings.proxy.api_security.hide_spec_details() {
        diagnostics.push(Diagnostic::warning(
            "WARNING: Insecure configuration detected: The API is configured to return the full spec of \
             proxies, this may contain sensitive values such as the container image, secret environment \
             variables etc. Remove the proxy.api-security.hide-spec-details property to enable API security.",
        ));
    }

    if proxy.container_backend().eq_ignore_ascii_case("local") {
        diagnostics.push(Diagnostic::warning(
            "WARNING: container-backend is set to 'local'. This backend starts apps as local processes \
             and exists for testing purposes only; do not use it in production.",
        ));
    }

    for property in unknown_properties {
        diagnostics.push(Diagnostic::warning(format!(
            "WARNING: Unknown configuration property '{property}'; it is ignored. \
             Check docs/CONFIGURATION.md for the list of supported properties."
        )));
    }

    // Fatal: app recovery cannot be combined with container pre-initialization/sharing. The Java
    // implementation checks this per spec once the specs are parsed; the application crate calls
    // `validate_specs` for that. Here we only check the global combination that is always invalid.
    if recovery && redis_store {
        diagnostics.push(Diagnostic::fatal(
            "Cannot use App Recovery together with store-mode Redis",
        ));
    }

    diagnostics
}

/// Logs the diagnostics and returns an error message when startup must be aborted.
pub fn report(diagnostics: &[Diagnostic]) -> Option<String> {
    for diagnostic in diagnostics {
        match diagnostic.severity {
            Severity::Warning => tracing::warn!("{}", diagnostic.message),
            Severity::Fatal => tracing::error!("{}", diagnostic.message),
        }
    }
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == Severity::Fatal)
        .map(|diagnostic| diagnostic.message.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    fn messages(diagnostics: &[Diagnostic]) -> String {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn clean_configuration_has_no_diagnostics() {
        let settings = settings("proxy:\n  port: 8080\n  container-backend: docker\n");
        assert!(validate(&settings, &[]).is_empty());
    }

    #[test]
    fn warns_about_redis_store_without_redis_sessions() {
        let settings = settings("proxy:\n  store-mode: Redis\n");
        let diagnostics = validate(&settings, &[]);
        let text = messages(&diagnostics);
        assert!(text.contains("not using Redis for user sessions"), "{text}");
        assert!(text.contains("proxies are stopped at shutdown"), "{text}");
        assert!(!diagnostics.iter().any(|d| d.severity == Severity::Fatal));
    }

    #[test]
    fn warns_about_redis_sessions_without_redis_store() {
        let settings = settings("spring:\n  session:\n    store-type: redis\n");
        let text = messages(&validate(&settings, &[]));
        assert!(text.contains("store-more is not set to Redis"), "{text}");
    }

    #[test]
    fn warns_about_insecure_and_removed_properties() {
        let settings = settings(
            "proxy:\n  api-security:\n    hide-spec-details: false\n  same-site-cookie: None\nserver:\n  use-forward-headers: true\n",
        );
        let text = messages(&validate(&settings, &[]));
        assert!(text.contains("server.use-forward-headers"), "{text}");
        assert!(
            text.contains("same-site-cookie policy is set to None"),
            "{text}"
        );
        assert!(text.contains("full spec of proxies"), "{text}");
    }

    #[test]
    fn warns_about_unknown_properties_and_local_backend() {
        let settings = settings("proxy:\n  container-backend: local\n");
        let text = messages(&validate(&settings, &["proxy.typo".to_string()]));
        assert!(text.contains("testing purposes only"), "{text}");
        assert!(
            text.contains("Unknown configuration property 'proxy.typo'"),
            "{text}"
        );
    }

    #[test]
    fn app_recovery_with_redis_store_is_fatal() {
        let settings = settings("proxy:\n  store-mode: Redis\n  recover-running-proxies: true\n");
        let diagnostics = validate(&settings, &[]);
        assert!(
            diagnostics.iter().any(|d| d.severity == Severity::Fatal),
            "{:?}",
            diagnostics
        );
        assert!(report(&diagnostics).is_some());
    }
}
