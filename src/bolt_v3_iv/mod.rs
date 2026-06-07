//! NT-backed implied-volatility engine boundary.
//!
//! Foundational types are intentionally strategy-, venue-, market-, asset-,
//! instrument-, source-, and cadence-agnostic.

pub mod audit;
pub mod authz;
pub mod bounds;
pub mod error;
pub mod health;
pub mod provenance;
pub mod selector;
pub mod time;
pub mod types;
