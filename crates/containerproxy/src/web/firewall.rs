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

//! The checks of Spring Security's `StrictHttpFirewall`, which the Java implementation runs on every
//! request, including the ones it forwards to an app.
//!
//! Without them, the path of a request reaches the handlers and the apps exactly as the client wrote it:
//! `%2F` becomes a `/` in a decoded path parameter (so `/app_i/01_hello/..%2F..%2Flogout` makes the page
//! of an app post to `/logout`), and `..` segments escape the `target-path` of an app. Spring answers
//! these requests with 400, and so does this implementation.

/// Encoded (and raw) sequences that `StrictHttpFirewall` refuses in the path of a request: semicolons
/// (path parameters), encoded slashes, backslashes, periods and percent signs, null bytes, line breaks and
/// double slashes.
const BLOCKED: &[&str] = &[
    ";", "%3b", "%2f", "%5c", "\\", "%2e", "%25", "%00", "%0a", "%0d", "//",
];

/// Why the path of a request is refused, or `None` when it is acceptable.
pub fn rejection(path: &str) -> Option<String> {
    if let Some(character) = path.chars().find(|c| !(' '..='~').contains(c)) {
        return Some(format!(
            "the URL contains a non-printable character ({:?})",
            character
        ));
    }
    let lowercase = path.to_ascii_lowercase();
    if let Some(blocked) = BLOCKED.iter().find(|blocked| lowercase.contains(**blocked)) {
        return Some(format!(
            "the URL contained a potentially malicious String \"{blocked}\""
        ));
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Some("the URL was not normalized".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for path in [
            "/",
            "/app/01_hello",
            "/app_proxy/2c7a3e51-0a1c-4a6b-9d3b-5f0c1f1d2e3a/session/abc/dataobj/plot?w=1",
            "/app_i/01_hello/my-instance.v2/",
            "/webjars/jquery/3.7.1/jquery.min.js",
            "/app_direct/rstudio/p/58fab3e4/",
            "/files/report%20final.html",
            "/app/%C3%BCnicode",
            "/file.with.dots/.hidden",
        ] {
            assert_eq!(rejection(path), None, "{path}");
        }
    }

    #[test]
    fn refuses_what_spring_refuses() {
        for path in [
            "/app_i/01_hello/..%2F..%2Flogout",
            "/app_i/01_hello/..%2f..%2flogout",
            "/app_proxy/id/../../admin",
            "/app_proxy/id/./x",
            "/app_proxy/id/..",
            "/app_proxy/%2e%2e/admin",
            "/app_proxy//admin",
            "/app;jsessionid=1",
            "/app%3Bx",
            "/app/%5C..%5Cadmin",
            "/app/\\admin",
            "/app/%252e%252e",
            "/app/%00",
            "/app/01_hello%0d%0aX-Injected:%20yes",
        ] {
            assert!(rejection(path).is_some(), "{path}");
        }
    }
}
