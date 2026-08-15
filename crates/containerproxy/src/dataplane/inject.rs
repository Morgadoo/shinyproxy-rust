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

//! Injection of the ShinyProxy iframe script into HTML responses of an app.
//!
//! ShinyProxy serves apps inside an iframe and needs a small script inside the app to communicate with
//! the surrounding page (reload handling, heartbeats, navigation tracking). The Java implementation
//! rewrites the response stream (`ShinyProxyIframeScriptInjector`); this module does the same and works
//! on chunk boundaries, so the tag may be split across chunks.

use bytes::Bytes;

/// Rewrites an HTML stream, inserting a script tag right after the opening `<head>` tag.
///
/// When there is no `<head>` element, the script is inserted before `</body>`, and when that is missing
/// too, at the end of the document — so the script is always present.
#[derive(Debug)]
pub struct ScriptInjector {
    script: String,
    /// Buffered tail of the previous chunk, in case a tag is split across chunks.
    pending: String,
    injected: bool,
}

/// Longest marker we search for, used to decide how much of a chunk has to be buffered.
const MAX_MARKER_LENGTH: usize = "</body>".len();

impl ScriptInjector {
    /// Creates an injector for the given script URL.
    pub fn new(script_url: &str) -> Self {
        ScriptInjector {
            script: format!("<script src=\"{script_url}\"></script>"),
            pending: String::new(),
            injected: false,
        }
    }

    /// Whether the script has been inserted already.
    pub fn injected(&self) -> bool {
        self.injected
    }

    /// Processes a chunk of the response, returning the data that can be sent to the client.
    pub fn push(&mut self, chunk: &[u8]) -> Bytes {
        if self.injected {
            return Bytes::copy_from_slice(chunk);
        }

        // work on text; invalid utf-8 is passed through unchanged
        let Ok(text) = std::str::from_utf8(chunk) else {
            self.injected = true;
            let mut output = std::mem::take(&mut self.pending).into_bytes();
            output.extend_from_slice(chunk);
            return Bytes::from(output);
        };

        let mut buffer = std::mem::take(&mut self.pending);
        buffer.push_str(text);

        if let Some(output) = self.try_inject(&buffer) {
            self.injected = true;
            return Bytes::from(output.into_bytes());
        }

        // keep a small tail, a marker may be split across chunks
        let keep_from = buffer.len().saturating_sub(MAX_MARKER_LENGTH);
        let keep_from = floor_char_boundary(&buffer, keep_from);
        self.pending = buffer[keep_from..].to_string();
        Bytes::from(buffer[..keep_from].to_string().into_bytes())
    }

    /// Flushes the buffered tail at the end of the response.
    pub fn finish(&mut self) -> Bytes {
        let buffer = std::mem::take(&mut self.pending);
        if self.injected {
            return Bytes::from(buffer.into_bytes());
        }
        self.injected = true;
        match self.try_inject(&buffer) {
            Some(output) => Bytes::from(output.into_bytes()),
            // no marker at all: append the script so that the app still works
            None => Bytes::from(format!("{buffer}{}", self.script).into_bytes()),
        }
    }

    /// Inserts the script if one of the markers is present.
    fn try_inject(&self, buffer: &str) -> Option<String> {
        let lowercase = buffer.to_ascii_lowercase();
        if let Some(position) = lowercase.find("<head>") {
            let insert_at = position + "<head>".len();
            return Some(format!(
                "{}{}{}",
                &buffer[..insert_at],
                self.script,
                &buffer[insert_at..]
            ));
        }
        if let Some(position) = lowercase.find("<head ") {
            // <head lang="en"> and friends: insert after the closing bracket
            if let Some(end) = lowercase[position..].find('>') {
                let insert_at = position + end + 1;
                return Some(format!(
                    "{}{}{}",
                    &buffer[..insert_at],
                    self.script,
                    &buffer[insert_at..]
                ));
            }
        }
        if let Some(position) = lowercase.find("</body>") {
            return Some(format!(
                "{}{}{}",
                &buffer[..position],
                self.script,
                &buffer[position..]
            ));
        }
        None
    }
}

/// Largest index `<= index` that is a char boundary.
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inject_all(chunks: &[&str]) -> String {
        let mut injector = ScriptInjector::new("/instance/js/shiny.iframe.js");
        let mut output = Vec::new();
        for chunk in chunks {
            output.extend_from_slice(&injector.push(chunk.as_bytes()));
        }
        output.extend_from_slice(&injector.finish());
        String::from_utf8(output).expect("utf8")
    }

    #[test]
    fn injects_after_the_head_tag() {
        let html = inject_all(&["<html><head><title>App</title></head><body>x</body></html>"]);
        assert_eq!(
            html,
            "<html><head><script src=\"/instance/js/shiny.iframe.js\"></script><title>App</title></head><body>x</body></html>"
        );
    }

    #[test]
    fn injects_after_a_head_tag_with_attributes() {
        let html = inject_all(&["<html><head lang=\"en\"><title>App</title></head></html>"]);
        assert!(
            html.contains(
                "<head lang=\"en\"><script src=\"/instance/js/shiny.iframe.js\"></script>"
            ),
            "{html}"
        );
    }

    #[test]
    fn injects_before_the_body_end_when_there_is_no_head() {
        let html = inject_all(&["<html><body>content</body></html>"]);
        assert_eq!(
            html,
            "<html><body>content<script src=\"/instance/js/shiny.iframe.js\"></script></body></html>"
        );
    }

    #[test]
    fn appends_the_script_when_no_marker_exists() {
        let html = inject_all(&["just text"]);
        assert_eq!(
            html,
            "just text<script src=\"/instance/js/shiny.iframe.js\"></script>"
        );
    }

    #[test]
    fn handles_markers_split_across_chunks() {
        let html = inject_all(&["<html><he", "ad><title>", "App</title></head></html>"]);
        assert!(
            html.contains("<head><script src=\"/instance/js/shiny.iframe.js\"></script><title>"),
            "{html}"
        );
        // the document itself is not modified otherwise
        assert!(html.contains("<title>App</title></head></html>"), "{html}");
    }

    #[test]
    fn injects_only_once() {
        let mut injector = ScriptInjector::new("/js/shiny.iframe.js");
        let first = injector.push(b"<html><head></head>");
        assert!(injector.injected());
        let second = injector.push(b"<head></head>");
        let rest = injector.finish();
        let html = format!(
            "{}{}{}",
            String::from_utf8_lossy(&first),
            String::from_utf8_lossy(&second),
            String::from_utf8_lossy(&rest)
        );
        assert_eq!(html.matches("shiny.iframe.js").count(), 1, "{html}");
    }

    #[test]
    fn passes_binary_data_through() {
        let mut injector = ScriptInjector::new("/js/shiny.iframe.js");
        let output = injector.push(&[0xff, 0xfe, 0x00]);
        assert_eq!(output.as_ref(), &[0xff, 0xfe, 0x00]);
        assert!(injector.injected(), "binary responses are not rewritten");
    }

    #[test]
    fn handles_multi_byte_characters_on_chunk_boundaries() {
        let html = inject_all(&["<html><body>héllo → ", "wörld</body></html>"]);
        assert!(html.contains("héllo → wörld"), "{html}");
        assert!(html.contains("shiny.iframe.js"), "{html}");
    }
}
