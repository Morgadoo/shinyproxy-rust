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

//! Golden test for the canonical YAML rendering that the ShinyProxy instance id is derived from.
//!
//! `fixtures/canonical-edge.expected.yml` was produced by the Java implementation
//! (`YAMLMapper` with sorted keys, see `docs/COMPATIBILITY.md` for the reference program), so this test
//! verifies byte compatibility for the tricky cases: string escaping, unicode, empty collections,
//! nested sequences, nulls, integers and floats.

use containerproxy::util::{sha1_hex, to_canonical_yaml};

const INPUT: &str = include_str!("fixtures/canonical-edge.yml");
const EXPECTED: &str = include_str!("fixtures/canonical-edge.expected.yml");

#[test]
fn renders_the_same_canonical_yaml_as_jackson() {
    let parsed: serde_json::Value = serde_yaml_ng::from_str(INPUT).expect("fixture parses");
    let rendered = to_canonical_yaml(&parsed);
    assert_eq!(
        rendered, EXPECTED,
        "\nrendered:\n{rendered}\nexpected:\n{EXPECTED}"
    );
}

#[test]
fn hashes_to_the_value_computed_by_java() {
    let parsed: serde_json::Value = serde_yaml_ng::from_str(INPUT).expect("fixture parses");
    assert_eq!(
        sha1_hex(&to_canonical_yaml(&parsed)),
        "68594e0ccaeb8b4db199920f58fb595f68641fc4"
    );
}
