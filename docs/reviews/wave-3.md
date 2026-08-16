# Wave 3 — Compatibility gap closure

Status: 🟨 in progress

Closes the gaps Wave 2 deferred. SAML remains optional (OpenID is the recommended path);
live AWS ECS validation is a runbook, not a CI gate, unless credentials are available.

## Tracker

| Phase | Work | Status |
| --- | --- | --- |
| 0 | `docs/reviews/` + COMPATIBILITY HA hygiene | ✅ |
| 1 | Usage-stats attribute expressions | ✅ |
| 2 | InfluxDB usage-stats collector | ✅ |
| 3 | S3 container-log storage | ⬜ |
| 4 | `logging.requestdump` | ⬜ |
| 5 | ECS `sp-to-delete` + background cleanup | ⬜ |
| 6 | SAML authentication | ⬜ deferred (use OpenID unless required) |
| 7 | Stretch config parity | ⬜ optional |

## Out of scope

- Re-opening completed P0–P14 work unless a regression appears
- Porting Keycloak auth (removed upstream; use OpenID)
- Breaking `application.yml` compatibility

## References

- [COMPATIBILITY.md](../COMPATIBILITY.md) — deviations and gap status
- [MIGRATION.md](../MIGRATION.md) — operator-facing unsupported features
- [PROGRESS.md](../PROGRESS.md) — phase table
