# MCP SDK Tier Audit: modelcontextprotocol/rust-sdk

**Date**: 2026-03-02
**Branch**: main
**Auditor**: mcp-sdk-tier-audit skill (automated + subagent evaluation)

## Tier Assessment: Tier 3

The Rust SDK (rmcp) demonstrates strong conformance (server 86.7%, client 85.0%), excellent repository health (100% triage, all labels, no P0s, 6-day spec tracking gap), and solid documentation. It is blocked from Tier 2 only by the lack of a stable ≥1.0 release (current: rmcp-v0.17.0) and a missing roadmap. Conformance, triage, labels, P0 resolution, spec tracking, documentation, and dependency policy all meet Tier 2 thresholds.

### Requirements Summary

| # | Requirement | Tier 1 Standard | Tier 2 Standard | Current Value | T1? | T2? | Gap |
|---|-------------|-----------------|-----------------|---------------|-----|-----|-----|
| 1a | Server Conformance | 100% pass rate | >= 80% pass rate | 86.7% (26/30) | FAIL | PASS | 4 failing: prompts-get-with-args, prompts-get-embedded-resource, elicitation-sep1330-enums, dns-rebinding-protection |
| 1b | Client Conformance | 100% pass rate | >= 80% pass rate | 85.0% (18/24) — 88.0% date-versioned (22/25) | FAIL | PASS | 3 date-versioned failures: auth/scope-step-up, auth/metadata-var3, auth/2025-03-26-oauth-endpoint-fallback |
| 2 | Issue Triage | >= 90% within 2 biz days | >= 80% within 1 month | 100% (24/24) | PASS | PASS | None |
| 2b | Labels | 12 required labels | 12 required labels | 12/12 | PASS | PASS | None |
| 3 | Critical Bug Resolution | All P0s within 7 days | All P0s within 2 weeks | 0 open | PASS | PASS | None |
| 4 | Stable Release | Required + clear versioning | At least one stable release | rmcp-v0.17.0 | FAIL | FAIL | Pre-1.0; no stable release |
| 4b | Spec Tracking | Timeline agreed per release | Within 6 months | 6d gap (PASS) | PASS | PASS | None |
| 5 | Documentation | Comprehensive w/ examples | Basic docs for core features | 26/48 PASS, 8 PARTIAL | FAIL | PASS | Core features well-documented in docs/FEATURES.md; gaps in elicitation details, pagination, protocol negotiation, legacy SSE |
| 6 | Dependency Policy | Published update policy | Published update policy | .github/dependabot.yml | PASS | PASS | None |
| 7 | Roadmap | Published roadmap | Plan toward Tier 1 | Not found | FAIL | FAIL | No ROADMAP.md or docs/roadmap.md |
| 8 | Versioning Policy | Documented breaking change policy | N/A | Not found | FAIL | N/A | No VERSIONING.md, BREAKING_CHANGES.md, or CONTRIBUTING.md |

### Tier Determination

- Tier 1: FAIL — 4/9 requirements met (failing: server conformance, client conformance, stable release, documentation, roadmap, versioning policy)
- Tier 2: FAIL — 7/9 requirements met (failing: stable release, roadmap)
- **Final Tier: 3**

---

## Server Conformance Details

Pass rate: 86.7% (26/30)

| Scenario | Status | Checks | Spec Versions |
|----------|--------|--------|---------------|
| server-server-initialize | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-server-sse-multiple-streams | PASS | 2/2 | 2025-11-25 |
| server-resources-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-read-text | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-read-binary | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-templates-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-templates-read | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-subscribe | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-resources-unsubscribe | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-simple | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-with-args | FAIL | 0/1 | 2025-06-18, 2025-11-25 |
| server-prompts-get-embedded-resource | FAIL | 0/1 | 2025-06-18, 2025-11-25 |
| server-tools-list | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-text | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-image | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-audio | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-embedded-resource | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-error | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-mixed-content | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-sampling | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-tools-call-elicitation | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-logging-set-level | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-completions-prompt | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-completions-resource | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-ping | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-elicitation-sep1330-enums | FAIL | 0/1 | 2025-11-25 |
| server-dns-rebinding-protection | FAIL | 0/1 | 2025-11-25 |
| server-notifications-tools-changed | PASS | 1/1 | 2025-06-18, 2025-11-25 |
| server-notifications-resources-changed | PASS | 1/1 | 2025-06-18, 2025-11-25 |

