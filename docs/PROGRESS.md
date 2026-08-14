# Rewrite progress

ShinyProxy is being reimplemented in Rust. This file tracks the phases of the rewrite; the full plan lives
in the agent plan documents and is summarised in [ARCHITECTURE.md](ARCHITECTURE.md).

Legend: ⬜ not started · 🟨 in progress · ✅ done

| Phase | Title | Status | Notes |
| --- | --- | --- | --- |
| P0 | Foundations (workspace, toolchain, CI, fixture app) | ✅ | Rust 1.97.1 pinned, 4 crates, CI (fmt/clippy/test/build), `sp-testapp` fixture |
| P1 | Configuration subsystem (Spring-compatible) | ✅ | tree/schema/loader, typed settings, warnings, instance id (Jackson-compatible hash), 13 fixture configs, generated docs/CONFIGURATION.md |
| P2 | Domain model & spec provider | ✅ | runtime values (22 keys, Java flags), Proxy/Container with both JSON views, SpEL fields, ProxySpec/ContainerSpec with two-phase resolution, ShinyProxy notation → ProxySpec |
| P3 | `spel` expression engine | ⬜ | |
| P4 | HTTP shell, sessions, auth core, UI shell | ⬜ | |
| P5 | Proxy lifecycle engine + `local` backend | ⬜ | |
| P6 | Data plane (HTTP + WebSocket proxying, heartbeats) | ⬜ | |
| P7 | REST API parity | ⬜ | |
| P8 | Docker & Docker Swarm backends | ⬜ | |
| P9 | UI parity completion | ⬜ | |
| P10 | Operational features (logs, metrics, timeouts, stats) | ⬜ | |
| P11 | Authentication backends (OIDC, LDAP, SAML, ...) | ⬜ | |
| P12 | High availability (Redis), Kubernetes, ECS, proxy sharing | ⬜ | |
| P13 | Java decommission & packaging | ⬜ | |
| P14 | Validation & hardening | ⬜ | |

## Test inventory

| Suite | Tests | Notes |
| --- | --- | --- |
| `containerproxy` unit | 76 | config tree/schema/loader/settings/warnings, canonical YAML, identifiers |
| `containerproxy` golden | 2 | canonical YAML + SHA-1 vs Java reference output |
| `shinyproxy` config fixtures | 15 | 13 realistic configurations (docker, kubernetes, openid, ldap, saml, HA, parameters, template groups, usage stats, ecs, proxy sharing, api security) |
| `shinyproxy` docs/schema sync | 2 | generated CONFIGURATION.md + Java property inventory coverage |
| `shinyproxy` unit | 13 | schema lookups, generated docs, spec conversion |
| `shinyproxy` spec conversion | 3 | every fixture yields usable specs; docker/template-group details |
| `testapp` fixture contract | 5 | routes used by the integration tests |
| `spel` | 1 | placeholder until P3 |

## Ported Java test classes

Tracks the 13 Java integration test classes (see `src/test/java`) that must have a Rust counterpart by P7.

| Java test | Rust test | Status |
| --- | --- | --- |
| `IndexControllerTest` | | ⬜ |
| `AppControllerTest` | | ⬜ |
| `AppDirectControllerTest` | | ⬜ |
| `AdminControllerTest` | | ⬜ |
| `HeartbeatControllerTest` | | ⬜ |
| `IssueControllerTest` | | ⬜ |
| `ProxyApiControllerTest` | | ⬜ |
| `ProxyControllerTest` | | ⬜ |
| `ProxyStatusControllerTest` | | ⬜ |
| `DelegateProxyAdminControllerTest` | | ⬜ |
| `CleanHtmlTest` | | ⬜ |

## Toolchain decisions

* Pinned to Rust **1.97.1** (`rust-toolchain.toml`); the plan mentioned 1.90 but 1.97.1 is the current stable.
* `panic = "abort"` is **not** used in the release profile: a panic inside one request/task must not take the
  whole server down (Undertow/Spring behaves the same way).
