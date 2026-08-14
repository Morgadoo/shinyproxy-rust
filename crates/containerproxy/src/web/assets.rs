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

//! Static assets and templates, embedded into the binary.
//!
//! The browser assets (the ShinyProxy JavaScript, CSS, Handlebars templates and the vendored front-end
//! libraries) are shipped inside the executable, so that a single binary is all that is needed, just
//! like the Java jar. They are served under two prefixes, exactly as in the Java implementation:
//!
//! * `/{asset}` — for example `/js/shiny.common.js`;
//! * `/{instanceId}/{asset}` — the same file, but cacheable for a long time because the instance id
//!   changes whenever the configuration changes.

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// Static browser assets (`assets/static`).
#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../assets/static"]
pub struct StaticAssets;

/// HTML templates (`assets/templates`).
#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../assets/templates"]
#[include = "*.html"]
pub struct Templates;

/// Cache header used for assets served under the instance id prefix.
const LONG_CACHE: &str = "public, max-age=31536000, immutable";
/// Cache header used for assets served without the instance id prefix.
const SHORT_CACHE: &str = "no-cache";

/// Serves an embedded asset.
///
/// `cacheable` marks the request as coming from the instance-id-prefixed URL.
pub fn serve(path: &str, cacheable: bool) -> Response {
    let path = path.trim_start_matches('/');
    match StaticAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache = if cacheable { LONG_CACHE } else { SHORT_CACHE };
            (
                [
                    (CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap()),
                    (CACHE_CONTROL, HeaderValue::from_static(cache)),
                ],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found\n").into_response(),
    }
}

/// Whether an asset exists.
pub fn exists(path: &str) -> bool {
    StaticAssets::get(path.trim_start_matches('/')).is_some()
}

/// Reads an embedded asset.
pub fn get(path: &str) -> Option<Vec<u8>> {
    StaticAssets::get(path.trim_start_matches('/')).map(|file| file.data.to_vec())
}

/// The URL prefixes under which assets are available (used by the security configuration).
pub const ASSET_PREFIXES: &[&str] = &[
    "css",
    "js",
    "img",
    "assets",
    "webjars",
    "handlebars",
    "fonts",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_shinyproxy_frontend() {
        for path in [
            "js/shiny.common.js",
            "js/shiny.app.js",
            "js/shiny.api.js",
            "js/shiny.iframe.js",
            "css/default.css",
            "css/bootstrap.css",
            "css/login.css",
            "handlebars/precompiled.js",
            "img/loading.gif",
            "fonts/glyphicons-halflings-regular.woff2",
            "webjars/jquery/3.7.1/jquery.min.js",
            "webjars/handlebars/4.7.9/dist/handlebars.runtime.min.js",
            "webjars/datatables/1.13.5/js/jquery.dataTables.min.js",
            "webjars/fontawesome/4.7.0/css/font-awesome.min.css",
        ] {
            assert!(exists(path), "{path} must be embedded");
        }
    }

    #[test]
    fn embeds_the_templates() {
        for path in [
            "index.html",
            "login.html",
            "error.html",
            "auth-error.html",
            "auth-success.html",
            "logout-success.html",
            "app-access-denied.html",
            "fragments/navbar.html",
            "fragments/modal.html",
        ] {
            assert!(Templates::get(path).is_some(), "{path} must be embedded");
        }
    }

    #[test]
    fn serves_assets_with_content_type_and_cache_headers() {
        let response = serve("/js/shiny.common.js", false);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/javascript"
        );
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), SHORT_CACHE);

        let response = serve("css/default.css", true);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "text/css");
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), LONG_CACHE);

        assert_eq!(serve("/nope.js", false).status(), StatusCode::NOT_FOUND);
    }
}
