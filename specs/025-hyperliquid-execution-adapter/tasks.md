# Tasks: Hyperliquid Execution Adapter

**Input**: `specs/025-hyperliquid-execution-adapter/`
**Prerequisites**: Relay-Claude adversarial plan approval and user approval before implementation.
**TDD Rule**: Work one vertical red-green-refactor slice at a time. Do not write all tests first.

## Phase 1 - Setup

- [x] T001 Record current branch, current `origin/main` SHA, PR 480 merge SHA, and clean-worktree evidence in `specs/025-hyperliquid-execution-adapter/research.md`.
- [x] T002 Re-check repo rules in `AGENTS.md`, constitution gates in `.specify/memory/constitution.md`, and plan scope in `specs/025-hyperliquid-execution-adapter/plan.md`.
- [x] T003 Record current `nautilus_trader` pin and matching `nautilus-hyperliquid` crate evidence in `specs/025-hyperliquid-execution-adapter/research.md`.
- [x] T004 Record official Hyperliquid docs evidence for latency, nonces/API wallets, asset IDs, and rate-limit weights in `specs/025-hyperliquid-execution-adapter/research.md`.
- [x] T005 Record relay-Claude adversarial plan-review result in `specs/025-hyperliquid-execution-adapter/plan.md`.

## Phase 2 - Foundational

- [x] T006 Add Hyperliquid task-gate notes to `specs/025-hyperliquid-execution-adapter/quickstart.md`.
- [x] T007 Confirm production-entrypoint guard forbids raw `src/clients/hyperliquid.rs` in `tests/bolt_v3_production_entrypoint.rs`.
- [x] T008 Add `nautilus-hyperliquid` dependency from matching NT pin in `Cargo.toml`.

## Phase 3 - User Story 1: Register Hyperliquid Safely

**Goal**: Hyperliquid config maps through `ProviderBinding` with SSM-only credentials and no env fallback.

**Independent Test**: Provider binding accepts valid SSM-backed config and rejects raw secrets, missing SSM paths, env fallback, and duplicate signer owner.

- [x] T009 [US1] Add first failing valid-provider registration test in `tests/bolt_v3_provider_binding.rs`.
- [x] T010 [US1] Add minimal Hyperliquid provider binding in `src/bolt_v3_providers/mod.rs`.
- [x] T011 [US1] Refactor shared provider binding helpers only if needed in `src/bolt_v3_providers/mod.rs`.
- [x] T012 [US1] Add failing TOML config test for Hyperliquid execution mode and SSM paths in `tests/bolt_v3_provider_binding.rs`.
- [x] T013 [US1] Extend config structs for Hyperliquid client block and execution mode in `src/bolt_v3_config.rs`.
- [x] T014 [US1] Add failing raw-secret rejection test in `tests/bolt_v3_provider_binding.rs`.
- [x] T015 [US1] Implement SSM-only secret field validation in `src/bolt_v3_providers/mod.rs`.
- [x] T016 [US1] Add failing `HYPERLIQUID_*` env-fallback rejection test in `tests/bolt_v3_provider_binding.rs`.
- [x] T017 [US1] Implement forbidden-env validation before NT handoff in `src/bolt_v3_providers/mod.rs`.
- [x] T018 [US1] Add failing duplicate signer/API-wallet owner test in `tests/bolt_v3_provider_binding.rs`.
- [x] T019 [US1] Implement signer fingerprint and owner validation in `src/bolt_v3_providers/mod.rs`.

## Phase 4 - User Story 2: Prove Product Discovery Matrix

**Goal**: Standard perps, spot, HIP-3, and HIP-4 have discovery evidence and independent fail-closed submit status.

**Independent Test**: Product matrix artifact lists each surface with discovery source and submit readiness status.

- [x] T020 [US2] Add failing standard-perps discovery matrix test in `tests/hyperliquid_product_matrix.rs`.
- [x] T021 [US2] Implement standard-perps discovery mapping in `src/bolt_v3_providers/mod.rs`.
- [x] T022 [US2] Add failing spot discovery/fail-closed test in `tests/hyperliquid_product_matrix.rs`.
- [x] T023 [US2] Implement spot discovery status and fail-closed submit reason in `src/bolt_v3_providers/mod.rs`.
- [x] T024 [US2] Add failing HIP-3 discovery/fail-closed test in `tests/hyperliquid_product_matrix.rs`.
- [x] T025 [US2] Implement HIP-3 discovery status and fail-closed submit reason in `src/bolt_v3_providers/mod.rs`.
- [x] T026 [US2] Add failing HIP-4 discovery/fail-closed test in `tests/hyperliquid_product_matrix.rs`.
- [x] T027 [US2] Implement HIP-4 discovery status and fail-closed submit reason in `src/bolt_v3_providers/mod.rs`.
- [x] T028 [US2] Export product matrix evidence in `src/bolt_v3_operator_artifacts.rs`.

