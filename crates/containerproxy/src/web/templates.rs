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

use minijinja::{Environment, Value as TemplateValue};

use super::assets::Templates;
use crate::util::clean_html;

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
        environment.set_auto_escape_callback(|name| {
            if name.ends_with(".html") {
                minijinja::AutoEscape::Html
            } else {
                minijinja::AutoEscape::None
            }
        });

        // sanitises HTML that comes from the configuration, like the Java `CleanHtml` helper
        environment.add_filter("clean_html", |value: String| clean_html(&value));

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
    fn reports_missing_templates() {
        let engine = TemplateEngine::new(None).expect("engine");
        let error = engine.render("nope.html", context! {}).unwrap_err();
        assert!(matches!(error, TemplateError::Load { .. }), "{error}");
    }
}
