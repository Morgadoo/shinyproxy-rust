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

//! `proxy.authentication: simple` — users are defined in the configuration file.
//!
//! Passwords are compared in constant time. Both plain text passwords (as in the demo configuration)
//! and bcrypt hashes (`$2a$...`, which Spring Security also accepts) are supported.

use super::{normalise_group, AuthBackend, AuthError, AuthenticatedUser, LoginForm};
use crate::config::settings::UserSettings;

/// Name of the backend.
pub const NAME: &str = "simple";

/// A user from `proxy.users`.
#[derive(Debug, Clone)]
struct ConfiguredUser {
    name: String,
    password: String,
    groups: Vec<String>,
}

/// The `simple` authentication backend.
#[derive(Debug)]
pub struct SimpleAuthenticationBackend {
    users: Vec<ConfiguredUser>,
    case_sensitive: bool,
}

impl SimpleAuthenticationBackend {
    /// Creates the backend from the configured users.
    pub fn new(users: &[UserSettings], case_sensitive: bool) -> Self {
        let users = users
            .iter()
            .filter_map(|user| {
                let name = user.name.clone()?;
                Some(ConfiguredUser {
                    name,
                    password: user.password.clone().unwrap_or_default(),
                    groups: user.groups.values().iter().map(normalise_group).collect(),
                })
            })
            .collect();
        SimpleAuthenticationBackend {
            users,
            case_sensitive,
        }
    }

    fn find_user(&self, name: &str) -> Option<&ConfiguredUser> {
        self.users.iter().find(|user| {
            if self.case_sensitive {
                user.name == name
            } else {
                user.name.eq_ignore_ascii_case(name)
            }
        })
    }

    /// Number of configured users.
    pub fn user_count(&self) -> usize {
        self.users.len()
    }
}

#[async_trait::async_trait]
impl AuthBackend for SimpleAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        true
    }

    fn authenticate(&self, form: &LoginForm) -> Result<AuthenticatedUser, AuthError> {
        let Some(user) = self.find_user(&form.username) else {
            // Compare against a dummy value so that unknown users take the same time as known ones.
            let _ = verify_password(&form.password, "$2b$12$C6UzMDM.H6dfI/f/IKcEe.");
            return Err(AuthError::InvalidCredentials);
        };
        if !verify_password(&form.password, &user.password) {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(AuthenticatedUser {
            id: user.name.clone(),
            groups: user.groups.clone(),
            ..Default::default()
        })
    }
}

/// Verifies a password against the configured value (bcrypt hash or plain text).
fn verify_password(candidate: &str, configured: &str) -> bool {
    if configured.starts_with("$2a$")
        || configured.starts_with("$2b$")
        || configured.starts_with("$2y$")
    {
        return bcrypt::verify(candidate, configured).unwrap_or(false);
    }
    // Spring Security's `{noop}` prefix marks plain text passwords.
    let configured = configured.strip_prefix("{noop}").unwrap_or(configured);
    constant_time_eq(candidate.as_bytes(), configured.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn backend(yaml: &str, case_sensitive: bool) -> SimpleAuthenticationBackend {
        let settings: Settings = serde_yaml_ng::from_str(yaml).expect("settings");
        SimpleAuthenticationBackend::new(&settings.proxy.users, case_sensitive)
    }

    fn login(username: &str, password: &str) -> LoginForm {
        LoginForm {
            username: username.to_string(),
            password: password.to_string(),
        }
    }

    #[test]
    fn authenticates_users_from_the_configuration() {
        let backend = backend(
            "proxy:\n  users:\n    - name: jack\n      password: password\n      groups: scientists\n    - name: jeff\n      password: other\n      groups: [ mathematicians, admins ]\n",
            true,
        );
        assert_eq!(backend.user_count(), 2);

        let user = backend
            .authenticate(&login("jack", "password"))
            .expect("authenticates");
        assert_eq!(user.id, "jack");
        assert_eq!(user.groups, ["SCIENTISTS"]);
        assert!(user.is_member_of("scientists"));

        let user = backend
            .authenticate(&login("jeff", "other"))
            .expect("authenticates");
        assert_eq!(user.groups, ["MATHEMATICIANS", "ADMINS"]);
    }

    #[test]
    fn rejects_wrong_credentials() {
        let backend = backend(
            "proxy:\n  users:\n    - name: jack\n      password: password\n",
            true,
        );
        assert_eq!(
            backend.authenticate(&login("jack", "wrong")).unwrap_err(),
            AuthError::InvalidCredentials
        );
        assert_eq!(
            backend
                .authenticate(&login("unknown", "password"))
                .unwrap_err(),
            AuthError::InvalidCredentials
        );
        assert_eq!(
            backend
                .authenticate(&login("JACK", "password"))
                .unwrap_err(),
            AuthError::InvalidCredentials,
            "user names are case sensitive by default"
        );
    }

    #[test]
    fn honours_case_insensitive_user_names() {
        let backend = backend(
            "proxy:\n  users:\n    - name: jack\n      password: password\n",
            false,
        );
        let user = backend
            .authenticate(&login("JACK", "password"))
            .expect("authenticates");
        assert_eq!(user.id, "jack", "the configured spelling is used");
    }

    #[test]
    fn supports_bcrypt_hashes_and_noop_prefix() {
        // bcrypt hash of "password" (cost 4, generated with the bcrypt crate)
        let hash = bcrypt::hash("password", 4).expect("hash");
        let backend = backend(
            &format!("proxy:\n  users:\n    - name: jack\n      password: '{hash}'\n    - name: jeff\n      password: '{{noop}}plain'\n"),
            true,
        );
        assert!(backend.authenticate(&login("jack", "password")).is_ok());
        assert!(backend.authenticate(&login("jack", "wrong")).is_err());
        assert!(backend.authenticate(&login("jeff", "plain")).is_ok());
    }

    #[test]
    fn users_without_a_name_are_ignored() {
        let backend = backend("proxy:\n  users:\n    - password: password\n", true);
        assert_eq!(backend.user_count(), 0);
    }
}
