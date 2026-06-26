# CI Docs-Path Behavior

This table documents the docs-safe path registry from `ci/github-actions-runners.toml`.

`ci.yml` has no `pull_request.paths-ignore`: every PR runs the workflow, `host-health` runs,
and docs-safe PRs use an explicit `docs` policy path where heavy Rust lanes skip and `gate`
records a docs proof. Push, tag, workflow dispatch, and merge-queue events never use the docs
policy path.

| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | docs | heavy Rust lanes skip; `host-health` runs; `gate` records docs proof |
| root security policy | `SECURITY.md` | docs | heavy Rust lanes skip; `host-health` runs; `gate` records docs proof |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs |
| Rust source change | `src/lib.rs` | full-ci | full CI runs |
| managed rust-verification config | `ci/rust-verification.toml` | full-ci | full CI runs |
| forbidden legacy rust-verification config | `.claude/rust-verification.toml` | full-ci | full CI runs |
| feature registry input | `.specify/feature.json` | full-ci | full CI runs |
| lockfile change | `Cargo.lock` | full-ci | full CI runs |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs |
| ignored Claude agent dir | `.claude/skills/speckit-plan/SKILL.md` | docs | heavy Rust lanes skip; `host-health` runs; `gate` records docs proof |
| ignored config dir | `.codex/config.toml` | docs | heavy Rust lanes skip; `host-health` runs; `gate` records docs proof |

The classifier runs from the trusted PR base tree. A PR cannot edit the classifier or safe-path
registry and classify itself as docs-only. The docs proof is accepted only when every heavy lane
that would otherwise prove Rust CI resolved `skipped`; otherwise `gate` fails closed.

Safe ignored paths are intentionally narrow:

- `AGENTS.md`
- `CLAUDE.md`
- `GEMINI.md`
- `REASONIX.md`
- `LICENSE`
- `SECURITY.md`
- `.github/ISSUE_TEMPLATE/**`
- `.claude/**`
- `.codex/**`
- `.gemini/**`
- `.opencode/**`
- `.pi/**`
- `.specify/**`

Build inputs under those otherwise safe globs are forbidden from docs-only classification and
force full CI. Today that exception list is `.claude/rust-verification.toml` and
`.specify/feature.json`.

Do not add broad `docs/**`, `specs/**`, or `*.md` safe paths. This repo has docs/spec files that are build or test inputs.
