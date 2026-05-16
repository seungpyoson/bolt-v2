//! Updown `target.rotating_market_family` support.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//! (slug derivation rule lives in Section 5.3)
//!
//! This module reads validated bolt-v3 strategy config and computes the
//! current/next updown market slugs used by the NautilusTrader
//! Polymarket `MarketSlugFilter`. It registers nothing and opens no
//! client.
//!
//! The cadence duration and slug token are TOML-owned target fields.
//! Core startup validation (`crate::bolt_v3_validate`) dispatches
//! updown-specific target validation here, so validation and
//! instrument-filter construction share the same typed target fields.
//!
//! Provider-specific `MarketSlugFilter` construction lives in
//! `bolt_v3_providers::polymarket`.
//!
//! Out of scope for this module: provider price extraction,
//! reference data, and order construction.

use std::time::Duration;

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_instrument_filters::{InstrumentFilterError, InstrumentFilterTarget},
    bolt_v3_market_families::{
        MarketFamilyValidationBinding, MarketSelectionTarget, SelectedBinaryOptionMarket,
        TargetRuntimeFields,
    },
};

pub const KEY: &str = "updown";

pub fn validation_binding() -> MarketFamilyValidationBinding {
    MarketFamilyValidationBinding {
        key: KEY,
        validate_target: validate_target_block,
        instrument_filter_targets,
        target_runtime_fields,
        select_binary_option_market,
    }
}

/// Updown rotating-cadence target block. Owned by the updown market-
/// family binding because `cadence_seconds`, `cadence_slug_token`,
/// `underlying_asset`,
/// `rotating_market_family`, and `market_selection_rule` are family-
/// shaped fields. The strategy envelope (`crate::bolt_v3_config::
/// BoltV3StrategyConfig`) keeps the TOML field name `[target]` as a
/// generic `toml::Value`; the updown family deserializes that raw
/// envelope into this typed struct during validation and
/// instrument-filter construction.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_slug_token: String,
    pub market_selection_rule: MarketSelectionRule,
    pub retry_interval_seconds: u64,
    pub blocked_after_seconds: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    RotatingMarket,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RotatingMarketFamily {
    Updown,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketSelectionRule {
    ActiveOrNext,
}

/// Typed deserializer for the strategy envelope's raw `[target]` block.
/// Wraps `toml::de::Error` so callers can embed the message into
/// validation and instrument-filter errors without pulling the toml
/// crate's error type into their public API. This
/// is the single place the updown family's `deny_unknown_fields`
/// strictness fires after the strategy envelope was relaxed to raw
/// TOML.
pub fn deserialize_target_block(target: &toml::Value) -> Result<TargetBlock, String> {
    target
        .clone()
        .try_into::<TargetBlock>()
        .map_err(|error| error.to_string())
}

/// Family-specific structural validator for updown rotating-market
/// targets. Owns underlying-asset shape rules, cadence rules (via
/// `validate_target_cadence`), and the retry / blocked positive-
/// integer rules. Core startup validation in `crate::bolt_v3_validate`
/// dispatches the strategy envelope's raw `[target]` value here via
/// `crate::bolt_v3_market_families::validate_strategy_target`. The
/// `deny_unknown_fields` strictness for the rotating-market target
/// shape fires here at typed-deserialization time, since the strategy
/// envelope has been relaxed to raw `toml::Value`.
pub fn validate_target_block(context: &str, target: &toml::Value) -> Vec<String> {
    let block = match deserialize_target_block(target) {
        Ok(value) => value,
        Err(message) => return vec![format!("{context}: target: {message}")],
    };

    let mut errors = Vec::new();

    let underlying = block.underlying_asset.as_str();
    if underlying.is_empty() {
        errors.push(format!(
            "{context}: target.underlying_asset must not be empty"
        ));
    } else if !underlying
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push(format!(
            "{context}: target.underlying_asset must use only uppercase ASCII letters, digits, and underscores (got `{underlying}`)"
        ));
    }

    errors.extend(validate_target_cadence(context, block.cadence_seconds));
    validate_slug_token(
        context,
        "target.cadence_slug_token",
        block.cadence_slug_token.as_str(),
        &mut errors,
    );

    if block.retry_interval_seconds == 0 {
        errors.push(format!(
            "{context}: target.retry_interval_seconds must be a positive integer"
        ));
    }
    if block.blocked_after_seconds == 0 {
        errors.push(format!(
            "{context}: target.blocked_after_seconds must be a positive integer"
        ));
    }

    // Reading `block.market_selection_rule` and `block.kind` here is a
    // no-op exhaustive match: the only allowed variants are encoded by
    // the typed enums above, so any TOML value other than
    // `active_or_next` / `rotating_market` was already rejected by
    // typed deserialization.
    let MarketSelectionRule::ActiveOrNext = block.market_selection_rule;
    let TargetKind::RotatingMarket = block.kind;
    let RotatingMarketFamily::Updown = block.rotating_market_family;

    errors
}

