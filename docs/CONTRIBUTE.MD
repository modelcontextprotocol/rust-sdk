# Discuss first
If you have a idea, make sure it is discussed before you make a PR. 

# Fmt And Clippy
You can use [just](https://github.com/casey/just) to help you fix your commit rapidly:
```shell
just fix
```

# How Can I Rewrite My Commit Message?
You can `git reset --soft upstream/main` and `git commit --forge`, this will merge your changes into one commit.

Or you also can use git rebase. But we will still merge them into one commit when it is merged.

# Check Code Coverage
If you are developing on vscode, you can use vscode plugin [Coverage Gutters](https://marketplace.visualstudio.com/items?itemName=ryanluker.vscode-coverage-gutters)

And also need to install llvm-cov
```sh
cargo install cargo-llvm-cov

rustup component add llvm-tools-preview
```

If you are using coverage gutters plugin, add these config to let it know lcov output.
```json
{
    "coverage-gutters.coverageFileNames": [
        "coverage.lcov",
    ],
    "coverage-gutters.coverageBaseDir": "target/llvm-cov-target",
}
```

# Triage

New issues and pull requests are triaged by `.github/workflows/triage.yml`, which
runs in two stages.

1. **Automated classification.** Codex reads the issue against the checkout and
   applies a type (`bug`/`enhancement`/`question`), a priority (`P0`-`P3`),
   component `T-*` labels, and a workflow label. Pull requests skip this stage;
   their labels come from the diff via `auto-label-pr.yml`.
2. **Human review.** The maintainer on rotation that week is assigned and
   @-mentioned with what the AI concluded. They are responsible for the final
   call, and the labels from stage 1 are only a first pass.

Stage 2 runs even when stage 1 is skipped or fails, so every item reaches a
person regardless of whether the automation worked.

## Rotation

The roster is every member with the `rust-sdk` role in
[`modelcontextprotocol/access`](https://github.com/modelcontextprotocol/access)
(`src/config/users.ts`), sorted by login for a stable order, with the ISO week number selecting whose turn it is. The
rotation turns over Monday 00:00 UTC. Adding or removing a member there changes
the roster and re-partitions future weeks, so the person on rotation can shift
when membership changes.

While on rotation, the expectation is:

- New issues are triaged within two business days.
- Critical (`P0`/`P1`) pull requests are resolved within seven days.

Assignment is a starting point, not ownership: reassign or hand off freely.

## Configuration

Classification needs two secrets, and if either is missing that stage is
skipped rather than failing the run:

| Secret | Purpose |
| --- | --- |
| `CODEX_AUTH_JSON` | Codex credentials, refreshed and written back each run. |
| `CODEX_AUTH_STORE_TOKEN` | Fine-grained PAT with `secrets: write`, since `GITHUB_TOKEN` has no `secrets` scope and cannot persist the refresh. |

Reviewer assignment needs no secret. The roster is read from the public
`access` repo, and the default `GITHUB_TOKEN` handles assigning and commenting.

To test either stage without side effects, run the workflow from the Actions tab
with `dry_run` enabled: it logs the verdict and the rotation pick, and changes
nothing.
