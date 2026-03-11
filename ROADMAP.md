# RMCP Roadmap

This roadmap tracks the path to SEP-1730 Tier 1 for the Rust MCP SDK.

Server conformance: 87.5% (28/32) · Client conformance: 80.0% (16/20)

---

## Tier 2 → Tier 1

### Conformance

#### Server (87.5% → 100%)

- [ ] Fix `prompts-get-with-args` — prompt argument handling returns incorrect result (arg1/arg2 not substituted)
- [ ] Fix `prompts-get-embedded-resource` — embedded resource content in prompt responses (invalid content union)
- [ ] Fix `elicitation-sep1330-enums` — enum inference handling per SEP-1330 (missing enumNames for legacy titled enum)
- [ ] Fix `dns-rebinding-protection` — validate `Host` / `Origin` headers on Streamable HTTP transport (accepts invalid headers with 200)

#### Client (80.0% → 100%)

- [ ] Fix `auth/metadata-var3` — AS metadata discovery variant 3 (no authorization support detected)
- [ ] Fix `auth/scope-from-www-authenticate` — use scope parameter from WWW-Authenticate header on 403 insufficient_scope
- [ ] Fix `auth/scope-step-up` — handle 403 `insufficient_scope` and re-authorize with upgraded scopes
- [ ] Fix `auth/2025-03-26-oauth-endpoint-fallback` — legacy OAuth endpoint fallback for pre-2025-06-18 servers (no authorization support detected)

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

## Informational (not scored for tiering)

These draft/extension scenarios are tracked but do not count toward tier advancement:

| Scenario | Tag | Status |
|---|---|---|
| `auth/resource-mismatch` | draft | ❌ Failed |
| `auth/client-credentials-jwt` | extension | ❌ Failed — JWT `aud` claim verification error |
| `auth/client-credentials-basic` | extension | ✅ Passed |
| `auth/cross-app-access-complete-flow` | extension | ❌ Failed — sends `authorization_code` grant instead of `jwt-bearer` |
