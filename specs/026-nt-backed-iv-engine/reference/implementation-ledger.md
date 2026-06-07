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

## Phase 4 User Story 2 Subscription Planning

| Task | Evidence | Status |
|---|---|---|
| `T037` | `tests/bolt_v3_iv_subscription.rs` asserts option-greeks sources map to NT option-greeks subscribe operations with configured client, selector, params, and generation. | Complete |
| `T038` | `tests/bolt_v3_iv_subscription.rs` asserts option-chain sources map to NT option-chain subscribe operations with configured series and strike-range selector. | Complete |
| `T039` | `tests/bolt_v3_iv_subscription.rs` asserts aggregate-greeks sources map to NT greeks-topic subscribe operations. | Complete |
| `T040` | `tests/bolt_v3_iv_subscription.rs` asserts custom implied-volatility sources map to ledger-compatible NT custom-data subscribe operations. | Complete |
| `T041` | `tests/bolt_v3_iv_subscription.rs` asserts reload unsubscribes stale generations, subscribes the new generation, and emits source-removal operations for deleted sources. | Complete |
| `T042` | RED evidence below records the missing subscription/runtime module failure. | Complete |
| `T043` | `src/bolt_v3_iv/subscription.rs` defines `IvSubscriptionPlan`, profile/source subscription config, lifecycle, runtime operation, and NT source-kind enums. | Complete |
| `T044` | `src/bolt_v3_iv/subscription.rs` maps option-greeks sources to subscribe/unsubscribe option-greeks operations. | Complete |
| `T045` | `src/bolt_v3_iv/subscription.rs` maps option-chain sources to subscribe/unsubscribe option-chain operations. | Complete |
| `T046` | `src/bolt_v3_iv/subscription.rs` maps aggregate-greeks sources to subscribe/unsubscribe aggregate-greeks topic operations. | Complete |
| `T047` | `src/bolt_v3_iv/subscription.rs` maps custom implied-volatility sources to subscribe/unsubscribe custom-data operations. | Complete |
| `T048` | `src/bolt_v3_iv/subscription.rs` implements start, stop, reload, unsubscribe, and source-removal planning. | Complete |
| `T049` | `src/bolt_v3_iv/runtime.rs` defines the runtime binding adapter trait and plan outcome/source-health bridge. | Complete |
| `T050` | GREEN evidence below records the passing focused US2 test target. | Complete |

## Phase 4 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_subscription` | RED | Failed with unresolved imports because `bolt_v2::bolt_v3_iv::runtime` and `bolt_v2::bolt_v3_iv::subscription` did not exist. |
| `cargo test --locked --test bolt_v3_iv_subscription` | GREEN | 5 tests passed after adding typed subscription planning, reload/source-removal planning, and runtime binding adapter outcomes. |

NT runtime mapping decisions for US2: subscription plans carry only configured profile/source/client/selector/params/generation values; source kind to NT operation mapping is explicit and typed; no strategy, venue, market, asset, instrument, cadence, source ID, timeout, or policy runtime value is introduced in core logic.

## Phase 5 User Story 3 Raw Preservation And Indexed Products

| Task | Evidence | Status |
|---|---|---|
| `T051` | `tests/bolt_v3_iv_ingest.rs` asserts option-greeks raw payload preservation and mark/bid/ask IV plus greeks indexing. | Complete |
| `T052` | `tests/bolt_v3_iv_ingest.rs` asserts option-chain slices build retained smiles and surface views without interpolation. | Complete |
| `T053` | `tests/bolt_v3_iv_ingest.rs` asserts aggregate-greeks raw events index to aggregate-greeks products. | Complete |
| `T054` | `tests/bolt_v3_iv_ingest.rs` asserts custom implied-volatility events index as custom IV evidence. | Complete |
| `T055` | `tests/bolt_v3_iv_store.rs` asserts raw payload access is audit/replay/test-only and strategy role access is denied. | Complete |
| `T056` | `tests/bolt_v3_iv_store.rs` asserts indexed product provenance is complete and incomplete provenance rejects. | Complete |
| `T057` | RED evidence below records the missing ingest/store/raw-access/provenance validation failure. | Complete |
| `T058` | `src/bolt_v3_iv/ingest.rs` implements `IvRawEvent` preservation and raw provenance creation. | Complete |
| `T059` | `src/bolt_v3_iv/store.rs` indexes `IvPoint` and `IvGreeksPoint` from option-greeks payloads. | Complete |
| `T060` | `src/bolt_v3_iv/store.rs` constructs retained `IvSmile` views from option-chain slices. | Complete |
| `T061` | `src/bolt_v3_iv/store.rs` constructs `IvSurface` views from retained smiles. | Complete |
| `T062` | `src/bolt_v3_iv/store.rs` indexes `IvAggregateGreeks` products. | Complete |
| `T063` | `src/bolt_v3_iv/store.rs` indexes custom `IvEvidence` products. | Complete |
| `T064` | `src/bolt_v3_iv/raw_access.rs` enforces audit/replay/test-only raw event access. | Complete |
| `T065` | `src/bolt_v3_iv/provenance.rs` builds raw-event provenance and validates required provenance fields. | Complete |
| `T066` | GREEN evidence below records the passing focused US3 test targets. | Complete |

