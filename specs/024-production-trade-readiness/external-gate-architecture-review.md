# External Gate Architecture Review

**Head**: `3378c0965740658b9f655045827eb64b8449019e`
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
- `config/strategies/binary_oracle.toml`
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
- The canonical shipped strategy TOML and related fixtures must be migrated with the schema change; otherwise the shipped config will keep the old Chainlink-shaped runtime fields.

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

## Contract Review Round 5

The follow-up patch was committed as `3378c0965740658b9f655045827eb64b8449019e` and sent for final delta review against `af4c927a41651892bcc0f869b863fe90d71f2863..3378c0965740658b9f655045827eb64b8449019e`.

- Claude Code job `373cb414-9af5-41e6-94e7-de791583c1e1`: APPROVE.
- Gemini job `ff3e7751-fe94-4200-9819-84c92d1a352a`: APPROVE.
- Grok custom-file job `job_be545773-1fb2-4f17-8564-f0f715378f73`: REQUEST_CHANGES due to missing diff/tree evidence only; superseded by the branch-diff slot below.
- Grok branch-diff job `job_3f4435ed-ee26-406e-b6bf-97bb1007d503`: APPROVE.
- GLM job `job_7037569f-0518-49b1-9ae0-e8c149c83254`: APPROVE.
- DeepSeek job `job_472f7acc-7655-4ff1-b881-da7e026450e5`: REQUEST_CHANGES.

Accepted DeepSeek Round 5 finding:

- `provider_provenance_sha256` must specify the canonical JSON shape for every `ProviderProvenance` variant, including discriminator field, field names, numeric/string encoding, and hash input.

Revision applied after Round 5:

- `gate-dataflow-contract.md` now defines flat tagged canonical provider provenance JSON for Chainlink Data Streams, Pyth, Binance index, venue-native, and test-double provenance before hashing to `provider_provenance_sha256`.
- `gate-dataflow-contract.md` now makes `normalized_value_scale` a JSON number in the session hash example.
- `tasks.md` T036H4 now explicitly requires RED coverage for rejecting `|` in selected-market key components.

## Contract Review Round 6

The Round 5 follow-up patch was committed as `2f95b2c4a894e13012c5fc3a0ee2bfadbae0b591` and sent for exact-delta review against `3378c0965740658b9f655045827eb64b8449019e..2f95b2c4a894e13012c5fc3a0ee2bfadbae0b591`.

- Claude Code job `f5ae5d77-6378-4053-b21e-76f5ed6ad295`: APPROVE.
- Gemini job `e746dea2-4574-4ddc-8ee7-4d834fd1a79d`: APPROVE.
- DeepSeek job `job_17a25ce8-6022-424c-9751-6373c3f3a1a1`: APPROVE.
- GLM job `job_603fdd23-4914-455c-9dd1-d2fd249ce37e`: APPROVE.
- Grok branch-diff job `job_2be6f5b9-ffc3-425a-9540-3cef54483d90`: APPROVE.

Non-blocking Round 6 dispositions:

- Numeric provider provenance examples use concrete JSON numbers intentionally to prove schema and scale fields are JSON numbers, not strings.
- `normalized_value_decimal` remains a string intentionally because the session canonicalization rule says runtime decimal numbers are rendered as strings.
- `provider_provenance_sha256` inherits the existing lowercase hex SHA-256 and UTF-8 canonical JSON convention from the session hash canonicalization section.
- `|` rejection is already normative in the selected-market fail-closed contract and is also required by T036H4 RED coverage.
- Provider-kind closure is intentional; adding a new external provider kind requires a contract update before implementation can depend on it.

## Contract Review Round 7

After PR #487 was merged and PR #480 was final-tree synced to NT 0.58, the cleaned provider-agnostic T036H0D/T036H0E/T036H0F packet was sent as a custom-file adversarial review for the current readiness cleanup scope:

- `specs/024-production-trade-readiness/tasks.md`
- `specs/024-production-trade-readiness/gate-dataflow-contract.md`
- `specs/024-production-trade-readiness/spec.md`
- `specs/024-production-trade-readiness/plan.md`
- `specs/024-production-trade-readiness/evidence.md`
- `Cargo.toml`
- `Cargo.lock`

Review jobs:

- Claude Code job `e321f803-da4e-4970-96fe-a29a3b08694c`: APPROVE, with substantive non-blocking findings.
- Gemini job `00fc1acd-41d4-4a40-b591-563449324455`: APPROVE.
- Grok job `job_13b17658-fe5f-4b14-9508-f8089c0f4e22`: APPROVE.
- DeepSeek job `job_f5b951ae-d74d-496f-886f-a33c96ffa217`: APPROVE.
- GLM job `job_53401a4d-813c-46bf-be30-df18fd6b9f2a`: APPROVE.

Accepted Round 7 findings:

- `selected_market_key` derivation was still underspecified. The contract needed a concrete canonicalization and hash algorithm before RED tests depend on it.
- `market_metadata` was listed as a provider capability but the contract did not explicitly say it is not a readiness `GateRole` and does not create a `target.gate_subscriptions.market_metadata` join path.
- `instrument_ids` needed to say whether it represents the complete market instrument/outcome set or only the strategy-traded subset.

Revision applied after Round 7:

- `gate-dataflow-contract.md` now defines `selected_market_key` as lowercase hex SHA-256 over canonical selected-market identity JSON.
- `gate-dataflow-contract.md` now excludes `selected_at_ms` from `selected_market_key` and keeps it in the gate session hash.
- `gate-dataflow-contract.md` now requires `instrument_ids` to be the market-complete, lexicographically sorted instrument/outcome id set; strategy-specific traded subsets must live outside selected-market identity.
- `gate-dataflow-contract.md` now states that `market_metadata` is a provider capability used to build or validate selected-market identity and `metadata_provenance_sha256`; it is not a `GateRole`, does not create a target subscription, and cannot satisfy entry readiness by itself.
- `tasks.md` T036H2 and T036H4 now require RED coverage for these clarifications.
- `Cargo.toml` now keeps `nautilus-portfolio` grouped with the other Nautilus dependencies.

Non-blocking Round 7 dispositions:

- The exact `alloy-primitives = "=1.6.0"` pin remains intentional because the T036H0E sync resolved it to NT 0.58's required alloy line and evidence records that reason.
- Existing test/dev dependency cleanup is outside this T036H0F contract scope unless it becomes build-affecting.
- Concrete crypto-looking example strings remain examples only; the normative selected-market identity and task tests now require provider-neutral canonicalization.
