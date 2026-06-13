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

pub mod artifact_store;
pub mod canonical_trades;
pub mod catalog_projection;
pub mod nt_catalog_capability;
pub mod operator;
pub mod research_reader;
pub mod result_contract;
pub mod run_manifest;
pub mod runner;
pub mod source_proof;
