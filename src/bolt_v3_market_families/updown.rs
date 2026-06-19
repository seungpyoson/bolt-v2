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
//! truth for updown-specific target shape.
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
    sync::Arc,
    time::Duration,
};

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use super::OutcomeSide;

use crate::{
    bolt_v3_config::{
        GATE_PROVIDER_CAPABILITIES, GATE_PROVIDER_KINDS, GATE_ROLES, GATE_VALUE_KINDS,
        LoadedBoltV3Config, LoadedStrategy, NO_RESOLUTION_KIND, NO_RESOLUTION_VALUE_KIND,
        RESOLUTION_GATE_ROLE,
    },
    bolt_v3_instrument_filters::{InstrumentFilterError, format_target_prefix},
    bolt_v3_maker_settlement::BinarySettlementPayout,
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketIdentityPlan, MarketIdentityTarget,
        MarketSelectionCandidateWindow, MarketSelectionOutcome, MarketSelectionTarget,
        SelectedBinaryOptionMarket, SelectedMarketRequirement, SelectedMarketRequirementParts,
        SelectedMarketSourceIdentity, TargetRuntimeFields,
        selected_market_metadata_provenance_fields, selected_market_requirement_error,
        selected_market_requirement_from_parts,
    },
    bolt_v3_numeric::{
        HALF_F64, MILLIS_PER_SECOND_U64, POWER_OF_TWO, SECONDS_PER_YEAR_F64, UNIT_F64, ZERO_F64,
        is_non_negative_finite, is_positive_finite, sanitize_probability,
    },
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::{FamilyQuoteInputs, QuoteTargets},
};

pub const KEY: &str = "updown";
const BINARY_OPTION_MARKET_CLASS: &str = "binary_option";
const TARGET_ENUM_SERIALIZE_FAILURE_MESSAGE: &str =
    "updown target discriminator enum could not serialize to a string token";
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

pub fn maker_quote_targets(inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
    super::binary_outcome::maker_quote_targets(inputs)
}

pub fn maker_settlement_payout(payout: BinarySettlementPayout, leg: Leg) -> Option<f64> {
    super::binary_outcome::maker_settlement_payout(payout, leg)
}

pub fn maker_settlement_payout_from_reference_prices(
    close_price: f64,
    strike_price: f64,
) -> Option<BinarySettlementPayout> {
    if !is_positive_finite(close_price) || !is_positive_finite(strike_price) {
        return None;
    }
    if close_price >= strike_price {
        BinarySettlementPayout::new(UNIT_F64, ZERO_F64)
    } else {
        BinarySettlementPayout::new(ZERO_F64, UNIT_F64)
    }
}

pub fn maker_binary_fee_curve(fee_rate: f64, price: f64) -> Option<f64> {
    super::binary_outcome::maker_binary_fee_curve(fee_rate, price)
}

/// Updown rotating-cadence target block. Owned by the updown market-
/// family binding because `cadence_secs`, `underlying_asset`,
/// `rotating_market_family`, `cadence_slug_token`, and
/// `market_selection_rule` are family-shaped fields. The strategy
/// envelope (`crate::bolt_v3_config::
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
    pub cadence_slug_token: String,
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
    let mut value = target.clone();
    if let Some(table) = value.as_table_mut() {
        // Fill in an omitted `cadence_slug_token` from `cadence_secs` through the
        // shared, registry-dispatched family helper before typed deserialization,
        // so an operator never restates a token the updown runtime-contract fully
        // determines. The mapping lives in exactly one place
        // (`expected_cadence_slug_token`, wired into the registry binding); this
        // surface only plumbs its own field names. A provided token is left
        // untouched and still checked against the contract downstream.
        super::inject_derived_cadence_slug_token(
            table,
            stringify!(rotating_market_family),
            stringify!(cadence_secs),
        )?;
    }
    value
        .try_into::<TargetBlock>()
        .map_err(|error| error.to_string())
}

