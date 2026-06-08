# Tasks: Global Multi-Venue Robust RV Runtime

**Input**: Design documents from `specs/027-global-rv-surface-runtime/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`
**Tests**: Required. Implementation uses TDD after plan approval.
**Scope**: PR #615 accepted slice: global runtime, multi-venue production wiring, Option A microstructure-noise robustness, jump diagnostics, robust aggregation, and evidence/schema updates for those fields. Multi-horizon and forecast tasks remain future scope and are not PR #615 merge gates.

## Execution Rule

T006-T013 are pre-implementation approval tasks and must complete before any RED/GREEN implementation task. Writing or updating this task list is not implementation. Adding tests in T014+ is implementation and must wait for plan approval.

## Phase 1: Setup and Guardrails

- [ ] T001 Confirm branch starts from current `main` after PR #609 merge and has no unrelated local changes.
- [ ] T002 Update `.specify/feature.json` to point at `specs/027-global-rv-surface-runtime`.
- [ ] T003 Update the Speckit pointer in `AGENTS.md` to `specs/027-global-rv-surface-runtime/plan.md`.
- [ ] T004 Keep all current artifacts in `specs/027-global-rv-surface-runtime/` aligned with contracts.
- [ ] T005 Prefer GitHub CI over broad local cargo tests for final verification.

## Phase 2: Pre-Implementation External Review Gate

- [ ] T006 Run internal adversarial review against spec/plan/tasks/contracts before implementation starts.
- [ ] T007 Ask Claude relay for adversarial review of the full plan/tasks/contracts; skip only if it fails more than twice consecutively.
- [ ] T008 Ask Gemini relay for adversarial review of the full plan/tasks/contracts; skip only if it fails more than twice consecutively.
- [ ] T009 Ask Grok relay for adversarial review of the full plan/tasks/contracts; skip only if it fails more than twice consecutively.
- [ ] T010 Ask GLM relay for adversarial review of the full plan/tasks/contracts; skip only if it fails more than twice consecutively or source-send approval is denied.
- [ ] T011 Resolve all blocking and substantive non-blocking findings by updating plan/tasks/specs/contracts before implementation.
- [ ] T012 Re-run focused re-review on changed artifacts until Claude, Gemini, Grok, and GLM approve or are explicitly skipped under the failure rule.
- [ ] T013 Record approval evidence and any skipped reviewer rationale in the PR/issue comment.

## Phase 3: TDD Foundations

- [ ] T014 [P] RED: Add a source-fence test proving `src/strategies/**` contains no `RealizedVolEngine`, `realized_vol_engine`, or RV runtime construction.
- [ ] T015 [P] RED: Add a source-fence test proving RV subscription creation is not strategy-owned.
- [ ] T016 [P] RED: Add a source-fence test proving pricing/strategy consumers do not apply raw RV predicates such as `is_positive_finite` or `is_non_negative_finite` to snapshot RV fields.
- [ ] T017 [P] RED: Add a config validation test that every strategy needing RV requires `realized_volatility_surface_id` and rejects all legacy `vol_*` fields as unknown.
- [ ] T018 [P] RED: Add an evidence schema test requiring runtime-level robust RV fields and v9 schema bump.
- [ ] T019 [P] RED: Add a source-fence or type-visibility test that prevents strategy modules from importing or constructing the RV runtime/engine types directly.
- [ ] T020 GREEN: Add only enough scaffolding to compile the new test module names and failing assertions without implementing behavior.

## Phase 4: Global Runtime Outside Taker

**Goal**: A process-level runtime owns all RV surfaces, and taker/maker/future consumers only consume snapshots.

