//! Updown rotating-cadence market-family identity binding for bolt-v3.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//! (slug derivation rule lives in Section 5.3)
//!
//! This module owns the updown family's market-identity surface as a
//! pure data boundary. It projects a validated bolt-v3 configuration
//! plus an injected `now_unix_secs` value into a
//! `MarketIdentityPlan` plus current/next updown market slug
//! candidates. It registers nothing, opens no client, mutates no
//! shared instrument index, depends on no live wall-clock source, and
//! describes no provider-specific discovery mechanism.
//!
//! The runtime-contract slug-token table (`docs/bolt-v3/2026-04-25-
//! bolt-v3-runtime-contracts.md` Section 5.3) is owned by this
//! module. Core startup validation (`crate::bolt_v3_validate`) keeps
//! its target-shape checks structural and dispatches updown-specific
//! cadence rules to `validate_target_cadence` here, so the schema
//! validator and this market-family planner share one source of
//! truth for supported `cadence_secs` values.
//!
//! This module is family-specific by design: it lives under
//! `bolt_v3_market_families::updown` so the family-agnostic core
//! (`crate::bolt_v3_market_identity`) can stay neutral. Translation
//! of the neutral identity plan into provider-shaped adapter values
//! still lives in the adapter / provider-binding layer
//! (`bolt_v3_adapters`); a per-provider companion source-guard test
//! enforces that no provider-specific filter type leaks into this
//! family-binding module.
//!
//! Out of scope for this module: live runtime workflows, dynamic
//! instrument discovery, provider price extraction, fused reference
//! price derivation, and trade-action construction. Those boundaries
//! belong to later slices.

use std::time::Duration;

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_instrument_filters::{InstrumentFilterError, InstrumentFilterTarget},
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketFamilyValidationBinding, MarketIdentityPlan,
        MarketIdentityTarget, MarketSelectionCandidateWindow, MarketSelectionOutcome,
        MarketSelectionTarget, SelectedBinaryOptionMarket, SelectedMarketSourceIdentity,
        TargetRuntimeFields,
    },
    bolt_v3_numeric::{
        POWER_OF_TWO, SECONDS_PER_YEAR_F64, UNIT_F64, ZERO_F64, is_positive_finite,
        sanitize_probability,
    },
};

pub const KEY: &str = "updown";

pub fn validation_binding() -> MarketFamilyValidationBinding {
    MarketFamilyValidationBinding {
        key: KEY,
        validate_target: validate_target_block,
        instrument_filter_target_for_strategy,
        target_runtime_fields,
        select_binary_option_market,
        market_selection_candidate_windows,
        fair_probability_up,
    }
}

/// Updown rotating-cadence target block. Owned by the updown market-
/// family binding because `cadence_secs`, `underlying_asset`,
/// `rotating_market_family`, and `market_selection_rule` are family-
/// shaped fields. The strategy envelope (`crate::bolt_v3_config::
/// BoltV3StrategyConfig`) keeps the TOML field name `[target]` as a
/// generic `toml::Value`; the updown family deserializes that raw
/// envelope into this typed shape during validation and planning.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub underlying_asset: String,
    pub cadence_secs: i64,
    pub market_selection_rule: MarketSelectionRule,
    pub retry_interval_secs: u64,
    pub blocked_after_secs: u64,
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
/// Wraps `toml::de::Error` into a stringly-typed surface so callers can
/// embed the message into validation / planning error reports without
/// pulling the toml crate's error type into their public surface. This
/// is the single place the updown family's `deny_unknown_fields`
/// strictness fires after the strategy envelope was relaxed to raw
/// TOML.
pub fn deserialize_target_block(target: &toml::Value) -> Result<TargetBlock, String> {
    target
        .clone()
        .try_into::<TargetBlock>()
        .map_err(|error| error.to_string())
}

/// Runtime-contract `cadence_secs -> slug-token` table for the
/// updown market family. Authoritative reference:
/// `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` Section 5.3.
const UPDOWN_CADENCE_SLUG_TOKEN_TABLE: &[(i64, &str)] = &[
    (60, "1m"),
    (300, "5m"),
    (900, "15m"),
    (3600, "1h"),
    (14400, "4h"),
];

