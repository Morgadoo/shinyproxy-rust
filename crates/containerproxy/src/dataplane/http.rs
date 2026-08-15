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

//! Forwarding of HTTP requests to an app.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{HeaderMap, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use once_cell::sync::Lazy;

/// Headers that must not be forwarded (RFC 9110 hop-by-hop headers).
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "proxy-authenticate",
    "proxy-authorization",
    // `Upgrade` only means something for the hop it was sent on; it is kept for the handshake of a
    // WebSocket (see `filter_headers`) and dropped everywhere else
    "upgrade",
];

/// The pooled HTTP client used for proxying, shared by all proxies.
static CLIENT: Lazy<Client<HttpConnector, Body>> = Lazy::new(|| {
    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);
    connector.set_keepalive(Some(std::time::Duration::from_secs(60)));
    Client::builder(TokioExecutor::new())
        // apps are long-polling and streaming, so connections must not be closed eagerly
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build(connector)
});

/// Extra behaviour for a forwarded request.
#[derive(Debug, Clone, Default)]
pub struct ForwardOptions {
    /// Headers to add to the request (the `http-headers` of the app plus `X-SP-*`).
    pub extra_headers: std::sync::Arc<BTreeMap<String, String>>,
    /// Ask the app not to compress the response, needed when the response is rewritten.
    pub force_identity_encoding: bool,
}

/// Why a request could not be forwarded.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    /// The app could not be reached.
    #[error("cannot reach the app: {0}")]
    Unreachable(String),
    /// The request could not be built.
    #[error("invalid proxy request: {0}")]
    InvalidRequest(String),
}

/// Forwards a request to `url` and returns the response of the app.
///
/// Both the request and the response body are streamed. Connection failures are retried twice, which is
/// what Undertow's `ProxyHandler` does (`setMaxConnectionRetries(2)`).
pub async fn forward(
    request: Request<Body>,
    url: &str,
    options: &ForwardOptions,
) -> Result<Response<Body>, ForwardError> {
    // the answer is marked as coming from an app, so the cache headers of the server are not added to it
    let (parts, body) = request.into_parts();
    let uri: Uri = url
        .parse()
        .map_err(|error| ForwardError::InvalidRequest(format!("{url}: {error}")))?;

    let mut headers = filter_headers(&parts.headers);

    // Undertow adds X-Forwarded-* headers; the host header must include a non-standard port, which is
    // what apps compare the Origin header against (see #31010 in the Java implementation).
    if let Some(host) = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        insert_header(&mut headers, "x-forwarded-host", host);
    }
    if let Some(scheme) = parts.uri.scheme_str() {
        insert_header(&mut headers, "x-forwarded-proto", scheme);
    }
    for (name, value) in options.extra_headers.iter() {
        insert_header(&mut headers, name, value);
    }
    if options.force_identity_encoding {
        insert_header(&mut headers, "accept-encoding", "identity");
    }

    // Requests without a body can be retried on connection failures, like Undertow's ProxyHandler
    // (max 2 retries). Requests with a body cannot: the body is streamed and consumed once.
    let retries =
        if parts.method == axum::http::Method::GET || parts.method == axum::http::Method::HEAD {
            2
        } else {
            0
        };

    let mut body = Some(body);
    let mut attempt = 0;
    loop {
        let mut outgoing = Request::builder()
            .method(parts.method.clone())
            .uri(uri.clone());
        {
            let outgoing_headers = outgoing.headers_mut().expect("request builder has headers");
            *outgoing_headers = headers.clone();
        }
        let outgoing_body = match body.take() {
            Some(body) => body,
            None => Body::empty(),
        };
        let outgoing = outgoing
            .body(outgoing_body)
            .map_err(|error| ForwardError::InvalidRequest(error.to_string()))?;

        match CLIENT.request(outgoing).await {
            Ok(response) => {
                let mut response = response.map(Body::new);
                // the answer comes from the app, so the server does not add its cache headers to it
                response.extensions_mut().insert(super::AppAnswer);
                return Ok(response);
            }
            Err(error) => {
                if attempt < retries {
                    attempt += 1;
                    tracing::debug!("retrying request to {url} after error: {error}");
                    continue;
                }
                return Err(ForwardError::Unreachable(error.to_string()));
            }
        }
    }
}

