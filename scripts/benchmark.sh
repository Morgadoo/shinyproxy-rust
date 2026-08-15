#!/usr/bin/env bash
#
# ShinyProxy
#
# Copyright (C) 2016-2026 Open Analytics
#
# ===========================================================================
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the Apache License as published by
# The Apache Software Foundation, either version 2 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# Apache License for more details.
#
# You should have received a copy of the Apache License
# along with this program.  If not, see <http://www.apache.org/licenses/>
#
# Benchmarks this implementation against the Java ShinyProxy 3.2.4 and writes a comparison report.
#
# Both servers get the *same* configuration and the *same* Docker backend, are measured one after the other on
# the same machine, and every phase is run against both:
#
#   startup      how long until the login page answers, and the resident memory right after that
#   app-cycles   how long the server needs to start an app and to stop it again (median of N cycles)
#   proxy        requests per second and latency through the reverse proxy, at 8, 32 and 128 connections
#   index        requests per second and latency of a page the server renders itself (`/`)
#   api          requests per second and latency of the JSON API (`/api/proxy`)
#   big          a 64 KB body streamed through the proxy (megabytes per second)
#   upload       a 64 KB body posted through the proxy
#   ws-churn     WebSocket connect + message + close, as fast as possible (handshakes per second)
#   websockets   the proxy load with N WebSocket connections held open, plus the memory at the end
#
# Every phase also reports the CPU seconds the *server* burned, so throughput per CPU can be compared.
#
# Usage:
#   ./scripts/build-test-image.sh                                  # the app image both servers start
#   ./scripts/benchmark.sh                                         # this implementation only
#   ./scripts/benchmark.sh /tmp/sp-java-build/target/shinyproxy-3.2.4-exec.jar   # both, with the comparison
#
# The report is written to docs/generated/benchmark.md (override with $SP_BENCH_REPORT), the raw metrics to
# $SP_BENCH_WORK/{rust,java}.metrics. Knobs: SP_BENCH_SECONDS (20), SP_BENCH_CONNECTIONS (32),
# SP_BENCH_WEBSOCKETS (100), SP_BENCH_CYCLES (5).

set -euo pipefail

JAR="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${SP_BENCH_WORK:-/tmp/benchmark}"
REPORT="${SP_BENCH_REPORT:-$REPO_ROOT/docs/generated/benchmark.md}"
IMAGE="${SP_TEST_IMAGE:-sp-testapp:test}"
SECONDS_PER_PHASE="${SP_BENCH_SECONDS:-20}"
CONNECTIONS="${SP_BENCH_CONNECTIONS:-32}"
WEBSOCKETS="${SP_BENCH_WEBSOCKETS:-100}"
CYCLES="${SP_BENCH_CYCLES:-5}"
PORT="${SP_BENCH_PORT:-8097}"
MANAGEMENT_PORT="${SP_BENCH_MANAGEMENT_PORT:-9097}"

LOAD="$REPO_ROOT/target/release/sp-testapp"
RUST_BINARY="${SP_RUST_BINARY:-$REPO_ROOT/target/release/shinyproxy}"

rm -rf "$WORK"
mkdir -p "$WORK" "$(dirname "$REPORT")"

echo "building the release binaries"
cargo build --release --bin shinyproxy --bin sp-testapp >/dev/null

cat >"$WORK/application.yml" <<EOF
proxy:
  title: Benchmark
  port: $PORT
  authentication: simple
  container-wait-timeout: 60000
  heartbeat-rate: 10000
  heartbeat-timeout: 60000
  users:
    - name: jack
      password: password
  docker:
    port-range-start: 28000
    port-range-max: 28400
  specs:
    - id: 01_hello
      display-name: Benchmark Application
      container-image: $IMAGE
      port: 3838
EOF