- [ ] T021 RED: Add `tests/bolt_v3_realized_volatility_runtime.rs` test `runtime_builds_all_surfaces_from_root_config`.
- [ ] T022 RED: Add runtime test `runtime_publishes_snapshot_by_surface_id_for_multiple_real_consumers`.
- [ ] T023 RED: Add runtime test `runtime_fails_loudly_when_strategy_references_missing_surface`.
- [ ] T024 RED: Add runtime test `strategy_does_not_need_market_signal_feed_to_warm_rv_surface`.
- [ ] T025 RED: Add runtime tests `runtime_refresh_ignores_or_rejects_non_monotonic_as_of_ms`, `runtime_refresh_ignores_or_rejects_equal_as_of_ms`, and `first_refresh_at_initial_timestamp_has_deterministic_behavior`. Future forecast work must add no-advance forecast assertions.
- [ ] T026 RED: Add runtime test `snapshot_reads_cannot_mutate_runtime_state`.
- [ ] T027 GREEN: Implement `src/bolt_v3_realized_volatility_runtime.rs` with root-config construction, surface map, sorted surface refresh order, serialized refresh, and snapshot lookup.
- [ ] T028 GREEN: Move RV surface construction from `src/strategies/binary_oracle_edge_taker/mod.rs` into the global runtime build path.
- [ ] T029 GREEN: Remove any `RealizedVolEngine` fields, constructor calls, and refresh ownership from binary oracle taker strategy state.
- [ ] T030 GREEN: Add a runtime snapshot provider/accessor that `src/bolt_v3_taker_pricing.rs` can consume by `surface_id`.
- [ ] T031 GREEN: Wire binary oracle taker to consume runtime snapshots only.
- [ ] T032 GREEN: Wire a real non-taker runtime consumer, starting with evidence/monitoring snapshot export; binary oracle maker may additionally consume the same runtime snapshot API if it exists on main. Add `maker_consumes_runtime_rv_snapshot_when_present` if maker integration exists. Synthetic two-consumer tests alone are insufficient.
- [ ] T033 REFACTOR: Keep strategy code intent-only; any subscription, route, quorum, readiness, or aggregation code belongs outside strategies.

## Phase 5: Multi-Venue Available Sources

**Goal**: Production RV surfaces use every configured available public source instead of one hardcoded/single venue.

- [ ] T034 RED: Add root-config validation tests `surface_source_references_existing_public_client_and_instrument` and `surface_source_instrument_asset_must_match_surface_canonical_base_asset`.
- [ ] T035 RED: Add config tests `production_surfaces_use_all_available_public_sources` and `production_surface_source_bindings_are_root_toml_owned`.
- [ ] T036 RED: Add config/runtime tests `unsupported_mark_source_class_sample_kind_is_rejected_until_runtime_routing_exists` and `runtime_construction_rejects_mark_source_if_validation_is_bypassed`.
- [ ] T037 RED: Add runtime tests `deduplicates_physical_subscriptions_and_fans_out_to_multiple_sources` and `subscription_key_order_is_deterministic_for_equivalent_routes`.
- [ ] T038 RED: Add runtime tests `two_available_venue_sources_contribute_to_one_surface_snapshot` and `multi_venue_partial_source_down_remains_auditable_while_quorum_policy_decides_readiness`.
- [ ] T039 RED: Add runtime test `disabled_source_is_audited_but_not_subscribed`.
- [ ] T040 RED: Add runtime test `enabled_non_quorum_source_with_live_observations_remains_diagnostic_only`.
- [ ] T041 GREEN: Implement `RealizedVolSourceRoute` and `PhysicalSubscriptionKey` in the runtime module.
- [ ] T042 GREEN: Move RV subscription request generation out of strategy subscription methods.
- [ ] T043 GREEN: Implement observation fan-out from one physical stream to every configured source route.
- [ ] T044 GREEN: Extend root validation to reject source client/instrument drift before runtime starts.
- [ ] T045 GREEN: Update `config/root.toml` surfaces to include all available configured venue/source mappings per asset.
- [ ] T046 GREEN: Keep strategy TOMLs limited to `realized_volatility_surface_id` selectors.
- [ ] T047 REFACTOR: Ensure no asset, token, venue, provider, timeout, or subscription policy is hardcoded in Rust.

## Phase 6: Future Multi-Horizon RV

**PR #615 status**: Deferred. These tasks are retained as future horizon/regime robustness work and are not current merge gates.

**Goal**: Replace single-window brittleness with auditable short/medium/long horizon estimates.

