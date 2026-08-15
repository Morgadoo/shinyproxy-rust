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

//! Micro-benchmarks of the engine paths that run on every request or every app start.
//!
//! These are for tracking regressions in this implementation (`cargo bench -p containerproxy`); the
//! comparison with the Java implementation happens end to end in `scripts/benchmark.sh`, because the two
//! stacks cannot be compared function by function.

use containerproxy::config::{LoadOptions, Settings};
use containerproxy::dataplane::http::filter_headers;
use containerproxy::model::proxy::{Container, Proxy, ProxyStatus};
use containerproxy::model::runtime_value::{
    RuntimeValue, CREATED_TIMESTAMP, DISPLAY_NAME, HEARTBEAT_TIMEOUT, INSTANCE_ID, MAX_LIFETIME,
    PORT_MAPPINGS, PROXIED_APP, PROXY_ID, PROXY_SPEC_ID, PUBLIC_PATH, TARGET_ID, USER_GROUPS,
    USER_ID,
};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// A configuration of the size a real deployment has (ten apps, several users, a full docker block).
fn realistic_configuration() -> String {
    let mut configuration = String::from(
        "proxy:\n  title: Benchmark\n  port: 8080\n  authentication: simple\n  \
         admin-groups: admins\n  heartbeat-rate: 10000\n  heartbeat-timeout: 60000\n  \
         container-wait-timeout: 20000\n  default-max-instances: 3\n  \
         docker:\n    port-range-start: 20000\n    port-range-max: 21000\n    \
         internal-networking: false\n    privileged: false\n  users:\n",
    );
    for index in 0..10 {
        configuration.push_str(&format!(
            "    - name: user{index}\n      password: password{index}\n      groups: [ scientists ]\n"
        ));
    }
    configuration.push_str("  specs:\n");
    for index in 0..10 {
        configuration.push_str(&format!(
            "    - id: app_{index}\n      display-name: Application {index}\n      \
             description: The description of application {index}\n      \
             container-image: openanalytics/shinyproxy-demo\n      \
             container-cmd: [ \"R\", \"-e\", \"shinyproxy::run_01_hello()\" ]\n      port: 3838\n      \
             access-groups: [ scientists, admins ]\n      \
             container-env:\n        VARIABLE_A: value\n        VARIABLE_B: '#{{proxy.userId}}'\n      \
             labels:\n        my.label: value\n      max-instances: 2\n"
        ));
    }
    configuration
}

/// A proxy with the runtime values a running app has.
fn example_proxy() -> Proxy {
    let mut proxy = Proxy::new("5f39a7cf-c9ff-4a85-9313-d561ec79cca9", ProxyStatus::Up);
    proxy.spec_id = Some("app_0".to_string());
    proxy.user_id = Some("user0".to_string());
    proxy.display_name = Some("Application 0".to_string());
    proxy.created_timestamp = 1_786_779_809_238;
    proxy.startup_timestamp = 1_786_779_812_000;
    for value in [
        RuntimeValue::string(&PROXY_ID, "5f39a7cf-c9ff-4a85-9313-d561ec79cca9"),
        RuntimeValue::string(&PROXY_SPEC_ID, "app_0"),
        RuntimeValue::string(&INSTANCE_ID, "0921be2f0d4dd567e61fae84be56eadca15418b1"),
        RuntimeValue::string(&USER_ID, "user0"),
        RuntimeValue::string(&USER_GROUPS, "SCIENTISTS"),
        RuntimeValue::string(&DISPLAY_NAME, "Application 0"),
        RuntimeValue::string(&CREATED_TIMESTAMP, "1786779809238"),
        RuntimeValue::string(&PROXIED_APP, "true"),
        RuntimeValue::string(
            &PUBLIC_PATH,
            "/app_proxy/5f39a7cf-c9ff-4a85-9313-d561ec79cca9/",
        ),
        RuntimeValue::string(&TARGET_ID, "5f39a7cf-c9ff-4a85-9313-d561ec79cca9"),
        RuntimeValue::integer(&HEARTBEAT_TIMEOUT, 60000),
        RuntimeValue::integer(&MAX_LIFETIME, -1),
    ] {
        proxy.add_runtime_value(value, true);
    }

    let mut container = Container::new(0);
    container.id = Some("96a9e43437e356a8bbd6abb5bd4aa9f1436db49d95b3de8abcf03bccb15e2254".into());
    container.add_runtime_value(
        RuntimeValue::json(
            &PORT_MAPPINGS,
            serde_json::json!([{"name": "default", "port": 3838, "targetPath": ""}]),
        ),
        true,
    );
    proxy.containers.push(container);
    proxy
}

