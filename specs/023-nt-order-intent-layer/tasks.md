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