cleanup_containers() {
    local ids
    ids="$(docker ps -aq --filter 'label=openanalytics.eu/sp-proxied-app=true' 2>/dev/null || true)"
    if [[ -n "$ids" ]]; then
        # shellcheck disable=SC2086
        docker rm -f $ids >/dev/null 2>&1 || true
    fi
}

rss_kb() { # pid
    awk '/VmRSS/ {print $2}' "/proc/$1/status" 2>/dev/null || echo 0
}

# CPU seconds (user+system, including reaped children) of a process so far.
cpu_seconds() { # pid
    awk -v hz="$(getconf CLK_TCK)" '{printf "%.2f", ($14 + $15 + $16 + $17) / hz}' \
        "/proc/$1/stat" 2>/dev/null || echo 0
}

metric() { # file, name, value
    echo "$2 $3" >>"$1"
}

# Runs every phase against one server and writes `name value` lines.
benchmark_one() { # name, command...
    local name="$1"
    shift
    local metrics="$WORK/$name.metrics"
    : >"$metrics"
    cleanup_containers

    echo
    echo "=== $name ==="
    local started_at
    started_at=$(date +%s%3N)
    "$@" >"$WORK/$name-server.log" 2>&1 &
    local pid=$!
    # the pid of `java -jar` is the JVM itself, so it can be measured directly
    for _ in $(seq 1 600); do
        if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/login"; then
            break
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "error: $name exited during startup; see $WORK/$name-server.log" >&2
            tail -20 "$WORK/$name-server.log" >&2
            return 1
        fi
        sleep 0.1
    done
    local ready_at
    ready_at=$(date +%s%3N)
    metric "$metrics" startup_ms "$((ready_at - started_at))"
    metric "$metrics" rss_after_startup_mb "$(( $(rss_kb "$pid") / 1024 ))"
    echo "  startup: $((ready_at - started_at)) ms, $(( $(rss_kb "$pid") / 1024 )) MB"

    # how long a user waits for an app
    echo "  app start/stop cycles ($CYCLES)"
    "$LOAD" --load-test --machine-readable \
        --base-url "http://127.0.0.1:$PORT" --spec 01_hello \
        --measure-start-cycles "$CYCLES" |
        grep '^METRIC ' | sed 's/^METRIC //' >>"$metrics" || true

    # one phase: target, connections, websockets, metric prefix
    phase() {
        local target="$1" phase_connections="$2" phase_websockets="$3" prefix="$4"
        echo "  load: $prefix ($target, $SECONDS_PER_PHASE s, $phase_connections connections, $phase_websockets websockets)"
        local cpu_before cpu_after
        cpu_before="$(cpu_seconds "$pid")"
        "$LOAD" --load-test --machine-readable \
            --base-url "http://127.0.0.1:$PORT" --spec 01_hello --target "$target" \
            --websockets "$phase_websockets" --connections "$phase_connections" \
            --seconds "$SECONDS_PER_PHASE" |
            grep '^METRIC ' | sed "s/^METRIC /${prefix}_/" >>"$metrics" || true
        cpu_after="$(cpu_seconds "$pid")"
        metric "$metrics" "${prefix}_cpu_seconds" \
            "$(awk -v a="$cpu_after" -v b="$cpu_before" 'BEGIN {printf "%.2f", a - b}')"
    }

    # the stress ramp of the proxy path, then the other request paths
    phase app 8 0 app8
    phase app "$CONNECTIONS" 0 app
    phase app 128 0 app128
    phase index "$CONNECTIONS" 0 index
    phase api "$CONNECTIONS" 0 api
    phase big "$CONNECTIONS" 0 big
    phase upload "$CONNECTIONS" 0 upload
    phase ws-churn "$CONNECTIONS" 0 churn

    # the proxy path again, this time with WebSocket connections held open
    phase app "$CONNECTIONS" "$WEBSOCKETS" ws

    metric "$metrics" rss_under_load_mb "$(( $(rss_kb "$pid") / 1024 ))"

    if grep -qE 'panicked at|PANIC|Exception in thread' "$WORK/$name-server.log"; then
        echo "warning: $name logged a panic or an exception" >&2
        metric "$metrics" crashed 1
    else
        metric "$metrics" crashed 0
    fi

    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    cleanup_containers
}

