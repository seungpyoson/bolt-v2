# Internal Adversarial Review

**Feature**: `specs/026-nt-backed-iv-engine/`
**Review date**: 2026-06-08
**Reviewed working tree**: branch `026-nt-backed-iv-engine`, final committed implementation tree
**Recommendation**: PASS for the internally reviewed local scope

## Findings

No blocking findings remain after the local fixes below.

## Issues Found And Fixed

| Issue | Evidence | Resolution |
|---|---|---|
| Converter-level ingest rejections could bypass source-health recording. Malformed aggregate/custom custom data returned `IngestRejected` before an `IvIngestEvent` existed, and live msgbus handlers only logged the failure. | RED: `cargo test --locked --test bolt_v3_iv_live_integration live_iv_event_bindings_record_malformed_` failed with `ProductNotFound` for both malformed aggregate and custom-IV source-health queries. | Added runtime rejection recording for pre-ingest converter failures using configured source generation and incoming NT event timestamp. Added live msgbus tests proving malformed aggregate/custom custom data updates `InvalidIvValue` reject reason/counts and source-health timestamps. |
| Custom-data backed aggregate/custom-IV raw payloads discarded adapter-specific custom-data fields after extracting configured typed values. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads` failed because aggregate/custom raw payload variants had no preserved custom-data JSON field. | Added optional `nt_custom_data_json` to aggregate greeks and custom-IV raw payloads, populated it from NT `CustomDataTrait::to_json()`, and kept strategy products typed/provenance-only. |
| Configured `accepted_conventions` was only parsed, not enforced for NT greeks. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention` initially compiled only after the retention test helper was added, then failed behaviorally because Black-Scholes greeks were accepted when the source allowed only price-adjusted conventions. | Runtime source state now carries `accepted_conventions`, rejects unsupported NT option-greeks and nested option-chain greeks conventions, records `UnsupportedConvention` in source health, and happy-path NT fixtures use the canonical `BLACK_SCHOLES` convention name. |
| Option-greeks events with no mark/bid/ask IV basis were accepted as successful no-op indexing. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis` failed before the fix because an event with no IV basis returned `Ok`. | Store indexing now returns `MissingIvBasis` after raw preservation, runtime records typed source health, and retained raw evidence still flows through retention enforcement. |
| Failed runtime indexing could preserve raw payloads without applying profile retention. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest` first failed to compile because no raw-event count inspection existed for runtime state. | Added `IvQueryStateHandle::raw_event_count()` for evidence and enforced retention after both successful and failed runtime store ingestion. |
| Raw audit age checks used saturating subtraction, allowing `as_of_ns` before event receipt to bypass age limits. | RED: `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt` returned `Ok` before the fix. | Raw audit access now rejects requests whose `as_of_ns` precedes `received_ts_ns` with `RetentionMiss`. |
| Duplicate effective NT topics were checked only for option greeks and option chains. | RED: `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding` loaded duplicate aggregate/custom-IV custom-data topics successfully before the fix. | Config validation now rejects duplicate aggregate greeks `aggregate_key` topics and duplicate custom-IV `custom_iv_data_type` topics within a profile. |
| Reload planning produced new generations, but runtime source state could not apply a reloaded IV root. | RED: `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations` failed to compile because `IvRuntimeEngine` had no reload API. | Added `IvRuntimeEngine::apply_iv_root_reload()` to refresh source configs, policies, retention, derived inputs, and current subscription generations while preserving existing shared query state handles. |
| Existing cloned strategy handles could continue serving old products after a source generation reload or profile removal. | Adversarial review of `IvQueryHandle` freshness checks showed source-health state could override the new generation map, and removed profiles were dropped from the engine map without mutating already-cloned shared state. | Reload now marks removed sources/profiles as `Removed` in the shared query state before dropping engine references, and query freshness checks require the current generation map to match before source health can satisfy a product query. |
| Speckit data model documented the old aggregate selector shape. | `quickstart.md` and production validation required `delta_field`, `gamma_field`, `vega_field`, `theta_field`, and `rho_field`; `data-model.md` omitted them. | Updated `data-model.md` so `SourceAggregateGreeksSelector` includes the five configured greek field mappings. |

