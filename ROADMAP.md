# RMCP Roadmap

This roadmap tracks the path to [SEP-1730](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1730) Tier 1 for the Rust MCP SDK.

**Status (2026-07-29):** conformance is 100% across every date-versioned suite, and
the stable **v3.0.0** release has shipped. The remaining Tier 1 work is documentation
coverage and two governance documents.

| Suite (date-versioned) | Server        | Client        |
| ---------------------- | ------------- | ------------- |
| 2025-11-25             | 100% (30/30)  | 100%          |
| 2026-07-28             | 100% (30/30)  | 100%          |

Only date-versioned scenarios count toward SDK tiering. `draft` (2026-07-28 draft)
and `extension` scenarios are informational and reported separately below.

---

## Conformance

Per-scenario status for the current spec is tracked in the epic issue
[#977 — Tracking: 2026-07-28 spec conformance](https://github.com/modelcontextprotocol/rust-sdk/issues/977),
under the [`2026-07-28 spec` milestone](https://github.com/modelcontextprotocol/rust-sdk/milestone/3).

CI (`.github/workflows/conformance.yml`) runs the full `2025-11-25` and `2026-07-28`
server and client suites on every push and PR. Both suites are fully green.

### Informational (not scored for tiering)

Extension-tagged scenarios are excluded by `--spec-version` filters, so CI runs them
in separate steps against `conformance/expected-failures-extensions.yaml`:

| Scenario                                | Tag        | Status |
| --------------------------------------- | ---------- | ------ |
| `auth/client-credentials-basic`         | extension  | ✅ Pass |
| `auth/client-credentials-jwt`           | extension  | ✅ Pass |
| `auth/enterprise-managed-authorization` | extension  | ❌ Expected failure (not implemented by the conformance client) |
| `auth/wif-jwt-bearer`                   | 2026-07-28 draft | ❌ Expected failure (WIF / SEP-1933, draft) |
| `tasks-*` (SEP-2663)                    | extension  | ❌ 9 expected failures · ⏭️ 1 upstream-skipped |

### Spec features without conformance scenarios

Conformance does not cover the entire spec surface. Remaining feature work tracked via
the milestone:

- SEP-2567 sessionless MCP via explicit state handles (#870)
- SEP-2260 server requests must associate with a client request (#873)
- SEP-2549 follow-up: client-side TTL-honoring cache (#974)

---

## Tier 1 — remaining work

Conformance, stable release, labels, issue triage, and spec-tracking already meet the
Tier 1 bar. What's left:

### Documentation (Tier 1 requires all non-experimental features documented with examples)

The README now documents core primitives comprehensively with linked examples.

### Governance & Policy

- [ ] Add `VERSIONING.md` — document the semver scheme, what constitutes a breaking
      change, and how breaking changes are communicated (migration guides are linked
      from the README but the policy itself is not yet written down).
- [ ] Add `DEPENDENCY_POLICY.md` — a published dependency update policy (Dependabot is
      configured in `.github/dependabot.yml`, but Tier 1 requires a written, findable policy).
- [ ] Re-triage mislabeled `P0` issues — #869 / #871 / #872 are SEP *feature*
      implementation tasks, not critical bugs; they should not carry `P0`. Reserving
      `P0` for genuine critical bugs keeps the SEP-1730 critical-bug-resolution metric
      accurate.

### Nice-to-have (scorecard hygiene)

- [ ] Add a top-level `CHANGELOG.md` (release notes are currently managed by release-plz).
- [ ] Add a top-level `CONTRIBUTING.md` (contributor docs currently live at `docs/CONTRIBUTE.MD`).

---

## Completed

- [x] **v3.0.0 stable released** (2026-07-28) — MRTR, SEP-2549 cache hints, SEP-2243
      standard headers, SEP-2575 stateless MCP, and SEP-2106 relaxations
- [x] 2025-11-25 server conformance 100% (30/30)
- [x] 2025-11-25 client conformance 100%
- [x] 2026-07-28 server conformance 100% (30/30 dated)
- [x] 2026-07-28 client conformance 100% (dated)
- [x] SEP-2322 MRTR (server scenarios + `sep-2322-client-request-state`)
- [x] SEP-2575 Make MCP Stateless (`server-stateless`)
- [x] SEP-2164 resource not found
- [x] SEP-2549 cache hints (`caching`)
- [x] SEP-2243 HTTP standardization (`http-header-validation`, standard headers)
- [x] DNS rebinding protection
- [x] Full SEP-1730 issue-triage label taxonomy (bug, enhancement, question,
      needs confirmation, needs repro, ready for work, good first issue, help wanted, P0–P3)
- [x] `SECURITY.md` and Dependabot configuration
