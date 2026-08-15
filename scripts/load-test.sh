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
# Puts load through the proxy and reports the latency, the throughput and the memory of the server.
#
# The load generator is `crates/testapp --load-test` (it holds WebSocket sessions open and hammers the HTTP
# path at the same time), so no extra tools are needed. The same script can drive the Java implementation, so
# the numbers can be compared on one machine:
#
#   ./scripts/build-test-image.sh
#   ./scripts/load-test.sh                     # this implementation
#   ./scripts/load-test.sh /tmp/sp-java-build/target/shinyproxy-3.2.4-exec.jar   # the Java one
#
# Environment: SP_LOAD_SECONDS (default 60), SP_LOAD_WEBSOCKETS (200), SP_LOAD_CONNECTIONS (32).

set -euo pipefail

JAR="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${SP_LOAD_WORK:-/tmp/load-test}"
IMAGE="${SP_TEST_IMAGE:-sp-testapp:test}"
SECONDS_TO_RUN="${SP_LOAD_SECONDS:-60}"
WEBSOCKETS="${SP_LOAD_WEBSOCKETS:-200}"
CONNECTIONS="${SP_LOAD_CONNECTIONS:-32}"
PORT="${SP_LOAD_PORT:-8095}"
MANAGEMENT_PORT="${SP_LOAD_MANAGEMENT_PORT:-9095}"

rm -rf "$WORK"
mkdir -p "$WORK"

cat >"$WORK/application.yml" <<EOF
proxy:
  title: Load Test
  port: $PORT
  authentication: simple
  container-wait-timeout: 30000
  heartbeat-rate: 10000
  heartbeat-timeout: 60000
  users:
    - name: jack
      password: password
  docker:
    port-range-start: 27000
  specs:
    - id: 01_hello
      display-name: Load Test Application
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
cleanup_containers

if [[ -n "$JAR" ]]; then
    name="java"
    java -Xmx1g -jar "$JAR" --spring.config.location="$WORK/application.yml" \
        --management.server.port="$MANAGEMENT_PORT" >"$WORK/server.log" 2>&1 &
else
    name="rust"
    cargo build --release --bin shinyproxy --bin sp-testapp >/dev/null
    "$REPO_ROOT/target/release/shinyproxy" --spring.config.location="$WORK/application.yml" \
        --management.server.port="$MANAGEMENT_PORT" >"$WORK/server.log" 2>&1 &
fi
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; cleanup_containers' EXIT

started_at=$(date +%s%3N)
for _ in $(seq 1 180); do
    if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/login"; then
        break
    fi
    sleep 0.2
done
ready_at=$(date +%s%3N)
startup_ms=$((ready_at - started_at))
echo "$name: ready in ${startup_ms} ms"

# the resident memory of the server right after startup
rss_after_startup="$(awk '/VmRSS/ {print $2}' "/proc/$SERVER_PID/status" 2>/dev/null || echo 0)"

"$REPO_ROOT/target/release/sp-testapp" --load-test \
    --base-url "http://127.0.0.1:$PORT" \
    --username jack \
    --password password \
    --spec 01_hello \
    --websockets "$WEBSOCKETS" \
    --connections "$CONNECTIONS" \
    --seconds "$SECONDS_TO_RUN" | tee "$WORK/$name-load.txt"

rss_after_load="$(awk '/VmRSS/ {print $2}' "/proc/$SERVER_PID/status" 2>/dev/null || echo 0)"

{
    echo "implementation: $name"
    echo "startup_ms: $startup_ms"
    echo "rss_after_startup_kb: $rss_after_startup"
    echo "rss_after_load_kb: $rss_after_load"
    grep -E '^(requests|errors|websockets|latency)' "$WORK/$name-load.txt" || true
} >"$WORK/$name-summary.txt"

echo
cat "$WORK/$name-summary.txt"

# no panic may show up in the log of the server
if grep -qE 'panicked at|PANIC' "$WORK/server.log"; then
    echo "error: the server panicked during the load test" >&2
    exit 1
fi
