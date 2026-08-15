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

//! The OpenAPI description of the API.
//!
//! Like the Java implementation (springdoc), the description is disabled by default and enabled with
//! `springdoc.api-docs.enabled: true`; `springdoc.swagger-ui.enabled: true` adds a human readable page.
//! The document describes the same endpoints, tags and response envelopes as the Java annotations.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::state::AppState;

/// `GET /v3/api-docs` — the OpenAPI document.
pub async fn api_docs(State(state): State<Arc<AppState>>) -> Response {
    if !state.settings.springdoc.api_docs.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(document(&state)).into_response()
}

/// `GET /swagger-ui/index.html` — a readable overview of the API.
pub async fn swagger_ui(State(state): State<Arc<AppState>>) -> Response {
    if !state.settings.springdoc.swagger_ui.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let document = document(&state);
    let mut rows = String::new();
    if let Some(paths) = document["paths"].as_object() {
        for (path, methods) in paths {
            if let Some(methods) = methods.as_object() {
                for (method, operation) in methods {
                    rows.push_str(&format!(
                        "<tr><td><code>{}</code></td><td><code>{path}</code></td><td>{}</td><td>{}</td></tr>",
                        method.to_uppercase(),
                        operation["tags"][0].as_str().unwrap_or(""),
                        operation["summary"].as_str().unwrap_or("")
                    ));
                }
            }
        }
    }

    let context = state.context_path_with_slash();
    let html = format!(
        "<!DOCTYPE html>\n<html><head lang=\"en\"><title>ShinyProxy API</title>\
         <link media=\"screen\" rel=\"stylesheet\" href=\"{context}css/bootstrap.css\"/></head>\
         <body><div class=\"container\"><h2>ShinyProxy API</h2>\
         <p>The machine readable description is available at <a href=\"{context}v3/api-docs\">\
         {context}v3/api-docs</a>.</p>\
         <table class=\"table table-condensed\"><thead><tr><th>Method</th><th>Path</th><th>Tag</th>\
         <th>Description</th></tr></thead><tbody>{rows}</tbody></table></div></body></html>\n"
    );
    Html(html).into_response()
}

/// Builds the OpenAPI document.
pub fn document(state: &AppState) -> serde_json::Value {
    let context = state.context_path();
    let api_response = json!({
        "type": "object",
        "properties": {
            "status": {"type": "string", "enum": ["success", "fail", "error"]},
            "data": {}
        }
    });

    json!({
        "openapi": "3.0.1",
        "info": {
            "title": state.settings.application_name(),
            "description": "ShinyProxy API (Rust implementation)",
            "version": crate::VERSION,
        },
        "servers": [{"url": if context.is_empty() { "/".to_string() } else { context.clone() }}],
        "tags": [
            {"name": "ShinyProxy", "description": "Endpoints of ShinyProxy"},
            {"name": "ContainerProxy", "description": "Endpoints of the ContainerProxy engine"}
        ],
        "components": {"schemas": {"ApiResponse": api_response}},
        "paths": {
            "/api/proxyspec": {"get": {
                "tags": ["ContainerProxy"],
                "summary": "Get all app definitions the user has access to.",
                "responses": {"200": response("The app definitions.")}
            }},
            "/api/proxyspec/{proxySpecId}": {"get": {
                "tags": ["ContainerProxy"],
                "summary": "Get one app definition.",
                "parameters": [path_parameter("proxySpecId")],
                "responses": {"200": response("The app definition."), "403": response("No access.")}
            }},
            "/api/proxy": {"get": {
                "tags": ["ContainerProxy"],
                "summary": "Get the apps of the current user.",
                "responses": {"200": response("The apps of the user.")}
            }},
            "/api/proxy/{proxyId}": {"get": {
                "tags": ["ContainerProxy"],
                "summary": "Get one app of the current user.",
                "parameters": [path_parameter("proxyId")],
                "responses": {"200": response("The app."), "403": response("No access.")}
            }},
            "/api/proxy/{proxyId}/status": {
                "get": {
                    "tags": ["ContainerProxy"],
                    "summary": "Get the status of an app and optionally wait for it to change.",
                    "parameters": [
                        path_parameter("proxyId"),
                        query_parameter("watch", "boolean", "Whether to wait for the status to change."),
                        query_parameter("timeout", "integer", "Seconds to wait, between 10 and 60.")
                    ],
                    "responses": {"200": response("The status of the app.")}
                },
                "put": {
                    "tags": ["ContainerProxy"],
                    "summary": "Change the status of an app (Stopping, Pausing, Resuming).",
                    "parameters": [path_parameter("proxyId")],
                    "responses": {
                        "200": response("Status changed."),
                        "400": response("The app is not in the right status."),
                        "403": response("No access.")
                    }
                }
            },
            "/api/proxy/{proxyId}/userId": {"put": {
                "tags": ["ShinyProxy"],
                "summary": "Transfer an app to another user.",
                "parameters": [path_parameter("proxyId")],
                "responses": {"200": response("App transferred."), "403": response("No access.")}
            }},
            "/api/proxy/{proxyId}/details": {"get": {
                "tags": ["ShinyProxy"],
                "summary": "Get the custom app details of an app.",
                "parameters": [path_parameter("proxyId")],
                "responses": {"200": response("The app details."), "410": response("The app is gone.")}
            }},
            "/api/route/{targetId}/**": {"get": {
                "tags": ["ShinyProxy"],
                "summary": "Proxy a request to an app.",
                "parameters": [path_parameter("targetId")],
                "responses": {"200": response("The response of the app.")}
            }},
            "/app_i/{specId}/{instance}": {"post": {
                "tags": ["ShinyProxy"],
                "summary": "Start an app.",
                "parameters": [path_parameter("specId"), path_parameter("instance")],
                "responses": {
                    "200": response("The app has been created."),
                    "400": response("Invalid request, app not started."),
                    "403": response("No access to this app definition.")
                }
            }},
            "/heartbeat/{proxyId}": {
                "post": {
                    "tags": ["ShinyProxy"],
                    "summary": "Force a heartbeat for an app.",
                    "parameters": [path_parameter("proxyId")],
                    "responses": {"200": response("Heartbeat sent."), "410": response("The app is gone.")}
                },
                "get": {
                    "tags": ["ShinyProxy"],
                    "summary": "Get heartbeat information for an app.",
                    "parameters": [path_parameter("proxyId")],
                    "responses": {"200": response("Heartbeat information.")}
                }
            },
            "/issue": {"post": {
                "tags": ["ShinyProxy"],
                "summary": "Report an issue.",
                "responses": {"200": response("Issue reported."), "400": response("Invalid request.")}
            }},
            "/admin/data": {"get": {
                "tags": ["ShinyProxy"],
                "summary": "Get the active apps of all users (administrators only).",
                "responses": {"200": response("The active apps.")}
            }},
            "/admin/delegate-proxy": {"delete": {
                "tags": ["ShinyProxy"],
                "summary": "Remove the pre-initialized containers (administrators only).",
                "responses": {"200": response("Removal requested.")}
            }}
        }
    })
}

