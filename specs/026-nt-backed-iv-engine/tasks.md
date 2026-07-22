# Tasks: NT-Backed IV Engine

**Input**: Design documents from `specs/026-nt-backed-iv-engine/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/iv-engine-api.md`, `quickstart.md`
**Verification**: Required. The spec and `AGENTS.md` require evidence-driven verification. Use behavior tests and RED/GREEN evidence where tests are the smallest reliable proof; otherwise record the exact static check, source-fence result, remote CI result, review artifact, or live/operator evidence that proves the claim.
**Scope**: IV/implied-volatility engine only. FV and RV are explicitly out of scope.

## Phase 1: Setup

**Purpose**: Establish exact repository evidence and IV module scaffolding without implementing behavior.

- [X] T001 Record repository truth in `specs/026-nt-backed-iv-engine/reference/repository-truth.md`
- [X] T002 Record open PR and open issue overlap review in `specs/026-nt-backed-iv-engine/reference/overlap-ledger.md`
- [X] T003 Record current-main requirement evidence in `specs/026-nt-backed-iv-engine/reference/evidence-ledger.md`
- [X] T004 Record pinned NT dependency evidence from `Cargo.toml` and `Cargo.lock` in `specs/026-nt-backed-iv-engine/reference/nt-evidence.md`
- [X] T005 Create IV module directory skeleton in `src/bolt_v3_iv/mod.rs`
- [X] T006 [P] Add IV module export placeholder in `src/lib.rs`
- [X] T007 [P] Add empty IV fixture directory in `tests/fixtures/bolt_v3_iv/README.md`
- [X] T008 [P] Add IV evidence fixture inventory in `tests/fixtures/bolt_v3_iv/evidence.md`
- [X] T009 Add IV implementation progress ledger in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

---

## Phase 2: Foundational

**Purpose**: Shared types, boundaries, and source-fence infrastructure required before story implementation.

- [X] T010 Add shared IV type module declarations in `src/bolt_v3_iv/mod.rs`
- [X] T011 [P] Define IV source/product enums in `src/bolt_v3_iv/types.rs`
- [X] T012 [P] Define `IvRejectReason` in `src/bolt_v3_iv/error.rs`
- [X] T013 [P] Define nanosecond timestamp wrapper types in `src/bolt_v3_iv/time.rs`
- [X] T014 [P] Define `IvNumericBounds` and convention bounds in `src/bolt_v3_iv/bounds.rs`
- [X] T015 [P] Define `IvPolicyDecision` variants in `src/bolt_v3_iv/provenance.rs`
- [X] T016 [P] Define `IvProvenance` in `src/bolt_v3_iv/provenance.rs`
- [X] T017 [P] Define `IvSelector` source/query union in `src/bolt_v3_iv/selector.rs`
- [X] T018 [P] Define `IvSelectorAuthorization` in `src/bolt_v3_iv/authz.rs`
- [X] T019 [P] Define `IvAuditPolicy` and audit handle marker types in `src/bolt_v3_iv/audit.rs`
- [X] T020 [P] Define `IvSourceHealth` states and transitions in `src/bolt_v3_iv/health.rs`
- [X] T021 Add shared IV fixture builders for tests in `tests/bolt_v3_iv_support.rs`
- [X] T022 Add IV source-fence verifier entrypoint placeholder in `tests/bolt_v3_iv_source_fence.rs`
- [X] T023 Wire IV source-fence target into `justfile`
- [X] T024 Record foundational verification evidence requirements in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: Shared IV type boundary is ready; user story work can start.

---

## Phase 3: User Story 1 - Inventory NT IV Capabilities Completely (Priority: P1)

**Goal**: Generate a source-backed ledger of every IV/options capability reachable from the Cargo-pinned NT Rust APIs.

**Independent Test**: `tests/bolt_v3_iv_capability.rs` fails when an NT IV/options surface or whole-checkout candidate is unclassified.

### Tests for User Story 1

