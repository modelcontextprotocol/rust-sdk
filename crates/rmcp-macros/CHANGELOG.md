# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/modelcontextprotocol/rust-sdk/compare/rmcp-macros-v0.1.5...rmcp-macros-v0.2.0) - 2025-07-02

### Added

- add progress notification handling and related structures ([#282](https://github.com/modelcontextprotocol/rust-sdk/pull/282))
- *(server)* add annotation to tool macro ([#184](https://github.com/modelcontextprotocol/rust-sdk/pull/184))
- *(model)* add json schema generation support for all model types ([#176](https://github.com/modelcontextprotocol/rust-sdk/pull/176))
- *(transport)* support streamable http server ([#152](https://github.com/modelcontextprotocol/rust-sdk/pull/152))
- *(rmcp-macro)* generate description from docs ([#141](https://github.com/modelcontextprotocol/rust-sdk/pull/141))
- revision-2025-03-26 without streamable http ([#84](https://github.com/modelcontextprotocol/rust-sdk/pull/84))

### Fixed

- *(examples)* add clients in examples's readme ([#225](https://github.com/modelcontextprotocol/rust-sdk/pull/225))
- generic ServerHandler ([#223](https://github.com/modelcontextprotocol/rust-sdk/pull/223))
- cleanup zombie processes for child process client ([#156](https://github.com/modelcontextprotocol/rust-sdk/pull/156))
- *(rmcp-macros)* fix extract_doc_line code ([#142](https://github.com/modelcontextprotocol/rust-sdk/pull/142))
- *(macros)* add error deal ([#109](https://github.com/modelcontextprotocol/rust-sdk/pull/109))
- *(macro)* add generics marco types support ([#98](https://github.com/modelcontextprotocol/rust-sdk/pull/98))
- *(typo)* s/marcos/macros/ ([#85](https://github.com/modelcontextprotocol/rust-sdk/pull/85))
- *(test)* fix tool deserialization error ([#68](https://github.com/modelcontextprotocol/rust-sdk/pull/68))

### Other

- refactor tool macros and router implementation ([#261](https://github.com/modelcontextprotocol/rust-sdk/pull/261))
- revert badge ([#202](https://github.com/modelcontextprotocol/rust-sdk/pull/202))
- use hierarchical readme for publishing ([#198](https://github.com/modelcontextprotocol/rust-sdk/pull/198))
- Ci/coverage badge ([#191](https://github.com/modelcontextprotocol/rust-sdk/pull/191))
- Transport trait and worker transport, and streamable http client with those new features. ([#167](https://github.com/modelcontextprotocol/rust-sdk/pull/167))
- add oauth2 support ([#130](https://github.com/modelcontextprotocol/rust-sdk/pull/130))
- update calculator example description ([#115](https://github.com/modelcontextprotocol/rust-sdk/pull/115))
- fix the url ([#120](https://github.com/modelcontextprotocol/rust-sdk/pull/120))
- add a simple chat client for example ([#119](https://github.com/modelcontextprotocol/rust-sdk/pull/119))
- add spell check ([#82](https://github.com/modelcontextprotocol/rust-sdk/pull/82))
- Adopt Devcontainer for Development Environment ([#81](https://github.com/modelcontextprotocol/rust-sdk/pull/81))
- fix typos ([#79](https://github.com/modelcontextprotocol/rust-sdk/pull/79))
- format and fix typo ([#72](https://github.com/modelcontextprotocol/rust-sdk/pull/72))
- add documentation generation job ([#59](https://github.com/modelcontextprotocol/rust-sdk/pull/59))
- fmt the project ([#54](https://github.com/modelcontextprotocol/rust-sdk/pull/54))
- fix broken link ([#53](https://github.com/modelcontextprotocol/rust-sdk/pull/53))
- fix the branch name for git dependency ([#46](https://github.com/modelcontextprotocol/rust-sdk/pull/46))
- Move whole rmcp crate to official rust sdk ([#44](https://github.com/modelcontextprotocol/rust-sdk/pull/44))
- Initial commit
