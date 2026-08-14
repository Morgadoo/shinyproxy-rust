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

//! Keeps `docs/CONFIGURATION.md` in sync with the configuration schema, and keeps the schema in sync
//! with the property inventory extracted from the Java implementation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

#[test]
fn configuration_documentation_is_up_to_date() {
    let path = repository_root().join("docs/CONFIGURATION.md");
    let expected = shinyproxy::config_schema::markdown();
    let actual = std::fs::read_to_string(&path).expect("docs/CONFIGURATION.md exists");
    assert_eq!(
        actual, expected,
        "docs/CONFIGURATION.md is out of date; regenerate it with \
         `cargo run -q -p shinyproxy --example config-docs > docs/CONFIGURATION.md`"
    );
}

/// Every property the Java implementation reads must be in the schema (or explicitly listed below as
/// an internal property that never appears in a user's configuration file).
#[test]
fn schema_covers_the_java_property_inventory() {
    let path = repository_root().join("docs/generated/java-properties.txt");
    let inventory = std::fs::read_to_string(&path).expect("java property inventory exists");
    let schema = shinyproxy::config_schema::schema();

    // Properties that exist in the Java sources but are not user facing configuration:
    // metric tag names, Spring internals that ShinyProxy sets itself, and logger names of Java
    // libraries that do not exist in this implementation.
    let ignored: BTreeSet<&str> = [
        "proxy.created.timestamp",
        "proxy.id",
        "proxy.instance",
        "proxy.namespace",
        "proxy.docker.",
        "proxy.ecs.",
        "proxy.kubernetes.",
        "proxy.webservice.",
        "proxy.ldap.%s",
        "proxy.ldap[]",
        "proxy.admin-groups[]",
        "proxy.admin-users[]",
        "proxy.api-security.custom-headers",
        "proxy.users[].groups",
        "proxy.users[].name",
        "proxy.users[].password",
        "proxy.container-wait-time",
        "spring.servlet.multipart.enabled",
        "spring.session.redis.flush-mode",
        "spring.session.redis.repository-type",
        "server.undertow.max-http-post-size",
        "logging.include-application-name",
        "management.defaults.metrics.export.enabled",
        "management.endpoint.health.group.readiness.include",
        "management.endpoint.health.probes.enabled",
        "management.endpoints.web.exposure.include",
        "management.health.ldap.enabled",
        "management.health.redis.enabled",
    ]
    .into_iter()
    .collect();

    let mut missing = Vec::new();
    for line in inventory.lines() {
        let property = line.trim();
        if property.is_empty() || property.starts_with('#') {
            continue;
        }
        if ignored.contains(property) {
            continue;
        }
        // logger levels are a free form map
        if property.starts_with("logging.level.") {
            continue;
        }
        if !schema.is_known(property) {
            missing.push(property.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "these properties are used by the Java implementation but missing from the schema: {missing:#?}"
    );
}