## Phase 5 - User Story 3: Prove No-Submit Standard Perps

**Goal**: Standard-perps readiness exercises adapter construction, metadata, fees, signer, and admission logic with zero exchange-mutating requests.

**Independent Test**: No-submit readiness fails if submit, cancel, modify, transfer, or account mutation occurs.

- [x] T029 [US3] Add failing no-submit readiness test in `tests/hyperliquid_no_submit.rs`.
- [x] T030 [US3] Implement no-submit readiness artifact in `src/bolt_v3_operator_artifacts.rs`.
- [x] T031 [US3] Add failing exchange-mutation counter test in `tests/hyperliquid_no_submit.rs`.
- [x] T032 [US3] Implement exchange-mutation guard in shared execution/admission code under `src/`.
- [x] T033 [US3] Add failing `userFees` request-weight test in `tests/hyperliquid_no_submit.rs`.
- [x] T034 [US3] Implement official `userFees` weight accounting in `src/bolt_v3_providers/mod.rs`.

## Phase 6 - User Story 4: Gate Live Standard Perps Submit

**Goal**: Standard-perps live submit requires exact live-submit approval artifact.

**Independent Test**: Missing, stale, mismatched, expired, reused, or overbroad artifacts are rejected.

- [x] T035 [US4] Add failing missing-approval test in `tests/hyperliquid_live_submit_artifact.rs`.
- [x] T036 [US4] Implement live-submit artifact schema in `src/bolt_v3_operator_artifacts.rs`.
- [x] T037 [US4] Add failing stale/mismatched/expired/reused artifact tests in `tests/hyperliquid_live_submit_artifact.rs`.
- [x] T038 [US4] Implement artifact binding and one-time consumption in `src/bolt_v3_providers/mod.rs`.
- [x] T039 [US4] Add standard-perps submit path only through NT adapter and shared execution/admission code under `src/`.

## Phase 7 - User Story 5: Keep Spot, HIP-3, And HIP-4 Fail-Closed

**Goal**: Spot, HIP-3, and HIP-4 remain discoverable but blocked for live submit until product-specific proof exists.

**Independent Test**: Enabling any unproven surface fails with missing proof reason.

- [x] T040 [US5] Add failing spot live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T041 [US5] Implement spot missing-proof rejection in `src/bolt_v3_providers/mod.rs`.
- [x] T042 [US5] Add failing HIP-3 live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T043 [US5] Implement HIP-3 missing-proof rejection in `src/bolt_v3_providers/mod.rs`.
- [x] T044 [US5] Add failing HIP-4 live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T045 [US5] Implement HIP-4 missing-proof rejection in `src/bolt_v3_providers/mod.rs`.

## Phase 8 - User Story 6: Configure Latency Ops Separately

**Goal**: Local info-node and colocation profile are TOML-driven ops metadata and cannot change execution gates.

**Independent Test**: Latency profile affects exported artifacts only and cannot bypass submit guards.

- [x] T046 [US6] Add failing latency-profile config test in `tests/bolt_v3_provider_binding.rs`.
- [x] T047 [US6] Add latency profile fields in `src/bolt_v3_config.rs`.
- [x] T048 [US6] Add failing latency-profile no-bypass test in `tests/hyperliquid_no_submit.rs`.
- [x] T049 [US6] Export latency profile artifacts without changing submit gates in `src/bolt_v3_operator_artifacts.rs`.

## Phase 9 - Verification

- [x] T050 Run `cargo fmt --check` for `Cargo.toml` and `src/`.
- [x] T051 Run `cargo clippy --locked --lib -- -D warnings` for `src/lib.rs`.
- [x] T052 Run focused provider tests for `tests/bolt_v3_provider_binding.rs`.
- [x] T053 Run focused entrypoint tests for `tests/bolt_v3_production_entrypoint.rs`.
- [x] T054 Run focused Hyperliquid matrix tests for `tests/hyperliquid_product_matrix.rs`.
- [x] T055 Run focused no-submit tests for `tests/hyperliquid_no_submit.rs`.
- [x] T056 Produce evidence packet in `specs/025-hyperliquid-execution-adapter/quickstart.md`.

## Dependencies

- Phase 1 and Phase 2 block all implementation.
- US1 blocks US2-US6.
- US2 blocks US3-US5.
- US3 blocks US4.
- US6 can run after US1.
- Phase 9 runs after each changed vertical slice and once at the end.

## MVP Scope

MVP is US1, US2, US3, US5 fail-closed guards, and US6 ops metadata. US4 live standard-perps submit remains gated follow-up unless user explicitly approves that slice after MVP proof.
