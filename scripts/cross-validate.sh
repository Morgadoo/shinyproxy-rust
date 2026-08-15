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
# Compares this implementation with the Java ShinyProxy 3.2.4, scenario by scenario.
#
# Both servers are started with the *same* configuration and the Docker backend, and a scripted scenario list
# is run against each of them: the login flow, the index page, the app definitions of the API, starting an app,
# the status endpoint, the app page, a proxied request, the admin data, stopping the app and a few error cases.
# The answers (status codes, normalised JSON, the interesting headers, and the labels and environment of the
# container) are written to a report so that every difference is either fixed or documented in
# docs/COMPATIBILITY.md.
#
# Usage:
#   # build the Java jar once (the sources are no longer part of this repository):
#   git archive <commit-that-still-had-java> src pom.xml | tar -x -C /tmp/sp-java-build
#   (cd /tmp/sp-java-build && mvn -B -DskipTests package)
#
#   ./scripts/build-test-image.sh                       # the app image both servers start
#   ./scripts/cross-validate.sh /tmp/sp-java-build/target/shinyproxy-3.2.4-exec.jar
#
# The report is written to /tmp/cross-validation/report.md.

set -euo pipefail

JAR="${1:-/tmp/sp-java-build/target/shinyproxy-3.2.4-exec.jar}"
WORK="${SP_CROSS_WORK:-/tmp/cross-validation}"
IMAGE="${SP_TEST_IMAGE:-sp-testapp:test}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_BINARY="${SP_RUST_BINARY:-$REPO_ROOT/target/release/shinyproxy}"

if [[ ! -f "$JAR" ]]; then
    echo "error: $JAR not found; see the header of this script" >&2
    exit 1
fi
if [[ ! -x "$RUST_BINARY" ]]; then
    echo "error: $RUST_BINARY not found; run cargo build --release" >&2
    exit 1
fi

rm -rf "$WORK"
mkdir -p "$WORK"

cat >"$WORK/application.yml" <<EOF
proxy:
  title: Cross Validation
  port: 8080
  authentication: simple
  admin-groups: admins
  hide-navbar: false
  container-wait-timeout: 30000
  heartbeat-rate: 10000
  heartbeat-timeout: 60000
  allow-transfer-app: true
  users:
    - name: jack
      password: password
      groups: scientists
    - name: root
      password: rootpw
      groups: admins
  docker:
    port-range-start: 26000
  specs:
    - id: 01_hello
      display-name: Hello Application
      description: Application which demonstrates the basics of a Shiny app
      container-image: $IMAGE
      port: 3838
      access-groups: [ scientists, admins ]
    - id: 02_other
      display-name: Other Application
      container-image: $IMAGE
      port: 3838
      access-groups: admins
EOF

