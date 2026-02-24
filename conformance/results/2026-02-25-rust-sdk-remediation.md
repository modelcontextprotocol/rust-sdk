# Remediation Guide: modelcontextprotocol/rust-sdk

**Date**: 2026-02-25
**Current Tier**: 3

## Path to Tier 2

The following requirements must be met to advance from Tier 3 to Tier 2:

| # | Action | Requirement | Effort | Where |
|---|--------|-------------|--------|-------|
| 1 | Create 9 missing issue labels (bug, enhancement, needs confirmation, needs repro, ready for work, P0, P1, P2, P3) and triage existing issues | Labels (3/12 → 12/12) + Triage (14.1% → ≥80%) | Medium | GitHub repo settings, open issues |
| 2 | Publish stable release ≥ 1.0.0 | Stable Release | Medium | Cargo.toml, release process |
| 3 | Add prose documentation for core features: resources, prompts, sampling, roots, logging, completions, notifications, subscriptions | Documentation (basic docs for core features) | Large | README.md, docs/, crates/rmcp/README.md |
| 4 | Create ROADMAP.md with plan toward Tier 1 | Roadmap | Small | ROADMAP.md |

## Path to Tier 1

The following requirements must be met to advance to Tier 1 (includes all Tier 2 gaps):

| # | Action | Requirement | Effort | Where |
|---|--------|-------------|--------|-------|
| 1 | Fix 5 failing server conformance scenarios: prompts-get-with-args, prompts-get-embedded-resource, elicitation-sep1330-enums, elicitation-sep1034-defaults, dns-rebinding-protection | Server Conformance (83.3% → 100%) | Medium | Conformance server implementation |
| 2 | Fix 3 failing client conformance scenarios: auth/scope-step-up, auth/metadata-var3, auth/2025-03-26-oauth-endpoint-fallback | Client Conformance (85.0% → 100%) | Medium | OAuth client implementation |
| 3 | Create 9 missing issue labels and triage all open issues within 2 business days going forward | Labels + Triage (14.1% → ≥90%) | Medium | GitHub repo settings, issue triage process |
| 4 | Publish stable release ≥ 1.0.0 with clear versioning | Stable Release | Medium | Cargo.toml, release process |
| 5 | Document ALL 48 non-experimental features with prose and code examples | Documentation (9/48 → 48/48) | Large | README.md, docs/, crates/rmcp/README.md, examples/ |
| 6 | Create ROADMAP.md with concrete steps tracking MCP spec components | Roadmap | Small | ROADMAP.md |
| 7 | Create VERSIONING.md documenting breaking change policy and versioning scheme | Versioning Policy | Small | VERSIONING.md |

## Recommended Next Steps

1. **Set up issue labels and begin triage process** (Small effort, unblocks Tier 2 triage requirement). Create the 9 missing labels (bug, enhancement, needs confirmation, needs repro, ready for work, P0-P3) and begin labeling all new issues within 2 business days. Retroactively triage the 54 unlabeled issues.

2. **Create ROADMAP.md and VERSIONING.md** (Small effort, unblocks Tier 2 roadmap and Tier 1 versioning). Write a roadmap outlining the path to 1.0.0 and Tier 1, and document the versioning/breaking-change policy.

3. **Write prose documentation for core features** (Large effort, unblocks Tier 2 documentation). Priority features to document: resources (listing, reading, templates, subscriptions), prompts (listing, getting, arguments, embedded resources), sampling, roots, logging, completions, notifications, and change notifications. The SDK already has good examples in `examples/` — these need accompanying prose in `docs/` or `crates/rmcp/README.md`.

4. **Fix server conformance failures** (Medium effort, advances toward Tier 1). The 5 failures are in prompts-get-with-args, prompts-get-embedded-resource, elicitation-sep1330-enums, elicitation-sep1034-defaults, and dns-rebinding-protection. The elicitation failures appear to be in default value handling and enum validation; the prompts failures may be response format issues.

5. **Fix client auth conformance failures** (Medium effort, advances toward Tier 1). The 3 date-versioned failures are auth/scope-step-up (1 check failing), auth/metadata-var3 (all 4 checks failing — likely a metadata discovery edge case), and auth/2025-03-26-oauth-endpoint-fallback (all 3 checks failing — legacy endpoint fallback).

6. **Plan and execute 1.0.0 release** (Medium effort, unblocks Tier 2 stable release). The current version is 0.16.0. A 1.0.0 release signals production readiness and is required for both Tier 1 and Tier 2.
