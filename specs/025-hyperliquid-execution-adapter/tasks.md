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

**Goal**: Standard perps, spot, HIP-3, and HIP-4 have discovery evidence and independent approval-gated submit status.

**Independent Test**: Product matrix artifact lists each surface with discovery source and submit readiness status.

- [x] T020 [US2] Add failing standard-perps discovery matrix test in `tests/hyperliquid_product_matrix.rs`.
- [x] T021 [US2] Implement standard-perps discovery mapping in `src/bolt_v3_providers/mod.rs`.
- [x] T022 [US2] Add failing spot discovery/approval-gated test in `tests/hyperliquid_product_matrix.rs`.
- [x] T023 [US2] Implement spot discovery status and approval-gated submit status in `src/bolt_v3_providers/mod.rs`.
- [x] T024 [US2] Add failing HIP-3 discovery/approval-gated test in `tests/hyperliquid_product_matrix.rs`.
- [x] T025 [US2] Implement HIP-3 discovery status and approval-gated submit status in `src/bolt_v3_providers/mod.rs`.
- [x] T026 [US2] Add failing HIP-4 discovery/approval-gated test in `tests/hyperliquid_product_matrix.rs`.
- [x] T027 [US2] Implement HIP-4 discovery status and approval-gated submit status in `src/bolt_v3_providers/mod.rs`.
- [x] T028 [US2] Export product matrix evidence in `src/bolt_v3_operator_artifacts.rs`.

## Phase 5 - User Story 3: Prove Standard-Perps Fail-Closed Preconditions

**Goal**: Standard-perps remains blocked by fee/rate policy and shared exchange-mutation guards until a later live-submit slice supplies complete product proof.

**Independent Test**: The shared exchange-mutation guard fails closed for submit, cancel, modify, transfer, or account mutation; pinned NT `userFees` mismatch is reconciled by the Hyperliquid provider egress model using the official request weight.

- [x] T029 [US3] Remove Hyperliquid-specific no-submit readiness artifact from this slice.
- [x] T030 [US3] Keep Hyperliquid live submit approval-gated by product proof, product routing, and one-time approval.
- [x] T031 [US3] Add failing exchange-mutation counter test in `tests/hyperliquid_fail_closed.rs`.
- [x] T032 [US3] Implement exchange-mutation guard in shared execution/admission code under `src/`.
- [x] T033 [US3] Add failing `userFees` request-weight test in `tests/hyperliquid_fail_closed.rs`.
- [x] T034 [US3] Implement official `userFees` weight accounting in `src/bolt_v3_providers/mod.rs`.

## Phase 6 - User Story 4: Gate Standard-Perps Adapter Mapping

**Goal**: Standard-perps adapter mapping requires exact consumed live-submit approval artifact.

**Independent Test**: Missing, stale, mismatched, expired, reused, or overbroad artifacts are rejected at the mapper boundary.

- [x] T035 [US4] Add failing missing-approval test in `tests/hyperliquid_live_submit_artifact.rs`.
- [x] T036 [US4] Implement live-submit artifact schema in `src/bolt_v3_operator_artifacts.rs`.
- [x] T037 [US4] Add failing stale/mismatched/expired/reused artifact tests in `tests/hyperliquid_live_submit_artifact.rs`.
- [x] T038 [US4] Implement artifact binding and one-time consumption in `src/bolt_v3_providers/mod.rs`.
- [x] T039 [US4] Add standard-perps NT adapter mapping only behind consumed approval under `src/`.

## Phase 7 - User Story 5: Gate Spot, HIP-3, And HIP-4 Mapping By Surface Approval

**Goal**: Spot, HIP-3, and HIP-4 remain discoverable and blocked without consumed surface-bound approval; with exact approval they map through the NT Hyperliquid execution adapter at the mapper boundary. HIP-4 additionally requires positive outcome settlement polling.

**Independent Test**: Mapping any surface without consumed matching approval fails closed; consumed approval for one surface cannot authorize a different surface.

- [x] T040 [US5] Add failing spot live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T041 [US5] Implement spot surface-approval gate in `src/bolt_v3_providers/mod.rs`.
- [x] T042 [US5] Add failing HIP-3 live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T043 [US5] Implement HIP-3 surface-approval gate in `src/bolt_v3_providers/mod.rs`.
- [x] T044 [US5] Add failing HIP-4 live-submit rejection test in `tests/hyperliquid_product_matrix.rs`.
- [x] T045 [US5] Implement HIP-4 surface-approval and settlement-poll gate in `src/bolt_v3_providers/mod.rs`.