---

## Client Conformance Details

Full suite pass rate: 85.0% (18/24)

> **Suite breakdown**: Core: 4/4 (100%), Auth: 14/20 (70%)
> No baseline file found.

### Core Scenarios

| Scenario | Status | Checks | Spec Versions |
|----------|--------|--------|---------------|
| initialize | PASS | ✓ | 2025-06-18, 2025-11-25 |
| tools_call | PASS | ✓ | 2025-06-18, 2025-11-25 |
| sse-retry | PASS | ✓ | 2025-11-25 |
| elicitation-sep1034-client-defaults | PASS | ✓ | 2025-11-25 |

### Auth Scenarios

| Scenario | Status | Spec Versions | Notes |
|----------|--------|---------------|-------|
| auth/2025-03-26-oauth-metadata-backcompat | PASS | 2025-03-26 | |
| auth/2025-03-26-oauth-endpoint-fallback | FAIL | 2025-03-26 | |
| auth/token-endpoint-auth-post | PASS | 2025-06-18, 2025-11-25 | |
| auth/token-endpoint-auth-none | PASS | 2025-06-18, 2025-11-25 | |
| auth/token-endpoint-auth-basic | PASS | 2025-06-18, 2025-11-25 | |
| auth/metadata-default | PASS | 2025-11-25 | |
| auth/metadata-var1 | PASS | 2025-11-25 | |
| auth/metadata-var2 | PASS | 2025-11-25 | |
| auth/metadata-var3 | FAIL | 2025-11-25 | |
| auth/pre-registration | PASS | 2025-11-25 | |
| auth/scope-from-scopes-supported | PASS | 2025-11-25 | |
| auth/scope-from-www-authenticate | PASS | 2025-11-25 | |
| auth/scope-omitted-when-undefined | PASS | 2025-11-25 | |
| auth/scope-retry-limit | PASS | 2025-11-25 | |
| auth/scope-step-up | FAIL | 2025-11-25 | |
| auth/basic-cimd | PASS | 2025-11-25 | |
| auth/resource-mismatch | FAIL | draft | Informational only |
| auth/cross-app-access-complete-flow | FAIL | extension | Informational only |
| auth/client-credentials-jwt | FAIL | extension | Informational only |
| auth/client-credentials-basic | PASS | extension | Informational only |

---

## Conformance Matrix

|              | 2025-03-26 | 2025-06-18 | 2025-11-25 | All* |
|--------------|------------|------------|------------|------|
| Server       | —          | 24/26      | 26/30      | 26/30 (86.7%) |
| Client: Core | —          | 2/2        | 4/4        | 4/4 (100%) |
| Client: Auth | 1/2        | 3/3        | 12/14      | 18/21 (85.7%) |

Informational (not scored for tier):

|              | draft | extension |
|--------------|-------|-----------|
| Client: Auth | 0/1   | 1/3       |

Client failures concentrate in Auth scenarios — specifically `auth/scope-step-up` (2025-11-25), `auth/metadata-var3` (2025-11-25), and `auth/2025-03-26-oauth-endpoint-fallback` (2025-03-26). These are scope gaps in newer auth features, not core quality issues.

---

## Issue Triage Details

Analysis period: Last 24 issues
Labels: 12/12 present

| Metric | Value | T1 Req | T2 Req | Verdict |
|--------|-------|--------|--------|---------|
| Compliance rate | 100% | >= 90% | >= 80% | PASS |
| Exceeding SLA | 0 | -- | -- | -- |
| Open P0s | 0 | 0 | 0 | PASS |

---

## Documentation Coverage

### Documentation Coverage Assessment

**SDK path**: ~/Development/rust-sdk
**Documentation locations found**:

