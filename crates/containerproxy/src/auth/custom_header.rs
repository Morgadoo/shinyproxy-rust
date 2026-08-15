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

//! Header based authentication (`CustomHeaderAuthenticationBackend`).
//!
//! A reverse proxy in front of ShinyProxy authenticates the user and passes the result in headers:
//! `proxy.custom-header.username-header-name` (`REMOTE_USER` by default) and, optionally,
//! `proxy.custom-header.groups-header-name`. There is no login page; a request without the username
//! header is sent to `/auth-error`, exactly as in the Java implementation.

use axum::http::HeaderMap;

use super::{normalise_group, AuthBackend, AuthenticatedUser};
use crate::config::Settings;

/// Name of the backend (`proxy.authentication: custom-header`).
pub const NAME: &str = "custom-header";

/// Default name of the header with the user name.
pub const DEFAULT_USERNAME_HEADER: &str = "REMOTE_USER";

/// Authenticates users from request headers.
#[derive(Debug, Clone)]
pub struct CustomHeaderAuthenticationBackend {
    username_header: String,
    groups_header: Option<String>,
}

impl CustomHeaderAuthenticationBackend {
    /// Creates the backend from the configuration.
    pub fn new(settings: &Settings) -> Self {
        let custom_header = &settings.proxy.custom_header;
        CustomHeaderAuthenticationBackend {
            username_header: custom_header
                .username_header_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_USERNAME_HEADER.to_string()),
            groups_header: custom_header
                .groups_header_name
                .clone()
                .filter(|name| !name.trim().is_empty()),
        }
    }

    /// The name of the header with the user name.
    pub fn username_header(&self) -> &str {
        &self.username_header
    }

    /// The name of the header with the groups, when it is configured.
    pub fn groups_header(&self) -> Option<&str> {
        self.groups_header.as_deref()
    }
}

#[async_trait::async_trait]
impl AuthBackend for CustomHeaderAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        true
    }

    fn uses_login_form(&self) -> bool {
        false
    }

    fn logout_success_url(&self) -> String {
        // the user cannot log in again through ShinyProxy, so the logout page is shown
        "/logout-success".to_string()
    }

    fn user_from_headers(&self, headers: &HeaderMap) -> Option<AuthenticatedUser> {
        let username = header_value(headers, &self.username_header)?;
        if username.trim().is_empty() {
            return None;
        }

        let mut groups = Vec::new();
        if let Some(groups_header) = &self.groups_header {
            match header_value(headers, groups_header) {
                Some(value) => {
                    groups = value
                        .split(',')
                        .map(str::trim)
                        .filter(|group| !group.is_empty())
                        .map(normalise_group)
                        .collect();
                }
                None => tracing::warn!(
                    "Header '{groups_header}' does not contain the groups of user '{username}', the \
                     proxy should always override this header. This is a security risk, users might \
                     spoof groups!"
                ),
            }
        }

        Some(AuthenticatedUser::new(username, groups))
    }
}

/// Reads a header, case insensitively (as HTTP headers are).
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header, _)| header.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    #[test]
    fn reads_the_default_header() {
        let backend = CustomHeaderAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: custom-header\n",
        ));
        assert_eq!(backend.username_header(), "REMOTE_USER");
        assert_eq!(backend.groups_header(), None);
        assert!(!backend.uses_login_form());
        assert!(backend.has_authorization());

        let mut headers = HeaderMap::new();
        assert_eq!(backend.user_from_headers(&headers), None);

        headers.insert("remote_user", HeaderValue::from_static("jack"));
        let user = backend.user_from_headers(&headers).expect("user");
        assert_eq!(user.id, "jack");
        assert!(user.groups.is_empty());
    }

    #[test]
    fn reads_the_configured_headers() {
        let backend = CustomHeaderAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: custom-header\n  custom-header:\n    \
             username-header-name: X-SP-UserId\n    groups-header-name: X-SP-UserGroups\n",
        ));
        assert_eq!(backend.username_header(), "X-SP-UserId");
        assert_eq!(backend.groups_header(), Some("X-SP-UserGroups"));

        let mut headers = HeaderMap::new();
        headers.insert("x-sp-userid", HeaderValue::from_static("jack"));
        headers.insert(
            "x-sp-usergroups",
            HeaderValue::from_static("scientists, ROLE_admins"),
        );
        let user = backend.user_from_headers(&headers).expect("user");
        assert_eq!(user.id, "jack");
        assert_eq!(user.groups, vec!["SCIENTISTS", "ADMINS"]);

        // a missing groups header is a warning, not an error (the user has no groups)
        let mut headers = HeaderMap::new();
        headers.insert("x-sp-userid", HeaderValue::from_static("jack"));
        let user = backend.user_from_headers(&headers).expect("user");
        assert!(user.groups.is_empty());
    }

    #[test]
    fn ignores_an_empty_user_name() {
        let backend = CustomHeaderAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: custom-header\n",
        ));
        let mut headers = HeaderMap::new();
        headers.insert("remote_user", HeaderValue::from_static("  "));
        assert_eq!(backend.user_from_headers(&headers), None);
    }
}
