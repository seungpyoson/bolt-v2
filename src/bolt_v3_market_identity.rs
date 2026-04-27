//! Pure/control-plane market-identity derivation for bolt-v3.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//! (slug derivation rule lives in Section 5.3)
//!
//! This module is intentionally a pure data boundary. It projects a
//! validated bolt-v3 configuration plus an injected `now_unix_seconds`
//! value into a `MarketIdentityPlan` plus current/next updown market
//! slug candidates. It does not register strategies, build venue
//! adapters, perform live market selection, look up instruments,
//! mutate the NautilusTrader instrument index, or construct orders.
//!
//! The runtime-contract slug-token table is sourced from
//! `crate::bolt_v3_validate::updown_cadence_slug_token` so that the
//! schema validator and the market-identity planner share one source
//! of truth for supported `cadence_seconds` values.
//!
//! Out of scope for the slice introducing this module: live runtime
//! execution, the NT instrument index, dynamic instrument filtering,
//! Polymarket Gamma price-to-beat extraction, Chainlink and fused
//! reference price derivation, and any strategy or order construction.
//! Those boundaries belong to later slices.

use crate::{
    bolt_v3_config::{
        LoadedBoltV3Config, LoadedStrategy, RotatingMarketFamily, TargetBlock, TargetKind,
    },
    bolt_v3_validate::updown_cadence_slug_token,
};

/// Pure projection of all updown rotating-market targets in a
/// validated bolt-v3 configuration. One `UpdownTargetPlan` per
/// configured strategy whose target maps to the updown family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketIdentityPlan {
    pub updown_targets: Vec<UpdownTargetPlan>,
}

/// Pure identity facts for one configured updown rotating-market
/// target. Every value here is derived from validated configuration
/// and the runtime-contract slug-token table; nothing here depends on
/// wall-clock time, the NT instrument index, or any network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub venue_config_key: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_slug_token: String,
}

/// Current and next updown market-slug candidates for a single
/// `UpdownTargetPlan` evaluated against an injected `now_unix_seconds`
/// value (intended to come from the NautilusTrader node clock at the
/// caller, not from this module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownSlugCandidates {
    pub current_period_start_unix_seconds: i64,
    pub next_period_start_unix_seconds: i64,
    pub current_market_slug: String,
    pub next_market_slug: String,
}

#[derive(Debug)]
pub enum BoltV3MarketIdentityError {
    NonPositiveCadenceSeconds {
        strategy_instance_id: Option<String>,
        cadence_seconds: i64,
    },
    UnsupportedCadenceSeconds {
        strategy_instance_id: Option<String>,
        cadence_seconds: i64,
    },
    NegativeNowUnixSeconds {
        now_unix_seconds: i64,
    },
}

impl std::fmt::Display for BoltV3MarketIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
                strategy_instance_id,
                cadence_seconds,
            } => match strategy_instance_id {
                Some(id) => write!(
                    f,
                    "strategy `{id}`: target.cadence_seconds must be a positive integer (got {cadence_seconds})"
                ),
                None => write!(
                    f,
                    "cadence_seconds must be a positive integer (got {cadence_seconds})"
                ),
            },
            BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id,
                cadence_seconds,
            } => match strategy_instance_id {
                Some(id) => write!(
                    f,
                    "strategy `{id}`: target.cadence_seconds={cadence_seconds} has no runtime-contract-defined updown slug-token mapping"
                ),
                None => write!(
                    f,
                    "cadence_seconds={cadence_seconds} has no runtime-contract-defined updown slug-token mapping"
                ),
            },
            BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_seconds } => {
                write!(
                    f,
                    "now_unix_seconds must be non-negative (got {now_unix_seconds})"
                )
            }
        }
    }
}

impl std::error::Error for BoltV3MarketIdentityError {}

/// Project every configured strategy in `loaded` into an
/// `UpdownTargetPlan`. Returns the full `MarketIdentityPlan` in
/// strategy declaration order. Fails loud if a strategy's target has
/// been mutated to bypass schema validation (non-positive or
/// unsupported `cadence_seconds`).
pub fn plan_market_identity(
    loaded: &LoadedBoltV3Config,
) -> Result<MarketIdentityPlan, BoltV3MarketIdentityError> {
    let mut updown_targets = Vec::with_capacity(loaded.strategies.len());
    for strategy in &loaded.strategies {
        if let Some(plan) = plan_strategy_updown_target(strategy)? {
            updown_targets.push(plan);
        }
    }
    Ok(MarketIdentityPlan { updown_targets })
}

