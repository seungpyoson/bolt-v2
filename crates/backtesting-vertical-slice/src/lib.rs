//! NautilusTrader backtesting vertical slice (spec 023 `1-backtesting-engine`).
//!
//! The slice implements the smallest verified path from accepted historical
//! data to an objective backtest result:
//!
//! ```text
//! accepted SourceProofReport
//!   -> canonical normalized `trades` table (backfill-table-contract.v1)
//!   -> NautilusTrader ParquetDataCatalog projection (TradeTick)
//!   -> NautilusTrader BacktestNode run (existing compiled Rust strategy)
//!   -> objective BacktestResultContract
//! ```
//!
//! Bolt owns source-proof acceptance, canonical normalization, manifest
//! validation, and claim-limit enforcement. NautilusTrader owns the catalog,
//! simulated venue, order/fill lifecycle, portfolio truth, and results.
//!
//! Hard rule: no backtest consumes raw staged data directly. The only path to
//! backtest input is an [`source_proof::AcceptedDataset`] produced from an
//! accepted [`source_proof::SourceProofReport`].

pub mod artifact_index;
pub mod artifact_index_audit;
pub mod artifact_index_commit_proof;
pub mod artifact_index_iam_policy;
pub mod artifact_store;
pub mod artifact_store_secrets;
pub mod atomic_artifact_write;
pub use atomic_artifact_write::atomic_write;
pub mod backfill_accepted_tranche;
pub mod backfill_binding_coverage;
pub mod backfill_conversion_batch;
pub mod backfill_conversion_completion;
pub mod backfill_coverage;
pub mod backfill_execution_plan;
pub mod backfill_execution_readiness;
pub mod backfill_object_staging;
pub mod backfill_preflight;
pub mod backfill_readiness;
pub mod backfill_run_spec_materialization;
pub mod backfill_source_proof_scope;
pub mod canonical_bars;
pub mod canonical_market_data;
pub mod canonical_order_book_deltas;
pub mod canonical_trades;
pub mod catalog_projection;
pub mod conversion_boundary;
pub mod dashboard_contract;
pub mod domain_metrics;
pub mod economics;
pub mod execution_contract;
#[cfg(test)]
pub(crate) mod execution_evidence;
pub mod first_proof_selector;
pub mod hashing;
pub mod io_safety;
pub mod leadlag_catalog_reader;
pub mod mechanical_probe_strategy;
pub mod nt_catalog_capability;
pub mod nt_catalog_proof;
pub mod nt_dependency_proof;
pub mod operator;
pub mod path_resolution;
pub mod pmxt_one_off_backfill_projection;
pub mod polymarket_metadata_gate;
pub mod polymarket_nt_surface_proof;
pub mod reference_artifact;
pub mod reference_fixture_index;
pub mod research_analytics;
pub mod research_reader;
pub mod result_contract;
pub mod run_manifest;
pub mod runner;
pub mod seeded_l2_quotes;
pub mod selected_source_slice;
pub mod source_archive_discovery_seed;
pub mod source_archive_index_manifest;
pub mod source_archive_index_source_universe;
pub mod source_catalog_mapping_readiness;
pub mod source_proof;
pub mod source_proof_admissibility;
pub mod source_proof_evidence_staging;
pub mod source_proof_legacy_derivability;
pub mod source_proof_migration_preflight;
pub mod source_proof_shortlist;
pub mod source_selection_readiness;
pub mod source_universe_batch_execution;
pub mod source_universe_conversion_queue;
pub mod source_universe_conversion_run_plan;
pub mod source_universe_conversion_work_order;
pub mod source_universe_execution_acceptance;
pub mod source_universe_execution_pack;
pub mod source_universe_object_gates;
pub mod source_universe_operator_inputs;
pub mod source_universe_source_proofs;
pub mod tar_reader;
pub mod venue_scale_conversion_acceptance;
pub mod zip_reader;