## Prior Findings Rechecked

- Custom-data live routing for aggregate greeks and custom IV evidence is present in `src/bolt_v3_live_node.rs`, retained in `BoltV3IvRuntimeEventBindings`, and unsubscribed in `Drop`.
- Happy-path live routing is covered by `tests/bolt_v3_iv_live_integration.rs`.
- Duplicate option-greeks NT topic validation is present in `src/bolt_v3_iv/config.rs` and covered by `tests/bolt_v3_iv_config.rs`.

## Verification

- `cargo test --locked --test bolt_v3_iv_live_integration live_iv_event_bindings_record_malformed_`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_config --test bolt_v3_iv_store`: PASS, 49 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_ingest`: PASS, 21 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration`: PASS, 17 tests.
- `cargo test --locked --test bolt_v3_iv_subscription`: PASS, 15 tests.
- `cargo fmt --check`: PASS.
- `just source-fence`: PASS.
- `cargo clippy --locked --lib -- -D warnings`: PASS.
- `cargo clippy --locked --bin bolt-v2 -- -D warnings`: PASS.
- Focused IV target bundle: PASS.
- `cargo test --locked bolt_v3_iv`: PASS.
- `cargo test --locked`: PASS.

Current PR verification uses GitHub CI rather than local cargo reruns. Because any evidence-file commit changes the head SHA, final head status must be confirmed with `gh pr view 611 --json headRefOid` and `gh pr checks 611` after the final push.

## 2026-06-10 Delta Review

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `8b0a0710c7e5f7b07b73f402576d35aea0edf27b`
**Recommendation**: PASS for the locally reviewed delta after the fixes below; final PR approval still requires commit, push, and green GitHub CI on the new head.

| Issue | Evidence | Resolution |
|---|---|---|
| Aggregate projection was specified for aggregate products, but aggregate greeks carried no scalar IV value. Query projection rejected aggregate products unconditionally. | RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_aggregate_greeks_query_projects_configured_aggregate_iv -- --nocapture` initially failed because aggregate greeks had no configured aggregate-IV payload and projection support. | Added explicit optional aggregate IV mapping (`iv_field`, `iv_basis`, `iv_convention`) for aggregate custom-data sources, stores configured aggregate IV with provenance, rejects missing aggregate IV for scalar projection, and covers single-source plus quorum aggregate projection. |
| Source-health queries with `source_filter` could return the first retained historical row instead of the current source generation. | RED: `cargo test --locked --test bolt_v3_iv_query source_health_query_prefers_current_generation_for_source_filter -- --nocapture` returned generation 1 stale health before the fix. | Source-filtered health queries now use generation-aware selection before applying the state filter. |
| Smile/surface scalar projection recorded NT symbol text as convention, which violates the projection contract that basis and convention cannot silently change. | RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_smile_query_records_option_convention_not_nt_symbol -- --nocapture` failed with `ConfiguredNtSymbol` instead of `configured-convention`. | Indexed smiles now carry the nested NT greeks convention, option-chain smile construction groups by `(basis, convention)`, and smile/surface projection inputs use the typed convention. |
| Source-health product fingerprint helper returned `source_id` as if it were a selector fingerprint. | Internal review of `IvQueryProduct::selector_fingerprint()` showed the value is ignored for `SourceHealth` authorization but semantically wrong for audit/logging reuse. | `SourceHealth` now returns an empty fingerprint from that helper; source-health selector-scoped authorization remains source-id based. |

### 2026-06-10 Verification

- `cargo test --locked --test bolt_v3_iv_query projected_scalar_aggregate_greeks_query_projects_configured_aggregate_iv -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query source_health_query_prefers_current_generation_for_source_filter -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_smile_query_records_option_convention_not_nt_symbol -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query -- --nocapture`: PASS, 39 tests.
- `cargo test --locked --test bolt_v3_iv_ingest -- --nocapture`: PASS, 7 tests.
- `cargo test --locked --test bolt_v3_iv_store -- --nocapture`: PASS, 7 tests.
- `cargo test --locked --test bolt_v3_iv_subscription -- --nocapture`: PASS, 19 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration -- --nocapture`: PASS, 20 tests.
- `cargo test --locked --test bolt_v3_iv_config -- --nocapture`: PASS, 23 tests.
- `cargo fmt --check`: PASS.
- `just source-fence`: PASS.
- `git diff --check`: PASS.

