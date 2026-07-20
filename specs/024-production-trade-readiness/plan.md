# Implementation Plan: Production Trade Readiness

**Branch**: `goal/024-production-trade-readiness`
**PR**: #480
**Spec**: `specs/024-production-trade-readiness/spec.md`
**Tasks**: `specs/024-production-trade-readiness/tasks.md`

## Technical Context

**Language**: Rust
**Primary implementation files**: `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `src/bolt_v3_market_families/mod.rs`, `src/bolt_v3_market_families/updown.rs`, `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, `src/bolt_v3_providers/mod.rs`, `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs`, `src/bolt_v3_operator_artifacts.rs`, `src/bolt_v3_decision_evidence.rs`, `src/bolt_v3_tiny_canary_evidence.rs`, `src/bolt_v3_live_node.rs`, `src/bolt_v3_strategy_registration.rs`, `src/strategies/binary_oracle_edge_taker.rs`, `src/main.rs`, `config/strategies/binary_oracle.toml`
**Primary tests**: `tests/config_parsing.rs`, `tests/bolt_v3_operator_artifacts.rs`, `tests/bolt_v3_strategy_registration.rs`, `tests/bolt_v3_tiny_canary_preconditions.rs`, `tests/bolt_v3_tiny_canary_operator.rs`, `tests/bolt_v3_live_canary_gate.rs`, `tests/bolt_v3_cli.rs`
**Verification**: focused Rust tests, `cargo fmt --check`, `git diff --check`, runtime-literal verifier, source/slop/hardcode/secret scans, GitHub CI, external model review.

## Evidence Baseline

The current investigation found:

- PR #480 is the active production trade-readiness consolidation PR on `goal/024-production-trade-readiness`.
- Historical PR #478 was closed by GitHub after the stale branch was renamed; it is superseded by #480 and must not be treated as active readiness scope.
- PR #479 is separate #466 verifier decomposition work and is out of scope.
- #369 and #385 are open.
- #409 is open, but current source already contains PortfolioSnapshot capture; the task list must verify whether this is ready to close or still needs issue evidence.
- #360 is closed and remains historical tiny-canary readiness context, not production-readiness completion.
- T038 no-submit evidence is historically satisfied only for no-submit; final-packet T131/T122 remains unproven.
- The old `t038-operator-config-snapshot` branch has unique commits, but current source contains later no-submit/SBE work and recorded EC2/EIP no-submit proof. It must not be ported wholesale.
- The active readiness branch currently has source collectors for release manifest, host clock, market window, single-runner lock, and cancel-if-open.
- The active readiness branch now exposes collector functions for venue account/open orders/positions, funding/margin, egress identity, CLOB V2 signing/collateral/fee behavior, NT accepted/venue pending, partial fill, network partition, and panic/service policy.
- External T036H architecture review rejected the single concrete gate-id subscription model. The current code still has Chainlink-shaped archetype runtime fields and selected-market identity without generic resolution kind/identity/value-kind provenance, so T036H must revise the official gate model before implementation resumes.
- Second T036H plan review rejected the revised task list because it lacked a concrete TOML schema contract, lacked no-bypass/session RED coverage, lacked no-resolution and reference-vs-resolution negative tests, omitted strategy registration/consumption files, and kept one monolithic implementation task.
- End-to-end T036H investigation found that provider-specific readiness also flows through decision evidence, tiny-canary evidence, CLI artifact commands, and source replay. Those are mandatory contract boundaries, not cleanup.
- PR #487 merged the NT 0.58 bump into `main`, including upstream HIP-4 support. PR #480 must be synced to that mainline before T036H RED work, and the official gate contract must not exclude HIP-4, Deribit, outcome-oracle, sports, politics, entertainment, venue-native, or no-resolution markets through a closed provider or price-only schema.
- Hyperliquid HIP-4 is upstream NT-owned after the NT 0.58 bump. Bolt-v3 must add a thin readiness binding over that existing NT support, not rebuild a Hyperliquid/HIP-4 adapter.

See `specs/024-production-trade-readiness/evidence.md` for commands and exact outputs summarized.

## Constraints

- One readiness PR.
- No order-intent-layer work.
- No #466 decomposition-ledger work.
- No hardcoded runtime values.
- SSM remains the only secret source.
- No secret display.
- No live/no-submit/trading operations until the prerequisite artifacts and verification chain are ready. The operator has approved the listed operations, but approval does not bypass prerequisites.

## Gate Model Contract

The authoritative T036H dataflow contract is `specs/024-production-trade-readiness/gate-dataflow-contract.md`. The implementation must converge on this owner model before final-packet assembly:

- Root config owns `[gate_providers.<provider_id>]` blocks. Each provider declares registry-backed `provider_kind`, semantic `capabilities`, `client_id` or provider-owned connection fields, freshness policy, and exactly one provider-specific subtable such as `[gate_providers.<id>.chainlink_data_streams]` or `[gate_providers.<id>.hyperliquid_hip4]`.
- Provider-specific values such as feed ids, venue metadata scopes, report schema version, report decimal scale, endpoint, and credential references are valid only inside the matching gate provider block.
- Strategy archetypes declare required gate roles/classes/value-kinds only. They do not declare provider ids, feed ids, venue metadata scopes, report schema versions, decimal scales, endpoints, or stale windows.
- Strategy targets declare `[target.gate_subscriptions.<role>]` blocks. Each block declares whether the role is required, the allowed provider ids or provider kinds, accepted value kinds, deterministic provider preference when multiple providers match, whether no-resolution is compatible, and any config-owned market/family/asset mapping needed to resolve provider identity when venue metadata does not provide it.
- Selected markets carry observed or config-resolved generic requirement metadata: target id, venue, family, market id, instrument/outcome ids, `market_class`, `resolution_kind`, `resolution_identity`, `value_kind`, and metadata provenance. Selected markets do not own gate roles or root provider ids.
- Entry readiness performs the join across archetype role, target subscription, selected-market requirement, provider capability, and evidence. The join is keyed by selected-market identity and role, not by a single static target-to-gate id.
- Entry readiness returns an opaque gate session or normalized evidence object. Decision evidence, tiny-canary evidence, live-canary gates, CLI artifact commands, strategy registration, runtime strategy logic, final-packet binding, and replay helpers must consume that object and must not construct or fetch provider evidence through an unchecked second path.
- Strategy runtime files produce strategy-local signal state and order intent only. Execution admissibility, venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, and submit gating belong in shared execution/admission modules that use NT APIs. A strategy-file submit-mechanics change is out of scope unless the operator explicitly records it as strategy-local signal logic.

## Strategy

1. Finish task-list approval first.
2. Remove PR #480 scope contamination before deeper implementation.
3. Sync PR #480 with current `main` after the NT 0.58/HIP-4 merge and remove misleading static reference-data substitutions.
4. Implement missing source-owned evidence collectors in TDD slices.
5. Replace the hardcoded Chainlink price-to-beat assumption with a provider-neutral, value-kind-aware resolution/reference gate model.
6. Produce real current-head runtime artifacts.
7. Assemble and verify final packet.
8. Run final exact-head verification.
9. Run approved final-packet no-submit, then tiny-capital canary.
