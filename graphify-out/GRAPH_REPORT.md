# Graph Report - /Users/spson/Projects/Claude/bolt-v2  (2026-04-16)

## Corpus Check
- 84 files · ~90,124 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 1533 nodes · 3335 edges · 63 communities detected
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS · INFERRED: 11 edges (avg confidence: 0.81)
- Token cost: 0 input · 0 output

## Community Hubs (Navigation)
- [[_COMMUNITY_Community 0|Community 0]]
- [[_COMMUNITY_Community 1|Community 1]]
- [[_COMMUNITY_Community 2|Community 2]]
- [[_COMMUNITY_Community 3|Community 3]]
- [[_COMMUNITY_Community 4|Community 4]]
- [[_COMMUNITY_Community 5|Community 5]]
- [[_COMMUNITY_Community 6|Community 6]]
- [[_COMMUNITY_Community 7|Community 7]]
- [[_COMMUNITY_Community 8|Community 8]]
- [[_COMMUNITY_Community 9|Community 9]]
- [[_COMMUNITY_Community 10|Community 10]]
- [[_COMMUNITY_Community 11|Community 11]]
- [[_COMMUNITY_Community 12|Community 12]]
- [[_COMMUNITY_Community 13|Community 13]]
- [[_COMMUNITY_Community 14|Community 14]]
- [[_COMMUNITY_Community 15|Community 15]]
- [[_COMMUNITY_Community 16|Community 16]]
- [[_COMMUNITY_Community 17|Community 17]]
- [[_COMMUNITY_Community 18|Community 18]]
- [[_COMMUNITY_Community 19|Community 19]]
- [[_COMMUNITY_Community 20|Community 20]]
- [[_COMMUNITY_Community 21|Community 21]]
- [[_COMMUNITY_Community 22|Community 22]]
- [[_COMMUNITY_Community 23|Community 23]]
- [[_COMMUNITY_Community 24|Community 24]]
- [[_COMMUNITY_Community 25|Community 25]]
- [[_COMMUNITY_Community 26|Community 26]]
- [[_COMMUNITY_Community 27|Community 27]]
- [[_COMMUNITY_Community 28|Community 28]]
- [[_COMMUNITY_Community 29|Community 29]]
- [[_COMMUNITY_Community 30|Community 30]]
- [[_COMMUNITY_Community 31|Community 31]]
- [[_COMMUNITY_Community 32|Community 32]]
- [[_COMMUNITY_Community 33|Community 33]]
- [[_COMMUNITY_Community 34|Community 34]]
- [[_COMMUNITY_Community 35|Community 35]]
- [[_COMMUNITY_Community 36|Community 36]]
- [[_COMMUNITY_Community 37|Community 37]]
- [[_COMMUNITY_Community 38|Community 38]]
- [[_COMMUNITY_Community 39|Community 39]]
- [[_COMMUNITY_Community 40|Community 40]]
- [[_COMMUNITY_Community 41|Community 41]]
- [[_COMMUNITY_Community 42|Community 42]]
- [[_COMMUNITY_Community 43|Community 43]]
- [[_COMMUNITY_Community 44|Community 44]]
- [[_COMMUNITY_Community 45|Community 45]]
- [[_COMMUNITY_Community 46|Community 46]]
- [[_COMMUNITY_Community 47|Community 47]]
- [[_COMMUNITY_Community 48|Community 48]]
- [[_COMMUNITY_Community 49|Community 49]]
- [[_COMMUNITY_Community 50|Community 50]]
- [[_COMMUNITY_Community 51|Community 51]]
- [[_COMMUNITY_Community 52|Community 52]]
- [[_COMMUNITY_Community 53|Community 53]]
- [[_COMMUNITY_Community 54|Community 54]]
- [[_COMMUNITY_Community 55|Community 55]]
- [[_COMMUNITY_Community 56|Community 56]]
- [[_COMMUNITY_Community 57|Community 57]]
- [[_COMMUNITY_Community 58|Community 58]]
- [[_COMMUNITY_Community 59|Community 59]]
- [[_COMMUNITY_Community 60|Community 60]]
- [[_COMMUNITY_Community 61|Community 61]]
- [[_COMMUNITY_Community 62|Community 62]]

