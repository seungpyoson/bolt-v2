# IV Engine Implementation Ledger

**Feature**: `specs/026-nt-backed-iv-engine/`
**Scope**: NT-backed IV engine only. FV, RV, market-maker behavior, broad sidecar collectors, and strategy-specific IV logic are out of scope.

## Phase 1 Setup

| Task | Evidence | Status |
|---|---|---|
| `T001` | `reference/repository-truth.md` records fetch/prune, branch/head/main/merge-base, PR state, changed files, and diffstat. | Complete |
| `T002` | `reference/overlap-ledger.md` refreshed open PR and issue overlap at head `f994ae15198502aee9227aea5e813d12b8d5bf92`; no open PRs and no fully ported issues found. | Complete |
| `T003` | `reference/evidence-ledger.md` records current-main evidence and missing/implemented status by requirement group. | Complete |
| `T004` | `reference/nt-evidence.md` records the pinned NT git revision from `Cargo.toml` and `Cargo.lock`. | Complete |
| `T005` | `src/bolt_v3_iv/mod.rs` establishes the Rust module boundary without runtime behavior. | Complete |
| `T006` | `src/lib.rs` exports `bolt_v3_iv`. | Complete |
| `T007` | `tests/fixtures/bolt_v3_iv/README.md` establishes the fixture directory. | Complete |
| `T008` | `tests/fixtures/bolt_v3_iv/evidence.md` records expected fixture inventory. | Complete |
| `T009` | This ledger records implementation progress. | Complete |

## Verification Log

| Command | Outcome | Notes |
|---|---|---|
| `git diff --check` | PASS | No whitespace errors. |
| `cargo fmt --check` | PASS | Formatter check exited 0. |
| `cargo test --locked bolt_v3_iv` | PASS | Compiled the crate graph and ran filtered test binaries; no matching tests exist yet for the Phase 1 boundary, and all filtered binaries exited 0. |

`just source-fence`, clippy, full IV test targets, and CI are not complete for the full feature. They remain tracked by polish tasks `T123` and `T124` after implementation tests exist.

## TDD Boundary

Phase 1 adds documentation, module boundaries, and fixture inventory only. Runtime behavior starts in Phase 2 and later user-story tasks, where each production behavior must have RED/GREEN evidence before being marked complete.