fn configuration(criterion: &mut Criterion) {
    let text = realistic_configuration();
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("application.yml");
    std::fs::write(&path, &text).expect("write");

    let mut group = criterion.benchmark_group("configuration");
    group.bench_function("parse_and_bind_settings", |bencher| {
        bencher.iter(|| {
            let settings: Settings = serde_yaml_ng::from_str(&text).expect("settings");
            criterion::black_box(settings.proxy.specs.len())
        });
    });
    let schema = containerproxy::config::schema::Schema::engine();
    group.bench_function("load_like_the_binary", |bencher| {
        bencher.iter_batched(
            || LoadOptions {
                args: vec![format!("--spring.config.location={}", path.display())],
                ..LoadOptions::default()
            },
            |options| {
                let loaded =
                    containerproxy::config::load(&schema, &options).expect("configuration");
                criterion::black_box(loaded.unknown_properties.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn instance_id(criterion: &mut Criterion) {
    let text = realistic_configuration();
    let value: serde_json::Value = serde_yaml_ng::from_str(&text).expect("yaml");

    let mut group = criterion.benchmark_group("instance_id");
    group.bench_function("canonical_yaml", |bencher| {
        bencher.iter(|| {
            let yaml = containerproxy::util::canonical_yaml::to_canonical_yaml(&value);
            criterion::black_box(yaml.len())
        });
    });
    group.bench_function("canonical_yaml_and_sha1", |bencher| {
        bencher.iter(|| {
            let hash = containerproxy::service::identifier::instance_id_of(&value);
            criterion::black_box(hash.len())
        });
    });
    group.finish();
}

fn runtime_values(criterion: &mut Criterion) {
    let proxy = example_proxy();
    let container = proxy.containers[0].clone();

    let mut group = criterion.benchmark_group("runtime_values");
    group.bench_function("labels", |bencher| {
        bencher.iter(|| {
            let labels = proxy.runtime_values.labels();
            criterion::black_box(labels.len())
        });
    });
    group.bench_function("environment", |bencher| {
        bencher.iter(|| {
            let environment = proxy.runtime_values.environment();
            criterion::black_box(environment.len())
        });
    });
    group.bench_function("container_labels", |bencher| {
        bencher.iter(|| {
            let labels = container.runtime_values.labels();
            criterion::black_box(labels.len())
        });
    });
    group.finish();
}

fn json_views(criterion: &mut Criterion) {
    let proxy = example_proxy();
    let registry = containerproxy::model::runtime_value::RuntimeValueRegistry::engine();
    let internal = proxy.internal_json();

    let mut group = criterion.benchmark_group("json_views");
    group.bench_function("api_json", |bencher| {
        bencher.iter(|| criterion::black_box(proxy.api_json()));
    });
    group.bench_function("internal_json", |bencher| {
        bencher.iter(|| criterion::black_box(proxy.internal_json()));
    });
    group.bench_function("from_internal_json", |bencher| {
        bencher.iter(|| {
            let parsed = Proxy::from_internal_json(&registry, &internal).expect("proxy");
            criterion::black_box(parsed.id)
        });
    });
    group.finish();
}

fn headers(criterion: &mut Criterion) {
    use axum::http::{HeaderMap, HeaderValue};

    let mut request = HeaderMap::new();
    for (name, value) in [
        ("host", "shinyproxy.example.com"),
        ("user-agent", "Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0"),
        (
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("accept-encoding", "gzip, deflate, br"),
        ("accept-language", "en-US,en;q=0.9"),
        (
            "cookie",
            "JSESSIONID=3wnylr0l-y3nuEqrOGQVgZ0f95csQl0M6pS0zg5u",
        ),
        ("connection", "keep-alive"),
        ("keep-alive", "timeout=5"),
        ("upgrade-insecure-requests", "1"),
        ("x-forwarded-for", "203.0.113.7"),
    ] {
        request.insert(name, HeaderValue::from_static(value));
    }

    criterion.bench_function("dataplane/filter_headers", |bencher| {
        bencher.iter(|| {
            let filtered = filter_headers(&request);
            criterion::black_box(filtered.len())
        });
    });
}

criterion_group!(
    benches,
    configuration,
    instance_id,
    runtime_values,
    json_views,
    headers
);
criterion_main!(benches);
