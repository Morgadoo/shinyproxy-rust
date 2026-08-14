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

//! Authentication.
//!
//! `proxy.authentication` selects a backend; the backends behave like their Java counterparts
//! (`IAuthenticationBackend`). Phase P4 implements `none` and `simple`, the remaining backends follow
//! in P11 and fail at startup with a clear message until then.

pub mod none;
pub mod simple;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::spec::expression::{UserContext, UserKind};

/// The credentials submitted through the login form.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LoginForm {
    /// User name.
    pub username: String,
    /// Password.
    pub password: String,
}

/// An authenticated user.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// User name, as used for ownership checks and `SHINYPROXY_USERNAME`.
    pub id: String,
    /// Groups of the user, upper-cased and without the `ROLE_` prefix (as in Java).
    pub groups: Vec<String>,
    /// Attributes/claims provided by the authentication backend.
    pub attributes: BTreeMap<String, serde_json::Value>,
    /// Which backend authenticated the user.
    pub kind: AuthKind,
}

/// Which backend authenticated a user (decides the name of the user object in expressions).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    /// `simple`, `none` or `custom-header` authentication.
    #[default]
    Simple,
    /// OpenID Connect.
    Oidc,
    /// LDAP.
    Ldap,
    /// SAML.
    Saml,
    /// Web service.
    WebService,
}

impl From<AuthKind> for UserKind {
    fn from(kind: AuthKind) -> Self {
        match kind {
            AuthKind::Simple => UserKind::Simple,
            AuthKind::Oidc => UserKind::Oidc,
            AuthKind::Ldap => UserKind::Ldap,
            AuthKind::Saml => UserKind::Saml,
            AuthKind::WebService => UserKind::WebService,
        }
    }
}

impl AuthenticatedUser {
    /// A user with a name and groups.
    pub fn new(id: impl Into<String>, groups: Vec<String>) -> Self {
        AuthenticatedUser {
            id: id.into(),
            groups: groups.into_iter().map(normalise_group).collect(),
            ..Default::default()
        }
    }

    /// Whether the user is a member of the given group (case insensitive, as in Java).
    pub fn is_member_of(&self, group: &str) -> bool {
        self.groups
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(group))
    }

    /// The user as an expression context object.
    pub fn to_user_context(&self) -> UserContext {
        UserContext {
            user_id: self.id.clone(),
            groups: self.groups.clone(),
            attributes: self.attributes.clone(),
            kind: self.kind.into(),
        }
    }
}

/// Normalises a group name the way `UserService.getGroups` does: upper case, without `ROLE_`.
pub fn normalise_group(group: impl Into<String>) -> String {
    let group = group.into().to_uppercase();
    group
        .strip_prefix("ROLE_")
        .map(str::to_string)
        .unwrap_or(group)
}

/// Result of an authentication attempt.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    /// The credentials are wrong (the message is shown on the login page, like Spring's).
    #[error("Invalid user name or password")]
    InvalidCredentials,
    /// The backend does not support form login (OIDC, SAML, custom header, ...).
    #[error("this authentication backend does not use a login form")]
    NoFormLogin,
    /// The backend could not be reached or misbehaved.
    #[error("authentication failed: {0}")]
    Backend(String),
}

/// An authentication backend.
pub trait AuthBackend: Send + Sync + std::fmt::Debug {
    /// Name as used by `proxy.authentication`.
    fn name(&self) -> &'static str;

    /// Whether users have to authenticate (false for the `none` backend).
    fn has_authorization(&self) -> bool;

    /// Whether the backend authenticates through the ShinyProxy login form.
    fn uses_login_form(&self) -> bool {
        self.has_authorization()
    }

    /// URL that logs the user out (`/logout` for most backends).
    fn logout_url(&self) -> &str {
        "/logout"
    }

    /// Authenticates a user with the credentials of the login form.
    fn authenticate(&self, _form: &LoginForm) -> Result<AuthenticatedUser, AuthError> {
        Err(AuthError::NoFormLogin)
    }

    /// The user of an anonymous session (only the `none` backend has one).
    fn anonymous_user(&self, _session_id: &str) -> Option<AuthenticatedUser> {
        None
    }
}

/// Creates the configured authentication backend.
pub fn create(settings: &Settings) -> Result<Arc<dyn AuthBackend>, UnsupportedBackend> {
    match settings
        .proxy
        .authentication()
        .to_ascii_lowercase()
        .as_str()
    {
        "none" => Ok(Arc::new(none::NoAuthenticationBackend)),
        "simple" => Ok(Arc::new(simple::SimpleAuthenticationBackend::new(
            &settings.proxy.users,
            settings.proxy.username_case_sensitive(),
        ))),
        other => Err(UnsupportedBackend {
            name: other.to_string(),
        }),
    }
}

/// The configured authentication backend is not implemented yet.
#[derive(Debug, thiserror::Error)]
#[error(
    "authentication backend '{name}' is not supported yet by this implementation (supported: none, \
     simple); see docs/PROGRESS.md for the phase that adds it"
)]
pub struct UnsupportedBackend {
    /// The configured value of `proxy.authentication`.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_groups_like_java() {
        assert_eq!(normalise_group("scientists"), "SCIENTISTS");
        assert_eq!(normalise_group("ROLE_admins"), "ADMINS");
        assert_eq!(normalise_group("ROLE_ADMINS"), "ADMINS");
    }

    #[test]
    fn checks_group_membership_case_insensitively() {
        let user = AuthenticatedUser::new("jack", vec!["scientists".into()]);
        assert!(user.is_member_of("SCIENTISTS"));
        assert!(user.is_member_of("scientists"));
        assert!(!user.is_member_of("admins"));
    }

    #[test]
    fn creates_the_configured_backend() {
        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  authentication: none\n").unwrap();
        assert_eq!(create(&settings).unwrap().name(), "none");

        let settings: Settings = serde_yaml_ng::from_str(
            "proxy:\n  authentication: simple\n  users:\n    - name: jack\n      password: pw\n",
        )
        .unwrap();
        let backend = create(&settings).unwrap();
        assert_eq!(backend.name(), "simple");
        assert!(backend.has_authorization());

        let settings: Settings =
            serde_yaml_ng::from_str("proxy:\n  authentication: openid\n").unwrap();
        let error = create(&settings).unwrap_err();
        assert!(error.to_string().contains("not supported yet"), "{error}");
    }

    #[test]
    fn defaults_to_no_authentication() {
        let settings = Settings::default();
        let backend = create(&settings).unwrap();
        assert_eq!(backend.name(), "none");
        assert!(!backend.has_authorization());
    }
}
