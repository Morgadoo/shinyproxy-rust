# Rewrite progress

ShinyProxy is being reimplemented in Rust. This file tracks the phases of the rewrite; the full plan lives
in the agent plan documents and is summarised in [ARCHITECTURE.md](ARCHITECTURE.md).

Legend: ⬜ not started · 🟨 in progress · ✅ done

| Phase | Title | Status | Notes |
| --- | --- | --- | --- |
| P0 | Foundations (workspace, toolchain, CI, fixture app) | ✅ | Rust 1.97.1 pinned, 4 crates, CI (fmt/clippy/test/build), `sp-testapp` fixture |
| P1 | Configuration subsystem (Spring-compatible) | ✅ | tree/schema/loader, typed settings, warnings, instance id (Jackson-compatible hash), 13 fixture configs, generated docs/CONFIGURATION.md |
| P2 | Domain model & spec provider | ✅ | runtime values (22 keys, Java flags), Proxy/Container with both JSON views, SpEL fields, ProxySpec/ContainerSpec with two-phase resolution, ShinyProxy notation → ProxySpec |
| P3 | `spel` expression engine | ✅ | lexer/parser/evaluator + template splitting, engine-side context (proxy/spec/user objects) and SpecResolver; cross-validated against Spring (0 mismatches, 2 documented supersets) |
| P4 | HTTP shell, sessions, auth core, UI shell | ✅ | axum server (context path, sessions, security headers, authorization), none/simple auth with CSRF login, embedded assets, index/login/error pages ported from Thymeleaf, verified in a browser |
| P5 | Proxy lifecycle engine + `local` backend | ✅ | store, events, backend trait, port allocator, local backend, runtime values, ProxyService; wired into the server |
| P6 | Data plane (HTTP + WebSocket proxying, heartbeats) | ✅ | streaming HTTP forwarding, WebSocket tunnel with browser pings (Java semantics), iframe script injection, cache header modes, crash detection; app page + /app_proxy + heartbeat endpoints |
| P7 | REST API parity | ✅ | all documented endpoints (specs, proxies, status+watch, transfer, details, admin pages/data, issue reporting, delegate-proxy, app_direct, api/route) plus the OpenAPI document; the Java integration test classes are all covered |
| P8 | Docker & Docker Swarm backends | ✅ | bollard based `docker` and `docker-swarm` backends, app recovery with the startup page and the readiness gate; verified end to end against a real Docker daemon (see `crates/shinyproxy/tests/docker.rs`, `SP_TEST_DOCKER=1`) |
| P9 | UI parity completion | ✅ | parameters (validation, form, conversion, runtime values), admin pages, `/grafana/**`, my-apps modes, template groups, logos, notification message, body classes, hide-navbar and landing page; every page is locked down by an HTML snapshot |
| P10 | Operational features (logs, metrics, timeouts, stats) | ✅ | release timers (heartbeat timeout, max lifetime, logout), management server (health/readiness/recyclable/prometheus on `management.server.port`), Micrometer-compatible metrics, container log collection, `proxy.log-as-json` + `logging.*`, CSV and SQL usage statistics collectors. Documented gaps: S3 log storage, InfluxDB collector, `logging.requestdump`, attribute expressions |
| P11 | Authentication backends (OIDC, LDAP, SAML, ...) | 🟨 | `none`, `simple`, `custom-header`, `webservice`, `openid` (auth-code flow, PKCE, JWKS, token refresh, ms-graph groups), `ldap` (verified against a real OpenLDAP) and oauth2 bearer tokens; `saml` and `keycloak` fail at startup with explicit migration messages |
| P12 | High availability (Redis), Kubernetes, ECS, proxy sharing | 🟨 | Redis proxy/heartbeat/port stores, the leader election and the Kubernetes backend work (verified against a real Redis and a real k3s cluster); Redis sessions, `RedisCheckLatestConfigService`, the ECS backend and proxy sharing remain |
| P13 | Java decommission & packaging | ⬜ | |
| P14 | Validation & hardening | ⬜ | |

## Test inventory

447 tests pass with `cargo test --workspace`; the Docker (4), LDAP (3) and Redis (4) suites need their
service and are enabled with `SP_TEST_DOCKER=1`, `SP_TEST_LDAP=1` and `SP_TEST_REDIS=1`.

