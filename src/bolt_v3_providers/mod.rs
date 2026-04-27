//! Per-provider binding root for bolt-v3 venue config block shapes.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus minimal dispatch identifiers such as
//! `VenueKind::Polymarket` / `VenueKind::Binance`; the concrete
//! `[venues.<name>.{data,execution,secrets}]` block shapes themselves
//! live in per-provider binding modules under this root. This namespace
//! is the family-agnostic entry point: today it exposes `polymarket`
//! and `binance`, and a future provider adds its own per-provider
//! module here without changes to core config.

pub mod binance;
pub mod polymarket;