## Phase 8 - User Story 6: Configure Latency Ops Separately

**Goal**: Local info-node and colocation profile are TOML-driven ops metadata and cannot change execution gates.

**Independent Test**: Latency profile affects exported artifacts only and cannot bypass submit guards.

- [x] T046 [US6] Add failing latency-profile config test in `tests/bolt_v3_provider_binding.rs`.
- [x] T047 [US6] Add latency profile fields in `src/bolt_v3_config.rs`.
- [x] T048 [US6] Add failing latency-profile no-bypass test in `tests/hyperliquid_fail_closed.rs`.
- [x] T049 [US6] Export latency profile artifacts without changing submit gates in `src/bolt_v3_operator_artifacts.rs`.

## Phase 8A - User Story 5: Register Hyperliquid Market Data

**Goal**: Hyperliquid `[data]` maps to NT market-data adapter config without enabling live submit or requiring signer material.

**Independent Test**: Hyperliquid data-only config validates, maps to NT `HyperliquidDataClientConfig`, and leaves `private_key` unset.

- [x] T049A [US5] Add failing Hyperliquid data-only mapping test in `tests/bolt_v3_adapter_mapping.rs`.
- [x] T049B [US5] Implement Hyperliquid data config validation and NT data adapter mapping in `src/bolt_v3_providers/hyperliquid.rs`.
- [x] T049C [US5] Add provider-binding validation tests for Hyperliquid data config in `tests/bolt_v3_provider_binding.rs`.

## Phase 8B - Shared Live-Node Approval Loading

**Goal**: Production live-node adapter mapping can consume provider-owned live-submit approval artifacts without making core live-node code depend on Hyperliquid internals.

**Independent Test**: Production live-node adapter mapping consumes the configured Hyperliquid approval artifact once, persists `used_at`, maps the execution client, and rejects replay.

- [x] T049D Add failing production live-node approval-consumption test in `src/bolt_v3_live_node.rs`.
- [x] T049E Add provider-neutral live-submit approval hook and Hyperliquid artifact loading in `src/bolt_v3_providers/mod.rs` and `src/bolt_v3_providers/hyperliquid.rs`.
- [x] T049F Persist consumed Hyperliquid approval artifacts and pass consumed approvals through provider-neutral runtime approvals.
- [x] T049G Update literal audit and source-fence coverage for live-node approval loading.

## Phase 8C - Shared Approval Order-Limit Enforcement

**Goal**: Consumed provider approval order limits constrain actual submit admission before NT submit can be reached.

**Independent Test**: A provider approval with tighter order count and notional caps than the live canary report rejects wider or excess orders in shared submit admission.

- [x] T049H Add failing submit-admission test for provider approval count and notional caps.
- [x] T049I Add provider-neutral live-submit order limits and carry Hyperliquid consumed approval limits into live-node construction.
- [x] T049J Add execution-client identity to submit-admission requests so limits apply to the exact approved client.
- [x] T049K Verify Hyperliquid approval loading exposes submit-admission limits.

## Phase 8D - HIP-4 Market-Family Route Gate

**Goal**: Hyperliquid HIP-4 outcome execution clients can pass the shared market-family compatibility gate for existing `updown` targets before surface-bound approval mapping.

**Independent Test**: A HIP-4 Hyperliquid execution client with a consumed HIP-4 approval and an `updown` target plan maps instead of failing on `SUPPORTED_MARKET_FAMILIES`.

- [x] T049L Add failing Hyperliquid HIP-4/updown routing gate test in `tests/bolt_v3_adapter_mapping.rs`.
- [x] T049M Populate Hyperliquid `SUPPORTED_MARKET_FAMILIES` with the existing `updown` market family.
- [x] T049N Document remaining product-specific routing gaps for standard perps, spot, and HIP-3.

## Phase 8E - Official UserFees Egress Reconciliation

**Goal**: Live-submit validation no longer fails only because pinned NT internally underweights `userFees`; Bolt's provider policy must reserve the official Hyperliquid weight before validation can pass.

**Independent Test**: Hyperliquid live-submit config validation passes with required approval fields when `venue_egress_model("HYPERLIQUID")` reserves 20 request-weight per order command, while the NT caller inventory still proves the pinned NT base weight is lower.

- [x] T049O Add failing provider-binding test for Hyperliquid egress policy and live-submit validation.
- [x] T049P Add Hyperliquid REST egress model using the official `userFees` weight.
- [x] T049Q Remove stale `userFees` missing-proof blocker from the product matrix.

