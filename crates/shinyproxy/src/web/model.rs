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

//! The model of the ShinyProxy pages.
//!
//! This is the Rust counterpart of `BaseController.prepareMap`: it collects everything the templates
//! and the front-end JavaScript need. The attribute names are part of the contract with the (unchanged)
//! JavaScript, so they keep their Java spelling.

use std::collections::BTreeMap;

use containerproxy::auth::AuthenticatedUser;
use containerproxy::model::spec::ProxySpec;
use containerproxy::spec::SpecProvider;
use containerproxy::util::clean_html;
use serde_json::{json, Map, Value};

use super::state::AppState;
use crate::spec_provider::ShinyProxySpecProvider;

/// Information about the logo of an app or of ShinyProxy itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LogoInfo {
    /// URL or data URI of the image.
    pub src: String,
    /// Width attribute, when configured.
    pub width: Option<String>,
    /// Height attribute, when configured.
    pub height: Option<String>,
    /// Style attribute, when configured.
    pub style: Option<String>,
    /// Extra CSS classes, when configured.
    pub classes: Option<String>,
}

/// Which page is being rendered (used by the navbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The app overview.
    Index,
    /// The page of a single app.
    App,
    /// The admin page.
    Admin,
    /// Any other page.
    Other,
}

impl Page {
    fn as_str(&self) -> &'static str {
        match self {
            Page::Index => "index",
            Page::App => "app",
            Page::Admin => "admin",
            Page::Other => "",
        }
    }
}

/// Builds the model of a page.
pub fn prepare_model(
    state: &AppState,
    page: Page,
    user: Option<&AuthenticatedUser>,
    hide_navbar_requested: bool,
) -> Map<String, Value> {
    let settings = &state.settings;
    let proxy = &settings.proxy;
    let context_path = state.context_path_with_slash();
    let resource_prefix = format!(
        "{}/{}",
        context_path.trim_end_matches('/'),
        state.identifiers.instance_id
    );

    let accessible: Vec<&ProxySpec> = state
        .specs
        .specs()
        .iter()
        .filter(|spec| state.can_access(user, spec))
        .collect();

    let mut model = Map::new();
    model.insert("title".into(), json!(state.resolve_title(user)));
    model.insert("logo".into(), json!(state.resolve_logo(user)));
    model.insert(
        "application_name".into(),
        json!(settings.application_name()),
    );
    model.insert(
        "showNavbar".into(),
        json!(!hide_navbar_requested && !proxy.hide_navbar()),
    );
    model.insert("bootstrapCss".into(), json!("/css/bootstrap.css"));
    model.insert("bootstrapJs".into(), json!("/js/bootstrap.js"));
    model.insert(
        "jqueryJs".into(),
        json!("/webjars/jquery/3.7.1/jquery.min.js"),
    );
    model.insert(
        "handlebars".into(),
        json!("/webjars/handlebars/4.7.9/dist/handlebars.runtime.min.js"),
    );
    model.insert(
        "fontAwesomeCss".into(),
        json!("/webjars/fontawesome/4.7.0/css/font-awesome.min.css"),
    );
    model.insert(
        "isLoggedIn".into(),
        json!(user.is_some() && state.auth.has_authorization()),
    );
    model.insert("isAdmin".into(), json!(state.is_admin(user)));
    model.insert(
        "isSupportEnabled".into(),
        json!(user.is_some() && proxy.support.mail_to_address.is_some()),
    );
    model.insert("logoutUrl".into(), json!(format!("{context_path}logout")));
    model.insert("page".into(), json!(page.as_str()));
    model.insert("maxInstances".into(), json!(0));
    model.insert("contextPath".into(), json!(context_path));
    model.insert("resourcePrefix".into(), json!(resource_prefix));
    model.insert("spInstance".into(), json!(state.identifiers.instance_id));
    model.insert("allowTransferApp".into(), json!(proxy.allow_transfer_app()));
    model.insert("pauseSupported".into(), json!(state.pause_supported));
    model.insert(
        "notificationMessage".into(),
        match &proxy.notification_message {
            Some(message) => json!(clean_html(message)),
            None => Value::Null,
        },
    );
    model.insert(
        "bodyClasses".into(),
        json!(proxy.body_classes.values().join(" ")),
    );
    model.insert(
        "myAppsMode".into(),
        json!(proxy
            .my_apps_mode
            .clone()
            .unwrap_or_else(|| "None".to_string())),
    );
    model.insert(
        "userId".into(),
        match user {
            Some(user) => json!(user.id),
            None => Value::Null,
        },
    );

    // per app values
    let max_instances = state.max_instances(user);
    model.insert("appMaxInstances".into(), json!(max_instances));

    let apps: Vec<Value> = accessible.iter().map(|spec| app_value(spec)).collect();
    let app_ids: Vec<String> = accessible.iter().map(|spec| spec.id.clone()).collect();
    model.insert("apps".into(), json!(apps));
    model.insert("appIds".into(), json!(app_ids));

    let mut app_logos = Map::new();
    let mut app_urls = Map::new();
    let mut clean_descriptions = Map::new();
    let mut switch_instead_of_app = Map::new();
    for spec in &accessible {
        if let Some(logo) = state.app_logo(spec) {
            app_logos.insert(
                spec.id.clone(),
                serde_json::to_value(logo).unwrap_or(Value::Null),
            );
        }
        app_urls.insert(spec.id.clone(), json!(state.app_url(spec)));
        clean_descriptions.insert(
            spec.id.clone(),
            json!(clean_html(spec.description.as_deref().unwrap_or(""))),
        );
        switch_instead_of_app.insert(
            spec.id.clone(),
            json!(ShinyProxySpecProvider::always_show_switch_instance(
                spec,
                state
                    .settings
                    .proxy
                    .default_always_switch_instance
                    .map(|value| value.0)
                    .unwrap_or(false)
            )),
        );
    }
    model.insert("appLogos".into(), Value::Object(app_logos));
    model.insert("appUrl".into(), Value::Object(app_urls));
    model.insert("cleanDescription".into(), Value::Object(clean_descriptions));
    model.insert(
        "openSwitchInstanceInsteadOfApp".into(),
        Value::Object(switch_instead_of_app),
    );

    // grouping
    let (grouped, ungrouped, groups) = group_apps(state, &accessible);
    model.insert("groupedApps".into(), grouped);
    model.insert("ungroupedApps".into(), ungrouped);
    model.insert("templateGroups".into(), groups);

    model
}

