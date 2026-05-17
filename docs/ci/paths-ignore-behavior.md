# CI Paths-Ignore Behavior

This table documents the current `pull_request.paths-ignore` contract from `.github/workflows/ci.yml`.

Push and tag events do not use `paths-ignore`; they always run CI.

| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs; pass-stub does not trigger |
| Rust source change | `src/lib.rs` | full-ci | full CI runs; pass-stub does not trigger |
| managed rust-verification config | `.claude/rust-verification.toml` | full-ci | full CI runs; pass-stub does not trigger |
| lockfile change | `Cargo.lock` | full-ci | full CI runs; pass-stub does not trigger |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs; pass-stub records `docs_only=false` without blocking |
| ignored config dir | `.codex/config.toml` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |

The pass-stub required-check jobs have no job-level `if:` condition. GitHub reports skipped jobs as successful, so the classifier fails each required stub job directly when the changed-file list is empty, unavailable, or cannot be classified. If classification succeeds and any path is outside the ignored-safe set, full CI owns the real required signals.

Safe ignored paths are intentionally narrow:

- `AGENTS.md`
- `CLAUDE.md`
- `GEMINI.md`
- `REASONIX.md`
- `LICENSE`
- `.github/ISSUE_TEMPLATE/**`
- `.codex/**`
- `.gemini/**`
- `.opencode/**`
- `.pi/**`
- `.specify/**`

Do not add broad `docs/**`, `specs/**`, or `*.md` ignores. This repo has docs/spec files that are build or test inputs.
