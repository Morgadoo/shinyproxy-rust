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

//! Shared state of the ShinyProxy server.

use std::collections::BTreeMap;
use std::sync::Arc;

use containerproxy::auth::{self, AuthBackend, AuthenticatedUser};
use containerproxy::config::{RawConfig, Settings};
use containerproxy::model::spec::ProxySpec;
use containerproxy::service::Identifiers;
use containerproxy::spec::expression::{ExpressionContextBuilder, SpelResolver};
use containerproxy::spec::SpecProvider;
use containerproxy::web::{SecurityHeaders, TemplateEngine};

use crate::spec_provider::{ShinyProxySpecProvider, SpecError};
use crate::web::model::LogoInfo;

/// Everything the request handlers need.
pub struct AppState {
    /// The raw configuration (for dynamic property lookups).
    pub raw: RawConfig,
    /// The typed configuration.
    pub settings: Settings,
    /// Identifiers of this server and its configuration.
    pub identifiers: Identifiers,
    /// The app definitions.
    pub specs: ShinyProxySpecProvider,
    /// The authentication backend.
    pub auth: Arc<dyn AuthBackend>,
    /// The template engine.
    pub templates: TemplateEngine,
    /// Security headers added to every response.
    pub security_headers: SecurityHeaders,
    /// Whether the container backend supports pausing apps (false until the backends land in P5/P8).
    pub pause_supported: bool,
    /// Cached logo data URIs, keyed by the configured URL.
    logo_cache: dashmap::DashMap<String, Option<String>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("instance_id", &self.identifiers.instance_id)
            .field("specs", &self.specs.specs().len())
            .field("auth", &self.auth.name())
            .finish()
    }
}