## Phase 5 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_ingest --test bolt_v3_iv_store` | RED | Failed with unresolved imports because `ingest`, `store`, `raw_access`, and `validate_iv_provenance` did not exist. |
| `cargo test --locked --test bolt_v3_iv_ingest --test bolt_v3_iv_store` | GREEN | 6 tests passed after adding raw preservation, indexed products, audit-only raw reads, and provenance validation. |

Raw-boundary decisions for US3: raw payloads stay in `IvRawEvent` and are reachable only through `raw_access`; strategy-role raw access rejects; strategy-safe products carry provenance references to raw event IDs but do not expose raw payload values.

## Phase 6 User Story 4 NT Helper Derivation

| Task | Evidence | Status |
|---|---|---|
| `T067` | `tests/bolt_v3_iv_derive.rs` asserts helper policy selection chooses the configured NT helper symbol. | Complete |
| `T068` | `tests/bolt_v3_iv_derive.rs` asserts complete inputs derive IV through the NT helper and record helper provenance. | Complete |
| `T069` | `tests/bolt_v3_iv_derive.rs` asserts every required derived-input field rejects when missing. | Complete |
| `T070` | `tests/bolt_v3_iv_derive.rs` asserts stale/skewed input timestamps reject before helper invocation. | Complete |
| `T071` | `tests/bolt_v3_iv_derive.rs` asserts expired operator-configured rate/carry inputs reject before helper invocation. | Complete |
| `T072` | `tests/bolt_v3_iv_derive.rs` asserts helper output outside configured IV bounds rejects. | Complete |
| `T073` | RED evidence below records the missing derive module failure. | Complete |
| `T074` | `src/bolt_v3_iv/derive.rs` implements `IvHelperPolicy` and typed NT helper symbol selection. | Complete |
| `T075` | `src/bolt_v3_iv/derive.rs` resolves required derived input fields from the typed input set. | Complete |
| `T076` | `src/bolt_v3_iv/derive.rs` validates missing, non-finite, non-positive, skewed, and expired inputs before helper invocation. | Complete |
| `T077` | `src/bolt_v3_iv/derive.rs` invokes `nautilus_model::data::imply_vol_and_greeks` from the pinned NT dependency. | Complete |
| `T078` | `src/bolt_v3_iv/derive.rs` validates helper IV output against configured numeric/convention bounds. | Complete |
| `T079` | `src/bolt_v3_iv/derive.rs` attaches `IvHelperIdentity` and `IvPolicyDecision::Helper` to derived-output provenance. | Complete |
| `T080` | GREEN evidence below records the passing focused US4 test target. | Complete |

## Phase 6 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_derive` | RED | Failed with unresolved import because `bolt_v2::bolt_v3_iv::derive` did not exist. |
| `cargo test --locked --test bolt_v3_iv_derive` | GREEN | 6 tests passed after adding helper policy selection, complete-input derivation, fail-closed input validation, output-bound validation, and helper provenance. |

Helper NT-source decisions for US4: the wrapper calls `nautilus_model::data::imply_vol_and_greeks`; test fixtures compute the expected price/vol from `nautilus_model::data::black_scholes_greeks`; no strategy-local or non-NT IV helper path was introduced.