/// Look up the runtime-contract slug-token for a configured updown
/// `cadence_secs`, or `None` if the value is not in the table.
pub fn updown_cadence_slug_token(cadence_secs: i64) -> Option<&'static str> {
    UPDOWN_CADENCE_SLUG_TOKEN_TABLE
        .iter()
        .find_map(|(seconds, token)| (*seconds == cadence_secs).then_some(*token))
}

/// Enumerate the `cadence_secs` values currently supported by the
/// runtime-contract slug-token table, in declaration order. Used in
/// startup-validation error messages so the operator sees the exact
/// allowed set.
pub fn supported_updown_cadence_secs() -> Vec<i64> {
    UPDOWN_CADENCE_SLUG_TOKEN_TABLE
        .iter()
        .map(|(seconds, _)| *seconds)
        .collect()
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
    } else if underlying.chars().count() > 32 {
        errors.push(format!(
            "{context}: target.underlying_asset must be 1-32 characters (got {})",
            underlying.chars().count()
        ));
    } else if !underlying
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push(format!(
            "{context}: target.underlying_asset must use only uppercase ASCII letters, digits, and underscores (got `{underlying}`)"
        ));
    }

    errors.extend(validate_target_cadence(context, block.cadence_secs));

    if block.retry_interval_secs == 0 {
        errors.push(format!(
            "{context}: target.retry_interval_secs must be a positive integer"
        ));
    }
    if block.blocked_after_secs == 0 {
        errors.push(format!(
            "{context}: target.blocked_after_secs must be a positive integer"
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

/// Family-specific cadence validator for updown rotating-market
/// targets. Owns the positive / minute-aligned / table-membership
/// rules so core startup validation can stay structural and dispatch
/// per-family cadence policy here.
pub fn validate_target_cadence(context: &str, cadence_secs: i64) -> Vec<String> {
    let mut errors = Vec::new();
    if cadence_secs <= 0 {
        errors.push(format!(
            "{context}: target.cadence_secs must be a positive integer (got {cadence_secs})"
        ));
    } else if cadence_secs % 60 != 0 {
        errors.push(format!(
            "{context}: target.cadence_secs must be divisible by 60 (got {cadence_secs})"
        ));
    } else if updown_cadence_slug_token(cadence_secs).is_none() {
        let supported = supported_updown_cadence_secs();
        errors.push(format!(
            "{context}: target.cadence_secs={cadence_secs} has no runtime-contract-defined updown slug-token mapping; supported values are {supported:?}"
        ));
    }
    errors
}

/// Pure identity facts for one configured updown rotating-market
/// target. Every value here is derived from validated configuration
/// and the runtime-contract slug-token table; nothing here depends on
/// wall-clock time, the NT instrument index, or any network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub execution_client_id: String,
    pub underlying_asset: String,
    pub cadence_secs: i64,
    pub cadence_slug_token: String,
}

impl MarketIdentityTarget for UpdownTargetPlan {
    fn family_key(&self) -> &'static str {
        KEY
    }

    fn configured_target_id(&self) -> &str {
        &self.configured_target_id
    }

    fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn target_plans(plan: &MarketIdentityPlan) -> impl Iterator<Item = &UpdownTargetPlan> {
    plan.targets()
        .filter_map(|target| target.as_any().downcast_ref::<UpdownTargetPlan>())
}

/// Current and next updown market-slug candidates for a single
/// `UpdownTargetPlan` evaluated against an injected `now_unix_secs`
/// value (intended to come from the NautilusTrader node clock at the
/// caller, not from this module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownSlugCandidates {
    pub current_period_start_unix_secs: i64,
    pub next_period_start_unix_secs: i64,
    pub current_market_slug: String,
    pub next_market_slug: String,
}

/// Strategy-facing target facts needed to select an updown market from
/// NautilusTrader-loaded instruments. Values come from TOML plus the
/// NautilusTrader node clock supplied by caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdownSelectionTarget<'a> {
    pub underlying_asset: &'a str,
    pub cadence_secs: i64,
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
    pub selection_outcome: MarketSelectionOutcome,
    pub start_timestamp_milliseconds: u64,
    pub expiration_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
    pub source_identity: SelectedMarketSourceIdentity,
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
    condition_id: String,
    market_slug: String,
    question_id: String,
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
    TargetParseFailed {
        strategy_instance_id: String,
        message: String,
    },
}