fn response(description: &str) -> serde_json::Value {
    json!({
        "description": description,
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ApiResponse"}}}
    })
}

fn path_parameter(name: &str) -> serde_json::Value {
    json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}})
}

fn query_parameter(name: &str, kind: &str, description: &str) -> serde_json::Value {
    json!({
        "name": name,
        "in": "query",
        "required": false,
        "description": description,
        "schema": {"type": kind}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use containerproxy::config::LoadOptions;

    async fn build_state(yaml: &str) -> AppState {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("application.yml");
        std::fs::write(&path, yaml).expect("write");
        let options = LoadOptions {
            args: vec![format!("--spring.config.location={}", path.display())],
            ..LoadOptions::default()
        };
        let (raw, mut settings) = crate::load_config(options).expect("config");
        settings.proxy.container_backend = Some("local".to_string());
        AppState::new(raw, settings).await.expect("state")
    }

    #[tokio::test]
    async fn describes_every_documented_endpoint() {
        let state = build_state("proxy:\n  authentication: none\n  specs: []\n").await;
        let document = document(&state);
        assert_eq!(document["openapi"], "3.0.1");
        assert_eq!(document["info"]["version"], crate::VERSION);

        let paths = document["paths"].as_object().expect("paths");
        for path in [
            "/api/proxyspec",
            "/api/proxyspec/{proxySpecId}",
            "/api/proxy",
            "/api/proxy/{proxyId}",
            "/api/proxy/{proxyId}/status",
            "/api/proxy/{proxyId}/userId",
            "/api/proxy/{proxyId}/details",
            "/app_i/{specId}/{instance}",
            "/heartbeat/{proxyId}",
            "/issue",
            "/admin/data",
            "/admin/delegate-proxy",
        ] {
            assert!(paths.contains_key(path), "{path} is missing");
        }
        assert!(paths["/api/proxy/{proxyId}/status"]["get"]["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .any(|parameter| parameter["name"] == "watch"));
    }

    #[tokio::test]
    async fn uses_the_context_path_as_server() {
        let state = build_state(
            "proxy:\n  authentication: none\n  specs: []\nserver:\n  servlet:\n    context-path: /shinyproxy\n",
        ).await;
        let document = document(&state);
        assert_eq!(document["servers"][0]["url"], "/shinyproxy");
    }
}
