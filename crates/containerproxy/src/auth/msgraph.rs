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

//! Groups from the Microsoft Graph API (`MicrosoftGraphGroupFetcher`).
//!
//! Azure AD only puts a limited number of groups in the id token, so ShinyProxy can ask Microsoft Graph
//! for the memberships of the user instead. When `proxy.ms-graph.client-id` is configured, the groups of
//! the token are *replaced* by the display names of `/v1.0/{tenant}/users/{oid}/memberOf`, exactly like the
//! authorities mapper of the Java implementation does (which also warns and continues without groups when
//! the `oid` claim is missing or the API fails).

use std::time::Duration;

use serde::Deserialize;

use super::normalise_group;
use crate::config::Settings;

/// Fetches the groups of a user from Microsoft Graph.
pub struct MicrosoftGraphGroupFetcher {
    client_id: String,
    client_secret: Option<String>,
    token_url: String,
    api_url: String,
    tenant_id: String,
    scopes: Vec<String>,
    /// The client credentials token, cached until it expires.
    token: moka::future::Cache<(), String>,
}

impl std::fmt::Debug for MicrosoftGraphGroupFetcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MicrosoftGraphGroupFetcher")
            .field("client_id", &self.client_id)
            .field("api_url", &self.api_url)
            .field("tenant_id", &self.tenant_id)
            .finish()
    }
}

/// The answer of the `memberOf` endpoint.
#[derive(Debug, Deserialize)]
struct MemberOfResponse {
    #[serde(default)]
    value: Vec<Membership>,
}