## 2026-06-10 Relay Follow-Up Review

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `4cd28a966c62fd009894e88367f83f5ac29c35d3`
**Recommendation**: PASS for the locally reviewed fixes below; external relay approvals and GitHub CI must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| Option-greeks and aggregate-greeks products could cache non-finite greek values while IV itself was valid. | RED: `cargo test --locked --test bolt_v3_iv_store non_finite -- --nocapture` failed because `NaN`/`inf` greek payloads returned `Ok` and indexed products. | Added `IvGreekValues::has_non_finite_value()`, fail-closed store validation for option greeks and aggregate greeks, and a derived-helper guard before returning helper greeks. Raw events remain preserved before index rollback. |
| Live-node stop removed IV event bindings but left `self.iv_runtime` reachable, so `has_iv_runtime()` stayed true after stop. | RED: `cargo test --locked --lib live_node_runtime_stop_applies_iv_unsubscribe_lifecycle -- --nocapture` failed on the new stop-state assertion. | `stop_iv_engine_lifecycle()` now takes the runtime, applies unsubscribe outcomes to the shared state, restores it only if outcome application fails, and otherwise leaves the live node without IV runtime or event bindings. |
| API contract wording could be read as promising dynamic creation of brand-new strategy profile handles during reload. | Internal review of `contracts/iv-engine-api.md` found the reload sentence did not distinguish existing handles from new profile registration. | Narrowed the contract to already-issued strategy handles for profiles present before reload. Existing handles still share reloaded `IvQueryStateHandle` state. |

### 2026-06-10 Relay Follow-Up Verification