| Suite | Tests | Notes |
| --- | --- | --- |
| `containerproxy` unit | 246 | config tree/schema/loader/settings/warnings, canonical YAML, identifiers |
| `containerproxy` golden | 2 | canonical YAML + SHA-1 vs Java reference output |
| `containerproxy` dataplane (end to end) | 6 | streamed bodies, header forwarding, WebSocket + heartbeats, cache headers, injection, crashed app |
| `shinyproxy` kubernetes backend (end to end, `SP_TEST_K8S=1`) | 3 | pod and NodePort service contents, HTTP + WebSocket proxying, cleanup on stop, pod patches and additional/persistent manifests, app recovery of running pods |
| `shinyproxy` docker backend (end to end, `SP_TEST_DOCKER=1`) | 4 | container create request (labels, env, published ports), HTTP + WebSocket proxying, stop/cleanup, pause/resume, app recovery after a restart and the instanceId check |
| `shinyproxy` OpenID Connect (end to end) | 8 | the whole flow against a fake provider with real RS256 id tokens (redirects, code exchange, verification, user info, groups, access token in the app), a wrong state, roles claims of every shape, PKCE |
| `shinyproxy` Redis store (end to end, `SP_TEST_REDIS=1`) | 4 | two servers sharing apps (start on one, see and stop on the other), shared host port allocation from one range, shared heartbeats, realm isolation |
| `shinyproxy` LDAP (end to end, `SP_TEST_LDAP=1`) | 3 | a user DN pattern, a user search with the manager account, group based access and admin rights, wrong passwords, unknown users, an unreachable directory |
| `shinyproxy` authentication backends (end to end) | 4 | header based authentication (headers decide the user per request, no login page, API access, admin groups), the default header name, web service authentication against a fake service (groups from the answer, wrong credentials), and its startup validation |
| `shinyproxy` container logs (end to end) | 2 | files created with the Java names, the output of the app collected, the paths in an issue report, disabled without a path |
| `shinyproxy` actuator & metrics (end to end) | 4 | health/liveness/readiness, recyclable with an open WebSocket, the Prometheus output of a start/stop cycle, the management server |
| `shinyproxy` release timers (end to end) | 5 | inactive apps released while used/unused, disabled timeout, max lifetime, logout stopping (and `stop-on-logout` overrides) |
| `shinyproxy` HTML snapshots | 8 | login (plain and expired), index (user, admin, inline my-apps with template groups), app page (plain and with parameters + hidden navbar), admin (proxies and about), error pages |
| `shinyproxy` parameters (end to end) | 6 | the form for two kinds of users, validation of chosen values, values reaching the app and the API, preselection when resuming, a configuration provided form, startup validation |
| `shinyproxy` admin & api (end to end) | 9 | admin pages and assets, app transfer, custom app details, issue reporting validation, app_direct, api/route, delegate-proxy authorization |
| `shinyproxy` app flow (end to end) | 5 | start app → status watch → app page → proxied HTTP/WebSocket → heartbeat → admin data → stop; ownership and limits |
| `containerproxy` lifecycle (end to end) | 8 | real app processes: start/reachable/env vars/stop/cleanup, failed start, max instances, shutdown behaviour, events |
| `shinyproxy` config fixtures | 15 | 13 realistic configurations (docker, kubernetes, openid, ldap, saml, HA, parameters, template groups, usage stats, ecs, proxy sharing, api security) |
| `shinyproxy` docs/schema sync | 2 | generated CONFIGURATION.md + Java property inventory coverage |
| `shinyproxy` unit | 45 | schema lookups, generated docs, spec conversion, page model, state (access control, admin, max instances, logos) |
| `shinyproxy` ui (end to end) | 16 | login/logout/CSRF, index rendering, admin authorization, assets, security headers, context path, landing page, JSON 401 |
| `shinyproxy` spec conversion | 3 | every fixture yields usable specs; docker/template-group details |
| `testapp` fixture contract | 5 | routes used by the integration tests |
| `spel` | 40 | unit tests + 116 expression corpus cross-validated against Spring |
| `containerproxy` expression context | 5 | Java context names, runtime values, end-to-end spec resolution |

## Ported Java test classes

Tracks the 13 Java integration test classes (see `src/test/java`) that must have a Rust counterpart by P7.

| Java test | Rust test | Status |
| --- | --- | --- |
| `IndexControllerTest` | tests/ui.rs | ✅ |
| `AppControllerTest` | tests/apps.rs | ✅ |
| `AppDirectControllerTest` | tests/admin_and_api.rs | ✅ |
| `AdminControllerTest` | tests/admin_and_api.rs | ✅ |
| `HeartbeatControllerTest` | tests/apps.rs | ✅ |
| `IssueControllerTest` | tests/admin_and_api.rs | ✅ |
| `ProxyApiControllerTest` | tests/admin_and_api.rs | ✅ |
| `ProxyControllerTest` | tests/apps.rs | ✅ |
| `ProxyStatusControllerTest` | tests/apps.rs | ✅ |
| `DelegateProxyAdminControllerTest` | tests/admin_and_api.rs | ✅ |
| `CleanHtmlTest` | containerproxy util::clean_html | ✅ |

## Toolchain decisions

* Pinned to Rust **1.97.1** (`rust-toolchain.toml`); the plan mentioned 1.90 but 1.97.1 is the current stable.
* `panic = "abort"` is **not** used in the release profile: a panic inside one request/task must not take the
  whole server down (Undertow/Spring behaves the same way).