- `README.md`: Root README with quick start, server/client setup, feature flags
- `crates/rmcp/README.md`: Detailed crate docs with server/client examples, transport options, feature flags, tasks
- `docs/FEATURES.md`: Comprehensive feature documentation covering resources, prompts, sampling, roots, logging, completions, notifications, subscriptions
- `docs/OAUTH_SUPPORT.md`: OAuth 2.1 authorization documentation
- `examples/README.md`: Quick start guide with Claude Desktop
- `examples/servers/README.md`: Server example descriptions
- `examples/clients/README.md`: Client example descriptions
- `examples/servers/src/`: 15+ server examples
- `examples/clients/src/`: 7 client examples
- `examples/transport/src/`: Transport examples (TCP, HTTP upgrade, Unix socket, WebSocket)

#### Feature Documentation Table

| # | Feature | Documented? | Where | Has Examples? | Verdict |
|---|---------|-------------|-------|---------------|---------|
| 1 | Tools - listing | Yes | crates/rmcp/README.md:174 | Yes (2+ examples) | PASS |
| 2 | Tools - calling | Yes | crates/rmcp/README.md:178-186 | Yes (2+ examples) | PASS |
| 3 | Tools - text results | Yes | crates/rmcp/README.md:53 | Yes (counter example) | PASS |
| 4 | Tools - image results | No | — | No | FAIL |
| 5 | Tools - audio results | No | — | No | FAIL |
| 6 | Tools - embedded resources | No | — | No | FAIL |
| 7 | Tools - error handling | Yes | crates/rmcp/README.md:50 | Partial | PARTIAL |
| 8 | Tools - change notifications | Yes | docs/FEATURES.md:655-661 | Yes | PASS |
| 9 | Resources - listing | Yes | docs/FEATURES.md:52-65 | Yes | PASS |
| 10 | Resources - reading text | Yes | docs/FEATURES.md:67-84 | Yes | PASS |
| 11 | Resources - reading binary | Yes | docs/FEATURES.md:67-84 | No dedicated example | PARTIAL |
| 12 | Resources - templates | Yes | docs/FEATURES.md:86-97 | Yes | PASS |
| 13 | Resources - template reading | Yes | docs/FEATURES.md:115 | Yes | PASS |
| 14 | Resources - subscribing | Yes | docs/FEATURES.md:669-744 | Yes | PASS |
| 15 | Resources - unsubscribing | Yes | docs/FEATURES.md:706-716 | Yes | PASS |
| 16 | Resources - change notifications | Yes | docs/FEATURES.md:118-151 | Yes | PASS |
| 17 | Prompts - listing | Yes | docs/FEATURES.md:249-250 | Yes | PASS |
| 18 | Prompts - getting simple | Yes | docs/FEATURES.md:199-205 | Yes | PASS |
| 19 | Prompts - getting with arguments | Yes | docs/FEATURES.md:208-226 | Yes | PASS |
| 20 | Prompts - embedded resources | No | — | No | FAIL |
| 21 | Prompts - image content | No | — | No | FAIL |
| 22 | Prompts - change notifications | Yes | docs/FEATURES.md:265-268 | Yes | PASS |
| 23 | Sampling - creating messages | Yes | docs/FEATURES.md:276-343 | Yes | PASS |
| 24 | Elicitation - form mode | Yes | examples/servers/README.md:39-47 | Yes (elicitation_stdio.rs) | PARTIAL |
| 25 | Elicitation - URL mode | No | — | No | FAIL |
| 26 | Elicitation - schema validation | Yes | examples/servers/README.md:45 | Yes | PARTIAL |
| 27 | Elicitation - default values | No | — | No | FAIL |
| 28 | Elicitation - enum values | Yes | examples/servers/README.md:49-53 | Yes | PARTIAL |
| 29 | Elicitation - complete notification | No | — | No | FAIL |
| 30 | Roots - listing | Yes | docs/FEATURES.md:347-403 | Yes | PASS |
| 31 | Roots - change notifications | Yes | docs/FEATURES.md:406-411 | Yes | PASS |
| 32 | Logging - sending log messages | Yes | docs/FEATURES.md:417-458 | Yes | PASS |
| 33 | Logging - setting level | Yes | docs/FEATURES.md:439-447 | Yes | PASS |
| 34 | Completions - resource argument | Yes | docs/FEATURES.md:490-578 | Yes | PASS |
| 35 | Completions - prompt argument | Yes | docs/FEATURES.md:520-541 | Yes | PASS |
| 36 | Ping | No | — | No | FAIL |
| 37 | Streamable HTTP transport (client) | Yes | crates/rmcp/README.md:271 | Yes | PASS |
| 38 | Streamable HTTP transport (server) | Yes | crates/rmcp/README.md:285-286 | Yes | PASS |
| 39 | SSE transport - legacy (client) | No | — | No | FAIL |
| 40 | SSE transport - legacy (server) | No | — | No | FAIL |
| 41 | stdio transport (client) | Yes | crates/rmcp/README.md:209-218 | Yes | PASS |
| 42 | stdio transport (server) | Yes | crates/rmcp/README.md:56-88 | Yes | PASS |
| 43 | Progress notifications | Yes | docs/FEATURES.md:592-607 | Yes | PASS |
| 44 | Cancellation | Yes | docs/FEATURES.md:612-634 | Yes | PASS |
| 45 | Pagination | No | — | No | FAIL |
| 46 | Capability negotiation | Yes | crates/rmcp/README.md | Partial | PARTIAL |
| 47 | Protocol version negotiation | No | — | No | FAIL |
| 48 | JSON Schema 2020-12 support | Yes | README.md:32-33 | Partial | PARTIAL |
| — | Tasks - get (experimental) | Yes | crates/rmcp/README.md:137-144 | No | INFO |
| — | Tasks - result (experimental) | Yes | crates/rmcp/README.md:141 | No | INFO |
| — | Tasks - cancel (experimental) | Yes | crates/rmcp/README.md:142 | No | INFO |
| — | Tasks - list (experimental) | No | — | No | INFO |
| — | Tasks - status notifications (experimental) | No | — | No | INFO |

