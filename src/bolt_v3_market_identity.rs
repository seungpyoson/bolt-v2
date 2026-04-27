//! Provider-neutral, family-agnostic market-identity boundary for
//! bolt-v3.
//!
//! This module is the family-agnostic boundary between configuration
//! and per-family identity binding. Today bolt-v3 has a single
//! configured market family; everything specific to that family
//! (token-table lookup, period arithmetic, market-id formatting,
//! candidate selection, target projection, family-specific error
//! variants) is owned by a per-family binding module under
//! `crate::bolt_v3_market_families`. Core may not name a family, a
//! data or venue provider, or a strategy archetype: those choices
//! belong in per-family / per-provider / per-policy modules. When a
//! second market family is introduced in a future slice, the family-
//! agnostic identity contract goes here.
//!
//! Neutrality on this file is enforced by the source-level guard
//! tests in `tests/bolt_v3_market_identity.rs`.
