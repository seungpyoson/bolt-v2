# Tasks: NT Order Intent Layer

**Input**: Design documents from `specs/023-nt-order-intent-layer/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/order-intent-layer.md`, `quickstart.md`

**Tests**: Required. Every production behavior change must follow red, green, refactor.

## Phase 1: Setup And Architecture Evidence

**Purpose**: Make the architecture auditable before implementation.

- [x] T001 [US1] Record current branch, head, pinned NT checkout, and no-mistakes status in `research.md`
- [x] T002 [US1] Record NT core order model, factory, invariant, submit, risk, execution, and adapter evidence in `research.md`
- [x] T003 [US1] Record current Bolt narrowing points in `research.md`
- [x] T004 [US1] Create `contracts/order-intent-layer.md` with Bolt/NT ownership boundaries
- [x] T005 [US1] Create `data-model.md` with `StrategyPositionContract`, `NtOrderTemplate`, `OrderBuildInputs`, and `SubmitContext`

## Phase 2: Pre-Implementation Multi-Agent Review

**Purpose**: Challenge the spec and task plan before touching production code.

- [x] T006 [P] [US1] Run minimalism/YAGNI agent review against `spec.md`, `plan.md`, `data-model.md`, and `contracts/order-intent-layer.md`
- [x] T007 [P] [US1] Run venue/market agnosticism agent review against the same docs and pinned NT adapter evidence
- [x] T008 [P] [US1] Run end-to-end execution correctness agent review against the same docs and NT submit/risk/execution evidence
- [x] T009 [US1] Resolve or record every pre-implementation review finding in `research.md`

## Phase 3: TDD Slice 1 - Remove Tuple Narrowing Without Broadening Venue Policy

**Goal**: Preserve existing mixed maker/taker coverage and add valid short-side strategy position contracts without adding a maker-only mode or venue capability table.

**Independent Test**: `tests/config_parsing.rs` validates coherent short-side order configs through the public config validation path.

