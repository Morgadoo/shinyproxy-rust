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

//! Micro-benchmarks of the expression engine, which runs for every `#{...}` field of every app start.

use criterion::{criterion_group, criterion_main, Criterion};
use spel::{Context, Expression};

/// The context an app start has.
fn context() -> Context {
    Context::new()
        .with_root("userId", "jack")
        .with_root(
            "groups",
            vec!["scientists".to_string(), "mathematicians".to_string()],
        )
        .with_root("proxyId", "5f39a7cf-c9ff-4a85-9313-d561ec79cca9")
        .with_root("appId", "01_hello")
}

fn parsing(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("spel/parse");
    for (name, expression) in [
        ("literal", "'/home/' + userId"),
        ("ternary", "groups.contains('SCIENTISTS') ? 10 : 1"),
        (
            "nested",
            "userId.toLowerCase() + '-' + (groups.isEmpty() ? 'none' : groups.get(0)) + '/data'",
        ),
    ] {
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                let parsed = Expression::parse(expression).expect("expression");
                criterion::black_box(parsed)
            });
        });
    }
    group.finish();
}

fn evaluation(criterion: &mut Criterion) {
    let context = context();
    let mut group = criterion.benchmark_group("spel/evaluate");
    for (name, expression) in [
        ("literal", "'/home/' + userId"),
        ("ternary", "groups.contains('SCIENTISTS') ? 10 : 1"),
        (
            "nested",
            "userId.toLowerCase() + '-' + (groups.isEmpty() ? 'none' : groups.get(0)) + '/data'",
        ),
    ] {
        let parsed = Expression::parse(expression).expect("expression");
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                let value = parsed.evaluate(&context).expect("value");
                criterion::black_box(value)
            });
        });
    }
    group.finish();
}

fn templates(criterion: &mut Criterion) {
    let context = context();
    let mut group = criterion.benchmark_group("spel/template");
    for (name, template) in [
        ("without_expression", "/home/shiny/data"),
        ("one_expression", "/home/#{userId}/data"),
        (
            "several_expressions",
            "/mnt/#{userId}/#{appId}/#{groups.contains('scientists') ? 'science' : 'other'}/data",
        ),
    ] {
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                let text = spel::evaluate_template(template, &context).expect("text");
                criterion::black_box(text)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, parsing, evaluation, templates);
criterion_main!(benches);
