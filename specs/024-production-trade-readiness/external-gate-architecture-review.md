# External Gate Architecture Review

**Head**: `e8eb1f31d0bc71cebbbd73df76acbf7e1fd1dab3`
**Date**: 2026-05-25
**Scope**: corrected production-readiness resolution/reference gate model for T036H.

## Source Packet

- `src/bolt_v3_config.rs`
- `src/bolt_v3_validate.rs`
- `src/bolt_v3_market_families/mod.rs`
- `src/bolt_v3_market_families/updown.rs`
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- `src/bolt_v3_providers/mod.rs`
- `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs`
- `config/strategies/binary_oracle.example.toml`
- `specs/024-production-trade-readiness/tasks.md`

## Reviewer Verdicts

- Claude Code job `d3fc3370-e386-4cbe-aa99-0343cc43377b`: REQUEST_CHANGES.
- Gemini job `52e8d7e1-a657-4726-aab4-03c422e28330`: REQUEST_CHANGES.
- Grok job `job_035baa62-c0d7-4752-bfd3-586f28a9dd0a`: REQUEST_CHANGES.
- DeepSeek job `job_180fc63e-6f3a-4cd0-9d0a-a88da2a1fdf6`: REQUEST_CHANGES.
- GLM job `job_2d3a8f36-9dce-4279-94c0-35ea80fe97d5`: REQUEST_CHANGES.
- Kimi: operator-waived because prior Kimi attempts did not produce useful verdicts.

## Disposition

The review findings are accepted for T036H scope. Production code must not start from the pre-review model where `strategy_instance_id + configured_target_id + gate_role` maps to a single concrete root gate id.

The revised model is:

- Root config owns reusable gate provider definitions, credentials, endpoints, freshness policy, capabilities, and provider-specific validation schema.
- Strategy archetypes own required gate roles/classes only, for example reference price, resolution proof, volatility, or no-resolution.
- Configured strategy targets own allowed gate subscriptions per role, not one mandatory concrete provider. A role may allow one provider kind, several provider kinds, or explicit no-resolution where the market class permits it.
- Market-family code owns venue/instrument metadata extraction into provider-neutral selected-market requirements.
- Selected markets carry observed or config-resolved requirement metadata such as market class, resolution kind, resolution identity, source condition, and market slug. They do not carry operator config gate ids.
- Entry readiness performs the join across archetype role, target subscription, selected-market requirement, provider capability, and evidence. It must fail closed on missing metadata, missing configured mapping, provider mismatch, stale evidence, or cross-market evidence reuse.
- Entry readiness creates the validated gate/evidence session consumed by strategy logic. The strategy must not re-open an unchecked provider path after readiness passes.
- Reference gates and resolution gates have separate matching rules. Chainlink is one possible resolution/reference provider, not a global requirement.

## Code Evidence Checked

- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` currently keeps Chainlink-shaped runtime fields: `price_to_beat_feed_id`, `price_to_beat_report_schema_version`, `price_to_beat_report_decimal_scale`, and `forced_flat_stale_chainlink_ms`.
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` currently validates Chainlink feed-id shape inside archetype parameter validation.
- `src/bolt_v3_market_families/mod.rs` currently exposes `SelectedMarketSourceIdentity` with condition id, market slug, and question id only.
- `src/bolt_v3_market_families/updown.rs` currently derives selected identity from BinaryOption `info` fields `market_id`, `condition_id`, `market_slug`, and `question_id`; there is no current resolution kind or resolution identity path.
- `src/bolt_v3_validate.rs` currently enforces globally unique `configured_target_id`, so a key that includes both strategy id and target id is not the actual dynamic join key.

## Required RED Coverage

- Static single-gate subscription cannot pass when selected market rotates to a different required resolution kind.
- Missing selected-market resolution metadata fails with an explicit metadata-unavailable error.
- Config-owned mapping can resolve provider identity only when the selected market/family/asset key matches; wrong or missing mapping fails closed.
- Provider-specific fields are rejected from archetype runtime parameters and accepted only under the matching gate provider/subscription validation owner.
- Reference gate evidence cannot satisfy resolution gate requirements, and resolution evidence cannot satisfy reference gate requirements.
- No-resolution markets can pass only when the archetype role is optional or explicitly no-resolution-compatible.
- Gate evidence is bound to selected market identity and role; evidence from a previous selected market cannot satisfy a later rotated market even when still fresh by timestamp.
- Strategy logic receives a readiness-created gate session or normalized evidence object; tests must prevent a second unchecked provider path.

## Plan Review Round 2

The revised plan was sent for a second adversarial review on the same head and 12-file packet.

- Claude Code job `7fa93f4c-5ae7-46d5-9c0e-dfa827c7e915`: REQUEST_CHANGES.
- Gemini job `975137e7-7e19-42e9-a4a9-a23ae70f93d9`: REQUEST_CHANGES.
- Grok job `job_71a1e368-bd93-42df-a45d-23b3e6384d6a`: REQUEST_CHANGES.
- DeepSeek job `job_3c0ec3ff-c469-4c5f-ae70-2a83c92bf69b`: REQUEST_CHANGES.
- GLM job `job_eec45652-c2c5-422d-9078-03ecfa2a749f`: REQUEST_CHANGES.

