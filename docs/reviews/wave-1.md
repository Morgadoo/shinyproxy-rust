# Wave 1 — Core rewrite

Status: ✅ complete

## Scope

Phases **P0–P9** of [PROGRESS.md](../PROGRESS.md):

| Phase | Title |
| --- | --- |
| P0 | Foundations (workspace, toolchain, CI, fixture app) |
| P1 | Configuration subsystem (Spring-compatible) |
| P2 | Domain model & spec provider |
| P3 | `spel` expression engine |
| P4 | HTTP shell, sessions, auth core, UI shell |
| P5 | Proxy lifecycle engine + `local` backend |
| P6 | Data plane (HTTP + WebSocket, heartbeats) |
| P7 | REST API parity |
| P8 | Docker & Docker Swarm backends |
| P9 | UI parity completion |

## Outcome

A single Rust binary that reads `application.yml`, serves the same routes and pages as
ShinyProxy 3.2.4 for the core product surface, and proxies apps through Docker / Swarm /
local backends. Landed in the rewrite PR that replaced the Java tree.

## Exit criteria (met)

- Workspace builds and tests under `cargo test --workspace` for self-contained suites
- Configuration, SpEL, lifecycle, data plane, API, and UI parity for the above phases
- Docker backend verified end to end (`SP_TEST_DOCKER=1`)
