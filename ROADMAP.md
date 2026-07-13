# RMCP Roadmap

This roadmap tracks the path to SEP-1730 Tier 1 for the Rust MCP SDK.

Spec 2025-11-25 (suite 0.1.16): Server 100% (30/30) · Client 100% (18/18)
Spec 2026-07-28 (suite 0.2.0-alpha.9): Server 92.5% (37/40) · Client 75.0% (24/32)

---

## Target spec: 2026-07-28 (release 2026-07-28)

All 2026-07-28 work carries the `2026-07-28` label and the
[`2026-07-28 spec` milestone](https://github.com/modelcontextprotocol/rust-sdk/milestone/3).
Per-scenario conformance status is tracked in the epic issue:
[#977 — Tracking: 2026-07-28 spec conformance](https://github.com/modelcontextprotocol/rust-sdk/issues/977).

### Conformance (baseline 2026-07-13, suite `0.2.0-alpha.9`)

- Server: 3 scenarios (`tools-call-with-progress` stateless behavior, SEP-2243 server-side custom headers, and `server-stateless` — the SEP-2575 discovery/negotiation suite at 2/28 checks)
- Client: 8 scenarios (SEP-2243 headers ×3, `request-metadata`, and 4 single-check auth failures: SEP-2350 step-up, pre-registration, SEP-2352 AS migration, SEP-2468 issuer validation); fixes for SEP-2350 (#888) and SEP-2352 (#965) are already in review
- CI: run the full `--spec-version 2026-07-28` suites (stateless server) instead of hand-picked scenario lists; re-baseline on each draft-suite bump

### Spec features without conformance scenarios

Conformance alone does not cover the full spec surface. Feature work tracked via the milestone:

- SEP-2567 sessionless MCP via explicit state handles (#870)
- SEP-2260 server requests must associate with a client request (#873)
- SEP-2549 follow-up: client-side TTL-honoring cache (#974)

(SEP-2575 discovery & negotiation is covered by the `server-stateless` conformance scenario;
implementation is in review — #869, PRs #973, #943.)

### Release

The 2026-07-28 implementation ships as **v3.0.0** (release PR #964): MRTR, SEP-2549 cache hints,
SEP-2243 standard headers, and the SEP-2106 relaxations are merged but unreleased — tiering and
relegation are evaluated against the latest stable release, so cutting v3.0.0 with the remaining
conformance fixes is on the critical path. Migration guide (draft, kept current until release):
[discussion #969](https://github.com/modelcontextprotocol/rust-sdk/discussions/969).

---

## Tier 1 (non-conformance requirements)

### Governance & Policy

- [ ] Create `VERSIONING.md` — document semver scheme, what constitutes a breaking change, and how breaking changes are communicated
- [ ] Publish a dependency update policy (Tier 1 requires a published policy)
- [ ] Cut v3.0.0 (#964) including all conformance fixes (tier relegation is evaluated against the latest stable release)

### Documentation (26/48 → 48/48 features with prose + examples)

#### Undocumented features (14)

- [ ] Tools — image results
- [ ] Tools — audio results
- [ ] Tools — embedded resources
- [ ] Prompts — embedded resources
- [ ] Prompts — image content
- [ ] Elicitation — URL mode
- [ ] Elicitation — default values
- [ ] Elicitation — complete notification
- [ ] Ping
- [ ] SSE transport — legacy (client)
- [ ] SSE transport — legacy (server)
- [ ] Pagination
- [ ] Protocol version negotiation
- [ ] JSON Schema 2020-12 support *(upgrade from partial)*

#### Partially documented features (7)

- [ ] Tools — error handling *(add dedicated prose + example)*
- [ ] Resources — reading binary *(add dedicated example)*
- [ ] Elicitation — form mode *(add prose docs, not just example README)*
- [ ] Elicitation — schema validation *(add prose docs)*
- [ ] Elicitation — enum values *(add prose docs)*
- [ ] Capability negotiation *(add dedicated prose explaining the builder API)*
- [ ] Protocol version negotiation *(document version negotiation behavior)*

---

## Completed

- [x] 2025-11-25 server conformance 100% (30 scenarios + pending `json-schema-2020-12`, `server-sse-polling`)
- [x] 2025-11-25 client conformance 100% (18 scenarios + legacy `auth/2025-03-26-*`)
- [x] SEP-2322 MRTR (14 server scenarios + `sep-2322-client-request-state`)
- [x] SEP-2164 resource not found
- [x] Cache hints (`caching`)
- [x] `http-header-validation`
- [x] Issue triage labels (bug, enhancement, needs confirmation, needs repro, ready for work, P0–P3)

---

## Informational (not scored for tiering)

These extension scenarios are tracked but do not count toward tier advancement:

| Scenario | Tag | Status |
|---|---|---|
| `auth/client-credentials-jwt` | extension | ❌ Failed — JWT `aud` claim verification error |
| `auth/client-credentials-basic` | extension | ✅ Passed |
| `auth/cross-app-access-complete-flow` | extension | ❌ Failed — sends `authorization_code` grant instead of `jwt-bearer` |
| `tasks-*` | extension | Not yet attempted |
