# Implementation waves

This folder tracks the rewrite in delivery waves. Phase-level detail stays in
[PROGRESS.md](../PROGRESS.md); deviations from Java ShinyProxy 3.2.4 stay in
[COMPATIBILITY.md](../COMPATIBILITY.md).

Legend: ⬜ not started · 🟨 in progress · ✅ done

| Wave | Scope | Status | Review |
| --- | --- | --- | --- |
| 1 | Core rewrite (P0–P9) | ✅ | [wave-1.md](wave-1.md) |
| 2 | Enterprise features, packaging, validation, first release (P10–P14) | ✅ | [wave-2.md](wave-2.md) |
| 3 | Close documented compatibility gaps | 🟨 | [wave-3.md](wave-3.md) |

Wave 1 delivered a working proxy with UI parity. Wave 2 made it an enterprise-capable
drop-in for most deployments and shipped `v0.1.0`. Wave 3 closes the remaining gaps
that Wave 2 deferred on purpose (SAML, S3 logs, InfluxDB stats, request dumping, and
related operational extras).
