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

//! Typed configuration.
//!
//! The property tree produced by [`super::loader`] is deserialized into these structures. Optional
//! fields keep the "not configured" information, and accessor methods apply the same defaults as the
//! Java implementation, so that both the value and its origin remain visible.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::flex::{FlexBool, FlexI64, FlexString, StringList};

/// Default heartbeat rate in milliseconds (`ActiveProxiesService.DEFAULT_RATE`).
pub const DEFAULT_HEARTBEAT_RATE_MS: i64 = 10_000;
/// Default heartbeat timeout in milliseconds (`RuntimeValueService.DEFAULT_TIMEOUT`).
pub const DEFAULT_HEARTBEAT_TIMEOUT_MS: i64 = 60_000;
/// Default time to wait for a container to become reachable, in milliseconds.
pub const DEFAULT_CONTAINER_WAIT_TIMEOUT_MS: i64 = 5_000;
/// Default port ShinyProxy listens on.
pub const DEFAULT_PORT: u16 = 8080;
/// Default address ShinyProxy binds to.
pub const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0";
/// Default management (actuator) port.
pub const DEFAULT_MANAGEMENT_PORT: u16 = 9090;
/// Default `SameSite` cookie policy.
pub const DEFAULT_SAME_SITE_COOKIE: &str = "Lax";
/// Default application name.
pub const DEFAULT_APPLICATION_NAME: &str = "ShinyProxy";
/// Default UI title.
pub const DEFAULT_TITLE: &str = "ShinyProxy";

/// Root of the typed configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    /// Everything below `proxy.*`.
    pub proxy: ProxySettings,
    /// Everything below `server.*`.
    pub server: ServerSettings,
    /// The subset of `spring.*` ShinyProxy uses.
    pub spring: SpringSettings,
    /// Logging configuration.
    pub logging: LoggingSettings,
    /// Actuator/management configuration.
    pub management: ManagementSettings,
    /// OpenAPI/Swagger configuration.
    pub springdoc: SpringdocSettings,
}

/// `proxy.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ProxySettings {
    // --- server ---
    pub port: Option<FlexI64>,
    pub bind_address: Option<String>,
    pub same_site_cookie: Option<String>,
    // --- identity ---
    pub realm_id: Option<String>,
    pub version: Option<FlexI64>,
    // --- ui ---
    pub title: Option<String>,
    pub logo_url: Option<String>,
    pub logo_width: Option<FlexString>,
    pub logo_height: Option<FlexString>,
    pub logo_style: Option<String>,
    pub favicon_path: Option<String>,
    pub landing_page: Option<String>,
    pub hide_navbar: Option<FlexBool>,
    pub body_classes: StringList,
    pub notification_message: Option<String>,
    pub template_path: Option<String>,
    pub my_apps_mode: Option<String>,
    pub default_app_logo_url: Option<String>,
    pub default_app_logo_width: Option<FlexString>,
    pub default_app_logo_height: Option<FlexString>,
    pub default_app_logo_style: Option<String>,
    pub default_app_logo_classes: Option<String>,
    pub support: SupportSettings,
    pub monitoring: MonitoringSettings,
    // --- authentication ---
    pub authentication: Option<String>,
    pub admin_groups: StringList,
    pub admin_users: StringList,
    pub username_case_sensitive: Option<FlexBool>,
    pub users: Vec<UserSettings>,
    pub api_security: ApiSecuritySettings,
    pub oauth2: OAuth2Settings,
    pub openid: OpenIdSettings,
    pub ldap: LdapConfigured,
    pub webservice: WebServiceSettings,
    pub custom_header: CustomHeaderSettings,
    pub ms_graph: MsGraphSettings,
    pub saml: SamlSettings,
    // --- proxy behaviour ---
    pub heartbeat_rate: Option<FlexI64>,
    pub heartbeat_timeout: Option<FlexI64>,
    pub container_wait_time: Option<FlexI64>,
    pub container_wait_timeout: Option<FlexI64>,
    pub max_total_instances: Option<FlexI64>,
    pub default_max_instances: Option<FlexString>,
    pub default_proxy_max_lifetime: Option<FlexI64>,
    pub default_cache_headers_mode: Option<String>,
    pub default_stop_proxy_on_logout: Option<FlexBool>,
    pub default_always_switch_instance: Option<FlexBool>,
    pub default_websocket_reconnection_mode: Option<String>,
    pub default_track_app_url: Option<FlexBool>,
    pub allow_transfer_app: Option<FlexBool>,
    pub stop_proxies_on_shutdown: Option<FlexBool>,
    pub recover_running_proxies: Option<FlexBool>,
    pub recover_running_proxies_from_different_config: Option<FlexBool>,
    pub store_mode: Option<String>,
    pub seat_wait_time: Option<FlexI64>,
    pub log_as_json: Option<FlexBool>,
    // --- backends ---
    pub container_backend: Option<String>,
    pub docker: DockerSettings,
    pub kubernetes: KubernetesSettings,
    pub ecs: EcsSettings,
    // --- container logs ---
    pub container_log_path: Option<String>,
    pub container_log_s3_access_key: Option<String>,
    pub container_log_s3_access_secret: Option<String>,
    pub container_log_s3_endpoint: Option<String>,
    pub container_log_s3_sse: Option<FlexBool>,
    // --- usage statistics ---
    pub usage_stats_url: Option<String>,
    pub usage_stats_username: Option<String>,
    pub usage_stats_password: Option<String>,
    pub usage_stats_table_name: Option<String>,
    pub usage_stats_attributes: Vec<NamedExpression>,
    pub usage_stats: Vec<UsageStatsSettings>,
    pub usage_stats_micrometer_prefix: Option<String>,
    pub usage_stats_hikari: HikariSettings,
    // --- application specific ---
    /// `proxy.specs`, interpreted by the application (ShinyProxy notation).
    pub specs: Vec<Value>,
    /// `proxy.template-groups`, interpreted by the application.
    pub template_groups: Vec<Value>,
}

