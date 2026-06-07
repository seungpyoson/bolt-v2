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
pub mod artifact_store_secrets;
pub mod backfill_binding_coverage;
pub mod backfill_coverage;
pub mod backfill_preflight;
pub mod backfill_readiness;
pub mod backfill_source_proof_scope;
pub mod canonical_trades;
pub mod catalog_projection;
pub mod conversion_boundary;
pub mod operator;
pub mod research_analytics;
pub mod result_contract;
pub mod run_manifest;
pub mod runner;
pub mod source_proof;
pub mod source_proof_admissibility;
pub mod source_proof_legacy_derivability;
pub mod source_proof_migration_preflight;
