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

//! The evaluation context of app definitions (`SpecExpressionContext` in the Java implementation).
//!
//! Expressions in `application.yml` are evaluated against the current user, proxy and app definition.
//! This module exposes those objects to the [`spel`] engine with the same names and properties as the
//! Java implementation, and implements [`SpecResolver`] so that specs can be resolved.

use std::collections::BTreeMap;
use std::sync::Arc;

use spel::value::SpelObject;
use spel::{Context, SpelError, Value};

use crate::model::proxy::Proxy;
use crate::model::spec::{ContainerSpec, ProxySpec};
use crate::model::spel_field::{ResolveError, SpecResolver};

/// The authenticated user, as far as expressions are concerned.
///
/// The authentication backends fill this in; `kind` decides under which name the attributes are
/// exposed (`oidcUser`, `ldapUser`, `samlCredential`, `webServiceUser`), matching the Java context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserContext {
    /// User name.
    pub user_id: String,
    /// Groups of the user, upper-cased without the `ROLE_` prefix (as in Java).
    pub groups: Vec<String>,
    /// Attributes/claims of the user.
    pub attributes: BTreeMap<String, serde_json::Value>,
    /// Which authentication backend the user comes from.
    pub kind: UserKind,
}

/// Authentication backend a user comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserKind {
    /// Simple, none or custom-header authentication: no attributes.
    #[default]
    Simple,
    /// OpenID Connect: exposed as `oidcUser`.
    Oidc,
    /// LDAP: exposed as `ldapUser`.
    Ldap,
    /// SAML: exposed as `samlCredential`.
    Saml,
    /// Web service: exposed as `webServiceUser`.
    WebService,
}

impl UserKind {
    /// Name under which the user is exposed in expressions.
    pub fn context_name(&self) -> Option<&'static str> {
        match self {
            UserKind::Simple => None,
            UserKind::Oidc => Some("oidcUser"),
            UserKind::Ldap => Some("ldapUser"),
            UserKind::Saml => Some("samlCredential"),
            UserKind::WebService => Some("webServiceUser"),
        }
    }
}

impl UserContext {
    /// A user with only a name and groups.
    pub fn new(user_id: impl Into<String>, groups: Vec<String>) -> Self {
        UserContext {
            user_id: user_id.into(),
            groups,
            ..Default::default()
        }
    }
}

/// Builds the evaluation context of an app definition.
#[derive(Debug, Clone, Default)]
pub struct ExpressionContextBuilder {
    user: Option<UserContext>,
    proxy: Option<Proxy>,
    spec: Option<ProxySpec>,
    container_spec: Option<ContainerSpec>,
    server_name: Option<String>,
    json: Option<serde_json::Value>,
    environment: BTreeMap<String, String>,
}

impl ExpressionContextBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        ExpressionContextBuilder::default()
    }

    /// Adds the current user.
    pub fn user(mut self, user: UserContext) -> Self {
        self.user = Some(user);
        self
    }

    /// Adds the proxy that is being started.
    pub fn proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Adds the app definition.
    pub fn spec(mut self, spec: ProxySpec) -> Self {
        self.spec = Some(spec);
        self
    }

    /// Adds the container definition (container level expressions).
    pub fn container_spec(mut self, container_spec: ContainerSpec) -> Self {
        self.container_spec = Some(container_spec);
        self
    }

    /// Adds the host name the request was made to (`serverName`).
    pub fn server_name(mut self, server_name: impl Into<String>) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    /// Adds a JSON document (`json`), used by the web service authentication backend.
    pub fn json(mut self, json: serde_json::Value) -> Self {
        self.json = Some(json);
        self
    }

    /// Sets the environment visible to `T(java.lang.System).getenv(...)`.
    pub fn environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = environment;
        self
    }

    /// Uses the environment of this process.
    pub fn process_environment(self) -> Self {
        self.environment(std::env::vars().collect())
    }

    /// Builds the [`spel`] context.
    pub fn build(self) -> Context {
        let mut context = Context::new().with_environment(self.environment);

        if let Some(user) = &self.user {
            context = context
                .with_root("userId", user.user_id.clone())
                .with_root("groups", user.groups.clone());
            let attributes = Value::Map(
                user.attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(value.clone())))
                    .collect(),
            );
            if let Some(name) = user.kind.context_name() {
                context = context.with_object(
                    name,
                    Arc::new(UserView {
                        user_id: user.user_id.clone(),
                        attributes,
                    }),
                );
            }
        }

        if let Some(proxy) = self.proxy {
            context = context.with_object("proxy", Arc::new(ProxyView { proxy }));
        }
        if let Some(spec) = self.spec {
            context = context.with_object("proxySpec", Arc::new(SpecView { spec }));
        }
        if let Some(container_spec) = self.container_spec {
            context = context.with_object(
                "containerSpec",
                Arc::new(ContainerSpecView { container_spec }),
            );
        }
        if let Some(server_name) = self.server_name {
            context = context.with_root("serverName", server_name);
        }
        if let Some(json) = self.json {
            context = context.with_root("json", Value::from(json));
        }
        context
    }
}

