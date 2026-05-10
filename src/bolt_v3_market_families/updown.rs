//! Updown rotating-cadence market-family identity binding for bolt-v3.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//! (slug derivation rule lives in Section 5.3)
//!
//! This module owns the updown family's market-identity surface as a
//! pure data boundary. It projects a validated bolt-v3 configuration
//! plus an injected `now_unix_seconds` value into a
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
//! truth for supported `cadence_seconds` values.
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

use serde::Deserialize;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_market_families::MarketFamilyValidationBinding,
};

pub const KEY: &str = "updown";

pub fn validation_binding() -> MarketFamilyValidationBinding {
    MarketFamilyValidationBinding {
        key: KEY,
        validate_target: validate_target_block,
    }
}

/// Updown rotating-cadence target block. Owned by the updown market-
/// family binding because `cadence_seconds`, `underlying_asset`,
/// `rotating_market_family`, and `market_selection_rule` are family-
/// shaped fields. The strategy envelope (`crate::bolt_v3_config::
/// BoltV3StrategyConfig`) keeps the TOML field name `[target]` as a
/// generic `toml::Value`; the updown family deserializes that raw
/// envelope into this typed shape during validation and planning.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub market_selection_type: MarketSelectionType,
    pub rotating_market_family: RotatingMarketFamily,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub market_selection_rule: MarketSelectionRule,
    pub retry_interval_seconds: u64,
    pub blocked_after_seconds: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketSelectionType {
    RotatingMarket,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RotatingMarketFamily {
    Updown,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
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

/// Runtime-contract `cadence_seconds -> slug-token` table for the
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
/// `cadence_seconds`, or `None` if the value is not in the table.
pub fn updown_cadence_slug_token(cadence_seconds: i64) -> Option<&'static str> {
    UPDOWN_CADENCE_SLUG_TOKEN_TABLE
        .iter()
        .find_map(|(seconds, token)| (*seconds == cadence_seconds).then_some(*token))
}

/// Enumerate the `cadence_seconds` values currently supported by the
/// runtime-contract slug-token table, in declaration order. Used in
/// startup-validation error messages so the operator sees the exact
/// allowed set.
pub fn supported_updown_cadence_seconds() -> Vec<i64> {
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

    errors.extend(validate_target_cadence(context, block.cadence_seconds));

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

    // Reading `block.market_selection_rule` and
    // `block.market_selection_type` here is a
    // no-op exhaustive match: the only allowed variants are encoded by
    // the typed enums above, so any TOML value other than
    // `active_or_next` / `rotating_market` was already rejected by
    // typed deserialization.
    let MarketSelectionRule::ActiveOrNext = block.market_selection_rule;
    let MarketSelectionType::RotatingMarket = block.market_selection_type;
    let RotatingMarketFamily::Updown = block.rotating_market_family;

    errors
}

/// Family-specific cadence validator for updown rotating-market
/// targets. Owns the positive / minute-aligned / table-membership
/// rules so core startup validation can stay structural and dispatch
/// per-family cadence policy here.
pub fn validate_target_cadence(context: &str, cadence_seconds: i64) -> Vec<String> {
    let mut errors = Vec::new();
    if cadence_seconds <= 0 {
        errors.push(format!(
            "{context}: target.cadence_seconds must be a positive integer (got {cadence_seconds})"
        ));
    } else if cadence_seconds % 60 != 0 {
        errors.push(format!(
            "{context}: target.cadence_seconds must be divisible by 60 (got {cadence_seconds})"
        ));
    } else if updown_cadence_slug_token(cadence_seconds).is_none() {
        let supported = supported_updown_cadence_seconds();
        errors.push(format!(
            "{context}: target.cadence_seconds={cadence_seconds} has no runtime-contract-defined updown slug-token mapping; supported values are {supported:?}"
        ));
    }
    errors
}

/// Pure projection of all updown rotating-market targets in a
/// validated bolt-v3 configuration. One `UpdownTargetPlan` per
/// configured strategy whose target maps to the updown family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketIdentityPlan {
    pub updown_targets: Vec<UpdownTargetPlan>,
}