Accepted plan-review findings:

- The plan must define the TOML gate schema before RED tests depend on that schema.
- The plan must include explicit RED coverage for the no-bypass gate-session invariant.
- The plan must include explicit RED coverage for no-resolution markets with mandatory vs optional resolution roles.
- The plan must include explicit RED coverage for reference evidence never satisfying resolution evidence, and resolution evidence never satisfying reference evidence.
- The plan must include explicit RED coverage for config-owned provider mapping mismatches.
- The implementation task must be split into reviewable sub-slices instead of one broad T036H4 task.
- The implementation scope must include the strategy registration and strategy consumption path, not only config, market-family, provider, and operator-artifact files.
- The example strategy TOML and related fixtures must be migrated with the schema change; otherwise the shipped example will keep the old Chainlink-shaped runtime fields.

## Contract Review Round 3

The T036H contract packet was sent for contract-only review on the same branch head plus uncommitted spec/task source packet:

- Claude Code job `5063440c-bfca-45e4-88ff-4acc9bf87742`: APPROVE with precision concerns.
- Gemini job `a0270d63-d683-441e-a569-f51c1366c9ab`: REQUEST_CHANGES.
- Grok job `job_1ab16914-e980-46ed-98b7-f6dc1c9b7f3d`: REQUEST_CHANGES.
- DeepSeek: not rerun in this round before revision because Gemini/Grok already identified blocking contract gaps.
- GLM: not rerun in this round before revision because Gemini/Grok already identified blocking contract gaps.

Accepted contract-review findings:

- The contract must include canonical TOML examples for root `gate_providers`, target `gate_subscriptions`, provider-specific subtables, market mappings, freshness, provider preference, and no-resolution targets.
- The contract must define positive archetype role declaration shape, selected-market canonical key, `GateEvidence`, `EntryReadinessGateSession`, `GateSatisfaction`, and the join algorithm before RED tests start.
- The contract must define deterministic behavior when multiple providers satisfy the same role.
- The contract must define the freshness comparison clock, source/collector timestamp skew behavior, and provider timeout/partial-response fail-closed behavior.
- The task list must include positive CLI acceptance for provider-neutral `--gate-session` and `--expected-gate-session-sha256`, not only legacy flag rejection.
- The task list must include live-canary gate coverage, final-packet gate-session binding, and replay/helper no-bypass coverage outside only the strategy file.
- The contract packet must include hard code evidence for the current Chainlink-shaped path and the exact consumer surfaces affected.

Revision applied after Round 3:

- `gate-dataflow-contract.md` now contains canonical TOML, canonical object shapes, join algorithm, consumer contracts, live-canary boundary, CLI acceptance/rejection contract, final-packet binding contract, and file:line current-code evidence.
- `tasks.md` now expands T036H1-T036H12A with RED coverage for config, target subscription, archetype, selected market, provider evidence, entry join, lifecycle freshness, decision evidence, tiny canary, live canary, CLI, registration/runtime/replay, and final-packet binding.
- `spec.md` now adds FR-017 and SC-010 for live-canary/final-packet readiness gate-session binding.
- `plan.md` now includes `src/bolt_v3_live_node.rs` and names live-canary/final-packet consumption in the gate model.

## Contract Review Round 4

The revised contract packet was committed as `af4c927a41651892bcc0f869b863fe90d71f2863` and sent for exact-head contract review.

- Claude Code job `6ce1dc6b-a0af-41ed-ad9c-ec764ecb7f51`: APPROVE.
- Gemini job `e00b813d-d41a-4e86-ae54-78435a14a751`: APPROVE.
- Grok job `job_e22d14e7-2265-44dd-bc50-fbefd3471477`: REQUEST_CHANGES.
- DeepSeek job `job_49805002-e88d-47b2-810a-27dda275ee1f`: APPROVE.
- GLM job `job_7ec64c78-a336-4c4a-a16e-cf428b94900a`: APPROVE.

Accepted Grok Round 4 findings:

- `session_hash` construction must specify hash algorithm, canonical serialization, field order, no-resolution representation, provenance hashing, and artifact ordering before RED tests depend on it.
- Archetype gate requirements need a positive exposure mechanism, not only a negative prohibition on provider-specific runtime fields.
- The RED slice must explicitly cover the live-node build/registration path so a later live-canary gate cannot backfill missing registration evidence.

Revision applied after Round 4:

- `gate-dataflow-contract.md` now pins `session_hash` to lowercase hex SHA-256 over canonical JSON with sorted object keys, sorted role/provenance/artifact inputs, and explicit `NoResolution` hash material.
- `gate-dataflow-contract.md` now defines binary-oracle positive gate requirement exposure through `binary_oracle_edge_taker::gate_requirements() -> Vec<ArchetypeGateRequirement>`.
- `gate-dataflow-contract.md` now defines canonical enum/value/provenance/helper shapes, selected-market key delimiter rejection, `test_double` test-only behavior, provider id/kind coexistence semantics, and provider collector positive binding behavior.
- `tasks.md` now adds T036H12B for live-node registration wiring and expands T036H1, T036H3, and T036H11 for test-double rejection, archetype requirement exposure, and provider collector positive CLI coverage.
