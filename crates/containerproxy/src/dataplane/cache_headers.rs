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

//! Cache headers of proxied responses (`ProxyCacheHeadersService`).

use axum::http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE, EXPIRES, PRAGMA};
use axum::http::{HeaderMap, Method};

use crate::model::spec::CacheHeadersMode;

/// Media types that count as a cacheable asset.
const ASSET_TYPES: &[&str] = &[
    "application/javascript",
    "text/javascript",
    "text/css",
    "application/font-woff",
    "application/font-woff2",
    "application/font-sfnt",
    "application/font-tdpfr",
];

/// Applies the cache headers of the given mode to a proxied response.
pub fn apply(mode: CacheHeadersMode, method: &Method, headers: &mut HeaderMap) {
    match mode {
        CacheHeadersMode::EnforceNoCache => write_no_cache(headers),
        // trust whatever the app sends
        CacheHeadersMode::Passthrough => {}
        CacheHeadersMode::EnforceCacheAssets => {
            if is_asset(method, headers) {
                write_cache(headers);
            } else {
                write_no_cache(headers);
            }
        }
    }
}

/// Whether the response is a static asset of the app.
fn is_asset(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET {
        return false;
    }
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    // ignore parameters such as `; charset=utf-8`
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type.starts_with("font/") || ASSET_TYPES.contains(&media_type.as_str())
}

fn write_no_cache(headers: &mut HeaderMap) {
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, max-age=0, must-revalidate"),
    );
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(EXPIRES, HeaderValue::from_static("0"));
}

fn write_cache(headers: &mut HeaderMap) {
    // first remove the headers that would prevent caching
    headers.remove(PRAGMA);
    headers.remove(EXPIRES);
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=86400"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_headers(content_type: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(content_type) = content_type {
            headers.insert(CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
        }
        headers
    }

    #[test]
    fn enforce_no_cache_is_the_default_behaviour() {
        let mut headers = build_headers(Some("text/html"));
        apply(CacheHeadersMode::EnforceNoCache, &Method::GET, &mut headers);
        assert_eq!(
            headers.get(CACHE_CONTROL).unwrap(),
            "no-cache, no-store, max-age=0, must-revalidate"
        );
        assert_eq!(headers.get(PRAGMA).unwrap(), "no-cache");
        assert_eq!(headers.get(EXPIRES).unwrap(), "0");
    }

    #[test]
    fn passthrough_keeps_the_headers_of_the_app() {
        let mut headers = build_headers(Some("text/css"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=42"));
        apply(CacheHeadersMode::Passthrough, &Method::GET, &mut headers);
        assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "max-age=42");
    }

    #[test]
    fn assets_are_cached_in_enforce_cache_assets_mode() {
        for content_type in [
            "text/css",
            "application/javascript",
            "text/javascript; charset=utf-8",
            "font/woff2",
            "application/font-woff",
        ] {
            let mut headers = build_headers(Some(content_type));
            headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
            apply(
                CacheHeadersMode::EnforceCacheAssets,
                &Method::GET,
                &mut headers,
            );
            assert_eq!(
                headers.get(CACHE_CONTROL).unwrap(),
                "max-age=86400",
                "{content_type}"
            );
            assert!(!headers.contains_key(PRAGMA), "{content_type}");
        }
    }

    #[test]
    fn non_assets_are_not_cached_in_enforce_cache_assets_mode() {
        let mut headers = build_headers(Some("text/html"));
        apply(
            CacheHeadersMode::EnforceCacheAssets,
            &Method::GET,
            &mut headers,
        );
        assert_eq!(
            headers.get(CACHE_CONTROL).unwrap(),
            "no-cache, no-store, max-age=0, must-revalidate"
        );

        // only GET requests can be assets
        let mut headers = build_headers(Some("text/css"));
        apply(
            CacheHeadersMode::EnforceCacheAssets,
            &Method::POST,
            &mut headers,
        );
        assert_eq!(
            headers.get(CACHE_CONTROL).unwrap(),
            "no-cache, no-store, max-age=0, must-revalidate"
        );

        // and responses without a content type are not assets
        let mut headers = build_headers(None);
        apply(
            CacheHeadersMode::EnforceCacheAssets,
            &Method::GET,
            &mut headers,
        );
        assert_eq!(
            headers.get(CACHE_CONTROL).unwrap(),
            "no-cache, no-store, max-age=0, must-revalidate"
        );
    }
}
