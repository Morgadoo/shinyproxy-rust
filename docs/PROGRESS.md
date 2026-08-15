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
| P12 | High availability (Redis), Kubernetes, ECS, proxy sharing | ✅ | Redis proxy/heartbeat/port stores, the leader election and the Kubernetes backend work (verified against a real Redis and a real k3s cluster); Redis sessions (`spring.session.store-type: redis`, shared across the servers of a realm) with the logged-in/active user gauges; the version check of rolling updates (`RedisCheckLatestConfigService`); and the pre-initialized, shared containers (memory seat store); the ECS backend (unit tested, needs validation against a real AWS account) and the Redis seat store |
| P13 | Java decommission & packaging | ✅ | the Java sources, the Maven build and the Java CI workflow are removed (the templates and assets live in `assets/`, the demo configuration in `examples/`), README and docs rewritten (`MIGRATION.md`, `TESTING.md`), the configuration reference states what is really implemented, `Dockerfile` + `.dockerignore` (124 MB image, verified serving the demo configuration), the binary is stamped with the commit and the build time, CI runs the Docker/Kubernetes/Redis/LDAP suites and builds the image, and `release.yml` publishes binaries with checksums and a multi-arch image |
| P14 | Validation & hardening | ✅ | cross validation against the Java jar (report in `generated/cross-validation.md`, four cosmetic differences left), the security review (route × role matrix and the classic weaknesses), robustness (property tests plus nasty URLs and bodies), the chaos checks, the load and soak runs (numbers in `COMPATIBILITY.md`) and the sign-off table below |

## Test inventory

526 tests pass with `cargo test --workspace`; the Docker (4), Kubernetes (3), LDAP (3) and Redis (7) suites
need their service and are enabled with `SP_TEST_DOCKER=1`, `SP_TEST_K8S=1`, `SP_TEST_LDAP=1` and
`SP_TEST_REDIS=1`. See [TESTING.md](TESTING.md).

