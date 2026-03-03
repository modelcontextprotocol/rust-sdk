# MCP SDK Tier Audit: modelcontextprotocol/rust-sdk

**Date**: 2026-02-25
**Branch**: alexhancock/conformance
**Auditor**: mcp-sdk-tier-audit skill (automated + subagent evaluation)

## Tier Assessment: Tier 3

The Rust SDK (rmcp) is currently at Tier 3. While server and client conformance pass rates exceed the 80% Tier 2 threshold, several critical Tier 2 requirements are not met: issue triage compliance is very low (14.1%), required labels are largely missing (3/12), no stable release ≥1.0.0 exists, and no roadmap is published.

### Requirements Summary

| # | Requirement | Tier 1 Standard | Tier 2 Standard | Current Value | T1? | T2? | Gap |
|---|-------------|----------------|-----------------|---------------|-----|-----|-----|
| 1a | Server Conformance | 100% pass rate | >= 80% pass rate | 83.3% (25/30) | FAIL | PASS | 5 failing scenarios (prompts-get-with-args, prompts-get-embedded-resource, elicitation-sep1330-enums, elicitation-sep1034-defaults, dns-rebinding-protection) |
| 1b | Client Conformance | 100% pass rate | >= 80% pass rate | 85.0% (17/20) | FAIL | PASS | 3 failing date-versioned scenarios (scope-step-up, metadata-var3, 2025-03-26-oauth-endpoint-fallback) |
| 2 | Issue Triage | >= 90% within 2 biz days | >= 80% within 1 month | 14.1% (9/64) | FAIL | FAIL | 54 issues exceeding SLA; median 4341h |
| 2b | Labels | 12 required labels | 12 required labels | 3/12 | FAIL | FAIL | Missing: bug, enhancement, needs confirmation, needs repro, ready for work, P0, P1, P2, P3 |
| 3 | Critical Bug Resolution | All P0s within 7 days | All P0s within 2 weeks | 0 open | PASS | PASS | None |
| 4 | Stable Release | Required + clear versioning | At least one stable release | rmcp-v0.16.0 | FAIL | FAIL | No release >= 1.0.0 |
| 4b | Spec Tracking | Timeline agreed per release | Within 6 months | 6d gap (PASS) | PASS | PASS | None |
| 5 | Documentation | Comprehensive w/ examples | Basic docs for core features | ~8/48 features | FAIL | FAIL | Most features lack prose documentation |
| 6 | Dependency Policy | Published update policy | Published update policy | dependabot.yml configured | PASS | PASS | None |
| 7 | Roadmap | Published roadmap | Plan toward Tier 1 | Not found | FAIL | FAIL | No ROADMAP.md or docs/roadmap.md |
| 8 | Versioning Policy | Documented breaking change policy | N/A | Not found | FAIL | N/A | No VERSIONING.md or BREAKING_CHANGES.md |

### Tier Determination

- Tier 1: FAIL — 3/11 requirements met (failing: server_conformance, client_conformance, triage, labels, stable_release, documentation, roadmap, versioning)
- Tier 2: FAIL — 4/9 requirements met (failing: triage, labels, stable_release, documentation, roadmap)
- **Final Tier: 3**

---

## Server Conformance Details

Pass rate: 83.3% (25/30)

| Scenario | Status | Checks | Spec Versions |
|----------|--------|--------|---------------|
| server-tools-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-with-progress | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-with-logging | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-simple-text | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-sampling | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-mixed-content | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-image | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-error | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-embedded-resource | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-elicitation | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-audio | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-server-sse-multiple-streams | PASS | 2/2 | 2025-11-25 |
| server-server-initialize | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-unsubscribe | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-templates-read | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-subscribe | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-read-text | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-read-binary | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-with-image | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-with-args | FAIL | 0/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-simple | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-embedded-resource | FAIL | 0/1 | 2025-06-18, 2025-11-25 |
| server-ping | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-logging-set-level | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-elicitation-sep1330-enums | FAIL | 4/5 | 2025-11-25 |
| server-elicitation-sep1034-defaults | FAIL | 2/5 | 2025-11-25 |
| server-dns-rebinding-protection | FAIL | 1/2 | 2025-11-25 |
| server-completion-complete | PASS | 1/1 | 2025-06-18, 2025-11-25 |

---

## Client Conformance Details

Full suite pass rate: 85.0% (17/20 date-versioned)

> **Suite breakdown**: Core: 4/4 (100%), Auth (date-versioned): 13/16 (81.3%)

### Core Scenarios

| Scenario | Status | Checks | Spec Versions |
|----------|--------|--------|---------------|
| tools_call | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| sse-retry | PASS | 3/3 | 2025-11-25 |
| initialize | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| elicitation-sep1034-client-defaults | PASS | 5/5 | 2025-11-25 |

### Auth Scenarios (Date-Versioned)