- `cargo test --locked --test bolt_v3_iv_store non_finite -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --lib live_node_runtime_stop_applies_iv_unsubscribe_lifecycle -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_store -- --nocapture`: PASS, 9 tests.
- `cargo test --locked --test bolt_v3_iv_derive -- --nocapture`: PASS, 16 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration -- --nocapture`: PASS, 20 tests.
- `cargo test --locked --test bolt_v3_iv_ingest -- --nocapture`: PASS, 7 tests.
- `cargo fmt --check`: PASS.
- `just source-fence`: PASS.
- `git diff --check`: PASS.

## 2026-06-10 Internal Adversarial Review After `e862650d`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `e862650d5038b26f6fccae128c57309d740b6aed`
**Recommendation**: PASS for the locally reviewed fixes below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| Option-greeks products could cache non-finite strategy-visible metadata even when IV and greeks were finite. | RED: `cargo test --locked --test bolt_v3_iv_store non_finite -- --nocapture` failed because `underlying_price = NaN` and `open_interest = inf` returned `Ok` and indexed products. | Store validation now rejects non-finite optional option-greeks metadata before indexing while preserving the raw event for audit/replay. |
| Option-chain smiles could expose a non-finite `atm_strike` even though per-strike rows were filtered. | RED: `cargo test --locked --test bolt_v3_iv_ingest option_chain_with_non_finite_atm_strike_preserves_raw_event_without_indexing_smiles -- --nocapture` failed because the slice returned `Ok` and indexed a smile with `atm_strike = NaN`. | Store validation now rejects non-finite chain `atm_strike` before smile construction while preserving the raw event. |

### 2026-06-10 Post-`e862650d` Verification

- `cargo test --locked --test bolt_v3_iv_store non_finite -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_ingest option_chain_with_non_finite_atm_strike_preserves_raw_event_without_indexing_smiles -- --nocapture`: RED before fix, GREEN after fix.

## 2026-06-10 Internal Adversarial Review After `2ce39b4f`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `2ce39b4f5ae308df730c4986e47bbbbfc019cfe0`
**Recommendation**: PASS for the locally reviewed fixes below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| `IvProjectionPolicy` selector fields were parsed as strings and only checked for non-empty values, so unknown policy values could load and then be ignored. | RED: `cargo test --locked --test bolt_v3_iv_config unknown_projection_policy_values_reject_at_parse` failed because `configured-unknown-strike-selection` loaded successfully. | Converted projection selector fields to typed serde enums (`IvBasisSelection`, `IvStrikeSelection`, `IvTenorSelection`, `IvEvidenceMapping`) so unknown TOML values reject at parse. |
| Smile projection with `strike_selection = "all_configured_strikes"` still applied the interpolation policy to ATM/first strike, silently overriding TOML-owned projection semantics. | RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_all_configured_strikes_uses_all_smile_points_when_interpolation_policy_exists` failed with the ATM interpolation value instead of the all-strikes mean. | Projection now skips interpolation for `AllConfiguredStrikes`; interpolation only runs for single-strike selections such as `AtmStrike` and falls back to the configured projection/fallback path when interpolation yields no scalar input. |
| Single-strike smile projection could silently degrade back to all-strikes projection when the configured interpolation policy produced no eligible input. | RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_query_rejects_when_single_strike_interpolation_has_no_eligible_source` failed because the query returned `Ok(ProjectedScalarIv { value: 0.42, ... })` instead of `ProjectionRejected`. | Interpolation now reports `NotApplicable`, `Interpolated`, or `Rejected`. A rejected single-strike interpolation enters configured fallback if present, otherwise fails closed instead of normal projection over all strikes. |
| Smile quorum could not operate across multiple sources because projection discovery fell back to first-match smile lookup and quorum ran over flattened smile points before per-source interpolation. | RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_smile_quorum_interpolates_each_source_before_quorum` failed with `ProjectionRejected`. | Smile/surface projection discovery now collects all matching source products; configured interpolation runs per source before quorum, and quorum gates the projected scalar over the synchronized scalar inputs. |
| Selector-scoped smile queries could reject an authorized source when an unauthorized matching source was ingested first. | RED: `cargo test --locked --test bolt_v3_iv_query selector_scoped_smile_query_skips_unauthorized_matching_source` failed with `StrategyNotAuthorized`. | Direct smile/surface queries now retry the matching product set and return an authorized current product when one exists. |
| Ordinary product queries cloned the full `IvQueryState`, including retained raw events, on every query. | RED: `cargo test --locked --lib only_derived_queries_require_snapshot_for_query_time_writes` failed to compile before the query classification helper existed. | Non-derived queries now use the shared read guard. Snapshot cloning is limited to derived query paths that can write derived-output cache or source-health rejection state during query evaluation. |

### 2026-06-10 Post-`2ce39b4f` Verification

- `cargo test --locked --test bolt_v3_iv_query projected_scalar_all_configured_strikes_uses_all_smile_points_when_interpolation_policy_exists`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_query_rejects_when_single_strike_interpolation_has_no_eligible_source`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query selector_scoped_smile_query_skips_unauthorized_matching_source`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_smile_quorum_interpolates_each_source_before_quorum`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config unknown_projection_policy_values_reject_at_parse`: RED before fix, GREEN after fix.
- `cargo test --locked --lib only_derived_queries_require_snapshot_for_query_time_writes`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query`: PASS, 43 tests.
- `cargo test --locked --test bolt_v3_iv_config`: PASS, 24 tests.
- `cargo test --locked --test bolt_v3_iv_policy`: PASS, 5 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration`: PASS, 20 tests.
- `cargo fmt --check`: PASS.
- `just source-fence`: PASS.
- `cargo clippy --locked --lib -- -D warnings`: NOT COMPLETED locally; this worktree's build script panicked before linting while canonicalizing a missing `.git/worktrees/026-nt-backed-iv-engine-fix/refs/heads/026-nt-backed-iv-engine` path. GitHub CI clippy is the required authoritative check after push.

