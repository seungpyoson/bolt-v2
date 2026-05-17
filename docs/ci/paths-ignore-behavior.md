# CI Paths-Ignore Behavior

This table documents the current `pull_request.paths-ignore` contract from `.github/workflows/ci.yml`.

Push and tag events do not use `paths-ignore`; they always run CI.

| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | ignored-safe | full CI skipped; pass-stub `gate` runs and succeeds |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs; pass-stub does not trigger |
| Rust source change | `src/lib.rs` | full-ci | full CI runs; pass-stub does not trigger |
| managed rust-verification config | `.claude/rust-verification.toml` | full-ci | full CI runs; pass-stub does not trigger |
| lockfile change | `Cargo.lock` | full-ci | full CI runs; pass-stub does not trigger |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs; pass-stub triggers and fails closed |
| ignored config dir | `.codex/config.toml` | ignored-safe | full CI skipped; pass-stub `gate` runs and succeeds |

The pass-stub `gate` job has no job-level `if:` condition. GitHub reports skipped jobs as successful, so the classifier fails the `gate` job directly when the changed-file list is empty, unavailable, or includes any path outside the ignored-safe set.

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