| Scenario | Status | Checks | Spec Versions | Notes |
|----------|--------|--------|---------------|-------|
| auth/token-endpoint-auth-post | PASS | 19/19 | 2025-06-18, 2025-11-25 | |
| auth/token-endpoint-auth-none | PASS | 19/19 | 2025-06-18, 2025-11-25 | |
| auth/token-endpoint-auth-basic | PASS | 19/19 | 2025-06-18, 2025-11-25 | |
| auth/scope-step-up | FAIL | 13/14 | 2025-11-25 | |
| auth/scope-retry-limit | PASS | 11/11 | 2025-11-25 | |
| auth/scope-omitted-when-undefined | PASS | 15/15 | 2025-11-25 | |
| auth/scope-from-www-authenticate | PASS | 11/11 | 2025-11-25 | |
| auth/scope-from-scopes-supported | PASS | 15/15 | 2025-11-25 | |
| auth/pre-registration | PASS | 14/14 | 2025-11-25 | |
| auth/metadata-var3 | FAIL | 0/4 | 2025-11-25 | |
| auth/metadata-var2 | PASS | 14/14 | 2025-11-25 | |
| auth/metadata-var1 | PASS | 14/14 | 2025-11-25 | |
| auth/metadata-default | PASS | 14/14 | 2025-11-25 | |
| auth/basic-cimd | PASS | 14/14 | 2025-11-25 | |
| auth/2025-03-26-oauth-metadata-backcompat | PASS | 12/12 | 2025-03-26 | |
| auth/2025-03-26-oauth-endpoint-fallback | FAIL | 0/3 | 2025-03-26 | |

### Auth Scenarios (Informational — not scored)

| Scenario | Status | Checks | Spec Versions |
|----------|--------|--------|---------------|
| auth/resource-mismatch | FAIL | 14/15 | draft |
| auth/cross-app-access-complete-flow | FAIL | 10/12 | extension |
| auth/client-credentials-jwt | FAIL | 4/5 | extension |
| auth/client-credentials-basic | PASS | 9/9 | extension |

---

## Issue Triage Details

Analysis period: Last 64 issues
Labels present: question, good first issue, help wanted (3/12)
Uses issue types: No

| Metric | Value | T1 Req | T2 Req | Verdict |
|--------|-------|--------|--------|---------|
| Compliance rate | 14.1% | >= 90% | >= 80% | FAIL |
| Triaged within SLA | 9 | — | — | — |
| Exceeding SLA | 54 | — | — | — |
| Median triage time | 4341.3h | — | — | — |
| P95 triage time | 8095.1h | — | — | — |
| Open P0s | 0 | 0 | 0 | PASS |

---

## Documentation Coverage

### Documentation Coverage Assessment

**SDK path**: ~/Development/rust-sdk
**Documentation locations found**:

- README.md: Top-level overview, basic client/server setup
- crates/rmcp/README.md: Core library docs with quick start, transport options, feature flags, structured output, tasks
- examples/README.md: Quick start with Claude Desktop
- examples/servers/README.md: Server example descriptions
- examples/clients/README.md: Client example descriptions
- docs/OAUTH_SUPPORT.md: OAuth 2.1 authorization documentation
- crates/rmcp-macros/README.md: Macro crate documentation

#### Feature Documentation Table