## Phase 8F - Operator Approval Artifact Materialization

**Goal**: Operators can generate the exact Hyperliquid live-submit approval artifact that production live-node construction later consumes, without raw secret CLI inputs or a second artifact path.

**Independent Test**: A provider-binding writer derives the approval artifact from loaded TOML plus resolved SSM secrets and writes the configured artifact path, while the CLI exposes `provider-artifacts generate-live-submit-approval` with config/client/product-surface/expiry inputs and no raw private-key or account-address arguments.

- [x] T049R Add failing provider-binding test for configured Hyperliquid approval artifact materialization.
- [x] T049S Add provider-neutral live-submit approval artifact writer hook and Hyperliquid implementation.
- [x] T049T Add `provider-artifacts generate-live-submit-approval` CLI entry point using Rust SSM secret resolution.

## Phase 8G - Static Hyperliquid Instrument Route Identity

**Goal**: Standard perps, spot, and HIP-3 Hyperliquid targets can pass the shared execution-client market-family compatibility gate without reusing the up/down rotating-market target shape.

**Independent Test**: A static Hyperliquid instrument target projects into `MarketIdentityPlan`, and a consumed standard-perps approval with that target maps instead of failing on `SUPPORTED_MARKET_FAMILIES`.

- [x] T049U Add failing static Hyperliquid instrument market-identity planner test.
- [x] T049V Add `hyperliquid_instrument` market-family binding with strict static-instrument target parsing and fail-closed binary market-selection APIs.
- [x] T049W Populate Hyperliquid `SUPPORTED_MARKET_FAMILIES` with `hyperliquid_instrument` and prove adapter mapping passes the route gate.

## Phase 8H - Static Target Surface Match Guard

**Goal**: A static Hyperliquid instrument target cannot route through a client configured and approved for a different Hyperliquid product surface.

**Independent Test**: A standard-perps execution client with a consumed standard-perps approval rejects a static target configured for `spot`.

- [x] T049X Add failing adapter-mapping test for static target product-surface mismatch.
- [x] T049Y Enforce static target product-surface compatibility in the Hyperliquid provider mapper.
- [x] T049Z Document the target/client surface-match invariant.

## Phase 8I - Pre-Spend Static Target Approval Guard

**Goal**: A known static Hyperliquid target surface mismatch cannot spend a one-time live-submit approval artifact before adapter mapping rejects the client.

**Independent Test**: Production live-node approval loading rejects a spot static target on a standard-perps client and leaves the approval artifact `used_at` unset.

- [x] T049AA Add failing live-node test for static target surface mismatch approval non-consumption.
- [x] T049AB Reuse Hyperliquid static target surface validation in the approval loader before artifact consumption.
- [x] T049AC Keep provider-binding test fakes current with the live-submit approval writer hook.

## Phase 8J - Proof-Policy Transport Approval Scope

**Goal**: A proof-policy-only live run cannot retain unrelated execution clients and accidentally consume unrelated provider live-submit approval artifacts during live-node construction.

**Independent Test**: With no strategies and an enabled proof policy for `polymarket_main`, trade transport scoping drops an unrelated execution client before adapter mapping and approval loading.

- [x] T049AD Add failing proof-policy-only trade transport scoping test.
- [x] T049AE Derive required trade transport clients from strategy and proof-policy references before the empty-runtime shortcut.

## Phase 8K - Hyperliquid Fee-Provider Boundary

**Goal**: Hyperliquid execution strategies resolve fees through the provider registry instead of failing on a missing provider binding or silently assuming zero fees.

**Independent Test**: The Hyperliquid provider binding builds a fee provider from provider-owned execution config, returns no fee bound before warmup, and warms from the NT `userFees` response for the SSM-resolved account address.

- [x] T049AF Add failing provider-binding test for Hyperliquid fee-provider construction.
- [x] T049AG Register a Hyperliquid fee-provider builder with an empty cold cache.
- [x] T049AH Warm the Hyperliquid fee provider from `userFees.userCrossRate` and cache the taker fee bound in basis points.

## Phase 8L - Hyperliquid Product-Submit Proof Binding

**Goal**: A consumed Hyperliquid live-submit approval cannot open the execution adapter unless the approval is bound to the product-submit proof evidence configured for that exact runtime.

**Independent Test**: A legacy approval artifact without `product_submit_proof` fails closed; generated approval artifacts include product proof path and sha256; consumed approval carries the proof binding; missing or mismatched product proof files fail before approval consumption.