impl ProxySettings {
    /// Port to listen on (`proxy.port`, default 8080).
    pub fn port(&self) -> u16 {
        self.port
            .map(|value| value.0)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(DEFAULT_PORT)
    }

    /// Address to bind to (`proxy.bind-address`, default `0.0.0.0`).
    pub fn bind_address(&self) -> &str {
        self.bind_address.as_deref().unwrap_or(DEFAULT_BIND_ADDRESS)
    }

    /// `SameSite` policy of the session cookie (`proxy.same-site-cookie`, default `Lax`).
    pub fn same_site_cookie(&self) -> &str {
        self.same_site_cookie
            .as_deref()
            .unwrap_or(DEFAULT_SAME_SITE_COOKIE)
    }

    /// UI title (`proxy.title`, default `ShinyProxy`).
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(DEFAULT_TITLE)
    }

    /// Authentication backend name (`proxy.authentication`, default `none`).
    pub fn authentication(&self) -> &str {
        self.authentication.as_deref().unwrap_or("none")
    }

    /// Container backend name (`proxy.container-backend`, default `docker`).
    pub fn container_backend(&self) -> &str {
        self.container_backend.as_deref().unwrap_or("docker")
    }

    /// Heartbeat rate in milliseconds (`proxy.heartbeat-rate`, default 10000).
    pub fn heartbeat_rate_ms(&self) -> i64 {
        self.heartbeat_rate
            .map(|value| value.0)
            .unwrap_or(DEFAULT_HEARTBEAT_RATE_MS)
    }

    /// Heartbeat timeout in milliseconds (`proxy.heartbeat-timeout`, default 60000).
    pub fn heartbeat_timeout_ms(&self) -> i64 {
        self.heartbeat_timeout
            .map(|value| value.0)
            .unwrap_or(DEFAULT_HEARTBEAT_TIMEOUT_MS)
    }

    /// Time to wait for a container to respond, in milliseconds (`proxy.container-wait-timeout`).
    pub fn container_wait_timeout_ms(&self) -> i64 {
        self.container_wait_timeout
            .or(self.container_wait_time)
            .map(|value| value.0)
            .unwrap_or(DEFAULT_CONTAINER_WAIT_TIMEOUT_MS)
    }

    /// Maximum number of proxies across all users (`proxy.max-total-instances`, default -1 = unlimited).
    pub fn max_total_instances(&self) -> i64 {
        self.max_total_instances.map(|value| value.0).unwrap_or(-1)
    }

    /// Whether proxies are stopped when ShinyProxy shuts down (default true).
    pub fn stop_proxies_on_shutdown(&self) -> bool {
        self.stop_proxies_on_shutdown
            .map(|value| value.0)
            .unwrap_or(true)
    }

    /// Whether usernames are compared case sensitively (default true).
    pub fn username_case_sensitive(&self) -> bool {
        self.username_case_sensitive
            .map(|value| value.0)
            .unwrap_or(true)
    }

    /// Default maximum number of instances per user and app as configured (may be an expression).
    pub fn default_max_instances(&self) -> &str {
        self.default_max_instances
            .as_ref()
            .map(FlexString::as_str)
            .unwrap_or("1")
    }

    /// Landing page (`proxy.landing-page`, default `/`).
    pub fn landing_page(&self) -> &str {
        self.landing_page.as_deref().unwrap_or("/")
    }

    /// Whether the navbar is hidden by default (`proxy.hide-navbar`, default false).
    pub fn hide_navbar(&self) -> bool {
        self.hide_navbar.map(|value| value.0).unwrap_or(false)
    }

    /// Whether apps may be transferred to another user (`proxy.allow-transfer-app`, default false).
    pub fn allow_transfer_app(&self) -> bool {
        self.allow_transfer_app
            .map(|value| value.0)
            .unwrap_or(false)
    }

    /// Store mode (`proxy.store-mode`, default `None`).
    pub fn store_mode(&self) -> &str {
        self.store_mode.as_deref().unwrap_or("None")
    }

    /// Whether app recovery is enabled in one of its two forms.
    pub fn recover_running_proxies(&self) -> bool {
        self.recover_running_proxies
            .map(|value| value.0)
            .unwrap_or(false)
            || self
                .recover_running_proxies_from_different_config
                .map(|value| value.0)
                .unwrap_or(false)
    }

    /// Whether apps are stopped when their user logs out (`proxy.default-stop-proxy-on-logout`,
    /// default true).
    pub fn default_stop_proxy_on_logout(&self) -> bool {
        self.default_stop_proxy_on_logout
            .map(|value| value.0)
            .unwrap_or(true)
    }

    /// Whether logs are emitted as JSON (`proxy.log-as-json`, default false).
    pub fn log_as_json(&self) -> bool {
        self.log_as_json.map(|value| value.0).unwrap_or(false)
    }
}