impl std::fmt::Display for BoltV3InstrumentFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => write!(
                f,
                "{prefix}target.cadence_seconds must be a positive integer (got {cadence_seconds})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            Self::NegativeNowUnixSeconds { now_unix_seconds } => write!(
                f,
                "now_unix_seconds must be non-negative (got {now_unix_seconds})"
            ),
            Self::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => write!(
                f,
                "updown period pair overflows i64 (now_unix_seconds={now_unix_seconds}, cadence_seconds={cadence_seconds})"
            ),
            Self::TargetParseFailed {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategy `{strategy_instance_id}`: target failed updown typed deserialization after validation: {message}"
            ),
        }
    }
}

impl std::error::Error for BoltV3InstrumentFilterError {}

#[derive(Debug)]
pub enum BoltV3MarketIdentityError {
    NonPositiveCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_secs: i64,
    },
    UnsupportedCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_secs: i64,
    },
    NegativeNowUnixSeconds {
        now_unix_secs: i64,
    },
    PeriodPairOverflow {
        now_unix_secs: i64,
        cadence_secs: i64,
    },
    /// The strategy envelope's raw `[target]` value failed updown typed
    /// deserialization at planning time. Validation runs the same
    /// typed deserialization, so reaching this branch means the
    /// `target` value was mutated between validation and planning, or
    /// a programmatic caller bypassed the validator. The error wraps
    /// the original toml-deserialization message so the operator sees
    /// the exact field that failed.
    TargetParseFailed {
        strategy_instance_id: String,
        message: String,
    },
}

impl std::fmt::Display for BoltV3MarketIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_secs,
            } => write!(
                f,
                "{prefix}target.cadence_secs must be a positive integer (got {cadence_secs})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_secs,
            } => write!(
                f,
                "{prefix}target.cadence_secs={cadence_secs} has no runtime-contract-defined updown slug-token mapping",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_secs } => {
                write!(
                    f,
                    "now_unix_secs must be non-negative (got {now_unix_secs})"
                )
            }
            BoltV3MarketIdentityError::PeriodPairOverflow {
                now_unix_secs,
                cadence_secs,
            } => write!(
                f,
                "updown period pair overflows i64 (now_unix_secs={now_unix_secs}, cadence_secs={cadence_secs})"
            ),
            BoltV3MarketIdentityError::TargetParseFailed {
                strategy_instance_id,
                message,
            } => write!(
                f,
                "strategy `{strategy_instance_id}`: target failed updown typed deserialization at planning time: {message}"
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

impl std::error::Error for BoltV3MarketIdentityError {}

/// Project every configured strategy in `loaded` into an
/// `UpdownTargetPlan`. Returns the full `MarketIdentityPlan` in the
/// same sequence as the configured strategies. Fails loud if a
/// strategy's target has been mutated to bypass schema validation
/// (non-positive or unsupported `cadence_secs`).
pub fn plan_market_identity(
    loaded: &LoadedBoltV3Config,
) -> Result<MarketIdentityPlan, BoltV3MarketIdentityError> {
    let mut plan = MarketIdentityPlan::empty();
    for strategy in &loaded.strategies {
        if let Some(target) = plan_strategy_updown_target(strategy)? {
            plan.push_target(target);
        }
    }
    Ok(plan)
}

fn plan_strategy_updown_target(
    strategy: &LoadedStrategy,
) -> Result<Option<UpdownTargetPlan>, BoltV3MarketIdentityError> {
    let strategy_instance_id = strategy.config.strategy_instance_id.clone();
    let execution_client_id = strategy.config.execution_client_id.to_string();
    let target: TargetBlock =
        deserialize_target_block(&strategy.config.target).map_err(|message| {
            BoltV3MarketIdentityError::TargetParseFailed {
                strategy_instance_id: strategy_instance_id.clone(),
                message,
            }
        })?;

    // Exhaustive matches: when a future variant is added to either
    // enum the build breaks here, forcing a deliberate decision about
    // how the new variant is projected into market identity.
    let TargetKind::RotatingMarket = target.kind;
    let RotatingMarketFamily::Updown = target.rotating_market_family;

    let configured_target_id = target.configured_target_id.clone();
    if target.cadence_secs <= 0 {
        return Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_secs: target.cadence_secs,
        });
    }
    let token = match updown_cadence_slug_token(target.cadence_secs) {
        Some(token) => token,
        None => {
            return Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id: Some(strategy_instance_id),
                configured_target_id: Some(configured_target_id),
                cadence_secs: target.cadence_secs,
            });
        }
    };

    Ok(Some(UpdownTargetPlan {
        strategy_instance_id,
        configured_target_id,
        execution_client_id,
        underlying_asset: target.underlying_asset.clone(),
        cadence_secs: target.cadence_secs,
        cadence_slug_token: token.to_string(),
    }))
}