# ---------------------------------------------------------------------------------------------------------
# scenario runner: everything one implementation is asked, in order
# ---------------------------------------------------------------------------------------------------------
run_scenarios() {
    local name="$1" base="$2" out="$3"
    local jar_cookies="$WORK/$name-cookies.txt"
    rm -f "$jar_cookies"
    : >"$out"

    record() { # description, curl arguments...
        local description="$1"
        shift
        {
            echo "### $description"
            echo '```'
            curl -sS -o "$WORK/body.tmp" -D "$WORK/headers.tmp" "$@" || true
            echo "status: $(head -1 "$WORK/headers.tmp" | tr -d '\r' | awk '{print $2}')"
            # only the headers that are part of the contract
            # `|| true`: a request without any of these headers is not an error
            # header names are case insensitive, and so is the charset spelling; both are lower cased so
            # that the report only shows differences that matter
            (grep -iE '^(location|content-type|cache-control|x-frame-options|x-content-type-options|set-cookie):' \
                "$WORK/headers.tmp" || true) | tr -d '\r' |
                sed -e 's/^\([A-Za-z-]*\):/\L\1:/' -e 's/; *charset=UTF-8/;charset=utf-8/I' \
                    -e 's/JSESSIONID=[^;]*/JSESSIONID=<id>/' -e 's/SESSION=[^;]*/SESSION=<id>/' \
                    -e 's#http://127.0.0.1:80[0-9][0-9]##' | sort
            # the body, normalised: ids, timestamps and ports are different by nature
            if grep -qi 'application/json' "$WORK/headers.tmp"; then
                python3 - "$WORK/body.tmp" <<'PYTHON'
import json, re, sys

def normalise(value):
    if isinstance(value, dict):
        return {key: normalise(item) for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalise(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', '<uuid>', value)
        # container ids (64 hex), the instance id (40 hex) and the host name of a container
        value = re.sub(r'\b[0-9a-f]{64}\b', '<container-id>', value)
        value = re.sub(r'\b[0-9a-f]{40}\b', '<sha1>', value)
        value = re.sub(r'\b[0-9a-f]{12}\b', '<hostname>', value)
        value = re.sub(r'\b(1[0-9]{12})\b', '<timestamp>', value)
        value = re.sub(r':2[0-9]{4}\b', ':<port>', value)
        return value
    if isinstance(value, int) and value > 1_000_000_000_000:
        return '<timestamp>'
    return value

try:
    document = json.load(open(sys.argv[1]))
except Exception as error:  # not JSON after all
    print(f'<unparseable json: {error}>')
else:
    print(json.dumps(normalise(document), indent=2, sort_keys=True))
PYTHON
            else
                # HTML: only the structure that matters (titles, forms, the app ids)
                (grep -oE '<title>[^<]*</title>|data-app-id="[^"]*"|id="[a-zA-Z-]+"|name="[a-zA-Z_]+"' \
                    "$WORK/body.tmp" || true) | sort -u
            fi
            echo '```'
            echo
        } >>"$out"
    }

    record "GET /login" "$base/login"
    record "POST /login (wrong password)" -X POST -d "username=jack&password=wrong" \
        -H 'Content-Type: application/x-www-form-urlencoded' "$base/login"

    # the login needs the CSRF token of the session
    curl -sS -c "$jar_cookies" -o "$WORK/login.html" "$base/login"
    local token
    token="$( (grep -oE 'name="_csrf" value="[^"]+"' "$WORK/login.html" || true) | head -1 |
        sed -e 's/.*value="//' -e 's/"//')"
    curl -sS -b "$jar_cookies" -c "$jar_cookies" -o /dev/null -X POST \
        -d "username=jack&password=password&_csrf=$token" \
        -H 'Content-Type: application/x-www-form-urlencoded' "$base/login"

    record "GET / (logged in)" -b "$jar_cookies" "$base/"
    record "GET /api/proxyspec" -b "$jar_cookies" -H 'Accept: application/json' "$base/api/proxyspec"
    record "GET /api/proxyspec/01_hello" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxyspec/01_hello"
    record "GET /api/proxyspec/02_other (no access)" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxyspec/02_other"
    record "GET /api/proxy (empty)" -b "$jar_cookies" -H 'Accept: application/json' "$base/api/proxy"
    record "GET /app/01_hello" -b "$jar_cookies" "$base/app/01_hello"

    # start an app and wait for it
    curl -sS -b "$jar_cookies" -o "$WORK/$name-started.json" -X POST \
        -H 'Content-Type: application/json' -d '{}' "$base/app_i/01_hello/_"
    local proxy_id
    proxy_id="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['data']['id'])" \
        "$WORK/$name-started.json")"
    curl -sS -b "$jar_cookies" -o /dev/null \
        "$base/api/proxy/$proxy_id/status?watch=true&timeout=30"

    record "GET /api/proxy (one app)" -b "$jar_cookies" -H 'Accept: application/json' "$base/api/proxy"
    record "GET /api/proxy/{id}" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxy/$proxy_id"
    record "GET /api/proxy/{id}/status" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxy/$proxy_id/status"
    record "GET /app_proxy/{id}/ (the app answers)" -b "$jar_cookies" "$base/app_proxy/$proxy_id/"
    record "GET /app_proxy/{id}/env (the environment of the app)" -b "$jar_cookies" \
        -H 'Accept: application/json' "$base/app_proxy/$proxy_id/env"
    record "POST /heartbeat/{id}" -b "$jar_cookies" -X POST "$base/heartbeat/$proxy_id"
    record "GET /admin (not an administrator)" -b "$jar_cookies" "$base/admin"
    record "GET /api/proxy/unknown-id/status" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxy/unknown-id/status"
    record "GET /app_proxy/unknown-id/" -b "$jar_cookies" "$base/app_proxy/unknown-id/"

    # the container of the app, as the backend created it
    {
        echo "### docker inspect (labels and environment)"
        echo '```'
        local container
        container="$(docker ps --filter "label=openanalytics.eu/sp-proxy-id=$proxy_id" \
            --format '{{.ID}}' 2>/dev/null | head -1 || true)"
        if [[ -n "$container" ]]; then
            docker inspect "$container" --format '{{json .Config.Labels}}' |
                python3 -c "
import json, re, sys
labels = json.load(sys.stdin)
for key in sorted(labels):
    value = re.sub(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', '<uuid>', labels[key])
    value = re.sub(r'[0-9a-f]{40}', '<sha1>', value)
    value = re.sub(r'\b1[0-9]{12}\b', '<timestamp>', value)
    print(f'{key}={value}')
"
            docker inspect "$container" --format '{{json .Config.Env}}' |
                python3 -c "
import json, re, sys
for entry in sorted(json.load(sys.stdin)):
    if entry.startswith('SHINYPROXY_'):
        entry = re.sub(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', '<uuid>', entry)
        print(entry)
"
        else
            echo "<no container found>"
        fi
        echo '```'
        echo
    } >>"$out"

    # administration with the administrator account
    local admin_cookies="$WORK/$name-admin-cookies.txt"
    rm -f "$admin_cookies"
    curl -sS -c "$admin_cookies" -o "$WORK/login.html" "$base/login"
    token="$( (grep -oE 'name="_csrf" value="[^"]+"' "$WORK/login.html" || true) | head -1 |
        sed -e 's/.*value="//' -e 's/"//')"
    curl -sS -b "$admin_cookies" -c "$admin_cookies" -o /dev/null -X POST \
        -d "username=root&password=rootpw&_csrf=$token" \
        -H 'Content-Type: application/x-www-form-urlencoded' "$base/login"
    record "GET /admin (administrator)" -b "$admin_cookies" "$base/admin"
    record "GET /admin/data" -b "$admin_cookies" -H 'Accept: application/json' "$base/admin/data"

    # stop the app again
    record "PUT /api/proxy/{id}/status (stop)" -b "$jar_cookies" -X PUT \
        -H 'Content-Type: application/json' -d '{"status":"Stopping"}' \
        "$base/api/proxy/$proxy_id/status"
    sleep 5
    record "GET /api/proxy (after the stop)" -b "$jar_cookies" -H 'Accept: application/json' \
        "$base/api/proxy"
    record "GET /logout" -b "$jar_cookies" "$base/logout"
}

wait_for() { # base url
    for _ in $(seq 1 120); do
        if curl -fsS -o /dev/null "$1/login"; then
            return 0
        fi
        sleep 1
    done
    echo "error: $1 did not start" >&2
    return 1
}

cleanup_containers() {
    local ids
    ids="$(docker ps -aq --filter 'label=openanalytics.eu/sp-proxied-app=true')"
    if [[ -n "$ids" ]]; then
        # shellcheck disable=SC2086
        docker rm -f $ids >/dev/null 2>&1 || true
    fi
}

# ---------------------------------------------------------------------------------------------------------
# the Java implementation
# ---------------------------------------------------------------------------------------------------------
cleanup_containers
echo "starting the Java implementation"
java -jar "$JAR" --spring.config.location="$WORK/application.yml" --proxy.port=8091 \
    --management.server.port=9091 >"$WORK/java.log" 2>&1 &
JAVA_PID=$!
trap 'kill $JAVA_PID 2>/dev/null || true' EXIT
wait_for http://127.0.0.1:8091
run_scenarios java http://127.0.0.1:8091 "$WORK/java.md"
kill "$JAVA_PID" 2>/dev/null || true
wait "$JAVA_PID" 2>/dev/null || true
trap - EXIT
cleanup_containers

# ---------------------------------------------------------------------------------------------------------
# this implementation
# ---------------------------------------------------------------------------------------------------------
echo "starting the Rust implementation"
"$RUST_BINARY" --spring.config.location="$WORK/application.yml" --proxy.port=8092 \
    --management.server.port=9092 >"$WORK/rust.log" 2>&1 &
RUST_PID=$!
trap 'kill $RUST_PID 2>/dev/null || true' EXIT
wait_for http://127.0.0.1:8092
run_scenarios rust http://127.0.0.1:8092 "$WORK/rust.md"
kill "$RUST_PID" 2>/dev/null || true
wait "$RUST_PID" 2>/dev/null || true
trap - EXIT
cleanup_containers

# ---------------------------------------------------------------------------------------------------------
# the report
# ---------------------------------------------------------------------------------------------------------
{
    echo "# Cross validation: Java ShinyProxy 3.2.4 vs this implementation"
    echo
    echo "Generated by \`scripts/cross-validate.sh\` on $(date -u +%Y-%m-%dT%H:%M:%SZ)."
    echo
    echo "Both servers ran the same configuration with the Docker backend. Ids, timestamps and host ports are"
    echo "normalised; everything else is compared verbatim."
    echo
    if diff -u "$WORK/java.md" "$WORK/rust.md" >"$WORK/diff.txt"; then
        echo "**No differences.**"
    else
        echo "## Differences"
        echo
        echo '```diff'
        cat "$WORK/diff.txt"
        echo '```'
    fi
} >"$WORK/report.md"

echo "wrote $WORK/report.md ($(grep -c '^' "$WORK/diff.txt") lines of diff)"
