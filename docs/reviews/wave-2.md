# Wave 2 — Enterprise features and first release

Status: ✅ complete

## Scope

Phases **P10–P14** of [PROGRESS.md](../PROGRESS.md), plus the first public release and the
GHCR image-tag fix:

| Phase | Title | Notes |
| --- | --- | --- |
| P10 | Operational features | Logs, metrics, timeouts, CSV/SQL usage stats |
| P11 | Authentication backends | All backends except SAML (deferred); Keycloak correctly refused |
| P12 | HA (Redis), Kubernetes, ECS, proxy sharing | Redis sessions, seats, version check included |
| P13 | Java decommission & packaging | Dockerfile, release workflow |
| P14 | Validation & hardening | Parity, security, chaos, soak |
| — | Release | `v0.1.0` pre-release; PR #3 lowercases GHCR tags |

## Acceptance checklist

- [x] Operational features (timers, management port, metrics, FS logs, CSV/SQL stats, JSON logging)
- [x] Auth backends except SAML (`none`, `simple`, `custom-header`, `webservice`, `openid`+refresh+ms-graph, `ldap`, bearer)
- [x] Redis HA (proxies, heartbeats, ports, sessions, seats, leader, version check)
- [x] Pre-initialized / shared containers
- [x] Kubernetes / Docker / Swarm verified; ECS unit-tested
- [x] Java sources removed; Dockerfile + release workflow
- [x] Parity suite, cross-validation, security/robustness/chaos, soak
- [x] First GitHub release + GHCR naming fix

## Deferred to Wave 3

These were documented as known gaps at ship time, not unfinished Wave 2 work:

- SAML authentication (startup refuse; migrate to OpenID)
- S3 container-log storage
- InfluxDB usage-statistics collector
- `logging.requestdump`
- Evaluation of `usage-stats-attributes[].expression`
- ECS `openanalytics.eu/sp-to-delete` tagging and background cleanup
- Live validation of the ECS backend against a real AWS account
- Optional Spring config extras (`SPRING_APPLICATION_JSON`, multi-document YAML, log rotation)

See [wave-3.md](wave-3.md).