/// A user exposed to expressions (`oidcUser`, `ldapUser`, ...).
#[derive(Debug)]
struct UserView {
    user_id: String,
    attributes: Value,
}

impl SpelObject for UserView {
    fn type_name(&self) -> &'static str {
        "user"
    }

    fn property(&self, name: &str) -> Option<Value> {
        match name {
            // `attributes` and `claims` are the two names used by ShinyProxy configurations
            "attributes" | "claims" | "userInfo" => Some(self.attributes.clone()),
            "name" | "username" | "userId" => Some(Value::Str(self.user_id.clone())),
            other => match &self.attributes {
                Value::Map(entries) => entries.get(other).cloned(),
                _ => None,
            },
        }
    }

    fn to_display(&self) -> String {
        self.user_id.clone()
    }
}

/// A proxy exposed to expressions.
#[derive(Debug)]
struct ProxyView {
    proxy: Proxy,
}

impl SpelObject for ProxyView {
    fn type_name(&self) -> &'static str {
        "proxy"
    }

    fn property(&self, name: &str) -> Option<Value> {
        let proxy = &self.proxy;
        match name {
            "id" => Some(Value::Str(proxy.id.clone())),
            "status" => Some(Value::Str(proxy.status.to_string())),
            "userId" => Some(optional_string(&proxy.user_id)),
            "specId" => Some(optional_string(&proxy.spec_id)),
            "displayName" => Some(optional_string(&proxy.display_name)),
            "targetId" => Some(Value::Str(proxy.target_id().to_string())),
            "startupTimestamp" => Some(Value::Int(proxy.startup_timestamp)),
            "createdTimestamp" => Some(Value::Int(proxy.created_timestamp)),
            "targets" => Some(Value::Map(
                proxy
                    .targets
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::Str(value.clone())))
                    .collect(),
            )),
            "runtimeValues" => Some(Value::Map(
                proxy
                    .runtime_values
                    .internal_json()
                    .into_iter()
                    .map(|(key, value)| (key, Value::Str(value)))
                    .collect(),
            )),
            _ => None,
        }
    }

    fn call(&self, name: &str, arguments: &[Value]) -> Option<Result<Value, SpelError>> {
        match (name, arguments.len()) {
            ("getRuntimeValue", 1) | ("getRuntimeObject", 1) => {
                let key = arguments[0].to_display_string();
                Some(Ok(self
                    .proxy
                    .runtime_values
                    .get_by_env_var(&key)
                    .map(|value| match name {
                        "getRuntimeObject" => Value::from(value.data.to_json()),
                        _ => Value::Str(value.to_value_string()),
                    })
                    .unwrap_or(Value::Null)))
            }
            ("getRuntimeValueOrNull", 1) => {
                let key = arguments[0].to_display_string();
                Some(Ok(self
                    .proxy
                    .runtime_values
                    .get_by_env_var(&key)
                    .map(|value| Value::Str(value.to_value_string()))
                    .unwrap_or(Value::Null)))
            }
            ("getRuntimeValueOrDefault", 2) => {
                let key = arguments[0].to_display_string();
                Some(Ok(self
                    .proxy
                    .runtime_values
                    .get_by_env_var(&key)
                    .map(|value| Value::Str(value.to_value_string()))
                    .unwrap_or_else(|| arguments[1].clone())))
            }
            ("getId", 0) => Some(Ok(Value::Str(self.proxy.id.clone()))),
            ("getUserId", 0) => Some(Ok(optional_string(&self.proxy.user_id))),
            ("getSpecId", 0) => Some(Ok(optional_string(&self.proxy.spec_id))),
            ("getDisplayName", 0) => Some(Ok(optional_string(&self.proxy.display_name))),
            _ => None,
        }
    }

    fn to_display(&self) -> String {
        self.proxy.id.clone()
    }
}

/// An app definition exposed to expressions.
#[derive(Debug)]
struct SpecView {
    spec: ProxySpec,
}

