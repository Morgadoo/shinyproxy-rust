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

//! OpenID Connect authentication (`OpenIDAuthenticationBackend`, `OpenIDConfiguration`).
//!
//! The authorization code flow, with the same URLs and configuration keys as the Java implementation:
//!
//! * `/oauth2/authorization/shinyproxy` starts the flow (that is where the login page sends the user),
//! * `/login/oauth2/code/shinyproxy` is the redirect URI the provider is configured with,
//! * `proxy.openid.{auth-url,token-url,jwks-url,userinfo-url,client-id,client-secret,scopes,
//!   username-attribute,roles-claim,logout-url,with-pkce,include-default-scopes,
//!   jwks-signature-algorithm}` configure it.
//!
//! Groups come from `roles-claim` in the id token *and* in the user info, parsed with the rules of
//! `parseRolesClaim` (a list, or a string containing a JSON list).

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{normalise_group, AuthBackend, AuthError, AuthKind, AuthenticatedUser};
use crate::config::Settings;

/// Name of the backend.
pub const NAME: &str = "openid";

/// The registration id of the client, which is part of the URLs (as in Java).
pub const REGISTRATION_ID: &str = "shinyproxy";

/// Where the login page sends the user to start the flow.
pub const AUTHORIZATION_PATH: &str = "/oauth2/authorization/shinyproxy";

/// The redirect URI the provider sends the user back to.
pub const CALLBACK_PATH: &str = "/login/oauth2/code/shinyproxy";

/// Environment variable with the access token of the user (`SHINYPROXY_OIDC_ACCESS_TOKEN`).
pub const ACCESS_TOKEN_ENV_VAR: &str = "SHINYPROXY_OIDC_ACCESS_TOKEN";

/// Authenticates users with an OpenID Connect provider.
#[derive(Debug, Clone)]
pub struct OpenIdAuthenticationBackend {
    /// Authorization endpoint of the provider.
    pub auth_url: String,
    /// Token endpoint of the provider.
    pub token_url: String,
    /// JWKS endpoint, used to verify the id token.
    pub jwks_url: Option<String>,
    /// User info endpoint, when the provider has one.
    pub userinfo_url: Option<String>,
    /// Client id and secret of ShinyProxy.
    pub client_id: String,
    pub client_secret: Option<String>,
    /// Scopes of the authorization request.
    pub scopes: Vec<String>,
    /// Claim with the user name (`email` by default).
    pub username_attribute: String,
    /// Claim with the groups of the user.
    pub roles_claim: Option<String>,
    /// Where the user goes after logging out.
    pub logout_url: Option<String>,
    /// Whether PKCE is used.
    pub with_pkce: bool,
    /// Signature algorithm of the id token (`RS256` by default).
    pub signature_algorithm: String,
}

impl OpenIdAuthenticationBackend {
    /// Reads the configuration, with the startup errors of the Java implementation.
    pub fn new(settings: &Settings) -> Result<Self, String> {
        let openid = &settings.proxy.openid;
        let required = |value: &Option<String>, key: &str| {
            value
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("OpenID authentication enabled, but no '{key}' defined!"))
        };

        // `openid` and `email` are added unless include-default-scopes is disabled
        let mut scopes: Vec<String> = Vec::new();
        if openid
            .include_default_scopes
            .map(|value| value.0)
            .unwrap_or(true)
        {
            scopes.push("openid".to_string());
            scopes.push("email".to_string());
        }
        for scope in openid.scopes.values() {
            if !scopes.contains(scope) {
                scopes.push(scope.clone());
            }
        }

