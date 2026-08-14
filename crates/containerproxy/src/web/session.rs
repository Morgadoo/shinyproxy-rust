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

//! User sessions.
//!
//! A session is created for every request (as in the Java implementation, which uses
//! `SessionCreationPolicy.ALWAYS` so that the `none` authentication backend can keep per-browser
//! state). The cookie is named `JSESSIONID` for in-memory sessions and `SESSION` when sessions are
//! stored in Redis, matching the Java behaviour.

use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::auth::AuthenticatedUser;

/// Cookie name used with in-memory sessions (Undertow's default in the Java implementation).
pub const COOKIE_MEMORY: &str = "JSESSIONID";
/// Cookie name used when sessions are stored in Redis (Spring Session's default).
pub const COOKIE_REDIS: &str = "SESSION";
/// Key under which ShinyProxy stores its session data.
const SESSION_KEY: &str = "shinyproxy";

/// Everything ShinyProxy keeps in a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionData {
    /// The authenticated user, when the user has logged in.
    pub user: Option<AuthenticatedUser>,
    /// CSRF token of the login form.
    pub csrf_token: Option<String>,
    /// Page to redirect to after authentication (`AUTH_SUCCESS_URL_SESSION_ATTR` in Java).
    pub auth_success_url: Option<String>,
    /// Whether the user pressed "sign out" (Java: `SP_USER_INITIATED_LOGOUT`), which decides whether a
    /// destroyed session counts as a logout or as an expiry.
    pub user_initiated_logout: bool,
}

impl SessionData {
    /// Reads the session data (an empty value when the session is new).
    pub async fn load(session: &Session) -> SessionData {
        match session.get::<SessionData>(SESSION_KEY).await {
            Ok(Some(data)) => data,
            Ok(None) => SessionData::default(),
            Err(error) => {
                tracing::warn!("cannot read session data, starting a new session: {error}");
                SessionData::default()
            }
        }
    }

    /// Stores the session data.
    pub async fn store(&self, session: &Session) {
        if let Err(error) = session.insert(SESSION_KEY, self.clone()).await {
            tracing::warn!("cannot store session data: {error}");
        }
    }

    /// The CSRF token, creating one when the session does not have one yet.
    pub async fn csrf_token(&mut self, session: &Session) -> String {
        if let Some(token) = &self.csrf_token {
            return token.clone();
        }
        let token = generate_token();
        self.csrf_token = Some(token.clone());
        self.store(session).await;
        token
    }

    /// Whether the given token matches the token of this session.
    pub fn csrf_token_matches(&self, candidate: &str) -> bool {
        match &self.csrf_token {
            Some(token) => !token.is_empty() && token == candidate,
            None => false,
        }
    }
}

/// Session id of the current session, used for anonymous user names and heartbeat bookkeeping.
pub fn session_id(session: &Session) -> String {
    session
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown-session".to_string())
}

/// Generates a random token (CSRF tokens and anonymous ids).
pub fn generate_token() -> String {
    use rand::Rng;
    let bytes: [u8; 24] = rand::rng().random();
    hex::encode(bytes)
}

/// Cookie name for the configured session store.
pub fn cookie_name(redis_sessions: bool) -> &'static str {
    if redis_sessions {
        COOKIE_REDIS
    } else {
        COOKIE_MEMORY
    }
}

/// `SameSite` policy from the configuration value of `proxy.same-site-cookie`.
pub fn same_site(value: &str) -> tower_sessions::cookie::SameSite {
    match value.to_ascii_lowercase().as_str() {
        "strict" => tower_sessions::cookie::SameSite::Strict,
        "none" => tower_sessions::cookie::SameSite::None,
        _ => tower_sessions::cookie::SameSite::Lax,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_names_match_the_java_implementation() {
        assert_eq!(cookie_name(false), "JSESSIONID");
        assert_eq!(cookie_name(true), "SESSION");
    }

    #[test]
    fn parses_same_site_values() {
        assert_eq!(same_site("Lax"), tower_sessions::cookie::SameSite::Lax);
        assert_eq!(
            same_site("strict"),
            tower_sessions::cookie::SameSite::Strict
        );
        assert_eq!(same_site("None"), tower_sessions::cookie::SameSite::None);
        assert_eq!(same_site("nonsense"), tower_sessions::cookie::SameSite::Lax);
    }

    #[test]
    fn generates_unique_tokens() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 48);
        assert_ne!(first, second);
    }

    #[test]
    fn csrf_tokens_are_compared_exactly() {
        let data = SessionData {
            csrf_token: Some("abc".into()),
            ..Default::default()
        };
        assert!(data.csrf_token_matches("abc"));
        assert!(!data.csrf_token_matches("abd"));
        assert!(!data.csrf_token_matches(""));
        assert!(!SessionData::default().csrf_token_matches("abc"));
    }

    #[test]
    fn session_data_round_trips_through_json() {
        let data = SessionData {
            user: Some(AuthenticatedUser::new("jack", vec!["scientists".into()])),
            csrf_token: Some("token".into()),
            auth_success_url: Some("/app/01_hello".into()),
            user_initiated_logout: true,
        };
        let json = serde_json::to_string(&data).expect("serialises");
        let restored: SessionData = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, data);
    }
}
