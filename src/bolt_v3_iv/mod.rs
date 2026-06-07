//! NT-backed implied-volatility engine boundary.
//!
//! Foundational types are intentionally strategy-, venue-, market-, asset-,
//! instrument-, source-, and cadence-agnostic.

pub mod audit;
pub mod authz;
pub mod bounds;
pub mod capability;
pub mod error;
pub mod health;
pub mod ingest;
pub mod provenance;
pub mod raw_access;
pub mod runtime;
pub mod selector;
pub mod store;
pub mod subscription;
pub mod time;
pub mod types;