        Ok(OpenIdAuthenticationBackend {
            auth_url: required(&openid.auth_url, "proxy.openid.auth-url")?,
            token_url: required(&openid.token_url, "proxy.openid.token-url")?,
            jwks_url: openid.jwks_url.clone().filter(|url| !url.trim().is_empty()),
            userinfo_url: openid
                .userinfo_url
                .clone()
                .filter(|url| !url.trim().is_empty()),
            client_id: required(&openid.client_id, "proxy.openid.client-id")?,
            client_secret: openid
                .client_secret
                .clone()
                .filter(|secret| !secret.trim().is_empty()),
            scopes,
            username_attribute: openid
                .username_attribute
                .clone()
                .filter(|attribute| !attribute.trim().is_empty())
                .unwrap_or_else(|| "email".to_string()),
            roles_claim: openid
                .roles_claim
                .clone()
                .filter(|claim| !claim.trim().is_empty()),
            logout_url: openid
                .logout_url
                .clone()
                .filter(|url| !url.trim().is_empty()),
            with_pkce: openid.with_pkce.map(|value| value.0).unwrap_or(false),
            signature_algorithm: openid
                .jwks_signature_algorithm
                .clone()
                .filter(|algorithm| !algorithm.trim().is_empty())
                .unwrap_or_else(|| "RS256".to_string()),
        })
    }

    /// The URL the user is sent to, and the values that have to be remembered in the session.
    pub fn authorization_request(&self, redirect_uri: &str) -> AuthorizationRequest {
        let state = random_string(32);
        let nonce = random_string(32);
        let verifier = self.with_pkce.then(|| random_string(64));

        let mut url = format!(
            "{}{}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}",
            self.auth_url,
            if self.auth_url.contains('?') {
                "&"
            } else {
                "?"
            },
            encode(&self.client_id),
            encode(redirect_uri),
            encode(&self.scopes.join(" ")),
            encode(&state),
            encode(&nonce),
        );
        if let Some(verifier) = &verifier {
            url.push_str(&format!(
                "&code_challenge={}&code_challenge_method=S256",
                encode(&pkce_challenge(verifier))
            ));
        }

        AuthorizationRequest {
            url,
            state,
            nonce,
            verifier,
        }
    }

    /// Exchanges the authorization code for tokens.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: Option<&str>,
    ) -> Result<TokenResponse, AuthError> {
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.client_id.clone()),
        ];
        if let Some(verifier) = verifier {
            form.push(("code_verifier", verifier.to_string()));
        }

        let client = reqwest::Client::new();
        let mut request = client.post(&self.token_url).form(&form);
        // the client secret is sent with basic authentication, as Spring's default does
        if let Some(secret) = &self.client_secret {
            request = request.basic_auth(&self.client_id, Some(secret));
        }

        let response = request.send().await.map_err(|error| {
            AuthError::Backend(format!("cannot reach the token endpoint: {error}"))
        })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AuthError::Backend(format!(
                "the token endpoint answered {status}: {body}"
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AuthError::Backend(format!("invalid token response: {error} ({body})"))
        })
    }

    /// Reads the claims of an id token, verifying its signature when a JWKS endpoint is configured.
    pub async fn id_token_claims(
        &self,
        id_token: &str,
        expected_nonce: Option<&str>,
    ) -> Result<BTreeMap<String, serde_json::Value>, AuthError> {
        let claims = match &self.jwks_url {
            Some(jwks_url) => self.verify_with_jwks(id_token, jwks_url).await?,
            None => {
                // without a JWKS endpoint the token cannot be verified; the claims are still read, and
                // the warning says why that is not enough on its own
                tracing::warn!(
                    "proxy.openid.jwks-url is not configured, the signature of the id token is not \
                     verified"
                );
                decode_claims_without_verification(id_token)?
            }
        };

        if let Some(expected) = expected_nonce {
            let nonce = claims.get("nonce").and_then(|value| value.as_str());
            if nonce != Some(expected) {
                return Err(AuthError::Backend(
                    "the nonce of the id token does not match the request".to_string(),
                ));
            }
        }
        Ok(claims)
    }

    /// Verifies an id token with the keys of the provider.
    async fn verify_with_jwks(
        &self,
        id_token: &str,
        jwks_url: &str,
    ) -> Result<BTreeMap<String, serde_json::Value>, AuthError> {
        let response = reqwest::get(jwks_url)
            .await
            .map_err(|error| AuthError::Backend(format!("cannot read the JWKS: {error}")))?;
        let jwks: jsonwebtoken::jwk::JwkSet = response
            .json()
            .await
            .map_err(|error| AuthError::Backend(format!("invalid JWKS: {error}")))?;

        let header = jsonwebtoken::decode_header(id_token)
            .map_err(|error| AuthError::Backend(format!("invalid id token: {error}")))?;
        let key = match &header.kid {
            Some(kid) => jwks.find(kid),
            None => jwks.keys.first(),
        }
        .ok_or_else(|| AuthError::Backend("no matching key in the JWKS".to_string()))?;

        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(key)
            .map_err(|error| AuthError::Backend(format!("unusable key in the JWKS: {error}")))?;
        let algorithm = self
            .signature_algorithm
            .parse::<jsonwebtoken::Algorithm>()
            .map_err(|_| {
                AuthError::Backend(format!(
                    "invalid proxy.openid.jwks-signature-algorithm '{}'",
                    self.signature_algorithm
                ))
            })?;

        let mut validation = jsonwebtoken::Validation::new(algorithm);
        validation.set_audience(std::slice::from_ref(&self.client_id));
        let token = jsonwebtoken::decode::<BTreeMap<String, serde_json::Value>>(
            id_token,
            &decoding_key,
            &validation,
        )
        .map_err(|error| AuthError::Backend(format!("the id token is not valid: {error}")))?;
        Ok(token.claims)
    }

    /// Reads the claims of the user info endpoint.
    pub async fn userinfo_claims(
        &self,
        access_token: &str,
    ) -> Result<BTreeMap<String, serde_json::Value>, AuthError> {
        let Some(url) = &self.userinfo_url else {
            return Ok(BTreeMap::new());
        };
        let response = reqwest::Client::new()
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| AuthError::Backend(format!("cannot read the user info: {error}")))?;
        if !response.status().is_success() {
            tracing::warn!(
                "Error while loading user info: the endpoint answered {}",
                response.status()
            );
            return Ok(BTreeMap::new());
        }
        response
            .json()
            .await
            .map_err(|error| AuthError::Backend(format!("invalid user info: {error}")))
    }

    /// Builds the user of the claims of the id token and the user info.
    pub fn user(
        &self,
        id_claims: &BTreeMap<String, serde_json::Value>,
        userinfo_claims: &BTreeMap<String, serde_json::Value>,
    ) -> Result<AuthenticatedUser, AuthError> {
        let mut attributes: BTreeMap<String, serde_json::Value> = id_claims.clone();
        for (name, value) in userinfo_claims {
            attributes.insert(name.clone(), value.clone());
        }

        let username = username_of(&attributes, &self.username_attribute).ok_or_else(|| {
            AuthError::Backend(format!(
                "the claim '{}' (proxy.openid.username-attribute) is missing",
                self.username_attribute
            ))
        })?;

        let mut groups: Vec<String> = Vec::new();
        if let Some(claim) = &self.roles_claim {
            for source in [id_claims, userinfo_claims] {
                for role in parse_roles_claim(source.get(claim)) {
                    let group = normalise_group(role);
                    if !groups.contains(&group) {
                        groups.push(group);
                    }
                }
            }
        }

        Ok(AuthenticatedUser {
            id: username,
            groups,
            attributes,
            kind: AuthKind::Oidc,
        })
    }
}