/// Family-specific structural validator for updown rotating-market
/// targets. Owns underlying-asset shape rules, cadence rules (via
/// `validate_target_cadence`), cadence slug-token shape, and the retry
/// / blocked positive-integer rules. Core startup validation in
/// `crate::bolt_v3_validate` dispatches the strategy envelope's raw
/// `[target]` value here via
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

    errors.extend(super::validate_underlying_asset(
        context,
        "target.underlying_asset",
        block.underlying_asset.as_str(),
    ));

    let cadence_errors = validate_target_cadence(context, block.cadence_secs);
    let token_errors = validate_cadence_slug_token(context, block.cadence_slug_token.as_str());
    errors.extend(cadence_errors.iter().cloned());
    errors.extend(token_errors.iter().cloned());
    if cadence_errors.is_empty() && token_errors.is_empty() {
        errors.extend(validate_cadence_slug_contract(
            context,
            block.cadence_secs,
            block.cadence_slug_token.as_str(),
        ));
    }

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
        if allowed_provider_kinds.is_empty()
            && market_mappings.iter().any(|mapping| {
                !(subscription.allow_no_resolution && mapping.resolution_kind == NO_RESOLUTION_KIND)
            })
        {
            errors.push(format!(
                "{subscription_path}.allowed_provider_kinds must list provider kinds when market_mappings contain provider-backed resolution"
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
/// targets. Owns the positive / minute-aligned rules so core startup
/// validation can stay structural and dispatch per-family cadence
/// policy here.
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
    } else if expected_cadence_slug_token(cadence_secs).is_none() {
        errors.push(format!(
            "{context}: target.cadence_secs must be one of the updown runtime-contract values {} (got {cadence_secs})",
            cadence_contract_values()
        ));
    }
    errors
}

fn validate_cadence_slug_token(context: &str, cadence_slug_token: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if cadence_slug_token.is_empty() {
        errors.push(format!(
            "{context}: target.cadence_slug_token must not be empty"
        ));
    } else if cadence_slug_token.chars().count() > 32 {
        errors.push(format!(
            "{context}: target.cadence_slug_token must be 1-32 characters (got {})",
            cadence_slug_token.chars().count()
        ));
    } else if !cadence_slug_token
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        errors.push(format!(
            "{context}: target.cadence_slug_token must use only lowercase ASCII letters and digits (got `{cadence_slug_token}`)"
        ));
    }
    errors
}

/// LOAD-TIME validation of a maker's operator-declared updown market target.
/// Reuses the family's own underlying/cadence/slug validators (one home per rule),
/// enforces the cadence->slug contract (a non-canonical slug would silently fail to
/// resolve at runtime, since selection derives the market from
/// `expected_cadence_slug_token`), and rejects the static-market override fields —
/// updown is a rotating-cadence family, so a `static_condition_id` /
/// `static_yes_outcome` / `static_no_outcome` on an updown declaration is a
/// misconfiguration that must fail closed at load rather than be silently ignored.
pub fn validate_maker_market_target(
    context: &str,
    target: MarketSelectionTarget<'_>,
) -> Vec<String> {
    let mut errors =
        super::validate_underlying_asset(context, "underlying_asset", target.underlying_asset);
    errors.extend(validate_target_cadence(context, target.cadence_seconds));
    errors.extend(validate_cadence_slug_token(
        context,
        target.cadence_slug_token,
    ));
    errors.extend(validate_cadence_slug_contract(
        context,
        target.cadence_seconds,
        target.cadence_slug_token,
    ));
    if target.static_condition_id.is_some()
        || target.static_yes_outcome.is_some()
        || target.static_no_outcome.is_some()
    {
        errors.push(format!(
            "{context}: static_condition_id/static_yes_outcome/static_no_outcome are not valid for the rotating-cadence `updown` family"
        ));
    }
    errors
}

fn validate_cadence_slug_contract(
    context: &str,
    cadence_secs: i64,
    cadence_slug_token: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(expected) = expected_cadence_slug_token(cadence_secs)
        && cadence_slug_token != expected
    {
        errors.push(format!(
            "{context}: target.cadence_slug_token must be `{expected}` when target.cadence_secs is {cadence_secs} (got `{cadence_slug_token}`)"
        ));
    }
    errors
}

/// The updown runtime-contract: the ONLY `(cadence_secs, cadence_slug_token)`
/// pairs. Single source for slug derivation (`expected_cadence_slug_token`,
/// wired into the family registry binding) AND the valid-cadence diagnostics
/// (`cadence_contract_values`); no caller restates the pairs or the list, so the
/// derivation and the "must be one of …" error can never drift apart.
const CADENCE_SLUG_CONTRACT: &[(i64, &str)] = &[
    (60, "1m"),
    (300, "5m"),
    (900, "15m"),
    (3600, "1h"),
    (14400, "4h"),
];

pub(crate) fn expected_cadence_slug_token(cadence_secs: i64) -> Option<&'static str> {
    CADENCE_SLUG_CONTRACT
        .iter()
        .find(|(secs, _)| *secs == cadence_secs)
        .map(|(_, token)| *token)
}