/// One membership of the answer.
#[derive(Debug, Deserialize)]
struct Membership {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

/// The answer of the token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl MicrosoftGraphGroupFetcher {
    /// Creates the fetcher when `proxy.ms-graph.client-id` is configured.
    pub fn from_settings(settings: &Settings) -> Option<Result<Self, String>> {
        let graph = &settings.proxy.ms_graph;
        let client_id = graph
            .client_id
            .clone()
            .filter(|value| !value.trim().is_empty())?;

        let token_url = match graph
            .token_url
            .clone()
            .filter(|value| !value.trim().is_empty())
        {
            Some(url) => url,
            None => return Some(Err(
                "Microsoft Graph groups are enabled, but no 'proxy.ms-graph.token-url' defined!"
                    .to_string(),
            )),
        };
        let tenant_id = match graph
            .tenant_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        {
            Some(tenant) => tenant,
            None => return Some(Err(
                "Microsoft Graph groups are enabled, but no 'proxy.ms-graph.tenant-id' defined!"
                    .to_string(),
            )),
        };

        let mut scopes: Vec<String> = graph.scopes.values().to_vec();
        if scopes.is_empty() {
            scopes.push("https://graph.microsoft.com/.default".to_string());
        }

        Some(Ok(MicrosoftGraphGroupFetcher {
            client_id,
            client_secret: graph
                .client_secret
                .clone()
                .filter(|value| !value.trim().is_empty()),
            token_url,
            api_url: graph
                .api_url
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://graph.microsoft.com".to_string())
                .trim_end_matches('/')
                .to_string(),
            tenant_id,
            scopes,
            token: moka::future::Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(300))
                .build(),
        }))
    }

    /// The groups of a user, by their object id (the `oid` claim of the id token).
    ///
    /// Failures are logged and yield no groups, as in the Java implementation.
    pub async fn groups(&self, object_id: &str) -> Vec<String> {
        let token = match self.token().await {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(
                    "Error while fetching groups from Microsoft Graph API - continuing without \
                     groups: {error}"
                );
                return Vec::new();
            }
        };

        let url = format!(
            "{}/v1.0/{}/users/{object_id}/memberOf",
            self.api_url, self.tenant_id
        );
        let response = match reqwest::Client::new()
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    "Error while fetching groups from Microsoft Graph API - continuing without \
                     groups: {error}"
                );
                return Vec::new();
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            tracing::warn!(
                "Error from Microsoft Graph API, status: {status}, response: {body} - continuing \
                 without groups"
            );
            return Vec::new();
        }

        let memberships: MemberOfResponse = match serde_json::from_str(&body) {
            Ok(memberships) => memberships,
            Err(error) => {
                tracing::warn!(
                    "Error while fetching groups from Microsoft Graph API - continuing without \
                     groups: {error}"
                );
                return Vec::new();
            }
        };

        let mut groups = Vec::new();
        for membership in memberships.value {
            let Some(name) = membership.display_name else {
                continue;
            };
            let group = normalise_group(name);
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        if groups.is_empty() {
            tracing::warn!("No group memberships found for {object_id}");
        }
        groups
    }

    /// A client credentials token for the Graph API, from the cache when it is still valid.
    async fn token(&self) -> Result<String, String> {
        if let Some(cached) = self.token.get(&()).await {
            return Ok(cached);
        }

        let form = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", self.client_id.clone()),
            ("scope", self.scopes.join(" ")),
        ];
        let mut request = reqwest::Client::new().post(&self.token_url).form(&form);
        if let Some(secret) = &self.client_secret {
            request = request.basic_auth(&self.client_id, Some(secret));
        }

        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("the token endpoint answered {status}: {body}"));
        }
        let token: TokenResponse =
            serde_json::from_str(&body).map_err(|error| error.to_string())?;

        // the token is cached a minute short of its lifetime
        let lifetime = token
            .expires_in
            .map(|seconds| Duration::from_secs(seconds.saturating_sub(60).max(1)))
            .unwrap_or_else(|| Duration::from_secs(300));
        let cache = moka::future::Cache::builder()
            .max_capacity(1)
            .time_to_live(lifetime)
            .build();
        cache.insert((), token.access_token.clone()).await;
        self.token.insert((), token.access_token.clone()).await;
        Ok(token.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    #[test]
    fn is_only_enabled_when_a_client_id_is_configured() {
        assert!(MicrosoftGraphGroupFetcher::from_settings(&Settings::default()).is_none());

        let error = MicrosoftGraphGroupFetcher::from_settings(&settings(
            "proxy:\n  ms-graph:\n    client-id: the-client\n",
        ))
        .expect("configured")
        .unwrap_err();
        assert_eq!(
            error,
            "Microsoft Graph groups are enabled, but no 'proxy.ms-graph.token-url' defined!"
        );

        let error = MicrosoftGraphGroupFetcher::from_settings(&settings(
            "proxy:\n  ms-graph:\n    client-id: the-client\n    token-url: https://login/token\n",
        ))
        .expect("configured")
        .unwrap_err();
        assert_eq!(
            error,
            "Microsoft Graph groups are enabled, but no 'proxy.ms-graph.tenant-id' defined!"
        );

        let fetcher = MicrosoftGraphGroupFetcher::from_settings(&settings(
            "proxy:\n  ms-graph:\n    client-id: the-client\n    client-secret: the-secret\n    \
             token-url: https://login/token\n    tenant-id: the-tenant\n",
        ))
        .expect("configured")
        .expect("fetcher");
        assert_eq!(fetcher.api_url, "https://graph.microsoft.com");
        assert_eq!(fetcher.scopes, vec!["https://graph.microsoft.com/.default"]);
        assert_eq!(fetcher.tenant_id, "the-tenant");
    }

    #[tokio::test]
    async fn fetches_the_groups_of_a_user() {
        // a fake Graph API with a token endpoint and a memberOf endpoint
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let app = axum::Router::new()
                .route(
                    "/token",
                    axum::routing::post(|| async {
                        axum::Json(serde_json::json!({
                            "access_token": "graph-token",
                            "expires_in": 3600,
                        }))
                    }),
                )
                .route(
                    "/v1.0/the-tenant/users/the-oid/memberOf",
                    axum::routing::get(|headers: axum::http::HeaderMap| async move {
                        // the request must carry the client credentials token
                        assert_eq!(
                            headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok()),
                            Some("Bearer graph-token")
                        );
                        axum::Json(serde_json::json!({
                            "value": [
                                {"displayName": "scientists"},
                                {"displayName": "ROLE_admins"},
                                {"id": "no-display-name"}
                            ]
                        }))
                    }),
                );
            axum::serve(listener, app).await.ok();
        });

        let fetcher = MicrosoftGraphGroupFetcher::from_settings(&settings(&format!(
            "proxy:\n  ms-graph:\n    client-id: the-client\n    client-secret: the-secret\n    \
             token-url: http://{address}/token\n    tenant-id: the-tenant\n    \
             api-url: http://{address}\n"
        )))
        .expect("configured")
        .expect("fetcher");

        let groups = fetcher.groups("the-oid").await;
        assert_eq!(groups, vec!["SCIENTISTS", "ADMINS"]);

        // an unknown user yields no groups instead of an error
        let groups = fetcher.groups("unknown").await;
        assert!(groups.is_empty());

        server.abort();
    }
}
