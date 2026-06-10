# Tasks: World Cup Production Profit Gate

**Input**: `specs/032-world-cup-production-profit-gate/`
**Prerequisites**: `spec.md`, `plan.md`, `research.md`, `data-model.md`, `contracts/`

## Phase 1: Setup

- [x] T001 Confirm branch starts from current `origin/main` and working tree is clean.
- [x] T002 Confirm `AGENTS.md` and `.specify/feature.json` remain pinned to `specs/023-nt-order-intent-layer`.
- [x] T003 Run baseline `cargo test --locked --lib`.
- [x] T004 Add module declarations for source proof, provider capability, profit evidence, and live enablement gate using existing crate style.

## Phase 2: Source-Proof Admission

- [x] T005 Add `EventMarketSourceProof` types in `src/bolt_v3_event_market_source_proof.rs`.
- [x] T006 Add validation for missing official event source URL/hash.
- [x] T007 Add validation for stale official event and venue term proof.
- [x] T008 Add validation for missing or conflicting resolution rules.
- [x] T009 Add validation for jurisdiction/account/product unavailability.
- [x] T010 Add source-proof rejection reason serialization.
- [x] T011 Add tests for accepted and rejected source-proof bundles.

## Phase 3: Provider Capability And Quorum

- [x] T012 Add `ProviderCapabilityProof` types in `src/bolt_v3_provider_capability.rs`.
- [x] T013 Add provider-neutral transport/update semantics classification.
- [x] T014 Add direct-source proof validation and aggregator-source labeling.
- [x] T015 Add plan entitlement and capability expiry validation.
- [x] T016 Add `ReferenceQuorumPolicy` validation from TOML-owned fields.
- [x] T017 Add stale/lost/veto quorum rejection tests.

## Phase 4: Profit Evidence

- [x] T018 Add `ProfitEvidenceSession` in `src/bolt_v3_profit_evidence.rs`.
- [x] T019 Bind existing executable-edge decisions and exact-size VWAP evidence.
- [x] T020 Bind existing order-book-depth, fee, and submit-admission evidence.
- [x] T021 Add fill, no-fill, cancel, markout, and settlement evidence references.
- [x] T022 Reject positive edge without execution-quality evidence.
- [x] T023 Reject lower-fidelity backtest sessions for capital-scale promotion.
- [x] T024 Add tests for accepted and rejected profit-evidence sessions.

## Phase 5: Disabled Promotion Package

- [x] T025 Add promotion package artifact type and verifier.
- [x] T026 Generate disabled typed TOML only.
- [x] T027 Bind generated config to source-proof, provider-proof, profit-evidence, commit SHA, and config checksum.
- [x] T028 Reject generated packages that attempt to enable live execution.
- [x] T029 Add tests proving promotion cannot mutate SSM, venue state, orders, or funds.

## Phase 6: Live Enablement Gate Integration

- [x] T030 Add `LiveEnablementGate` verifier in `src/bolt_v3_live_enablement_gate.rs`.
- [x] T031 Consume exact-head CI/source-fence/controlled-connect evidence hashes.
- [x] T032 Consume capital-probe proof and operator approval hashes.
- [x] T033 Scope gate acceptance to exact venue/account/product/market-family/config hash.
- [x] T034 Reject missing/stale/mismatched controlled-connect and capital-probe artifacts.
- [x] T035 Add tests for each state transition and rejection reason.

## Phase 7: Verification And Review

- [x] T036 Run `cargo fmt --check`, `cargo test --locked --lib`, `git diff --check`, and `just source-fence`.
- [ ] T037 Request exact-head CI only after local checks pass.
- [ ] T038 Do not request external review until exact-head CI is green and all local deltas are committed.

## Dependencies

- T005-T011 block T018-T024.
- T012-T017 block T018-T024.
- T018-T024 block T025-T029.
- T025-T029 block T030-T035.
- T036-T038 are final gates.

## Parallel Work

- T005-T011 and T012-T017 can proceed in parallel after setup.
- T018-T024 can run in parallel with CLI artifact design only after source and provider hashes are stable.
- Documentation review can proceed while tests are being implemented, but review cannot replace tests.