- [ ] T048 RED: Add engine test `multi_horizon_source_requires_required_horizons_only`.
- [ ] T049 RED: Add engine test `optional_long_horizon_missing_does_not_block_surface_when_policy_allows`.
- [ ] T050 RED: Add engine test `weighted_blend_normalizes_ready_required_horizon_weights`.
- [ ] T051 RED: Add engine test `max_floor_uses_primary_and_floor_horizon_role_bindings`.
- [ ] T052 RED: Add engine test `short_with_long_floor_uses_floor_multiplier_and_named_horizons`.
- [ ] T053 RED: Add engine test `coverage_ratio_uses_valid_return_count_over_expected_return_count`.
- [ ] T054 RED: Add config validation tests for horizon uniqueness, positive windows, window >= interval, coverage bounds, positive total required weight, role bindings, and `floor_multiplier` bounds.
- [ ] T055 GREEN: Add horizon config structs to `src/bolt_v3_config.rs` and validation in `src/bolt_v3_validate.rs`.
- [ ] T056 GREEN: Extend `src/bolt_v3_realized_volatility.rs` to compute per-horizon fixed-grid RV per source.
- [ ] T057 GREEN: Implement final horizon policies `weighted_blend`, `max_floor`, and `short_with_long_floor`.
- [ ] T058 GREEN: Publish per-horizon estimates in `RealizedVolSnapshot` and diagnostics.
- [ ] T059 REFACTOR: Keep `ReadyRealizedVol` as the only consumer-facing numeric contract.

## Phase 7: Microstructure-Noise Robustness

**Goal**: Reduce false high-frequency volatility from quote midpoint bounce without hiding the base estimate.

- [ ] T060 RED: Add engine test `subsampled_rv_reduces_alternating_bid_ask_bounce_vs_base_grid`.
- [ ] T061 RED: Add engine test `subsampled_rv_requires_min_ready_subsamples`.
- [ ] T062 RED: Add engine tests `subsampled_offset_grid_coverage_uses_offset_grid_denominator`, `subsampled_offsets_are_distinct_for_valid_collision_free_config`, and `subsampled_evidence_records_actual_offsets_used`.
- [ ] T063 RED: Add config test `subsamples_greater_than_sampling_interval_is_rejected_or_collision_semantics_are_explicit`.
- [ ] T064 RED: Add engine tests `coarser_grid_rv_policy_selects_base_coarse_or_min_base_coarse`, `min_base_coarse_returns_base_when_base_is_lower`, and `min_base_coarse_returns_coarse_when_coarse_is_lower`.
- [ ] T065 RED: Add config validation tests for `noise_robust_method`, `subsamples`, `min_ready_subsamples`, `coarse_sampling_interval_ms`, and `coarser_grid_policy`.
- [ ] T066 GREEN: Implement `noise_robust_method = "none"` as current fixed-grid behavior.
- [ ] T067 GREEN: Implement `noise_robust_method = "coarser_grid"` per horizon.
- [ ] T068 GREEN: Implement `noise_robust_method = "subsampled"` per horizon with deterministic offset grids.
- [ ] T069 GREEN: Emit base fixed-grid RV and noise-robust RV separately in diagnostics/evidence.
- [ ] T070 REFACTOR: Keep all noise-robust parameters TOML-owned and engine-owned.

## Phase 8: Jump Separation

**Goal**: Separate jump component from continuous RV instead of deleting jumps.

- [ ] T071 RED: Add engine test `single_large_jump_increases_jump_component_without_erasing_measured_rv`.
- [ ] T072 RED: Add engine test `flat_source_publishes_zero_continuous_and_zero_jump_rv`.
- [ ] T073 RED: Add engine test `jump_separation_with_fewer_than_two_returns_is_diagnostic_only`.
- [ ] T074 RED: Add engine tests `measured_variance_equals_continuous_variance_plus_jump_variance_before_sqrt` and `bipower_variance_above_measured_variance_produces_zero_jump_component`.
- [ ] T075 RED: Add evidence test `jump_component_is_serialized_separately_from_final_rv`.
- [ ] T076 RED: Add config validation tests for jump policy and threshold bounds.
- [ ] T077 GREEN: Implement `jump_policy = "none"` as measured RV passthrough.
- [ ] T078 GREEN: Implement `jump_policy = "separate"` using finite-sample-corrected bipower variation.
- [ ] T079 GREEN: Emit measured, continuous, and jump annualized RV components per horizon/source/surface.
- [ ] T080 REFACTOR: Do not suppress real jumps silently; pricing selection must be explicit in config/evidence.

