# Versioning Policy

`rmcp` and `rmcp-macros` follow [Semantic Versioning 2.0.0](https://semver.org/),
the standard versioning scheme for the Rust/Cargo ecosystem. Given a version
`MAJOR.MINOR.PATCH`:

- **MAJOR** — incremented for breaking changes to the public API.
- **MINOR** — incremented for new, backwards-compatible functionality.
- **PATCH** — incremented for backwards-compatible bug fixes.

The `rmcp` and `rmcp-macros` crates are versioned together and released with the
same version number.

## What counts as a breaking change

We treat a change as breaking if it would cause code that compiled against the
previous release to fail to compile, or to change behavior in a way callers could
not reasonably anticipate. This follows the Cargo SemVer guidelines
([The Cargo Book — SemVer Compatibility](https://doc.rust-lang.org/cargo/reference/semver.html)).
Examples include:

- Removing or renaming a public item (function, type, trait, module, variant, field).
- Changing a public function signature, trait method, or trait bound.
- Adding a required method to a public trait, or a field to a struct that callers
  construct directly. (Public structs and enums are marked `#[non_exhaustive]` where
  practical so that additive changes remain non-breaking.)
- Raising the Minimum Supported Rust Version (MSRV) — see below.

Additive changes — new functions, new types, new enum variants on
`#[non_exhaustive]` enums, new optional Cargo features — are **not** breaking and
ship in MINOR releases.

### Cargo features

The public API is feature-gated. SemVer compatibility is evaluated against the
crate's **default features** and all features except `local`.

## Pre-1.0 crates in the workspace

The core crates (`rmcp`, `rmcp-macros`) are `>= 1.0.0` and follow the rules above.
Any workspace crate still in `0.x` follows Cargo's pre-1.0 convention, where the
`MINOR` version acts as the breaking-change signal (`0.MINOR.PATCH`).

## Minimum Supported Rust Version (MSRV)

The MSRV is declared via `rust-version` in `Cargo.toml`. Raising the MSRV is
considered a breaking change and is only done in a MINOR or MAJOR release with a
note in the release notes.

## How breaking changes are communicated

- Release automation is handled by [release-plz](https://release-plz.dev/), which
  derives version bumps from [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat!:` / `fix!:` and `BREAKING CHANGE:` footers trigger a MAJOR bump) and runs
  `cargo-semver-checks` to catch unintended API breaks.
- Every release is published as a tagged [GitHub Release](https://github.com/modelcontextprotocol/rust-sdk/releases)
  with generated notes.
- Significant migrations (for example the 2.x → 3.x upgrade) are accompanied by a
  migration guide linked from the release notes and the README.

## Yanking

A published version that is later found to be broken or to contain a security issue
may be [yanked](https://doc.rust-lang.org/cargo/commands/cargo-yank.html) from
crates.io. Yanking prevents new dependents from selecting the version but does not
delete it; a fixed PATCH release is published alongside.
