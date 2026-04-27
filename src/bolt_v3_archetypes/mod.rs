//! Strategy-archetype binding root for bolt-v3 startup validation.
//!
//! Core startup validation in `crate::bolt_v3_validate` is structural
//! and family-/archetype-agnostic. Anything specific to a concrete
//! `StrategyArchetype` variant — required reference-data roles,
//! the archetype's `[parameters]` row shape, archetype-shaped
//! parameter rules (entry/exit order combinations, decimal-syntax
//! checks, root risk-cap comparison), and archetype-specific error-
//! message policy — lives in a per-archetype binding module under
//! this root. This module is the family-agnostic dispatch layer: it
//! owns the match on `StrategyArchetype` and calls into the matching
//! archetype binding so core validation does not name any concrete
//! archetype variant, deserialize the archetype's parameter row, or
//! carry archetype-specific error wording. Core validation parses the
//! root risk cap once and passes it in here as
//! `default_max_notional_decimal`.
//!
//! Today bolt-v3 has a single archetype (`binary_oracle_edge_taker`).
//! When a second archetype is introduced, it adds its own per-
//! archetype module here and a new arm in
//! `validate_strategy_archetype`; core validation does not change.

pub mod binary_oracle_edge_taker;

use rust_decimal::Decimal;

use crate::bolt_v3_config::{BoltV3StrategyConfig, StrategyArchetype};

pub fn validate_strategy_archetype(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    default_max_notional_decimal: Option<&Decimal>,
) -> Vec<String> {
    match strategy.strategy_archetype {
        StrategyArchetype::BinaryOracleEdgeTaker => binary_oracle_edge_taker::validate_strategy(
            context,
            strategy,
            default_max_notional_decimal,
        ),
    }
}