/// The contract's valid cadence values rendered for fail-closed diagnostics
/// (`"60, 300, 900, 3600, or 14400"`), derived from the single contract source.
fn cadence_contract_values() -> String {
    let values: Vec<String> = CADENCE_SLUG_CONTRACT
        .iter()
        .map(|(secs, _)| secs.to_string())
        .collect();
    match values.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, head)) => format!("{}, or {last}", head.join(", ")),
        None => String::new(),
    }
}

/// Pure identity facts for one configured updown rotating-market
/// target. Every value here is derived from validated configuration
/// only; nothing here depends on wall-clock time, the NT instrument
/// index, or any network call.
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

#[derive(Debug, Clone)]
struct UpdownOutcomeInstrument {
    side: OutcomeSide,
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
pub enum BoltV3MarketIdentityError {
    NonPositiveCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_secs: i64,
    },
    InvalidCadenceSlugToken {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_slug_token: String,
    },
    UnsupportedCadenceSeconds {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_secs: i64,
    },
    CadenceSlugTokenMismatch {
        strategy_instance_id: Option<String>,
        configured_target_id: Option<String>,
        cadence_secs: i64,
        cadence_slug_token: String,
        expected_cadence_slug_token: &'static str,
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
            BoltV3MarketIdentityError::InvalidCadenceSlugToken {
                strategy_instance_id,
                configured_target_id,
                cadence_slug_token,
            } => write!(
                f,
                "{prefix}target.cadence_slug_token must use only lowercase ASCII letters and digits (got `{cadence_slug_token}`)",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
            ),
            BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_secs,
            } => write!(
                f,
                "{prefix}target.cadence_secs must be one of the updown runtime-contract values {values} (got {cadence_secs})",
                prefix = format_target_prefix(strategy_instance_id, configured_target_id),
                values = cadence_contract_values(),
            ),
            BoltV3MarketIdentityError::CadenceSlugTokenMismatch {
                strategy_instance_id,
                configured_target_id,
                cadence_secs,
                cadence_slug_token,
                expected_cadence_slug_token,
            } => write!(
                f,
                "{prefix}target.cadence_slug_token must be `{expected_cadence_slug_token}` when target.cadence_secs is {cadence_secs} (got `{cadence_slug_token}`)",
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

impl std::error::Error for BoltV3MarketIdentityError {}

/// Project every configured strategy in `loaded` into an
/// `UpdownTargetPlan`. Returns the full `MarketIdentityPlan` in the
/// same sequence as the configured strategies. Fails loud if a
/// strategy's target has been mutated to bypass schema validation
/// (non-positive `cadence_secs` or invalid `cadence_slug_token`).
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
    if !validate_cadence_slug_token("", target.cadence_slug_token.as_str()).is_empty() {
        return Err(BoltV3MarketIdentityError::InvalidCadenceSlugToken {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_slug_token: target.cadence_slug_token.clone(),
        });
    }
    let Some(expected_cadence_slug_token) = expected_cadence_slug_token(target.cadence_secs) else {
        return Err(BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_secs: target.cadence_secs,
        });
    };
    if target.cadence_slug_token != expected_cadence_slug_token {
        return Err(BoltV3MarketIdentityError::CadenceSlugTokenMismatch {
            strategy_instance_id: Some(strategy_instance_id),
            configured_target_id: Some(configured_target_id),
            cadence_secs: target.cadence_secs,
            cadence_slug_token: target.cadence_slug_token.clone(),
            expected_cadence_slug_token,
        });
    }