/// Copies the headers of a request/response, dropping the hop-by-hop ones.
///
/// `Connection: upgrade` and `Upgrade` are kept for a handshake, because otherwise the WebSocket handshake
/// of an app would never reach it; on a normal request they are dropped, so a client cannot smuggle them
/// into the app.
pub fn filter_headers(headers: &HeaderMap) -> HeaderMap {
    let upgrading = headers
        .get(axum::http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("upgrade"));

    let mut result = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        if upgrading
            && (name == axum::http::header::CONNECTION || name == axum::http::header::UPGRADE)
        {
            result.append(name.clone(), value.clone());
            continue;
        }
        if HOP_BY_HOP.contains(&name.as_str()) {
            continue;
        }
        result.append(name.clone(), value.clone());
    }
    result
}

/// Inserts a header, ignoring invalid names/values.
pub fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) {
    match (
        HeaderName::try_from(name.to_ascii_lowercase().as_str()),
        HeaderValue::from_str(value),
    ) {
        (Ok(name), Ok(value)) => {
            headers.insert(name, value);
        }
        _ => tracing::warn!("ignoring invalid header {name}"),
    }
}

/// The JSON body ShinyProxy returns when an app crashed (`app_crashed`).
pub const APP_CRASHED_BODY: &str = "{\"status\":\"fail\",\"data\":\"app_crashed\"}";
/// The JSON body ShinyProxy returns when an app is gone (`app_stopped_or_non_existent`).
pub const APP_STOPPED_BODY: &str = "{\"status\":\"fail\",\"data\":\"app_stopped_or_non_existent\"}";

/// Builds the response for an app that crashed or is gone.
pub fn app_unavailable_response(crashed: bool) -> Response<Body> {
    let body = if crashed {
        APP_CRASHED_BODY
    } else {
        APP_STOPPED_BODY
    };
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("static response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_connection_upgrade_for_websockets() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("Upgrade"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        let filtered = filter_headers(&headers);
        assert_eq!(filtered.get("connection").unwrap(), "Upgrade");
        assert_eq!(filtered.get("upgrade").unwrap(), "websocket");
    }

    #[test]
    fn drops_the_upgrade_header_of_a_normal_request() {
        let mut headers = HeaderMap::new();
        headers.insert("upgrade", HeaderValue::from_static("h2c"));
        headers.insert("x-ok", HeaderValue::from_static("value"));
        let filtered = filter_headers(&headers);
        assert!(
            filtered.get("upgrade").is_none(),
            "a client must not be able to send Upgrade to the app"
        );
        assert_eq!(filtered.get("x-ok").unwrap(), "value");

        // during a handshake it is needed
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("Upgrade"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        let filtered = filter_headers(&headers);
        assert_eq!(filtered.get("upgrade").unwrap(), "websocket");
        assert_eq!(filtered.get("connection").unwrap(), "Upgrade");
    }

    #[test]
    fn drops_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("cookie", HeaderValue::from_static("a=b"));
        headers.insert("x-custom", HeaderValue::from_static("value"));

        let filtered = filter_headers(&headers);
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("keep-alive"));
        assert!(!filtered.contains_key("transfer-encoding"));
        assert_eq!(filtered.get("cookie").unwrap(), "a=b");
        assert_eq!(filtered.get("x-custom").unwrap(), "value");
    }

    #[test]
    fn inserts_headers_and_ignores_invalid_ones() {
        let mut headers = HeaderMap::new();
        insert_header(&mut headers, "X-SP-UserId", "jack");
        insert_header(&mut headers, "invalid header", "value");
        insert_header(&mut headers, "x-bad-value", "line\nbreak");
        assert_eq!(headers.get("x-sp-userid").unwrap(), "jack");
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn builds_the_java_error_bodies() {
        let response = app_unavailable_response(true);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json"
        );

        let response = app_unavailable_response(false);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            APP_STOPPED_BODY,
            "{\"status\":\"fail\",\"data\":\"app_stopped_or_non_existent\"}"
        );
    }
}