/// Family-specific cadence validator for updown rotating-market targets.
/// The cadence duration is TOML-owned. The slug token is validated
/// separately as `target.cadence_slug_token`, so this check only rejects
/// non-positive durations.
pub fn validate_target_cadence(context: &str, cadence_seconds: i64) -> Vec<String> {
    let mut errors = Vec::new();
    if cadence_seconds <= 0 {
        errors.push(format!(
            "{context}: target.cadence_seconds must be a positive integer (got {cadence_seconds})"
        ));
    }
    errors
}

fn validate_slug_token(context: &str, field: &str, value: &str, errors: &mut Vec<String>) {
    if value.is_empty() {
        errors.push(format!("{context}: {field} must not be empty"));
    } else if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        errors.push(format!(
            "{context}: {field} must use only lowercase ASCII letters and digits (got `{value}`)"
        ));
    }
}

/// Pure projection of all updown rotating-market targets in a
/// validated bolt-v3 configuration. One `UpdownInstrumentFilterTarget` per
/// configured strategy whose target maps to the updown family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownInstrumentFilterConfig {
    pub updown_targets: Vec<UpdownInstrumentFilterTarget>,
}

/// Config facts for one configured updown rotating-market target.
/// Every value here is derived from validated configuration; nothing
/// here depends on wall-clock time, the NT instrument index, or any
/// network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownInstrumentFilterTarget {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub venue: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_slug_token: String,
}

/// Current and next updown market-slug candidates for a single
/// `UpdownInstrumentFilterTarget` evaluated against an injected `now_unix_seconds`
/// value (intended to come from the NautilusTrader node clock at the
/// caller, not from this module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownSlugCandidates {
    pub current_period_start_unix_seconds: i64,
    pub next_period_start_unix_seconds: i64,
    pub current_market_slug: String,
    pub next_market_slug: String,
}

/// Strategy-facing target facts needed to select an updown market from
/// NautilusTrader-loaded instruments. Values come from TOML plus the
/// NautilusTrader node clock supplied by caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdownSelectionTarget<'a> {
    pub underlying_asset: &'a str,
    pub cadence_seconds: i64,
    pub cadence_slug_token: &'a str,
}

/// Selected updown market from NautilusTrader `BinaryOption`
/// instruments. This module owns the NT metadata interpretation so
/// strategy code consumes typed up/down instrument facts instead of
/// reading product metadata keys in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedUpdownMarket {
    pub market_id: String,
    pub instrument_id: InstrumentId,
    pub up_instrument_id: InstrumentId,
    pub down_instrument_id: InstrumentId,
    pub start_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdownOutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone)]
struct UpdownOutcomeInstrument {
    side: UpdownOutcomeSide,
    market_id: String,
    instrument_id: InstrumentId,
    activation_milliseconds: u64,
    expiration_milliseconds: u64,
}

#[derive(Debug)]
struct UpdownOutcomePair {
    up: Option<UpdownOutcomeInstrument>,
    down: Option<UpdownOutcomeInstrument>,
}

impl UpdownOutcomePair {
    fn empty() -> Self {
        Self {
            up: None,
            down: None,
        }
    }
}