- [X] T025 [P] [US1] Add failing test for Cargo metadata and `Cargo.lock` NT checkout resolution in `tests/bolt_v3_iv_capability.rs`
- [X] T026 [P] [US1] Add failing test for seed-family IV/options surface discovery in `tests/bolt_v3_iv_capability.rs`
- [X] T027 [P] [US1] Add failing test for whole-checkout candidate sweep terms from FR-054 in `tests/bolt_v3_iv_capability.rs`, including option, options, greeks, implied, iv, volatility, smile, surface, chain, custom data, strike, expiry, expiration, tenor, moneyness, skew, premium, and vol
- [X] T028 [P] [US1] Add failing test for unclassified candidate rejection in `tests/bolt_v3_iv_capability.rs`
- [X] T029 [US1] Record US1 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 1

- [X] T030 [P] [US1] Implement Cargo metadata NT checkout resolver in `src/bolt_v3_iv/capability.rs`
- [X] T031 [P] [US1] Implement seed-family scanner for NT model, data actor, data engine, msgbus, option-chain, greeks-helper, adapter, and custom-data surfaces in `src/bolt_v3_iv/capability.rs`
- [X] T032 [P] [US1] Implement whole-checkout public-symbol candidate sweep in `src/bolt_v3_iv/capability.rs`
- [X] T033 [P] [US1] Implement candidate classification model in `src/bolt_v3_iv/capability.rs`
- [X] T034 [US1] Add generated ledger artifact loader for test fixtures in `src/bolt_v3_iv/capability.rs`
- [X] T035 [US1] Add capability fixture classifications in `tests/fixtures/bolt_v3_iv/capability-ledger.toml`
- [X] T036 [US1] Record US1 GREEN evidence and NT-first decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: IV capability scope is source-backed and independently testable.

---

## Phase 4: User Story 2 - Subscribe To Configured NT IV/Options Sources (Priority: P1)

**Goal**: Convert TOML-owned IV profiles into NT subscribe/unsubscribe operations for all supported source kinds.

**Independent Test**: `tests/bolt_v3_iv_subscription.rs` records expected NT subscribe/unsubscribe requests from generic TOML fixtures.

### Tests for User Story 2

- [X] T037 [P] [US2] Add failing option-greeks subscription planner test in `tests/bolt_v3_iv_subscription.rs`
- [X] T038 [P] [US2] Add failing option-chain subscription planner test in `tests/bolt_v3_iv_subscription.rs`
- [X] T039 [P] [US2] Add failing aggregate-greeks subscription planner test in `tests/bolt_v3_iv_subscription.rs`
- [X] T040 [P] [US2] Add failing custom-implied-volatility subscription planner test in `tests/bolt_v3_iv_subscription.rs`
- [X] T041 [P] [US2] Add failing reload/unsubscribe/source-removal test in `tests/bolt_v3_iv_subscription.rs`
- [X] T042 [US2] Record US2 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 2

- [X] T043 [P] [US2] Define `IvSubscriptionPlan` in `src/bolt_v3_iv/subscription.rs`
- [X] T044 [P] [US2] Implement option-greeks subscription planning in `src/bolt_v3_iv/subscription.rs`
- [X] T045 [P] [US2] Implement option-chain subscription planning in `src/bolt_v3_iv/subscription.rs`
- [X] T046 [P] [US2] Implement aggregate-greeks subscription planning in `src/bolt_v3_iv/subscription.rs`
- [X] T047 [P] [US2] Implement custom-implied-volatility subscription planning in `src/bolt_v3_iv/subscription.rs`
- [X] T048 [US2] Implement reload, unsubscribe, and source-removal planning in `src/bolt_v3_iv/subscription.rs`
- [X] T049 [US2] Implement runtime binding adapter traits in `src/bolt_v3_iv/runtime.rs`
- [X] T050 [US2] Record US2 GREEN evidence and NT runtime mapping decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: Configured IV sources map to bounded NT subscription operations.