- [x] T010 [US2] Confirm existing mixed maker/taker config coverage remains green before widening the contract
- [x] T011 [US2] RED: Add a config validation test for coherent short-side entry/exit in `tests/config_parsing.rs`
- [x] T012 [US2] GREEN: Replace hardcoded entry/exit tuple whitelist in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` with strategy position-contract validation
- [x] T013 [US2] GREEN: Allow coherent short-side contracts while keeping incoherent long/short contracts rejected
- [x] T014 [US2] Verify focused tests, `cargo fmt -- --check`, and `git diff --check`

## Phase 4: TDD Slice 2 - Normalize Order Template Once

**Goal**: Remove dual parsing between startup typed config and runtime string parsing for currently enabled limit/market order shapes.

**Independent Test**: A strategy registration/runtime mapping test proves one normalized order config feeds construction without reparsing narrower strings.

- [x] T015 [US2] RED: Add a public-path test proving NT enum TIF/order type accepted at startup is preserved into runtime strategy config
- [x] T016 [US2] GREEN: Introduce a normalized order config shape that is shared by archetype validation and strategy construction
- [x] T017 [US2] GREEN: Remove redundant runtime string whitelist for order type/TIF where the normalized shape already carries NT enum values
- [x] T018 [US2] Verify focused tests, `cargo fmt -- --check`, and `git diff --check`

## Phase 5: TDD Slice 3 - NT Factory Crash-Prevention Validation

**Goal**: Validate NT model invariants before `OrderFactory` calls without encoding venue policy.

**Independent Test**: Invalid NT model shapes fail before order construction; valid shapes still use NT `OrderFactory`.

- [x] T019 [US2] RED: Add tests for invalid model inputs reachable by currently enabled limit/market factory calls
- [x] T020 [US2] GREEN: Add minimal NT model invariant validation only for currently enabled variants
- [x] T021 [US2] Verify focused tests, `cargo fmt -- --check`, and `git diff --check`

## Phase 6: TDD Slice 4 - Submit Context And Admission Evidence

**Goal**: Keep NT submit context explicit and compute admission from the compiled NT order view without duplicating NT order events.

**Independent Test**: Strategy submission admission uses the same compiled `OrderAny` that is submitted to NT, and optional submit context is threaded only at the NT submit call.

- [x] T022 [US3] RED: Add an admission test proving current admission can be derived from pre-build intent instead of the compiled NT order view
- [x] T023 [US3] GREEN: Compute admission inputs from the compiled `OrderAny` view while linking to NT lifecycle by `client_order_id`
- [x] T024 [US3] RED: Add a submit-boundary test proving optional `client_id`, `position_id`, and params reach NT `submit_order`
- [x] T025 [US3] GREEN: Thread submit context only at the NT submit boundary without embedding it in `NtOrderTemplate`
- [x] T026 [US3] Verify focused tests, `cargo fmt -- --check`, and `git diff --check`

## Phase 7: Factory Variant Expansion Gates

**Goal**: Prevent false completion while non-limit/market NT factory variants remain unsupported.

**Status**: Deferred after multi-agent review. The current highest-confidence implementation gate is GTD expiry for existing limit orders, followed by forced-exit/position semantics. Factory variant expansion remains a support-claim gate, not a prerequisite for the GTD slice.

- [ ] T027 [US2] RED: Add one positive construction/admission test for the next factory-supported NT order variant selected by user-approved scope
- [ ] T028 [US2] GREEN: Enable that variant through the same normalized template path using NT `OrderFactory`
- [ ] T029 [US2] Repeat T027-T028 one variant at a time before claiming support for additional NT factory variants

## Phase 8: GTD, OMS/Position, And Forced-Exit Gates

**Goal**: Keep high-risk execution semantics explicit instead of implied by generic order config.

- [x] T030 [US3] Add a positive GTD expiry test before claiming GTD support
- [x] T031 [US3] Add NETTING/HEDGING and reduce-only position tests before claiming position-aware submit support
- [x] T032 [US3] Add a forced-exit behavior test or record forced-exit as residual scope before completion claims

**Residual**: T031 proves NT OMS enum acceptance, submit-boundary `PositionId` threading, and reduce-only forced-exit order construction. It does not prove live adapter-specific position behavior.

## Phase 9: Adapter-Proof Harness Planning

**Goal**: Prove venue legality through NT source or no-submit smoke, not Bolt runtime policy.

- [x] T033 [US4] Document adapter source evidence requirements for every adapter named by a support claim in `research.md`
- [x] T034 [US4] Define no-submit smoke proof boundaries for order templates without live submit
- [x] T035 [US4] Keep live/canary proof explicitly blocked until user approval
- [x] T036 [US4] Record absence of no-submit/canary artifacts as residual scope for any execution claim

## Phase 10: Post-Implementation Review And Verification

**Purpose**: Verify exact head before completion claims.

- [x] T037 [P] Run minimalism/YAGNI post-implementation review
- [x] T038 [P] Run venue/market agnosticism post-implementation review
- [x] T039 [P] Run end-to-end execution correctness post-implementation review
- [x] T040 Resolve or record every review finding in `research.md`
- [x] T041 Run relevant focused tests, full `cargo test`, `cargo fmt -- --check`, and `git diff --check`
- [x] T042 Confirm no-mistakes status is for the current branch/head or record why it is not proof
- [x] T043 Commit and push only after local verification and required review findings are resolved

## Phase 11: TDD Slice 8 - Review-Found Maker Exit Lifecycle

**Goal**: Fix the externally reviewed partial maker exit lifecycle without changing venue policy or narrowing NT order support.

- [x] T044 [US3] RED: Add a regression for partial maker/GTD exit fill followed by terminal remainder in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T045 [US3] GREEN: Recover to managed residual exposure when NT has reported an open residual position and the exit order is terminal in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T046 [US3] Verify focused exit lifecycle tests, full `cargo test`, `cargo fmt -- --check`, and `git diff --check`
- [x] T047 [US3] Record Claude/Gemini/Kimi post-review outcomes and the T044-T046 evidence in `research.md`

## Phase 12: TDD Slice 9 - Exact-Head CI Source-Fence Follow-Up

**Goal**: Resolve the PR #434 exact-head `fmt-check` and `source-fence` failures without changing order-intent behavior.

- [x] T048 RED: Reproduce the exact runtime-literal allowlist failure with `just fmt-check` and `just source-fence`
- [x] T049 GREEN: Update runtime-literal classifications in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`
- [x] T050 Verify `just fmt-check`, `just source-fence`, focused order-intent tests, and branch cleanliness
- [x] T051 Record CI follow-up evidence and no-mistakes state in `research.md`

## Phase 13: TDD Slice 10 - StopMarket Factory Variant

**Goal**: Enable the next NT `OrderFactory`-supported variant through the same order-template path without adding venue policy or bypassing NT.

- [x] T052 [US2] RED: Add a positive StopMarket construction/admission regression in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T053 [US2] GREEN: Thread TOML-owned StopMarket trigger price through archetype/runtime order config into NT `OrderFactory::stop_market`
- [x] T054 [US2] RED/GREEN: Add public config validation coverage for StopMarket order config in `tests/config_parsing.rs`
- [x] T055 [US2] Verify focused StopMarket tests, runtime literal/source fences, full `cargo test`, and branch cleanliness
- [x] T056 [US2] Record StopMarket evidence, exact-head gate state, and residual unsupported variants in `research.md`

