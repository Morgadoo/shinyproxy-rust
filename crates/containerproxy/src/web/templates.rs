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

//! HTML rendering.
//!
//! The Java implementation uses Thymeleaf; this implementation uses MiniJinja, which supports the same
//! model (a map of values) and, importantly, runtime compiled templates: `proxy.template-path` lets
//! administrators replace any template, and app definitions may contain a template for the parameter
//! form.

use std::path::PathBuf;

use minijinja::value::Value as TemplateValue;
use minijinja::{Environment, Error as MiniJinjaError, State};

use super::assets::Templates;
use crate::util::clean_html;

/// Escapes HTML like Thymeleaf does: `&`, `<`, `>`, `"` and `'`, but not `/`.
fn escape_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            other => output.push(other),
        }
    }
    output
}

/// Looks up `proxy.specs[].template-properties`, matching Java `@thymeleaf.getTemplateProperty`.
///
/// Reads the `templateProperties` map from the page model. With two arguments the missing case is
/// undefined (renders empty); with three arguments the third is used as the default.
fn get_template_property(
    state: &State,
    app_id: String,
    key: String,
    default: Option<String>,
) -> Result<TemplateValue, MiniJinjaError> {
    let missing = || {
        Ok(default
            .clone()
            .map(TemplateValue::from)
            .unwrap_or(TemplateValue::UNDEFINED))
    };

    let Some(all) = state.lookup("templateProperties") else {
        return missing();
    };
    let props = all.get_item(&TemplateValue::from(app_id))?;
    if props.is_undefined() || props.is_none() {
        return missing();
    }
    let value = props.get_item(&TemplateValue::from(key))?;
    if value.is_undefined() || value.is_none() {
        return missing();
    }
    Ok(value)
}

/// Renders the ShinyProxy pages.
pub struct TemplateEngine {
    environment: Environment<'static>,
    /// Directory with template overrides (`proxy.template-path`).
    template_path: Option<PathBuf>,
}

impl std::fmt::Debug for TemplateEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TemplateEngine")
            .field("template_path", &self.template_path)
            .finish()
    }
}

