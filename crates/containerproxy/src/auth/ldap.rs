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

//! LDAP authentication (`LDAPAuthenticationBackend`).
//!
//! One or more providers are configured (`proxy.ldap.url` or `proxy.ldap[i].url`) and tried in order, as
//! the Java implementation does with its authentication providers. A user is authenticated by binding as
//! their DN:
//!
//! * `user-dn-pattern` builds the DN directly (`uid={0},ou=people,dc=example,dc=com`), or
//! * `user-search-base` + `user-search-filter` look the DN up with the manager account.
//!
//! The groups come from a search with `group-search-filter` (`(uniqueMember={0})` by default) below
//! `group-search-base`, using the `cn` of the results, exactly like `CNLdapAuthoritiesPopulator`.

use ldap3::{LdapConnAsync, LdapConnSettings, Scope, SearchEntry};

use super::{normalise_group, AuthBackend, AuthError, AuthKind, AuthenticatedUser, LoginForm};
use crate::config::settings::{LdapConfigured, LdapSettings};
use crate::config::Settings;

/// Name of the backend.
pub const NAME: &str = "ldap";

/// The attribute the groups are read from (`groupRoleAttribute` in Java, always `cn`).
const GROUP_ROLE_ATTRIBUTE: &str = "cn";

/// Default filter that finds the groups of a user.
const DEFAULT_GROUP_SEARCH_FILTER: &str = "(uniqueMember={0})";

/// One configured LDAP provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// URL of the server, including the base DN (`ldap://host:389/dc=example,dc=com`).
    pub url: String,
    /// Value of `starttls` (`true`, `simple` or `external`).
    pub starttls: Option<String>,
    /// Pattern that builds the DN of a user.
    pub user_dn_pattern: Option<String>,
    /// Base and filter used to look a user up.
    pub user_search_base: String,
    pub user_search_filter: Option<String>,
    /// Base and filter used to find the groups of a user.
    pub group_search_base: String,
    pub group_search_filter: String,
    /// Account used for the searches.
    pub manager_dn: Option<String>,
    pub manager_password: Option<String>,
}

impl Provider {
    /// Reads one provider from its settings, with the defaults of the Java implementation.
    pub fn of(settings: &LdapSettings) -> Option<Self> {
        let url = settings.url.clone().filter(|url| !url.trim().is_empty())?;
        Some(Provider {
            url,
            starttls: settings
                .starttls
                .clone()
                .map(|value| value.0)
                .filter(|value| !value.trim().is_empty()),
            user_dn_pattern: settings
                .user_dn_pattern
                .clone()
                .filter(|value| !value.trim().is_empty()),
            user_search_base: settings.user_search_base.clone().unwrap_or_default(),
            user_search_filter: settings
                .user_search_filter
                .clone()
                .filter(|value| !value.trim().is_empty()),
            group_search_base: settings.group_search_base.clone().unwrap_or_default(),
            group_search_filter: settings
                .group_search_filter
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_GROUP_SEARCH_FILTER.to_string()),
            manager_dn: settings
                .manager_dn
                .clone()
                .filter(|value| !value.trim().is_empty()),
            manager_password: settings.manager_password.clone(),
        })
    }

    /// The URL without the base DN, and the base DN itself.
    ///
    /// Spring puts the base DN in the URL (`ldap://host:389/dc=example,dc=com`); the Rust client takes
    /// them separately.
    pub fn url_and_base(&self) -> (String, String) {
        let trimmed = self.url.trim_end_matches('/');
        // find the path after the host part (`scheme://host:port/base`)
        let Some(scheme_end) = trimmed.find("://") else {
            return (trimmed.to_string(), String::new());
        };
        let after_scheme = &trimmed[scheme_end + 3..];
        match after_scheme.find('/') {
            Some(index) => (
                trimmed[..scheme_end + 3 + index].to_string(),
                after_scheme[index + 1..].to_string(),
            ),
            None => (trimmed.to_string(), String::new()),
        }
    }

    /// Whether StartTLS is used.
    pub fn uses_starttls(&self) -> bool {
        matches!(
            self.starttls
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("true") | Some("simple") | Some("external")
        )
    }

    /// The DN of a user, when `user-dn-pattern` is configured.
    ///
    /// The pattern is relative to the base DN, exactly as in Spring.
    pub fn user_dn(&self, username: &str) -> Option<String> {
        let pattern = self.user_dn_pattern.as_deref()?;
        let (_, base) = self.url_and_base();
        let dn = pattern.replace("{0}", &escape_dn_value(username));
        Some(if base.is_empty() {
            dn
        } else {
            format!("{dn},{base}")
        })
    }

    /// The filter that finds a user (`user-search-filter` with `{0}` replaced).
    pub fn user_filter(&self, username: &str) -> Option<String> {
        Some(
            self.user_search_filter
                .as_deref()?
                .replace("{0}", &escape_filter_value(username)),
        )
    }

    /// The base of the user search, relative to the base DN of the URL.
    pub fn user_search_dn(&self) -> String {
        let (_, base) = self.url_and_base();
        join_dn(&self.user_search_base, &base)
    }

    /// The base of the group search.
    pub fn group_search_dn(&self) -> String {
        let (_, base) = self.url_and_base();
        join_dn(&self.group_search_base, &base)
    }

    /// The filter that finds the groups of a user (`{0}` is the DN, `{1}` the user name).
    pub fn group_filter(&self, user_dn: &str, username: &str) -> String {
        self.group_search_filter
            .replace("{0}", &escape_filter_value(user_dn))
            .replace("{1}", &escape_filter_value(username))
    }
}

