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

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::{
        GATE_PROVIDER_CAPABILITIES, GATE_PROVIDER_KINDS, GATE_ROLES, GATE_VALUE_KINDS,
        LoadedBoltV3Config, LoadedStrategy, NO_RESOLUTION_KIND, NO_RESOLUTION_VALUE_KIND,
        RESOLUTION_GATE_ROLE,
    },
    bolt_v3_instrument_filters::{InstrumentFilterError, InstrumentFilterTarget},
    bolt_v3_market_families::{
        MarketFamilyValidationBinding, MarketIdentityPlan, MarketIdentityTarget,
        MarketSelectionCandidateWindow, MarketSelectionOutcome, MarketSelectionTarget,
        SelectedBinaryOptionMarket, SelectedMarketRequirement, SelectedMarketRequirementParts,
        SelectedMarketSourceIdentity, TargetRuntimeFields,
        selected_market_metadata_provenance_fields, selected_market_requirement_error,
        selected_market_requirement_from_parts,
    },
};

pub const KEY: &str = "updown";
const BINARY_OPTION_MARKET_CLASS: &str = "binary_option";
const NT_INSTRUMENT_METADATA_SOURCE_KIND: &str = "nt_instrument_metadata";
const REQUIRED_UPDOWN_OUTCOME_INSTRUMENT_COUNT: usize = 2;
const METADATA_CONDITION_ID_FIELD: &str = "condition_id";
const METADATA_FAMILY_KEY_FIELD: &str = "family_key";
const METADATA_INSTRUMENT_IDS_FIELD: &str = "instrument_ids";
const METADATA_MARKET_CLASS_FIELD: &str = "market_class";
const METADATA_MARKET_ID_FIELD: &str = "market_id";
const METADATA_MARKET_SLUG_FIELD: &str = "market_slug";
const METADATA_QUESTION_ID_FIELD: &str = "question_id";
const METADATA_SOURCE_KIND_FIELD: &str = "source_kind";
const METADATA_VENUE_FIELD: &str = "venue";