/// `proxy.support.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SupportSettings {
    pub mail_to_address: Option<String>,
    pub mail_from_address: Option<String>,
    pub mail_subject: Option<String>,
}

/// `proxy.monitoring.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct MonitoringSettings {
    pub grafana_url: Option<String>,
}

/// `proxy.users[]` (simple authentication).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UserSettings {
    pub name: Option<String>,
    pub password: Option<String>,
    pub groups: StringList,
}

/// `proxy.api-security.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ApiSecuritySettings {
    pub hide_spec_details: Option<FlexBool>,
    pub disable_no_sniff_header: Option<FlexBool>,
    pub disable_hsts_header: Option<FlexBool>,
    pub disable_xss_protection_header: Option<FlexBool>,
    pub cors_allowed_origins: StringList,
    pub custom_headers: Vec<NameValue>,
}

impl ApiSecuritySettings {
    /// Whether spec details are hidden in the API (default true).
    pub fn hide_spec_details(&self) -> bool {
        self.hide_spec_details.map(|value| value.0).unwrap_or(true)
    }
}

/// A `name`/`value` pair.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct NameValue {
    pub name: Option<String>,
    pub value: Option<String>,
}

/// A `name`/`expression` pair (usage statistics attributes).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct NamedExpression {
    pub name: Option<String>,
    pub expression: Option<String>,
}

/// `proxy.oauth2.*` (bearer token authentication).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct OAuth2Settings {
    pub resource_id: Option<String>,
    pub jwks_url: Option<String>,
    pub roles_claim: Option<String>,
    pub username_attribute: Option<String>,
}