/// Error while rendering a page.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("cannot render template '{name}': {source}")]
    Render {
        name: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("cannot load template '{name}': {source}")]
    Load {
        name: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("cannot read template directory {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl TemplateEngine {
    /// Creates an engine that renders the embedded templates.
    pub fn new(template_path: Option<PathBuf>) -> Result<Self, TemplateError> {
        let mut environment = Environment::new();
        // everything ShinyProxy renders is HTML (including templates that come from the
        // configuration, which are rendered without a file name)
        environment.set_auto_escape_callback(|name| {
            if name.ends_with(".json") || name.ends_with(".txt") || name.ends_with(".js") {
                minijinja::AutoEscape::None
            } else {
                minijinja::AutoEscape::Html
            }
        });

        // sanitises HTML that comes from the configuration, like the Java `CleanHtml` helper
        environment.add_filter("clean_html", |value: String| clean_html(&value));

        // Java `@thymeleaf.getTemplateProperty(appId, key[, default])` for custom templates
        environment.add_function("get_template_property", get_template_property);
        environment.add_function("getTemplateProperty", get_template_property);

        // Thymeleaf does not escape `/` in attribute values; keep URLs readable and identical to the
        // Java output by using an escaper that only escapes the characters that matter.
        environment.set_formatter(|output, state, value| {
            if state.auto_escape() == minijinja::AutoEscape::Html && !value.is_safe() {
                if let Some(text) = value.as_str() {
                    output.write_str(&escape_html(text))?;
                    return Ok(());
                }
            }
            minijinja::escape_formatter(output, state, value)
        });

        let overrides = template_path.clone();
        environment.set_loader(move |name| {
            if let Some(directory) = &overrides {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return std::fs::read_to_string(&candidate)
                        .map(Some)
                        .map_err(|error| {
                            minijinja::Error::new(
                                minijinja::ErrorKind::InvalidOperation,
                                format!("cannot read {}: {error}", candidate.display()),
                            )
                        });
                }
            }
            match Templates::get(name) {
                Some(file) => Ok(Some(String::from_utf8_lossy(&file.data).to_string())),
                None => Ok(None),
            }
        });

        Ok(TemplateEngine {
            environment,
            template_path,
        })
    }

    /// Renders a template with the given model.
    pub fn render(&self, name: &str, model: TemplateValue) -> Result<String, TemplateError> {
        let template =
            self.environment
                .get_template(name)
                .map_err(|source| TemplateError::Load {
                    name: name.to_string(),
                    source,
                })?;
        template
            .render(model)
            .map_err(|source| TemplateError::Render {
                name: name.to_string(),
                source,
            })
    }

    /// Renders a template that is defined in the configuration (the parameter form of an app).
    pub fn render_string(
        &self,
        source: &str,
        model: TemplateValue,
    ) -> Result<String, TemplateError> {
        self.environment
            .render_str(source, model)
            .map_err(|source| TemplateError::Render {
                name: "<configuration>".to_string(),
                source,
            })
    }

    /// The configured template override directory.
    pub fn template_path(&self) -> Option<&PathBuf> {
        self.template_path.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    #[test]
    fn renders_embedded_templates() {
        let engine = TemplateEngine::new(None).expect("engine");
        let html = engine
            .render(
                "logout-success.html",
                context! { title => "ShinyProxy", contextPath => "/" },
            )
            .expect("renders");
        assert!(
            html.contains("You have been logged out successfully."),
            "{html}"
        );
        assert!(html.contains("<title>ShinyProxy</title>"), "{html}");
    }

    #[test]
    fn escapes_by_default_and_allows_marked_html() {
        let engine = TemplateEngine::new(None).expect("engine");
        let html = engine
            .render(
                "error.html",
                context! {
                    title => "ShinyProxy",
                    shortError => "<script>alert(1)</script>",
                    description => "boom",
                    mainPage => "/",
                },
            )
            .expect("renders");
        assert!(html.contains("&lt;script&gt;"), "must be escaped: {html}");
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn template_path_overrides_embedded_templates() {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            directory.path().join("logout-success.html"),
            "custom {{ title }}",
        )
        .expect("write");
        let engine = TemplateEngine::new(Some(directory.path().to_path_buf())).expect("engine");
        let html = engine
            .render("logout-success.html", context! { title => "SP" })
            .expect("renders");
        assert_eq!(html, "custom SP");

        // templates that are not overridden still come from the binary
        let html = engine
            .render("app-access-denied.html", context! { title => "SP" })
            .expect("renders");
        assert!(html.contains("You do not have access to this application."));
    }

    #[test]
    fn cleans_html_from_the_configuration() {
        let engine = TemplateEngine::new(None).expect("engine");
        let html = engine
            .render_string(
                "{{ message | clean_html | safe }}",
                context! { message => "<b>bold</b><script>alert(1)</script>" },
            )
            .expect("renders");
        assert_eq!(html, "<b>bold</b>");
    }

    #[test]
    fn looks_up_template_properties_like_java() {
        let engine = TemplateEngine::new(None).expect("engine");
        let model = serde_json::json!({
            "templateProperties": {
                "my-app": {
                    "category": "energy",
                    "icon": "fa-bolt"
                }
            }
        });
        let value = minijinja::Value::from_serialize(&model);

        let html = engine
            .render_string(
                "{{ get_template_property('my-app', 'category') }}|{{ getTemplateProperty('my-app', 'icon') }}|{{ get_template_property('my-app', 'missing', 'default-category') }}|{{ get_template_property('other', 'category') }}",
                value,
            )
            .expect("renders");
        assert_eq!(html, "energy|fa-bolt|default-category|");
    }

    #[test]
    fn template_properties_are_also_readable_from_the_model_map() {
        let engine = TemplateEngine::new(None).expect("engine");
        let html = engine
            .render_string(
                "{{ templateProperties['my-app']['type'] }}",
                context! {
                    templateProperties => minijinja::Value::from_serialize(serde_json::json!({
                        "my-app": { "type": "shiny" }
                    }))
                },
            )
            .expect("renders");
        assert_eq!(html, "shiny");
    }

    #[test]
    fn escapes_html_without_mangling_urls() {
        let engine = TemplateEngine::new(None).expect("engine");
        let html = engine
            .render_string(
                "<a href=\"{{ url }}\">{{ text }}</a>",
                context! { url => "/app/01_hello?a=1&b=2", text => "<script>x</script>" },
            )
            .expect("renders");
        assert_eq!(
            html,
            "<a href=\"/app/01_hello?a=1&amp;b=2\">&lt;script&gt;x&lt;/script&gt;</a>"
        );
    }

    #[test]
    fn reports_missing_templates() {
        let engine = TemplateEngine::new(None).expect("engine");
        let error = engine.render("nope.html", context! {}).unwrap_err();
        assert!(matches!(error, TemplateError::Load { .. }), "{error}");
    }
}

#[cfg(test)]
mod escaping_tests {
    use super::*;

    #[test]
    fn does_not_escape_slashes_in_named_templates() {
        let engine = TemplateEngine::new(None).expect("engine");
        let model = serde_json::json!({"title": "SP", "contextPath": "/sub/path/"});
        let html = engine
            .render(
                "logout-success.html",
                minijinja::Value::from_serialize(&model),
            )
            .expect("renders");
        assert!(html.contains("href=\"/sub/path/\""), "{html}");
        assert!(
            !html.contains("&#x2f;"),
            "slashes must not be escaped: {html}"
        );
    }
}
