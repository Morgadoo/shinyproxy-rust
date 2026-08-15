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

//! Bearer token authentication (the OAuth2 resource server of `WebSecurityConfig`).
//!
//! When `proxy.oauth2.resource-id` and `proxy.oauth2.jwks-url` are configured, an API client can send a JWT
//! in the `Authorization` header *next to* the configured authentication backend, which is how scripts and
//! CI jobs talk to the API. The token has to be signed by a key of the JWKS, be valid at this moment, have
//! the resource id in its audience and contain the user name claim (`proxy.oauth2.username-attribute`,
//! `sub` by default) — the same four validators as the Java implementation. `proxy.oauth2.roles-claim`
//! provides the groups.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::{normalise_group, AuthKind, AuthenticatedUser};
use crate::config::Settings;

/// Validates bearer tokens.
pub struct BearerTokenAuthenticator {
    /// Audience the token must carry (`proxy.oauth2.resource-id`).
    resource_id: String,
    /// Where the keys come from (`proxy.oauth2.jwks-url`).
    jwks_url: String,
    /// Claim with the user name (`sub` by default).
    username_claim: String,
    /// Claim with the groups.
    roles_claim: Option<String>,
    /// The keys of the provider, cached for a few minutes (Spring caches them as well).
    keys: moka::future::Cache<(), Arc<jsonwebtoken::jwk::JwkSet>>,
}

impl std::fmt::Debug for BearerTokenAuthenticator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BearerTokenAuthenticator")
            .field("resource_id", &self.resource_id)
            .field("jwks_url", &self.jwks_url)
            .field("username_claim", &self.username_claim)
            .field("roles_claim", &self.roles_claim)
            .finish()
    }
}

impl BearerTokenAuthenticator {
    /// Creates the authenticator when `proxy.oauth2.*` is configured.
    pub fn from_settings(settings: &Settings) -> Option<Self> {
        let oauth2 = &settings.proxy.oauth2;
        let resource_id = oauth2
            .resource_id
            .clone()
            .filter(|value| !value.trim().is_empty())?;
        let jwks_url = oauth2
            .jwks_url
            .clone()
            .filter(|value| !value.trim().is_empty())?;

        Some(BearerTokenAuthenticator {
            resource_id,
            jwks_url,
            username_claim: oauth2
                .username_attribute
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "sub".to_string()),
            roles_claim: oauth2
                .roles_claim
                .clone()
                .filter(|value| !value.trim().is_empty()),
            keys: moka::future::Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(300))
                .build(),
        })
    }

    /// The user of a bearer token, or `None` when the token is not usable.
    pub async fn authenticate(&self, token: &str) -> Option<AuthenticatedUser> {
        let jwks = match self.jwks().await {
            Ok(jwks) => jwks,
            Err(error) => {
                tracing::warn!("cannot read the JWKS of proxy.oauth2.jwks-url: {error}");
                return None;
            }
        };

        let header = jsonwebtoken::decode_header(token).ok()?;
        let key = match &header.kid {
            Some(kid) => jwks.find(kid),
            None => jwks.keys.first(),
        }?;
        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(key).ok()?;

        let mut validation = jsonwebtoken::Validation::new(header.alg);
        // the audience must contain the resource id, and the timestamps must be valid
        validation.set_audience(std::slice::from_ref(&self.resource_id));
        validation.validate_exp = true;
        validation.validate_nbf = true;
        // Spring's JwtTimestampValidator allows 60 seconds of clock skew by default
        validation.leeway = 60;

        let claims = match jsonwebtoken::decode::<BTreeMap<String, serde_json::Value>>(
            token,
            &decoding_key,
            &validation,
        ) {
            Ok(token) => token.claims,
            Err(error) => {
                tracing::debug!("refusing a bearer token: {error}");
                return None;
            }
        };

        let username = claims
            .get(&self.username_claim)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty());
        let Some(username) = username else {
            tracing::warn!(
                "Cannot extract username from OAuth token, no claim {} found",
                self.username_claim
            );
            return None;
        };

        let mut groups = Vec::new();
        if let Some(claim) = &self.roles_claim {
            for role in super::openid::parse_roles_claim(claims.get(claim)) {
                let group = normalise_group(role);
                if !groups.contains(&group) {
                    groups.push(group);
                }
            }
        }

        Some(AuthenticatedUser {
            id: username.to_string(),
            groups,
            attributes: claims,
            kind: AuthKind::Oidc,
        })
    }

    /// The keys of the provider, from the cache when they were read recently.
    async fn jwks(&self) -> Result<Arc<jsonwebtoken::jwk::JwkSet>, String> {
        if let Some(cached) = self.keys.get(&()).await {
            return Ok(cached);
        }
        let response = reqwest::get(&self.jwks_url)
            .await
            .map_err(|error| error.to_string())?;
        let jwks: jsonwebtoken::jwk::JwkSet =
            response.json().await.map_err(|error| error.to_string())?;
        let jwks = Arc::new(jwks);
        self.keys.insert((), jwks.clone()).await;
        Ok(jwks)
    }
}

/// The bearer token of a request, when it has one.
pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or(value.strip_prefix("bearer "))?;
    (!token.trim().is_empty()).then_some(token.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    #[test]
    fn is_only_enabled_when_configured() {
        assert!(BearerTokenAuthenticator::from_settings(&Settings::default()).is_none());
        assert!(BearerTokenAuthenticator::from_settings(&settings(
            "proxy:\n  oauth2:\n    resource-id: shinyproxy\n"
        ))
        .is_none());

        let authenticator = BearerTokenAuthenticator::from_settings(&settings(
            "proxy:\n  oauth2:\n    resource-id: shinyproxy\n    \
             jwks-url: https://idp/jwks\n    roles-claim: groups\n",
        ))
        .expect("authenticator");
        assert_eq!(authenticator.username_claim, "sub");
        assert_eq!(authenticator.roles_claim.as_deref(), Some("groups"));
    }

    #[test]
    fn reads_the_bearer_token_of_a_request() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer_token(&headers), None);

        headers.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer_token(&headers), None);

        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer the-token"),
        );
        assert_eq!(bearer_token(&headers), Some("the-token"));

        headers.insert(
            "authorization",
            HeaderValue::from_static("bearer the-token"),
        );
        assert_eq!(bearer_token(&headers), Some("the-token"));

        headers.insert("authorization", HeaderValue::from_static("Bearer   "));
        assert_eq!(bearer_token(&headers), None);
    }
}
