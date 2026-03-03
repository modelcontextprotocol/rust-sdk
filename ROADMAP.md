# RMCP Roadmap

This roadmap tracks the path to SEP-1730 Tier 1 for the Rust MCP SDK.

Server conformance: 86.7% (26/30) · Client conformance: 85.0% (18/24) · Spec tracking gap: 6 days

---

## Tier 2 → Tier 1

### Conformance

#### Server (86.7% → 100%)

- [ ] Fix `server-prompts-get-with-args` — prompt argument handling returns incorrect result
- [ ] Fix `server-prompts-get-embedded-resource` — embedded resource content in prompt responses
- [ ] Fix `server-elicitation-sep1330-enums` — enum inference handling per SEP-1330
- [ ] Fix `server-dns-rebinding-protection` — validate `Host` / `Origin` headers on Streamable HTTP transport

#### Client (85.0% → 100%)

- [ ] Fix `auth/scope-step-up` (2025-11-25) — handle 403 `insufficient_scope` and re-authorize with upgraded scopes
- [ ] Fix `auth/metadata-var3` (2025-11-25) — AS metadata discovery variant 3
- [ ] Fix `auth/2025-03-26-oauth-endpoint-fallback` (2025-03-26) — legacy OAuth endpoint fallback for pre-2025-06-18 servers

### Governance & Policy

- [ ] Create `VERSIONING.md` — document semver scheme, what constitutes a breaking change, and how breaking changes are communicated

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

## Informational (not scored)

These draft/extension scenarios are tracked but do not block tier advancement:

- [ ] `auth/resource-mismatch` (draft)
- [ ] `auth/cross-app-access-complete-flow` (extension)
- [ ] `auth/client-credentials-jwt` (extension)