## God Nodes (most connected - your core abstractions)
1. `assert_has_error()` - 119 edges
2. `replace()` - 118 edges
3. `errors_for()` - 77 edges
4. `runtime_errors_for()` - 51 edges
5. `valid_toml()` - 48 edges
6. `valid_phase1_runtime_toml()` - 31 edges
7. `push_error()` - 30 edges
8. `valid_phase1_toml()` - 30 edges
9. `validate_runtime_with_registry()` - 24 edges
10. `config()` - 22 edges

## Surprising Connections (you probably didn't know these)
- `NT Pointer Probe Design` --rationale_for--> `_validate_snapshot()`  [EXTRACTED]
  docs/superpowers/specs/2026-04-14-nt-pointer-probe-design.md → /Users/spson/Projects/Claude/bolt-v2/bolt_trust_root_validator.py
- `Log Sweep at Launch Plan` --rationale_for--> `accepts_canonical_nt_log()`  [EXTRACTED]
  docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md → /Users/spson/Projects/Claude/bolt-v2/tests/log_sweep.rs
- `Log Sweep at Launch Plan` --rationale_for--> `sweep_moves_matching_logs_to_target()`  [EXTRACTED]
  docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md → /Users/spson/Projects/Claude/bolt-v2/tests/log_sweep.rs
- `Log Sweep at Launch Plan` --rationale_for--> `sweep_skips_existing_destination()`  [EXTRACTED]
  docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md → /Users/spson/Projects/Claude/bolt-v2/tests/log_sweep.rs
- `Log Sweep at Launch Plan` --rationale_for--> `sweep_noop_when_no_logs()`  [EXTRACTED]
  docs/superpowers/plans/2026-04-11-log-sweep-at-launch.md → /Users/spson/Projects/Claude/bolt-v2/tests/log_sweep.rs

## Communities

### Community 0 - "Community 0"
Cohesion: 0.05
Nodes (158): account_id_without_hyphen_rejected(), all_ssm_paths_validated(), assert_error_message_contains(), assert_error_message_not_contains(), assert_has_error(), assert_no_errors(), contract_path_requires_streaming_catalog_path(), duplicate_data_client_names_rejected() (+150 more)