/// Compute the current and next updown period start values from
/// `cadence_secs` and `now_unix_secs`, following the runtime
/// contract:
///   `current = floor(now / cadence) * cadence`
///   `next = current + cadence`
pub fn updown_period_pair(
    cadence_secs: i64,
    now_unix_secs: i64,
) -> Result<(i64, i64), BoltV3MarketIdentityError> {
    if cadence_secs <= 0 {
        return Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id: None,
            configured_target_id: None,
            cadence_secs,
        });
    }
    if now_unix_secs < 0 {
        return Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_secs });
    }
    let current = (now_unix_secs / cadence_secs) * cadence_secs;
    let next =
        current
            .checked_add(cadence_secs)
            .ok_or(BoltV3MarketIdentityError::PeriodPairOverflow {
                now_unix_secs,
                cadence_secs,
            })?;
    Ok((current, next))
}

/// Format the runtime-contract updown market slug:
///   `"{underlying_asset_lowercase}-updown-{cadence_slug_token}-{period_start_unix_secs}"`.
pub fn updown_market_slug(
    asset: &str,
    cadence_slug_token: &str,
    period_start_unix_secs: i64,
) -> String {
    format!(
        "{asset_lower}-updown-{cadence_slug_token}-{period_start_unix_secs}",
        asset_lower = asset.to_ascii_lowercase()
    )
}

/// Produce the current and next updown market-slug candidates for a
/// single `UpdownTargetPlan` evaluated at `now_unix_secs`.
pub fn candidates_for_target(
    target_plan: &UpdownTargetPlan,
    now_unix_secs: i64,
) -> Result<UpdownSlugCandidates, BoltV3MarketIdentityError> {
    let (current_start, next_start) = updown_period_pair(target_plan.cadence_secs, now_unix_secs)?;
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
        current_period_start_unix_secs: current_start,
        next_period_start_unix_secs: next_start,
        current_market_slug,
        next_market_slug,
    })
}

pub fn instrument_filter_target_for_strategy(
    strategy: &LoadedStrategy,
) -> Result<Option<InstrumentFilterTarget>, InstrumentFilterError> {
    let target =
        instrument_filter_target_from_strategy(strategy).map_err(instrument_filter_error)?;
    Ok(target.map(|target| InstrumentFilterTarget {
        strategy_instance_id: target.strategy_instance_id,
        family_key: KEY,
        configured_target_id: target.configured_target_id,
        execution_client_id: target.execution_client_id,
        underlying_asset: target.underlying_asset,
        cadence_seconds: target.cadence_secs,
        cadence_slug_token: target.cadence_slug_token,
    }))
}

fn instrument_filter_target_from_strategy(
    strategy: &LoadedStrategy,
) -> Result<Option<UpdownTargetPlan>, BoltV3MarketIdentityError> {
    plan_strategy_updown_target(strategy)
}