#[derive(Debug)]
pub enum BoltV3InstrumentFilterError {
    NonPositiveCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_seconds: i64,
    },
    NegativeNowUnixSeconds {
        now_unix_seconds: i64,
    },
    PeriodPairOverflow {
        now_unix_seconds: i64,
        cadence_seconds: i64,
    },
    /// The strategy envelope's raw `[target]` value failed updown typed
    /// deserialization after validation. Validation runs the same
    /// typed deserialization, so reaching this branch means the
    /// `target` value was mutated after validation, or
    /// a programmatic caller bypassed the validator. The error wraps
    /// the original toml-deserialization message so the operator sees
    /// the exact field that failed.
    TargetParseFailed {
        strategy_instance_id: String,
        message: String,
    },
}

impl std::fmt::Display for BoltV3InstrumentFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => write!(
                f,
                "{prefix}target.cadence_seconds must be a positive integer (got {cadence_seconds})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3InstrumentFilterError::NegativeNowUnixSeconds { now_unix_seconds } => {
                write!(
                    f,
                    "now_unix_seconds must be non-negative (got {now_unix_seconds})"
                )
            }
            BoltV3InstrumentFilterError::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => write!(
                f,
                "updown period pair overflows i64 (now_unix_seconds={now_unix_seconds}, cadence_seconds={cadence_seconds})"
            ),
            BoltV3InstrumentFilterError::TargetParseFailed {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategy `{strategy_instance_id}`: target failed updown typed deserialization after validation: {message}"
            ),
        }
    }
}

fn format_target_prefix(
    strategy_instance_id: &Option<String>,
    configured_target_id: &Option<String>,
) -> String {
    match (strategy_instance_id, configured_target_id) {
        (Some(strategy), Some(target)) => format!("strategy `{strategy}` target `{target}`: "),
        (Some(strategy), None) => format!("strategy `{strategy}`: "),
        (None, Some(target)) => format!("target `{target}`: "),
        (None, None) => String::new(),
    }
}

impl std::error::Error for BoltV3InstrumentFilterError {}

/// Project every configured strategy in `loaded` into an
/// `UpdownInstrumentFilterTarget`. Returns the full `InstrumentFilterConfig` in the
/// same sequence as the configured strategies. Fails loud if a
/// strategy's target has been mutated to bypass schema validation
/// (non-positive `cadence_seconds`).
pub fn instrument_filters_from_config(
    loaded: &LoadedBoltV3Config,
) -> Result<UpdownInstrumentFilterConfig, BoltV3InstrumentFilterError> {
    let mut updown_targets = Vec::with_capacity(loaded.strategies.len());
    for strategy in &loaded.strategies {
        if let Some(target) = instrument_filter_target_from_strategy(strategy)? {
            updown_targets.push(target);
        }
    }
    Ok(UpdownInstrumentFilterConfig { updown_targets })
}

pub fn instrument_filter_targets(
    loaded: &LoadedBoltV3Config,
) -> Result<Vec<InstrumentFilterTarget>, InstrumentFilterError> {
    let updown_filters =
        instrument_filters_from_config(loaded).map_err(InstrumentFilterError::from)?;
    Ok(updown_filters
        .updown_targets
        .iter()
        .map(|target| InstrumentFilterTarget {
            strategy_instance_id: target.strategy_instance_id.clone(),
            family_key: KEY,
            configured_target_id: target.configured_target_id.clone(),
            venue: target.venue.clone(),
            underlying_asset: target.underlying_asset.clone(),
            cadence_seconds: target.cadence_seconds,
            cadence_slug_token: target.cadence_slug_token.clone(),
        })
        .collect())
}

pub fn target_runtime_fields(target: &toml::Value) -> Result<TargetRuntimeFields, String> {
    let target = deserialize_target_block(target)?;
    Ok(TargetRuntimeFields {
        configured_target_id: target.configured_target_id,
        target_kind: target_runtime_string(target.kind),
        rotating_market_family: target_runtime_string(target.rotating_market_family),
        underlying_asset: target.underlying_asset,
        cadence_seconds: target.cadence_seconds,
        cadence_seconds_source_field: "target.cadence_seconds",
        cadence_slug_token: target.cadence_slug_token,
        market_selection_rule: target_runtime_string(target.market_selection_rule),
        retry_interval_seconds: target.retry_interval_seconds,
        blocked_after_seconds: target.blocked_after_seconds,
    })
}