## 2026-06-10 Relay Shard Findings After `ca19dd4a`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `ca19dd4a5b1c3560f4bf33379fe202acec798740`
**Recommendation**: PASS for the locally reviewed fixes below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| Quorum rejection silently degraded through fallback policy instead of failing closed. | Follow-up adversarial review rejected fallback after quorum failure; RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_query_rejects_when_configured_quorum_rejects_even_with_fallback -- --nocapture` returned a projected fallback value. | Quorum rejection now returns `ProjectionRejected` even when the projection policy also names a fallback policy. |
| Selector-scoped point queries could reject an authorized later source when an unauthorized matching source was inserted first. | Gemini IV-module shard returned `REQUEST_CHANGES`; RED: `cargo test --locked --test bolt_v3_iv_query selector_scoped_point_query_skips_unauthorized_matching_source -- --nocapture` failed with `StrategyNotAuthorized`. | Authorized-current fallback selection now uses the matching product set for point, greeks, aggregate, custom evidence, smile, surface, and derived products instead of only smile/surface. |
| Stale historical indexed products could poison direct and projected queries for the same key before current-generation products were considered. | Grok query/policy shard returned `REQUEST_CHANGES`; RED: `cargo test --locked --test bolt_v3_iv_query strategy_query_skips_non_current_matching_product_for_current_generation -- --nocapture` and `cargo test --locked --test bolt_v3_iv_query projected_scalar_point_query_skips_non_current_matching_product_for_current_generation -- --nocapture` failed with `ProductNotFound`. | Direct queries now search for an authorized current alternative when the first match is non-current; projected scalar queries filter non-current input products before authorization, quorum, projection, or fallback policy evaluation. |
| Helper policy config accepted `allowed_outputs = ["iv"]`, but runtime derivation requires `iv_and_greeks` and would reject every query later. | Gemini IV-module shard noted the mismatch; RED: `cargo test --locked --test bolt_v3_iv_config helper_policy_allowed_outputs_must_include_engine_helper_output -- --nocapture` failed because validation accepted the config. | Config validation now rejects helper policies whose `allowed_outputs` do not include `iv_and_greeks`. |
| Projected scalar fallback outputs cloned provenance from the first input product rather than the accepted fallback candidate. | Internal adversarial review found the audit mismatch; RED: `cargo test --locked --test bolt_v3_iv_query projected_scalar_fallback_provenance_uses_accepted_candidate_source -- --nocapture` failed because the projected output reported `configured-source` while fallback selected `configured-backup-source`. | Query projection now carries the fallback-selected input and builds projected output provenance from that accepted candidate, falling back to first-input provenance only for non-selecting policy outputs. |

### 2026-06-10 Relay Shard Fix Verification

- `cargo test --locked --test bolt_v3_iv_query selector_scoped_point_query_skips_unauthorized_matching_source -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_query_rejects_when_configured_quorum_rejects_even_with_fallback -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query strategy_query_skips_non_current_matching_product_for_current_generation -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_point_query_skips_non_current_matching_product_for_current_generation -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config helper_policy_allowed_outputs_must_include_engine_helper_output -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query projected_scalar_fallback_provenance_uses_accepted_candidate_source -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_query`: PASS, 48 tests.
- `cargo test --locked --test bolt_v3_iv_config`: PASS, 25 tests.

## 2026-06-10 Internal Adversarial Review After `fb84d603`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `fb84d6038a884e0855b8e93c89efc0fbd6fb9c5c`
**Recommendation**: PASS for the locally reviewed fixes below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| Interpolation, fallback, and quorum policies could reference source IDs that were not configured in the owning IV profile. Projection `source_eligibility` was validated, but these policy-local `eligible_sources` fields were not. | RED: `cargo test --locked --test bolt_v3_iv_config policy_source_references_must_point_to_configured_sources -- --nocapture` failed because the invalid config produced no validation error. | Config validation now rejects unknown source IDs in interpolation, fallback, and quorum `eligible_sources`, and the valid config fixture no longer names an unconfigured quorum source. |
| A derived-input `profile_source_ref` could name a source/selector pair that does not exist in the owning profile. | RED: `cargo test --locked --test bolt_v3_iv_config derived_input_profile_source_ref_must_point_to_configured_source_selector_pair -- --nocapture` failed because validation accepted a missing selector. | Config validation now requires `profile_source_ref` to match a configured source ID and selector fingerprint pair. |
| Duplicate derived-input `field_sources` for the same helper field could load, and query-time resolution would silently use the first match. | RED: `cargo test --locked --test bolt_v3_iv_config derived_input_policy_field_sources_must_not_duplicate_fields -- --nocapture` failed because validation accepted duplicate `option_price` field policies. | Config validation now rejects duplicate `field_sources` entries per derived-input policy. |

### 2026-06-10 Post-`fb84d603` Verification

- `cargo test --locked --test bolt_v3_iv_config policy_source_references_must_point_to_configured_sources -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config derived_input_profile_source_ref_must_point_to_configured_source_selector_pair -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config derived_input_policy_field_sources_must_not_duplicate_fields -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config`: PASS, 28 tests.
- `cargo fmt --check`: PASS.
- `git diff --check`: PASS.
- `just source-fence`: PASS.

## 2026-06-10 Internal Adversarial Review After `a7074113`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `a707411387c5cc88d76a39a4d356ef6823c61cf1`
**Recommendation**: PASS for the locally reviewed fixes below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| `IvHelperPolicy.input_policy_ref` and `IvDerivedInputPolicy.helper_policy_ref` could disagree while both IDs existed. Query-time derivation follows only `helper.input_policy_ref`, so TOML could claim one helper/input relationship while runtime used another. | RED: `cargo test --locked --test bolt_v3_iv_config helper_and_derived_input_policy_refs_must_be_reciprocal -- --nocapture` failed because validation accepted the mismatched reciprocal references. | Config validation now requires helper and derived-input policies to reference each other reciprocally. |
| A derived-input field policy could set `allowed_source_kinds = []`, which query-time validation treated as unrestricted. | RED: `cargo test --locked --test bolt_v3_iv_config derived_input_policy_field_sources_must_allow_at_least_one_source_kind -- --nocapture` failed because validation accepted the empty allowlist. | Config validation now rejects empty `allowed_source_kinds` per derived-input field source. |
| Derived-input field-source config could be contradictory, such as a `profile_source_ref` present without allowing `profile_source_ref`, or an operator value whose embedded `source_kind` was not `operator_configured`. | RED: `cargo test --locked --test bolt_v3_iv_config derived_input_profile_source_ref_requires_profile_source_ref_kind -- --nocapture` and `cargo test --locked --test bolt_v3_iv_config derived_input_operator_values_must_be_operator_configured_kind -- --nocapture` failed because validation accepted both contradictions. | Config validation now rejects inconsistent profile-source and operator-configured field-source declarations before runtime query resolution. |

### 2026-06-10 Post-`a7074113` Verification

- `cargo test --locked --test bolt_v3_iv_config helper_and_derived_input_policy_refs_must_be_reciprocal -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config derived_input_policy_field_sources_must_allow_at_least_one_source_kind -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config derived_input_profile_source_ref_requires_profile_source_ref_kind -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config derived_input_operator_values_must_be_operator_configured_kind -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config`: PASS, 32 tests.
- `cargo fmt --check`: PASS.
- `git diff --check`: PASS.
- `just source-fence`: PASS.

## 2026-06-10 Internal Adversarial Review After `a25760bb`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after head `a25760bb686467f16797c574ea9ebf4821757057`
**Recommendation**: PASS for the locally reviewed fix below; GitHub CI and external relay approvals must be refreshed after this delta is committed and pushed.

| Issue | Evidence | Resolution |
|---|---|---|
| SC-006 was only partially proven: `tests/bolt_v3_iv_query.rs` manually built two `IvQueryHandle`s with different selector authorizations, but root TOML and live registration cloned one profile-level selector authorization for every strategy in the profile. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_registry_supports_two_configured_strategies_with_different_selectors -- --nocapture` failed because TOML rejected `strategy_authorizations`, proving the live/config path could not express two strategies with different selectors in one profile. | `IvProfile` now owns `strategy_authorizations: Vec<IvSelectorAuthorization>` entries, each carrying its strategy ID, product allowlist, selector allowlist, and source allowlist. Registry construction and strategy-reference validation consume those entries directly, and the RED test is GREEN. |