#[async_trait::async_trait]
impl AuthBackend for OpenIdAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        true
    }

    fn uses_login_form(&self) -> bool {
        false
    }

    fn login_redirect(&self) -> &str {
        // the login page of the Java implementation redirects to the provider
        "login"
    }
}

/// What the browser has to be sent to, and what the session has to remember.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    /// The URL of the provider, with every parameter.
    pub url: String,
    /// The `state` parameter, which is checked when the provider sends the user back.
    pub state: String,
    /// The `nonce` claim the id token must contain.
    pub nonce: String,
    /// The PKCE verifier, when PKCE is used.
    pub verifier: Option<String>,
}

/// The answer of the token endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenResponse {
    /// The id token (a JWT with the claims of the user).
    pub id_token: Option<String>,
    /// The access token, which apps receive in `SHINYPROXY_OIDC_ACCESS_TOKEN`.
    pub access_token: Option<String>,
    /// The refresh token, used to keep the session alive.
    pub refresh_token: Option<String>,
    /// Lifetime of the access token in seconds.
    pub expires_in: Option<i64>,
}

/// The user name of the claims, with the `emails` array of Azure AD B2C handled as in Java.
pub fn username_of(
    claims: &BTreeMap<String, serde_json::Value>,
    attribute: &str,
) -> Option<String> {
    let value = claims.get(attribute)?;
    if attribute == "emails" {
        return match value {
            serde_json::Value::Array(values) => values
                .first()
                .map(value_to_string)
                .filter(|value| !value.is_empty()),
            other => Some(value_to_string(other)),
        };
    }
    let username = value_to_string(value);
    (!username.is_empty()).then_some(username)
}

