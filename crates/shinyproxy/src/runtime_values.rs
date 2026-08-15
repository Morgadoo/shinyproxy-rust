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

//! The runtime values ShinyProxy adds to a proxy.
//!
//! These are the six keys of `eu.openanalytics.shinyproxy.runtimevalues`, with the same labels, env var
//! names and flags (verified against the Java sources).

use containerproxy::model::runtime_value::{RuntimeValueKey, ValueKind};

/// Name of the app instance (`_` for the default instance).
pub static APP_INSTANCE: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-app-instance",
    env_var: "SHINYPROXY_APP_INSTANCE",
    include_as_label: false,
    // included as annotation so that the value can be recovered
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: true,
    required: true,
    container_specific: false,
    kind: ValueKind::Str,
};

/// Whether the browser reloads the whole page when the app reconnects.
pub static FORCE_FULL_RELOAD: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-shiny-force-full-reload",
    env_var: "SHINYPROXY_FORCE_FULL_RELOAD",
    include_as_label: false,
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: true,
    required: true,
    container_specific: false,
    kind: ValueKind::Bool,
};

/// How the browser reconnects when the WebSocket connection is lost.
pub static WEBSOCKET_RECONNECTION_MODE: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-websocket-reconnection-mode",
    env_var: "SHINYPROXY_WEBSOCKET_RECONNECTION_MODE",
    include_as_label: false,
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: true,
    required: false,
    container_specific: false,
    kind: ValueKind::Str,
};

/// Time zone of the user, as reported by the browser.
pub static USER_TIMEZONE: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-user-timezone",
    env_var: "SHINYPROXY_USER_TIMEZONE",
    include_as_label: false,
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: true,
    required: true,
    container_specific: false,
    kind: ValueKind::Str,
};

/// Whether the URL of the app is tracked in the browser address bar.
pub static TRACK_APP_URL: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-track-app-url",
    env_var: "SHINYPROXY_TRACK_APP_URL",
    include_as_label: false,
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: true,
    required: true,
    container_specific: false,
    kind: ValueKind::Bool,
};

/// Extra details of the app, shown in the "App details" dialog.
pub static CUSTOM_APP_DETAILS: RuntimeValueKey = RuntimeValueKey {
    label: "openanalytics.eu/sp-custom-app-details",
    env_var: "SHINYPROXY_CUSTOM_APP_DETAILS",
    include_as_label: false,
    include_as_annotation: true,
    include_as_env: false,
    include_in_api: false,
    required: true,
    container_specific: false,
    kind: ValueKind::Json,
};

/// All runtime value keys of ShinyProxy.
pub static SHINYPROXY_KEYS: &[&RuntimeValueKey] = &[
    &APP_INSTANCE,
    &FORCE_FULL_RELOAD,
    &WEBSOCKET_RECONNECTION_MODE,
    &USER_TIMEZONE,
    &TRACK_APP_URL,
    &CUSTOM_APP_DETAILS,
];

/// Name of the default app instance.
pub const DEFAULT_INSTANCE: &str = "_";

/// Display name of an instance (`Default` for the default instance), as used by the admin page.
pub fn instance_display_name(instance: &str) -> &str {
    if instance == DEFAULT_INSTANCE {
        "Default"
    } else {
        instance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use containerproxy::model::runtime_value::RuntimeValueRegistry;

    #[test]
    fn keys_are_registered_next_to_the_engine_keys() {
        let registry = RuntimeValueRegistry::engine().with_keys(SHINYPROXY_KEYS);
        assert_eq!(
            registry
                .by_env_var("SHINYPROXY_APP_INSTANCE")
                .map(|key| key.label),
            Some("openanalytics.eu/sp-app-instance")
        );
        assert!(registry.by_env_var("SHINYPROXY_PROXY_ID").is_some());
        assert_eq!(registry.keys().len(), 29);
    }

    #[test]
    fn only_api_safe_values_are_exposed() {
        assert!(APP_INSTANCE.include_in_api);
        assert!(FORCE_FULL_RELOAD.include_in_api);
        assert!(
            !CUSTOM_APP_DETAILS.include_in_api,
            "may contain sensitive values"
        );
    }

    #[test]
    fn no_shinyproxy_value_is_injected_into_containers() {
        for key in SHINYPROXY_KEYS {
            assert!(
                !key.include_as_env,
                "{} must not be an env var",
                key.env_var
            );
            assert!(
                key.include_as_annotation,
                "{} must be recoverable",
                key.env_var
            );
        }
    }

    #[test]
    fn maps_instance_names_for_display() {
        assert_eq!(instance_display_name("_"), "Default");
        assert_eq!(instance_display_name("my-instance"), "my-instance");
    }
}
