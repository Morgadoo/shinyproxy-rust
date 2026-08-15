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

//! SHA-1 helpers, equivalent to `eu.openanalytics.containerproxy.util.Sha1`.

use serde_json::Value;
use sha1::{Digest, Sha1};

use super::canonical_yaml::to_canonical_yaml;

/// SHA-1 of the UTF-8 bytes of `value`, formatted as 40 lowercase hexadecimal characters.
pub fn sha1_hex(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// SHA-1 of a value rendered as canonical YAML (`Sha1#hash(Object)`).
pub fn sha1_of_value(value: &Value) -> String {
    sha1_hex(&to_canonical_yaml(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_strings_like_java() {
        // echo -n "abc" | sha1sum
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        // echo -n "" | sha1sum
        assert_eq!(sha1_hex(""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }
}