/// `proxy.openid.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct OpenIdSettings {
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub jwks_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub logout_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub client_authentication_method: Option<String>,
    pub scopes: StringList,
    pub username_attribute: Option<String>,
    pub roles_claim: Option<String>,
    pub with_pkce: Option<FlexBool>,
    pub include_default_scopes: Option<FlexBool>,
    pub enforce_https_redirect_uri: Option<FlexBool>,
    pub ignore_session_expire: Option<FlexBool>,
    pub jwks_signature_algorithm: Option<String>,
}

/// `proxy.ldap` accepts a single provider or a list of providers.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LdapConfigured {
    /// A list of providers (`proxy.ldap[0].url`, ...).
    Multiple(Vec<LdapSettings>),
    /// A single provider (`proxy.ldap.url`, ...).
    Single(Box<LdapSettings>),
}

impl Default for LdapConfigured {
    fn default() -> Self {
        LdapConfigured::Multiple(Vec::new())
    }
}

impl LdapConfigured {
    /// All configured providers, in configuration order.
    pub fn providers(&self) -> Vec<&LdapSettings> {
        match self {
            LdapConfigured::Multiple(providers) => providers.iter().collect(),
            LdapConfigured::Single(provider) => {
                if provider.url.is_some() {
                    vec![provider.as_ref()]
                } else {
                    Vec::new()
                }
            }
        }
    }
}

/// One LDAP provider.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LdapSettings {
    pub url: Option<String>,
    pub starttls: Option<String>,
    pub user_dn_pattern: Option<String>,
    pub user_search_base: Option<String>,
    pub user_search_filter: Option<String>,
    pub group_search_base: Option<String>,
    pub group_search_filter: Option<String>,
    pub manager_dn: Option<String>,
    pub manager_password: Option<String>,
}

/// `proxy.webservice.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct WebServiceSettings {
    pub authentication_url: Option<String>,
    pub authentication_request_body: Option<String>,
    pub groups_expression: Option<String>,
}

/// `proxy.custom-header.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CustomHeaderSettings {
    pub username_header_name: Option<String>,
    pub groups_header_name: Option<String>,
}

impl CustomHeaderSettings {
    /// Header carrying the user name (default `REMOTE_USER`).
    pub fn username_header_name(&self) -> &str {
        self.username_header_name
            .as_deref()
            .unwrap_or("REMOTE_USER")
    }
}

/// `proxy.ms-graph.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct MsGraphSettings {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub tenant_id: Option<String>,
    pub api_url: Option<String>,
    pub token_url: Option<String>,
    pub scopes: StringList,
}

/// `proxy.saml.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SamlSettings {
    pub app_entity_id: Option<String>,
    pub app_base_url: Option<String>,
    pub idp_metadata_url: Option<String>,
    pub keystore: Option<String>,
    pub keystore_password: Option<String>,
    pub encryption_cert_name: Option<String>,
    pub encryption_cert_password: Option<String>,
    pub name_attribute: Option<String>,
    pub roles_attribute: Option<String>,
    pub force_authn: Option<FlexBool>,
    pub log_attributes: Option<FlexBool>,
    pub logout_url: Option<String>,
    pub logout_method: Option<String>,
}

/// `proxy.docker.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DockerSettings {
    pub url: Option<String>,
    pub cert_path: Option<String>,
    pub port_range_start: Option<FlexI64>,
    pub port_range_max: Option<FlexI64>,
    pub target_url: Option<String>,
    pub target_bind_ip: Option<String>,
    pub default_container_network: Option<String>,
    pub internal_networking: Option<FlexBool>,
    pub container_protocol: Option<String>,
    pub privileged: Option<FlexBool>,
    pub image_pull_policy: Option<String>,
    pub loki_url: Option<String>,
    pub service_wait_time: Option<FlexI64>,
}

impl DockerSettings {
    /// First port of the range used to publish container ports (default 20000).
    pub fn port_range_start(&self) -> u16 {
        self.port_range_start
            .map(|value| value.0)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(20_000)
    }

    /// Last port of the range, or `None` for "no limit" (Java default: -1).
    pub fn port_range_max(&self) -> Option<u16> {
        match self.port_range_max.map(|value| value.0) {
            Some(value) if value > 0 => u16::try_from(value).ok(),
            _ => None,
        }
    }