fn instrument_filter_error(error: BoltV3MarketIdentityError) -> InstrumentFilterError {
    match error {
        BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_secs,
        } => InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds: cadence_secs,
        },
        BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_secs } => {
            InstrumentFilterError::NegativeNowUnixSeconds {
                now_unix_seconds: now_unix_secs,
            }
        }
        BoltV3MarketIdentityError::PeriodPairOverflow {
            now_unix_secs,
            cadence_secs,
        } => InstrumentFilterError::PeriodPairOverflow {
            now_unix_seconds: now_unix_secs,
            cadence_seconds: cadence_secs,
        },
        BoltV3MarketIdentityError::TargetParseFailed {
            strategy_instance_id,
            message,
        } => InstrumentFilterError::TargetParseFailed {
            strategy_instance_id,
            message,
        },
        BoltV3MarketIdentityError::UnsupportedCadenceSeconds { .. } => {
            InstrumentFilterError::TargetValidationFailure {
                message: error.to_string(),
            }
        }
    }
}

pub fn target_runtime_fields(
    target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    let target = deserialize_target_block(target)
        .map_err(|message| InstrumentFilterError::Other { message })?;
    let cadence_slug_token = updown_cadence_slug_token(target.cadence_secs).ok_or_else(|| {
        InstrumentFilterError::TargetValidationFailure {
            message: BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id: None,
                configured_target_id: Some(target.configured_target_id.clone()),
                cadence_secs: target.cadence_secs,
            }
            .to_string(),
        }
    })?;
    Ok(TargetRuntimeFields {
        configured_target_id: target.configured_target_id,
        target_kind: target_runtime_string(target.kind),
        rotating_market_family: target_runtime_string(target.rotating_market_family),
        underlying_asset: target.underlying_asset,
        cadence_seconds: target.cadence_secs,
        cadence_seconds_source_field: "target.cadence_secs",
        cadence_slug_token: cadence_slug_token.to_string(),
        market_selection_rule: target_runtime_string(target.market_selection_rule),
        retry_interval_seconds: target.retry_interval_secs,
        blocked_after_seconds: target.blocked_after_secs,
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

pub fn select_market_from_instruments(
    target: UpdownSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedUpdownMarket> {
    let now_unix_secs = i64::try_from(Duration::from_millis(now_milliseconds).as_secs()).ok()?;
    let (current_start, next_start) =
        updown_period_pair(target.cadence_secs, now_unix_secs).ok()?;
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

    candidate_market_for_slug(
        instruments,
        &current_slug,
        current_start,
        MarketSelectionOutcome::Current,
        now_milliseconds,
    )
    .or_else(|| {
        candidate_market_for_slug(
            instruments,
            &next_slug,
            next_start,
            MarketSelectionOutcome::Next,
            now_milliseconds,
        )
    })
}

pub fn select_binary_option_market(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    let market = select_market_from_instruments(
        UpdownSelectionTarget {
            underlying_asset: target.underlying_asset,
            cadence_secs: target.cadence_seconds,
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
        selection_outcome: market.selection_outcome,
        start_timestamp_milliseconds: market.start_timestamp_milliseconds,
        expiration_timestamp_milliseconds: market.expiration_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
        source_identity: market.source_identity,
    })
}

pub fn market_selection_candidate_windows(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    let now_unix_secs =
        i64::try_from(Duration::from_millis(now_milliseconds).as_secs()).map_err(|_| {
            InstrumentFilterError::PeriodPairOverflow {
                now_unix_seconds: i64::MAX,
                cadence_seconds: target.cadence_seconds,
            }
        })?;
    let (current_start, next_start) = updown_period_pair(target.cadence_seconds, now_unix_secs)
        .map_err(market_identity_error_to_instrument_filter_error)?;
    Ok(vec![
        MarketSelectionCandidateWindow {
            outcome: MarketSelectionOutcome::Current,
            market_slug: updown_market_slug(
                target.underlying_asset,
                target.cadence_slug_token,
                current_start,
            ),
            start_timestamp_milliseconds: period_start_milliseconds(
                current_start,
                target.cadence_seconds,
            )?,
        },
        MarketSelectionCandidateWindow {
            outcome: MarketSelectionOutcome::Next,
            market_slug: updown_market_slug(
                target.underlying_asset,
                target.cadence_slug_token,
                next_start,
            ),
            start_timestamp_milliseconds: period_start_milliseconds(
                next_start,
                target.cadence_seconds,
            )?,
        },
    ])
}

/// Family-owned binary up/down fair-value model.
///
/// Black-Scholes digital: under risk-neutral GBM the probability the
/// underlying finishes above the strike is `N(d2)`, where
/// `d2 = (ln(S/K) - sigma_eff^2/2 * T) / (sigma_eff * sqrt(T))`. The
/// realized-volatility estimate is widened by a kurtosis term
/// (`sigma_eff = realized_vol * (1 + kurtosis / 6)`) so fat-tailed
/// regimes price wider. Fails closed (returns `None`) on degenerate
/// inputs so the strategy treats the market as un-priceable rather
/// than acting on a step-function probability.
pub fn fair_probability_up(inputs: &FairProbabilityInputs) -> Option<f64> {
    if !inputs.spot_price.is_finite()
        || !is_positive_finite(inputs.spot_price)
        || !is_positive_finite(inputs.strike_price)
        || !is_positive_finite(inputs.realized_vol)
        || !inputs.pricing_kurtosis.is_finite()
    {
        return None;
    }

    let sigma_eff =
        inputs.realized_vol * (UNIT_F64 + inputs.pricing_kurtosis / KURTOSIS_NORMALIZATION);
    if !is_positive_finite(sigma_eff) {
        return None;
    }

    let time_to_expiry_years = inputs.seconds_to_market_end as f64 / SECONDS_PER_YEAR_F64;
    if time_to_expiry_years <= ZERO_F64 {
        return None;
    }

    let d2 = ((inputs.spot_price / inputs.strike_price).ln()
        - (sigma_eff.powi(POWER_OF_TWO) / SIGMA_SQUARED_HALF_DIVISOR) * time_to_expiry_years)
        / (sigma_eff * time_to_expiry_years.sqrt());
    sanitize_probability(standard_normal_cdf(d2))
}

fn standard_normal_cdf(x: f64) -> f64 {
    let t = UNIT_F64 / (UNIT_F64 + NORMAL_CDF_T_SCALE * x.abs());
    let d = NORMAL_CDF_DENSITY_SCALE * (-x * x / NORMAL_DENSITY_EXPONENT_DIVISOR).exp();
    let prob = d
        * t
        * (NORMAL_CDF_POLY_A1
            + t * (NORMAL_CDF_POLY_A2
                + t * (NORMAL_CDF_POLY_A3 + t * (NORMAL_CDF_POLY_A4 + t * NORMAL_CDF_POLY_A5))));
    if x > ZERO_F64 { UNIT_F64 - prob } else { prob }
}

const SIGMA_SQUARED_HALF_DIVISOR: f64 = 2.0;
const KURTOSIS_NORMALIZATION: f64 = 6.0;
const NORMAL_DENSITY_EXPONENT_DIVISOR: f64 = 2.0;
const NORMAL_CDF_T_SCALE: f64 = 0.231_641_9;
const NORMAL_CDF_DENSITY_SCALE: f64 = 0.398_942_3;
const NORMAL_CDF_POLY_A1: f64 = 0.319_381_5;
const NORMAL_CDF_POLY_A2: f64 = -0.356_563_8;
const NORMAL_CDF_POLY_A3: f64 = 1.781_478;
const NORMAL_CDF_POLY_A4: f64 = -1.821_256;
const NORMAL_CDF_POLY_A5: f64 = 1.330_274;

fn period_start_milliseconds(
    period_start_seconds: i64,
    cadence_seconds: i64,
) -> Result<u64, InstrumentFilterError> {
    let seconds = u64::try_from(period_start_seconds).map_err(|_| {
        InstrumentFilterError::NegativeNowUnixSeconds {
            now_unix_seconds: period_start_seconds,
        }
    })?;
    u64::try_from(Duration::from_secs(seconds).as_millis()).map_err(|_| {
        InstrumentFilterError::PeriodPairOverflow {
            now_unix_seconds: period_start_seconds,
            cadence_seconds,
        }
    })
}

fn market_identity_error_to_instrument_filter_error(
    error: BoltV3MarketIdentityError,
) -> InstrumentFilterError {
    match error {
        BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_secs,
        } => InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds: cadence_secs,
        },
        BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_secs } => {
            InstrumentFilterError::NegativeNowUnixSeconds {
                now_unix_seconds: now_unix_secs,
            }
        }
        BoltV3MarketIdentityError::PeriodPairOverflow {
            now_unix_secs,
            cadence_secs,
        } => InstrumentFilterError::PeriodPairOverflow {
            now_unix_seconds: now_unix_secs,
            cadence_seconds: cadence_secs,
        },
        BoltV3MarketIdentityError::UnsupportedCadenceSeconds { .. }
        | BoltV3MarketIdentityError::TargetParseFailed { .. } => {
            InstrumentFilterError::TargetValidationFailure {
                message: error.to_string(),
            }
        }
    }
}

