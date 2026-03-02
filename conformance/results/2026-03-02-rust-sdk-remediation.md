# Remediation Guide: modelcontextprotocol/rust-sdk

**Date**: 2026-03-02
**Current Tier**: 3

## Path to Tier 2

Only 2 requirements remain to reach Tier 2. Conformance, triage, labels, P0 resolution, spec tracking, documentation, and dependency policy all pass.

| # | Action | Requirement | Effort | Where |
|---|--------|-------------|--------|-------|
| 1 | Publish stable release >= 1.0.0 | Stable Release | Medium | Current version is rmcp-v0.17.0. Requires version bump to 1.0.0+ with no pre-release suffix. Review API stability, ensure public API is ready for semver commitment. Update `Cargo.toml` version fields in `crates/rmcp/` and `crates/rmcp-macros/`. |
| 2 | Create ROADMAP.md | Published plan toward Tier 1 | Small | Create `ROADMAP.md` in repo root with concrete work items tracking MCP spec components, timeline for 1.0 release, and plan for addressing remaining Tier 1 gaps. |

## Path to Tier 1

All Tier 2 gaps plus additional Tier 1 requirements.

| # | Action | Requirement | Effort | Where |
|---|--------|-------------|--------|-------|
| 1 | Publish stable release >= 1.0.0 | Stable Release + clear versioning | Medium | Same as Tier 2 item #1. |
| 2 | Create ROADMAP.md with concrete spec tracking | Published roadmap | Small | Create `ROADMAP.md` with concrete work items tracking MCP spec components, milestones, and timeline. Must be substantive, not just a placeholder. |
| 3 | Fix 4 failing server conformance scenarios | Server Conformance 100% | Medium | Currently 26/30 (86.7%). Fix: (a) `server-prompts-get-with-args` — prompt argument handling, (b) `server-prompts-get-embedded-resource` — embedded resource in prompt response, (c) `server-elicitation-sep1330-enums` — enum handling per SEP-1330, (d) `server-dns-rebinding-protection` — DNS rebinding protection for HTTP transport. |
| 4 | Fix 3 failing client auth conformance scenarios | Client Conformance 100% | Medium | Currently 18/24 unique (85.0%), 22/25 date-versioned (88.0%). Fix: (a) `auth/scope-step-up` (2025-11-25) — scope upgrade on 403 insufficient_scope, (b) `auth/metadata-var3` (2025-11-25) — AS metadata discovery variant, (c) `auth/2025-03-26-oauth-endpoint-fallback` (2025-03-26) — legacy OAuth endpoint fallback. |
| 5 | Create VERSIONING.md | Documented breaking change policy | Small | Create `VERSIONING.md` documenting: what constitutes a breaking change, how breaking changes are communicated, and the semver versioning scheme. |
| 6 | Document remaining 14 undocumented features | Comprehensive docs with examples | Medium | Features needing documentation: Tools (image results, audio results, embedded resources), Prompts (embedded resources, image content), Elicitation (URL mode, default values, complete notification), Ping, SSE legacy transport (client & server), Pagination, Protocol version negotiation. Add prose + code examples to `docs/FEATURES.md` or new doc files. |
| 7 | Upgrade 8 partially-documented features to PASS | Comprehensive docs with examples | Small | Features needing examples or prose: Tools error handling, Resources reading binary, Elicitation (form mode, schema validation, enum values), Capability negotiation, JSON Schema 2020-12 support. Add runnable examples or prose explanations. |

## Recommended Next Steps

1. **Create ROADMAP.md (quick win, Tier 2 blocker)**: Small effort, removes one of only two Tier 2 blockers. Document the plan for 1.0 release, conformance improvements, and spec tracking.

2. **Plan and publish 1.0 release (Tier 2 blocker)**: The SDK appears mature (v0.17.0 with comprehensive features). Evaluate API stability and plan the 1.0.0 release. This is the other Tier 2 blocker.

3. **Fix 4 server conformance failures (Tier 1)**: The server is at 86.7% — fixing the 4 failing scenarios would bring it to 100%.

4. **Fix 3 client auth failures (Tier 1)**: The client is at 85.0% — the 3 failing date-versioned scenarios are all in auth edge cases (`scope-step-up`, `metadata-var3`, `2025-03-26-oauth-endpoint-fallback`).

5. **Create VERSIONING.md (Tier 1)**: Document breaking change policy, versioning scheme, and communication strategy. Small effort.

6. **Expand documentation (Tier 1)**: Add prose documentation and examples for the 14 undocumented features and upgrade 8 partially-documented features.