    /// IP the published ports are bound to (default `127.0.0.1`).
    pub fn target_bind_ip(&self) -> &str {
        self.target_bind_ip.as_deref().unwrap_or("127.0.0.1")
    }

    /// Whether containers are reached over the internal container network (default false).
    pub fn internal_networking(&self) -> bool {
        self.internal_networking
            .map(|value| value.0)
            .unwrap_or(false)
    }
}

/// `proxy.kubernetes.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KubernetesSettings {
    pub url: Option<String>,
    pub cert_path: Option<String>,
    pub namespace: Option<String>,
    pub api_version: Option<String>,
    pub image_pull_policy: Option<String>,
    pub image_pull_secrets: StringList,
    pub image_pull_secret: Option<String>,
    pub node_selector: BTreeMap<String, String>,
    pub cluster_domain: Option<String>,
    pub internal_networking: Option<FlexBool>,
    pub container_protocol: Option<String>,
    pub privileged: Option<FlexBool>,
    pub pod_wait_time: Option<FlexI64>,
    pub debug_patches: Option<FlexBool>,
    pub authorized_pod_patches: StringList,
    pub authorized_additional_manifests: StringList,
    pub authorized_additional_persistent_manifests: StringList,
}

impl KubernetesSettings {
    /// Namespace pods are created in (default `default`).
    pub fn namespace(&self) -> &str {
        self.namespace.as_deref().unwrap_or("default")
    }

    /// Cluster domain used to build in-cluster URLs (default `cluster.local`).
    pub fn cluster_domain(&self) -> &str {
        self.cluster_domain.as_deref().unwrap_or("cluster.local")
    }
}

/// `proxy.ecs.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EcsSettings {
    pub name: Option<String>,
    pub region: Option<String>,
    pub service_wait_time: Option<FlexI64>,
    pub subnets: StringList,
    pub security_groups: StringList,
    #[serde(alias = "enable-cloudwatch")]
    pub enable_cloud_watch: Option<FlexBool>,
    pub cloud_watch_group_prefix: Option<String>,
    pub cloud_watch_region: Option<String>,
    pub cloud_watch_stream_prefix: Option<String>,
    pub default_repository_credentials_parameter: Option<String>,
    pub privileged: Option<FlexBool>,
    pub internal_networking: Option<FlexBool>,
}

/// `proxy.usage-stats[]`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UsageStatsSettings {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub table_name: Option<String>,
    pub attributes: Vec<NamedExpression>,
}

/// `proxy.usage-stats-hikari.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct HikariSettings {
    pub connection_timeout: Option<FlexI64>,
    pub idle_timeout: Option<FlexI64>,
    pub max_lifetime: Option<FlexI64>,
    pub minimum_idle: Option<FlexI64>,
    pub maximum_pool_size: Option<FlexI64>,
}

/// `server.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ServerSettings {
    pub servlet: ServletSettings,
    pub secure_cookies: Option<FlexBool>,
    pub frame_options: Option<String>,
    /// Removed in ShinyProxy 3.x; a warning is logged when it is present.
    pub use_forward_headers: Option<String>,
}

impl ServerSettings {
    /// Context path (`server.servlet.context-path`), normalised without a trailing slash.
    pub fn context_path(&self) -> String {
        let raw = self.servlet.context_path.clone().unwrap_or_default();
        let trimmed = raw.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            String::new()
        } else if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{trimmed}")
        }
    }

    /// Whether cookies are marked `Secure` (default false).
    pub fn secure_cookies(&self) -> bool {
        self.secure_cookies.map(|value| value.0).unwrap_or(false)
    }

    /// `X-Frame-Options` behaviour (default `disable`).
    pub fn frame_options(&self) -> &str {
        self.frame_options.as_deref().unwrap_or("disable")
    }
}

/// `server.servlet.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ServletSettings {
    pub context_path: Option<String>,
}

/// The `spring.*` subset ShinyProxy uses.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SpringSettings {
    pub application: ApplicationSettings,
    pub session: SessionSettings,
    pub data: DataSettings,
    pub mail: MailSettings,
    pub profiles: ProfileSettings,
}