impl SpelObject for SpecView {
    fn type_name(&self) -> &'static str {
        "proxySpec"
    }

    fn property(&self, name: &str) -> Option<Value> {
        match name {
            "id" => Some(Value::Str(self.spec.id.clone())),
            "displayName" => Some(optional_string(&self.spec.display_name)),
            "description" => Some(optional_string(&self.spec.description)),
            "logoURL" | "logoUrl" => Some(optional_string(&self.spec.logo_url)),
            "maxTotalInstances" => Some(Value::Int(self.spec.max_total_instances)),
            "stopOnLogout" => Some(match self.spec.stop_on_logout {
                Some(value) => Value::Bool(value),
                None => Value::Null,
            }),
            _ => None,
        }
    }

    fn call(&self, name: &str, arguments: &[Value]) -> Option<Result<Value, SpelError>> {
        match (name, arguments.len()) {
            ("getId", 0) => Some(Ok(Value::Str(self.spec.id.clone()))),
            ("getDisplayName", 0) => Some(Ok(optional_string(&self.spec.display_name))),
            _ => None,
        }
    }

    fn to_display(&self) -> String {
        self.spec.id.clone()
    }
}

/// A container definition exposed to expressions.
#[derive(Debug)]
struct ContainerSpecView {
    container_spec: ContainerSpec,
}

impl SpelObject for ContainerSpecView {
    fn type_name(&self) -> &'static str {
        "containerSpec"
    }

    fn property(&self, name: &str) -> Option<Value> {
        let spec = &self.container_spec;
        match name {
            "index" => Some(Value::Int(spec.index)),
            "image" => Some(spel_string(spec.image.original())),
            "memoryLimit" => Some(spel_string(spec.memory_limit.original())),
            "memoryRequest" => Some(spel_string(spec.memory_request.original())),
            "cpuLimit" => Some(spel_string(spec.cpu_limit.original())),
            "cpuRequest" => Some(spel_string(spec.cpu_request.original())),
            "network" => Some(spel_string(spec.network.original())),
            "privileged" => Some(Value::Bool(spec.privileged)),
            _ => None,
        }
    }
}

fn optional_string(value: &Option<String>) -> Value {
    match value {
        Some(value) => Value::Str(value.clone()),
        None => Value::Null,
    }
}

fn spel_string(value: Option<&String>) -> Value {
    match value {
        Some(value) => Value::Str(value.clone()),
        None => Value::Null,
    }
}

/// Resolves the expressions of app definitions using the [`spel`] engine.
pub struct SpelResolver {
    context: Context,
}

impl SpelResolver {
    /// Creates a resolver for the given context.
    pub fn new(context: Context) -> Self {
        SpelResolver { context }
    }

    /// The context expressions are evaluated against.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Evaluates a template into a string.
    pub fn evaluate_to_string(&self, raw: &str) -> Result<String, ResolveError> {
        spel::evaluate_to_string(raw, &self.context).map_err(to_resolve_error)
    }

    /// Evaluates a template into a list of strings.
    pub fn evaluate_to_list(&self, raw: &str) -> Result<Vec<String>, ResolveError> {
        spel::evaluate_to_list(raw, &self.context).map_err(to_resolve_error)
    }

    /// Evaluates a template into a boolean (access expressions).
    pub fn boolean_expression(&self, raw: &str) -> Result<bool, ResolveError> {
        spel::evaluate_to_boolean(raw, &self.context).map_err(to_resolve_error)
    }

    /// Evaluates a template into an integer (max instances, lifetimes, ...).
    pub fn integer_expression(&self, raw: &str) -> Result<i64, ResolveError> {
        spel::evaluate_to_integer(raw, &self.context).map_err(to_resolve_error)
    }
}

impl SpecResolver for SpelResolver {
    fn string(&self, raw: &str) -> Result<String, ResolveError> {
        spel::evaluate_to_string(raw, &self.context).map_err(to_resolve_error)
    }

    fn integer(&self, raw: &str) -> Result<i64, ResolveError> {
        spel::evaluate_to_integer(raw, &self.context).map_err(to_resolve_error)
    }

    fn boolean(&self, raw: &str) -> Result<bool, ResolveError> {
        spel::evaluate_to_boolean(raw, &self.context).map_err(to_resolve_error)
    }
}