fn target_runtime_string<T>(value: T) -> String
where
    T: serde::Serialize,
{
    toml::Value::try_from(value)
        .expect("validated updown target enum should serialize")
        .as_str()
        .expect("validated updown target enum should serialize to string")
        .to_string()
}

fn instrument_filter_target_from_strategy(
    strategy: &LoadedStrategy,
) -> Result<Option<UpdownInstrumentFilterTarget>, BoltV3InstrumentFilterError> {
    let strategy_instance_id = strategy.config.strategy_instance_id.clone();
    let venue = strategy.config.venue.clone();
    let target: TargetBlock =
        deserialize_target_block(&strategy.config.target).map_err(|message| {
            BoltV3InstrumentFilterError::TargetParseFailed {
                strategy_instance_id: strategy_instance_id.clone(),
                message,
            }
        })?;

    // Exhaustive matches: when a future variant is added to either
    // enum the build breaks here, forcing a deliberate decision about
    // how the new variant is projected into instrument filter.
    let TargetKind::RotatingMarket = target.kind;
    let RotatingMarketFamily::Updown = target.rotating_market_family;

    let configured_target_id = target.configured_target_id.clone();
    if target.cadence_seconds <= 0 {
        return Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_seconds: target.cadence_seconds,
        });
    }

    Ok(Some(UpdownInstrumentFilterTarget {
        strategy_instance_id,
        configured_target_id,
        venue,
        underlying_asset: target.underlying_asset.clone(),
        cadence_seconds: target.cadence_seconds,
        cadence_slug_token: target.cadence_slug_token.clone(),
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
) -> Result<(i64, i64), BoltV3InstrumentFilterError> {
    if cadence_seconds <= 0 {
        return Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id: None,
            configured_target_id: None,
            cadence_seconds,
        });
    }
    if now_unix_seconds < 0 {
        return Err(BoltV3InstrumentFilterError::NegativeNowUnixSeconds { now_unix_seconds });
    }
    let current = (now_unix_seconds / cadence_seconds) * cadence_seconds;
    let next = current.checked_add(cadence_seconds).ok_or(
        BoltV3InstrumentFilterError::PeriodPairOverflow {
            now_unix_seconds,
            cadence_seconds,
        },
    )?;
    Ok((current, next))
}

/// Format the runtime-contract updown market slug:
///   `"{underlying_asset_lowercase}-{family_key}-{cadence_slug_token}-{period_start_unix_seconds}"`.
pub fn updown_market_slug(
    asset: &str,
    cadence_slug_token: &str,
    period_start_unix_seconds: i64,
) -> String {
    format!(
        "{asset_lower}-{family_key}-{cadence_slug_token}-{period_start_unix_seconds}",
        asset_lower = asset.to_ascii_lowercase(),
        family_key = KEY
    )
}

/// Produce the current and next updown market-slug candidates for a
/// single `UpdownInstrumentFilterTarget` evaluated at `now_unix_seconds`.
pub fn candidates_for_target(
    target: &UpdownInstrumentFilterTarget,
    now_unix_seconds: i64,
) -> Result<UpdownSlugCandidates, BoltV3InstrumentFilterError> {
    let (current_start, next_start) = updown_period_pair(target.cadence_seconds, now_unix_seconds)?;
    let current_market_slug = updown_market_slug(
        &target.underlying_asset,
        &target.cadence_slug_token,
        current_start,
    );
    let next_market_slug = updown_market_slug(
        &target.underlying_asset,
        &target.cadence_slug_token,
        next_start,
    );
    Ok(UpdownSlugCandidates {
        current_period_start_unix_seconds: current_start,
        next_period_start_unix_seconds: next_start,
        current_market_slug,
        next_market_slug,
    })
}

