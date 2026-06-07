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

## Phase 2 Foundational Types

| Task | Evidence | Status |
|---|---|---|
| `T010` | `src/bolt_v3_iv/mod.rs` declares `audit`, `authz`, `bounds`, `error`, `health`, `provenance`, `selector`, `time`, and `types`. | Complete |
| `T011` | `src/bolt_v3_iv/types.rs` defines source/product/basis/convention enums. | Complete |
| `T012` | `src/bolt_v3_iv/error.rs` defines `IvRejectReason` and the required-reason list. | Complete |
| `T013` | `src/bolt_v3_iv/time.rs` defines `UnixNanos`. | Complete |
| `T014` | `src/bolt_v3_iv/bounds.rs` defines `IvNumericBounds`, `IvBoundUnit`, and `IvConventionBounds`. | Complete |
| `T015` | `src/bolt_v3_iv/provenance.rs` defines `IvPolicyDecision`. | Complete |
| `T016` | `src/bolt_v3_iv/provenance.rs` defines `IvProvenance` and typed helper identity. | Complete |
| `T017` | `src/bolt_v3_iv/selector.rs` defines the source/query selector union and product mapping. | Complete |
| `T018` | `src/bolt_v3_iv/authz.rs` defines `IvSelectorAuthorization`. | Complete |
| `T019` | `src/bolt_v3_iv/audit.rs` defines `IvAuditPolicy`, `IvAuditHandleId`, raw product kinds, and retention marker. | Complete |
| `T020` | `src/bolt_v3_iv/health.rs` defines source-health states, transitions, and current-query eligibility. | Complete |
| `T021` | `tests/bolt_v3_iv_support.rs` provides shared IV fixture builders. | Complete |
| `T022` | `tests/bolt_v3_iv_source_fence.rs` provides the IV source-fence entrypoint placeholder. | Complete |
| `T023` | `justfile` wires `--test bolt_v3_iv_source_fence` into the managed `source-fence` cargo test invocation. | Complete |
| `T024` | This section records the foundational RED/GREEN evidence. | Complete |

## Phase 2 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_foundation` | RED | Initial run was blocked by the Rust verification disk-pressure guard. After removing the generated managed target cache, the RED run failed with `E0432` unresolved imports for missing `bolt_v3_iv` modules. |
| `cargo test --locked --test bolt_v3_iv_foundation` | GREEN | 4 tests passed after adding foundational modules. |
| `cargo test --locked --test bolt_v3_iv_source_fence` | GREEN | 1 test passed for the IV source-fence entrypoint placeholder. |
| `cargo fmt --check` | GREEN | Final formatter check exited 0 after applying `cargo fmt`. |
| `git diff --check` | GREEN | No whitespace errors after Phase 2 edits. |

`just source-fence`, clippy, full IV user-story targets, and CI remain open for the broader feature and are still tracked by `T123` and `T124`.

## Phase 3 User Story 1 Capability Inventory

| Task | Evidence | Status |
|---|---|---|
| `T025` | `tests/bolt_v3_iv_capability.rs` asserts Cargo metadata plus `Cargo.lock` resolve to the pinned NT checkout and a single NT revision. | Complete |
| `T026` | `tests/bolt_v3_iv_capability.rs` asserts seed-family discovery covers model data, data actor subscriptions, data-engine publications, msgbus topics, option-chain manager, greeks helper, adapter, and custom-data surfaces. | Complete |
| `T027` | `tests/bolt_v3_iv_capability.rs` asserts the whole-checkout sweep includes every FR-054 term. | Complete |
| `T028` | `tests/bolt_v3_iv_capability.rs` asserts unclassified candidates are rejected before fixture-backed ledger validation passes. | Complete |
| `T029` | RED evidence below records the missing capability module failure. | Complete |
| `T030` | `src/bolt_v3_iv/capability.rs` implements the NT Cargo metadata and lockfile resolver. | Complete |
| `T031` | `src/bolt_v3_iv/capability.rs` implements seed-family scanning for the required NT capability families. | Complete |
| `T032` | `src/bolt_v3_iv/capability.rs` implements recursive public-symbol candidate discovery across Rust source files. | Complete |
| `T033` | `src/bolt_v3_iv/capability.rs` implements explicit candidate classification and unclassified-candidate rejection. | Complete |
| `T034` | `src/bolt_v3_iv/capability.rs` implements the capability ledger TOML fixture loader. | Complete |
| `T035` | `tests/fixtures/bolt_v3_iv/capability-ledger.toml` records supported fixture classifications. | Complete |
| `T036` | GREEN evidence below records the passing focused US1 test target. | Complete |

## Phase 3 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_capability` | RED | Failed with `E0432` unresolved import because `bolt_v2::bolt_v3_iv::capability` did not exist. |
| `cargo test --locked --test bolt_v3_iv_capability` | GREEN | 4 tests passed after adding the capability resolver, scanners, explicit ledger model, and TOML fixture. |

NT-first decisions for US1: the Cargo resolver derives NT evidence from `cargo metadata` and `Cargo.lock`; scanners operate against source files from the resolved checkout shape; no FV/RV, strategy-specific, venue-specific, or sidecar collection behavior was introduced.