/// `spring.application.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ApplicationSettings {
    pub name: Option<String>,
}

/// `spring.profiles.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ProfileSettings {
    pub active: Option<String>,
}

/// `spring.session.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SessionSettings {
    pub store_type: Option<String>,
    pub timeout: Option<String>,
    pub redis: RedisSessionSettings,
}

impl SessionSettings {
    /// Whether sessions are stored in Redis.
    pub fn is_redis(&self) -> bool {
        self.store_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("redis"))
    }
}

/// `spring.session.redis.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RedisSessionSettings {
    pub flush_mode: Option<String>,
    pub repository_type: Option<String>,
}

/// `spring.data.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DataSettings {
    pub redis: RedisSettings,
}

/// `spring.data.redis.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RedisSettings {
    pub host: Option<String>,
    pub port: Option<FlexI64>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: Option<FlexI64>,
    pub sentinel: SentinelSettings,
}

/// `spring.data.redis.sentinel.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SentinelSettings {
    pub master: Option<String>,
    pub nodes: StringList,
    pub password: Option<String>,
}

/// `spring.mail.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct MailSettings {
    pub host: Option<String>,
    pub port: Option<FlexI64>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub properties: BTreeMap<String, Value>,
}

/// `logging.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LoggingSettings {
    pub file: LoggingFileSettings,
    pub level: BTreeMap<String, String>,
    pub include_application_name: Option<FlexBool>,
    pub requestdump: Option<FlexBool>,
}

/// `logging.file.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LoggingFileSettings {
    pub name: Option<String>,
}

/// `management.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementSettings {
    pub server: ManagementServerSettings,
    pub endpoints: ManagementEndpointsSettings,
    pub endpoint: ManagementEndpointSettings,
    pub health: ManagementHealthSettings,
    pub defaults: ManagementDefaultsSettings,
}

impl ManagementSettings {
    /// Port of the management server (default 9090).
    pub fn port(&self) -> u16 {
        self.server
            .port
            .map(|value| value.0)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(DEFAULT_MANAGEMENT_PORT)
    }
}

/// `management.server.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementServerSettings {
    pub port: Option<FlexI64>,
}

/// `management.endpoints.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementEndpointsSettings {
    pub web: ManagementWebSettings,
}

/// `management.endpoints.web.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementWebSettings {
    pub exposure: ManagementExposureSettings,
}

/// `management.endpoints.web.exposure.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementExposureSettings {
    pub include: Option<String>,
}

/// `management.endpoint.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementEndpointSettings {
    pub health: ManagementHealthEndpointSettings,
}

/// `management.endpoint.health.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementHealthEndpointSettings {
    pub probes: ManagementProbesSettings,
    pub group: BTreeMap<String, Value>,
}

/// `management.endpoint.health.probes.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementProbesSettings {
    pub enabled: Option<FlexBool>,
}

/// `management.health.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementHealthSettings {
    pub ldap: ManagementToggle,
    pub redis: ManagementToggle,
}

/// `management.defaults.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementDefaultsSettings {
    pub metrics: ManagementMetricsSettings,
}

/// `management.defaults.metrics.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementMetricsSettings {
    pub export: ManagementToggle,
}

/// A simple `enabled` toggle.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ManagementToggle {
    pub enabled: Option<FlexBool>,
}

/// `springdoc.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SpringdocSettings {
    pub api_docs: SpringdocToggle,
    pub swagger_ui: SpringdocToggle,
}

/// `springdoc.{api-docs,swagger-ui}.*`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SpringdocToggle {
    pub enabled: Option<FlexBool>,
}

impl SpringdocToggle {
    /// Whether the endpoint is enabled (default false, as in the Java default properties).
    pub fn enabled(&self) -> bool {
        self.enabled.map(|value| value.0).unwrap_or(false)
    }
}