## Phase 9: Robust Cross-Source Aggregation

**Goal**: Make multi-venue RV useful by protecting against one bad feed while preserving fail-closed dispersion behavior.

- [ ] T081 RED: Add engine test `median_aggregation_ignores_one_extreme_ready_source_when_quorum_satisfied`.
- [ ] T082 RED: Add engine tests `trimmed_mean_requires_enough_ready_sources_for_trim_policy` and `trimmed_mean_rejects_when_trim_removes_all_contributors`.
- [ ] T083 FUTURE RED: Add engine tests `mad_dispersion_blocks_when_ready_sources_disagree_too_much` and `all_zero_contributors_have_zero_dispersion_and_zero_mad` if MAD-specific blocking is added.
- [ ] T084 FUTURE RED: Add engine test `raw_mad_threshold_is_used_without_normal_scaling` if MAD-specific blocking is added.
- [ ] T085 RED: Add engine test `upper_quantile_guard_uses_guard_weight_and_upper_quantile_value`.
- [ ] T086 RED: Add engine test `source_level_not_warm_does_not_block_satisfied_partial_quorum` to preserve PR #609 fix.
- [ ] T087 RED: Add boundary tests for exactly `min_ready_sources`, one below `min_ready_sources`, zero eligible contributors blocking before aggregation, equal contributors across every aggregation method, and every aggregation method.
- [ ] T088 GREEN: Extend aggregation config with `median`, `trimmed_mean`, `median_with_upper_quantile_guard`, `guard_weight`, and `trim_fraction`.
- [ ] T089 FUTURE GREEN: Implement MAD dispersion diagnostics and blocker only if the future MAD-specific policy is accepted.
- [ ] T090 GREEN: Keep source-level blockers out of surface blockers when quorum is satisfied.
- [ ] T091 GREEN: Preserve unknown-source, disabled-source, and non-quorum diagnostics.
- [ ] T092 REFACTOR: Ensure aggregation never pretends correlated sources are independent; evidence must list sources used.

## Phase 10: Future Optional Forecast RV

**PR #615 status**: Deferred. Forecast pricing is rejected by validation in the current slice.

**Goal**: Optionally forecast future volatility from realized components without introducing opaque model risk.

- [ ] T093 RED: Add engine test `forecast_none_uses_measured_or_blended_rv_as_final`.
- [ ] T094 RED: Add engine tests `ewma_forecast_advances_only_on_refresh_with_new_ready_component` and `ewma_forecast_advances_on_refresh_even_when_component_value_is_unchanged`.
- [ ] T095 RED: Add engine tests `ewma_forecast_does_not_advance_on_observation_without_refresh` and `ewma_forecast_does_not_advance_on_non_monotonic_now_ms`.
- [ ] T096 RED: Add engine tests `ewma_forecast_cold_starts_from_current_component_after_restart` and `ewma_cold_start_sets_previous_update_timestamp_for_next_refresh`.
- [ ] T097 RED: Add engine tests `forecast_config_change_changes_fingerprint`, `forecast_state_resets_when_config_fingerprint_changes`, and `forecast_warm_starts_on_next_refresh_after_fingerprint_reset`.
- [ ] T098 RED: Add engine test `forecast_state_is_independent_per_surface_id`.
- [ ] T099 RED: Add engine tests `har_lite_blends_short_medium_long_horizons_with_toml_weights` and `har_lite_all_zero_horizons_publish_valid_zero_forecast`.
- [ ] T100 RED: Add engine test `har_lite_blocks_when_any_role_horizon_is_not_ready`.
- [ ] T101 RED: Add engine tests `final_ready_realized_vol_equals_configured_pricing_component` and `forecast_component_invalid_blocks_when_selected_as_pricing_component`.
- [ ] T102 RED: Add config validation tests for forecast method, `ewma_alpha_zero_or_above_one_rejected`, HAR weights, referenced horizon roles, non-negative HAR weights, HAR weight sum in open-zero-to-one, `har_intercept_negative_or_non_finite_rejected`, and `pricing_component_noise_robust_requires_noise_method_not_none`.
- [ ] T103 GREEN: Implement `forecast_method = "none"`.
- [ ] T104 GREEN: Implement `forecast_method = "ewma"` with per-surface serialized refresh state and TOML-owned alpha.
- [ ] T105 GREEN: Implement `forecast_method = "har_lite"` using TOML-owned horizon weights.
- [ ] T106 GREEN: Add evidence fields showing forecast method, inputs, weights, cold-start flag, previous forecast, and final consumed RV component.
- [ ] T107 REFACTOR: Keep forecast code deterministic, explainable, and free of ML/model-serving dependencies.