/// Renders a claim value as a string (strings without their quotes).
fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Parses the claim with the roles, with the rules of `OpenIDAuthenticationBackend.parseRolesClaim`.
pub fn parse_roles_claim(claim: Option<&serde_json::Value>) -> Vec<String> {
    match claim {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(values)) => values.iter().map(value_to_string).collect(),
        Some(serde_json::Value::String(value)) => {
            // a string that contains a JSON list is parsed, anything else yields no roles
            match serde_json::from_str::<serde_json::Value>(value) {
                Ok(serde_json::Value::Array(values)) => {
                    values.iter().map(value_to_string).collect()
                }
                _ => Vec::new(),
            }
        }
        Some(_) => Vec::new(),
    }
}

/// Reads the claims of a JWT without verifying its signature.
fn decode_claims_without_verification(
    token: &str,
) -> Result<BTreeMap<String, serde_json::Value>, AuthError> {
    use base64::Engine;
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| AuthError::Backend("invalid id token".to_string()))?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|error| AuthError::Backend(format!("invalid id token: {error}")))?;
    serde_json::from_slice(&decoded)
        .map_err(|error| AuthError::Backend(format!("invalid id token: {error}")))
}

/// A random string of the given length, used for `state`, `nonce` and the PKCE verifier.
pub fn random_string(length: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// The PKCE challenge of a verifier (`S256`).
pub fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Percent-encodes a query parameter.
fn encode(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    utf8_percent_encode(value, UNRESERVED).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    const CONFIG: &str = r##"
proxy:
  authentication: openid
  openid:
    auth-url: https://idp.example.com/authorize
    token-url: https://idp.example.com/token
    jwks-url: https://idp.example.com/jwks
    userinfo-url: https://idp.example.com/userinfo
    client-id: shinyproxy
    client-secret: secret
    scopes: [ profile, groups ]
    roles-claim: groups
    logout-url: https://idp.example.com/logout
"##;

    #[test]
    fn reads_the_configuration() {
        let backend = OpenIdAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        assert_eq!(backend.name(), "openid");
        assert!(backend.has_authorization());
        assert!(!backend.uses_login_form());
        assert_eq!(
            backend.scopes,
            vec!["openid", "email", "profile", "groups"],
            "the default scopes come first"
        );
        assert_eq!(backend.username_attribute, "email");
        assert_eq!(backend.roles_claim.as_deref(), Some("groups"));
        assert_eq!(backend.signature_algorithm, "RS256");
        assert!(!backend.with_pkce);

        // the default scopes can be switched off
        let backend = OpenIdAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: openid\n  openid:\n    auth-url: a\n    token-url: t\n    \
             client-id: c\n    include-default-scopes: false\n    scopes: [ profile ]\n",
        ))
        .expect("backend");
        assert_eq!(backend.scopes, vec!["profile"]);
    }

    #[test]
    fn requires_the_endpoints_and_the_client_id() {
        let error =
            OpenIdAuthenticationBackend::new(&settings("proxy:\n  authentication: openid\n"))
                .unwrap_err();
        assert_eq!(
            error,
            "OpenID authentication enabled, but no 'proxy.openid.auth-url' defined!"
        );

        let error = OpenIdAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: openid\n  openid:\n    auth-url: a\n",
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "OpenID authentication enabled, but no 'proxy.openid.token-url' defined!"
        );

        let error = OpenIdAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: openid\n  openid:\n    auth-url: a\n    token-url: t\n",
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "OpenID authentication enabled, but no 'proxy.openid.client-id' defined!"
        );
    }

    #[test]
    fn builds_the_authorization_url() {
        let backend = OpenIdAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        let request =
            backend.authorization_request("http://localhost:8080/login/oauth2/code/shinyproxy");

        assert!(
            request
                .url
                .starts_with("https://idp.example.com/authorize?"),
            "{}",
            request.url
        );
        assert!(
            request.url.contains("response_type=code"),
            "{}",
            request.url
        );
        assert!(
            request.url.contains("client_id=shinyproxy"),
            "{}",
            request.url
        );
        assert!(
            request.url.contains(
                "redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Flogin%2Foauth2%2Fcode%2Fshinyproxy"
            ),
            "{}",
            request.url
        );
        assert!(
            request
                .url
                .contains("scope=openid%20email%20profile%20groups"),
            "{}",
            request.url
        );
        assert!(
            request.url.contains(&format!("state={}", request.state)),
            "{}",
            request.url
        );
        assert!(
            request.url.contains(&format!("nonce={}", request.nonce)),
            "{}",
            request.url
        );
        assert!(request.verifier.is_none(), "PKCE is disabled by default");
        assert_eq!(request.state.len(), 32);
        assert_ne!(request.state, request.nonce);

        // with PKCE the challenge is added
        let backend = OpenIdAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: openid\n  openid:\n    auth-url: https://idp/authorize?foo=1\n    \
             token-url: t\n    client-id: c\n    with-pkce: true\n",
        ))
        .expect("backend");
        let request = backend.authorization_request("http://localhost/callback");
        assert!(
            request.url.starts_with("https://idp/authorize?foo=1&"),
            "{}",
            request.url
        );
        let verifier = request.verifier.expect("verifier");
        assert_eq!(verifier.len(), 64);
        assert!(
            request.url.contains(&format!(
                "code_challenge={}",
                encode(&pkce_challenge(&verifier))
            )),
            "{}",
            request.url
        );
        assert!(
            request.url.contains("code_challenge_method=S256"),
            "{}",
            request.url
        );
    }

    #[test]
    fn computes_the_pkce_challenge() {
        // the example of RFC 7636 appendix B
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn parses_the_roles_claim_like_java() {
        assert!(parse_roles_claim(None).is_empty());
        assert!(parse_roles_claim(Some(&serde_json::Value::Null)).is_empty());
        assert_eq!(
            parse_roles_claim(Some(&serde_json::json!(["scientists", "admins"]))),
            vec!["scientists", "admins"]
        );
        // numbers and other values are rendered as strings
        assert_eq!(
            parse_roles_claim(Some(&serde_json::json!(["a", 1, true]))),
            vec!["a", "1", "true"]
        );
        // a string containing a JSON list
        assert_eq!(
            parse_roles_claim(Some(&serde_json::json!("[\"scientists\", \"admins\"]"))),
            vec!["scientists", "admins"]
        );
        // a plain string is not a list of roles
        assert!(parse_roles_claim(Some(&serde_json::json!("scientists"))).is_empty());
        // an object is not supported either
        assert!(parse_roles_claim(Some(&serde_json::json!({"a": "b"}))).is_empty());
    }

    #[test]
    fn builds_the_user_of_the_claims() {
        let backend = OpenIdAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        let id_claims: BTreeMap<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({"email": "jack@example.com", "groups": ["scientists"], "sub": "1"}),
        )
        .expect("claims");
        let userinfo: BTreeMap<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({"groups": ["ROLE_admins"], "name": "Jack"}))
                .expect("claims");

        let user = backend.user(&id_claims, &userinfo).expect("user");
        assert_eq!(user.id, "jack@example.com");
        assert_eq!(
            user.groups,
            vec!["SCIENTISTS", "ADMINS"],
            "the groups of both sources are merged and normalised"
        );
        assert_eq!(user.kind, AuthKind::Oidc);
        assert_eq!(
            user.attributes.get("name"),
            Some(&serde_json::json!("Jack")),
            "the claims are available to expressions"
        );

        // a missing user name claim is an error
        let error = backend
            .user(&BTreeMap::new(), &BTreeMap::new())
            .unwrap_err();
        assert!(error.to_string().contains("email"), "{error}");
    }

    #[test]
    fn supports_the_emails_array_of_azure() {
        let backend = OpenIdAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: openid\n  openid:\n    auth-url: a\n    token-url: t\n    \
             client-id: c\n    username-attribute: emails\n",
        ))
        .expect("backend");
        let claims: BTreeMap<String, serde_json::Value> = serde_json::from_value(
            serde_json::json!({"emails": ["jack@example.com", "other@example.com"]}),
        )
        .expect("claims");
        let user = backend.user(&claims, &BTreeMap::new()).expect("user");
        assert_eq!(user.id, "jack@example.com");

        // a single string works as well
        let claims: BTreeMap<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({"emails": "jack@example.com"}))
                .expect("claims");
        assert_eq!(
            backend.user(&claims, &BTreeMap::new()).expect("user").id,
            "jack@example.com"
        );
    }
}
