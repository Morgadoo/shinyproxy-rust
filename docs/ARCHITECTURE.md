# Architecture

ShinyProxy (Rust) is a single binary that replaces the Java stack (`shinyproxy.jar`, which bundled the
`containerproxy` engine and ran on Spring Boot + Undertow). The layering of the Java implementation is kept,
because it maps cleanly onto crates:

```
crates/
  spel/            SpEL-compatible expression engine (used for #{...} expressions in application.yml)
  containerproxy/  engine: configuration, domain model, container backends, authentication,
                   proxy lifecycle, reverse proxy data plane, REST API, stores, metrics
  shinyproxy/      binary: ShinyProxy configuration notation, controllers, templates, assets
  testapp/         test fixture app (HTTP + WebSocket + env dump) used by the integration tests
```

## Runtime stack

| Concern | Java | Rust |
| --- | --- | --- |
| HTTP server | Undertow + Spring MVC | `axum` on `hyper` |
| Reverse proxy | Undertow `ProxyHandler` + servlet dispatch | `hyper-util` pooled client + explicit forwarding |
| WebSockets | Undertow conduits (ping/pong injection) | `hyper` upgrade + bidirectional tunnel with frame sniffing |
| Templates | Thymeleaf | MiniJinja (runtime templates, needed for config-provided templates) |
| Configuration | Spring `Environment` / `@ConfigurationProperties` | custom Spring-compatible loader (YAML + env + CLI) |
| Sessions | Spring Session (memory/Redis) | `tower-sessions` (memory/Redis) |
| Container backends | docker-client, fabric8, AWS SDK | `bollard`, `kube`, `aws-sdk-*` |
| Metrics | Micrometer + Actuator | `metrics` + Prometheus exporter on the management port |
| Logging | log4j2/logback | `tracing` (+ JSON layer for `proxy.log-as-json`) |

## Compatibility

The rewrite is contract-preserving: configuration files, HTTP routes, JSON payloads, container environment
variables/labels and the browser assets are unchanged. Deviations are recorded in
[COMPATIBILITY.md](COMPATIBILITY.md) as they are introduced, and progress per phase is tracked in
[PROGRESS.md](PROGRESS.md).

## Development

```bash
cargo build --workspace          # build everything
cargo test --workspace           # self-contained test suite (no Docker required)
cargo run -p shinyproxy          # run the server
cargo run -p testapp -- --port 3838   # run the test fixture app standalone
./scripts/license-header.sh --fix     # add missing Apache-2.0 headers
```