- [x] T049AI Add failing approval-artifact test for missing `product_submit_proof`.
- [x] T049AJ Add TOML-owned product proof path/sha256 fields to Hyperliquid live-submit approval validation and materialization.
- [x] T049AK Carry product proof binding through consumed Hyperliquid approvals.
- [x] T049AL Add failing live-node tests proving missing and mismatched product proof files do not spend one-time approvals.
- [x] T049AM Verify the bound product proof artifact sha256 before consuming Hyperliquid live-submit approvals.
- [x] T049AMa Add a separate TOML-owned product proof artifact byte cap so proof evidence and approval artifacts do not share one read limit.
- [x] T049AMb Add provider-owned product-submit proof artifact writer tests for proof-reference schema validation and HIP-4 settlement proof requirements.
- [x] T049AMc Register provider-neutral `provider-artifacts generate-product-submit-proof` through `ProviderBinding`.

## Phase 8M - Hyperliquid Product Matrix Approval-Gated Status

**Goal**: The operator-facing Hyperliquid product matrix must match the implemented live-execution state: standard perps, spot, HIP-3, and HIP-4 are not globally fail-closed once product-proof binding exists, but remain approval-gated by exact consumed surface approval.

**Independent Test**: The matrix artifact serializes all four surfaces with `live_submit_status = "approval_gated"` and empty `missing_submit_proof` arrays while adapter mapping still rejects each surface without a consumed approval.

- [x] T049AN Add failing product-matrix test for approval-gated status after product-proof binding.
- [x] T049AO Add `approval_gated` submit status and remove stale missing-proof gaps for all four Hyperliquid surfaces.

## Phase 8N - Static Hyperliquid Canary Proof Artifacts

**Goal**: Operators can generate shared canary proof gate-session, candidate-source, and order-intent artifacts for a static Hyperliquid instrument through the production provider collector path.

**Independent Test**: A static Hyperliquid target with TOML-owned sizing constraints and an enabled proof policy writes a no-resolution gate session plus canary candidate/order-intent artifacts bound to the configured Hyperliquid execution client.

- [x] T049AP Add failing operator-artifact test for Hyperliquid static instrument canary proof collection.
- [x] T049AQ Add TOML-owned static target sizing constraints and selected-market identity for Hyperliquid static instruments.
- [x] T049AR Register the Hyperliquid canary proof artifact collector with the provider binding and reuse the shared proof-policy projection.

## Phase 8O - External Review Hardening

**Goal**: Address external-review safety gaps without widening live-submit authority.

**Independent Test**: Non-HIP-4 Hyperliquid clients reject `updown` targets even with a consumed non-HIP-4 approval, and test-only submit-admission helpers require the caller's execution client id instead of hardcoding a venue client.

- [x] T049AS Add failing adapter-mapping test for `updown` targets on non-HIP-4 Hyperliquid product surfaces.
- [x] T049AT Reject `updown` targets unless the Hyperliquid execution client selects `hip4_outcomes`, including the pre-consumption approval-loading path.
- [x] T049AU Parameterize the test-only submit-admission helper by execution client id to avoid provider-cap masking in future venue tests.

## Phase 9 - Verification

- [x] T050 Run `cargo fmt --check` for `Cargo.toml` and `src/`.
- [x] T051 Run `cargo clippy --locked --lib -- -D warnings` for `src/lib.rs`.
- [x] T052 Run focused provider tests for `tests/bolt_v3_provider_binding.rs`.
- [x] T053 Run focused entrypoint tests for `tests/bolt_v3_production_entrypoint.rs`.
- [x] T054 Run focused Hyperliquid matrix tests for `tests/hyperliquid_product_matrix.rs`.
- [x] T055 Run focused fail-closed tests for `tests/hyperliquid_fail_closed.rs`.
- [x] T056 Produce evidence packet in `specs/025-hyperliquid-execution-adapter/quickstart.md`.

## Dependencies

- Phase 1 and Phase 2 block all implementation.
- US1 blocks US2-US6.
- US2 blocks US3-US5.
- US3 blocks US4.
- US6 can run after US1.
- Phase 9 runs after each changed vertical slice and once at the end.

## MVP Scope

MVP is US1, US2, US3 fail-closed guards, US5 approval-gated mapping scaffolding, and US6 ops metadata. US4 live standard-perps submit remains gated follow-up unless user explicitly approves that slice after MVP proof.