pub struct MarketIdentityClientTargetRef<'a> {
    pub family_key: &'static str,
    pub configured_target_id: &'a str,
    pub client_id_key: &'a str,
}

impl MarketIdentityPlan {
    pub fn client_id_target_refs(&self) -> impl Iterator<Item = MarketIdentityClientTargetRef<'_>> {
        self.updown_targets
            .iter()
            .map(|target| MarketIdentityClientTargetRef {
                family_key: KEY,
                configured_target_id: target.configured_target_id.as_str(),
                client_id_key: target.client_id_key.as_str(),
            })
    }
}

/// Pure identity facts for one configured updown rotating-market
/// target. Every value here is derived from validated configuration
/// and the runtime-contract slug-token table; nothing here depends on
/// wall-clock time, the NT instrument index, or any network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub client_id_key: String,
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
        configured_target_id: Option<String>,
        cadence_seconds: i64,
    },
    UnsupportedCadenceSeconds {
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
                cadence_seconds,
            } => write!(
                f,
                "{prefix}target.cadence_seconds must be a positive integer (got {cadence_seconds})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => write!(
                f,
                "{prefix}target.cadence_seconds={cadence_seconds} has no runtime-contract-defined updown slug-token mapping",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_seconds } => {
                write!(
                    f,
                    "now_unix_seconds must be non-negative (got {now_unix_seconds})"
                )
            }
            BoltV3MarketIdentityError::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => write!(
                f,
                "updown period pair overflows i64 (now_unix_seconds={now_unix_seconds}, cadence_seconds={cadence_seconds})"
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
/// (non-positive or unsupported `cadence_seconds`).
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
    let client_id_key = strategy.config.execution_client_id.clone();
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
    let MarketSelectionType::RotatingMarket = target.market_selection_type;
    let RotatingMarketFamily::Updown = target.rotating_market_family;

    let configured_target_id = target.configured_target_id.clone();
    if target.cadence_seconds <= 0 {
        return Err(BoltV3MarketIdentityError::NonPositiveCadenceSeconds {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_seconds: target.cadence_seconds,
        });
    }
    let token = match updown_cadence_slug_token(target.cadence_seconds) {
        Some(token) => token,
        None => {
            return Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id: Some(strategy_instance_id),
                configured_target_id: Some(configured_target_id),
                cadence_seconds: target.cadence_seconds,
            });
        }
    };

    Ok(Some(UpdownTargetPlan {
        strategy_instance_id,
        configured_target_id,
        client_id_key,
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
            configured_target_id: None,
            cadence_seconds,
        });
    }
    if now_unix_seconds < 0 {
        return Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds { now_unix_seconds });
    }
    let current = (now_unix_seconds / cadence_seconds) * cadence_seconds;
    let next = current.checked_add(cadence_seconds).ok_or(
        BoltV3MarketIdentityError::PeriodPairOverflow {
            now_unix_seconds,
            cadence_seconds,
        },
    )?;
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

    fn target_value(source: &str) -> toml::Value {
        toml::from_str(source).expect("target TOML should parse")
    }

    #[test]
    fn updown_target_accepts_market_selection_type_field() {
        let target = target_value(
            r#"
configured_target_id = "eth_updown_5m"
market_selection_type = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "ETH"
cadence_seconds = 300
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60
"#,
        );

        assert_eq!(
            validate_target_block("strategy", &target),
            Vec::<String>::new()
        );
    }

    #[test]
    fn updown_target_rejects_legacy_kind_field() {
        let target = target_value(
            r#"
configured_target_id = "eth_updown_5m"
kind = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "ETH"
cadence_seconds = 300
market_selection_rule = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60
"#,
        );

        let errors = validate_target_block("strategy", &target);
        assert!(
            errors
                .iter()
                .any(|message| message.contains("unknown field `kind`")),
            "legacy target.kind must be rejected after market_selection_type rename: {errors:#?}"
        );
    }

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
                cadence_seconds: 0,
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
}