fn app_value(spec: &ProxySpec) -> Value {
    json!({
        "id": spec.id,
        "displayName": spec.display_name,
        "description": spec.description,
    })
}

/// Groups the apps according to the configured template groups (`Thymeleaf.groupApps`).
fn group_apps(state: &AppState, apps: &[&ProxySpec]) -> (Value, Value, Value) {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut ungrouped: Vec<Value> = Vec::new();

    for spec in apps {
        match ShinyProxySpecProvider::extension(spec).template_group {
            Some(group) if !group.is_empty() => {
                grouped.entry(group).or_default().push(app_value(spec));
            }
            _ => ungrouped.push(app_value(spec)),
        }
    }

    // only groups that actually contain an app are rendered, and in configuration order
    let groups: Vec<Value> = state
        .specs
        .template_groups()
        .iter()
        .filter(|group| grouped.contains_key(&group.id))
        .map(|group| json!({"id": group.id, "properties": group.properties}))
        .collect();

    (
        serde_json::to_value(grouped).unwrap_or(Value::Null),
        Value::Array(ungrouped),
        Value::Array(groups),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::state::AppState;
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

    fn user() -> AuthenticatedUser {
        AuthenticatedUser::new("jack", vec!["scientists".into()])
    }

    #[test]
    fn builds_the_index_model() {
        let state = build_state(
            "proxy:\n  title: My Proxy\n  authentication: simple\n  admin-groups: admins\n  users:\n    - name: jack\n      password: pw\n      groups: scientists\n  specs:\n    - id: 01_hello\n      display-name: Hello\n      description: 'A <b>demo</b><script>bad()</script>'\n      container-image: img\n      access-groups: scientists\n    - id: 02_secret\n      container-image: img\n      access-groups: admins\n",
        );
        let model = prepare_model(&state, Page::Index, Some(&user()), false);

        assert_eq!(model["title"], json!("My Proxy"));
        assert_eq!(model["application_name"], json!("ShinyProxy"));
        assert_eq!(model["page"], json!("index"));
        assert_eq!(model["isLoggedIn"], json!(true));
        assert_eq!(model["isAdmin"], json!(false));
        assert_eq!(model["showNavbar"], json!(true));
        assert_eq!(model["userId"], json!("jack"));
        assert_eq!(model["contextPath"], json!("/"));
        assert_eq!(model["logoutUrl"], json!("/logout"));
        assert_eq!(model["myAppsMode"], json!("None"));

        // only apps the user may access are listed
        assert_eq!(model["appIds"], json!(["01_hello"]));
        assert_eq!(model["apps"][0]["displayName"], json!("Hello"));
        assert_eq!(model["appUrl"]["01_hello"], json!("/app/01_hello"));
        assert_eq!(
            model["cleanDescription"]["01_hello"],
            json!("A <b>demo</b>"),
            "descriptions are sanitised"
        );
        assert_eq!(model["ungroupedApps"].as_array().map(Vec::len), Some(1));
        assert_eq!(model["templateGroups"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn admins_see_the_admin_button_and_all_apps() {
        let state = build_state(
            "proxy:\n  authentication: simple\n  admin-groups: scientists\n  users:\n    - name: jack\n      password: pw\n      groups: scientists\n  specs:\n    - id: app\n      container-image: img\n",
        );
        let model = prepare_model(&state, Page::Index, Some(&user()), false);
        assert_eq!(model["isAdmin"], json!(true));
    }

    #[test]
    fn groups_apps_by_template_group() {
        let state = build_state(
            "proxy:\n  authentication: none\n  template-groups:\n    - id: reporting\n      properties:\n        display-name: Reporting\n    - id: empty\n      properties:\n        display-name: Empty\n  specs:\n    - id: report\n      container-image: img\n      template-group: reporting\n    - id: other\n      container-image: img\n",
        );
        let model = prepare_model(&state, Page::Index, None, false);
        assert_eq!(model["templateGroups"].as_array().map(Vec::len), Some(1));
        assert_eq!(model["templateGroups"][0]["id"], json!("reporting"));
        assert_eq!(
            model["templateGroups"][0]["properties"]["display-name"],
            json!("Reporting")
        );
        assert_eq!(
            model["groupedApps"]["reporting"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(model["ungroupedApps"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn honours_navbar_and_context_path_settings() {
        let state = build_state(
            "proxy:\n  authentication: none\n  hide-navbar: true\n  body-classes: [ dark, compact ]\n  notification-message: '<b>hi</b><script>x</script>'\n  my-apps-mode: Inline\n  specs: []\nserver:\n  servlet:\n    context-path: /shinyproxy\n",
        );
        let model = prepare_model(&state, Page::Index, None, false);
        assert_eq!(model["showNavbar"], json!(false));
        assert_eq!(model["bodyClasses"], json!("dark compact"));
        assert_eq!(model["notificationMessage"], json!("<b>hi</b>"));
        assert_eq!(model["myAppsMode"], json!("Inline"));
        assert_eq!(model["contextPath"], json!("/shinyproxy/"));
        assert_eq!(model["logoutUrl"], json!("/shinyproxy/logout"));
        assert!(model["resourcePrefix"]
            .as_str()
            .expect("prefix")
            .starts_with("/shinyproxy/"));
    }

    #[test]
    fn hide_navbar_query_parameter_overrides_the_setting() {
        let state = build_state("proxy:\n  authentication: none\n  specs: []\n");
        let model = prepare_model(&state, Page::Index, None, true);
        assert_eq!(model["showNavbar"], json!(false));
    }
}
