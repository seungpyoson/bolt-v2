# Tasks: Global Multi-Venue Robust RV Runtime

**Input**: Design documents from `specs/027-global-rv-surface-runtime/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`
**Tests**: Required. This work must be implemented with TDD after plan approval.
**Scope**: Full scope only. Do not split out the global runtime, multi-venue production wiring, or robust estimator work unless the owner explicitly changes scope in writing.

## Phase 1: Setup and Guardrails

- [ ] T001 Confirm branch starts from current `main` after PR #609 merge and has no unrelated local changes.
- [ ] T002 Update `.specify/feature.json` to point at `specs/027-global-rv-surface-runtime`.
- [ ] T003 Update the Speckit pointer in `AGENTS.md` to `specs/027-global-rv-surface-runtime/plan.md`.
- [ ] T004 Add or update a source-fence fixture list in `tests/bolt_v3_realized_volatility_source_fence.rs` for global runtime ownership terms.
- [ ] T005 Verify exact branch head CI is green before external review requests; prefer GitHub CI over broad local cargo tests.

## Phase 2: TDD Foundations

- [ ] T006 [P] RED: Add a source-fence test proving `src/strategies/**` contains no `RealizedVolEngine`, `realized_vol_engine`, or RV runtime construction.
- [ ] T007 [P] RED: Add a source-fence test proving RV subscription creation is not strategy-owned.
- [ ] T008 [P] RED: Add a source-fence test proving pricing/strategy consumers do not apply raw RV predicates such as `is_positive_finite` or `is_non_negative_finite` to snapshot RV fields.
- [ ] T009 [P] RED: Add a config validation test that every strategy needing RV requires `realized_volatility_surface_id` and rejects all legacy `vol_*` fields as unknown.
- [ ] T010 [P] RED: Add an evidence schema test requiring runtime-level robust RV fields and a schema-version bump.
- [ ] T011 GREEN: Add only enough scaffolding to compile the new test module names and failing assertions without implementing behavior.

## Phase 3: User Story 1 - Global Runtime Outside Taker

**Goal**: A process-level runtime owns all RV surfaces, and taker/maker/future consumers only consume snapshots.

- [ ] T012 RED: Add `tests/bolt_v3_realized_volatility_runtime.rs` test `runtime_builds_all_surfaces_from_root_config`.
- [ ] T013 RED: Add runtime test `runtime_publishes_snapshot_by_surface_id_for_multiple_consumers`.
- [ ] T014 RED: Add runtime test `runtime_fails_loudly_when_strategy_references_missing_surface`.
- [ ] T015 RED: Add runtime test `strategy_does_not_need_market_signal_feed_to_warm_rv_surface`.
- [ ] T016 GREEN: Implement `src/bolt_v3_realized_volatility_runtime.rs` with root-config construction, surface map, and snapshot lookup.
- [ ] T017 GREEN: Move RV surface construction from `src/strategies/binary_oracle_edge_taker/mod.rs` into the global runtime build path.
- [ ] T018 GREEN: Remove any `RealizedVolEngine` fields, constructor calls, and refresh ownership from binary oracle taker strategy state.
- [ ] T019 GREEN: Add a runtime snapshot provider/accessor that `src/bolt_v3_taker_pricing.rs` can consume by `surface_id`.
- [ ] T020 GREEN: Wire binary oracle taker to consume runtime snapshots only.
- [ ] T021 GREEN: Wire binary oracle maker or the nearest maker-side consumer to select a surface snapshot without constructing RV state.
- [ ] T022 REFACTOR: Keep strategy code intent-only; any subscription, route, quorum, readiness, or aggregation code belongs outside strategies.

## Phase 4: User Story 2 - Multi-Venue Available Sources

**Goal**: Production RV surfaces use every configured available public source instead of one hardcoded/single venue.

- [ ] T023 RED: Add root-config validation test `surface_source_references_existing_public_client_and_instrument`.
- [ ] T024 RED: Add runtime test `deduplicates_physical_subscriptions_and_fans_out_to_multiple_sources`.
- [ ] T025 RED: Add runtime test `two_available_venue_sources_contribute_to_one_surface_snapshot`.
- [ ] T026 RED: Add runtime test `disabled_source_is_audited_but_not_subscribed`.
- [ ] T027 RED: Add runtime test `enabled_non_quorum_source_with_live_observations_remains_diagnostic_only`.
- [ ] T028 GREEN: Implement `RealizedVolSourceRoute` and `PhysicalSubscriptionKey` in the runtime module.
- [ ] T029 GREEN: Move RV subscription request generation out of strategy subscription methods.
- [ ] T030 GREEN: Implement observation fan-out from one physical stream to every configured source route.
- [ ] T031 GREEN: Extend root validation to reject source client/instrument drift before runtime starts.
- [ ] T032 GREEN: Update `config/root.toml` surfaces to include all available configured venue/source mappings per asset.
- [ ] T033 GREEN: Keep strategy TOMLs limited to `realized_volatility_surface_id` selectors.
- [ ] T034 REFACTOR: Ensure no asset, token, venue, provider, timeout, or subscription policy is hardcoded in Rust.

