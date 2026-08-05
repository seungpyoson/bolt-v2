# Tasks: NT Order Intent Layer

> **Historical task record — not an active work queue.** Unchecked boxes below
> preserve the former feature history; do not execute them. Current `main`,
> `AGENTS.md`, and tracked issues are authoritative.

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

**Goal**: Preserve existing mixed maker/taker coverage while removing maker/taker tuple narrowing without adding a maker-only mode or venue capability table. Phase 28 supersedes the initial short-side acceptance attempt because current `binary_oracle_edge_taker` economics are long-side only.

**Independent Test**: `tests/config_parsing.rs` validates mixed maker/taker order configs through the public config validation path; Phase 28 adds the current short-side rejection regression.

- [x] T010 [US2] Confirm existing mixed maker/taker config coverage remains green before widening the contract
- [x] T011 [US2] RED: Add a config validation test for coherent short-side entry/exit in `tests/config_parsing.rs`
- [x] T012 [US2] GREEN: Replace hardcoded entry/exit tuple whitelist in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` with strategy position-contract validation
- [x] T013 [US2] HISTORICAL GREEN: The initial implementation temporarily accepted coherent short-side contracts; Phase 28 supersedes that behavior and restores current short-side rejection until strategy-owned short economics exist
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

**Status**: Reconciled by later one-variant slices. The pinned single-order `OrderFactory` surface is exhausted for this spec boundary; `MarketToLimit` and `TrailingStopLimit` remain separate approval/upstream-factory-support scope because pinned NT exposes no single-order factory methods for them.

- [x] T027 [US2] RED: Add one positive construction/admission test for each selected factory-supported NT order variant before claiming support
- [x] T028 [US2] GREEN: Enable each selected variant through the same normalized template path using NT `OrderFactory`
- [x] T029 [US2] Repeat T027-T028 one variant at a time before claiming support for additional NT factory variants

## Phase 8: GTD, OMS/Position, And Forced-Exit Gates

**Goal**: Keep high-risk execution semantics explicit instead of implied by generic order config.

- [x] T030 [US3] Add a positive GTD expiry test before claiming GTD support
- [x] T031 [US3] Add NETTING/HEDGING and reduce-only position tests before claiming position-aware submit support
- [x] T032 [US3] Add a forced-exit behavior test or record forced-exit as residual scope before completion claims

**Residual**: T031 proves NT OMS enum acceptance, submit-boundary `PositionId` threading, and reduce-only forced-exit order construction. It does not prove live adapter-specific position behavior.

## Phase 9: Adapter-Proof Harness Planning

**Goal**: Prove venue legality through NT source or strategy-free smoke, not Bolt runtime policy.

- [x] T033 [US4] Document adapter source evidence requirements for every adapter named by a support claim in `research.md`
- [x] T034 [US4] Define strategy-free smoke proof boundaries for order templates without live submit
- [x] T035 [US4] Keep live-submit proof explicitly blocked until user approval
- [x] T036 [US4] Record absence of strategy-free/live-submit artifacts as residual scope for any execution claim

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

## Phase 22: TDD Slice 18 - MarketIfTouched Exit Ledger Correction

**Goal**: Close the reviewed entry/exit narrowing gap for `MarketIfTouched` without adding venue policy or changing its NT factory construction path.

- [x] T096 [P] [US2] Record multi-agent finding that `MarketIfTouched` source/unit coverage was entry-only in public archetype validation
- [x] T097 [US2] RED: Add public exit config and raw runtime round-trip coverage for `order_type=market_if_touched`
- [x] T098 [US2] GREEN: Allow coherent `MarketIfTouched` exit configs through the same order-template path
- [x] T099 [US2] Verify focused `MarketIfTouched` exit tests, source fences as possible, and branch cleanliness

## Phase 23: TDD Slice 19 - TrailingStopMarket Factory Variant

**Goal**: Enable NT `OrderFactory::trailing_stop_market` only after adding explicit TOML-owned trailing fields, without relying on hidden NT defaults, adding venue policy, direct NT constructors, or a parallel submit path.

- [x] T100 [P] [US2] Record NT-source/Bolt-path/adversarial findings for `TrailingStopMarket`, `MarketToLimit`, and `TrailingStopLimit`
- [x] T101 [US2] RED: Add positive `TrailingStopMarket` construction/admission coverage for entry and exit with TOML-owned trigger, trigger type, trailing offset, and trailing offset type
- [x] T102 [US2] RED: Add public archetype config and raw runtime round-trip coverage for `order_type=trailing_stop_market`
- [x] T103 [US2] RED/GREEN: Add negative coverage for missing/non-positive trailing offset, missing trigger or activation input, GTD without expiry, and unsupported post-only
- [x] T104 [US2] GREEN: Construct `TrailingStopMarket` through `OrderFactory::trailing_stop_market` and validate pinned NT model invariants before factory calls
- [x] T105 [US2] Verify focused `TrailingStopMarket` tests, runtime literal/source fences as possible, full local tests as possible, branch cleanliness, exact-head gate state, and reviewer state

## Phase 24: TDD Slice 20 - Explicit TriggerType For Existing Triggered Variants

**Goal**: Resolve post-review scope ambiguity by proving optional NT `trigger_type` pass-through is intentional for already-enabled triggered factories, not accidental broadening.

- [x] T106 [P] [US2] Record review finding that `trigger_type` pass-through now applies to `StopMarket`, `MarketIfTouched`, `StopLimit`, and `LimitIfTouched`
- [x] T107 [US2] RED: Add trigger-type preservation coverage for already-enabled triggered variants and prove the tests fail if Bolt passes `None` to NT factories
- [x] T108 [US2] GREEN: Keep explicit `trigger_type` threaded to existing triggered `OrderFactory` calls through the same order-template path
- [x] T109 [US2] Verify focused trigger-type tests, source fences as possible, branch cleanliness, exact-head gate state, and reviewer state

## Phase 25: TDD Slice 21 - Remove Residual Archetype Tuple Policy

**Goal**: Replace the remaining public entry/exit maker-taker tuple allowlists with reusable enabled-order invariant validation, so public config accepts NT-model-valid order templates without encoding hardcoded maker/taker policy.

- [x] T110 [P] [US1] Record current exact-head CI/no-mistakes state and multi-agent findings for stale branch proof, tuple policy, forced-exit policy, provider binding scope, and NT factory gaps
- [x] T111 [US2] RED: Add public config coverage for a model-valid enabled order template rejected only by the residual entry/exit tuple policy
- [x] T112 [US2] GREEN: Replace entry/exit tuple allowlists with reusable enabled-order invariant validation matching the runtime NT-model checks for currently enabled order types
- [x] T113 [US2] Refactor or retire tests that asserted old tuple policy while preserving negative coverage for real NT model invariants
- [x] T114 [US2] Verify focused config/runtime tests, source fences as possible, full local tests as possible, branch cleanliness, exact-head gate state, and reviewer state

## Phase 26: TDD Slice 22 - No-Mistakes Runtime Exit Follow-Up

**Goal**: Resolve proven no-mistakes runtime findings without widening the generic order-intent layer: block exits while a resting entry can still fill, do not keep IOC/FOK entries pending after a fill, and use triggered exit config prices for triggered-market exit submission.

- [x] T115 [P] [US3] Record no-mistakes review commit evidence and local triage for pending-entry exit blocking, non-resting entry liveness, and triggered-market exit pricing
- [x] T116 [US3] RED: Add focused behavior regressions for resting pending-entry exit blocking, IOC entry fill cleanup, and triggered-market exit submission without book liquidity
- [x] T117 [US3] GREEN: Apply minimal strategy fixes on the existing exit decision and NT `OrderFactory` construction path
- [x] T118 [US3] Verify focused runtime tests, source fences as possible, full local tests as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 27: TDD Slice 23 - Active Schema Doc Consistency

**Goal**: Resolve proven active-doc drift after the order-intent implementation without changing runtime behavior: schema/status docs must describe the current NT-backed TOML surface, not the superseded netting-only and maker/taker tuple policy.

- [x] T119 [P] [US3] Record source-backed evidence for stale active docs: `oms_type`, enabled order templates, factory-gap order types, and status-map single-value enum wording
- [x] T120 [US3] RED: Add a docs consistency verifier proving the active schema/status docs still contain superseded netting-only and tuple-policy claims
- [x] T121 [US3] GREEN: Update active docs to reflect current NT-backed order template scope while preserving live/strategy-free/live-submit proof boundaries
- [x] T122 [US3] Verify the docs verifier, source fences as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 28: TDD Slice 24 - Strategy Economics Boundary After Order Template Widening

**Goal**: Resolve no-mistakes runtime findings from the tuple-policy removal: keep NT order-template breadth where the strategy can size/admit it correctly, and reject configuration that needs strategy economics not provided by NT.

- [x] T123 [P] [US3] Record no-mistakes and pinned-NT evidence for short-side economics and quote-quantity sizing/admission
- [x] T124 [US3] RED: Add public/runtime regressions for short-side rejection, exit quote-quantity rejection, entry quote-quantity sizing, quote-quantity admission notional, NT market quote/trade fallback, and active-doc overclaim detection
- [x] T125 [US3] GREEN: Apply minimal strategy fixes on the existing config validation, entry sizing, submit-admission path, and active-doc verifier
- [x] T126 [US3] Verify focused runtime/config tests, docs/source fences as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 29: TDD Slice 25 - Trailing Stop Companion Price Validation

**Goal**: Resolve no-mistakes trailing-stop validation finding without adding venue policy: any provided TrailingStopMarket `trigger_price` or `activation_price` must be positive, and at least one of them must be present.

- [x] T127 [P] [US3] Record no-mistakes review commit evidence and NT/Bolt boundary for TrailingStopMarket companion prices
- [x] T128 [US3] RED: Add config validation regressions for non-positive TrailingStopMarket trigger and activation companion prices
- [x] T129 [US3] GREEN: Apply minimal archetype validation fix on the existing order-template validation path
- [x] T130 [US3] Verify focused config tests, docs/source fences as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 30: TDD Slice 26 - Entry Reduce-Only Rejection

**Goal**: Resolve the no-mistakes reduce-only entry finding without narrowing NT order-template breadth: `binary_oracle_edge_taker` entry orders open the managed position, so `entry_order.is_reduce_only=true` must be rejected before NT submission.

- [x] T131 [P] [US3] Record no-mistakes and pinned-NT evidence for reduce-only entry semantics
- [x] T132 [US3] RED: Add public config and runtime builder regressions for reduce-only entry rejection
- [x] T133 [US3] GREEN: Reject reduce-only entry orders on the existing strategy/archetype validation paths
- [x] T134 [US3] Verify focused config/runtime tests, source fences as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 31: TDD Slice 27 - Market Expiry Rejection

**Goal**: Resolve the no-mistakes market-expiry finding without narrowing NT order-template breadth: NT `OrderFactory::market` has no expiry argument, so market order templates must reject configured `expire_time_unix_nanos` instead of accepting and dropping it.

- [x] T135 [P] [US3] Record no-mistakes evidence for market expiry being accepted and silently dropped
- [x] T136 [US3] RED: Add public config and runtime builder regressions for market order expiry rejection
- [x] T137 [US3] GREEN: Reject market order expiry on the existing strategy/archetype validation paths and update active contract/docs verifier coverage
- [x] T138 [US3] Verify focused config/runtime tests, docs/source fences as possible, branch cleanliness, exact-head gate state, and reviewer/no-mistakes state

## Phase 32: TDD Slice 28 - Triggered Exit EV Pricing

**Goal**: Resolve the no-mistakes triggered-exit EV finding by pricing normal exit EV through the same configured exit-order price path used for submission/admission, so triggered exit templates do not evaluate with a different live-book price source than the submitted order intent.

- [x] T139 [P] [US3] Record no-mistakes evidence for triggered exit EV using live-book pricing while StopMarket submission uses trigger pricing
- [x] T140 [US3] RED: Add a StopMarket exit EV regression proving the EV source is not the configured trigger price
- [x] T141 [US3] GREEN: Route exit EV pricing through configured exit-order pricing for triggered exit templates
- [x] T142 [US3] Verify focused runtime tests, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 33: TDD Slice 29 - Spec Short-Side Boundary Consistency

**Goal**: Resolve stale Speckit spec wording after Phase 28 restored short-side rejection for `binary_oracle_edge_taker`, and extend the active schema-current verifier so future spec overclaims are caught.

- [x] T143 [P] [US3] Record source-backed evidence that the spec still claimed coherent short-side acceptance while current code/docs reject it until strategy-owned short economics exist
- [x] T144 [US3] RED: Add a schema-current verifier regression proving stale spec short-side acceptance is not checked
- [x] T145 [US3] GREEN: Include `spec.md` in the schema-current verifier and update the spec acceptance scenario to the current strategy-economics boundary
- [x] T146 [US3] Verify schema-current tests, source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 34: TDD Slice 30 - TrailingStopMarket NT Default Pass-Through

**Goal**: Resolve multi-agent NT-source review finding that `TrailingStopMarket` over-validates fields NT already defaults; keep required strategy-owned trigger/activation and trailing offset inputs, but allow NT to default optional `trigger_type` and `trailing_offset_type`.

- [x] T147 [P] [US2] Record pinned NT evidence that `OrderFactory::trailing_stop_market` defaults `trigger_type` and `trailing_offset_type`
- [x] T148 [US2] RED: Add public config and runtime builder regressions for omitted optional TrailingStopMarket default fields
- [x] T149 [US2] GREEN: Remove only the extra default-field requirements while preserving required trigger/activation, positive trailing offset, GTD expiry, and post-only rejection
- [x] T150 [US2] Verify focused TrailingStopMarket tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 35: TDD Slice 31 - Non-GTD Expiry NT Pass-Through Guard

**Goal**: Reject the no-mistakes blanket non-GTD expiry patch as NT-over-narrowing, while preserving the real Market expiry rejection and adding guardrails so the stale policy does not return.

- [x] T151 [P] [US2] Record pinned NT and multi-agent evidence that non-market factories preserve `expire_time` even when TIF is not GTD
- [x] T152 [US2] RED: Add a schema-current verifier regression proving stale data-model wording still claims expiry is GTD-only
- [x] T153 [US2] GREEN: Include `data-model.md` in the verifier, update expiry wording, and add config/runtime pass-through regressions
- [x] T154 [US2] Verify focused non-GTD expiry tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 36: TDD Slice 32 - Single-Submit Live Order-Field Approval Binding

**Goal**: Treat no-mistakes `b30f4044` as a live-submit proof hardening slice, not a source order-intent gap, by binding the Phase 8 financial envelope to every currently TOML-owned entry/exit order-shape field that can change the approved order intent.

- [x] T155 [P] [US3] Record read-only local and multi-agent evidence that the current single-submit envelope binds only order type, TIF, and boolean flags
- [x] T156 [US3] RED: Add a single-submit precondition regression proving side or position-side drift can still consume approval
- [x] T157 [US3] GREEN: Bind required entry/exit side and position-side fields from loaded TOML into the financial envelope comparison
- [x] T158 [US3] RED: Add a single-submit precondition regression proving optional expiry, trigger, activation, trigger-type, trailing-offset, or trailing-offset-type drift can still consume approval
- [x] T159 [US3] GREEN: Bind optional entry/exit order-shape fields without adding order-template or venue policy
- [x] T160 [US3] Verify focused single-submit tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 37: TDD Slice 33 - Compiled Order Intent Evidence Binding

**Goal**: Close the accepted-scope evidence gap by binding `OrderIntentEvidence` to the compiled NT order fields used to explain Bolt admission, while keeping the admission outcome in the existing linked post-admission decision record.

- [x] T161 [P] [US3] Record read-only local and multi-agent evidence that `OrderIntentEvidence` lacks compiled NT order fields while `AdmissionDecisionEvidence` already records the post-gate outcome
- [x] T162 [US3] RED: Add an order-intent evidence regression proving the JSONL intent record must include selected compiled NT order fields
- [x] T163 [US3] GREEN: Populate selected order fields from the compiled NT `OrderAny` without adding venue, maker/taker, or order-template policy
- [x] T164 [US3] Update data-model wording to make the pre-admission intent record and post-admission outcome record boundary explicit
- [x] T165 [US3] Verify focused evidence tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 38: TDD Slice 34 - Submit Params Carry Boundary

**Goal**: Prove the generic submit context can carry already-typed NT `Params` to NT `submit_order` without adding adapter-key policy, global venue capability tables, or order-template fields.

- [x] T166 [P] [US3] Record pinned NT and local source evidence that `Strategy::submit_order` and adapters use optional `Params`, while current Bolt only ever builds `SubmitContext` with `params=None`
- [x] T167 [US3] RED: Add a submit-boundary regression proving a non-empty NT `Params` map must reach the emitted NT `SubmitOrder`
- [x] T168 [US3] GREEN: Add a minimal generic `SubmitContext` constructor path for already-typed params, without hardcoding adapter param keys or TOML schema
- [x] T169 [US3] Record the architecture boundary that provider bindings own concrete param names and the order-intent layer only carries typed NT params
- [x] T170 [US3] Verify focused submit-boundary tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 39: TDD Slice 35 - Trigger Instrument Boundary

**Goal**: Resolve the accepted triggered-order field gap found by pinned-NT review: enabled triggered order slices should either pass through NT `trigger_instrument_id` or explicitly residualize why the current strategy does not expose it yet. `emulation_trigger` remains a separate residual order-emulation slice unless positive tests enable it.

- [x] T171 [P] [US2] Record pinned NT and local source evidence for `trigger_instrument_id` and `emulation_trigger` against enabled factory variants
- [x] T172 [US2] RED: Add public/runtime regressions for the chosen `trigger_instrument_id` boundary before production code changes
- [x] T173 [US2] GREEN: Either pass TOML-owned `trigger_instrument_id` through the existing order-template path or update the contract to residualize it without claiming full triggered-field support
- [x] T174 [US2] Record `emulation_trigger` as an explicit deferred order-emulation slice unless it is enabled through TDD in this phase
- [x] T175 [US2] Verify focused triggered-order tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 40: TDD Slice 36 - Forced-Flat Pending Entry Review Blocker

**Goal**: Resolve the Greptile finding that a resting GTC/GTD pending entry can suppress forced-flat exit submission for an already-open managed position. Normal exits still block while the entry remainder may fill; forced-flat liquidation must take precedence.

- [x] T176 [P] [US3] Record source and multi-agent evidence that the pending-entry block returned before forced-flat evaluation
- [x] T177 [US3] RED: Split the existing pending-entry regression into normal-exit blocking and forced-flat override coverage
- [x] T178 [US3] GREEN: Give forced-flat exit precedence over the managed pending-entry guard without changing the normal-exit guard
- [x] T179 [US3] RED: Add a forced-flat submit lifecycle regression proving NT cancel emission for the resting entry and residual recovery if an entry fill races while exit is pending
- [x] T180 [US3] GREEN: Use NT `cancel_order(...)` for forced-flat pending-entry cancellation and recover raced fills as residual managed exposure
- [x] T181 [US3] Verify focused exit lifecycle tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 41: TDD Slice 37 - Strategy/Venue/Market Agnostic NT Order Template

**Goal**: Extract only the reusable NT order-template mechanics from the current `binary_oracle_edge_taker` implementation. The shared layer may validate NT model invariants and build orders through NT `OrderFactory`; it must not know strategy IDs, strategy archetypes, venue/provider names, market families, entry/exit economics, submit/admission policy, or live support claims.

- [x] T182 [P] [US2] Record source and multi-agent evidence that generic NT order-template validation/building is still housed inside `binary_oracle_edge_taker`
- [x] T183 [US2] RED: Add a strategy/venue/market agnostic regression proving the shared order-template module builds NT orders from `OrderFactory` without submission, admission, archetype, or provider dependencies
- [x] T184 [US2] GREEN: Move NT order-template fields, validation, and `OrderFactory` construction into a shared module that accepts typed NT inputs and an NT `OrderFactory`
- [x] T185 [US2] Wire `binary_oracle_edge_taker` to the shared builder while leaving position-contract checks, entry reduce-only rejection, exit quote-quantity sizing, forced-flat behavior, evidence, admission, and submit context in the strategy-owned path
- [x] T186 [US2] Verify generic order-template tests, focused strategy regressions, source fences for forbidden coupling, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 42: TDD Slice 38 - Latest-Head External Review Runtime Guard

**Goal**: Record exact-head external review state after the shared extraction, and harden the public shared builder against direct callers that bypass config-time validation. The fix must remain pure NT model validation and must not add venue, market, strategy, maker-only, or taker-only policy.

- [x] T187 [P] [US2] Run latest-head external reviews against PR #434 head `f7e873bb3906cf4c9842107f941e7dd728a4031a` and record clean, failed, and non-transmitted review slots separately
- [x] T188 [US2] RED: Add a direct shared-builder regression proving non-positive trigger or activation inputs can reach NT factory construction
- [x] T189 [US2] GREEN: Mirror trigger and activation positivity validation in `validate_nt_order_template(...)` before `OrderFactory` calls
- [x] T190 [US2] Verify focused order-template tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state

## Phase 43: TDD Slice 39 - Latest-Head Review Coverage Closure

**Goal**: Close Claude latest-head NB3 by making the direct shared-builder regression cover every enabled triggered NT factory variant affected by the Phase 42 runtime validation gap. This is regression coverage only unless the expanded test exposes a production behavior gap.

- [x] T191 [P] [US2] Record exact-head Claude NB3 that shared-builder direct coverage only exercised StopMarket zero-trigger and TrailingStopMarket zero-activation
- [x] T192 [US2] REGRESSION: Expand direct shared-builder coverage for StopLimit, MarketIfTouched, LimitIfTouched, and TrailingStopMarket zero-trigger rejection
- [x] T193 [US2] Verify focused order-template tests, source fences, formatting, branch cleanliness, and latest-head reviewer state

## Phase 44: TDD Slice 40 - Direct Shared-Builder Validation Matrix Closure

**Goal**: Close latest-head Claude non-blocking direct-builder coverage concerns without changing production behavior unless tests expose a gap. Direct callers of `build_nt_order(...)` should have regression coverage for negative trigger/activation values and the other generic NT model invariants enforced by `validate_nt_order_template(...)`.

- [x] T194 [P] [US2] Record latest-head external review state for PR #434 head `3e3679f3763c59139041bb36bff460f05136668d`
- [x] T195 [US2] REGRESSION: Add direct shared-builder negative trigger/activation rejection coverage
- [x] T196 [US2] REGRESSION: Add direct shared-builder coverage for GTD-without-expiry, Market GTD rejection, TrailingStopMarket post-only rejection, LimitIfTouched side/price rejection, and non-triggered trigger rejection
- [x] T197 [US2] Verify focused order-template tests, source fences, formatting, branch cleanliness, CI state, and latest-head reviewer state

## Phase 45: TDD Slice 41 - Latest-Head Direct Validation Residual Closure

**Goal**: Close latest-head Claude non-blocking residuals by directly testing the remaining generic shared-builder validation arms and order-arm post-only fail-closed checks. This is regression coverage only unless the expanded tests expose a production behavior gap.

- [x] T198 [P] [US2] Record latest-head Gemini and Claude review state for PR #434 head `e48c5493321ec3cd05b7d2baa6010720527f22a2`
- [x] T199 [US2] REGRESSION: Add direct shared-builder coverage for Market expiry rejection, missing trigger prices, non-trailing activation/trailing fields, non-triggered trigger type/instrument fields, and TrailingStopMarket trigger/trailing-offset requirements
- [x] T200 [US2] REGRESSION: Add direct post-only fail-closed coverage for Market, StopMarket, and MarketIfTouched order-arm invariants and make the source fence less brittle for `Entry`/`Exit` tokens
- [x] T201 [US2] Verify focused order-template tests, source fences, formatting, branch cleanliness, CI state, and latest-head reviewer state

## Phase 46: TDD Slice 42 - Latest-Head Review Disposition Refresh

**Goal**: Refresh latest-head multi-agent review evidence after Phase 45, resolve or disprove review findings without adding Bolt-only NT narrowing, and add regression coverage where the implementation is already correct.

- [x] T202 [P] [US2] Record latest-head Gemini, Claude, Kimi, and Grok review slots for PR #434 head `9c57563d9717642d643f4c59d789a69ea64e588d`
- [x] T203 [US2] Disprove the Claude runtime-literal/source-fence concern with exact-head `just source-fence`
- [x] T204 [US2] REGRESSION: Add direct shared-builder coverage proving `trigger_instrument_id` is preserved for every enabled triggered NT factory
- [x] T205 [US2] Disposition the non-positive limit-price suggestion against pinned NT evidence without adding a shared-layer price policy
- [x] T206 [US2] Verify focused order-template tests, source fences, formatting, branch cleanliness, exact-head CI state, and reviewer state

## Phase 47: TDD Slice 43 - TOML-Owned Forced-Exit Order Template

**Goal**: Resolve current-head architecture review evidence that forced-flat exit submission still synthesizes a market-order template in strategy code before calling the shared NT builder. Forced-exit urgency remains strategy-owned, but forced-exit order semantics must be TOML-owned NT order-template data and must continue through the shared `build_nt_order(...)` path without venue, market, or maker/taker policy.

- [x] T207 [P] [US3] Record current-head source and multi-agent evidence that forced-flat exit order semantics are still hardcoded before the shared builder
- [x] T208 [US3] RED: Add public/runtime regressions proving a configured `forced_exit_order` template is accepted and used for forced-flat submission
- [x] T209 [US3] GREEN: Add a single TOML-owned forced-exit order template path and remove the hardcoded forced-flat market-order synthesis
- [x] T210 [US3] Verify focused forced-flat/config/order-intent tests, source fences, formatting, branch cleanliness, exact-head PR state, and reviewer/no-mistakes state

## Phase 48: TDD Slice 44 - Forced-Exit Schema Drift And NT Manage-Stop Boundary

**Goal**: Resolve latest-head review evidence that active schema docs still describe the removed market-exit fields, and that `manage_stop=true` can silently route a configured non-market `forced_exit_order` through NT's built-in market close path. The fix must document the current TOML-owned forced-exit template and fail closed when NT `manage_stop` cannot honor the configured forced-exit order semantics.

- [x] T211 [P] [US3] Record latest-head multi-agent evidence for stale schema docs and NT `manage_stop` market-close behavior
- [x] T212 [US3] RED: Add schema-current verifier coverage for removed market-exit fields and required `forced_exit_order` docs
- [x] T213 [US3] RED: Add public config validation coverage that rejects `manage_stop=true` with a non-market `forced_exit_order`
- [x] T214 [US3] GREEN: Update active schema docs/verifier and add the NT manage-stop compatibility guard without adding venue or maker/taker policy
- [x] T215 [US3] Verify focused schema/config tests, source fences, formatting, branch cleanliness, exact-head PR state, and reviewer/no-mistakes state

## Phase 49: TDD Slice 45 - NT Order Model Surface Gap Review

**Goal**: Investigate latest-head review evidence that the shared order-intent layer still follows the pinned NT single-order `OrderFactory` surface rather than every NT model builder variant, and that shared build inputs require a selected price even for NT market-like constructors that do not take a limit price. No implementation is approved until pinned NT source, strategy economics, submit/admission requirements, and TDD proof define the smallest architecture-safe slice.

- [x] T216 [P] [US2] Record pinned NT evidence for `MarketToLimit`, `TrailingStopLimit`, and order-builder versus order-factory construction paths
- [x] T217 [P] [US2] Record current Bolt evidence for mandatory `price` in shared build inputs and strategy admission/evidence dependencies
- [x] T218 [US2] Decide, with evidence, whether the next implementation slice should expand beyond `OrderFactory`, make runtime price optional for market-like NT constructors, or keep the current boundary as explicit residual scope
- [x] T219 [US2] Add RED tests only after the architecture decision identifies a concrete behavior gap

## Phase 50: TDD Slice 46 - Current-Head Maker Lifecycle And Sizing Review

**Goal**: Resolve current-head Greptile/PR-body evidence that maker entry sizing and external close cleanup still use taker/dead-letter semantics after the order-intent implementation. The fix must stay strategy-owned, use NT cancel APIs for live order lifecycle, and not add venue, market, maker-only, or taker-only policy to the shared order-intent layer.

- [x] T220 [P] [US3] Record current-head evidence for stale PR-body review state, maker book-impact cap side selection, and Managed-position pending-entry close cleanup
- [x] T221 [US3] RED: Add regression proving post-only entry book-impact cap derives depth from the passive book side
- [x] T222 [US3] RED: Add regression proving Managed external position close cancels a resting pending entry before flattening
- [x] T223 [US3] GREEN: Fix strategy-owned sizing and lifecycle paths without changing shared NT order construction
- [x] T224 [US3] Verify focused strategy tests, source fences, formatting, branch cleanliness, exact-head PR checks, and reviewer/no-mistakes state

## Phase 51: TDD Slice 47 - TrailingStopMarket Schema Default Drift

**Goal**: Resolve current-head multi-agent evidence that active schema docs still require `trigger_type` and `trailing_offset_type` for `trailing_stop_market` even though pinned NT and current code default those optional fields. The fix is docs/verifier-only and must not change runtime order construction, add venue policy, or narrow NT order-template support.

- [x] T225 [P] [US2] Record current-head multi-agent and pinned NT evidence for optional `TrailingStopMarket` default fields
- [x] T226 [US2] RED: Add schema-current verifier regression proving stale required default-field wording is not caught
- [x] T227 [US2] GREEN: Update the verifier and active schema docs to describe NT default pass-through for omitted `trigger_type` and `trailing_offset_type`
- [x] T228 [US2] Verify schema-current tests, active schema verifier, formatting/diff checks, branch cleanliness, exact-head PR checks, and reviewer/no-mistakes state
- [x] T229 [US2] RED: Add schema-current verifier regression for equivalent required/default-field wording that does not reuse the exact stale line
- [x] T230 [US2] GREEN: Generalize the verifier to reject equivalent TrailingStopMarket default-field requirement wording without flagging optional/default-pass-through wording

## Phase 52: TDD Slice 48 - Speckit Dependency Note Closure

**Goal**: Resolve current-head Speckit drift where dependency notes still describe completed Phase 47, Phase 48, and Phase 50 findings as active completion blockers and omit the Phase 51 dependency note. The fix is docs/verifier-only and must not change runtime order construction or reset source/unit support boundaries.

- [x] T231 [US3] RED: Add schema-current verifier regressions proving stale completed-phase dependency wording and missing Phase 51 tracking are not caught in `tasks.md`
- [x] T232 [US3] GREEN: Update the verifier and Speckit dependency notes so completed Phase 47/48/50/51 slices are not recorded as active blockers
- [x] T233 [US3] Verify schema-current tests, active verifier, formatting/diff checks, branch cleanliness, exact-head PR checks, and reviewer/no-mistakes state

## Phase 53: TDD Slice 49 - Speckit Phase 34 Dependency Closure

**Goal**: Resolve current-head Speckit drift where the dependency notes still describe the completed Phase 34 TrailingStopMarket NT-default finding as an active completion blocker even though Phase 34 and Phase 51 now record the implementation, schema, and verifier closure. The fix is docs/verifier-only and must not change runtime order construction or narrow NT order-template support.

- [x] T234 [US3] RED: Add schema-current verifier regression proving stale completed-Phase-34 dependency wording is not caught in `tasks.md`
- [x] T235 [US3] GREEN: Update the verifier and Speckit dependency note so Phase 34 is recorded as closed rather than an active blocker
- [x] T236 [US3] Verify schema-current tests, active verifier, formatting/diff checks, source fences, branch cleanliness, exact-head PR checks, and reviewer/no-mistakes state

## Phase 54: TDD Slice 50 - Forced-Exit Approval Envelope Binding

**Goal**: Resolve exact-head no-mistakes evidence that Phase 8 approval envelopes bind entry and normal-exit order-shape fields but not the TOML-owned `forced_exit_order` introduced by Phase 47. The fix is live-submit approval-envelope hardening only; it must not change runtime order construction, add venue policy, or touch the shared order-intent layer.

- [x] T237 [P] [US3] Record exact-head no-mistakes evidence for the forced-exit approval-envelope gap and the live-submit boundary decision
- [x] T238 [US3] RED: Add an approval-envelope regression proving `forced_exit_order` drift can consume approval
- [x] T239 [US3] GREEN: Bind required and optional `forced_exit_order` order-shape fields in `Phase8FinancialEnvelopeEvidenceFile`
- [x] T240 [US3] Verify focused single-submit tests, schema/source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, and reviewer/no-mistakes state

## Phase 55: TDD Slice 51 - Active Speckit Context Pointer

**Goal**: Resolve current-head Speckit workflow drift where the checked-in active feature pointer still targets `specs/023-nt-research-analytics-platform/` even though this branch's controlling plan/tasks are `specs/023-nt-order-intent-layer/`. The fix is docs/verifier-only and must not change runtime order construction or narrow NT order-template support.

- [x] T241 [US3] RED: Add schema-current verifier regression proving wrong `AGENTS.md` and `.specify/feature.json` active Speckit pointers are not caught
- [x] T242 [US3] GREEN: Update the verifier and active Speckit context pointers so future `$speckit-plan` and `$speckit-tasks` commands target `specs/023-nt-order-intent-layer/`
- [x] T243 [US3] Verify schema-current tests, active verifier, formatting/diff checks, branch cleanliness, and record the post-push exact-head PR/reviewer/no-mistakes refresh boundary

## Phase 56: TDD Slice 52 - Active Speckit Context Malformed JSON Regression

**Goal**: Close the external-review coverage gap for malformed `.specify/feature.json` without changing runtime order construction, NT order-template mapping, or venue/market/strategy behavior. The verifier already rejects malformed JSON; this slice makes that branch explicit regression coverage.

- [x] T244 [US3] Add schema-current verifier regression proving malformed `.specify/feature.json` reports a finding without crashing
- [x] T245 [US3] Verify schema-current tests, active verifier, formatting/diff checks, branch cleanliness, and record the post-push exact-head PR/reviewer/no-mistakes refresh boundary

## Phase 57: Refactor Slice 53 - Strategy-Local NT Template Mapping Dedupe

**Goal**: Resolve exact-head no-mistakes evidence that entry and exit copied the same NT order-template field mapping in strategy code. The fix is a behavior-preserving refactor only: it must not add source-count tests, runtime hardcodes, venue policy, market policy, strategy admission policy, or any new shared order-intent responsibility.

- [x] T246 [P] [US3] Record exact-head no-mistakes evidence and boundary decision for duplicated strategy-local NT template mapping
- [x] T247 [US3] REFACTOR: Centralize strategy-local configured NT template field mapping while leaving shared `build_nt_order(...)` strategy/venue/market agnostic
- [x] T248 [US3] Verify focused maker/forced-exit/order-intent tests, schema/source fences, formatting/diff checks, branch cleanliness, and record the post-push exact-head PR/reviewer/no-mistakes refresh boundary

## Phase 58: Review Slice 54 - OMS Finding Disposition

**Goal**: Resolve exact-head no-mistakes OMS evidence without adding a Bolt-only OMS allowlist. The disposition must be source-backed against pinned NT and current Bolt paths, and must keep OMS support venue/market/strategy agnostic unless a real runtime/accounting failure is proven.

- [x] T249 [P] [US3] Record exact-head no-mistakes OMS evidence in `specs/023-nt-order-intent-layer/research.md`
- [x] T250 [P] [US3] Record pinned NT OMS source evidence for enum support, position-id validation, fill position-id determination, and adapter OMS choices
- [x] T251 [US3] Confirm existing public/runtime regressions keep Bolt from narrowing NT `oms_type`
- [x] T252 [US3] Record Gemini/Claude focused OMS review disposition and boundary decision not to add a Netting-only guard
- [x] T253 [US3] Verify focused OMS/config tests, schema/source fences, formatting/diff checks, branch cleanliness, exact-head PR state, reviewer state, and no-mistakes state

## Phase 59: TDD Slice 55 - Direct Validator Post-Only Completeness

**Goal**: Resolve current-head GLM review evidence that direct callers of `validate_nt_order_template(...)` do not receive the same market-like post-only rejection as `build_nt_order(...)`. The fix must stay inside the shared NT model-invariant validator and must not add venue, market, strategy, maker-only, or taker-only policy.

- [x] T254 [P] [US2] Record exact-head GLM/Claude/no-mistakes review state and the validator-completeness finding in `specs/023-nt-order-intent-layer/research.md`
- [x] T255 [US2] RED: Add direct `validate_nt_order_template(...)` regression coverage for post-only `Market`, `StopMarket`, and `MarketIfTouched`
- [x] T256 [US2] GREEN: Move those post-only rejections into `validate_nt_order_template(...)` while preserving existing `OrderFactory` construction behavior
- [x] T257 [US2] RED: Add source-fence coverage proving the shared order-intent module does not hardcode archetype TOML `parameters.*` diagnostics
- [x] T258 [US2] GREEN: Let archetype callers pass the full config field path into the shared validator and add direct unsupported-variant coverage for the shared builder
- [x] T259 [US2] Verify focused shared-builder tests, schema/source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 60: TDD Slice 56 - Direct Validator Factory-Gap Closure

**Goal**: Resolve exact-head Claude/Kimi review evidence that unsupported NT factory-gap variants still validate successfully through the direct shared validator, and remove duplicate/shared-maintenance builder diagnostics without adding venue, market, strategy, maker-only, or taker-only policy.

- [x] T260 [P] [US2] Record exact-head Gemini/Claude/Kimi/no-mistakes review state and the direct-validator factory-gap finding in `specs/023-nt-order-intent-layer/research.md`
- [x] T261 [US2] RED: Add direct `validate_nt_order_template(...)` regression coverage for unsupported pinned NT single-order factory-gap variants
- [x] T262 [US2] RED: Add source/error coverage proving TrailingStopMarket post-only rejection is not duplicated outside direct validation and unsupported builder diagnostics do not hardcode a supported-type list
- [x] T263 [US2] GREEN: Move unsupported factory-gap rejection into `validate_nt_order_template(...)`, make the builder catch-all use the same pinned-NT boundary message, and remove the duplicate TrailingStopMarket post-only builder guard
- [x] T264 [US2] Verify focused shared-builder tests, schema/source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 61: TDD Slice 57 - Phase 8 OMS Approval Binding

**Goal**: Resolve no-mistakes review evidence that Phase 8 financial-envelope approval did not bind the loaded NT `oms_type`, and close the concrete Kimi coverage concern for direct config-checker unsupported factory-gap variants without adding venue, market, strategy, maker-only, or taker-only policy.

- [x] T265 [P] [US2] Record no-mistakes review commit `4688ccb4` and exact-head reviewer state in `specs/023-nt-order-intent-layer/research.md`
- [x] T266 [US2] RED: Add Phase 8 financial-envelope regression proving a loaded NT `oms_type` mismatch must fail closed before approval consumption
- [x] T267 [US2] GREEN: Bind `oms_type` in Phase 8 financial-envelope evidence and compare it against the loaded TOML-derived NT OMS enum value
- [x] T268 [US2] GREEN: Add direct `check_nt_order_template_config(...)` regression for unsupported pinned NT single-order factory-gap variants
- [x] T269 [US2] Verify focused Phase 8/order-intent tests, schema/source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 62: TDD Slice 58 - Phase 8 OMS Canonicalization

**Goal**: Resolve current-head Kimi review evidence that Phase 8 approval-envelope `oms_type` comparison used raw string equality instead of NT enum canonicalization. The fix must keep the operator envelope NT-backed and must not add a Bolt-only OMS allowlist or venue, market, strategy, maker-only, or taker-only policy.

- [x] T270 [P] [US2] Record Kimi exact-diff review state and disposition for the OMS canonicalization, validation-split, and source-fence concerns in `specs/023-nt-order-intent-layer/research.md`
- [x] T271 [US2] RED: Add approval-envelope regression proving an NT-equivalent `oms_type` spelling should validate against the loaded TOML-derived OMS enum
- [x] T272 [US2] GREEN: Canonicalize approved financial-envelope `oms_type` through NautilusTrader `OmsType` parsing before comparison
- [x] T273 [US2] Document the Phase 8 approval-envelope OMS canonicalization and confirm the fail-closed diagnostic remains verifier-ignored diagnostic text
- [x] T274 [US2] Verify focused Phase 8 test, schema/runtime verifiers, formatting/diff checks, full relevant suites, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 63: Review Slice 59 - Phase 8 OMS Coverage Closure

**Goal**: Resolve or disprove Kimi's exact-head OMS coverage concerns without changing runtime behavior or adding Bolt-only OMS policy.

- [x] T275 [P] [US2] Record Kimi exact-diff review state and separate real coverage work from disproven helper concern in `specs/023-nt-order-intent-layer/research.md`
- [x] T276 [US2] Add direct regression coverage proving invalid financial-envelope `oms_type` strings fail closed before approval consumption
- [x] T277 [US2] Add direct disproof coverage proving the OMS alternate helper covers every current NT `OmsType` variant used by the test
- [x] T278 [US2] Verify focused Phase 8 tests, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 64: TDD Slice 60 - Phase 8 Order Enum Canonicalization

**Goal**: Resolve no-mistakes review evidence that Phase 8 financial-envelope order-shape enum comparisons canonicalized `oms_type` but still compared entry, exit, and forced-exit order enum fields as raw strings. The fix must parse through NautilusTrader enum types, must not add Bolt-owned enum value allowlists, and must not add venue, market, strategy, maker-only, or taker-only policy.

- [x] T279 [P] [US2] Record no-mistakes review commit `386850dc` and reject its hardcoded test-value shape in `specs/023-nt-order-intent-layer/research.md`
- [x] T280 [US2] RED: Add Phase 8 financial-envelope regression proving NT-equivalent order enum spellings validate and invalid approved order enum strings fail closed before approval consumption
- [x] T281 [US2] GREEN: Canonicalize entry, exit, and forced-exit `OrderSide`, `PositionSide`, `OrderType`, `TimeInForce`, `TriggerType`, and `TrailingOffsetType` values through pinned NautilusTrader enum parsing before approval comparison
- [ ] T282 [US2] Verify focused Phase 8 test, formatting/diff checks, schema/runtime verifiers, source fences, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 65: TDD Slice 61 - Activation Price Admission Source

**Goal**: Resolve no-mistakes review evidence for activation-price admission evidence without adding venue, market, strategy, maker-only, taker-only, or order-type policy. The fix must use NautilusTrader order fields generically and keep strategy admission on the same compiled-order evidence path.

- [x] T283 [P] [US2] Record no-mistakes review commit `c4adfa05` and pinned NautilusTrader activation-price evidence in `specs/023-nt-order-intent-layer/research.md`
- [x] T284 [US2] RED: Add a central compiled-order price-source regression proving activation price is selected before caller fallback when NT `price` and `trigger_price` are absent
- [x] T285 [US2] GREEN: Centralize compiled-order price-source selection as `price -> trigger_price -> activation_price -> fallback` and reuse it for order-intent evidence plus submit-admission notional
- [ ] T286 [US2] Verify focused admission tests, schema/runtime verifiers, source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 66: TDD Slice 62 - Source-Derived Financial Envelope Schema Guard

**Goal**: Resolve no-mistakes review evidence for Phase 8 financial-envelope schema drift without adding another static schema field allowlist. The verifier must derive the required fields from the Rust `Phase8FinancialEnvelopeEvidenceFile` source struct and compare the active schema docs against that source of truth.

- [x] T287 [P] [US2] Record no-mistakes review commit `d537b295` and financial-envelope schema drift evidence in `specs/023-nt-order-intent-layer/research.md`
- [x] T288 [US2] RED: Add a verifier regression proving active schema docs must include every field from the Rust `Phase8FinancialEnvelopeEvidenceFile` source struct
- [x] T289 [US2] GREEN: Derive required financial-envelope schema fields from the Rust source struct and update docs to match current Phase 8 financial-envelope source
- [ ] T290 [US2] Verify schema tests/verifier, focused admission tests, schema/runtime verifiers, source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 67: TDD Slice 63 - Exact Financial Envelope Schema Field Set

**Goal**: Resolve exact-head Kimi review evidence that the Phase 66 schema guard was one-directional. The active verifier must reject both missing source fields and stale doc-only financial-envelope fields.

- [x] T291 [P] [US2] Record Kimi exact-head review finding for one-directional `financial_envelope` schema validation in `specs/023-nt-order-intent-layer/research.md`
- [x] T292 [US2] RED: Add a verifier regression proving stale doc-only `financial_envelope` fields are rejected
- [x] T293 [US2] GREEN: Parse documented `financial_envelope` field names and compare them against the Rust source-derived field set in both directions
- [ ] T294 [US2] Verify schema tests/verifier, focused admission tests, schema/runtime verifiers, source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 68: TDD Slice 64 - No-Mistakes Documentation Drift Closure

**Goal**: Resolve exact-head no-mistakes documentation findings without changing runtime behavior or adding venue, market, strategy, maker-only, or taker-only policy. The active verifier must cover decision-evidence JSONL schema v4 fields, runtime order-template evidence fields, forced-exit builder status evidence, and stale Phase 022 maker-scope supersession text.

- [x] T295 [P] [US2] Record no-mistakes document findings for JSONL schema v4, runtime order-template fields, forced-exit builder status, and stale Phase 022 forced-exit/GTD text in `specs/023-nt-order-intent-layer/research.md`
- [x] T296 [US2] RED: Add a schema-current verifier regression proving those documentation drift cases are rejected
- [x] T297 [US2] GREEN: Extend the active verifier and update only the affected schema/runtime/status/spec docs
- [ ] T298 [US2] Verify schema tests/verifier, runtime-literal verifier, formatting/diff checks, source fences as needed, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 69: TDD Slice 65 - Strategy Schema Version Boundary

**Goal**: Resolve exact-head no-mistakes review evidence that the strategy file still advertises schema version `1` after the forced-exit order-template schema change. The fix must version the strategy file shape only, must not change the root schema, and must not add venue, market, strategy, maker-only, or taker-only policy.

- [x] T299 [P] [US2] Record no-mistakes review findings for OMS widening and strategy schema versioning in `specs/023-nt-order-intent-layer/research.md`
- [x] T300 [US2] RED: Add a config validation regression proving previous strategy schema version `1` is rejected after the forced-exit schema update
- [x] T301 [US2] GREEN: Bump the supported strategy schema version and current strategy schema docs/fixtures without changing root schema version
- [ ] T302 [US2] Verify focused config tests, schema/runtime verifiers, source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 70: TDD Slice 66 - Example Config And Decision Evidence Schema Follow-Up

**Goal**: Resolve exact-head no-mistakes review evidence that the shipped example strategy config still uses the previous strategy schema version and that active decision-evidence docs describe `record_type` although the serializer emits `kind` plus `intent` or `decision`. The fix must align examples and docs to source only, without changing runtime evidence shape or adding venue, market, strategy, maker-only, or taker-only policy.

- [x] T303 [P] [US2] Record no-mistakes review findings for stale example schema version and decision-evidence envelope wording in `specs/023-nt-order-intent-layer/research.md`
- [x] T304 [US2] RED: Add regressions proving shipped examples use the supported strategy schema version and active docs reject stale `record_type` wording
- [x] T305 [US2] GREEN: Update shipped example strategy schema version and decision-evidence docs/verifier to the actual serializer envelope
- [ ] T306 [US2] Verify focused config/doc tests, schema/runtime verifiers, source fences, formatting/diff checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 71: TDD Slice 67 - External Review Clippy Follow-Up

**Goal**: Resolve exact-head Gemini relay evidence that all-targets clippy fails on test-only helper/assertion patterns, without changing runtime behavior or adding venue, market, strategy, maker-only, or taker-only policy.

- [x] T307 [P] [US2] Record Gemini relay, Claude/Gemini/Kimi/Grok, and no-mistakes review state in `specs/023-nt-order-intent-layer/research.md`
- [x] T308 [US2] RED: Reproduce all-targets clippy failures for equal-operand zero-price construction, boolean assertion comparisons, and unit let-bindings
- [x] T309 [US2] GREEN: Apply mechanical test-only clippy fixes without changing order-intent runtime behavior
- [ ] T310 [US2] Verify focused tests, all-targets clippy, formatting/diff checks, schema/runtime verifiers, source fences, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Phase 72: TDD Slice 68 - Shared Position-Contract Helpers

**Goal**: Resolve exact-head no-mistakes/Greptile evidence that `binary_oracle_edge_taker` archetype and strategy modules define identical position-contract helper functions, without changing runtime order construction, admission, venue policy, market policy, strategy policy, maker-only policy, or taker-only policy.

- [x] T311 [P] [US2] Record exact-head no-mistakes/Greptile helper-duplication evidence and the separate P1 generic admission-wrapper follow-up issue in `specs/023-nt-order-intent-layer/research.md`
- [x] T312 [US2] RED: Add a source-derived schema-current verifier guard proving duplicated position-contract helper definitions are rejected
- [x] T313 [US2] GREEN: Move the shared position-contract helper definitions to one neutral Bolt module and import them from the archetype and strategy modules
- [ ] T314 [US2] Verify schema-current tests/verifier, focused position-contract regressions, formatting/diff checks, source fences, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state

## Dependencies & Execution Order

- Phase 1 blocks implementation.
- Phase 2 blocks implementation.
- Phase 3 is the first implementation slice and must complete before later slices.
- Phase 4 depends on Phase 3 because it changes the same order config path.
- Phase 5 depends on the normalized template from Phase 4.
- Phase 6 depends on compiled order behavior from Phase 4 and validation from Phase 5.
- Phase 7 blocks broad NT order-variant support claims until reconciled against the pinned single-order `OrderFactory` surface.
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
- Phase 22 blocks broad MarketIfTouched source/unit support claims because public exit validation was still narrowed to reject it.
- Phase 23 blocks TrailingStopMarket source/unit support claims until explicit trailing fields are tested, constructed, and reviewed.
- Phase 24 blocks completion because post-review found `trigger_type` pass-through was broader than the TrailingStopMarket task text documented.
- Phase 25 blocks completion until focused/local checks, reviewer findings, branch cleanliness, and post-push exact-head gate state are recorded for the tuple-policy removal.
- Phase 26 blocks completion because no-mistakes produced runtime exit findings on top of the current branch.
- Phase 27 blocks completion because no-mistakes and direct source inspection found active documentation still describing superseded config scope after the order-intent implementation.
- Phase 28 blocks completion because no-mistakes found tuple-policy removal admitted short-side and quote-quantity configurations beyond current strategy economics and sizing/admission proof.
- Phase 31 blocks completion because no-mistakes found market order expiry configs were accepted even though the pinned NT market factory drops expiry.
- Phase 32 blocks completion because no-mistakes found triggered exit EV was priced from the live book while the configured StopMarket exit order submits with trigger pricing.
- Phase 33 blocks completion because source inspection found the Speckit spec still claimed short-side acceptance after Phase 28 restored current strategy-economics rejection.
- Phase 34 closes the original TrailingStopMarket NT-default pass-through implementation; Phase 51 closes the active schema-default drift and equivalent-wording verifier gap.
- Phase 35 blocks completion because no-mistakes produced a non-terminal blanket non-GTD expiry patch that conflicts with pinned NT Rust behavior and prior Speckit decisions.
- Phase 36 blocks live-submit proof claims because no-mistakes and read-only reviewers found the Phase 8 financial envelope did not bind every currently TOML-owned order-shape field.
- Phase 37 blocks completion because read-only audit found the pre-admission order-intent evidence did not include the compiled NT order fields needed to explain Bolt admission.
- Phase 38 blocks submit-context completion claims because current source inspection found no path that can set non-empty NT submit params.
- Phase 39 blocks triggered-order field-completeness claims because pinned-NT review found `trigger_instrument_id` is accepted by enabled NT factories but currently dropped by Bolt, and `emulation_trigger` is not listed as residual scope.
- Phase 40 blocks completion because Greptile found forced-flat exit submission was still blocked behind a resting managed pending-entry remainder.
- Phase 41 blocks completion because multi-agent review found generic NT order-template mechanics still housed in `binary_oracle_edge_taker`; extraction must remain submission-agnostic, venue-agnostic, market-agnostic, and strategy-agnostic.
- Phase 42 blocks completion because latest-head external review exposed a public shared-builder validation asymmetry after Phase 41 extraction.
- Phase 43 blocks completion because latest-head Claude review found the Phase 42 direct shared-builder regression did not enumerate every affected triggered factory variant.
- Phase 44 blocks completion because latest-head Claude review found more direct shared-builder invariants still covered only indirectly through strategy/config paths.
- Phase 45 blocks completion because latest-head Claude review found remaining direct-builder validation arms and order-arm post-only fail-closed checks without direct shared-builder coverage.
- Phase 46 closes the latest-head Claude review disposition because the real triggered-factory test gap is covered directly and the proposed price validation change was rejected against pinned NT evidence.
- Phase 47 closes the forced-exit order-template synthesis finding by routing forced-flat order semantics through TOML-owned `forced_exit_order` and the shared NT builder path.
- Phase 48 closes the removed market-exit schema drift and `manage_stop=true` non-market forced-exit gap with active schema/verifier coverage and fail-closed config validation.
- Phase 49 blocks broad "all NT order model surface" claims until pinned NT builder-vs-factory evidence and a TDD slice resolve or explicitly scope the remaining model-surface and runtime-price findings.
- Phase 50 is closed by T224 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.
- Phase 51 is closed by T228 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.
- Phase 52 is closed by T233 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.
- Phase 53 is closed by T236 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.
- Phase 54 is closed by T240 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.
- Phase 55 is closed by T243 verification at 2026-05-22 14:03:07 KST: schema-current self-tests and active verifier passed after RED coverage for stale active pointers, outside-block historical pointers, same-block false negatives, empty context files, and non-object feature JSON; `git diff --check`, `cargo fmt -- --check`, and `just source-fence` passed; Franklin and Kant reported no findings after Ampere and Beauvoir findings were fixed. Exact-head PR/reviewer/no-mistakes state must be refreshed after committing and pushing this docs/verifier-only follow-up head.
- Phase 56 is closed by T245 verification at 2026-05-22 14:39:09 KST: schema-current self-tests and active verifier passed with malformed `.specify/feature.json` regression coverage. Exact-head PR/reviewer/no-mistakes state remains a post-push evidence surface, not a pre-push local verification claim.
- Phase 57 is closed by T248 verification at 2026-05-22 14:39:09 KST: focused maker entry/exit, forced-exit template, shared order-intent source fence, schema/runtime verifiers, `cargo fmt -- --check`, `git diff --check`, `just source-fence` after cache-lock escalation, `cargo test --locked --quiet`, and `just clippy` passed locally. Exact-head PR/reviewer/no-mistakes state must be refreshed after committing and pushing this follow-up head.
- Phase 58 is closed by T253 verification at 2026-05-22 15:37:40 KST: focused OMS regressions, full config parsing, schema-current verifier, `cargo fmt -- --check`, `git diff --check`, `just source-fence` after cache-lock escalation, and `cargo test --locked --quiet` passed locally. Exact-head PR/reviewer/no-mistakes state must be refreshed after committing and pushing this docs-only disposition head.
- Phase 59 blocked completion because exact-head GLM custom review found a direct-validator completeness gap for market-like post-only rejection after Phase 58.
- Phase 59 is closed by T259 verification at 2026-05-22 16:10:00 KST: direct-validator RED/GREEN, shared-module source-fence RED/GREEN, direct trigger/trailing and factory-gap coverage, focused config tests, schema/runtime verifiers, `just source-fence`, `just clippy`, and `cargo test --locked --quiet` passed locally. Exact-head PR checks, reviewer state, and no-mistakes state must be refreshed after committing and pushing this follow-up head.
- Phase 60 is closed by T264 verification: focused shared-builder tests, schema/runtime verifiers, source fences, full relevant local checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state were refreshed by the final exact-head evidence recorded in `research.md`.
- Phase 61 is closed by T269 verification: Phase 8/order-intent tests, schema/runtime verifiers, source fences, full relevant local checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state were refreshed by the final exact-head evidence recorded in `research.md`.
- Phase 62 is closed by T274 verification: Phase 8 OMS canonicalization tests, schema/runtime verifiers, full relevant local checks, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state were refreshed by the final exact-head evidence recorded in `research.md`.
- Phase 63 is closed by T278 verification: invalid-OMS fail-closed coverage, helper disproof coverage, branch cleanliness, exact-head PR checks, reviewer state, and no-mistakes state were refreshed by the final exact-head evidence recorded in `research.md`.
- Phase 64 blocks final Phase 8 approval-envelope completion until focused tests, verifiers, source fences, exact-head PR checks, external reviewer state, and no-mistakes state are refreshed for the order-enum canonicalization head.
- Phase 65 blocks final branch completion because no-mistakes produced activation-price admission evidence on top of the Phase 64 head.
- Phase 66 blocks final branch completion because no-mistakes produced financial-envelope schema-field drift evidence on top of the Phase 65 head.
- Phase 67 blocks final branch completion because exact-head Kimi review found the Phase 66 financial-envelope schema guard still allowed stale doc-only fields.
- Phase 68 blocks final branch completion because exact-head no-mistakes document review found active docs/specs still omitted decision-evidence JSONL v4/order-template fields and retained stale Phase 022 forced-exit/GTD text.
- Phase 69 blocks final branch completion because exact-head no-mistakes found the strategy schema version still advertised the previous forced-exit schema shape.
- Phase 70 blocks final branch completion because exact-head no-mistakes found the shipped example strategy schema and decision-evidence envelope docs lagged current source.
- Phase 71 blocks final branch completion because exact-head Gemini relay review found all-targets clippy failures in test-only helper/assertion code after Phase 70.
- Phase 72 blocks final branch completion because exact-head no-mistakes/Greptile review found duplicated position-contract helper definitions across the archetype and strategy modules after Phase 71.

## Parallel Opportunities

- T006, T007, and T008 can run in parallel.
- T037, T038, and T039 can run in parallel after implementation.
- Adapter source evidence in T027 can be gathered independently from TDD slices if it does not modify production code.

## Implementation Strategy

1. Finish pre-implementation review.
2. Execute one TDD slice at a time.
3. Keep implementation inside the listed source/test files unless a review finding proves extraction is necessary.
4. Verify each slice before moving to the next.
5. Treat source-level proof, strategy-free proof, and live proof as separate evidence classes.