---

## Phase 5: User Story 3 - Preserve Raw NT Data And Expose Indexed IV Products (Priority: P1)

**Goal**: Preserve raw NT IV/options payloads and expose strategy-safe indexed products.

**Independent Test**: `tests/bolt_v3_iv_ingest.rs` and `tests/bolt_v3_iv_store.rs` prove raw preservation, indexing, provenance, and audit-only raw access.

### Tests for User Story 3

- [X] T051 [P] [US3] Add failing option-greeks raw preservation and indexing test in `tests/bolt_v3_iv_ingest.rs`
- [X] T052 [P] [US3] Add failing option-chain smile and surface indexing test in `tests/bolt_v3_iv_ingest.rs`
- [X] T053 [P] [US3] Add failing aggregate-greeks product indexing test in `tests/bolt_v3_iv_ingest.rs`
- [X] T054 [P] [US3] Add failing custom-IV-evidence indexing test in `tests/bolt_v3_iv_ingest.rs`
- [X] T055 [P] [US3] Add failing audit-only raw access test in `tests/bolt_v3_iv_store.rs`
- [X] T056 [P] [US3] Add failing provenance completeness test in `tests/bolt_v3_iv_store.rs`
- [X] T057 [US3] Record US3 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 3

- [X] T058 [P] [US3] Implement `IvRawEvent` preservation in `src/bolt_v3_iv/ingest.rs`
- [X] T059 [P] [US3] Implement `IvPoint` and `IvGreeksPoint` indexing in `src/bolt_v3_iv/store.rs`
- [X] T060 [P] [US3] Implement `IvSmile` construction in `src/bolt_v3_iv/store.rs`
- [X] T061 [P] [US3] Implement `IvSurface` construction in `src/bolt_v3_iv/store.rs`
- [X] T062 [P] [US3] Implement `IvAggregateGreeks` indexing in `src/bolt_v3_iv/store.rs`
- [X] T063 [P] [US3] Implement `IvEvidence` indexing in `src/bolt_v3_iv/store.rs`
- [X] T064 [US3] Implement audit/replay raw reader enforcement in `src/bolt_v3_iv/raw_access.rs`
- [X] T065 [US3] Implement provenance construction for raw and indexed products in `src/bolt_v3_iv/provenance.rs`
- [X] T066 [US3] Record US3 GREEN evidence and raw-boundary decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: Raw NT evidence is preserved internally and strategy-safe IV products are queryable.

---

## Phase 6: User Story 4 - Derive IV With NT Math Helpers When Inputs Are Complete (Priority: P1)

**Goal**: Use NT math helpers through explicit helper/input policy and reject incomplete or invalid derivations.

**Independent Test**: `tests/bolt_v3_iv_derive.rs` proves helper-backed outputs and fail-closed missing/invalid input classes.

### Tests for User Story 4

- [X] T067 [P] [US4] Add failing helper-policy selection test in `tests/bolt_v3_iv_derive.rs`
- [X] T068 [P] [US4] Add failing complete-input derived-IV test in `tests/bolt_v3_iv_derive.rs`
- [X] T069 [P] [US4] Add failing missing-input rejection matrix in `tests/bolt_v3_iv_derive.rs`
- [X] T070 [P] [US4] Add failing stale/skewed-input rejection test in `tests/bolt_v3_iv_derive.rs`
- [X] T071 [P] [US4] Add failing expired operator rate/carry rejection test in `tests/bolt_v3_iv_derive.rs`
- [X] T072 [P] [US4] Add failing helper-output-bound rejection test in `tests/bolt_v3_iv_derive.rs`
- [X] T073 [US4] Record US4 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 4