## Phase 5: User Story 3 - Multi-Horizon RV

**Goal**: Replace single-window brittleness with auditable short/medium/long horizon estimates.

- [ ] T035 RED: Add engine test `multi_horizon_source_requires_required_horizons_only`.
- [ ] T036 RED: Add engine test `optional_long_horizon_missing_does_not_block_surface_when_policy_allows`.
- [ ] T037 RED: Add engine test `weighted_blend_normalizes_ready_required_horizon_weights`.
- [ ] T038 RED: Add engine test `long_horizon_floor_prevents_understated_short_window_zero_vol`.
- [ ] T039 RED: Add config validation tests for horizon uniqueness, positive windows, window >= interval, coverage bounds, and positive total required weight.
- [ ] T040 GREEN: Add horizon config structs to `src/bolt_v3_config.rs` and validation in `src/bolt_v3_validate.rs`.
- [ ] T041 GREEN: Extend `src/bolt_v3_realized_volatility.rs` to compute per-horizon fixed-grid RV per source.
- [ ] T042 GREEN: Implement final horizon policies `weighted_blend`, `max_floor`, and `short_with_long_floor`.
- [ ] T043 GREEN: Publish per-horizon estimates in `RealizedVolSnapshot` and diagnostics.
- [ ] T044 REFACTOR: Keep `ReadyRealizedVol` as the only consumer-facing numeric contract.

## Phase 6: User Story 4 - Microstructure-Noise Robustness

**Goal**: Reduce false high-frequency volatility from quote midpoint bounce without hiding the base estimate.

- [ ] T045 RED: Add engine test `subsampled_rv_reduces_alternating_bid_ask_bounce_vs_base_grid`.
- [ ] T046 RED: Add engine test `coarser_grid_rv_remains_audited_next_to_base_fixed_grid_rv`.
- [ ] T047 RED: Add config validation tests for `noise_robust_method`, `subsamples`, and `coarse_sampling_interval_ms`.
- [ ] T048 GREEN: Implement `noise_robust_method = "none"` as current fixed-grid behavior.
- [ ] T049 GREEN: Implement `noise_robust_method = "coarser_grid"` per horizon.
- [ ] T050 GREEN: Implement `noise_robust_method = "subsampled"` per horizon with deterministic offset grids.
- [ ] T051 GREEN: Emit base fixed-grid RV and noise-robust RV separately in diagnostics/evidence.
- [ ] T052 REFACTOR: Keep all noise-robust parameters TOML-owned and engine-owned.

## Phase 7: User Story 5 - Jump Separation

**Goal**: Separate jump component from continuous RV instead of deleting jumps.

- [ ] T053 RED: Add engine test `single_large_jump_increases_jump_component_without_erasing_measured_rv`.
- [ ] T054 RED: Add engine test `flat_source_publishes_zero_continuous_and_zero_jump_rv`.
- [ ] T055 RED: Add evidence test `jump_component_is_serialized_separately_from_final_rv`.
- [ ] T056 RED: Add config validation tests for jump policy and threshold bounds.
- [ ] T057 GREEN: Implement `jump_policy = "none"` as measured RV passthrough.
- [ ] T058 GREEN: Implement `jump_policy = "separate"` using an auditable bipower/truncated-return style first implementation.
- [ ] T059 GREEN: Emit measured, continuous, and jump annualized RV components per horizon/source/surface.
- [ ] T060 REFACTOR: Do not suppress real jumps silently; pricing selection must be explicit in config/evidence.

## Phase 8: User Story 6 - Robust Cross-Source Aggregation

**Goal**: Make multi-venue RV useful by protecting against one bad feed while preserving fail-closed dispersion behavior.

