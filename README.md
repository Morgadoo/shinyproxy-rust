<pre>
               _____ _     _             _____
              / ____| |   (_)           |  __ \
             | (___ | |__  _ _ __  _   _| |__) | __ _____  ___   _
              \___ \| '_ \| | '_ \| | | |  ___/ '__/ _ \ \/ / | | |
              ____) | | | | | | | | |_| | |   | | | (_) >  <| |_| |
             |_____/|_| |_|_|_| |_|\__, |_|   |_|  \___/_/\_\\__, |
                                    __/ |                     __/ |
                                   |___/                     |___/

</pre>

# ShinyProxy (Rust)

Open Source Enterprise Deployment for Data Science Apps — a **Rust reimplementation** of
[ShinyProxy](https://shinyproxy.io) by Open Analytics (which bundles the
[ContainerProxy](https://github.com/openanalytics/containerproxy) engine).

**(c) Copyright Open Analytics NV, 2016-2026 - Apache License 2.0**

This repository contains a from-scratch rewrite of ShinyProxy 3.2.4 in Rust. It is a drop-in replacement: the
same `application.yml`, the same HTTP routes and JSON API, the same container labels and environment
variables, the same pages — but a single static binary instead of a JVM, and a fraction of the memory.
Everything that behaves differently is listed in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

## Quick start

```sh
# build (Rust 1.97 or newer, see rust-toolchain.toml)
cargo build --release

# run with the built-in demo configuration (needs Docker for the demo apps)
./target/release/shinyproxy

# or with your own configuration
./target/release/shinyproxy --spring.config.location=/etc/shinyproxy/application.yml
```

Navigate to <http://localhost:8080>; the demo configuration logs in with the username `jack` and the
password `password`.

With Docker:

```sh
docker build -t shinyproxy-rust .
docker run --rm -p 8080:8080 -v /var/run/docker.sock:/var/run/docker.sock \
    -v "$PWD/examples/application-demo.yml:/opt/shinyproxy/application.yml:ro" \
    shinyproxy-rust
```

## What is supported

| Area | Status |
| --- | --- |
| Configuration (`application.yml`, environment variables, profiles, `#{...}` expressions) | Spring-compatible, including relaxed binding and placeholders |
| Container backends | `docker`, `docker-swarm`, `kubernetes`, `ecs` (best effort, see below), `local` (testing only) |
| Authentication | `none`, `simple`, `ldap`, `openid` (auth-code flow, PKCE, refresh, MS Graph groups), `webservice`, `custom-header`, OAuth2 bearer tokens for the API |
| UI | index, app page, admin pages, parameters, template groups, logos, my-apps modes, error pages — ported from the Thymeleaf templates |
| API | every documented endpoint, plus the OpenAPI document and Swagger UI |
| Operations | heartbeats and timeouts, max lifetime, container logs, Prometheus metrics on a management port, usage statistics (CSV and SQL), structured and JSON logging |
| High availability | `store-mode: Redis` (apps, heartbeats, host ports, sessions, seats), leader election, rolling updates with `proxy.version` |
| Pre-initialized containers | `minimum-seats-available` with the seat model, the scaler and the delegate proxies |

Known gaps: SAML authentication (fails at startup with a migration message), the ECS backend has not been
validated against a real AWS account, and a few operational extras (S3 log storage, InfluxDB usage
statistics, request dumping). See [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) for the full list and
[docs/PROGRESS.md](docs/PROGRESS.md) for the state of the rewrite.

## Documentation

* [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — every configuration property that is understood
  (generated from the schema).
* [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) — behaviour compared with the Java implementation, and
  every deviation.
* [docs/MIGRATION.md](docs/MIGRATION.md) — how to move an existing ShinyProxy deployment to this build.
* [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the crates map onto the Java layering.
* [docs/TESTING.md](docs/TESTING.md) — how to run the test suites, including the ones that need Docker,
  Kubernetes, Redis or LDAP.
* [docs/PROGRESS.md](docs/PROGRESS.md) — phases, test inventory and what is left.
* [docs/reviews/](docs/reviews/) — delivery waves (Wave 1–3 status and gap closure tracker).
* The upstream documentation of the configuration format lives at <https://shinyproxy.io>.

## Repository layout

```
crates/spel            SpEL-compatible expression engine for #{...} expressions
crates/containerproxy  the engine: configuration, model, backends, authentication, lifecycle, data plane, API
crates/shinyproxy      the binary: ShinyProxy notation, controllers, templates, assets
crates/testapp         the test fixture app used by the integration tests
assets/                templates and static assets (embedded in the binary)
examples/              example configurations (demo, high availability, Kubernetes)
docs/                  documentation
scripts/               development helpers (test images, Redis/LDAP/k3s for the tests, license headers)
tools/                 cross-validation of the expression engine against Spring
```

## Development

```sh
cargo fmt --all                      # formatting
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace               # ~500 tests, no external services needed
./scripts/license-header.sh --check  # every source file carries the Apache-2.0 header
```

The suites that need a service are opt-in: `SP_TEST_DOCKER=1`, `SP_TEST_K8S=1`, `SP_TEST_REDIS=1` and
`SP_TEST_LDAP=1` (see [docs/TESTING.md](docs/TESTING.md)).

## Support

ShinyProxy itself is developed by Open Analytics; see the [website](https://shinyproxy.io/support/) for
support on the product and the [community forum](https://support.openanalytics.eu/c/shinyproxy/10) for
announcements. This rewrite is not affiliated with Open Analytics.