- [X] T074 [P] [US4] Implement `IvHelperPolicy` in `src/bolt_v3_iv/derive.rs`
- [X] T075 [P] [US4] Implement `IvDerivedInputPolicy` resolution in `src/bolt_v3_iv/derive.rs`
- [X] T076 [P] [US4] Implement `IvDerivedInputSet` validation in `src/bolt_v3_iv/derive.rs`
- [X] T077 [US4] Implement NT helper invocation wrapper in `src/bolt_v3_iv/derive.rs`
- [X] T078 [US4] Implement helper output validation and typed rejection in `src/bolt_v3_iv/derive.rs`
- [X] T079 [US4] Implement helper provenance and `HelperDecision` recording in `src/bolt_v3_iv/provenance.rs`
- [X] T080 [US4] Record US4 GREEN evidence and helper NT-source decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: Derived IV is NT-backed, policy-selected, provenance-recorded, and fail-closed.

---

## Phase 7: User Story 5 - Enforce Generic Config And Lifecycle Rules (Priority: P1)

**Goal**: Load all IV runtime behavior from TOML with typed validation, lifecycle, retention, bounds, and policy decisions.

**Independent Test**: `tests/bolt_v3_iv_config.rs`, `tests/bolt_v3_iv_policy.rs`, and `tests/bolt_v3_iv_live_integration.rs` prove typed config and lifecycle behavior.

### Tests for User Story 5

- [X] T081 [P] [US5] Add failing full-profile TOML parse test in `tests/bolt_v3_iv_config.rs`
- [X] T082 [P] [US5] Add failing unknown schema-version rejection test in `tests/bolt_v3_iv_config.rs`
- [X] T083 [P] [US5] Add failing selector/source/product mismatch test in `tests/bolt_v3_iv_config.rs`
- [X] T084 [P] [US5] Add failing numeric and convention bounds rejection test in `tests/bolt_v3_iv_config.rs`
- [X] T085 [P] [US5] Add failing interpolation/projection/fallback/quorum policy tests in `tests/bolt_v3_iv_policy.rs`
- [X] T086 [P] [US5] Add failing typed `IvPolicyDecision` provenance test in `tests/bolt_v3_iv_policy.rs`
- [X] T087 [P] [US5] Add failing source-health transition and retention eviction test in `tests/bolt_v3_iv_live_integration.rs`
- [X] T088 [US5] Record US5 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 5

- [X] T089 [P] [US5] Implement IV TOML schema and `IvProfile` parsing in `src/bolt_v3_iv/config.rs`
- [X] T090 [P] [US5] Implement schema-version policy validation in `src/bolt_v3_iv/config.rs`
- [X] T091 [P] [US5] Implement selector/source/product validation in `src/bolt_v3_iv/config.rs`
- [X] T092 [P] [US5] Implement bounds validation in `src/bolt_v3_iv/bounds.rs`
- [X] T093 [P] [US5] Implement projection policy in `src/bolt_v3_iv/policy.rs`
- [X] T094 [P] [US5] Implement interpolation and extrapolation policy in `src/bolt_v3_iv/policy.rs`
- [X] T095 [P] [US5] Implement fallback policy in `src/bolt_v3_iv/policy.rs`
- [X] T096 [P] [US5] Implement quorum policy in `src/bolt_v3_iv/policy.rs`
- [X] T097 [US5] Implement source-health state machine in `src/bolt_v3_iv/health.rs`
- [X] T098 [US5] Implement retention and memory-bound eviction in `src/bolt_v3_iv/store.rs`
- [X] T099 [US5] Integrate IV profile loading into root config in `src/bolt_v3_config.rs`
- [X] T100 [US5] Record US5 GREEN evidence and config group-by-change decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: IV runtime behavior is config-owned, typed, bounded, and lifecycle-aware.

---

## Phase 8: User Story 6 - Let Strategies Consume IV Generically (Priority: P1)

**Goal**: Provide one strategy-facing IV API while source-fencing strategies away from direct NT IV mechanics and raw payload dereference.

**Independent Test**: `tests/bolt_v3_iv_query.rs`, `tests/bolt_v3_iv_live_integration.rs`, and `tests/bolt_v3_iv_source_fence.rs` prove generic strategy consumption and bypass rejection.