benchmark_one rust "$RUST_BINARY" \
    --spring.config.location="$WORK/application.yml" \
    --management.server.port="$MANAGEMENT_PORT"

if [[ -n "$JAR" ]]; then
    if [[ ! -f "$JAR" ]]; then
        echo "error: $JAR not found" >&2
        exit 1
    fi
    benchmark_one java java -jar "$JAR" \
        --spring.config.location="$WORK/application.yml" \
        --management.server.port="$MANAGEMENT_PORT"
fi

# ---------------------------------------------------------------------------------------------------------
# the report
# ---------------------------------------------------------------------------------------------------------
python3 - "$WORK" "$REPORT" "$SECONDS_PER_PHASE" "$CONNECTIONS" "$WEBSOCKETS" "$CYCLES" <<'PYTHON'
import os
import subprocess
import sys
from datetime import datetime, timezone

work, report, seconds, connections, websockets, cycles = sys.argv[1:7]


def read(name):
    path = os.path.join(work, f"{name}.metrics")
    if not os.path.exists(path):
        return None
    values = {}
    for line in open(path):
        parts = line.split()
        if len(parts) == 2:
            try:
                values[parts[0]] = float(parts[1])
            except ValueError:
                pass
    return values


rust = read("rust")
java = read("java")
if rust is None:
    raise SystemExit("no metrics for this implementation")
BODY_KB = 64


def derive(values):
    """The numbers that are computed from the raw metrics."""
    if values is None:
        return
    for prefix in ("big", "upload"):
        rate = values.get(f"{prefix}_requests_per_second")
        if rate is not None:
            values[f"{prefix}_mb_per_second"] = rate * BODY_KB / 1024.0
    rate = values.get("app_requests_per_second")
    cpu = values.get("app_cpu_seconds")
    if rate and cpu is not None and rate > 0:
        total_requests = rate * float(seconds)
        if total_requests > 0:
            values["app_cpu_per_1k"] = cpu * 1000.0 * 1000.0 / total_requests  # ms per 1k requests




derive(rust)
derive(java)

# name, label, unit, higher_is_better
ROWS = [
    ("startup_ms", "Startup until the login page answers", "ms", False),
    ("rss_after_startup_mb", "Resident memory after startup", "MB", False),
    ("rss_under_load_mb", "Resident memory after the load phases", "MB", False),
    ("app_start_ms", "Starting an app (median)", "ms", False),
    ("app_stop_ms", "Stopping an app (median)", "ms", False),
    ("app8_requests_per_second", "Proxy requests per second, 8 connections", "req/s", True),
    ("app_requests_per_second", f"Proxy requests per second, {connections} connections", "req/s", True),
    ("app128_requests_per_second", "Proxy requests per second, 128 connections", "req/s", True),
    ("app_latency_p50_ms", "Proxy latency p50", "ms", False),
    ("app_latency_p99_ms", "Proxy latency p99", "ms", False),
    ("app128_latency_p99_ms", "Proxy latency p99 at 128 connections", "ms", False),
    ("app_cpu_per_1k", "Server CPU per 1000 proxied requests", "ms", False),
    ("index_requests_per_second", "Requests per second of the index page", "req/s", True),
    ("index_latency_p99_ms", "Index latency p99", "ms", False),
    ("api_requests_per_second", "Requests per second of the JSON API", "req/s", True),
    ("api_latency_p99_ms", "API latency p99", "ms", False),
    ("big_mb_per_second", f"Streaming a {BODY_KB} KB body through the proxy", "MB/s", True),
    ("upload_mb_per_second", f"Posting a {BODY_KB} KB body through the proxy", "MB/s", True),
    ("churn_requests_per_second", "WebSocket handshakes per second (connect + message + close)", "req/s", True),
    ("churn_latency_p99_ms", "WebSocket handshake p99", "ms", False),
    ("ws_requests_per_second", f"Proxy requests per second with {websockets} websockets open", "req/s", True),
    ("ws_latency_p99_ms", f"Proxy latency p99 with {websockets} websockets open", "ms", False),
]