/// Joins a relative DN with the base DN.
fn join_dn(relative: &str, base: &str) -> String {
    match (relative.trim().is_empty(), base.trim().is_empty()) {
        (true, _) => base.to_string(),
        (false, true) => relative.to_string(),
        (false, false) => format!("{relative},{base}"),
    }
}

/// Escapes a value that is used in a DN (RFC 4514).
pub fn escape_dn_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            ',' | '+' | '"' | '\\' | '<' | '>' | ';' | '=' | '#' => {
                escaped.push('\\');
                escaped.push(character);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Escapes a value that is used in a search filter (RFC 4515).
pub fn escape_filter_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '*' => escaped.push_str("\\2a"),
            '(' => escaped.push_str("\\28"),
            ')' => escaped.push_str("\\29"),
            '\\' => escaped.push_str("\\5c"),
            '\0' => escaped.push_str("\\00"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Authenticates users against one or more LDAP servers.
#[derive(Debug, Clone)]
pub struct LdapAuthenticationBackend {
    providers: Vec<Provider>,
}

impl LdapAuthenticationBackend {
    /// Reads the configuration, with the startup error of the Java implementation.
    pub fn new(settings: &Settings) -> Result<Self, String> {
        let providers: Vec<Provider> = match &settings.proxy.ldap {
            LdapConfigured::Single(single) => Provider::of(single).into_iter().collect(),
            LdapConfigured::Multiple(multiple) => {
                multiple.iter().filter_map(Provider::of).collect()
            }
        };
        if providers.is_empty() {
            return Err("Cannot initialize LDAP backend: no LDAP configuration found".to_string());
        }
        Ok(LdapAuthenticationBackend { providers })
    }

    /// The configured providers, in the order they are tried.
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    /// Authenticates a user against one provider.
    async fn authenticate_with(
        provider: &Provider,
        form: &LoginForm,
    ) -> Result<AuthenticatedUser, AuthError> {
        let (url, _) = provider.url_and_base();
        let settings = LdapConnSettings::new().set_starttls(provider.uses_starttls());
        let (connection, mut ldap) = LdapConnAsync::with_settings(settings, &url)
            .await
            .map_err(|error| AuthError::Backend(format!("cannot reach {url}: {error}")))?;
        ldap3::drive!(connection);

        // the DN of the user comes from the pattern, or from a search with the manager account
        let user_dn = match provider.user_dn(&form.username) {
            Some(dn) => dn,
            None => {
                if let Some(manager_dn) = &provider.manager_dn {
                    ldap.simple_bind(
                        manager_dn,
                        provider.manager_password.as_deref().unwrap_or_default(),
                    )
                    .await
                    .map_err(|error| {
                        AuthError::Backend(format!("cannot bind as the manager: {error}"))
                    })?
                    .success()
                    .map_err(|error| {
                        AuthError::Backend(format!("the manager account was refused: {error}"))
                    })?;
                }
                let filter = provider.user_filter(&form.username).ok_or_else(|| {
                    AuthError::Backend(
                        "neither user-dn-pattern nor user-search-filter is configured".to_string(),
                    )
                })?;
                let (entries, _) = ldap
                    .search(
                        &provider.user_search_dn(),
                        Scope::Subtree,
                        &filter,
                        vec!["dn"],
                    )
                    .await
                    .map_err(|error| {
                        AuthError::Backend(format!("the user search failed: {error}"))
                    })?
                    .success()
                    .map_err(|error| {
                        AuthError::Backend(format!("the user search failed: {error}"))
                    })?;
                let entry = entries
                    .into_iter()
                    .next()
                    .ok_or(AuthError::InvalidCredentials)?;
                SearchEntry::construct(entry).dn
            }
        };

        // binding as the user with their password is the actual authentication
        let bind = ldap
            .simple_bind(&user_dn, &form.password)
            .await
            .map_err(|error| AuthError::Backend(format!("cannot bind as {user_dn}: {error}")))?;
        if bind.rc != 0 {
            return Err(AuthError::InvalidCredentials);
        }

        // the groups are searched with the manager account when there is one (the user may not be
        // allowed to read them)
        if let Some(manager_dn) = &provider.manager_dn {
            let _ = ldap
                .simple_bind(
                    manager_dn,
                    provider.manager_password.as_deref().unwrap_or_default(),
                )
                .await;
        }

        let mut groups = Vec::new();
        let filter = provider.group_filter(&user_dn, &form.username);
        match ldap
            .search(
                &provider.group_search_dn(),
                Scope::Subtree,
                &filter,
                vec![GROUP_ROLE_ATTRIBUTE],
            )
            .await
        {
            Ok(result) => match result.success() {
                Ok((entries, _)) => {
                    for entry in entries {
                        let entry = SearchEntry::construct(entry);
                        for value in entry
                            .attrs
                            .get(GROUP_ROLE_ATTRIBUTE)
                            .cloned()
                            .unwrap_or_default()
                        {
                            let group = normalise_group(value);
                            if !groups.contains(&group) {
                                groups.push(group);
                            }
                        }
                    }
                }
                Err(error) => tracing::warn!("the group search failed: {error}"),
            },
            Err(error) => tracing::warn!("the group search failed: {error}"),
        }

        let _ = ldap.unbind().await;

        let mut user = AuthenticatedUser {
            id: form.username.clone(),
            groups,
            kind: AuthKind::Ldap,
            ..Default::default()
        };
        user.attributes
            .insert("dn".to_string(), serde_json::json!(user_dn));
        Ok(user)
    }
}

#[async_trait::async_trait]
impl AuthBackend for LdapAuthenticationBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn has_authorization(&self) -> bool {
        true
    }

    async fn authenticate_async(&self, form: &LoginForm) -> Result<AuthenticatedUser, AuthError> {
        let mut last_error = AuthError::InvalidCredentials;
        for provider in &self.providers {
            match Self::authenticate_with(provider, form).await {
                Ok(user) => return Ok(user),
                Err(AuthError::InvalidCredentials) => last_error = AuthError::InvalidCredentials,
                Err(error) => {
                    tracing::warn!("LDAP provider {} failed: {error}", provider.url);
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings")
    }

    const CONFIG: &str = r##"
proxy:
  authentication: ldap
  ldap:
    url: ldap://ldap.example.com:389/dc=example,dc=com
    user-dn-pattern: uid={0},ou=people
    group-search-base: ou=groups
    group-search-filter: (uniqueMember={0})
    manager-dn: cn=admin,dc=example,dc=com
    manager-password: secret
"##;

    #[test]
    fn reads_a_single_provider() {
        let backend = LdapAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        assert_eq!(backend.name(), "ldap");
        assert!(backend.has_authorization());
        assert!(backend.uses_login_form());
        assert_eq!(backend.providers().len(), 1);

        let provider = &backend.providers()[0];
        assert_eq!(
            provider.url_and_base(),
            (
                "ldap://ldap.example.com:389".to_string(),
                "dc=example,dc=com".to_string()
            )
        );
        assert_eq!(provider.group_search_filter, "(uniqueMember={0})");
        assert!(!provider.uses_starttls());
    }

    #[test]
    fn reads_several_providers() {
        let backend = LdapAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: ldap\n  ldap:\n    - url: ldap://first/dc=a\n      \
             user-dn-pattern: uid={0}\n    - url: ldap://second/dc=b\n      user-dn-pattern: cn={0}\n",
        ))
        .expect("backend");
        assert_eq!(backend.providers().len(), 2);
        assert_eq!(backend.providers()[1].url, "ldap://second/dc=b");
    }

    #[test]
    fn refuses_an_empty_configuration() {
        let error = LdapAuthenticationBackend::new(&settings("proxy:\n  authentication: ldap\n"))
            .unwrap_err();
        assert_eq!(
            error,
            "Cannot initialize LDAP backend: no LDAP configuration found"
        );
    }

    #[test]
    fn builds_dns_and_filters_like_spring() {
        let backend = LdapAuthenticationBackend::new(&settings(CONFIG)).expect("backend");
        let provider = &backend.providers()[0];

        assert_eq!(
            provider.user_dn("jack").as_deref(),
            Some("uid=jack,ou=people,dc=example,dc=com"),
            "the pattern is relative to the base DN of the URL"
        );
        assert_eq!(provider.group_search_dn(), "ou=groups,dc=example,dc=com");
        assert_eq!(
            provider.group_filter("uid=jack,ou=people,dc=example,dc=com", "jack"),
            "(uniqueMember=uid=jack,ou=people,dc=example,dc=com)"
        );

        // a search based configuration
        let backend = LdapAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: ldap\n  ldap:\n    url: ldap://ldap/dc=example,dc=com\n    \
             user-search-base: ou=people\n    user-search-filter: (uid={0})\n",
        ))
        .expect("backend");
        let provider = &backend.providers()[0];
        assert_eq!(provider.user_dn("jack"), None, "no pattern is configured");
        assert_eq!(provider.user_search_dn(), "ou=people,dc=example,dc=com");
        assert_eq!(provider.user_filter("jack").as_deref(), Some("(uid=jack)"));
        assert_eq!(
            provider.group_filter("uid=jack", "jack"),
            "(uniqueMember=uid=jack)",
            "the default filter is used"
        );
    }

    #[test]
    fn escapes_values() {
        assert_eq!(escape_dn_value("jack,jr"), "jack\\,jr");
        assert_eq!(escape_dn_value("a+b=c"), "a\\+b\\=c");
        assert_eq!(escape_filter_value("ja*ck"), "ja\\2ack");
        assert_eq!(escape_filter_value("(jack)"), "\\28jack\\29");

        // the escaping is applied when the DN and the filter are built
        let backend = LdapAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: ldap\n  ldap:\n    url: ldap://ldap/dc=a\n    \
             user-dn-pattern: uid={0}\n    user-search-filter: (uid={0})\n",
        ))
        .expect("backend");
        let provider = &backend.providers()[0];
        assert_eq!(
            provider.user_dn("jack,jr").as_deref(),
            Some("uid=jack\\,jr,dc=a")
        );
        assert_eq!(
            provider.user_filter("ja*ck").as_deref(),
            Some("(uid=ja\\2ack)")
        );
    }

    #[test]
    fn recognises_the_starttls_values() {
        for value in ["true", "simple", "external", "SIMPLE"] {
            let backend = LdapAuthenticationBackend::new(&settings(&format!(
                "proxy:\n  authentication: ldap\n  ldap:\n    url: ldap://ldap/dc=a\n    \
                 starttls: {value}\n"
            )))
            .expect("backend");
            assert!(backend.providers()[0].uses_starttls(), "{value}");
        }
        let backend = LdapAuthenticationBackend::new(&settings(
            "proxy:\n  authentication: ldap\n  ldap:\n    url: ldap://ldap/dc=a\n    starttls: false\n",
        ))
        .expect("backend");
        assert!(!backend.providers()[0].uses_starttls());
    }
}
