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

//! The Prometheus exposition format of the metrics.
//!
//! Micrometer's Prometheus registry writes counters as `{name}_total`, timers as `{name}_seconds_count`
//! and `{name}_seconds_sum`, and turns `.` in names and tag keys into `_`. This module does the same, so
//! that the output of `/actuator/prometheus` matches the Java implementation.

use std::collections::BTreeMap;

use super::Metrics;

/// The series of one metric: its labels and its value.
type Series<'a, T> = Vec<(&'a BTreeMap<String, String>, T)>;

/// Renders the metrics.
pub fn render(metrics: &Metrics) -> String {
    let (counters, gauges, timers) = metrics.snapshot();
    let mut output = String::new();

    // counters: one HELP/TYPE block per metric name
    let mut by_name: BTreeMap<String, Series<u64>> = BTreeMap::new();
    for ((name, labels), value) in &counters {
        by_name
            .entry(name.clone())
            .or_default()
            .push((labels, *value));
    }
    for (name, series) in by_name {
        let metric = format!("{}{}_total", metrics.prefix(), sanitise(&name));
        output.push_str(&format!("# HELP {metric}\n# TYPE {metric} counter\n"));
        for (labels, value) in series {
            output.push_str(&format!(
                "{metric}{} {value}\n",
                render_labels(labels, metrics.common_labels())
            ));
        }
    }

    let mut by_name: BTreeMap<String, Series<f64>> = BTreeMap::new();
    for ((name, labels), value) in &gauges {
        by_name
            .entry(name.clone())
            .or_default()
            .push((labels, *value));
    }
    for (name, series) in by_name {
        let metric = format!("{}{}", metrics.prefix(), sanitise(&name));
        output.push_str(&format!("# HELP {metric}\n# TYPE {metric} gauge\n"));
        for (labels, value) in series {
            output.push_str(&format!(
                "{metric}{} {}\n",
                render_labels(labels, metrics.common_labels()),
                format_float(value)
            ));
        }
    }

    let mut by_name: BTreeMap<String, Series<(u64, f64)>> = BTreeMap::new();
    for ((name, labels), value) in &timers {
        by_name
            .entry(name.clone())
            .or_default()
            .push((labels, *value));
    }
    for (name, series) in by_name {
        let metric = format!("{}{}_seconds", metrics.prefix(), sanitise(&name));
        output.push_str(&format!("# HELP {metric}\n# TYPE {metric} summary\n"));
        for (labels, (count, sum)) in series {
            let rendered = render_labels(labels, metrics.common_labels());
            output.push_str(&format!("{metric}_count{rendered} {count}\n"));
            output.push_str(&format!("{metric}_sum{rendered} {}\n", format_float(sum)));
        }
    }

    output
}

/// Turns a metric name into a valid Prometheus name.
fn sanitise(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == ':' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Renders the labels of a series, adding the labels that every metric has.
fn render_labels(labels: &BTreeMap<String, String>, common: &BTreeMap<String, String>) -> String {
    let mut all: BTreeMap<&String, &String> = BTreeMap::new();
    for (key, value) in common {
        all.insert(key, value);
    }
    for (key, value) in labels {
        all.insert(key, value);
    }
    if all.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = all
        .into_iter()
        .map(|(key, value)| format!("{}=\"{}\"", sanitise(key), escape(value)))
        .collect();
    format!("{{{}}}", rendered.join(","))
}

/// Escapes a label value.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Formats a float the way Prometheus expects it.
fn format_float(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{:.1}", value)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::identifier::Identifiers;
    use crate::stat::labels;

    fn identifiers() -> Identifiers {
        Identifiers {
            runtime_id: "runtime".to_string(),
            instance_id: "instance-1".to_string(),
            realm_id: None,
            version: None,
        }
    }

    #[test]
    fn renders_the_micrometer_format() {
        let metrics = Metrics::new(None, &identifiers());
        metrics.increment("appStarts", labels([("spec_id", "01_hello")]));
        metrics.set_gauge(
            "absolute_apps_running",
            labels([("spec_id", "01_hello")]),
            2.0,
        );
        metrics.record("startupTime", labels([("spec_id", "01_hello")]), 1500);

        let output = metrics.to_prometheus();
        assert!(
            output.contains("# TYPE appStarts_total counter"),
            "{output}"
        );
        assert!(
            output.contains(
                "appStarts_total{shinyproxy_instance=\"instance-1\",shinyproxy_realm=\"\",spec_id=\"01_hello\"} 1"
            ),
            "{output}"
        );
        assert!(
            output.contains("# TYPE absolute_apps_running gauge"),
            "{output}"
        );
        assert!(
            output.contains(
                "absolute_apps_running{shinyproxy_instance=\"instance-1\",shinyproxy_realm=\"\",spec_id=\"01_hello\"} 2.0"
            ),
            "{output}"
        );
        assert!(
            output.contains("startupTime_seconds_count{shinyproxy_instance=\"instance-1\",shinyproxy_realm=\"\",spec_id=\"01_hello\"} 1"),
            "{output}"
        );
        assert!(
            output.contains("startupTime_seconds_sum{shinyproxy_instance=\"instance-1\",shinyproxy_realm=\"\",spec_id=\"01_hello\"} 1.5"),
            "{output}"
        );
    }

    #[test]
    fn uses_the_configured_prefix() {
        let metrics = Metrics::new(Some("shinyproxy"), &identifiers());
        metrics.increment("appStarts", labels([("spec_id", "app")]));
        let output = metrics.to_prometheus();
        assert!(output.contains("shinyproxy_appStarts_total{"), "{output}");
    }

    #[test]
    fn escapes_label_values() {
        let metrics = Metrics::new(None, &identifiers());
        metrics.set_gauge(
            "appInfo",
            labels([("user_id", "jack \"the\\ user\"")]),
            10.0,
        );
        let output = metrics.to_prometheus();
        assert!(
            output.contains(r#"user_id="jack \"the\\ user\"""#),
            "{output}"
        );
    }
}