| Suite | Tests | Notes |
| --- | --- | --- |
| `containerproxy` unit | 290 | config tree/schema/loader/settings/warnings, canonical YAML, identifiers |
| `containerproxy` golden | 2 | canonical YAML + SHA-1 vs Java reference output |
| `containerproxy` dataplane (end to end) | 6 | streamed bodies, header forwarding, WebSocket + heartbeats, cache headers, injection, crashed app |
| `shinyproxy` security review | 9 | the route × visitor matrix (anonymous, owner, other user, administrator), app actions of another user, header smuggling and CRLF, redirects that stay on this server, the CSRF token of the login form, the cookie and header settings, secrets never logged or exposed, and a session that stays alive while it is used |
| `shinyproxy` robustness | 6 | property tests of the app request parser, the WebSocket sniffer, the expression engine and the configuration binder, plus nasty URLs and broken bodies against a running server |
| `shinyproxy` chaos | 4 | an app killed during a WebSocket session, an app stopped while starting, a Redis that disappears, a shutdown with running apps |
| `shinyproxy` proxy sharing (end to end) | 7 | containers pre-started before anybody logs in, users claiming seats instantly, several users on one container, containers that may not be re-used, waiting and failing without a seat, removal through the admin endpoint, the seat metrics and the startup validations |
| `shinyproxy` kubernetes backend (end to end, `SP_TEST_K8S=1`) | 3 | pod and NodePort service contents, HTTP + WebSocket proxying, cleanup on stop, pod patches and additional/persistent manifests, app recovery of running pods |
| `shinyproxy` docker backend (end to end, `SP_TEST_DOCKER=1`) | 4 | container create request (labels, env, published ports), HTTP + WebSocket proxying, stop/cleanup, pause/resume, app recovery after a restart and the instanceId check |
| `shinyproxy` OpenID Connect (end to end) | 8 | the whole flow against a fake provider with real RS256 id tokens (redirects, code exchange, verification, user info, groups, access token in the app), a wrong state, roles claims of every shape, PKCE |
| `shinyproxy` Redis store (end to end, `SP_TEST_REDIS=1`) | 7 | two servers sharing apps (start on one, see and stop on the other), shared host port allocation from one range, shared heartbeats, the leader election, shared sessions (login on one server, use the app API on the other, the namespace of Spring Session, the user counts, sign out everywhere), a rolling update where the newest `proxy.version` takes the leadership, shared pre-initialized containers (the leader creates them, a user of the other server claims a seat and gives it back) |
| `shinyproxy` LDAP (end to end, `SP_TEST_LDAP=1`) | 3 | a user DN pattern, a user search with the manager account, group based access and admin rights, wrong passwords, unknown users, an unreachable directory |
| `shinyproxy` authentication backends (end to end) | 4 | header based authentication (headers decide the user per request, no login page, API access, admin groups), the default header name, web service authentication against a fake service (groups from the answer, wrong credentials), and its startup validation |
| `shinyproxy` container logs (end to end) | 2 | files created with the Java names, the output of the app collected, the paths in an issue report, disabled without a path |
| `shinyproxy` actuator & metrics (end to end) | 5 | health/liveness/readiness, recyclable with an open WebSocket, the Prometheus output of a start/stop cycle, the management server |
| `shinyproxy` release timers (end to end) | 5 | inactive apps released while used/unused, disabled timeout, max lifetime, logout stopping (and `stop-on-logout` overrides) |
| `shinyproxy` HTML snapshots | 8 | login (plain and expired), index (user, admin, inline my-apps with template groups), app page (plain and with parameters + hidden navbar), admin (proxies and about), error pages |
| `shinyproxy` parameters (end to end) | 6 | the form for two kinds of users, validation of chosen values, values reaching the app and the API, preselection when resuming, a configuration provided form, startup validation |
| `shinyproxy` admin & api (end to end) | 9 | admin pages and assets, app transfer, custom app details, issue reporting validation, app_direct, api/route, delegate-proxy authorization |
| `shinyproxy` app flow (end to end) | 5 | start app → status watch → app page → proxied HTTP/WebSocket → heartbeat → admin data → stop; ownership and limits |
| `containerproxy` lifecycle (end to end) | 8 | real app processes: start/reachable/env vars/stop/cleanup, failed start, max instances, shutdown behaviour, events |
| `shinyproxy` config fixtures | 16 | 13 realistic configurations (docker, kubernetes, openid, ldap, saml, HA, parameters, template groups, usage stats, ecs, proxy sharing, api security) |
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

## Sign-off: every Java source has a Rust counterpart

The Java implementation consisted of 37 files in ShinyProxy (`src/main/java/eu/openanalytics/shinyproxy`) and
248 files in the ContainerProxy engine it bundled. They map onto this repository as follows; a row that is not
ported says why.

### ShinyProxy

| Java | Rust |
| --- | --- |
| `AppRequestInfo` | `shinyproxy::web::apps::AppRequestInfo` |
| `AuthenticationRequiredFilter` | `shinyproxy::web::router::{needs_authentication_answer, authorize}` |
| `controllers/AdminController` | `shinyproxy::web::admin` |
| `controllers/AppController` | `shinyproxy::web::apps` (`/app/**`, `/app_i/**`, `/app_proxy/**`) |
| `controllers/AppDirectController` | `shinyproxy::web::apps::app_direct` |
| `controllers/BaseController` | `shinyproxy::web::model::prepare_model` |
| `controllers/DelegateProxyAdminController` | `shinyproxy::web::api::remove_delegate_proxies` |
| `controllers/HeartbeatController` | `shinyproxy::web::apps::{heartbeat, heartbeat_info}` |
| `controllers/IndexController` | `shinyproxy::web::router::index` |
| `controllers/IssueController` | `shinyproxy::web::issue` |
| `controllers/ProxyApiController` | `shinyproxy::web::api` |
| `controllers/dto/*` | the request and answer types next to their handlers (`shinyproxy::web::{api,issue}`) |
| `external/ExternalAppSpecExtension[Provider]` | `shinyproxy::spec_provider` (the `external` spec extension) |
| `monitoring/Monitoring{Controller,Service}` | `shinyproxy::web::monitoring` (`/grafana/**`) |
| `runtimevalues/*` (9 files) | `shinyproxy::runtime_values` |
| `ShinyProxyConfiguration` | `shinyproxy::config_schema` + `containerproxy::config` |
| `ShinyProxyIframeScriptInjector` | `containerproxy::dataplane::inject` |
| `ShinyProxySpecExtension[Provider]`, `ShinyProxySpecProvider` | `shinyproxy::spec_provider` |
| `ShinyProxyTestStrategy` | `containerproxy::service::proxy_service::wait_until_reachable` |
| `Thymeleaf` | `containerproxy::web::templates` (MiniJinja) |
| `UISecurityConfig` | `shinyproxy::web::router::authorize` + `containerproxy::web::security` |
| `UserAndAppNameAndInstanceNameProxyIndex` | `containerproxy::store` (the lookups of `ProxyStore`) |