| # | Feature | Documented? | Where | Has Examples? | Verdict |
|---|---------|-------------|-------|---------------|---------|
| 1 | Tools - listing | Yes | crates/rmcp/README.md:21-90 | Yes (1 example) | PASS |
| 2 | Tools - calling | Yes | crates/rmcp/README.md:21-90, examples/clients/README.md | Yes (2 examples) | PASS |
| 3 | Tools - text results | Yes | crates/rmcp/README.md:50-60 | Yes (1 example) | PASS |
| 4 | Tools - image results | No | — | No | FAIL |
| 5 | Tools - audio results | No | — | No | FAIL |
| 6 | Tools - embedded resources | No | — | No | FAIL |
| 7 | Tools - error handling | No | — | No | FAIL |
| 8 | Tools - change notifications | No | — | No | FAIL |
| 9 | Resources - listing | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 10 | Resources - reading text | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 11 | Resources - reading binary | No | — | No | FAIL |
| 12 | Resources - templates | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 13 | Resources - template reading | No | — | No | FAIL |
| 14 | Resources - subscribing | No | — | No | FAIL |
| 15 | Resources - unsubscribing | No | — | No | FAIL |
| 16 | Resources - change notifications | No | — | No | FAIL |
| 17 | Prompts - listing | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 18 | Prompts - getting simple | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 19 | Prompts - getting with arguments | No | — | Yes (example in everything_stdio.rs) | PARTIAL |
| 20 | Prompts - embedded resources | No | — | No | FAIL |
| 21 | Prompts - image content | No | — | No | FAIL |
| 22 | Prompts - change notifications | No | — | No | FAIL |
| 23 | Sampling - creating messages | No | — | Yes (servers/sampling_stdio.rs, clients/sampling_stdio.rs) | PARTIAL |
| 24 | Elicitation - form mode | Yes | examples/servers/README.md:38-53 | Yes (elicitation_stdio.rs) | PASS |
| 25 | Elicitation - URL mode | No | — | No | FAIL |
| 26 | Elicitation - schema validation | No | — | No | FAIL |
| 27 | Elicitation - default values | No | — | No | FAIL |
| 28 | Elicitation - enum values | No | — | Yes (elicitation_enum_inference.rs) | PARTIAL |
| 29 | Elicitation - complete notification | No | — | No | FAIL |
| 30 | Roots - listing | No | — | No | FAIL |
| 31 | Roots - change notifications | No | — | No | FAIL |
| 32 | Logging - sending log messages | No | — | No | FAIL |
| 33 | Logging - setting level | No | — | No | FAIL |
| 34 | Completions - resource argument | No | — | Yes (completion_stdio.rs) | PARTIAL |
| 35 | Completions - prompt argument | No | — | Yes (completion_stdio.rs) | PARTIAL |
| 36 | Ping | No | — | No | FAIL |
| 37 | Streamable HTTP transport (client) | Yes | crates/rmcp/README.md:175-195 | Yes (clients/streamable_http.rs) | PASS |
| 38 | Streamable HTTP transport (server) | Yes | crates/rmcp/README.md:175-195 | Yes (servers/counter_streamhttp.rs) | PASS |
| 39 | SSE transport - legacy (client) | No | — | No | FAIL |
| 40 | SSE transport - legacy (server) | No | — | No | FAIL |
| 41 | stdio transport (client) | Yes | crates/rmcp/README.md:140-165 | Yes (clients/git_stdio.rs) | PASS |
| 42 | stdio transport (server) | Yes | crates/rmcp/README.md:21-90 | Yes (servers/counter_stdio.rs) | PASS |
| 43 | Progress notifications | No | — | Yes (servers/progress_demo.rs, clients/progress_client.rs) | PARTIAL |
| 44 | Cancellation | No | — | No | FAIL |
| 45 | Pagination | No | — | No | FAIL |
| 46 | Capability negotiation | No | — | No | FAIL |
| 47 | Protocol version negotiation | No | — | No | FAIL |
| 48 | JSON Schema 2020-12 support | Yes | README.md:32-33, crates/rmcp/README.md:92-120 | Yes (structured output example) | PASS |
| — | Tasks - get (experimental) | Yes | crates/rmcp/README.md (Tasks section) | No | INFO |
| — | Tasks - result (experimental) | Yes | crates/rmcp/README.md (Tasks section) | No | INFO |
| — | Tasks - cancel (experimental) | Yes | crates/rmcp/README.md (Tasks section) | No | INFO |
| — | Tasks - list (experimental) | No | — | No | INFO |
| — | Tasks - status notifications (experimental) | No | — | No | INFO |

#### Summary

**Total non-experimental features**: 48
**PASS (documented with examples)**: 9/48
**PARTIAL (documented or examples only)**: 11/48
**FAIL (not documented)**: 28/48

**Core features documented**: ~6/36 (16.7%)
**All features documented with examples**: 9/48 (18.8%)

#### Tier Verdicts

**Tier 1** (all non-experimental features documented with examples): **FAIL**

- 39 features missing full documentation with examples

**Tier 2** (basic docs covering core features): **FAIL**

- Most core features (resources, prompts, sampling, roots, logging, completions, notifications, subscriptions) lack prose documentation
- Only tools (basic), transports (stdio, streamable HTTP), elicitation (form mode), and JSON Schema have adequate prose docs

---

## Policy Evaluation

### Policy Evaluation Assessment

**SDK path**: ~/Development/rust-sdk
**Repository**: modelcontextprotocol/rust-sdk

---

#### 1. Dependency Update Policy: PASS

| File | Exists (CLI) | Content Verdict |
|------|-------------|----------------|
| DEPENDENCY_POLICY.md | No | N/A |
| docs/dependency-policy.md | No | N/A |
| .github/dependabot.yml | Yes | Configured — weekly Cargo updates, daily GitHub Actions updates, with PR limits and labeling |
| .github/renovate.json | No | N/A |

**Verdict**: **PASS** — Dependabot is properly configured with weekly Cargo dependency updates and daily GitHub Actions updates.

---

#### 2. Roadmap: FAIL

| File | Exists (CLI) | Content Verdict |
|------|-------------|----------------|
| ROADMAP.md | No | N/A |
| docs/roadmap.md | No | N/A |

**Verdict**:

- **Tier 1**: **FAIL** — No roadmap file exists.
- **Tier 2**: **FAIL** — No roadmap or plan-toward-Tier-1 file exists.

---

#### 3. Versioning Policy: FAIL

| File | Exists (CLI) | Content Verdict |
|------|-------------|----------------|
| VERSIONING.md | No | N/A |
| docs/versioning.md | No | N/A |
| BREAKING_CHANGES.md | No | N/A |
| CONTRIBUTING.md (versioning section) | No | N/A |

**Verdict**:

- **Tier 1**: **FAIL** — No versioning or breaking change documentation exists.
- **Tier 2**: **N/A** — only requires stable release.

---

#### Overall Policy Summary

| Policy Area | Tier 1 | Tier 2 |
|-------------|--------|--------|
| Dependency Update Policy | PASS | PASS |
| Roadmap | FAIL | FAIL |
| Versioning Policy | FAIL | N/A |
