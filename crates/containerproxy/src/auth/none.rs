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

//! `proxy.authentication: none` — no authentication at all.
//!
//! Every session gets an anonymous user whose name is derived from the session id, exactly like the
//! Java `NoAuthenticationBackend` (which stores an `ANONYMOUS_USER_ID` session attribute). Apps of one
//! browser session therefore stay separated from those of another.

use super::{AuthBackend, AuthenticatedUser};

/// The `none` authentication backend.
#[derive(Debug, Default)]
pub struct NoAuthenticationBackend;

/// Name of the backend.
pub const NAME: &str = "none";

impl AuthBackend for NoAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        false
    }

    fn uses_login_form(&self) -> bool {
        false
    }

    fn anonymous_user(&self, session_id: &str) -> Option<AuthenticatedUser> {
        Some(AuthenticatedUser::new(
            anonymous_user_id(session_id),
            Vec::new(),
        ))
    }
}

/// User name of an anonymous session.
pub fn anonymous_user_id(session_id: &str) -> String {
    format!("anonymous-{}", &crate::util::sha1_hex(session_id)[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provides_a_stable_anonymous_user_per_session() {
        let backend = NoAuthenticationBackend;
        assert!(!backend.has_authorization());
        assert!(!backend.uses_login_form());

        let first = backend.anonymous_user("session-a").expect("anonymous user");
        let again = backend.anonymous_user("session-a").expect("anonymous user");
        let other = backend.anonymous_user("session-b").expect("anonymous user");

        assert_eq!(first, again);
        assert_ne!(first, other);
        assert!(first.id.starts_with("anonymous-"), "{}", first.id);
        assert!(first.groups.is_empty());
    }

    #[test]
    fn does_not_authenticate_credentials() {
        let backend = NoAuthenticationBackend;
        let error = backend
            .authenticate(&super::super::LoginForm {
                username: "jack".into(),
                password: "pw".into(),
            })
            .unwrap_err();
        assert_eq!(error, super::super::AuthError::NoFormLogin);
    }
}