### ContainerProxy

| Java package | Rust |
| --- | --- |
| `model/runtime/runtimevalues` (28) | `containerproxy::model::runtime_value` |
| `service` (21) | `containerproxy::service::{proxy_service,release,recovery,logs,leader,sessions,sharing,identifier,runtime_values,parameters}` |
| `util` (19) | `containerproxy::{util, web::security, dataplane}` |
| `event` (15) | `containerproxy::events` |
| `model/spec` (10) | `containerproxy::model::spec` |
| `model/runtime` (10) | `containerproxy::model::proxy` |
| `backend/dispatcher/proxysharing` (+ stores, 19) | `containerproxy::service::sharing` (memory and Redis stores) |
| `log` (8) | `containerproxy::service::logs` + `shinyproxy::logging` |
| `backend/kubernetes` (7) | `containerproxy::backend::kubernetes` |
| `auth/impl` (7) | `containerproxy::auth::{simple,none,ldap,openid,webservice,custom_header,bearer}` |
| `stat/impl` (6) | `containerproxy::stat::{collectors,prometheus}` |
| `security` (6) | `containerproxy::web::security` + `shinyproxy::web::router` |
| `ui` (5) | `shinyproxy::web::{router,admin,issue}` and the templates in `assets/` |
| `spec/expression` (5) | `spel` + `containerproxy::spec::expression` |
| `service/hearbeat` (5) | `containerproxy::dataplane::ws` (client pings) + `containerproxy::service::release` |
| `backend/ecs` (5) | `containerproxy::backend::ecs` |
| `api`, `api/dto` (8) | `shinyproxy::web::{api,openapi}` |
| `backend/strategy` (6) | `containerproxy::backend::ports` (the port allocation strategies) |
| `backend/docker` (3) | `containerproxy::backend::{docker,swarm}` |
| `backend/dispatcher` (3) | `containerproxy::service::proxy_service` (the dispatcher is chosen per app definition) |
| `service/session` (4) | `containerproxy::service::sessions` + `containerproxy::store::RedisSessionStore` |
| `service/leader` (4) | `containerproxy::service::leader` (including the version check of rolling updates) |
| `model/store` (6) | `containerproxy::store` (memory and Redis) |
| `stat` (2), `spec` (3), `backend` (2) | `containerproxy::{stat,spec,backend}` |
| `auth/impl/saml` (5) | **not ported**: SAML authentication; the server refuses to start with `authentication: saml` and points at `openid` |
| Spring plumbing (`ContainerProxyApplication`, `*Configuration`, `*AutoConfiguration`, ...) | not applicable: there is no dependency injection container; the wiring is `shinyproxy::web::AppState` |

The features inside these files that are deliberately not implemented (S3 log storage, the InfluxDB collector,
request dumping, ECS validated only by unit tests, ...) are listed in
[COMPATIBILITY.md](COMPATIBILITY.md).