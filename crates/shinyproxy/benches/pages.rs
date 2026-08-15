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

//! Micro-benchmarks of the work behind a page: building the model and rendering the template.
//!
//! The index page is the one a user hits on every visit, and the Java implementation renders it with
//! Thymeleaf; `scripts/benchmark.sh` compares the two end to end, while this benchmark tracks regressions in
//! this implementation.

use std::sync::Arc;

use containerproxy::auth::AuthenticatedUser;
use containerproxy::config::LoadOptions;
use containerproxy::spec::SpecProvider;
use criterion::{criterion_group, criterion_main, Criterion};
use shinyproxy::web::model::{prepare_model, Page};
use shinyproxy::web::AppState;

/// A configuration with `count` app definitions.
fn configuration(count: usize) -> String {
    let mut text = String::from(
        "proxy:\n  title: Benchmark\n  authentication: simple\n  admin-groups: admins\n  \
         container-backend: local\n  users:\n    - name: jack\n      password: password\n      \
         groups: [ scientists ]\n  specs:\n",
    );
    for index in 0..count {
        text.push_str(&format!(
            "    - id: app_{index}\n      display-name: Application {index}\n      \
             description: The description of application {index}\n      \
             container-image: sp-testapp\n      access-groups: [ scientists ]\n"
        ));
    }
    text
}

/// The state of a server with `count` app definitions.
fn state(runtime: &tokio::runtime::Runtime, count: usize) -> Arc<AppState> {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("application.yml");
    std::fs::write(&path, configuration(count)).expect("write");
    let options = LoadOptions {
        args: vec![format!("--spring.config.location={}", path.display())],
        ..LoadOptions::default()
    };
    let (raw, settings) = shinyproxy::load_config(options).expect("configuration");
    let state = runtime
        .block_on(AppState::new(raw, settings))
        .expect("state");
    // the temporary directory may go away; the configuration is already loaded
    drop(directory);
    Arc::new(state)
}

fn model_and_render(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let user = AuthenticatedUser::new("jack", vec!["scientists".to_string()]);

    let mut group = criterion.benchmark_group("index_page");
    for count in [1usize, 10, 50] {
        let state = state(&runtime, count);
        group.bench_function(format!("prepare_model/{count}_apps"), |bencher| {
            bencher.iter(|| {
                let model = prepare_model(&state, Page::Index, Some(&user), false);
                criterion::black_box(model.len())
            });
        });
        group.bench_function(format!("render/{count}_apps"), |bencher| {
            let model = prepare_model(&state, Page::Index, Some(&user), false);
            bencher.iter(|| {
                let html = state
                    .templates
                    .render(
                        "index.html",
                        minijinja::value::Value::from_serialize(&model),
                    )
                    .expect("html");
                criterion::black_box(html.len())
            });
        });
    }
    group.finish();
}

fn access_control(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let state = state(&runtime, 50);
    let user = AuthenticatedUser::new("jack", vec!["scientists".to_string()]);

    let mut group = criterion.benchmark_group("access_control");
    group.bench_function("accessible_specs/50_apps", |bencher| {
        bencher.iter(|| {
            let count = state
                .specs
                .specs()
                .iter()
                .filter(|spec| state.can_access(Some(&user), spec))
                .count();
            criterion::black_box(count)
        });
    });
    group.bench_function("max_instances/50_apps", |bencher| {
        bencher.iter(|| {
            let instances = state.max_instances(Some(&user));
            criterion::black_box(instances.len())
        });
    });
    group.finish();
}

criterion_group!(benches, model_and_render, access_control);
criterion_main!(benches);
