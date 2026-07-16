#![recursion_limit = "256"]

#[path = "backtesting_vertical_slice_artifact_index.rs"]
mod backtesting_vertical_slice_artifact_index;
#[path = "backtesting_vertical_slice_artifact_index_commit_proof.rs"]
mod backtesting_vertical_slice_artifact_index_commit_proof;
#[path = "backtesting_vertical_slice_artifact_index_iam_policy.rs"]
mod backtesting_vertical_slice_artifact_index_iam_policy;
#[path = "backtesting_vertical_slice_artifact_store_secrets.rs"]
mod backtesting_vertical_slice_artifact_store_secrets;
#[path = "backtesting_vertical_slice_backfill_accepted_tranche.rs"]
mod backtesting_vertical_slice_backfill_accepted_tranche;
#[path = "backtesting_vertical_slice_backfill_binding_coverage.rs"]
mod backtesting_vertical_slice_backfill_binding_coverage;
#[path = "backtesting_vertical_slice_backfill_conversion_batch.rs"]
mod backtesting_vertical_slice_backfill_conversion_batch;
#[path = "backtesting_vertical_slice_backfill_coverage.rs"]
mod backtesting_vertical_slice_backfill_coverage;
#[path = "backtesting_vertical_slice_backfill_coverage_cli.rs"]
mod backtesting_vertical_slice_backfill_coverage_cli;
#[path = "backtesting_vertical_slice_backfill_execution_plan.rs"]
mod backtesting_vertical_slice_backfill_execution_plan;
#[path = "backtesting_vertical_slice_backfill_execution_readiness.rs"]
mod backtesting_vertical_slice_backfill_execution_readiness;
#[path = "backtesting_vertical_slice_retired_backfill_evidence.rs"]
mod backtesting_vertical_slice_retired_backfill_evidence;
#[path = "backtesting_vertical_slice_bybit_source_universe_reference_artifacts.rs"]
mod backtesting_vertical_slice_bybit_source_universe_reference_artifacts;
#[path = "backtesting_vertical_slice_backfill_object_staging.rs"]
mod backtesting_vertical_slice_backfill_object_staging;
#[path = "backtesting_vertical_slice_backfill_preflight.rs"]
mod backtesting_vertical_slice_backfill_preflight;
#[path = "backtesting_vertical_slice_backfill_readiness.rs"]
mod backtesting_vertical_slice_backfill_readiness;
#[path = "backtesting_vertical_slice_backfill_run_spec_materialization.rs"]
mod backtesting_vertical_slice_backfill_run_spec_materialization;
#[path = "backtesting_vertical_slice_backfill_source_proof_scope.rs"]
mod backtesting_vertical_slice_backfill_source_proof_scope;
#[path = "backtesting_vertical_slice_bar_format_families.rs"]
mod backtesting_vertical_slice_bar_format_families;
#[path = "backtesting_vertical_slice_bar_projection.rs"]
mod backtesting_vertical_slice_bar_projection;
#[path = "backtesting_vertical_slice_bar_source_adapter.rs"]
mod backtesting_vertical_slice_bar_source_adapter;
#[path = "backtesting_vertical_slice_catalog_and_node.rs"]
mod backtesting_vertical_slice_catalog_and_node;
#[path = "backtesting_vertical_slice_conversion_boundary.rs"]
mod backtesting_vertical_slice_conversion_boundary;
#[path = "backtesting_vertical_slice_custom_data_replay.rs"]
mod backtesting_vertical_slice_custom_data_replay;
#[path = "backtesting_vertical_slice_end_to_end.rs"]
mod backtesting_vertical_slice_end_to_end;
#[path = "backtesting_vertical_slice_first_proof_selector.rs"]
mod backtesting_vertical_slice_first_proof_selector;
#[path = "backtesting_vertical_slice_hash_what_you_write.rs"]
mod backtesting_vertical_slice_hash_what_you_write;
#[path = "backtesting_vertical_slice_l2_snapshot_adapter.rs"]
mod backtesting_vertical_slice_l2_snapshot_adapter;
#[path = "backtesting_vertical_slice_mechanical_order_proof.rs"]
mod backtesting_vertical_slice_mechanical_order_proof;
#[path = "backtesting_vertical_slice_nt_dependency_proof.rs"]
mod backtesting_vertical_slice_nt_dependency_proof;
#[path = "backtesting_vertical_slice_operator_binding_bars.rs"]
mod backtesting_vertical_slice_operator_binding_bars;
#[path = "backtesting_vertical_slice_operator_binding_deltas.rs"]
mod backtesting_vertical_slice_operator_binding_deltas;
#[path = "backtesting_vertical_slice_operator_binding_parquet.rs"]
mod backtesting_vertical_slice_operator_binding_parquet;
#[path = "backtesting_vertical_slice_operator_binding_trade_stability.rs"]
mod backtesting_vertical_slice_operator_binding_trade_stability;
#[path = "backtesting_vertical_slice_order_book_delta_projection.rs"]
mod backtesting_vertical_slice_order_book_delta_projection;
#[path = "backtesting_vertical_slice_parquet_event_adapter.rs"]
mod backtesting_vertical_slice_parquet_event_adapter;
#[path = "backtesting_vertical_slice_pmxt_one_off_projection.rs"]
mod backtesting_vertical_slice_pmxt_one_off_projection;
#[path = "backtesting_vertical_slice_pmxt_reference_conversion.rs"]
mod backtesting_vertical_slice_pmxt_reference_conversion;
#[path = "backtesting_vertical_slice_polymarket_metadata_gate.rs"]
mod backtesting_vertical_slice_polymarket_metadata_gate;
#[path = "backtesting_vertical_slice_polymarket_nt_surface.rs"]
mod backtesting_vertical_slice_polymarket_nt_surface;
#[path = "backtesting_vertical_slice_reference_fixture_eviction.rs"]
mod backtesting_vertical_slice_reference_fixture_eviction;
#[path = "backtesting_vertical_slice_research_analytics.rs"]
mod backtesting_vertical_slice_research_analytics;
#[path = "backtesting_vertical_slice_s3_catalog_smoke.rs"]
mod backtesting_vertical_slice_s3_catalog_smoke;
#[path = "backtesting_vertical_slice_sample_venue_guard.rs"]
mod backtesting_vertical_slice_sample_venue_guard;
#[path = "backtesting_vertical_slice_selected_source_slice.rs"]
mod backtesting_vertical_slice_selected_source_slice;
#[path = "backtesting_vertical_slice_source_archive_discovery_seed.rs"]
mod backtesting_vertical_slice_source_archive_discovery_seed;
#[path = "backtesting_vertical_slice_source_archive_index_manifest.rs"]
mod backtesting_vertical_slice_source_archive_index_manifest;
#[path = "backtesting_vertical_slice_source_archive_index_source_universe.rs"]
mod backtesting_vertical_slice_source_archive_index_source_universe;
#[path = "backtesting_vertical_slice_source_catalog_mapping_readiness.rs"]
mod backtesting_vertical_slice_source_catalog_mapping_readiness;
#[path = "backtesting_vertical_slice_source_proof_admissibility.rs"]
mod backtesting_vertical_slice_source_proof_admissibility;
#[path = "backtesting_vertical_slice_source_proof_admissibility_cli.rs"]
mod backtesting_vertical_slice_source_proof_admissibility_cli;
#[path = "backtesting_vertical_slice_source_proof_evidence_staging.rs"]
mod backtesting_vertical_slice_source_proof_evidence_staging;
#[path = "backtesting_vertical_slice_source_proof_legacy_derivability.rs"]
mod backtesting_vertical_slice_source_proof_legacy_derivability;
#[path = "backtesting_vertical_slice_source_proof_migration_preflight.rs"]
mod backtesting_vertical_slice_source_proof_migration_preflight;
#[path = "backtesting_vertical_slice_source_proof_reference_fixtures.rs"]
mod backtesting_vertical_slice_source_proof_reference_fixtures;
#[path = "backtesting_vertical_slice_source_proof_shortlist.rs"]
mod backtesting_vertical_slice_source_proof_shortlist;
#[path = "backtesting_vertical_slice_source_selection_readiness.rs"]
mod backtesting_vertical_slice_source_selection_readiness;
#[path = "backtesting_vertical_slice_source_universe_artifact_refs.rs"]
mod backtesting_vertical_slice_source_universe_artifact_refs;
#[path = "backtesting_vertical_slice_source_universe_conversion_queue.rs"]
mod backtesting_vertical_slice_source_universe_conversion_queue;
#[path = "backtesting_vertical_slice_source_universe_conversion_run_plan.rs"]
mod backtesting_vertical_slice_source_universe_conversion_run_plan;
#[path = "backtesting_vertical_slice_source_universe_conversion_work_order.rs"]
mod backtesting_vertical_slice_source_universe_conversion_work_order;
#[path = "backtesting_vertical_slice_source_universe_execution_acceptance.rs"]
mod backtesting_vertical_slice_source_universe_execution_acceptance;
#[path = "backtesting_vertical_slice_source_universe_execution_pack.rs"]
mod backtesting_vertical_slice_source_universe_execution_pack;
#[path = "backtesting_vertical_slice_source_universe_object_gates.rs"]
mod backtesting_vertical_slice_source_universe_object_gates;
#[path = "backtesting_vertical_slice_source_universe_operator_inputs.rs"]
mod backtesting_vertical_slice_source_universe_operator_inputs;
#[path = "backtesting_vertical_slice_source_universe_portable_paths.rs"]
mod backtesting_vertical_slice_source_universe_portable_paths;
#[path = "backtesting_vertical_slice_source_universe_source_proofs.rs"]
mod backtesting_vertical_slice_source_universe_source_proofs;
#[path = "backtesting_vertical_slice_tar_snapshot_adapter.rs"]
mod backtesting_vertical_slice_tar_snapshot_adapter;
#[path = "backtesting_vertical_slice_test_support.rs"]
mod backtesting_vertical_slice_test_support;
#[path = "backtesting_vertical_slice_venue_scale_conversion_acceptance.rs"]
mod backtesting_vertical_slice_venue_scale_conversion_acceptance;
#[path = "dashboard_contract.rs"]
mod dashboard_contract;
#[path = "research_reader_contract.rs"]
mod research_reader_contract;