def fmt(value, unit):
    if value is None:
        return "—"
    if unit == "req/s":
        return f"{value:,.0f}".replace(",", " ")
    if value >= 100:
        return f"{value:.0f}"
    return f"{value:.1f}"


def ratio(rust_value, java_value, higher_is_better):
    if rust_value is None or java_value is None or rust_value == 0 or java_value == 0:
        return "—"
    factor = rust_value / java_value if higher_is_better else java_value / rust_value
    if factor >= 1:
        return f"**{factor:.1f}× better**"
    return f"{1 / factor:.1f}× worse"


try:
    commit = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, check=False
    ).stdout.strip()
except Exception:
    commit = "unknown"

lines = [
    "# Benchmark: this implementation vs the Java ShinyProxy 3.2.4",
    "",
    f"Generated by `scripts/benchmark.sh` on {datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}"
    f" (commit `{commit}`).",
    "",
    "Both servers ran the same configuration with the Docker backend, one after the other, on the same"
    " machine.",
    f"Every load phase ran for {seconds} seconds with {connections} connections; the WebSocket phase held"
    f" {websockets} connections open. The app start/stop numbers are the median of {cycles} cycles.",
    "",
    "The `index` and `api` phases give every connection its own user session, because a servlet container"
    " serialises the requests of one session; the proxy phases necessarily share the session of the user that"
    " owns the app.",
    "",
]

if java is None:
    lines += [
        "> Only this implementation was measured. Pass the Java jar to compare:",
        "> `./scripts/benchmark.sh /path/to/shinyproxy-3.2.4-exec.jar`",
        "",
        "| Measurement | This implementation |",
        "| --- | --- |",
    ]
    for name, label, unit, _ in ROWS:
        lines.append(f"| {label} ({unit}) | {fmt(rust.get(name), unit)} |")
else:
    lines += [
        "| Measurement | This implementation | Java 3.2.4 | Difference |",
        "| --- | --- | --- | --- |",
    ]
    for name, label, unit, higher in ROWS:
        lines.append(
            f"| {label} ({unit}) | {fmt(rust.get(name), unit)} | {fmt(java.get(name), unit)} |"
            f" {ratio(rust.get(name), java.get(name), higher)} |"
        )

errors = [
    ("app8_errors", "proxy, 8 connections"),
    ("app_errors", "proxy"),
    ("app128_errors", "proxy, 128 connections"),
    ("index_errors", "index page"),
    ("api_errors", "JSON API"),
    ("big_errors", "streamed body"),
    ("upload_errors", "posted body"),
    ("churn_errors", "websocket churn"),
    ("ws_errors", "proxy with websockets"),
    ("ws_websocket_errors", "websocket connections"),
]
lines += ["", "## Errors", "", "| Phase | This implementation | Java 3.2.4 |", "| --- | --- | --- |"]
for name, label in errors:
    java_value = "—" if java is None else fmt(java.get(name, 0), "count")
    lines.append(f"| {label} | {fmt(rust.get(name, 0), 'count')} | {java_value} |")

crashed = ["", "Neither implementation panicked or threw during the run."]
if rust.get("crashed") or (java or {}).get("crashed"):
    crashed = ["", "**A server logged a panic or an exception during the run; see the logs in the work directory.**"]
lines += crashed

open(report, "w").write("\n".join(lines) + "\n")
print(f"wrote {report}")
PYTHON

echo
cat "$REPORT"