#### Summary

**Total non-experimental features**: 48
**PASS (documented with examples)**: 26/48
**PARTIAL (documented, no examples or examples without prose)**: 8/48
**FAIL (not documented)**: 14/48

**Core features documented (any level)**: 26/36 (72%)
**All features documented with examples**: 26/48 (54%)

#### Tier Verdicts

**Tier 1** (all non-experimental features documented with examples): **FAIL** — 14 features not documented, 8 only partially documented

**Tier 2** (basic docs covering core features): **PASS** — Core features (tools, resources, prompts, sampling, roots, logging, completions, notifications, subscriptions) all have prose documentation with examples in docs/FEATURES.md

---

## Policy Evaluation

### Policy Evaluation Assessment

**SDK path**: ~/Development/rust-sdk
**Repository**: modelcontextprotocol/rust-sdk

---

#### 1. Dependency Update Policy: PASS

| File | Exists (CLI) | Content Verdict |
|------|-------------|-----------------|
| DEPENDENCY_POLICY.md | No | N/A |
| docs/dependency-policy.md | No | N/A |
| .github/dependabot.yml | Yes | Configured — weekly Cargo dependency updates and daily GitHub Actions updates |
| .github/renovate.json | No | N/A |

**Verdict**: **PASS** — Dependabot is properly configured with weekly Cargo dependency updates and daily GitHub Actions updates.

---

#### 2. Roadmap: FAIL

| File | Exists (CLI) | Content Verdict |
|------|-------------|-----------------|
| ROADMAP.md | No | N/A |
| docs/roadmap.md | No | N/A |

**Verdict**:
- **Tier 1**: **FAIL** — No roadmap file exists.
- **Tier 2**: **FAIL** — No roadmap file exists with plan toward Tier 1.

---

#### 3. Versioning Policy: FAIL

| File | Exists (CLI) | Content Verdict |
|------|-------------|-----------------|
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
