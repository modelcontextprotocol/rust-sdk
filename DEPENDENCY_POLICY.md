# Dependency Policy

This document describes how the Rust MCP SDK (`rmcp`, `rmcp-macros`) selects,
updates, and maintains its dependencies. It applies to all crates in the workspace.

## Goals

- Keep dependencies current and secure without churn that destabilizes the public API.
- Minimize the dependency footprint we impose on downstream users.
- Make dependency updates reviewable and traceable.

## Choosing dependencies

Before adding a new dependency we consider:

- **Necessity** — whether the functionality is worth an added dependency, or is small
  enough to implement directly.
- **Maintenance & trust** — active maintenance, a healthy release history, and a
  permissive, compatible license (the workspace is `Apache-2.0`).
- **Footprint** — transitive dependency count and compile-time cost.
- **Optionality** — anything not needed by every user should sit behind a Cargo
  feature so downstream builds stay lean. The SDK is heavily feature-gated for this
  reason (transports, `auth`, `schemars`, `reqwest`, etc.).

Version requirements are specified with caret (`^`, the Cargo default) constraints so
compatible upgrades are picked up automatically.

## Automated updates

Dependency updates are automated with [Dependabot](https://docs.github.com/en/code-security/dependabot),
configured in [`.github/dependabot.yml`](.github/dependabot.yml):

| Ecosystem      | Cadence | PR label          |
| -------------- | ------- | ----------------- |
| Cargo crates   | Weekly  | `T-dependencies`  |
| GitHub Actions | Daily   | `T-CI`            |

Dependabot opens a limited number of PRs at a time (currently 3 per ecosystem) to
keep the review queue manageable. Update PRs run the full CI suite (build, format,
clippy, tests, and conformance) and must be green before merge.

## Review and merge

- Patch and minor updates that pass CI are reviewed and merged promptly by maintainers.
- Major (breaking) updates are evaluated individually; they may require code changes
  and are scheduled deliberately rather than merged automatically.
- Any dependency change that affects the **public API** (for example a type re-exported
  from a dependency) is treated as a potential breaking change and follows
  [`VERSIONING.md`](VERSIONING.md).

## Security updates

- Security fixes take priority over routine updates and are expedited.
- Vulnerabilities in the SDK itself are handled per [`SECURITY.md`](SECURITY.md) via
  GitHub's private Security Advisory process.
- Advisories affecting our dependencies (e.g. via the [RustSec Advisory Database](https://rustsec.org/))
  are addressed as soon as a fixed version is available; if a released version of the
  SDK is impacted, a patched release is published and, where warranted, the affected
  versions are yanked from crates.io.

## Minimum Supported Rust Version

Dependency upgrades must not raise the SDK's declared MSRV (`rust-version` in
`Cargo.toml`) without an explicit, documented MSRV bump — which is itself treated as a
breaking change per [`VERSIONING.md`](VERSIONING.md).