fn plan_strategy_updown_target(
    strategy: &LoadedStrategy,
) -> Result<Option<UpdownTargetPlan>, BoltV3MarketIdentityError> {
    let strategy_instance_id = strategy.config.strategy_instance_id.clone();
    let venue_config_key = strategy.config.venue.clone();
    let target: &TargetBlock = &strategy.config.target;

    // Exhaustive matches: when a future variant is added to either
    // enum the build breaks here, forcing a deliberate decision about
    // how the new variant is projected into market identity.
    let TargetKind::RotatingMarket = target.kind;
    let RotatingMarketFamily::Updown = target.rotating_market_family;

    if target.cadence_seconds <= 0 {
        return Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id: Some(strategy_instance_id),
            cadence_seconds: target.cadence_seconds,
        });
    }
    let token = match updown_cadence_slug_token(target.cadence_seconds) {
        Some(token) => token,
        None => {
            return Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id: Some(strategy_instance_id),
                cadence_seconds: target.cadence_seconds,
            });
        }
    };

    Ok(Some(UpdownTargetPlan {
        strategy_instance_id,
        configured_target_id: target.configured_target_id.clone(),
        venue_config_key,
        underlying_asset: target.underlying_asset.clone(),
        cadence_seconds: target.cadence_seconds,
        cadence_slug_token: token.to_string(),
    }))
}

/// Compute the current and next updown period start values from
/// `cadence_seconds` and `now_unix_seconds`, following the runtime
/// contract:
///   `current = floor(now / cadence) * cadence`
///   `next = current + cadence`
pub fn updown_period_pair(
    cadence_seconds: i64,
    now_unix_seconds: i64,
) -> Result<(i64, i64), BoltV3MarketIdentityError> {
    if cadence_seconds <= 0 {
        return Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id: None,
            cadence_seconds,
        });
    }
    if now_unix_seconds < 0 {
        return Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_seconds });
    }
    let current = (now_unix_seconds / cadence_seconds) * cadence_seconds;
    let next = current + cadence_seconds;
    Ok((current, next))
}

/// Format the runtime-contract updown market slug:
///   `"{underlying_asset_lowercase}-updown-{cadence_slug_token}-{period_start_unix_seconds}"`.
pub fn updown_market_slug(
    asset: &str,
    cadence_slug_token: &str,
    period_start_unix_seconds: i64,
) -> String {
    format!(
        "{asset_lower}-updown-{cadence_slug_token}-{period_start_unix_seconds}",
        asset_lower = asset.to_ascii_lowercase()
    )
}

/// Produce the current and next updown market-slug candidates for a
/// single `UpdownTargetPlan` evaluated at `now_unix_seconds`.
pub fn candidates_for_target(
    target_plan: &UpdownTargetPlan,
    now_unix_seconds: i64,
) -> Result<UpdownSlugCandidates, BoltV3MarketIdentityError> {
    let (current_start, next_start) =
        updown_period_pair(target_plan.cadence_seconds, now_unix_seconds)?;
    let current_market_slug = updown_market_slug(
        &target_plan.underlying_asset,
        &target_plan.cadence_slug_token,
        current_start,
    );
    let next_market_slug = updown_market_slug(
        &target_plan.underlying_asset,
        &target_plan.cadence_slug_token,
        next_start,
    );
    Ok(UpdownSlugCandidates {
        current_period_start_unix_seconds: current_start,
        next_period_start_unix_seconds: next_start,
        current_market_slug,
        next_market_slug,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updown_period_pair_floor_examples() {
        assert_eq!(updown_period_pair(60, 119).unwrap(), (60, 120));
        assert_eq!(updown_period_pair(60, 120).unwrap(), (120, 180));
        assert_eq!(updown_period_pair(60, 121).unwrap(), (120, 180));
        assert_eq!(updown_period_pair(3600, 7199).unwrap(), (3600, 7200));
        assert_eq!(updown_period_pair(3600, 7200).unwrap(), (7200, 10800));
    }

    #[test]
    fn updown_market_slug_examples() {
        assert_eq!(
            updown_market_slug("BTC", "5m", 1_700_000_000),
            "btc-updown-5m-1700000000"
        );
        assert_eq!(
            updown_market_slug("ETH", "1h", 1_700_003_600),
            "eth-updown-1h-1700003600"
        );
    }

    #[test]
    fn updown_period_pair_rejects_zero_cadence() {
        assert!(matches!(
            updown_period_pair(0, 600),
            Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
                strategy_instance_id: None,
                cadence_seconds: 0,
            })
        ));
    }
}