## Phase 14: TDD Slice 11 - StopMarket Review Blocker Fixes

**Goal**: Resolve external-review blockers without adding venue policy or bypassing NT.

- [x] T057 [US2] RED: Reproduce StopMarket admission/sizing underestimation when `Order::price()` is absent and trigger price is higher than the pre-trigger book price
- [x] T058 [US2] GREEN: Use NT `Order::trigger_price()` for triggered-order admission fallback and StopMarket entry sizing price
- [x] T059 [US2] RED/GREEN: Add raw archetype-to-runtime strategy build round-trip for `order_type=stop_market`
- [x] T060 [US2] Verify post-review focused tests, runtime literal/source fences, full `cargo test`, and branch cleanliness
- [x] T061 [US2] Record external-review findings, fixes, exact-head gate state, and residual risks in `research.md`

## Phase 15: TDD Slice 12 - StopLimit Factory Variant

**Goal**: Enable NT `OrderFactory::stop_limit` through the same order-template path without adding venue policy or changing triggered-market sizing semantics.

- [x] T062 [P] [US2] Run internal NT-source review comparing remaining factory variants and record the recommendation in `research.md`
- [x] T063 [P] [US2] Run internal Bolt-path review for the minimal next variant slice and record the recommendation in `research.md`
- [x] T064 [US2] RED/GREEN: Add positive StopLimit construction/admission coverage in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T065 [US2] RED/GREEN: Add public archetype config coverage for StopLimit with TOML-owned `trigger_price` in `tests/config_parsing.rs`
- [x] T066 [US2] Add raw archetype-to-runtime strategy build round-trip for `order_type=stop_limit` in `tests/bolt_v3_strategy_registration.rs`
- [x] T067 [US2] Verify StopLimit focused tests, runtime literal/source fences, full `cargo test`, branch cleanliness, and exact-head gate state

## Phase 16: TDD Slice 12 Review Regression Coverage

**Goal**: Close post-review StopLimit regression gaps without changing production behavior unless a test exposes a real defect.

- [x] T068 [P] [US2] Run post-slice external adversarial review against the exact StopLimit diff and record usable findings in `research.md`
- [x] T069 [US2] Add StopLimit entry/exit construction coverage for GTD expiry and post-only factory fields in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T070 [US2] Add StopLimit exit-order archetype and runtime round-trip coverage in `tests/config_parsing.rs` and `tests/bolt_v3_strategy_registration.rs`
- [x] T071 [US2] Add negative StopLimit archetype coverage for missing/non-positive `trigger_price`, GTD-without-expiry, and unsupported strategy-scope flags
- [x] T072 [US2] Verify focused tests, full local checks, and repo gates after the regression patch
- [x] T073 [US2] Verify post-push exact-head GitHub gate, no-mistakes state, and external review state for the regression patch head

## Phase 17: TDD Slice 13 - MarketIfTouched Factory Variant

**Goal**: Enable NT `OrderFactory::market_if_touched` through the same order-template path without adding venue policy or changing `LimitIfTouched`/trailing semantics.

- [x] T074 [P] [US2] Run internal NT-source/Bolt-path/adversarial architecture reviews for the next variant slice and record the recommendation in `research.md`
- [x] T075 [US2] RED: Add positive MarketIfTouched construction/admission/sizing coverage in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T076 [US2] GREEN: Thread TOML-owned MarketIfTouched trigger price through archetype/runtime order config into NT `OrderFactory::market_if_touched`
- [x] T077 [US2] RED/GREEN: Add public archetype and runtime round-trip coverage for `order_type=market_if_touched`
- [x] T078 [US2] Verify focused MarketIfTouched tests, runtime literal/source fences as possible, full local tests as possible, branch cleanliness, exact-head gate state, and external review state

## Phase 18: TDD Slice 14 - Optional Order Field Shape Validation

**Goal**: Resolve the no-mistakes optional-field finding only where current pinned NT Rust and Bolt architecture evidence support it, without adding venue policy or narrowing supported order variants.

- [x] T079 [P] [US2] Review no-mistakes optional-field patch against pinned NT source, current Bolt config/runtime paths, and architecture docs
- [x] T080 [US2] Characterize existing `trigger_price` rejection on non-triggered public archetype order configs before runtime construction
- [x] T081 [US2] Record why non-GTD `expire_time_unix_nanos` rejection is not adopted as an NT invariant for the pure Rust path
- [x] T082 [US2] Verify focused tests, full local checks as possible, branch cleanliness, exact-head gate state, no-mistakes state, and external review state

## Phase 19: TDD Slice 15 - Isolated Nextest Trigger Field Validation