## Phase 11: Evidence, Diagnostics, and Compatibility

- [ ] T108 RED: Add decision-evidence round-trip test for the new runtime robust RV fields.
- [ ] T109 RED: Add stale-schema rejection test by reading the current evidence version on `main`, then asserting the feature branch rejects the previous version and bumps by exactly one.
- [ ] T110 FUTURE RED: Add runtime tests `unknown_source_diagnostics_are_bounded_and_evictions_are_reported`, `unknown_source_diagnostics_remain_bounded_under_sustained_churn`, and `unknown_source_eviction_policy_is_deterministic_and_documented` before any raw external source-ID ingestion path is added.
- [ ] T111 FUTURE RED: Add combined-mode determinism test for `subsampled + jump_separate + ewma` when forecast mode ships.
- [ ] T112 RED: Add surface ID hygiene tests for empty, whitespace, duplicate, trim-equivalent duplicate, and case-sensitive IDs.
- [ ] T113 GREEN: Bump evidence schema from current `main` by exactly one version.
- [ ] T114 GREEN: Update serializers/deserializers for runtime/noise/jump fields; horizon/forecast serializers are future scope.
- [ ] T115 FUTURE GREEN: Implement bounded unknown-source diagnostic capacity and deterministic eviction reporting before raw external source-ID ingestion exists.
- [ ] T116 GREEN: Update evidence fixtures and docs for runtime/noise/jump fields; horizon/forecast fixtures are future scope.
- [ ] T117 GREEN: Update runtime literal audit for any new enum labels or schema fields.
- [ ] T118 GREEN: Update source-integrity golden digest after source changes.

## Phase 12: Final Verification and PR Closure

- [ ] T119 Push all implementation commits and wait for exact PR-head CI.
- [ ] T120 Confirm GitHub CI green for fmt, clippy, deny, source-fence, nextest shards, source integrity, CodeQL, actionlint, and gate.
- [ ] T121 Run relay review on the final pushed diff only after CI is green; instruct reviewers not to run local cargo tests if CI passed.
- [ ] T122 Address any remaining review findings with TDD commits.
- [ ] T123 Update issue #614 with final scope mapping: global runtime, multi-venue, and math robustness.
- [ ] T124 Prepare PR description that explicitly states no strategy-owned RV path remains and names any accepted remaining scope.
- [ ] T125 Merge only after CI green, review findings resolved or waived, and no uncommitted/unpushed work remains.

## Dependency Notes

- T006-T013 must complete before T014 or any implementation task.
- T014-T020 must complete before runtime implementation so source fences fail first.
- T021-T033 must complete before multi-venue wiring because route ownership belongs to the global runtime.
- T034-T047 must complete before robust cross-source aggregation can be trusted.
- T048-T059 are future multi-horizon work and are not prerequisites for PR #615 Option A fixed-grid noise robustness.
- T060-T092 can proceed after the current fixed-grid engine contracts are stable.
- T093-T107 are future forecast work; PR #615 explicitly disables forecast pricing.
- T119-T125 require exact PR-head CI evidence, not inference from local tests.