pub fn validation_binding() -> MarketFamilyValidationBinding {
    MarketFamilyValidationBinding {
        key: KEY,
        validate_target: validate_target_block,
        instrument_filter_target_for_strategy,
        target_runtime_fields,
        select_binary_option_market,
        market_selection_candidate_windows,
        selected_market_requirement,
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
    pub gate_subscriptions: Option<BTreeMap<String, TargetGateSubscription>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetGateSubscription {
    pub required: bool,
    pub allowed_provider_ids: Option<Vec<String>>,
    pub allowed_provider_kinds: Option<Vec<String>>,
    pub allowed_value_kinds: Option<Vec<String>>,
    pub provider_preference: Option<Vec<String>>,
    pub allow_no_resolution: bool,
    pub market_mappings: Option<Vec<TargetGateMarketMapping>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetGateMarketMapping {
    pub family_key: String,
    pub market_class: String,
    pub resolution_kind: String,
    pub resolution_identity: String,
    pub value_kind: String,
    pub provider_id: Option<String>,
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

    errors.extend(validate_gate_subscriptions(context, &block));

    errors
}

fn validate_gate_subscriptions(context: &str, block: &TargetBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(gate_subscriptions) = &block.gate_subscriptions else {
        return errors;
    };

    for (role, subscription) in gate_subscriptions {
        let subscription_path = format!("{context}: target.gate_subscriptions.{role}");
        if !GATE_ROLES.contains(&role.as_str()) {
            let role_kind = if GATE_PROVIDER_CAPABILITIES.contains(&role.as_str()) {
                "provider capability"
            } else {
                "unknown role"
            };
            errors.push(format!(
                "{subscription_path} is a {role_kind}, not a GateRole"
            ));
            continue;
        }

        let allowed_provider_ids = subscription.allowed_provider_ids.as_deref().unwrap_or(&[]);
        let allowed_provider_kinds = subscription
            .allowed_provider_kinds
            .as_deref()
            .unwrap_or(&[]);
        let allowed_value_kinds = subscription.allowed_value_kinds.as_deref().unwrap_or(&[]);
        let provider_preference = subscription.provider_preference.as_deref().unwrap_or(&[]);
        let market_mappings = subscription.market_mappings.as_deref().unwrap_or(&[]);

        for provider_kind in allowed_provider_kinds {
            if !GATE_PROVIDER_KINDS.contains(&provider_kind.as_str()) {
                errors.push(format!(
                    "{subscription_path}.allowed_provider_kinds contains unregistered provider kind `{provider_kind}`"
                ));
            }
        }
        for value_kind in allowed_value_kinds {
            if !GATE_VALUE_KINDS.contains(&value_kind.as_str()) {
                errors.push(format!(
                    "{subscription_path}.allowed_value_kinds contains unregistered value_kind `{value_kind}`"
                ));
            }
        }

        if subscription.required && market_mappings.is_empty() && allowed_provider_ids.len() == 1 {
            errors.push(format!(
                "{subscription_path} uses a single static provider for a rotating market; add market_mappings or provider_kind rotation metadata"
            ));
        }
        if allowed_provider_ids.len() > 1 && provider_preference.is_empty() {
            errors.push(format!(
                "{subscription_path}.provider_preference is required when multiple providers can match"
            ));
        }

        let mut mapping_keys = BTreeSet::new();
        for mapping in market_mappings {
            let mapping_key = (
                mapping.family_key.as_str(),
                mapping.market_class.as_str(),
                mapping.resolution_kind.as_str(),
                mapping.resolution_identity.as_str(),
                mapping.value_kind.as_str(),
            );
            if !mapping_keys.insert(mapping_key) {
                errors.push(format!(
                    "{subscription_path}.market_mappings contains ambiguous duplicate mapping for family_key `{}`, market_class `{}`, resolution_kind `{}`, resolution_identity `{}`, value_kind `{}`",
                    mapping.family_key,
                    mapping.market_class,
                    mapping.resolution_kind,
                    mapping.resolution_identity,
                    mapping.value_kind
                ));
            }

            let no_resolution_mapping =
                subscription.allow_no_resolution && mapping.resolution_kind == NO_RESOLUTION_KIND;
            let provider_kind_matches = allowed_provider_kinds.is_empty()
                || no_resolution_mapping
                || allowed_provider_kinds
                    .iter()
                    .any(|kind| kind == &mapping.resolution_kind);
            let value_kind_matches = allowed_value_kinds
                .iter()
                .any(|kind| kind == &mapping.value_kind);
            if !provider_kind_matches || !value_kind_matches {
                errors.push(format!(
                    "{subscription_path} market mapping resolution_kind `{}` must match allowed_provider_kinds and value_kind `{}` must match allowed_value_kinds",
                    mapping.resolution_kind, mapping.value_kind
                ));
            }

            if subscription.allow_no_resolution
                && mapping.resolution_kind == NO_RESOLUTION_KIND
                && mapping.value_kind != NO_RESOLUTION_VALUE_KIND
            {
                errors.push(format!(
                    "{subscription_path}.allow_no_resolution with no_resolution requires value_kind `none`, got `{}`",
                    mapping.value_kind
                ));
            }
        }
    }

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

pub fn selected_market_requirement(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    let target = deserialize_target_block(target)
        .map_err(|message| InstrumentFilterError::Other { message })?;
    // Updown readiness currently extracts the selected market's resolution
    // requirement. Role joins and additional gate roles belong to T036H16+.
    let mapping = selected_market_resolution_mapping(&target)?;
    let mut instrument_ids = vec![
        selected.down_instrument_id.to_string(),
        selected.up_instrument_id.to_string(),
    ];
    instrument_ids.sort();
    instrument_ids.dedup();
    if instrument_ids.len() != REQUIRED_UPDOWN_OUTCOME_INSTRUMENT_COUNT {
        return Err(selected_market_requirement_error(
            "selected-market instrument_ids must include distinct up/down outcomes",
        ));
    }
    let up_venue = selected.up_instrument_id.venue.as_str();
    let down_venue = selected.down_instrument_id.venue.as_str();
    if up_venue != down_venue {
        return Err(selected_market_requirement_error(
            "selected-market up/down instrument venues must match",
        ));
    }

    let mut provenance_fields = selected_market_metadata_provenance_fields([
        (
            METADATA_CONDITION_ID_FIELD,
            selected.source_identity.condition_id.as_str(),
        ),
        (METADATA_FAMILY_KEY_FIELD, KEY),
        (METADATA_MARKET_CLASS_FIELD, BINARY_OPTION_MARKET_CLASS),
        (METADATA_MARKET_ID_FIELD, selected.market_id.as_str()),
        (
            METADATA_MARKET_SLUG_FIELD,
            selected.source_identity.market_slug.as_str(),
        ),
        (
            METADATA_QUESTION_ID_FIELD,
            selected.source_identity.question_id.as_str(),
        ),
        (
            METADATA_SOURCE_KIND_FIELD,
            NT_INSTRUMENT_METADATA_SOURCE_KIND,
        ),
        (METADATA_VENUE_FIELD, up_venue),
    ]);
    provenance_fields.insert(
        METADATA_INSTRUMENT_IDS_FIELD.to_string(),
        serde_json::json!(instrument_ids),
    );

    selected_market_requirement_from_parts(SelectedMarketRequirementParts {
        configured_target_id: target.configured_target_id.as_str(),
        venue: up_venue,
        family_key: KEY,
        market_id: selected.market_id.as_str(),
        instrument_ids,
        market_class: mapping.market_class.as_str(),
        resolution_kind: mapping.resolution_kind.as_str(),
        resolution_identity: mapping.resolution_identity.as_str(),
        value_kind: mapping.value_kind.as_str(),
        metadata_provenance_fields: provenance_fields,
        selected_at_ms,
    })
}

fn selected_market_resolution_mapping(
    target: &TargetBlock,
) -> Result<&TargetGateMarketMapping, InstrumentFilterError> {
    let mappings = target
        .gate_subscriptions
        .as_ref()
        .and_then(|subscriptions| subscriptions.get(RESOLUTION_GATE_ROLE))
        .and_then(|subscription| subscription.market_mappings.as_ref())
        .filter(|mappings| !mappings.is_empty())
        .ok_or_else(|| {
            selected_market_requirement_error(
                "target.gate_subscriptions.resolution.market_mappings must resolve selected-market identity",
            )
        })?;

    let mut matches = mappings.iter().filter(|mapping| {
        mapping.family_key == KEY && mapping.market_class == BINARY_OPTION_MARKET_CLASS
    });
    let Some(first) = matches.next() else {
        return Err(selected_market_requirement_error(
            "target.gate_subscriptions.resolution.market_mappings must include an updown binary_option mapping",
        ));
    };
    if matches.next().is_some() {
        return Err(selected_market_requirement_error(
            "target.gate_subscriptions.resolution.market_mappings contains ambiguous updown binary_option mappings",
        ));
    }
    if first.resolution_kind.is_empty()
        || first.resolution_identity.is_empty()
        || first.value_kind.is_empty()
    {
        return Err(selected_market_requirement_error(
            "target.gate_subscriptions.resolution.market_mappings must include non-empty resolution_kind, resolution_identity, and value_kind",
        ));
    }
    Ok(first)
}

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

    fn selected_market_fixture() -> SelectedBinaryOptionMarket {
        SelectedBinaryOptionMarket {
            market_id: "market-1".to_string(),
            instrument_id: InstrumentId::from("condition-1-UP.POLYMARKET"),
            up_instrument_id: InstrumentId::from("condition-1-UP.POLYMARKET"),
            down_instrument_id: InstrumentId::from("condition-1-DOWN.POLYMARKET"),
            selection_outcome: MarketSelectionOutcome::Current,
            start_timestamp_milliseconds: 600_000,
            expiration_timestamp_milliseconds: 900_000,
            seconds_to_end: 300,
            source_identity: SelectedMarketSourceIdentity {
                condition_id: "condition-1".to_string(),
                market_slug: "btc-updown-5m-600".to_string(),
                question_id: "question-1".to_string(),
            },
        }
    }

    fn target_with_resolution_mapping() -> toml::Value {
        toml::toml! {
            configured_target_id = "btc_updown_5m"
            kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "BTC"
            cadence_secs = 300
            market_selection_rule = "active_or_next"
            retry_interval_secs = 5
            blocked_after_secs = 30

            [gate_subscriptions.resolution]
            required = true
            allowed_provider_kinds = ["chainlink_data_streams", "pyth"]
            allowed_value_kinds = ["price"]
            provider_preference = ["resolution_oracle_primary"]
            allow_no_resolution = false

            [[gate_subscriptions.resolution.market_mappings]]
            family_key = "updown"
            market_class = "binary_option"
            resolution_kind = "chainlink_data_streams"
            resolution_identity = "btc-usd-5m"
            value_kind = "price"
            provider_id = "resolution_oracle_primary"
        }
        .into()
    }

    fn resolution_mapping_array_mut(target: &mut toml::Value) -> &mut Vec<toml::Value> {
        target
            .as_table_mut()
            .expect("target should be a table")
            .get_mut("gate_subscriptions")
            .expect("gate subscriptions should exist")
            .as_table_mut()
            .expect("gate subscriptions should be a table")
            .get_mut("resolution")
            .expect("resolution subscription should exist")
            .as_table_mut()
            .expect("resolution should be a table")
            .get_mut("market_mappings")
            .expect("market mappings should exist")
            .as_array_mut()
            .expect("market mappings should be an array")
    }

    fn assert_selected_market_requirement_error(target: toml::Value, expected: &str) {
        let error = selected_market_requirement(&target, &selected_market_fixture(), 700_000)
            .expect_err("selected-market requirement should fail closed");
        assert!(
            error.to_string().contains(expected),
            "expected error to contain `{expected}`, got: {error}"
        );
    }

    fn is_lowercase_sha256(value: &str) -> bool {
        value.len() == 64
            && value
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    }

    #[test]
    fn selected_market_requirement_resolves_updown_identity_from_target_mapping() {
        let requirement = selected_market_requirement(
            &target_with_resolution_mapping(),
            &selected_market_fixture(),
            700_000,
        )
        .expect("updown selected-market requirement should resolve from target mapping");

        assert_eq!(requirement.configured_target_id, "btc_updown_5m");
        assert_eq!(requirement.venue, "POLYMARKET");
        assert_eq!(requirement.family_key, KEY);
        assert_eq!(requirement.market_id, "market-1");
        assert_eq!(
            requirement.instrument_ids,
            vec!["condition-1-DOWN.POLYMARKET", "condition-1-UP.POLYMARKET"]
        );
        assert_eq!(requirement.market_class, "binary_option");
        assert_eq!(requirement.resolution_kind, "chainlink_data_streams");
        assert_eq!(requirement.resolution_identity, "btc-usd-5m");
        assert_eq!(requirement.value_kind, "price");
        assert_eq!(requirement.selected_at_ms, 700_000);
        assert!(is_lowercase_sha256(&requirement.metadata_provenance_sha256));
        assert!(is_lowercase_sha256(&requirement.selected_market_key));
    }

    #[test]
    fn selected_market_requirement_dispatches_through_public_family_registry() {
        let requirement = crate::bolt_v3_market_families::selected_market_requirement_from_target(
            &target_with_resolution_mapping(),
            &selected_market_fixture(),
            700_000,
        )
        .expect("public family registry should dispatch updown requirement extraction");

        assert_eq!(requirement.family_key, KEY);
        assert_eq!(requirement.resolution_identity, "btc-usd-5m");
        assert_eq!(
            requirement.instrument_ids,
            vec!["condition-1-DOWN.POLYMARKET", "condition-1-UP.POLYMARKET"]
        );
    }

    #[test]
    fn selected_market_requirement_fails_without_config_resolved_mapping() {
        let mut target = target_with_resolution_mapping();
        resolution_mapping_array_mut(&mut target).clear();

        assert_selected_market_requirement_error(
            target,
            "target.gate_subscriptions.resolution.market_mappings",
        );
    }

    #[test]
    fn selected_market_requirement_fails_when_subscription_shape_is_absent() {
        let mut no_gate_subscriptions = target_with_resolution_mapping();
        no_gate_subscriptions
            .as_table_mut()
            .expect("target should be a table")
            .remove("gate_subscriptions");
        assert_selected_market_requirement_error(
            no_gate_subscriptions,
            "target.gate_subscriptions.resolution.market_mappings",
        );

        let mut no_resolution = target_with_resolution_mapping();
        no_resolution
            .as_table_mut()
            .expect("target should be a table")
            .get_mut("gate_subscriptions")
            .expect("gate subscriptions should exist")
            .as_table_mut()
            .expect("gate subscriptions should be a table")
            .remove("resolution");
        assert_selected_market_requirement_error(
            no_resolution,
            "target.gate_subscriptions.resolution.market_mappings",
        );

        let mut no_market_mappings = target_with_resolution_mapping();
        no_market_mappings
            .as_table_mut()
            .expect("target should be a table")
            .get_mut("gate_subscriptions")
            .expect("gate subscriptions should exist")
            .as_table_mut()
            .expect("gate subscriptions should be a table")
            .get_mut("resolution")
            .expect("resolution should exist")
            .as_table_mut()
            .expect("resolution should be a table")
            .remove("market_mappings");
        assert_selected_market_requirement_error(
            no_market_mappings,
            "target.gate_subscriptions.resolution.market_mappings",
        );
    }

    #[test]
    fn selected_market_requirement_fails_on_ambiguous_resolution_mapping() {
        let mut target = target_with_resolution_mapping();
        let mut alternate = resolution_mapping_array_mut(&mut target)[0].clone();
        alternate
            .as_table_mut()
            .expect("mapping should be a table")
            .insert(
                "resolution_identity".to_string(),
                toml::Value::String("btc-usd-backup-5m".to_string()),
            );
        resolution_mapping_array_mut(&mut target).push(alternate);

        assert_selected_market_requirement_error(target, "ambiguous");
    }

    #[test]
    fn selected_market_requirement_fails_when_selected_instrument_venues_differ() {
        let mut selected = selected_market_fixture();
        selected.down_instrument_id = InstrumentId::from("condition-1-DOWN.SIM");

        let error =
            selected_market_requirement(&target_with_resolution_mapping(), &selected, 700_000)
                .expect_err("venue mismatch should fail closed");
        assert!(
            error.to_string().contains("venue"),
            "expected venue mismatch, got: {error}"
        );
    }

    #[test]
    fn selected_market_requirement_fails_when_key_or_provenance_component_contains_pipe() {
        let mut selected = selected_market_fixture();
        selected.source_identity.question_id = "question|1".to_string();

        let error =
            selected_market_requirement(&target_with_resolution_mapping(), &selected, 700_000)
                .expect_err("pipe in source identity should fail closed");
        assert!(
            error.to_string().contains("|"),
            "expected pipe rejection, got: {error}"
        );

        let mut target = target_with_resolution_mapping();
        resolution_mapping_array_mut(&mut target)[0]
            .as_table_mut()
            .expect("mapping should be a table")
            .insert(
                "resolution_identity".to_string(),
                toml::Value::String("btc|usd".to_string()),
            );
        assert_selected_market_requirement_error(target, "|");
    }

    #[test]
    fn selected_market_requirement_fails_when_metadata_provenance_component_is_empty() {
        let mut selected = selected_market_fixture();
        selected.source_identity.condition_id.clear();

        let error =
            selected_market_requirement(&target_with_resolution_mapping(), &selected, 700_000)
                .expect_err("empty source identity should fail closed");
        assert!(
            error.to_string().contains("metadata_provenance"),
            "expected provenance rejection, got: {error}"
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
}