**Goal**: Fix the CI nextest-archive isolated failure by enforcing only the NT-supported trigger-field shape for non-triggered order types, without adopting unsupported expiry policy.

- [x] T083 [US2] RED: Capture CI nextest isolated failure for `bolt_v3_archetype_rejects_non_triggered_entry_order_with_trigger_price`
- [x] T084 [US2] GREEN: Reject `trigger_price` on non-triggered `Limit`/`Market` entry and exit order configs in the archetype validator
- [x] T085 [US2] Verify focused tests, nextest isolated tests, full local checks as possible, branch cleanliness, exact-head CI state, stale no-mistakes state, and reviewer state

## Phase 20: TDD Slice 16 - Source-Fence Follow-Up

**Goal**: Resolve the exact-head source-fence failure without adding hardcoded order-field literals or widening runtime literal policy.

- [x] T086 [US2] RED: Reproduce `just source-fence` failure for the new inline `exit_order` production literal
- [x] T087 [US2] GREEN: Fold non-triggered `trigger_price` rejection into the existing entry/exit order-combination predicates
- [x] T088 [US2] Verify focused tests, source-fence, full local checks as possible, and archived nextest replay before push

## Phase 21: TDD Slice 17 - LimitIfTouched Factory Variant

**Goal**: Enable NT `OrderFactory::limit_if_touched` through the existing normalized order path without adding venue policy, direct NT constructors, or a parallel submit path.

- [x] T089 [P] [US2] Run current NT-source and adversarial architecture reviews for `LimitIfTouched` and record the findings
- [x] T090 [US2] Correct stale support ledgers before implementation so `MarketIfTouched` is no longer listed as remaining scope
- [x] T091 [US2] RED: Add positive `LimitIfTouched` construction/admission coverage for entry and exit in `src/strategies/binary_oracle_edge_taker.rs`
- [x] T092 [US2] RED: Add pre-factory side-aware trigger/limit price rejection coverage for BUY and SELL `LimitIfTouched`
- [x] T093 [US2] GREEN: Construct `LimitIfTouched` through `OrderFactory::limit_if_touched` and validate pinned NT model invariants before factory calls
- [x] T094 [US2] RED/GREEN: Add public archetype config and raw runtime round-trip coverage for `order_type=limit_if_touched`
- [x] T095 [US2] Verify focused `LimitIfTouched` tests, runtime literal/source fences as possible, full local tests as possible, branch cleanliness, exact-head gate state, and stale reviewer state

## Dependencies & Execution Order

- Phase 1 blocks implementation.
- Phase 2 blocks implementation.
- Phase 3 is the first implementation slice and must complete before later slices.
- Phase 4 depends on Phase 3 because it changes the same order config path.
- Phase 5 depends on the normalized template from Phase 4.
- Phase 6 depends on compiled order behavior from Phase 4 and validation from Phase 5.
- Phase 7 blocks broad NT order-variant support claims.
- Phase 8 blocks GTD, position-aware, and forced-exit support claims.
- Phase 9 can run in parallel with later implementation planning but cannot claim live support.
- Phase 10 blocks completion.
- Phase 11 blocks completion because it resolves a post-implementation review blocker.
- Phase 12 blocks completion because PR #434 exact-head CI failed after the pushed implementation.
- Phase 13 is the first deferred Phase 7 factory-variant slice and blocks StopMarket support claims until verified.
- Phase 14 blocks completion because Claude and Gemini both found post-Phase 13 StopMarket blockers.
- Phase 15 is the next deferred Phase 7 factory-variant slice and blocks StopLimit support claims until verified and reviewed.
- Phase 16 blocks StopLimit support claims because post-slice review found missing regression coverage.
- Phase 17 is the next deferred Phase 7 factory-variant slice and blocks MarketIfTouched support claims until verified and reviewed.
- Phase 18 blocks completion because no-mistakes produced an optional-field validation finding after the MarketIfTouched exact-head review.
- Phase 19 blocks completion because CI proved the Phase 18 characterization test was not isolated-green under nextest archive.
- Phase 21 is the next deferred Phase 7 factory-variant slice and blocks LimitIfTouched support claims until verified and reviewed.

## Parallel Opportunities

- T006, T007, and T008 can run in parallel.
- T037, T038, and T039 can run in parallel after implementation.
- Adapter source evidence in T027 can be gathered independently from TDD slices if it does not modify production code.

## Implementation Strategy

1. Finish pre-implementation review.
2. Execute one TDD slice at a time.
3. Keep implementation inside the listed source/test files unless a review finding proves extraction is necessary.
4. Verify each slice before moving to the next.
5. Treat source-level proof, no-submit proof, and live proof as separate evidence classes.