### 2026-06-10 Post-`a25760bb` Verification

- `cargo test --locked --test bolt_v3_iv_live_integration runtime_registry_supports_two_configured_strategies_with_different_selectors -- --nocapture`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config --test bolt_v3_iv_subscription --test bolt_v3_iv_live_integration`: PASS, 72 tests.
- `cargo test --locked --test bolt_v3_iv_query --test bolt_v3_iv_foundation`: PASS, 53 tests.
- `cargo fmt --check`: PASS.
- `git diff --check`: PASS.
- `just source-fence`: PASS.

## 2026-06-12 Completion Review After `ad57ac4`

**Reviewed working tree**: branch `026-nt-backed-iv-engine`, local delta after PR head `ad57ac408dbeed4e2d61673f3fd80ed2d815e4d8`
**Recommendation**: PASS for local non-compile review gates; publish with `git push` and adjudicate advisory CI for the exact head.

| Issue | Evidence | Resolution |
|---|---|---|
| Capability classification rules could still hide review-worthy NT IV/options candidates behind deep module prefixes. | The existing broad-prefix guard only checked crate-root prefixes. A new regression covers a volatility candidate matched by `nt.crates.indicators.src.volatility.`. | `candidate_requires_exact_review()` now applies to any exclusion/not-IV/unreachable rule match, forcing exact ledger entries for greeks, implied, IV, volatility, smile, and option-surface candidates. Current pinned-checkout false positives are classified explicitly in `capability-ledger.toml`. |
| Strategy-registration IV query handles copied config-provided derived inputs without the Cargo-pinned NT revision stamping used by runtime profile state. | `live_root_registry_stamps_derived_inputs_with_cargo_pinned_nt_revision` covers a config-derived input whose stale revision must be replaced before reaching the strategy handle. | `runtime_derived_inputs_from_profile()` is shared with strategy registration, and the `DerivedInputDiagnostics` query product exposes test evidence for the stamped state through `IvQuery`. |
| Aggregate-greeks custom-data subscription command translation lacked direct live-node coverage. | New `iv_aggregate_greeks_start_plan_translates_to_runtime_custom_data_command` checks command type, client ID, data type, identifier, merged source/selector metadata, and command params. | Live-node command translation now has explicit aggregate-greeks evidence matching the existing custom-IV path. |
| Strategy authorization `allowed_source_ids` validation had production logic but no direct regression coverage. | `strategy_authorization_rejects_unknown_allowed_source_id` proves unknown source IDs reject at config validation. | The existing validator behavior is now locked by a focused config test. |
| PR branch verification tooling was stale relative to current main's remote-first Rust verification policy. | `cargo fmt --check` and `just source-fence` initially failed because the branch had schema v1 `ci/rust-verification.toml`, while the machine cargo shim requires schema v2 `local_compile_policy`. | Ported the current-main schema v2 policy, `verify-remote`, `source-fence-static`, local-compile refusal logic, no-mistakes command mapping, and matching verifier self-tests while preserving the IV source-fence test in full `source-fence` for CI. |

### 2026-06-12 Local Non-Compile Verification

- `cargo fmt --check`: PASS.
- `git diff --check`: PASS.
- `just source-fence-static`: PASS.
- `python3 scripts/test_verify_remote.py`: PASS.
- `just ci-lint-workflow`: PASS.
- Local compile-heavy Rust tests/clippy were not run by design; exact-head GitHub CI is the required proof after push.