fn to_resolve_error(error: SpelError) -> ResolveError {
    ResolveError::new(error.expression.clone(), error.message.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::proxy::{Proxy, ProxyStatus};
    use crate::model::runtime_value::{RuntimeValue, USER_GROUPS, USER_ID};
    use crate::model::spel_field::{SpelString, SpelStringMap};

    fn user() -> UserContext {
        UserContext {
            user_id: "jack".into(),
            groups: vec!["SCIENTISTS".into()],
            attributes: BTreeMap::from([
                ("dept".to_string(), serde_json::json!("research")),
                ("quota".to_string(), serde_json::json!(4)),
            ]),
            kind: UserKind::Oidc,
        }
    }

    fn proxy() -> Proxy {
        let mut proxy = Proxy::new("proxy-1", ProxyStatus::Up);
        proxy.user_id = Some("jack".into());
        proxy.spec_id = Some("01_hello".into());
        proxy.display_name = Some("Hello".into());
        proxy.add_runtime_value(RuntimeValue::string(&USER_ID, "jack"), false);
        proxy.add_runtime_value(RuntimeValue::string(&USER_GROUPS, "SCIENTISTS"), false);
        proxy
    }

    fn resolver() -> SpelResolver {
        SpelResolver::new(
            ExpressionContextBuilder::new()
                .user(user())
                .proxy(proxy())
                .spec(ProxySpec::new("01_hello"))
                .server_name("shinyproxy.example.com")
                .environment(BTreeMap::from([(
                    "HOME".to_string(),
                    "/home/sp".to_string(),
                )]))
                .build(),
        )
    }

    #[test]
    fn exposes_the_java_context_names() {
        let resolver = resolver();
        assert_eq!(resolver.string("#{userId}").unwrap(), "jack");
        assert_eq!(resolver.string("#{groups[0]}").unwrap(), "SCIENTISTS");
        assert_eq!(
            resolver.string("#{oidcUser.attributes['dept']}").unwrap(),
            "research"
        );
        assert_eq!(resolver.string("#{oidcUser.claims['quota']}").unwrap(), "4");
        assert_eq!(resolver.string("#{proxy.id}").unwrap(), "proxy-1");
        assert_eq!(resolver.string("#{proxy.userId}").unwrap(), "jack");
        assert_eq!(resolver.string("#{proxy.status}").unwrap(), "Up");
        assert_eq!(resolver.string("#{proxySpec.id}").unwrap(), "01_hello");
        assert_eq!(
            resolver.string("#{serverName}").unwrap(),
            "shinyproxy.example.com"
        );
        assert_eq!(
            resolver
                .string("#{T(java.lang.System).getenv('HOME')}")
                .unwrap(),
            "/home/sp"
        );
    }

    #[test]
    fn exposes_runtime_values_of_the_proxy() {
        let resolver = resolver();
        assert_eq!(
            resolver
                .string("#{proxy.getRuntimeValue('SHINYPROXY_USERNAME')}")
                .unwrap(),
            "jack"
        );
        assert_eq!(
            resolver
                .string("#{proxy.getRuntimeValueOrDefault('MISSING', 'fallback')}")
                .unwrap(),
            "fallback"
        );
        assert_eq!(
            resolver
                .string("#{proxy.runtimeValues['SHINYPROXY_USERGROUPS']}")
                .unwrap(),
            "SCIENTISTS"
        );
    }

    #[test]
    fn resolves_a_spec_end_to_end() {
        let mut spec = ProxySpec::new("01_hello");
        spec.container_specs = vec![ContainerSpec {
            image: SpelString::raw("registry/#{userId}-app".into()),
            memory_limit: SpelString::raw("#{groups.contains('SCIENTISTS') ? '4g' : '2g'}".into()),
            env: SpelStringMap::raw(BTreeMap::from([
                ("USER".to_string(), "#{userId}".to_string()),
                (
                    "DEPT".to_string(),
                    "#{oidcUser.attributes['dept']}".to_string(),
                ),
            ])),
            ..Default::default()
        }];
        spec.max_lifetime = crate::model::spel_field::SpelLong::raw(
            "#{groups.contains('SCIENTISTS') ? 120 : 30}".into(),
        );
        spec.set_container_index();

        let resolver = resolver();
        let spec = spec.first_resolve(&resolver).expect("first resolve");
        let spec = spec.final_resolve(&resolver).expect("final resolve");

        let container = spec.container_spec().expect("container");
        assert_eq!(container.image.as_str(), Some("registry/jack-app"));
        assert_eq!(container.memory_limit.as_str(), Some("4g"));
        assert_eq!(
            container
                .env
                .value()
                .unwrap()
                .get("USER")
                .map(String::as_str),
            Some("jack")
        );
        assert_eq!(
            container
                .env
                .value()
                .unwrap()
                .get("DEPT")
                .map(String::as_str),
            Some("research")
        );
        assert_eq!(spec.max_lifetime.value(), Some(&120));
    }

    #[test]
    fn reports_errors_with_the_expression() {
        let resolver = resolver();
        let error = resolver.string("#{unknownValue}").unwrap_err();
        assert!(error.message.contains("available values"), "{error}");
        assert_eq!(error.expression, "#{unknownValue}");
    }

    #[test]
    fn users_without_attributes_are_not_exposed_as_oidc() {
        let resolver = SpelResolver::new(
            ExpressionContextBuilder::new()
                .user(UserContext::new("jack", vec!["ADMINS".into()]))
                .build(),
        );
        assert_eq!(resolver.string("#{userId}").unwrap(), "jack");
        let error = resolver.string("#{oidcUser.attributes['x']}").unwrap_err();
        assert!(error.message.contains("not available"), "{error}");
    }
}