### Community 1 - "Community 1"
Cohesion: 0.03
Nodes (64): Batch Lake + Enrichment Job, Canonical identity (condition_id + token_id), Controlplane (bolt v1 deployment layer), NT execution event-history seam (#33), NT FeatherWriter streaming sink, GROUP BY CHANGE rule, Main is authoritative after merge, Do not reference bolt v1 source (+56 more)

### Community 2 - "Community 2"
Cohesion: 0.04
Nodes (76): create_dir_all_at(), default_client_name(), default_delay_post_stop_secs(), default_delay_shutdown_secs(), default_environment(), default_file_level(), default_gamma_refresh_interval_secs(), default_live_raw_capture_output_dir() (+68 more)

### Community 3 - "Community 3"
Cohesion: 0.09
Nodes (54): audit_task_failure_triggers_fail_closed_shutdown(), background_producers_stop_emitting_before_runtime_shutdown(), binance_btcusdt_1m(), build_delayed_start_node(), build_lifecycle_node(), build_node(), cancellation_token_stops_background_tasks_cleanly(), candidate_market() (+46 more)

### Community 4 - "Community 4"
Cohesion: 0.05
Nodes (57): CanaryEntry, canonical_dependency_name(), canonical_toml_value(), cargo_config_state_includes_full_config_contents(), cargo_nt_extractor_scans_replace_section(), cargo_nt_extractor_scans_workspace_target_build_and_patch_sections(), collect_nt_data_from_toml_table(), compare_branch_governance_responses() (+49 more)

### Community 5 - "Community 5"
Cohesion: 0.05
Nodes (37): build_chainlink_reference_data_client(), build_chainlink_reference_data_client_with_secrets(), chainlink_data_type_for_venue(), chainlink_feed_version(), chainlink_topic_for_venue(), ChainlinkOracleUpdate, ChainlinkReferenceClientConfig, ChainlinkReferenceDataClient (+29 more)

### Community 6 - "Community 6"
Cohesion: 0.06
Nodes (40): ActiveUpload, audit_channel(), AuditChannelCloser, AuditChannelState, AuditReceiver, AuditRecord, AuditSender, AuditSendError (+32 more)

### Community 7 - "Community 7"
Cohesion: 0.06
Nodes (44): Typed ResolutionBasis model, Resolution-Basis Selector Generalization Design, description_and_ruleset_parsers_land_on_same_canonical_basis(), event_with_markets(), load_markets_from_event_markets(), load_markets_from_event_pages(), load_markets_from_ruleset_and_event_pages(), loads_candidate_markets_for_ruleset_and_translates_seconds_to_end() (+36 more)

### Community 8 - "Community 8"
Cohesion: 0.06
Nodes (43): add_reference_actor(), add_runtime_strategy_applier_actor(), build_empty_node(), CandidateMarketLoader, client_id_for_reference_venue(), config_with_ruleset_and_zero_templates(), config_with_runtime_strategy(), fail_closed() (+35 more)

### Community 9 - "Community 9"
Cohesion: 0.06
Nodes (26): _default_policy(), _file_sha256(), _load_policy(), main(), _normalize_policy_path(), _normalize_protected_entry(), _parse_args(), _validate_snapshot() (+18 more)

### Community 10 - "Community 10"
Cohesion: 0.05
Nodes (11): clear_mock_data_subscriptions(), mock_data_subscriptions(), MockDataClient, MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClient, MockExecutionClientFactory (+3 more)

### Community 11 - "Community 11"
Cohesion: 0.08
Nodes (48): Fail-closed gate policy, NT probe evidence contract, Safe-list (non-overlapping NT paths), NT seam registry, NT Pointer Probe Design, absolute_control_plane_paths_fail_before_loading_artifacts(), branch_governance_comparison_accepts_matching_fixture(), branch_governance_comparison_rejects_bypass_actor_drift() (+40 more)

### Community 12 - "Community 12"
Cohesion: 0.07
Nodes (28): Atomic exec_tester purge, Polymarket FeeProvider (CLOB, TTL cache), RuntimeSelectionSnapshot envelope, Typed SlugMatcher / event_slug_prefix, StrategyRegistry / StrategyBuilder seam, Issue #134 Runtime Enablement Plan, ClobFeeRateFetcher, fee_provider_cache_hit_within_ttl_skips_refresh() (+20 more)

### Community 13 - "Community 13"
Cohesion: 0.16
Nodes (37): audit_records_serialize_as_jsonl(), backlog_limit_breach_returns_error(), blocked_inflight_upload_times_out_on_shutdown_and_retains_spool_file(), blocked_upload_does_not_prevent_continued_spooling_or_backlog_enforcement(), config(), corrupt_retained_jsonl_fails_closed_on_restart(), decision_record(), failed_retained_upload_retries_on_next_ship_tick_not_failure_deadline() (+29 more)

### Community 14 - "Community 14"
Cohesion: 0.1
Nodes (32): AnyHandlers, bars_pattern(), book_deltas_pattern(), book_depth10_pattern(), contract_startup_summary(), ContractStartupSummary, ensure_local_catalog_path(), failure_state_latches_first_error_and_sets_stop_flag() (+24 more)

### Community 15 - "Community 15"
Cohesion: 0.1
Nodes (30): absolute_path(), build_completeness_report(), classify_flat_file(), classify_unknown_flat_file(), collect_feather_files(), completeness_report_keeps_optional_unconverted_spool_nonfatal(), convert_class_to_parquet(), convert_live_spool_to_parquet() (+22 more)

### Community 16 - "Community 16"
Cohesion: 0.16
Nodes (36): check_allowlist(), check_chainlink_feed_id(), check_chainlink_reference_config(), check_chainlink_shared_config(), check_chainlink_ws_origins(), check_contract_path_catalog_dependency(), check_hex_prefixed(), check_instrument_id() (+28 more)

### Community 17 - "Community 17"
Cohesion: 0.11
Nodes (25): build_data_client(), build_data_client_skips_new_market_filter_for_mixed_selectors(), build_data_client_uses_event_params_filter_for_tag_only_selectors(), build_data_client_uses_event_slug_filter_when_selector_state_present(), build_exec_client(), build_fee_provider(), build_selector_state(), fetch_gamma_events_paginated() (+17 more)

### Community 18 - "Community 18"
Cohesion: 0.18
Nodes (28): assert_contract_ignores_legacy_flat_instruments_file(), assert_failure_report_only(), base_polymarket_streams(), contract_allows_optional_class_with_spool_present_but_no_converted_rows(), contract_fails_when_disabled_conditional_stream_has_data(), contract_fails_when_disabled_supported_stream_has_data(), contract_fails_when_legacy_flat_status_file_is_present(), contract_fails_when_required_class_absent() (+20 more)

### Community 19 - "Community 19"
Cohesion: 0.2
Nodes (22): collect_polymarket_startup_validation_targets(), collect_polymarket_startup_validation_targets_with_resolver(), polymarket_prefix_ruleset(), polymarket_ruleset(), PolymarketDiscoveryMode, PolymarketStartupValidationTargets, startup_validation_accepts_matching_discovered_instrument_ids(), startup_validation_accepts_matching_instrument_id() (+14 more)

### Community 20 - "Community 20"
Cohesion: 0.34
Nodes (20): actor_config(), advance_actor_clock_to(), chainlink_custom_data_updates_latest_oracle_observation_and_publishes_snapshot(), collect_snapshots(), disabled_venue_still_appears_in_snapshot_with_zero_weight(), min_publish_interval_throttles_snapshot_emission(), out_of_order_chainlink_custom_data_does_not_replace_latest_or_publish(), out_of_order_quotes_do_not_replace_latest_or_publish() (+12 more)

### Community 21 - "Community 21"
Cohesion: 0.16
Nodes (17): enters_freeze_state_at_exact_freeze_boundary(), enters_freeze_state_near_market_end(), enters_freeze_state_with_rejected_candidates_present(), evaluate_market_selection_yields_empty_rejected_when_all_candidates_eligible(), exposes_rejected_candidates_with_explicit_eligibility_reasons(), polymarket_selector(), polymarket_selector_with_prefix(), rejects_market_when_resolution_basis_mismatches() (+9 more)

### Community 22 - "Community 22"
Cohesion: 0.22
Nodes (15): assert_generated_output(), assert_mode(), assert_read_only(), make_writable_if_needed(), materialize_live_config_creates_missing_parent_directories(), materialize_live_config_creates_read_only_output(), materialize_live_config_leaves_matching_read_only_output_unchanged(), materialize_live_config_repairs_permissions_without_rewriting_contents() (+7 more)

### Community 23 - "Community 23"
Cohesion: 0.19
Nodes (4): OracleOrdering, ReferenceActor, ReferenceActorConfig, ReferenceSubscription

### Community 24 - "Community 24"
Cohesion: 0.14
Nodes (13): all_venues_stale_returns_none_fair_value(), assert_wrapper(), confidence_is_ratio_of_effective_to_configured_weight(), fused_price_is_weighted_mean_of_enabled_prices(), manual_disable_reason_overrides_age_based_disable(), oracle(), orderbook(), ReferenceOnlyConfig (+5 more)

### Community 25 - "Community 25"
Cohesion: 0.26
Nodes (20): binary_up_down_token_ids(), load_candidate_markets_for_ruleset(), load_candidate_markets_for_ruleset_with_gamma_client(), load_events_for_selector(), parse_market(), parse_timestamp_ms(), polymarket_instrument_id(), seconds_to_end() (+12 more)

### Community 26 - "Community 26"
Cohesion: 0.12
Nodes (4): collect_paths(), converts_execution_state_sidecars_into_parquet_outputs(), converts_legacy_flat_spool_layout(), converts_live_spool_into_queryable_parquet_under_separate_output_root()

### Community 27 - "Community 27"
Cohesion: 0.15
Nodes (6): EffectiveVenueState, fuse_reference_snapshot(), ReferenceObservation, ReferenceSnapshot, VenueHealth, VenueKind

### Community 28 - "Community 28"
Cohesion: 0.26
Nodes (13): assert_runtime_validation_failed(), new_temp_config_path(), secrets_check_fails_on_invalid_config_via_load_validation(), secrets_check_fails_when_region_is_blank(), secrets_check_fails_when_required_fields_are_missing(), secrets_check_fails_when_runtime_has_no_active_path(), secrets_check_reports_complete_secret_config(), secrets_resolve_fails_fast_when_region_is_blank() (+5 more)

### Community 29 - "Community 29"
Cohesion: 0.18
Nodes (9): Capability, ClassReport, CompletenessReport, normalize_absolute_path(), normalize_local_absolute_contract_path(), Policy, Provenance, StreamContract (+1 more)

### Community 30 - "Community 30"
Cohesion: 0.23
Nodes (5): captures_typed_quote_and_close_status_and_flushes_on_shutdown(), collect_paths(), does_not_persist_startup_buffer_if_running_was_never_reached(), keeps_bars_on_flat_legacy_spool_contract(), writes_quote_spool_with_per_instrument_layout_and_metadata()

### Community 31 - "Community 31"
Cohesion: 0.29
Nodes (9): convert_order_events_to_parquet(), convert_position_events_to_parquet(), convert_sidecars_to_parquet(), order_events_path(), OrderEventRow, position_events_path(), PositionEventRow, read_jsonl_rows() (+1 more)

### Community 32 - "Community 32"
Cohesion: 0.18
Nodes (0): 

### Community 33 - "Community 33"
Cohesion: 0.25
Nodes (2): build_gamma_http_client(), gamma_default_headers()

### Community 34 - "Community 34"
Cohesion: 0.36
Nodes (8): Cli, collect_raw_capture_targets(), current_ingest_date(), http_output_path(), main(), now_unix_nanos(), RawCaptureTargets, ws_output_path()

### Community 35 - "Community 35"
Cohesion: 0.32
Nodes (2): StubRuntimeStrategy, StubRuntimeStrategyBuilder

### Community 36 - "Community 36"
Cohesion: 0.4
Nodes (0): 

### Community 37 - "Community 37"
Cohesion: 0.4
Nodes (0): 

### Community 38 - "Community 38"
Cohesion: 0.6
Nodes (3): bootstrap_test_secrets(), builds_live_node_without_pre_registering_runtime_templates_in_ruleset_mode(), seam_test_uses_non_secret_placeholders()

### Community 39 - "Community 39"
Cohesion: 0.4
Nodes (1): StubRuntimeTemplateBuilder

### Community 40 - "Community 40"
Cohesion: 0.5
Nodes (0): 

### Community 41 - "Community 41"
Cohesion: 0.5
Nodes (2): Cli, Command

### Community 42 - "Community 42"
Cohesion: 0.67
Nodes (1): Cli

### Community 43 - "Community 43"
Cohesion: 0.67
Nodes (1): Cli

### Community 44 - "Community 44"
Cohesion: 1.0
Nodes (0): 

### Community 45 - "Community 45"
Cohesion: 1.0
Nodes (0): 

### Community 46 - "Community 46"
Cohesion: 1.0
Nodes (0): 

### Community 47 - "Community 47"
Cohesion: 1.0
Nodes (0): 

### Community 48 - "Community 48"
Cohesion: 1.0
Nodes (0): 

### Community 49 - "Community 49"
Cohesion: 1.0
Nodes (0): 

### Community 50 - "Community 50"
Cohesion: 1.0
Nodes (0): 

### Community 51 - "Community 51"
Cohesion: 1.0
Nodes (0): 

### Community 52 - "Community 52"
Cohesion: 1.0
Nodes (0): 

### Community 53 - "Community 53"
Cohesion: 1.0
Nodes (1): F

### Community 54 - "Community 54"
Cohesion: 1.0
Nodes (0): 

### Community 55 - "Community 55"
Cohesion: 1.0
Nodes (0): 

### Community 56 - "Community 56"
Cohesion: 1.0
Nodes (0): 

### Community 57 - "Community 57"
Cohesion: 1.0
Nodes (0): 

### Community 58 - "Community 58"
Cohesion: 1.0
Nodes (0): 

### Community 59 - "Community 59"
Cohesion: 1.0
Nodes (0): 

### Community 60 - "Community 60"
Cohesion: 1.0
Nodes (1): T

### Community 61 - "Community 61"
Cohesion: 1.0
Nodes (0): 

### Community 62 - "Community 62"
Cohesion: 1.0
Nodes (0): 

## Knowledge Gaps
- **145 isolated node(s):** `TestServerState`, `UploadCall`, `UploadCall`, `MockUploaderState`, `ReferenceOnlyConfig` (+140 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **Thin community `Community 44`** (2 nodes): `main()`, `test_registry_bug.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 45`** (2 nodes): `main()`, `test_multiple_strategies.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 46`** (2 nodes): `run_starts_and_stops_cleanly_with_test_clients_and_no_strategies()`, `live_node_run.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 47`** (2 nodes): `build_reference_data_client()`, `bybit.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 48`** (2 nodes): `build_reference_data_client()`, `kraken.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 49`** (2 nodes): `build_reference_data_client()`, `okx.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 50`** (2 nodes): `build_reference_data_client()`, `hyperliquid.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 51`** (2 nodes): `build_reference_data_client()`, `binance.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 52`** (2 nodes): `build_reference_data_client()`, `deribit.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 53`** (2 nodes): `F`, `.load()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 54`** (1 nodes): `os.py`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 55`** (1 nodes): `test_hash.py`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 56`** (1 nodes): `test_policy.py`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 57`** (1 nodes): `json.py`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 58`** (1 nodes): `script.py`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 59`** (1 nodes): `mod.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 60`** (1 nodes): `T`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 61`** (1 nodes): `mod.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Community 62`** (1 nodes): `mod.rs`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Issue #134 Runtime Enablement Plan` connect `Community 12` to `Community 1`, `Community 2`, `Community 8`, `Community 9`, `Community 16`, `Community 17`, `Community 19`, `Community 25`?**
  _High betweenness centrality (0.221) - this node is a cross-community bridge._
- **Why does `NT Pointer Probe Design` connect `Community 11` to `Community 17`, `Community 9`, `Community 23`?**
  _High betweenness centrality (0.132) - this node is a cross-community bridge._
- **Why does `replace()` connect `Community 0` to `Community 9`?**
  _High betweenness centrality (0.110) - this node is a cross-community bridge._
- **What connects `TestServerState`, `UploadCall`, `UploadCall` to the rest of the system?**
  _145 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.05 - nodes in this community are weakly interconnected._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.03 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.04 - nodes in this community are weakly interconnected._