fn candidate_market_for_slug(
    instruments: &[InstrumentAny],
    market_slug: &str,
    period_start_unix_secs: i64,
    selection_outcome: MarketSelectionOutcome,
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
    if up.market_id != down.market_id
        || up.condition_id != down.condition_id
        || up.market_slug != down.market_slug
        || up.question_id != down.question_id
    {
        return None;
    }

    let expiration_milliseconds = up.expiration_milliseconds.min(down.expiration_milliseconds);
    if expiration_milliseconds <= now_milliseconds {
        return None;
    }

    let start_timestamp_milliseconds = if up.activation_milliseconds == 0
        || down.activation_milliseconds == 0
    {
        u64::try_from(Duration::from_secs(u64::try_from(period_start_unix_secs).ok()?).as_millis())
            .ok()?
    } else {
        up.activation_milliseconds.min(down.activation_milliseconds)
    };

    Some(SelectedUpdownMarket {
        market_id: up.market_id,
        source_identity: SelectedMarketSourceIdentity {
            condition_id: up.condition_id,
            market_slug: up.market_slug,
            question_id: up.question_id,
        },
        instrument_id: up.instrument_id,
        up_instrument_id: up.instrument_id,
        down_instrument_id: down.instrument_id,
        selection_outcome,
        start_timestamp_milliseconds,
        expiration_timestamp_milliseconds: expiration_milliseconds,
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
        condition_id: info.get_str("condition_id")?.to_string(),
        market_slug: info.get_str("market_slug")?.to_string(),
        question_id: info.get_str("question_id")?.to_string(),
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
            Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
                strategy_instance_id: None,
                configured_target_id: None,
                cadence_secs: 0,
            })
        ));
    }

    #[test]
    fn updown_cadence_token_table_matches_runtime_contract() {
        assert_eq!(updown_cadence_slug_token(60), Some("1m"));
        assert_eq!(updown_cadence_slug_token(300), Some("5m"));
        assert_eq!(updown_cadence_slug_token(900), Some("15m"));
        assert_eq!(updown_cadence_slug_token(3600), Some("1h"));
        assert_eq!(updown_cadence_slug_token(14400), Some("4h"));
        assert_eq!(updown_cadence_slug_token(120), None);
    }

    #[test]
    fn fair_probability_is_directional_and_fail_closed_on_invalid_inputs() {
        let above = fair_probability_up(&FairProbabilityInputs {
            spot_price: 3_105.0,
            strike_price: 3_100.0,
            seconds_to_market_end: 60,
            realized_vol: 0.45,
            pricing_kurtosis: 0.0,
        })
        .expect("valid inputs should produce fair probability");
        let below = fair_probability_up(&FairProbabilityInputs {
            spot_price: 3_095.0,
            strike_price: 3_100.0,
            seconds_to_market_end: 60,
            realized_vol: 0.45,
            pricing_kurtosis: 0.0,
        })
        .expect("valid inputs should produce fair probability");

        assert!(
            above > 0.5,
            "above-strike spot should imply >50% up probability"
        );
        assert!(
            below < 0.5,
            "below-strike spot should imply <50% up probability"
        );
        assert!(above > below);
        assert!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_100.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 60,
                realized_vol: 0.0,
                pricing_kurtosis: 0.0,
            })
            .is_none()
        );
    }

    #[test]
    fn fair_probability_fails_closed_when_expired() {
        assert!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_105.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 0,
                realized_vol: 0.45,
                pricing_kurtosis: 0.0,
            })
            .is_none(),
            "expired markets must not produce a step-function entry probability"
        );
    }
}