/// Errors while building the state.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(transparent)]
    Specs(#[from] SpecError),
    #[error(transparent)]
    Auth(#[from] auth::UnsupportedBackend),
    #[error(transparent)]
    Templates(#[from] containerproxy::web::TemplateError),
}

impl AppState {
    /// Builds the state from the loaded configuration.
    pub fn new(raw: RawConfig, settings: Settings) -> Result<Self, StateError> {
        let identifiers =
            Identifiers::from_config(&raw, std::env::var("SP_KUBE_POD_NAME").ok().as_deref());
        let specs = ShinyProxySpecProvider::from_settings(&settings)?;
        let auth = auth::create(&settings)?;
        let templates = TemplateEngine::new(
            settings
                .proxy
                .template_path
                .as_ref()
                .map(std::path::PathBuf::from),
        )?;
        let security_headers = SecurityHeaders::from_settings(&settings);

        Ok(AppState {
            raw,
            settings,
            identifiers,
            specs,
            auth,
            templates,
            security_headers,
            pause_supported: false,
            logo_cache: dashmap::DashMap::new(),
        })
    }

    /// The context path, always ending with a slash (`ContextPathHelper.withEndingSlash`).
    pub fn context_path_with_slash(&self) -> String {
        let path = self.settings.server.context_path();
        if path.is_empty() {
            "/".to_string()
        } else {
            format!("{path}/")
        }
    }

    /// The context path without a trailing slash (empty for the root).
    pub fn context_path(&self) -> String {
        self.settings.server.context_path()
    }

    /// Builds an expression resolver for the given user.
    pub fn resolver(&self, user: Option<&AuthenticatedUser>) -> SpelResolver {
        let mut builder = ExpressionContextBuilder::new().process_environment();
        if let Some(user) = user {
            builder = builder.user(user.to_user_context());
        }
        SpelResolver::new(builder.build())
    }

    /// The title of the UI, with expressions resolved (`proxy.title`).
    pub fn resolve_title(&self, user: Option<&AuthenticatedUser>) -> String {
        let title = self.settings.proxy.title();
        if !spel::contains_expression(title) {
            return title.to_string();
        }
        match self.resolver(user).evaluate_to_string(title) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("cannot resolve proxy.title: {error}");
                title.to_string()
            }
        }
    }

    /// The logo of the UI, with expressions resolved and `file://` URLs inlined (`proxy.logo-url`).
    pub fn resolve_logo(&self, user: Option<&AuthenticatedUser>) -> Option<String> {
        let logo = self.settings.proxy.logo_url.as_deref()?;
        let resolved = if spel::contains_expression(logo) {
            match self.resolver(user).evaluate_to_string(logo) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!("cannot resolve proxy.logo-url: {error}");
                    return None;
                }
            }
        } else {
            logo.to_string()
        };
        self.resolve_image_uri(&resolved)
    }

    /// Resolves an image URI, inlining `file://` URLs as data URIs (`BaseController.resolveImageURI`).
    pub fn resolve_image_uri(&self, uri: &str) -> Option<String> {
        if uri.trim().is_empty() {
            return None;
        }
        if let Some(cached) = self.logo_cache.get(uri) {
            return cached.clone();
        }
        let resolved = if uri.to_ascii_lowercase().starts_with("file://") {
            inline_file_uri(uri)
        } else {
            Some(uri.to_string())
        };
        self.logo_cache.insert(uri.to_string(), resolved.clone());
        resolved
    }

    /// Logo information of an app, falling back to the configured defaults.
    pub fn app_logo(&self, spec: &ProxySpec) -> Option<LogoInfo> {
        let proxy = &self.settings.proxy;
        let src = spec
            .logo_url
            .as_deref()
            .and_then(|url| self.resolve_image_uri(url))
            .or_else(|| {
                proxy
                    .default_app_logo_url
                    .as_deref()
                    .and_then(|url| self.resolve_image_uri(url))
            })?;
        Some(LogoInfo {
            src,
            width: spec
                .logo_width
                .clone()
                .or_else(|| proxy.default_app_logo_width.clone().map(String::from)),
            height: spec
                .logo_height
                .clone()
                .or_else(|| proxy.default_app_logo_height.clone().map(String::from)),
            style: spec
                .logo_style
                .clone()
                .or_else(|| proxy.default_app_logo_style.clone()),
            classes: spec
                .logo_classes
                .clone()
                .or_else(|| proxy.default_app_logo_classes.clone()),
        })
    }

    /// The URL of an app on the index page (`Thymeleaf.getAppUrl`).
    pub fn app_url(&self, spec: &ProxySpec) -> String {
        let external = ShinyProxySpecProvider::external(spec).external_url;
        if let Some(url) = external.filter(|url| !url.trim().is_empty()) {
            return url;
        }
        let mut url = format!("{}app/{}", self.context_path_with_slash(), spec.id);
        if ShinyProxySpecProvider::hide_navbar_on_main_page_link(spec) {
            url.push_str("?sp_hide_navbar=true");
        }
        url
    }

    /// Whether the user may access the app (`ProxyAccessControlService.canAccess`).
    ///
    /// Access expressions are evaluated with the user's context; the strict expression must always
    /// hold, exactly like in the Java `AccessControlEvaluationService`.
    pub fn can_access(&self, user: Option<&AuthenticatedUser>, spec: &ProxySpec) -> bool {
        let access = &spec.access_control;

        if self.auth.has_authorization() && user.is_none() {
            // anonymous users may only access apps when authentication is disabled
            return false;
        }

        if access.has_strict_expression_access() {
            let expression = access.strict_expression.clone().unwrap_or_default();
            match self.resolver(user).boolean_expression(&expression) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(error) => {
                    tracing::warn!(
                        "cannot evaluate access-strict-expression of {}: {error}",
                        spec.id
                    );
                    return false;
                }
            }
        }

        if access.is_open() {
            return true;
        }

        if let Some(user) = user {
            if access.groups.iter().any(|group| user.is_member_of(group)) {
                return true;
            }
            if access
                .users
                .iter()
                .any(|allowed| self.username_equals(&user.id, allowed))
            {
                return true;
            }
        }

        if access.has_expression_access() {
            let expression = access.expression.clone().unwrap_or_default();
            return match self.resolver(user).boolean_expression(&expression) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!("cannot evaluate access-expression of {}: {error}", spec.id);
                    false
                }
            };
        }

        false
    }

    /// Compares user names honouring `proxy.username-case-sensitive`.
    pub fn username_equals(&self, left: &str, right: &str) -> bool {
        if self.settings.proxy.username_case_sensitive() {
            left == right
        } else {
            left.eq_ignore_ascii_case(right)
        }
    }

    /// Whether the user is an administrator (`UserService.isAdmin`).
    pub fn is_admin(&self, user: Option<&AuthenticatedUser>) -> bool {
        if !self.auth.has_authorization() {
            return false;
        }
        let Some(user) = user else { return false };
        let proxy = &self.settings.proxy;
        proxy
            .admin_groups
            .values()
            .iter()
            .any(|group| user.is_member_of(group))
            || proxy
                .admin_users
                .values()
                .iter()
                .any(|admin| self.username_equals(&user.id, admin))
    }

    /// Maximum number of instances per app for this user (`ShinyProxySpecProvider.getMaxInstances`).
    pub fn max_instances(&self, user: Option<&AuthenticatedUser>) -> BTreeMap<String, i64> {
        let resolver = self.resolver(user);
        let default = self.settings.proxy.default_max_instances().to_string();
        let default = resolver
            .integer_expression(&default)
            .unwrap_or_else(|error| {
                tracing::warn!("cannot resolve proxy.default-max-instances: {error}");
                1
            });

        self.specs
            .specs()
            .iter()
            .map(|spec| {
                let configured = ShinyProxySpecProvider::extension(spec).max_instances;
                let value = match configured.original() {
                    Some(raw) => resolver.integer_expression(raw).unwrap_or_else(|error| {
                        tracing::warn!("cannot resolve max-instances of {}: {error}", spec.id);
                        default
                    }),
                    None => default,
                };
                (spec.id.clone(), value)
            })
            .collect()
    }
}