### Tests for User Story 6

- [X] T101 [P] [US6] Add failing profile-wide strategy query test in `tests/bolt_v3_iv_query.rs`
- [X] T102 [P] [US6] Add failing selector-scoped strategy query authorization test in `tests/bolt_v3_iv_query.rs`
- [X] T103 [P] [US6] Add failing raw-product strategy query rejection test in `tests/bolt_v3_iv_query.rs`
- [X] T104 [P] [US6] Add failing live-node strategy-handle registration test in `tests/bolt_v3_iv_live_integration.rs`
- [X] T105 [P] [US6] Add failing direct NT IV subscription source-fence case in `tests/bolt_v3_iv_source_fence.rs`
- [X] T106 [P] [US6] Add failing strategy-local NT helper derivation source-fence case in `tests/bolt_v3_iv_source_fence.rs`
- [X] T107 [P] [US6] Add failing raw audit reader and raw DTO strategy import source-fence case in `tests/bolt_v3_iv_source_fence.rs`
- [X] T108 [P] [US6] Add failing IV-core hardcoded runtime value source-fence case in `tests/bolt_v3_iv_source_fence.rs`
- [X] T109 [US6] Record US6 RED evidence in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

### Implementation for User Story 6

- [X] T110 [P] [US6] Implement `IvQuery` and `IvQueryHandle` in `src/bolt_v3_iv/query.rs`
- [X] T111 [P] [US6] Implement profile-wide strategy authorization in `src/bolt_v3_iv/authz.rs`
- [X] T112 [P] [US6] Implement selector-scoped strategy authorization in `src/bolt_v3_iv/authz.rs`
- [X] T113 [US6] Implement strategy-safe product query routing in `src/bolt_v3_iv/query.rs`
- [X] T114 [US6] Implement raw-product rejection on strategy handles in `src/bolt_v3_iv/query.rs`
- [X] T115 [US6] Integrate IV query handles into strategy registration in `src/bolt_v3_strategy_registration.rs`
- [X] T116 [US6] Integrate IV engine start/stop into live node in `src/bolt_v3_live_node.rs`
- [X] T117 [US6] Implement IV source-fence checks in `tests/bolt_v3_iv_source_fence.rs`
- [X] T118 [US6] Record US6 GREEN evidence and strategy-boundary decisions in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`

**Checkpoint**: Strategies consume IV through one generic engine API and cannot own IV mechanics.

---

## Phase 9: Polish & Cross-Cutting

**Purpose**: Close documentation, review, and verification obligations after implementation.

- [X] T119 [P] Update IV quickstart with final TOML schema in `specs/026-nt-backed-iv-engine/quickstart.md`
- [X] T120 [P] Update IV API contract with final public type names in `specs/026-nt-backed-iv-engine/contracts/iv-engine-api.md`
- [X] T121 [P] Update implementation evidence ledger with all RED/GREEN command outputs in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`
- [X] T122 [P] Update overlap ledger and close any fully ported issues or PRs in `specs/026-nt-backed-iv-engine/reference/overlap-ledger.md`
- [X] T123 Run focused IV test targets (`cargo test --locked bolt_v3_iv`, `cargo test --locked --test bolt_v3_iv_capability`, `cargo test --locked --test bolt_v3_iv_config`, `cargo test --locked --test bolt_v3_iv_live_integration`, `cargo test --locked --test bolt_v3_iv_subscription`, `cargo test --locked --test bolt_v3_iv_ingest`, `cargo test --locked --test bolt_v3_iv_store`, `cargo test --locked --test bolt_v3_iv_query`, `cargo test --locked --test bolt_v3_iv_policy`, `cargo test --locked --test bolt_v3_iv_derive`, `cargo test --locked --test bolt_v3_iv_source_fence`, `cargo test --locked --test config_parsing`) and record outcomes in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`
- [X] T124 Run broader repository verification gates (`cargo fmt --check`, `cargo clippy --locked --lib -- -D warnings`, `just source-fence`) and record outcomes in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`
- [X] T125 Conduct internal adversarial review and record findings in `specs/026-nt-backed-iv-engine/reference/internal-review.md`
- [X] T126 Request external reviews only after exact-head local verification and CI are green, then record results in `specs/026-nt-backed-iv-engine/reference/external-review.md`
- [X] T127 If a new commit lands after review approval, rerun external reviews and update `specs/026-nt-backed-iv-engine/reference/external-review.md`
- [X] T128 Prepare final branch summary with base/head SHAs, NT APIs used, tests, review status, and residual risks in `specs/026-nt-backed-iv-engine/reference/final-summary.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies.
- **Foundational (Phase 2)**: Depends on Setup and blocks all user stories.
- **User Stories (Phases 3-8)**: Depend on Foundational. US1 should be completed before finalizing source-kind mappings in US2, US4, and US5 because it defines the supported NT capability ledger.
- **Polish (Phase 9)**: Depends on all implemented stories selected for the branch.

### User Story Dependencies

- **US1**: Starts after Foundational; defines source-backed NT capability scope.
- **US2**: Starts after Foundational; final source mappings depend on US1 ledger classifications.
- **US3**: Starts after Foundational; can proceed with fixture-backed raw/indexed products while US2 runtime binding is in progress.
- **US4**: Starts after Foundational; helper selection depends on US1 ledger classifications.
- **US5**: Starts after Foundational; can implement typed config independently, then align source/helper enums with US1.
- **US6**: Starts after Foundational; query API can be built against store/config contracts, then integrated after US3 and US5.

### Parallel Opportunities

- Setup documentation tasks T002-T004 can run in parallel.
- Foundational type tasks T011-T020 can run in parallel.
- Tests within each user story are parallelizable until they depend on shared helper fixtures.
- US3 store work and US5 config/policy work can run in parallel after Foundational.
- US6 source-fence tests can run in parallel with query-handle implementation.

---

## Parallel Examples

### User Story 1

```text
Task: T025 Add failing Cargo metadata and lockfile resolution test.
Task: T026 Add failing seed-family scan test.
Task: T027 Add failing whole-checkout candidate sweep test.
Task: T028 Add failing unclassified candidate rejection test.
```

### User Story 3

```text
Task: T058 Implement raw event preservation.
Task: T059 Implement IV/greeks point indexing.
Task: T060 Implement smile construction.
Task: T062 Implement aggregate greeks indexing.
Task: T063 Implement custom IV evidence indexing.
```

### User Story 5

```text
Task: T089 Implement IV TOML schema parsing.
Task: T092 Implement numeric and convention bounds validation.
Task: T093 Implement projection policy.
Task: T094 Implement interpolation and extrapolation policy.
Task: T095 Implement fallback policy.
Task: T096 Implement quorum policy.
```

---

## Implementation Strategy

### MVP First

1. Complete Phase 1 and Phase 2.
2. Complete US1 so "all NT offers" is source-backed.
3. Complete enough of US5 to parse one full IV profile with no runtime defaults.
4. Complete enough of US3 and US6 for one strategy-safe IV product query.
5. Stop and validate the MVP independently before adding the remaining product families.

### Full IV Engine Delivery

1. Complete US1 capability ledger.
2. Complete US5 typed config and policy validation.
3. Complete US2 runtime subscription planning and live binding.
4. Complete US3 raw/indexed store.
5. Complete US4 NT helper-backed derivation.
6. Complete US6 strategy query API and source-fence hardening.
7. Complete Phase 9 verification and reviews.

### Review Discipline

Each story must leave a clean evidence trail in `specs/026-nt-backed-iv-engine/reference/implementation-ledger.md`: requirement, current-main evidence, NT evidence, selected verification evidence, test-first RED/GREEN evidence when used, source-fence impact, and residual risk.
