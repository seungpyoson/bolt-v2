# Phase 8 Quickstart

## Safe Local Checks Only

Current Phase 8 status: live action blocked. Use these commands only after Phase 8 implementation exists.

```bash
cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture
cargo test --test bolt_v3_tiny_canary_operator -- --nocapture
cargo fmt --check
git diff --check
```

Expected local result:

- precondition tests pass
- operator test is ignored by default
- format and diff checks pass

## Live Order Command

No live order command is approved by this quickstart.

Before any live-order command can be run, the user must approve in the current thread:

- exact head SHA
- exact command
- approved root TOML path
- approved root TOML checksum
- approved SSM manifest hash
- operator approval id
- canary evidence path

## Current Blockers

- Phase 7 no-submit readiness is not merged to current main in this worktree.
- No accepted real no-submit readiness report is available.
- Strategy-input safety audit blocks live action.
- Exact live config is absent from tracked files and must not be printed if present locally.
- External review of Phase 8 spec/plan/tasks has not run.