/// Reads a `file://` URL and returns it as a data URI.
fn inline_file_uri(uri: &str) -> Option<String> {
    let path = uri.trim_start_matches("file://");
    let mime = mime_guess::from_path(path).first();
    let Some(mime) = mime else {
        tracing::warn!("Cannot determine mimetype for resource: {uri}");
        return None;
    };
    match std::fs::read(path) {
        Ok(data) => {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(data);
            Some(format!("data:{mime};base64,{encoded}"))
        }
        Err(error) => {
            tracing::warn!("Failed to convert file URI to data URI: {uri} ({error})");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use containerproxy::config::LoadOptions;

    fn build_state(yaml: &str) -> AppState {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("application.yml");
        std::fs::write(&path, yaml).expect("write");
        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (raw, settings) = crate::load_config(options).expect("config");
        AppState::new(raw, settings).expect("state")
    }

    #[test]
    fn resolves_title_and_logo_expressions() {
        let state = build_state(
            "proxy:\n  title: 'Proxy of #{userId}'\n  logo-url: 'https://example.com/#{userId}.png'\n  authentication: simple\n  users:\n    - name: jack\n      password: pw\n  specs: []\n",
        );
        let user = AuthenticatedUser::new("jack", vec![]);
        assert_eq!(state.resolve_title(Some(&user)), "Proxy of jack");
        assert_eq!(
            state.resolve_logo(Some(&user)).as_deref(),
            Some("https://example.com/jack.png")
        );
    }

    #[test]
    fn inlines_file_logos_as_data_uris() {
        let directory = tempfile::tempdir().expect("temp dir");
        let logo = directory.path().join("logo.png");
        std::fs::write(&logo, b"\x89PNG\r\n\x1a\nfake").expect("write logo");
        let state = build_state(&format!(
            "proxy:\n  logo-url: 'file://{}'\n  authentication: none\n  specs: []\n",
            logo.display()
        ));
        let resolved = state.resolve_logo(None).expect("logo");
        assert!(resolved.starts_with("data:image/png;base64,"), "{resolved}");
    }

    #[test]
    fn evaluates_access_control() {
        let state = build_state(
            "proxy:\n  authentication: simple\n  username-case-sensitive: false\n  users:\n    - name: jack\n      password: pw\n      groups: scientists\n  specs:\n    - id: open\n      container-image: img\n    - id: by-group\n      container-image: img\n      access-groups: SCIENTISTS\n    - id: by-user\n      container-image: img\n      access-users: JACK\n    - id: by-expression\n      container-image: img\n      access-expression: \"#{userId == 'jack'}\"\n    - id: denied\n      container-image: img\n      access-groups: admins\n    - id: strict\n      container-image: img\n      access-groups: scientists\n      access-strict-expression: \"#{userId == 'jill'}\"\n",
        );
        let user = AuthenticatedUser::new("jack", vec!["scientists".into()]);
        let accessible: Vec<&str> = state
            .specs
            .specs()
            .iter()
            .filter(|spec| state.can_access(Some(&user), spec))
            .map(|spec| spec.id.as_str())
            .collect();
        assert_eq!(accessible, ["open", "by-group", "by-user", "by-expression"]);
    }

    #[test]
    fn anonymous_users_only_have_access_without_authentication() {
        let state = build_state("proxy:\n  authentication: none\n  specs:\n    - id: open\n      container-image: img\n");
        assert!(state.can_access(None, &state.specs.specs()[0]));

        let state = build_state(
            "proxy:\n  authentication: simple\n  users:\n    - name: jack\n      password: pw\n  specs:\n    - id: open\n      container-image: img\n",
        );
        assert!(!state.can_access(None, &state.specs.specs()[0]));
    }

    #[test]
    fn detects_administrators() {
        let state = build_state(
            "proxy:\n  authentication: simple\n  admin-groups: [ admins ]\n  admin-users: [ root ]\n  users:\n    - name: jack\n      password: pw\n  specs: []\n",
        );
        assert!(state.is_admin(Some(&AuthenticatedUser::new("x", vec!["admins".into()]))));
        assert!(state.is_admin(Some(&AuthenticatedUser::new("root", vec![]))));
        assert!(!state.is_admin(Some(&AuthenticatedUser::new(
            "jack",
            vec!["scientists".into()]
        ))));
        assert!(!state.is_admin(None));

        // without authentication nobody is an administrator
        let state =
            build_state("proxy:\n  authentication: none\n  admin-users: [ root ]\n  specs: []\n");
        assert!(!state.is_admin(Some(&AuthenticatedUser::new("root", vec![]))));
    }

    #[test]
    fn resolves_max_instances_per_app() {
        let state = build_state(
            "proxy:\n  authentication: simple\n  default-max-instances: 2\n  users:\n    - name: jack\n      password: pw\n      groups: scientists\n  specs:\n    - id: default\n      container-image: img\n    - id: fixed\n      container-image: img\n      max-instances: 5\n    - id: expression\n      container-image: img\n      max-instances: \"#{groups.contains('SCIENTISTS') ? 10 : 1}\"\n",
        );
        let user = AuthenticatedUser::new("jack", vec!["scientists".into()]);
        let instances = state.max_instances(Some(&user));
        assert_eq!(instances.get("default"), Some(&2));
        assert_eq!(instances.get("fixed"), Some(&5));
        assert_eq!(instances.get("expression"), Some(&10));
    }

    #[test]
    fn builds_app_urls() {
        let state = build_state(
            "proxy:\n  authentication: none\n  specs:\n    - id: plain\n      container-image: img\n    - id: hidden\n      container-image: img\n      hide-navbar-on-main-page-link: true\n    - id: external\n      external-url: https://example.com/app\n",
        );
        let urls: Vec<String> = state
            .specs
            .specs()
            .iter()
            .map(|spec| state.app_url(spec))
            .collect();
        assert_eq!(
            urls,
            [
                "/app/plain".to_string(),
                "/app/hidden?sp_hide_navbar=true".to_string(),
                "https://example.com/app".to_string()
            ]
        );
    }
}