pub fn select_market_from_instruments(
    target: UpdownSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedUpdownMarket> {
    let now_unix_seconds = i64::try_from(Duration::from_millis(now_milliseconds).as_secs()).ok()?;
    let (current_start, next_start) =
        updown_period_pair(target.cadence_seconds, now_unix_seconds).ok()?;
    let current_slug = updown_market_slug(
        target.underlying_asset,
        target.cadence_slug_token,
        current_start,
    );
    let next_slug = updown_market_slug(
        target.underlying_asset,
        target.cadence_slug_token,
        next_start,
    );

    candidate_market_for_slug(instruments, &current_slug, current_start, now_milliseconds).or_else(
        || candidate_market_for_slug(instruments, &next_slug, next_start, now_milliseconds),
    )
}

pub fn select_binary_option_market(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    let market = select_market_from_instruments(
        UpdownSelectionTarget {
            underlying_asset: target.underlying_asset,
            cadence_seconds: target.cadence_seconds,
            cadence_slug_token: target.cadence_slug_token,
        },
        instruments,
        now_milliseconds,
    )?;
    Some(SelectedBinaryOptionMarket {
        market_id: market.market_id,
        instrument_id: market.instrument_id,
        up_instrument_id: market.up_instrument_id,
        down_instrument_id: market.down_instrument_id,
        start_timestamp_milliseconds: market.start_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
    })
}

fn candidate_market_for_slug(
    instruments: &[InstrumentAny],
    market_slug: &str,
    period_start_unix_seconds: i64,
    now_milliseconds: u64,
) -> Option<SelectedUpdownMarket> {
    let mut pair = UpdownOutcomePair::empty();
    for instrument in instruments {
        let Some(outcome) = updown_outcome_instrument(instrument, market_slug) else {
            continue;
        };
        match outcome.side {
            UpdownOutcomeSide::Up if pair.up.is_none() => pair.up = Some(outcome),
            UpdownOutcomeSide::Down if pair.down.is_none() => pair.down = Some(outcome),
            _ => return None,
        }
    }

    let up = pair.up?;
    let down = pair.down?;
    if up.market_id != down.market_id {
        return None;
    }

    let expiration_milliseconds = up.expiration_milliseconds.min(down.expiration_milliseconds);
    if expiration_milliseconds <= now_milliseconds {
        return None;
    }

    let start_timestamp_milliseconds =
        if up.activation_milliseconds == 0 || down.activation_milliseconds == 0 {
            u64::try_from(
                Duration::from_secs(u64::try_from(period_start_unix_seconds).ok()?).as_millis(),
            )
            .ok()?
        } else {
            up.activation_milliseconds.min(down.activation_milliseconds)
        };

    Some(SelectedUpdownMarket {
        market_id: up.market_id,
        instrument_id: up.instrument_id,
        up_instrument_id: up.instrument_id,
        down_instrument_id: down.instrument_id,
        start_timestamp_milliseconds,
        seconds_to_end: Duration::from_millis(
            expiration_milliseconds.saturating_sub(now_milliseconds),
        )
        .as_secs(),
    })
}

fn updown_outcome_instrument(
    instrument: &InstrumentAny,
    expected_market_slug: &str,
) -> Option<UpdownOutcomeInstrument> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    let info = binary.info.as_ref()?;
    if info.get_str("market_slug")? != expected_market_slug {
        return None;
    }
    let side = match binary.outcome.as_ref()?.as_str() {
        "Up" => UpdownOutcomeSide::Up,
        "Down" => UpdownOutcomeSide::Down,
        _ => return None,
    };
    Some(UpdownOutcomeInstrument {
        side,
        market_id: info.get_str("market_id")?.to_string(),
        instrument_id: binary.id,
        activation_milliseconds: u64::try_from(
            Duration::from_nanos(binary.activation_ns.as_u64()).as_millis(),
        )
        .ok()?,
        expiration_milliseconds: u64::try_from(
            Duration::from_nanos(binary.expiration_ns.as_u64()).as_millis(),
        )
        .ok()?,
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
            Err(BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
                strategy_instance_id: None,
                configured_target_id: None,
                cadence_seconds: 0,
            })
        ));
    }
}