    Ok(Some(UpdownTargetPlan {
        strategy_instance_id,
        configured_target_id,
        execution_client_id,
        underlying_asset: target.underlying_asset.clone(),
        cadence_secs: target.cadence_secs,
        cadence_slug_token: target.cadence_slug_token.clone(),
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

/// Registry-routed market-identity projector for the updown family.
/// The parent dispatcher
/// (`crate::bolt_v3_market_families::market_identity_plan_from_config_with_bindings`)
/// reads each strategy's `target.rotating_market_family` and only routes
/// matching strategies here, so this never sees a non-updown strategy.
/// Returns the projected target type-erased as `MarketIdentityTarget`
/// so the shared plan builder owns the single projection path.
pub fn plan_strategy_target(
    strategy: &LoadedStrategy,
) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError> {
    let target = plan_strategy_updown_target(strategy).map_err(plan_strategy_error)?;
    Ok(target.map(|target| -> Arc<dyn MarketIdentityTarget> { Arc::new(target) }))
}

fn plan_strategy_error(error: BoltV3MarketIdentityError) -> InstrumentFilterError {
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
        BoltV3MarketIdentityError::InvalidCadenceSlugToken { .. }
        | BoltV3MarketIdentityError::UnsupportedCadenceSeconds { .. }
        | BoltV3MarketIdentityError::CadenceSlugTokenMismatch { .. } => {
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
    if !validate_cadence_slug_token("", target.cadence_slug_token.as_str()).is_empty() {
        return Err(InstrumentFilterError::TargetValidationFailure {
            message: BoltV3MarketIdentityError::InvalidCadenceSlugToken {
                strategy_instance_id: None,
                configured_target_id: Some(target.configured_target_id.clone()),
                cadence_slug_token: target.cadence_slug_token.clone(),
            }
            .to_string(),
        });
    }
    if let Some(expected_cadence_slug_token) = expected_cadence_slug_token(target.cadence_secs) {
        if target.cadence_slug_token != expected_cadence_slug_token {
            return Err(InstrumentFilterError::TargetValidationFailure {
                message: BoltV3MarketIdentityError::CadenceSlugTokenMismatch {
                    strategy_instance_id: None,
                    configured_target_id: Some(target.configured_target_id.clone()),
                    cadence_secs: target.cadence_secs,
                    cadence_slug_token: target.cadence_slug_token.clone(),
                    expected_cadence_slug_token,
                }
                .to_string(),
            });
        }
    } else {
        return Err(InstrumentFilterError::TargetValidationFailure {
            message: BoltV3MarketIdentityError::UnsupportedCadenceSeconds {
                strategy_instance_id: None,
                configured_target_id: Some(target.configured_target_id.clone()),
                cadence_secs: target.cadence_secs,
            }
            .to_string(),
        });
    }
    Ok(TargetRuntimeFields {
        configured_target_id: target.configured_target_id,
        target_kind: target_runtime_string(target.kind)?,
        rotating_market_family: target_runtime_string(target.rotating_market_family)?,
        underlying_asset: target.underlying_asset,
        cadence_seconds: target.cadence_secs,
        cadence_seconds_source_field: "target.cadence_secs",
        cadence_slug_token: target.cadence_slug_token,
        market_selection_rule: target_runtime_string(target.market_selection_rule)?,
        static_condition_id: None,
        static_yes_outcome: None,
        static_no_outcome: None,
        static_fair_probability_source: None,
        retry_interval_seconds: target.retry_interval_secs,
        blocked_after_seconds: target.blocked_after_secs,
    })
}

/// Serialize a validated updown target discriminator enum to its TOML
/// string token. Validated callers always pass an enum that serializes
/// to a string, so this normally cannot fail; the fallible path replaces
/// the prior pair of `.expect()` panics so a latent serialization
/// mismatch surfaces as a fail-closed error instead of aborting the node.
fn target_runtime_string<T>(value: T) -> Result<String, InstrumentFilterError>
where
    T: serde::Serialize,
{
    toml::Value::try_from(value)
        .ok()
        .as_ref()
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| InstrumentFilterError::Other {
            message: TARGET_ENUM_SERIALIZE_FAILURE_MESSAGE.to_string(),
        })
}

pub fn select_market_from_instruments(
    target: UpdownSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedUpdownMarket> {
    // The cadence is config-validated positive and `now_milliseconds` comes from the
    // runtime clock, so neither step below can fail in practice. If one ever does, fail
    // LOUD (operator-visible `error!`) instead of a silent `None` that masks a clock /
    // overflow fault (P5-8). The signature stays `Option` for the same reason as
    // `select_binary_option_market_from_target_with_bindings` (P5-3): no `Result` refactor
    // of the live-money selection chain for a branch that cannot be reached.
    let Ok(now_unix_secs) = i64::try_from(Duration::from_millis(now_milliseconds).as_secs()) else {
        log::error!(
            "bolt-v3 updown selection: now_milliseconds={now_milliseconds} overflows i64 seconds; selecting no market"
        );
        return None;
    };
    let (current_start, next_start) = match updown_period_pair(target.cadence_secs, now_unix_secs) {
        Ok(pair) => pair,
        Err(error) => {
            log::error!(
                "bolt-v3 updown selection: period-pair failed (cadence_secs={}, now_unix_secs={now_unix_secs}); selecting no market: {error}",
                target.cadence_secs
            );
            return None;
        }
    };
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
    // DEFERRED FAIL-CLOSED INVARIANT (P5-5, multi-venue): this guards only that
    // the selected up/down outcomes share ONE venue (self-consistency). It does
    // NOT yet assert that venue equals the strategy's configured EXECUTION venue
    // (`root.clients[execution_client_id].venue`). Under the current single-venue
    // (Polymarket-only) config a cross-venue collision cannot occur, so this is
    // unreachable today. When a second venue's instruments can coexist in the NT
    // cache, selection must be scoped to the execution venue AND this must assert
    // the selected venue equals it (fail closed). Tracked for the multi-venue
    // workstream; see specs/024-production-trade-readiness/external-review/P5-adjudication.md (P5-5).
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

/// Family-owned binary up/down fair-value model.
///
/// Black-Scholes digital: under risk-neutral GBM the probability the
/// underlying finishes above the strike is `N(d2)`, where
/// `d2 = (ln(S/K) - sigma_eff^2/2 * T) / (sigma_eff * sqrt(T))`. The
/// realized-volatility estimate is widened by a kurtosis term
/// (`sigma_eff = realized_vol * (1 + kurtosis / 6)`) so fat-tailed
/// regimes price wider. Invalid degenerate inputs fail closed
/// (return `None`); zero effective volatility returns the deterministic
/// expiry-limit probability.
pub fn fair_probability_up(inputs: &FairProbabilityInputs) -> Option<f64> {
    if !is_positive_finite(inputs.spot_price)
        || !is_positive_finite(inputs.strike_price)
        || !is_non_negative_finite(inputs.realized_vol)
        || !inputs.pricing_kurtosis.is_finite()
    {
        return None;
    }

    let time_to_expiry_years = inputs.seconds_to_market_end as f64 / SECONDS_PER_YEAR_F64;
    if time_to_expiry_years <= ZERO_F64 {
        return None;
    }

    let sigma_eff =
        inputs.realized_vol * (UNIT_F64 + inputs.pricing_kurtosis / KURTOSIS_NORMALIZATION);
    if !is_non_negative_finite(sigma_eff) {
        return None;
    }
    if sigma_eff == ZERO_F64 {
        return Some(deterministic_up_probability(
            inputs.spot_price,
            inputs.strike_price,
        ));
    }

    let d2 = ((inputs.spot_price / inputs.strike_price).ln()
        - (sigma_eff.powi(POWER_OF_TWO) / SIGMA_SQUARED_HALF_DIVISOR) * time_to_expiry_years)
        / (sigma_eff * time_to_expiry_years.sqrt());
    sanitize_probability(standard_normal_cdf(d2))
}

fn deterministic_up_probability(spot_price: f64, strike_price: f64) -> f64 {
    if spot_price > strike_price {
        UNIT_F64
    } else if spot_price < strike_price {
        ZERO_F64
    } else {
        HALF_F64
    }
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
        BoltV3MarketIdentityError::InvalidCadenceSlugToken { .. }
        | BoltV3MarketIdentityError::UnsupportedCadenceSeconds { .. }
        | BoltV3MarketIdentityError::CadenceSlugTokenMismatch { .. }
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
            OutcomeSide::Up if pair.up.is_none() => pair.up = Some(outcome),
            OutcomeSide::Down if pair.down.is_none() => pair.down = Some(outcome),
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

    let period_start_milliseconds =
        u64::try_from(Duration::from_secs(u64::try_from(period_start_unix_secs).ok()?).as_millis())
            .ok()?;
    let start_timestamp_milliseconds = period_start_milliseconds
        .max(up.activation_milliseconds)
        .max(down.activation_milliseconds);
    // The live resolution strike is queried at this interval-open boundary in
    // whole seconds (Chainlink Data Streams reports are second-resolution), and
    // the strategy requires the returned report's `valid_from` to equal this
    // millisecond boundary exactly. A boundary carrying a sub-second component
    // (a non-second-aligned instrument activation surfaced via `.max` above)
    // could never bind a strike, so reject the candidate fail-closed rather than
    // select a market that can never trade.
    if !start_timestamp_milliseconds.is_multiple_of(MILLIS_PER_SECOND_U64) {
        return None;
    }

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
        "Up" => OutcomeSide::Up,
        "Down" => OutcomeSide::Down,
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

    use nautilus_core::Params;
    use nautilus_model::{
        enums::AssetClass,
        identifiers::Symbol,
        instruments::BinaryOption,
        types::{Currency, Price, Quantity},
    };

    const TEST_CONFIGURED_TARGET_ID: &str = "configured_updown_target";
    const TEST_UNDERLYING_ASSET: &str = "CONFIGUREDASSET";
    const TEST_CADENCE_SLUG_TOKEN: &str = "5m";
    const TEST_MARKET_SLUG: &str = "configuredasset-updown-5m-600";
    const TEST_UP_INSTRUMENT_ID: &str = "configured-condition-UP.POLYMARKET";
    const TEST_DOWN_INSTRUMENT_ID: &str = "configured-condition-DOWN.POLYMARKET";
    const TEST_CONDITION_ID: &str = "configured-condition";

    fn selected_market_fixture() -> SelectedBinaryOptionMarket {
        SelectedBinaryOptionMarket {
            market_id: "market-1".to_string(),
            instrument_id: InstrumentId::from(TEST_UP_INSTRUMENT_ID),
            up_instrument_id: InstrumentId::from(TEST_UP_INSTRUMENT_ID),
            down_instrument_id: InstrumentId::from(TEST_DOWN_INSTRUMENT_ID),
            selection_outcome: MarketSelectionOutcome::Current,
            start_timestamp_milliseconds: 600_000,
            expiration_timestamp_milliseconds: 900_000,
            seconds_to_end: 300,
            source_identity: SelectedMarketSourceIdentity {
                condition_id: TEST_CONDITION_ID.to_string(),
                market_slug: TEST_MARKET_SLUG.to_string(),
                question_id: "question-1".to_string(),
            },
        }
    }

    fn target_with_resolution_mapping() -> toml::Value {
        toml::toml! {
            configured_target_id = "configured_updown_target"
            kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "CONFIGUREDASSET"
            cadence_secs = 300
            cadence_slug_token = "5m"
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
            resolution_identity = "configured-reference-price"
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

        assert_eq!(requirement.configured_target_id, TEST_CONFIGURED_TARGET_ID);
        assert_eq!(requirement.venue, "POLYMARKET");
        assert_eq!(requirement.family_key, KEY);
        assert_eq!(requirement.market_id, "market-1");
        assert_eq!(
            requirement.instrument_ids,
            vec![TEST_DOWN_INSTRUMENT_ID, TEST_UP_INSTRUMENT_ID]
        );
        assert_eq!(requirement.market_class, "binary_option");
        assert_eq!(requirement.resolution_kind, "chainlink_data_streams");
        assert_eq!(
            requirement.resolution_identity,
            "configured-reference-price"
        );
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
        assert_eq!(
            requirement.resolution_identity,
            "configured-reference-price"
        );
        assert_eq!(
            requirement.instrument_ids,
            vec![TEST_DOWN_INSTRUMENT_ID, TEST_UP_INSTRUMENT_ID]
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
                toml::Value::String("configured-backup-resolution".to_string()),
            );
        resolution_mapping_array_mut(&mut target).push(alternate);

        assert_selected_market_requirement_error(target, "ambiguous");
    }

    #[test]
    fn selected_market_requirement_fails_when_selected_instrument_venues_differ() {
        let mut selected = selected_market_fixture();
        selected.down_instrument_id = InstrumentId::from("configured-condition-DOWN.SIM");

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
                toml::Value::String("configured|resolution".to_string()),
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
    fn selected_updown_market_start_uses_configured_period_not_gamma_creation_time() {
        let market_slug = updown_market_slug(TEST_UNDERLYING_ASSET, TEST_CADENCE_SLUG_TOKEN, 600);
        let instruments = vec![
            test_binary_option(
                "configured-condition-up.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Up",
                100_000,
                900_000,
            ),
            test_binary_option(
                "configured-condition-down.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Down",
                100_000,
                900_000,
            ),
        ];

        let selected = select_market_from_instruments(
            UpdownSelectionTarget {
                underlying_asset: TEST_UNDERLYING_ASSET,
                cadence_secs: 300,
                cadence_slug_token: TEST_CADENCE_SLUG_TOKEN,
            },
            &instruments,
            600_001,
        )
        .expect("configured current updown market should select");

        assert_eq!(selected.start_timestamp_milliseconds, 600_000);
    }

    #[test]
    fn selected_updown_market_start_preserves_later_instrument_activation() {
        let market_slug = updown_market_slug(TEST_UNDERLYING_ASSET, TEST_CADENCE_SLUG_TOKEN, 600);
        let instruments = vec![
            test_binary_option(
                "configured-condition-up.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Up",
                650_000,
                900_000,
            ),
            test_binary_option(
                "configured-condition-down.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Down",
                660_000,
                900_000,
            ),
        ];

        let selected = select_market_from_instruments(
            UpdownSelectionTarget {
                underlying_asset: TEST_UNDERLYING_ASSET,
                cadence_secs: 300,
                cadence_slug_token: TEST_CADENCE_SLUG_TOKEN,
            },
            &instruments,
            600_001,
        )
        .expect("configured current updown market should select");

        assert_eq!(selected.start_timestamp_milliseconds, 660_000);
    }

    #[test]
    fn selected_updown_market_rejects_non_second_aligned_open_boundary() {
        // F7b regression lock. The live Chainlink resolution strike is queried at
        // the interval-open boundary in whole seconds (the Data Streams "report at
        // T" endpoint is second-resolution); the strategy derives that second by
        // truncating `start_timestamp_milliseconds / 1000` and then requires the
        // returned report's `valid_from` to equal the original millisecond
        // boundary. When `start_timestamp_milliseconds` carries a sub-second
        // component (an instrument activation that is not second-aligned, surfaced
        // through `.max(activation_milliseconds)`), that ms->s->ms round-trip can
        // never match and `price_to_beat` is stranded for the market's whole life.
        // Such a market must be rejected fail-closed at selection, not selected and
        // then silently never traded.
        let market_slug = updown_market_slug(TEST_UNDERLYING_ASSET, TEST_CADENCE_SLUG_TOKEN, 600);
        let instruments = vec![
            test_binary_option(
                "configured-condition-up.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Up",
                650_000,
                900_000,
            ),
            test_binary_option(
                "configured-condition-down.POLYMARKET",
                &market_slug,
                "market-1",
                TEST_CONDITION_ID,
                "question-1",
                "Down",
                660_500,
                900_000,
            ),
        ];

        let selected = select_market_from_instruments(
            UpdownSelectionTarget {
                underlying_asset: TEST_UNDERLYING_ASSET,
                cadence_secs: 300,
                cadence_slug_token: TEST_CADENCE_SLUG_TOKEN,
            },
            &instruments,
            600_001,
        );

        assert!(
            selected.is_none(),
            "a market whose interval-open boundary is not second-aligned (660_500 ms) must be \
             rejected fail-closed; no second-resolution Chainlink strike can ever bind it",
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
            updown_market_slug("ASSET", "5m", 1_700_000_000),
            "asset-updown-5m-1700000000"
        );
        assert_eq!(
            updown_market_slug("ALT", "1h", 1_700_003_600),
            "alt-updown-1h-1700003600"
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
    fn validate_target_block_rejects_cadence_slug_token_contract_mismatch() {
        let mut target = target_with_resolution_mapping();
        target
            .as_table_mut()
            .expect("target should be a table")
            .insert(
                "cadence_slug_token".to_string(),
                toml::Value::String("configuredwindow".to_string()),
            );

        let errors = validate_target_block("strategy `configured_updown_main`", &target);
        assert!(
            errors.iter().any(|message| {
                message.contains("target.cadence_slug_token")
                    && message.contains("must be `5m`")
                    && message.contains("configuredwindow")
            }),
            "expected cadence/token contract mismatch, got: {errors:#?}"
        );
    }

    #[test]
    fn target_block_uses_configured_cadence_slug_token() {
        let target: TargetBlock = target_with_resolution_mapping()
            .try_into()
            .expect("target should deserialize");
        assert_eq!(target.cadence_slug_token, TEST_CADENCE_SLUG_TOKEN);
    }

    #[allow(clippy::too_many_arguments)]
    fn test_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        condition_id: &str,
        question_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        info.insert(
            "market_slug".to_string(),
            serde_json::Value::String(market_slug.to_string()),
        );
        info.insert(
            "market_id".to_string(),
            serde_json::Value::String(market_id.to_string()),
        );
        info.insert(
            "condition_id".to_string(),
            serde_json::Value::String(condition_id.to_string()),
        );
        info.insert(
            "question_id".to_string(),
            serde_json::Value::String(question_id.to_string()),
        );
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(1_000_000)).into(),
            (expiration_ms.saturating_mul(1_000_000)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(info),
            1.into(),
            1.into(),
        ))
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
    }

    #[test]
    fn fair_probability_accepts_zero_volatility_as_deterministic_limit() {
        assert_eq!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_101.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 60,
                realized_vol: 0.0,
                pricing_kurtosis: 0.0,
            }),
            Some(UNIT_F64)
        );
        assert_eq!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_099.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 60,
                realized_vol: 0.0,
                pricing_kurtosis: 0.0,
            }),
            Some(ZERO_F64)
        );
        assert_eq!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_100.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 60,
                realized_vol: 0.0,
                pricing_kurtosis: 0.0,
            }),
            Some(HALF_F64)
        );
    }

    #[test]
    fn fair_probability_fails_closed_on_invalid_inputs() {
        assert!(
            fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_100.0,
                strike_price: 3_100.0,
                seconds_to_market_end: 60,
                realized_vol: -0.01,
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

    fn target_without_cadence_slug_token() -> toml::Value {
        let mut target = target_with_resolution_mapping();
        target
            .as_table_mut()
            .expect("target fixture is a table")
            .remove(stringify!(cadence_slug_token));
        target
    }

    #[test]
    fn deserialize_target_block_derives_omitted_cadence_slug_token() {
        // Fixture cadence_secs = 300 ⇒ contract slug "5m"; with the operator token
        // omitted the shared seam must fill it in before typed deserialization.
        let target = target_without_cadence_slug_token();
        let block = deserialize_target_block(&target)
            .expect("an omitted cadence_slug_token must derive from cadence_secs");
        assert_eq!(block.cadence_slug_token, TEST_CADENCE_SLUG_TOKEN);
    }

    #[test]
    fn deserialize_target_block_preserves_explicit_cadence_slug_token() {
        // The fixture's cadence_secs = 300 would derive "5m", so a deliberately
        // different supplied token proves the seam never clobbers operator input.
        let mut target = target_with_resolution_mapping();
        target
            .as_table_mut()
            .expect("target fixture is a table")
            .insert(
                stringify!(cadence_slug_token).to_string(),
                toml::Value::String("operator_choice".to_string()),
            );
        let block =
            deserialize_target_block(&target).expect("an explicit token deserializes verbatim");
        assert_eq!(block.cadence_slug_token, "operator_choice");
    }

    #[test]
    fn deserialize_target_block_rejects_omitted_token_for_non_contract_cadence() {
        let mut target = target_without_cadence_slug_token();
        target
            .as_table_mut()
            .expect("target fixture is a table")
            .insert(
                stringify!(cadence_secs).to_string(),
                toml::Value::Integer(120),
            );
        let error = deserialize_target_block(&target)
            .expect_err("a non-contract cadence with no token must fail closed");
        assert!(
            error.contains("cadence_secs=120") && error.contains("cadence_slug_token is required"),
            "error must name the offending cadence: {error}"
        );
    }

    #[test]
    fn deserialize_target_block_still_rejects_unknown_fields() {
        // The raw-table preprocessing must not weaken `deny_unknown_fields`.
        let mut target = target_with_resolution_mapping();
        target
            .as_table_mut()
            .expect("target fixture is a table")
            .insert("unexpected_field".to_string(), toml::Value::Boolean(true));
        let error =
            deserialize_target_block(&target).expect_err("an unknown field must still be rejected");
        assert!(
            error.contains("unexpected_field") || error.contains("unknown field"),
            "deny_unknown_fields must reject stray keys: {error}"
        );
    }

    #[test]
    fn cadence_slug_contract_matches_independent_pins() {
        // Authoritative bidirectional drift guard, colocated with the single
        // source it protects. The seam tests in the parent module iterate a
        // pinned list, so they catch a CHANGED or REMOVED pair but cannot catch
        // an ADDED cadence. Comparing the whole slice against an independent
        // restatement fails closed in every direction: a changed token, an added
        // cadence, or a removed cadence all break this.
        const PINS: &[(i64, &str)] = &[
            (60, "1m"),
            (300, "5m"),
            (900, "15m"),
            (3600, "1h"),
            (14400, "4h"),
        ];
        assert_eq!(
            CADENCE_SLUG_CONTRACT, PINS,
            "updown cadence->slug contract drifted from its pinned expectation"
        );
    }

    #[test]
    fn target_runtime_fields_from_target_inherits_derived_cadence_slug_token() {
        // The taker holds no cadence_slug_token derivation of its own; it inherits
        // the derived value through `target_runtime_fields_from_target` -- the exact
        // dispatcher `raw_taker_config` calls -- which routes the updown family
        // through `deserialize_target_block`'s shared seam. Omitting the operator
        // token must still surface the derived slug in the runtime fields the taker
        // copies into its config, so deleting the taker's old per-config derivation
        // stays safe.
        let target = target_without_cadence_slug_token();
        let runtime = crate::bolt_v3_market_families::target_runtime_fields_from_target(&target)
            .expect("dispatcher must derive the omitted cadence_slug_token for the updown family");
        assert_eq!(runtime.cadence_slug_token, TEST_CADENCE_SLUG_TOKEN);
    }
}
