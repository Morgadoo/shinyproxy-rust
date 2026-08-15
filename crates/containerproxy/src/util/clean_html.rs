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

//! Sanitising of HTML that comes from the configuration.
//!
//! App descriptions, notification messages and parameter descriptions may contain limited HTML. The
//! Java implementation runs them through jsoup's `Safelist.basic()`; this module reproduces that
//! allowlist with `ammonia`.
//!
//! `Safelist.basic()` allows: a, b, blockquote, br, cite, code, dd, dl, dt, em, i, li, ol, p, pre, q,
//! small, span, strike, strong, sub, sup, u, ul — with `href` on `a`, `cite` on blockquote/q and
//! `title` on abbr/acronym (which are not in the tag list). jsoup also enforces `rel=nofollow` on links
//! and only allows the http(s), ftp and mailto protocols.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;

static ALLOWED_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "cite",
    "code",
    "dd",
    "dl",
    "dt",
    "em",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "q",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "u",
    "ul",
];

static CLEANER: Lazy<ammonia::Builder<'static>> = Lazy::new(|| {
    let mut builder = ammonia::Builder::default();
    builder.tags(HashSet::from_iter(ALLOWED_TAGS.iter().copied()));
    let mut attributes: HashMap<&str, HashSet<&str>> = HashMap::new();
    attributes.insert("a", HashSet::from_iter(["href"]));
    attributes.insert("blockquote", HashSet::from_iter(["cite"]));
    attributes.insert("q", HashSet::from_iter(["cite"]));
    builder.tag_attributes(attributes);
    builder.url_schemes(HashSet::from_iter(["http", "https", "ftp", "mailto"]));
    builder.link_rel(Some("nofollow"));
    builder
});

/// Removes everything that is not in the allowlist, returning safe HTML.
pub fn clean_html(html: &str) -> String {
    CLEANER.clean(html).to_string()
}

/// Sanitises an optional value, keeping `None` as `None`.
pub fn clean_html_opt(html: Option<&str>) -> Option<String> {
    html.map(clean_html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_basic_formatting() {
        assert_eq!(
            clean_html("<b>bold</b> and <i>italic</i>"),
            "<b>bold</b> and <i>italic</i>"
        );
        assert_eq!(clean_html("plain text"), "plain text");
        assert_eq!(
            clean_html("<p>paragraph</p><ul><li>item</li></ul>"),
            "<p>paragraph</p><ul><li>item</li></ul>"
        );
    }

    #[test]
    fn removes_scripts_and_event_handlers() {
        assert_eq!(clean_html("<script>alert(1)</script>"), "");
        assert_eq!(clean_html("<b onclick=\"alert(1)\">x</b>"), "<b>x</b>");
        assert_eq!(clean_html("<img src=x onerror=alert(1)>"), "");
        assert_eq!(
            clean_html("<iframe src='https://example.com'></iframe>"),
            ""
        );
    }

    #[test]
    fn keeps_links_but_marks_them_nofollow() {
        let cleaned = clean_html("<a href=\"https://example.com\">link</a>");
        assert!(
            cleaned.contains("href=\"https://example.com\""),
            "{cleaned}"
        );
        assert!(cleaned.contains("rel=\"nofollow\""), "{cleaned}");
        // javascript: urls are dropped
        let cleaned = clean_html("<a href=\"javascript:alert(1)\">link</a>");
        assert!(!cleaned.contains("javascript"), "{cleaned}");
    }

    #[test]
    fn handles_optional_values() {
        assert_eq!(clean_html_opt(None), None);
        assert_eq!(
            clean_html_opt(Some("<b>x</b><script>y</script>")),
            Some("<b>x</b>".to_string())
        );
    }
}