- [ ] T061 RED: Add engine test `median_aggregation_ignores_one_extreme_ready_source_when_quorum_satisfied`.
- [ ] T062 RED: Add engine test `trimmed_mean_requires_enough_ready_sources_for_trim_policy`.
- [ ] T063 RED: Add engine test `mad_dispersion_blocks_when_ready_sources_disagree_too_much`.
- [ ] T064 RED: Add engine test `upper_quantile_guard_selects_conservative_value_after_median_estimate`.
- [ ] T065 RED: Add engine test `source_level_not_warm_does_not_block_satisfied_partial_quorum` to preserve PR #609 fix.
- [ ] T066 GREEN: Extend aggregation config with `median`, `trimmed_mean`, and `median_with_upper_quantile_guard`.
- [ ] T067 GREEN: Implement MAD dispersion diagnostics and blocker.
- [ ] T068 GREEN: Keep source-level blockers out of surface blockers when quorum is satisfied.
- [ ] T069 GREEN: Preserve unknown-source, disabled-source, and non-quorum diagnostics.
- [ ] T070 REFACTOR: Ensure aggregation never pretends correlated sources are independent; evidence must list sources used.

## Phase 9: User Story 7 - Optional Forecast RV

**Goal**: Optionally forecast future volatility from realized components without introducing opaque model risk.

- [ ] T071 RED: Add engine test `forecast_none_uses_measured_or_blended_rv_as_final`.
- [ ] T072 RED: Add engine test `ewma_forecast_is_deterministic_and_audited`.
- [ ] T073 RED: Add engine test `har_lite_blends_short_medium_long_horizons_with_toml_weights`.
- [ ] T074 RED: Add config validation tests for forecast method and weights.
- [ ] T075 GREEN: Implement `forecast_method = "none"`.
- [ ] T076 GREEN: Implement `forecast_method = "ewma"` with TOML-owned decay.
- [ ] T077 GREEN: Implement `forecast_method = "har_lite"` using TOML-owned horizon weights.
- [ ] T078 GREEN: Add evidence fields showing forecast method, inputs, weights, and final consumed RV component.
- [ ] T079 REFACTOR: Keep forecast code deterministic, explainable, and free of ML/model-serving dependencies.

## Phase 10: Evidence, Docs, and Compatibility

- [ ] T080 RED: Add decision-evidence round-trip test for the new runtime robust RV fields.
- [ ] T081 RED: Add stale-schema rejection test for the previous evidence schema version.
- [ ] T082 GREEN: Bump evidence schema version and update serializers/deserializers.
- [ ] T083 GREEN: Update evidence fixtures and docs for runtime/horizon/noise/jump/forecast fields.
- [ ] T084 GREEN: Update runtime literal audit for any new enum labels or schema fields.
- [ ] T085 GREEN: Update source-integrity golden digest after source changes.
- [ ] T086 GREEN: Update `specs/027-global-rv-surface-runtime/root-redesign.md` or equivalent design notes if implementation decisions change during TDD.

## Phase 11: External Review Gate

- [ ] T087 Run internal adversarial review against spec/plan/tasks before implementation starts.
- [ ] T088 Ask Claude relay for adversarial review of the full plan/tasks; skip only if it fails more than twice consecutively.
- [ ] T089 Ask Gemini relay for adversarial review of the full plan/tasks; skip only if it fails more than twice consecutively.
- [ ] T090 Ask Grok relay for adversarial review of the full plan/tasks; skip only if it fails more than twice consecutively.
- [ ] T091 Ask GLM relay for adversarial review of the full plan/tasks; skip only if it fails more than twice consecutively.
- [ ] T092 Resolve all blocking and substantive non-blocking findings by updating plan/tasks/specs before implementation.
- [ ] T093 Record unanimous approval or explicit skipped-after-two-failures evidence in the PR/issue comment.

## Phase 12: Final Verification and PR Closure

- [ ] T094 Push all implementation commits and wait for exact PR-head CI.
- [ ] T095 Confirm GitHub CI green for fmt, clippy, deny, source-fence, nextest shards, source integrity, CodeQL, actionlint, and gate.
- [ ] T096 Run relay review on the final pushed diff only after CI is green; instruct reviewers not to run local cargo tests if CI passed.
- [ ] T097 Address any remaining review findings with TDD commits.
- [ ] T098 Update issue #614 with final scope mapping: global runtime, multi-venue, and math robustness.
- [ ] T099 Prepare PR description that explicitly states no legacy strategy-owned RV path remains and names any accepted remaining scope.
- [ ] T100 Merge only after CI green, review findings resolved or waived, and no uncommitted/unpushed work remains.

## Dependency Notes

- T006-T011 must complete before implementation tasks so source fences fail first.
- T012-T022 must complete before multi-venue wiring because route ownership belongs to the global runtime.
- T023-T034 must complete before robust cross-source aggregation can be trusted.
- T035-T044 should complete before noise, jump, and forecast work because those build on horizon estimates.
- T045-T070 can proceed in parallel by module once the horizon model is stable.
- T071-T079 are optional only if the approved plan explicitly disables forecast mode; otherwise they are in scope.
- T087-T093 are mandatory before implementation per the owner's review gate.