impl Settings {
    /// Application name (`spring.application.name`, default `ShinyProxy`).
    pub fn application_name(&self) -> &str {
        self.spring
            .application
            .name
            .as_deref()
            .unwrap_or(DEFAULT_APPLICATION_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Settings {
        serde_yaml_ng::from_str(yaml).expect("settings parse")
    }

    #[test]
    fn defaults_match_java() {
        let settings = Settings::default();
        assert_eq!(settings.proxy.port(), 8080);
        assert_eq!(settings.proxy.bind_address(), "0.0.0.0");
        assert_eq!(settings.proxy.same_site_cookie(), "Lax");
        assert_eq!(settings.proxy.authentication(), "none");
        assert_eq!(settings.proxy.container_backend(), "docker");
        assert_eq!(settings.proxy.heartbeat_rate_ms(), 10_000);
        assert_eq!(settings.proxy.heartbeat_timeout_ms(), 60_000);
        assert_eq!(settings.proxy.container_wait_timeout_ms(), 5_000);
        assert_eq!(settings.proxy.max_total_instances(), -1);
        assert!(settings.proxy.stop_proxies_on_shutdown());
        assert!(settings.proxy.username_case_sensitive());
        assert!(!settings.proxy.allow_transfer_app());
        assert_eq!(settings.proxy.landing_page(), "/");
        assert_eq!(settings.proxy.store_mode(), "None");
        assert!(settings.proxy.api_security.hide_spec_details());
        assert_eq!(settings.proxy.docker.port_range_start(), 20_000);
        assert_eq!(settings.proxy.docker.target_bind_ip(), "127.0.0.1");
        assert_eq!(settings.proxy.kubernetes.namespace(), "default");
        assert_eq!(settings.management.port(), 9090);
        assert_eq!(settings.application_name(), "ShinyProxy");
        assert_eq!(settings.server.context_path(), "");
        assert_eq!(settings.server.frame_options(), "disable");
        assert!(!settings.springdoc.swagger_ui.enabled());
    }

    #[test]
    fn parses_demo_configuration() {
        let settings = parse(include_str!("../../../../examples/application-demo.yml"));
        assert_eq!(settings.proxy.title(), "Open Analytics Shiny Proxy");
        assert_eq!(settings.proxy.port(), 8080);
        assert_eq!(settings.proxy.authentication(), "simple");
        assert_eq!(settings.proxy.heartbeat_rate_ms(), 10_000);
        assert_eq!(settings.proxy.admin_groups.values(), ["scientists"]);
        assert_eq!(settings.proxy.users.len(), 2);
        assert_eq!(settings.proxy.users[0].name.as_deref(), Some("jack"));
        assert_eq!(settings.proxy.users[0].groups.values(), ["scientists"]);
        assert_eq!(settings.proxy.docker.port_range_start(), 20_000);
        assert_eq!(settings.proxy.specs.len(), 2);
        assert_eq!(settings.proxy.specs[0]["id"], "01_hello");
        assert_eq!(
            settings.logging.file.name.as_deref(),
            Some("shinyproxy.log")
        );
    }

    #[test]
    fn normalises_context_path() {
        let settings = parse("server:\n  servlet:\n    context-path: shinyproxy/\n");
        assert_eq!(settings.server.context_path(), "/shinyproxy");
        let settings = parse("server:\n  servlet:\n    context-path: /sub/path\n");
        assert_eq!(settings.server.context_path(), "/sub/path");
        let settings = parse("server:\n  servlet:\n    context-path: /\n");
        assert_eq!(settings.server.context_path(), "");
    }

    #[test]
    fn supports_single_and_multiple_ldap_providers() {
        let single =
            parse("proxy:\n  ldap:\n    url: ldap://localhost:389\n    user-dn-pattern: uid={0}\n");
        assert_eq!(single.proxy.ldap.providers().len(), 1);
        assert_eq!(
            single.proxy.ldap.providers()[0].user_dn_pattern.as_deref(),
            Some("uid={0}")
        );

        let multiple = parse(
            "proxy:\n  ldap:\n    - url: ldap://one:389\n    - url: ldap://two:389\n      manager-dn: cn=admin\n",
        );
        let providers = multiple.proxy.ldap.providers();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[1].manager_dn.as_deref(), Some("cn=admin"));
    }

    #[test]
    fn accepts_ecs_cloudwatch_alias() {
        let settings = parse("proxy:\n  ecs:\n    enable-cloudwatch: true\n");
        assert_eq!(settings.proxy.ecs.enable_cloud_watch, Some(FlexBool(true)));
    }
}
