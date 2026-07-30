# RMCP Roadmap

This roadmap tracks the path to [SEP-1730](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1730) Tier 1 for the Rust MCP SDK.

**Status (2026-07-30): all SEP-1730 Tier 1 requirements are met.** Conformance is
100% across every date-versioned suite, the stable **v3.0.1** release has shipped,
issue triage and critical-bug (P0) resolution are within the Tier 1 SLAs, the
governance documents (`VERSIONING.md`, `DEPENDENCY_POLICY.md`, this `ROADMAP.md`) are
published, and all 48 non-experimental features are documented with examples. This
document now serves as the ongoing tracker for spec conformance and SDK health.

| SEP-1730 Tier 1 requirement                         | Status | Evidence |
| --------------------------------------------------- | ------ | -------- |
| Server conformance 100% (date-versioned)            | ✅     | 30/30 — see below |
| Client conformance 100% (date-versioned)            | ✅     | 39/39 scored — see below |
| Issue triage ≥90% within 2 business days            | ✅     | 95.2% (20/21) |
| All P0 bugs resolved within 7 days                  | ✅     | 0 open; last P0 (#741) resolved in 3 days |
| Stable release ≥1.0.0 (no pre-release suffix)       | ✅     | `rmcp-v3.0.1` (see tooling note below) |
| Clear versioning + breaking-change policy           | ✅     | [`VERSIONING.md`](VERSIONING.md) |
| All non-experimental features documented w/ examples| ✅     | 48/48 in [`README.md`](README.md) |
| Published dependency update policy                  | ✅     | [`DEPENDENCY_POLICY.md`](DEPENDENCY_POLICY.md) + [`.github/dependabot.yml`](.github/dependabot.yml) |
| Published roadmap tracking spec components          | ✅     | this document |

| Suite (date-versioned) | Server        | Client        |
| ---------------------- | ------------- | ------------- |
| 2025-11-25             | 100% (30/30)  | 100%          |
| 2026-07-28             | 100% (30/30)  | 100%          |

Only date-versioned scenarios count toward SDK tiering. `draft` (2026-07-28 draft)
and `extension` scenarios are informational and reported separately below.

> **Tooling note — `stable_release`:** the SEP-1730 `tier-check` CLI may report
> `stable_release` as failing because it does not parse the workspace tag prefix
> `rmcp-v` (as in `rmcp-v3.0.1`). `rmcp-v3.0.1` is a genuine stable, non-pre-release
> release ([Releases](https://github.com/modelcontextprotocol/rust-sdk/releases)); the
> flag is a tooling artifact, not an unmet requirement.

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

## Completed

### Tier 1 requirements

- [x] **v3.0.0 stable released** (2026-07-28) — MRTR, SEP-2549 cache hints, SEP-2243
      standard headers, SEP-2575 stateless MCP, and SEP-2106 relaxations; **v3.0.1**
      is the current stable release
- [x] 2025-11-25 server conformance 100% (30/30)
- [x] 2025-11-25 client conformance 100%
- [x] 2026-07-28 server conformance 100% (30/30 dated)
- [x] 2026-07-28 client conformance 100% (dated)
- [x] Issue triage ≥90% within 2 business days (95.2%, 20/21) with the full SEP-1730
      label taxonomy (bug, enhancement, question, needs confirmation, needs repro,
      ready for work, good first issue, help wanted, P0–P3)
- [x] All P0 bugs resolved within 7 days (0 open; #741 resolved in 3 days). #815 was
      reclassified from `P0` to `T-security` — it was a CVE/advisory-coordination task
      for an already-shipped fix (PR #764, released in v1.4.0), not a critical-bug fix
- [x] `VERSIONING.md` — semver scheme, breaking-change definition, and communication policy
- [x] `DEPENDENCY_POLICY.md` — published dependency update policy (with `.github/dependabot.yml`)
- [x] `SECURITY.md` and Dependabot configuration
- [x] All 48 non-experimental features documented with examples in the README
      (closed the last 6 gaps on 2026-07-30: audio results, resource-template reading,
      resource-argument completion, ping, and the legacy HTTP+SSE non-goal writeup)

### Spec implementation

- [x] SEP-2322 MRTR (server scenarios + `sep-2322-client-request-state`)
- [x] SEP-2575 Make MCP Stateless (`server-stateless`)
- [x] SEP-2164 resource not found
- [x] SEP-2549 cache hints (`caching`)
- [x] SEP-2243 HTTP standardization (`http-header-validation`, standard headers)
- [x] DNS rebinding protection

---

## Nice-to-have (scorecard hygiene, not required for Tier 1)

- [ ] Add a top-level `CHANGELOG.md` (release notes are currently managed by release-plz).
- [ ] Add a top-level `CONTRIBUTING.md` (contributor docs currently live at `docs/CONTRIBUTE.MD`).
