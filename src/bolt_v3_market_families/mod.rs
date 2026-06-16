//! Rotating-market target parsing for bolt-v3 strategy config.
//!
//! Each supported `target.rotating_market_family` has a module here
//! that owns its typed `[target]` fields, cadence checks, slug
//! construction, and instrument-filter errors.

mod binary_outcome;
pub mod hyperliquid_instrument;
pub mod outcome_group;
pub mod static_binary_event;
pub mod updown;

pub use updown::TargetGateSubscription;

use std::{any::Any, collections::BTreeMap, fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_instrument_filters::InstrumentFilterError,
    bolt_v3_maker_settlement::BinarySettlementPayout,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::{FamilyQuoteInputs, QuoteTargets},
};
use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};

/// Canonical binary-market outcome side (`Up`/`Down`). Homed in the
/// market-family layer as the single source of truth: updown family
/// instruments are keyed by it, and the taker decision math
/// (`bolt_v3_taker_updown_signal`) consumes it. Variants and derives match the
/// prior family-local `UpdownOutcomeSide` and the strategy-local
/// `OutcomeSide` it replaces (pure type-identity unification).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSide {
    Up,
    Down,
}

/// Target metadata read by startup validation before dispatching to a
/// `target.rotating_market_family` validator.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetMetadata {
    pub configured_target_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetFamilyDispatch {
    rotating_market_family: String,
}

/// Per-strategy market-identity projector signature. The parent
/// dispatcher (`market_identity_plan_from_config_with_bindings`) reads
/// each strategy's `target.rotating_market_family` first and routes only
/// the matching strategies into this function; family bindings never see
/// strategies from a different family, so a future non-updown strategy
/// cannot fail inside the updown binding's typed deserialization.
/// Returns a type-erased `MarketIdentityTarget` so the shared plan
/// builder owns the single projection path and no family is dispatched by
/// a hardcoded call.
pub type PlanStrategyTargetFn =
    fn(&LoadedStrategy) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError>;

#[derive(Clone)]
pub struct MarketFamilyValidationBinding {
    pub key: &'static str,
    pub validate_target: fn(&str, &toml::Value) -> Vec<String>,
    pub plan_strategy_target: PlanStrategyTargetFn,
    pub target_runtime_fields:
        fn(&toml::Value) -> Result<TargetRuntimeFields, InstrumentFilterError>,
    pub select_binary_option_market:
        fn(MarketSelectionTarget<'_>, &[InstrumentAny], u64) -> Option<SelectedBinaryOptionMarket>,
    pub market_selection_candidate_windows:
        fn(
            MarketSelectionTarget<'_>,
            u64,
        ) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError>,
    pub selected_market_requirement: fn(
        &toml::Value,
        &SelectedBinaryOptionMarket,
        u64,
    )
        -> Result<SelectedMarketRequirement, InstrumentFilterError>,
    /// Family-owned fair-value model. The binary up/down pricing math
    /// is instrument-type-specific, so it lives at the family seam
    /// rather than inlined in a strategy. Returns the model's fair
    /// probability that the underlying finishes *up*, or `None` when
    /// the family's inputs are degenerate (the strategy already treats
    /// `None` as "pricing unavailable").
    pub fair_probability_up: fn(&FairProbabilityInputs) -> Option<f64>,
    pub maker_quote_targets: fn(FamilyQuoteInputs) -> Option<QuoteTargets>,
    pub maker_settlement_payout: fn(BinarySettlementPayout, Leg) -> Option<f64>,
    pub maker_settlement_payout_from_reference_prices:
        fn(f64, f64) -> Option<BinarySettlementPayout>,
    pub maker_binary_fee_curve: fn(f64, f64) -> Option<f64>,
}

/// Shared pricing contract handed to a family's fair-value model.
///
/// These inputs are family-agnostic: a spot reference, the binary's
/// strike, the time remaining, and the realized-volatility / kurtosis
/// estimates the strategy already maintains. The family binding owns
/// how it turns them into a fair probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FairProbabilityInputs {
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
    pub pricing_kurtosis: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketSelectionTarget<'a> {
    pub family_key: &'a str,
    pub underlying_asset: &'a str,
    pub cadence_seconds: i64,
    pub cadence_slug_token: &'a str,
    pub static_condition_id: Option<&'a str>,
    pub static_yes_outcome: Option<&'a str>,
    pub static_no_outcome: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedBinaryOptionMarket {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMarketSourceIdentity {
    pub condition_id: String,
    pub market_slug: String,
    pub question_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedMarketRequirement {
    pub configured_target_id: String,
    pub venue: String,
    pub family_key: String,
    pub market_id: String,
    pub instrument_ids: Vec<String>,
    pub market_class: String,
    pub resolution_kind: String,
    pub resolution_identity: String,
    pub value_kind: String,
    pub metadata_provenance_sha256: String,
    pub selected_market_key: String,
    pub selected_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectedMarketRequirementParts<'a> {
    pub configured_target_id: &'a str,
    pub venue: &'a str,
    pub family_key: &'a str,
    pub market_id: &'a str,
    pub instrument_ids: Vec<String>,
    pub market_class: &'a str,
    pub resolution_kind: &'a str,
    pub resolution_identity: &'a str,
    pub value_kind: &'a str,
    pub metadata_provenance_fields: BTreeMap<String, serde_json::Value>,
    pub selected_at_ms: u64,
}

const SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD: &str = "configured_target_id";
const SELECTED_MARKET_FAMILY_KEY_FIELD: &str = "family_key";
const SELECTED_MARKET_INSTRUMENT_IDS_FIELD: &str = "instrument_ids";
const SELECTED_MARKET_MARKET_CLASS_FIELD: &str = "market_class";
const SELECTED_MARKET_MARKET_ID_FIELD: &str = "market_id";
const SELECTED_MARKET_METADATA_PROVENANCE_FIELD: &str = "metadata_provenance";
const SELECTED_MARKET_METADATA_PROVENANCE_KEY_FIELD: &str = "metadata_provenance key";
const SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD: &str = "metadata_provenance_sha256";
const SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const SELECTED_MARKET_RESOLUTION_KIND_FIELD: &str = "resolution_kind";
const SELECTED_MARKET_VALUE_KIND_FIELD: &str = "value_kind";
const SELECTED_MARKET_VENUE_FIELD: &str = "venue";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketSelectionOutcome {
    Current,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketSelectionCandidateWindow {
    pub outcome: MarketSelectionOutcome,
    pub market_slug: String,
    pub start_timestamp_milliseconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRuntimeFields {
    pub configured_target_id: String,
    pub target_kind: String,
    pub rotating_market_family: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_seconds_source_field: &'static str,
    pub cadence_slug_token: String,
    pub market_selection_rule: String,
    pub static_condition_id: Option<String>,
    pub static_yes_outcome: Option<String>,
    pub static_no_outcome: Option<String>,
    pub static_fair_probability_source: Option<String>,
    pub retry_interval_seconds: u64,
    pub blocked_after_seconds: u64,
}

pub trait MarketIdentityTarget: fmt::Debug + Send + Sync + Any {
    fn family_key(&self) -> &'static str;
    fn configured_target_id(&self) -> &str;
    fn execution_client_id(&self) -> &str;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone)]
pub struct MarketIdentityPlan {
    targets: Vec<Arc<dyn MarketIdentityTarget>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketIdentityPlanError {
    message: String,
}

impl MarketIdentityPlanError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for MarketIdentityPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MarketIdentityPlanError {}

pub struct MarketIdentityExecutionClientTargetRef<'a> {
    pub family_key: &'static str,
    pub configured_target_id: &'a str,
    pub execution_client_id: &'a str,
}

impl MarketIdentityPlan {
    pub fn empty() -> Self {
        Self {
            targets: Vec::new(),
        }
    }

    pub fn push_target<T>(&mut self, target: T)
    where
        T: MarketIdentityTarget + 'static,
    {
        self.targets.push(Arc::new(target));
    }

    pub fn push_arc_target(&mut self, target: Arc<dyn MarketIdentityTarget>) {
        self.targets.push(target);
    }

    pub fn targets(&self) -> impl Iterator<Item = &dyn MarketIdentityTarget> {
        self.targets.iter().map(Arc::as_ref)
    }

    pub fn execution_client_target_refs(
        &self,
    ) -> impl Iterator<Item = MarketIdentityExecutionClientTargetRef<'_>> {
        self.targets()
            .map(|target| MarketIdentityExecutionClientTargetRef {
                family_key: target.family_key(),
                configured_target_id: target.configured_target_id(),
                execution_client_id: target.execution_client_id(),
            })
    }
}

const VALIDATION_BINDINGS: &[MarketFamilyValidationBinding] = &[
    MarketFamilyValidationBinding {
        key: updown::KEY,
        validate_target: updown::validate_target_block,
        plan_strategy_target: updown::plan_strategy_target,
        target_runtime_fields: updown::target_runtime_fields,
        select_binary_option_market: updown::select_binary_option_market,
        market_selection_candidate_windows: updown::market_selection_candidate_windows,
        selected_market_requirement: updown::selected_market_requirement,
        fair_probability_up: updown::fair_probability_up,
        maker_quote_targets: updown::maker_quote_targets,
        maker_settlement_payout: updown::maker_settlement_payout,
        maker_settlement_payout_from_reference_prices:
            updown::maker_settlement_payout_from_reference_prices,
        maker_binary_fee_curve: updown::maker_binary_fee_curve,
    },
    MarketFamilyValidationBinding {
        key: outcome_group::KEY,
        validate_target: outcome_group::validate_target_block,
        plan_strategy_target: outcome_group::plan_strategy_target,
        target_runtime_fields: outcome_group::target_runtime_fields,
        select_binary_option_market: outcome_group::select_binary_option_market,
        market_selection_candidate_windows: outcome_group::market_selection_candidate_windows,
        selected_market_requirement: outcome_group::selected_market_requirement,
        fair_probability_up: outcome_group::fair_probability_up,
        maker_quote_targets: unsupported_maker_quote_targets,
        maker_settlement_payout: unsupported_maker_settlement_payout,
        maker_binary_fee_curve: unsupported_maker_binary_fee_curve,
    },
    MarketFamilyValidationBinding {
        key: static_binary_event::KEY,
        validate_target: static_binary_event::validate_target_block,
        plan_strategy_target: static_binary_event::plan_strategy_target,
        target_runtime_fields: static_binary_event::target_runtime_fields,
        select_binary_option_market: static_binary_event::select_binary_option_market,
        market_selection_candidate_windows: static_binary_event::market_selection_candidate_windows,
        selected_market_requirement: static_binary_event::selected_market_requirement,
        fair_probability_up: static_binary_event::fair_probability_up,
        maker_quote_targets: static_binary_event::maker_quote_targets,
        maker_settlement_payout: static_binary_event::maker_settlement_payout,
        maker_settlement_payout_from_reference_prices:
            unsupported_maker_settlement_payout_from_reference_prices,
        maker_binary_fee_curve: static_binary_event::maker_binary_fee_curve,
    },
    MarketFamilyValidationBinding {
        key: hyperliquid_instrument::KEY,
        validate_target: hyperliquid_instrument::validate_target_block,
        plan_strategy_target: hyperliquid_instrument::plan_strategy_target,
        target_runtime_fields: hyperliquid_instrument::target_runtime_fields,
        select_binary_option_market: hyperliquid_instrument::select_binary_option_market,
        market_selection_candidate_windows:
            hyperliquid_instrument::market_selection_candidate_windows,
        selected_market_requirement: hyperliquid_instrument::selected_market_requirement,
        fair_probability_up: hyperliquid_instrument::fair_probability_up,
        maker_quote_targets: unsupported_maker_quote_targets,
        maker_settlement_payout: unsupported_maker_settlement_payout,
        maker_settlement_payout_from_reference_prices:
            unsupported_maker_settlement_payout_from_reference_prices,
        maker_binary_fee_curve: unsupported_maker_binary_fee_curve,
    },
];

pub fn validation_bindings() -> &'static [MarketFamilyValidationBinding] {
    VALIDATION_BINDINGS
}

pub fn static_binary_event_family_key() -> &'static str {
    static_binary_event::KEY
}

pub fn static_binary_event_reference_current_price_fair_probability_source() -> &'static str {
    static_binary_event::REFERENCE_CURRENT_PRICE_FAIR_PROBABILITY_SOURCE
}

fn unsupported_maker_quote_targets(_inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
    None
}

fn unsupported_maker_settlement_payout(_payout: BinarySettlementPayout, _leg: Leg) -> Option<f64> {
    None
}

fn unsupported_maker_settlement_payout_from_reference_prices(
    _close_price: f64,
    _strike_price: f64,
) -> Option<BinarySettlementPayout> {
    None
}

fn unsupported_maker_binary_fee_curve(_fee_rate: f64, _price: f64) -> Option<f64> {
    None
}

pub fn market_identity_plan_from_config(
    loaded: &LoadedBoltV3Config,
) -> Result<MarketIdentityPlan, MarketIdentityPlanError> {
    market_identity_plan_from_config_with_bindings(loaded, validation_bindings())
        .map_err(|error| MarketIdentityPlanError::new(error.to_string()))
}

/// Build the market-identity plan by routing every configured strategy
/// through the family registry. Each strategy's
/// `target.rotating_market_family` selects the binding that owns its
/// projection, so no family is dispatched by a hardcoded call and an
/// unknown family fails loud as `UnsupportedFamily` (the same fail-loud
/// policy the sibling target dispatchers use).
pub fn market_identity_plan_from_config_with_bindings(
    loaded: &LoadedBoltV3Config,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<MarketIdentityPlan, InstrumentFilterError> {
    let mut plan = MarketIdentityPlan::empty();
    for strategy in &loaded.strategies {
        let dispatch: TargetFamilyDispatch =
            strategy.config.target.clone().try_into().map_err(|error| {
                InstrumentFilterError::TargetParseFailed {
                    strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                    message: format!("target: {error}"),
                }
            })?;
        let binding = bindings
            .iter()
            .find(|binding| binding.key == dispatch.rotating_market_family)
            .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
                context: Some(format!(
                    "strategy `{}`",
                    strategy.config.strategy_instance_id
                )),
                family_key: dispatch.rotating_market_family.clone(),
                supported: bindings.iter().map(|b| b.key).collect(),
            })?;
        if let Some(target) = (binding.plan_strategy_target)(strategy)? {
            plan.push_arc_target(target);
        }
    }
    Ok(plan)
}

pub fn target_runtime_fields_from_target(
    target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    target_runtime_fields_from_target_with_bindings(target, validation_bindings())
}

pub fn target_runtime_fields_from_target_with_bindings(
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    let dispatch: TargetFamilyDispatch =
        target
            .clone()
            .try_into()
            .map_err(|error| InstrumentFilterError::Other {
                message: format!("target: {error}"),
            })?;
    bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.target_runtime_fields)(target))
}

pub fn select_binary_option_market_from_target(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    select_binary_option_market_from_target_with_bindings(
        target,
        instruments,
        now_milliseconds,
        validation_bindings(),
    )
}

pub fn select_binary_option_market_from_target_with_bindings(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<SelectedBinaryOptionMarket> {
    // An unknown `family_key` must never reach here: startup validation (P5-10) rejects
    // an unregistered `rotating_market_family` at config load. If it somehow does, fail
    // LOUD — an `error!` an operator can see — rather than a silent `None` that is
    // indistinguishable from "no market this cycle" and masks the broken invariant
    // (P5-3). The signature stays `Option` so the live-money strategy/operator selection
    // chain is not refactored to `Result` for a branch that cannot be reached.
    let Some(binding) = bindings
        .iter()
        .find(|binding| binding.key == target.family_key)
    else {
        log::error!(
            "bolt-v3 market selection: no registered family binding for `{}` (config validation should have rejected this before runtime); selecting no market",
            target.family_key
        );
        return None;
    };
    (binding.select_binary_option_market)(target, instruments, now_milliseconds)
}

pub fn fair_probability_up_for_family(
    family_key: &str,
    inputs: &FairProbabilityInputs,
) -> Option<f64> {
    fair_probability_up_for_family_with_bindings(family_key, inputs, validation_bindings())
}

pub fn fair_probability_up_for_family_with_bindings(
    family_key: &str,
    inputs: &FairProbabilityInputs,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<f64> {
    bindings
        .iter()
        .find(|binding| binding.key == family_key)
        .and_then(|binding| (binding.fair_probability_up)(inputs))
}

pub fn maker_quote_targets_for_family(
    family_key: &str,
    inputs: FamilyQuoteInputs,
) -> Option<QuoteTargets> {
    maker_quote_targets_for_family_with_bindings(family_key, inputs, validation_bindings())
}

pub fn maker_quote_targets_for_family_with_bindings(
    family_key: &str,
    inputs: FamilyQuoteInputs,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<QuoteTargets> {
    bindings
        .iter()
        .find(|binding| binding.key == family_key)
        .and_then(|binding| (binding.maker_quote_targets)(inputs))
}

pub fn maker_settlement_payout_for_family(
    family_key: &str,
    payout: BinarySettlementPayout,
    leg: Leg,
) -> Option<f64> {
    maker_settlement_payout_for_family_with_bindings(family_key, payout, leg, validation_bindings())
}

pub fn maker_settlement_payout_for_family_with_bindings(
    family_key: &str,
    payout: BinarySettlementPayout,
    leg: Leg,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<f64> {
    bindings
        .iter()
        .find(|binding| binding.key == family_key)
        .and_then(|binding| (binding.maker_settlement_payout)(payout, leg))
}

pub fn maker_settlement_payout_from_reference_prices_for_family(
    family_key: &str,
    close_price: f64,
    strike_price: f64,
) -> Option<BinarySettlementPayout> {
    maker_settlement_payout_from_reference_prices_for_family_with_bindings(
        family_key,
        close_price,
        strike_price,
        validation_bindings(),
    )
}

pub fn maker_settlement_payout_from_reference_prices_for_family_with_bindings(
    family_key: &str,
    close_price: f64,
    strike_price: f64,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<BinarySettlementPayout> {
    bindings
        .iter()
        .find(|binding| binding.key == family_key)
        .and_then(|binding| {
            (binding.maker_settlement_payout_from_reference_prices)(close_price, strike_price)
        })
}

pub fn maker_binary_fee_curve_for_family(
    family_key: &str,
    fee_rate: f64,
    price: f64,
) -> Option<f64> {
    maker_binary_fee_curve_for_family_with_bindings(
        family_key,
        fee_rate,
        price,
        validation_bindings(),
    )
}

pub fn maker_binary_fee_curve_for_family_with_bindings(
    family_key: &str,
    fee_rate: f64,
    price: f64,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<f64> {
    bindings
        .iter()
        .find(|binding| binding.key == family_key)
        .and_then(|binding| (binding.maker_binary_fee_curve)(fee_rate, price))
}

pub fn market_selection_candidate_windows_from_target(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    market_selection_candidate_windows_from_target_with_bindings(
        target,
        now_milliseconds,
        validation_bindings(),
    )
}

pub fn market_selection_candidate_windows_from_target_with_bindings(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    bindings
        .iter()
        .find(|binding| binding.key == target.family_key)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: target.family_key.to_string(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.market_selection_candidate_windows)(target, now_milliseconds))
}

pub fn selected_market_requirement_from_target(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    selected_market_requirement_from_target_with_bindings(
        target,
        selected,
        selected_at_ms,
        validation_bindings(),
    )
}

pub fn selected_market_requirement_from_target_with_bindings(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    let dispatch: TargetFamilyDispatch =
        target
            .clone()
            .try_into()
            .map_err(|error| InstrumentFilterError::Other {
                message: format!("target: {error}"),
            })?;
    bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.selected_market_requirement)(target, selected, selected_at_ms))
}

pub(crate) fn selected_market_metadata_provenance_fields<I, K, V>(
    fields: I,
) -> BTreeMap<String, serde_json::Value>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    fields
        .into_iter()
        .map(|(key, value)| (key.into(), serde_json::Value::String(value.into())))
        .collect()
}

pub(crate) fn selected_market_requirement_from_parts(
    parts: SelectedMarketRequirementParts<'_>,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    ensure_selected_market_text(
        SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD,
        parts.configured_target_id,
        false,
    )?;
    ensure_selected_market_text(SELECTED_MARKET_VENUE_FIELD, parts.venue, false)?;
    ensure_selected_market_text(SELECTED_MARKET_FAMILY_KEY_FIELD, parts.family_key, false)?;
    ensure_selected_market_text(SELECTED_MARKET_MARKET_ID_FIELD, parts.market_id, false)?;
    ensure_selected_market_text(
        SELECTED_MARKET_MARKET_CLASS_FIELD,
        parts.market_class,
        false,
    )?;
    ensure_selected_market_text(
        SELECTED_MARKET_RESOLUTION_KIND_FIELD,
        parts.resolution_kind,
        false,
    )?;
    ensure_selected_market_text(
        SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD,
        parts.resolution_identity,
        false,
    )?;
    ensure_selected_market_text(SELECTED_MARKET_VALUE_KIND_FIELD, parts.value_kind, false)?;
    if parts.instrument_ids.is_empty() {
        return Err(selected_market_requirement_error(
            "selected-market instrument_ids must not be empty",
        ));
    }
    let mut previous_instrument_id = None;
    for instrument_id in &parts.instrument_ids {
        if let Some(previous) = previous_instrument_id
            && previous >= instrument_id
        {
            return Err(selected_market_requirement_error(
                "selected-market instrument_ids must be sorted and unique",
            ));
        }
        ensure_selected_market_text(SELECTED_MARKET_INSTRUMENT_IDS_FIELD, instrument_id, false)?;
        previous_instrument_id = Some(instrument_id);
    }
    for (key, value) in &parts.metadata_provenance_fields {
        ensure_selected_market_text(SELECTED_MARKET_METADATA_PROVENANCE_KEY_FIELD, key, false)?;
        ensure_selected_market_json_text(SELECTED_MARKET_METADATA_PROVENANCE_FIELD, value)?;
    }

    let metadata_provenance_sha256 = canonical_json_sha256(&parts.metadata_provenance_fields)?;
    let mut requirement = SelectedMarketRequirement {
        configured_target_id: parts.configured_target_id.to_string(),
        venue: parts.venue.to_string(),
        family_key: parts.family_key.to_string(),
        market_id: parts.market_id.to_string(),
        instrument_ids: parts.instrument_ids,
        market_class: parts.market_class.to_string(),
        resolution_kind: parts.resolution_kind.to_string(),
        resolution_identity: parts.resolution_identity.to_string(),
        value_kind: parts.value_kind.to_string(),
        metadata_provenance_sha256,
        selected_market_key: String::new(),
        selected_at_ms: parts.selected_at_ms,
    };
    requirement.selected_market_key = selected_market_key_for_requirement(&requirement)?;
    Ok(requirement)
}

pub(crate) fn selected_market_key_for_requirement(
    requirement: &SelectedMarketRequirement,
) -> Result<String, InstrumentFilterError> {
    let input = BTreeMap::from([
        (
            SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD.to_string(),
            serde_json::json!(requirement.configured_target_id),
        ),
        (
            SELECTED_MARKET_FAMILY_KEY_FIELD.to_string(),
            serde_json::json!(requirement.family_key),
        ),
        (
            SELECTED_MARKET_INSTRUMENT_IDS_FIELD.to_string(),
            serde_json::json!(requirement.instrument_ids),
        ),
        (
            SELECTED_MARKET_MARKET_CLASS_FIELD.to_string(),
            serde_json::json!(requirement.market_class),
        ),
        (
            SELECTED_MARKET_MARKET_ID_FIELD.to_string(),
            serde_json::json!(requirement.market_id),
        ),
        (
            SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD.to_string(),
            serde_json::json!(requirement.metadata_provenance_sha256),
        ),
        (
            SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD.to_string(),
            serde_json::json!(requirement.resolution_identity),
        ),
        (
            SELECTED_MARKET_RESOLUTION_KIND_FIELD.to_string(),
            serde_json::json!(requirement.resolution_kind),
        ),
        (
            SELECTED_MARKET_VALUE_KIND_FIELD.to_string(),
            serde_json::json!(requirement.value_kind),
        ),
        (
            SELECTED_MARKET_VENUE_FIELD.to_string(),
            serde_json::json!(requirement.venue),
        ),
    ]);
    canonical_json_sha256(&input)
}

pub(crate) fn canonical_json_sha256<T>(value: &T) -> Result<String, InstrumentFilterError>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|error| InstrumentFilterError::Other {
        message: format!("selected-market canonical JSON serialization failed: {error}"),
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn ensure_selected_market_json_text(
    field: &'static str,
    value: &serde_json::Value,
) -> Result<(), InstrumentFilterError> {
    match value {
        serde_json::Value::String(text) => ensure_selected_market_text(field, text, false),
        serde_json::Value::Array(items) => {
            for item in items {
                ensure_selected_market_json_text(field, item)?;
            }
            Ok(())
        }
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                ensure_selected_market_text(field, key, false)?;
                ensure_selected_market_json_text(field, nested)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ensure_selected_market_text(
    field: &'static str,
    value: &str,
    allow_empty: bool,
) -> Result<(), InstrumentFilterError> {
    if !allow_empty && value.is_empty() {
        return Err(selected_market_requirement_error(format!(
            "selected-market {field} must not be empty"
        )));
    }
    if value.contains('|') {
        return Err(selected_market_requirement_error(format!(
            "selected-market {field} must not contain `|`"
        )));
    }
    Ok(())
}

pub(crate) fn selected_market_requirement_error(
    message: impl Into<String>,
) -> InstrumentFilterError {
    InstrumentFilterError::TargetValidationFailure {
        message: message.into(),
    }
}

/// Target validation entry point used by core startup validation.
/// Returns `(metadata, errors)`: the metadata is `None` when the raw
/// `[target]` value cannot even produce a `configured_target_id` (in
/// which case the family-specific validator's full error set still
/// surfaces in `errors`).
pub fn validate_strategy_target(
    context: &str,
    target: &toml::Value,
) -> (Option<TargetMetadata>, Vec<InstrumentFilterError>) {
    validate_strategy_target_with_bindings(context, target, validation_bindings())
}

pub fn validate_strategy_target_with_bindings(
    context: &str,
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> (Option<TargetMetadata>, Vec<InstrumentFilterError>) {
    let metadata = target.clone().try_into::<TargetMetadata>().ok();
    let dispatch: TargetFamilyDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (
                metadata,
                vec![InstrumentFilterError::Other {
                    message: format!("{context}: target: {error}"),
                }],
            );
        }
    };
    let errors = match bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
    {
        Some(binding) => (binding.validate_target)(context, target)
            .into_iter()
            .map(|message| InstrumentFilterError::TargetValidationFailure { message })
            .collect(),
        None => vec![InstrumentFilterError::UnsupportedFamily {
            context: Some(context.to_string()),
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        }],
    };
    (metadata, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_quoting::{FamilyQuoteInputs, QuoteSide},
    };

    #[test]
    fn production_registry_binds_static_binary_event_family() {
        assert!(
            validation_bindings()
                .iter()
                .any(|binding| binding.key == "static_binary_event"),
            "Static Polymarket binary events must be selectable through the production market-family registry"
        );
    }

    fn fake_validate_target(_context: &str, _target: &toml::Value) -> Vec<String> {
        Vec::new()
    }

    const FAKE_FAMILY_BINDINGS: &[MarketFamilyValidationBinding] =
        &[MarketFamilyValidationBinding {
            key: "fixture_family",
            validate_target: fake_validate_target,
            plan_strategy_target: fake_plan_strategy_target,
            target_runtime_fields: fake_target_runtime_fields,
            select_binary_option_market: fake_select_binary_option_market,
            market_selection_candidate_windows: fake_market_selection_candidate_windows,
            selected_market_requirement: fake_selected_market_requirement,
            fair_probability_up: fake_fair_probability_up,
            maker_quote_targets: fake_maker_quote_targets,
            maker_settlement_payout: fake_maker_settlement_payout,
            maker_settlement_payout_from_reference_prices:
                fake_maker_settlement_payout_from_reference_prices,
            maker_binary_fee_curve: fake_maker_binary_fee_curve,
        }];

    fn fake_plan_strategy_target(
        strategy: &LoadedStrategy,
    ) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError> {
        Err(InstrumentFilterError::Other {
            message: format!(
                "fixture_family binding invoked for strategy `{}`",
                strategy.config.strategy_instance_id
            ),
        })
    }

    fn fake_target_runtime_fields(
        _target: &toml::Value,
    ) -> Result<TargetRuntimeFields, InstrumentFilterError> {
        Err(InstrumentFilterError::Other {
            message: "fixture_family target runtime binding invoked".to_string(),
        })
    }

    fn fake_select_binary_option_market(
        _target: MarketSelectionTarget<'_>,
        _instruments: &[InstrumentAny],
        _now_milliseconds: u64,
    ) -> Option<SelectedBinaryOptionMarket> {
        Some(fake_selected_binary_option_market())
    }

    fn fake_selected_binary_option_market() -> SelectedBinaryOptionMarket {
        SelectedBinaryOptionMarket {
            market_id: "fixture-market".to_string(),
            instrument_id: InstrumentId::from("fixture-market.FIXTURE"),
            up_instrument_id: InstrumentId::from("fixture-up.FIXTURE"),
            down_instrument_id: InstrumentId::from("fixture-down.FIXTURE"),
            selection_outcome: MarketSelectionOutcome::Current,
            start_timestamp_milliseconds: 1_000,
            expiration_timestamp_milliseconds: 61_000,
            seconds_to_end: 60,
            source_identity: SelectedMarketSourceIdentity {
                condition_id: "fixture-condition".to_string(),
                market_slug: "fixture-market".to_string(),
                question_id: "fixture-question".to_string(),
            },
        }
    }

    fn fake_selected_market_requirement(
        _target: &toml::Value,
        selected: &SelectedBinaryOptionMarket,
        selected_at_ms: u64,
    ) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
        selected_market_requirement_from_parts(SelectedMarketRequirementParts {
            configured_target_id: "fixture-target",
            venue: "FIXTURE",
            family_key: "fixture_family",
            market_id: selected.market_id.as_str(),
            instrument_ids: vec![
                selected.down_instrument_id.to_string(),
                selected.up_instrument_id.to_string(),
            ],
            market_class: "fixture_market",
            resolution_kind: "fixture_resolution",
            resolution_identity: "fixture-resolution-1",
            value_kind: "fixture_value",
            metadata_provenance_fields: selected_market_metadata_provenance_fields([
                ("family_key", "fixture_family"),
                ("market_class", "fixture_market"),
                ("market_id", selected.market_id.as_str()),
                ("source_kind", "fixture_metadata"),
                ("venue", "FIXTURE"),
            ]),
            selected_at_ms,
        })
    }

    fn fixture_requirement_parts(
        market_id: &'static str,
        selected_at_ms: u64,
    ) -> SelectedMarketRequirementParts<'static> {
        SelectedMarketRequirementParts {
            configured_target_id: "target-a",
            venue: "FIXTURE",
            family_key: "fixture_family",
            market_id,
            instrument_ids: vec![
                "fixture-down.FIXTURE".to_string(),
                "fixture-up.FIXTURE".to_string(),
            ],
            market_class: "fixture_market",
            resolution_kind: "fixture_resolution",
            resolution_identity: "fixture-resolution-1",
            value_kind: "fixture_value",
            metadata_provenance_fields: selected_market_metadata_provenance_fields([
                ("family_key", "fixture_family"),
                ("market_class", "fixture_market"),
                ("market_id", market_id),
                ("source_kind", "fixture_metadata"),
                ("venue", "FIXTURE"),
            ]),
            selected_at_ms,
        }
    }

    fn fake_market_selection_candidate_windows(
        _target: MarketSelectionTarget<'_>,
        _now_milliseconds: u64,
    ) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
        Ok(vec![MarketSelectionCandidateWindow {
            outcome: MarketSelectionOutcome::Current,
            market_slug: "fixture-market".to_string(),
            start_timestamp_milliseconds: 1_000,
        }])
    }

    fn fake_fair_probability_up(_inputs: &FairProbabilityInputs) -> Option<f64> {
        Some(0.5)
    }

    fn fake_maker_quote_targets(_inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
        None
    }

    fn fake_maker_settlement_payout(_payout: BinarySettlementPayout, _leg: Leg) -> Option<f64> {
        None
    }

    fn fake_maker_settlement_payout_from_reference_prices(
        _close_price: f64,
        _strike_price: f64,
    ) -> Option<BinarySettlementPayout> {
        None
    }

    fn fake_maker_binary_fee_curve(_fee_rate: f64, _price: f64) -> Option<f64> {
        None
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root: crate::bolt_v3_config::BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml")).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: "test-config-bundle-checksum".to_string(),
            root,
            strategies: Vec::new(),
        }
    }

    /// Deserialize the fixture strategy and overwrite its
    /// `target.rotating_market_family` discriminator. Used by tests
    /// that need a non-updown strategy in `loaded.strategies` so the
    /// per-strategy dispatcher routes to an injected fake binding.
    fn fixture_strategy_with_family(family: &str) -> LoadedStrategy {
        let strategy_config: crate::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(
            include_str!("../../tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        )
        .unwrap();
        let mut strategy = LoadedStrategy {
            config_path: std::path::PathBuf::from(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy_config,
        };
        strategy
            .config
            .target
            .as_table_mut()
            .expect("strategy [target] should be a TOML table")
            .insert(
                "rotating_market_family".to_string(),
                toml::Value::String(family.to_string()),
            );
        strategy
    }

    /// Deserialize the fixture strategy and remove the
    /// `rotating_market_family` discriminator from its raw `[target]`
    /// envelope. Used by tests that exercise the parent dispatcher's
    /// own `TargetParseFailed` arm (raw TOML missing the field the
    /// dispatcher reads to route).
    fn fixture_strategy_without_family_discriminator() -> LoadedStrategy {
        let mut strategy = fixture_strategy_with_family("updown");
        strategy
            .config
            .target
            .as_table_mut()
            .expect("strategy [target] should be a TOML table")
            .remove("rotating_market_family");
        strategy
    }

    #[test]
    fn validation_can_use_injected_family_binding_without_editing_production_registry() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let (_, production_errors) = validate_strategy_target("strategy `fixture`", &target);
        assert!(
            production_errors
                .iter()
                .any(|error| error.to_string().contains("not supported by this build")),
            "production registry should not know the test family: {production_errors:?}"
        );

        let (_, injected_errors) = validate_strategy_target_with_bindings(
            "strategy `fixture`",
            &target,
            FAKE_FAMILY_BINDINGS,
        );
        assert!(
            injected_errors.is_empty(),
            "injected family binding should own target dispatch: {injected_errors:?}"
        );
    }

    #[test]
    fn market_identity_plan_uses_injected_family_binding_without_parent_family_branch() {
        let mut loaded = fixture_loaded_config();
        loaded
            .strategies
            .push(fixture_strategy_with_family("fixture_family"));

        // Production registry has only updown; a fixture_family
        // strategy must be rejected as UnsupportedFamily, not silently
        // dispatched to updown.
        let production_error =
            market_identity_plan_from_config_with_bindings(&loaded, validation_bindings())
                .expect_err("production registry must reject unknown family");
        match &production_error {
            InstrumentFilterError::UnsupportedFamily { family_key, .. } => {
                assert_eq!(family_key, "fixture_family");
            }
            other => panic!("expected UnsupportedFamily, got {other:?}"),
        }

        // Injected fake binding owns dispatch for the fixture_family
        // strategy and returns its own error, proving the per-strategy
        // dispatcher routes by `target.rotating_market_family`.
        let injected_error =
            market_identity_plan_from_config_with_bindings(&loaded, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error.to_string(),
            format!(
                "fixture_family binding invoked for strategy `{}`",
                loaded.strategies[0].config.strategy_instance_id
            )
        );
    }

    #[test]
    fn market_identity_plan_dispatch_routes_each_strategy_to_its_family_binding() {
        // Two strategies, one updown and one fixture_family. The
        // per-strategy dispatcher must read each strategy's
        // `target.rotating_market_family` and call only the matching
        // binding. With the prior broadcast dispatch, the updown
        // binding would have iterated every strategy and failed
        // updown-typed deserialization on the fixture_family strategy
        // before the fake binding could handle it.
        let mut loaded = fixture_loaded_config();
        let updown_strategy = fixture_strategy_with_family(updown::KEY);
        let fake_strategy = fixture_strategy_with_family("fixture_family");
        loaded.strategies.push(updown_strategy);
        loaded.strategies.push(fake_strategy);

        let combined_bindings: Vec<MarketFamilyValidationBinding> = validation_bindings()
            .iter()
            .chain(FAKE_FAMILY_BINDINGS.iter())
            .cloned()
            .collect();

        // The fake binding errors loud when its strategy reaches it.
        // The dispatcher must surface that error, proving the fake
        // strategy was routed to the fake binding and not to updown.
        let dispatch_error =
            market_identity_plan_from_config_with_bindings(&loaded, &combined_bindings)
                .expect_err("fake binding must reject the fixture_family strategy");
        match &dispatch_error {
            InstrumentFilterError::Other { message } => {
                assert!(
                    message.contains("fixture_family binding invoked for strategy"),
                    "fake binding should surface its own error, not an updown deserialization \
                     failure: {message}"
                );
            }
            other => panic!(
                "expected fake binding's Other error, got {other:?} — \
                 a TargetParseFailed here means updown was incorrectly called on the \
                 fixture_family strategy"
            ),
        }
    }

    #[test]
    fn market_identity_plan_dispatcher_rejects_strategy_with_missing_family_discriminator() {
        // The parent dispatcher reads `target.rotating_market_family`
        // from each strategy's raw TOML before routing to a family
        // binding. If the discriminator field is absent, the dispatcher
        // must surface its own `TargetParseFailed` keyed on the
        // strategy id — not silently fall through to UnsupportedFamily
        // or to a family binding.
        let mut loaded = fixture_loaded_config();
        loaded
            .strategies
            .push(fixture_strategy_without_family_discriminator());
        let strategy_id = loaded.strategies[0].config.strategy_instance_id.clone();

        match market_identity_plan_from_config_with_bindings(&loaded, validation_bindings()) {
            Err(InstrumentFilterError::TargetParseFailed {
                strategy_instance_id,
                message,
            }) => {
                assert_eq!(strategy_instance_id, strategy_id);
                assert!(
                    message.contains("rotating_market_family"),
                    "error message should name the missing discriminator field: {message}"
                );
                assert!(
                    message.contains("missing field"),
                    "error message should say missing field: {message}"
                );
            }
            other => panic!(
                "expected TargetParseFailed from the parent dispatcher when \
                 rotating_market_family is absent, got {other:?}"
            ),
        }
    }

    #[test]
    fn target_runtime_fields_use_injected_family_binding_without_parent_family_branch() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let production_error = target_runtime_fields_from_target(&target)
            .expect_err("production registry should not know the test family");
        assert!(
            production_error
                .to_string()
                .contains("not supported by this build"),
            "production registry should not know the test family: {production_error}"
        );

        let injected_error =
            target_runtime_fields_from_target_with_bindings(&target, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error.to_string(),
            "fixture_family target runtime binding invoked"
        );
    }

    #[test]
    fn market_selection_uses_injected_family_binding_without_parent_family_branch() {
        let target = MarketSelectionTarget {
            family_key: "fixture_family",
            underlying_asset: "FIXTURE",
            cadence_seconds: 60,
            cadence_slug_token: "fixture",
            static_condition_id: None,
            static_yes_outcome: None,
            static_no_outcome: None,
        };

        assert!(
            select_binary_option_market_from_target(target, &[], 0).is_none(),
            "production registry should not know the test family"
        );

        let selected = select_binary_option_market_from_target_with_bindings(
            target,
            &[],
            0,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own market selection dispatch");

        assert_eq!(selected.market_id, "fixture-market");
    }

    #[test]
    fn fair_probability_routes_to_injected_family_binding_without_parent_family_branch() {
        let inputs = FairProbabilityInputs {
            spot_price: 3_105.0,
            strike_price: 3_100.0,
            seconds_to_market_end: 60,
            realized_vol: 0.45,
            pricing_kurtosis: 0.0,
        };

        assert!(
            fair_probability_up_for_family("fixture_family", &inputs).is_none(),
            "production registry should not know the test family"
        );

        let routed = fair_probability_up_for_family_with_bindings(
            "fixture_family",
            &inputs,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own fair-value dispatch");

        assert_eq!(routed, 0.5);
    }

    fn fixture_quote_inputs() -> FamilyQuoteInputs {
        FamilyQuoteInputs {
            // The reservation band is mintable only through gm_binary_quote, so the
            // fixture builds an interior band (fair 0.60, mu 0.10) the same way
            // production does — there is no bare-literal band.
            band: crate::bolt_v3_maker_model::gm_binary_quote(0.60, 0.10)
                .expect("interior fair, valid mu"),
            inventory_skew: 0.0,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
            eps: 0.000_001,
            tau: 3_600.0,
            reference_tau: 3_600.0,
            time_widen_cap: 10.0,
            order_notional_target: 5.0,
            maximum_position_notional: 10.0,
        }
    }

    #[test]
    fn maker_quote_targets_route_through_canonical_updown_family_binding() {
        let targets = maker_quote_targets_for_family(updown::KEY, fixture_quote_inputs())
            .expect("updown binding should produce binary quote targets");

        assert_eq!(targets.leg_a.side, QuoteSide::Buy);
        assert_eq!(targets.leg_b.side, QuoteSide::Buy);
        assert!(targets.leg_a.price < 0.60);
        assert!(targets.leg_b.price < 0.40);
        // Pin the routed VALUE, not just its sign. The size must be the
        // half-spread-scaled target (5.0 * band.half_spread() ~= $0.2401 for this
        // fixture: reference max_half_spread 1.0 makes edge_scale == half_spread,
        // well below the $5 cap), NOT the raw cap. This is the ONLY test that
        // exercises the registered-family call site, so it must catch both an
        // edge-ignoring constant-cap sizer ($5.0) and a call-site edge/reference
        // arg transpose (also $5.0); the primitive's own unit tests call it with a
        // fixed arg order and structurally cannot detect a transposed call.
        let expected_size = 5.0
            * crate::bolt_v3_maker_model::gm_binary_quote(0.60, 0.10)
                .expect("interior band")
                .half_spread();
        assert!((targets.leg_a.size_notional - expected_size).abs() < 1e-12);
        assert!(
            targets.leg_a.size_notional < 1.0,
            "must be the edge-scaled size, not the $5 cap"
        );
        assert_eq!(targets.leg_a.size_notional, targets.leg_b.size_notional);
    }

    #[test]
    fn maker_settlement_and_fee_curve_route_through_canonical_updown_family_binding() {
        let up_payout = BinarySettlementPayout::new(1.0, 0.0).expect("terminal up payout");
        let down_payout = BinarySettlementPayout::new(0.0, 1.0).expect("terminal down payout");

        assert_eq!(
            maker_settlement_payout_for_family(updown::KEY, up_payout, Leg::Yes),
            Some(1.0)
        );
        assert_eq!(
            maker_settlement_payout_for_family(updown::KEY, up_payout, Leg::No),
            Some(0.0)
        );
        assert_eq!(
            maker_settlement_payout_for_family(updown::KEY, down_payout, Leg::No),
            Some(1.0)
        );
        assert_eq!(
            maker_settlement_payout_for_family(updown::KEY, down_payout, Leg::Yes),
            Some(0.0)
        );
        assert_eq!(
            maker_settlement_payout_from_reference_prices_for_family(updown::KEY, 100.0, 100.0)
                .map(|payout| payout.leg_payout(Leg::Yes)),
            Some(1.0),
            "tie-at-strike resolves to the up/YES payout"
        );
        assert_eq!(
            maker_settlement_payout_from_reference_prices_for_family(updown::KEY, 99.0, 100.0)
                .map(|payout| payout.leg_payout(Leg::No)),
            Some(1.0),
            "close below strike resolves to the down/NO payout"
        );
        assert!(
            maker_settlement_payout_from_reference_prices_for_family(updown::KEY, f64::NAN, 100.0)
                .is_none(),
            "invalid reference prices fail closed"
        );

        let fee = maker_binary_fee_curve_for_family(updown::KEY, 0.02, 0.5)
            .expect("updown fee curve should accept an interior probability");
        assert!((fee - 0.005).abs() < 1e-12);
        assert!(maker_binary_fee_curve_for_family(updown::KEY, -0.01, 0.5).is_none());
        assert!(maker_binary_fee_curve_for_family(updown::KEY, 0.02, f64::NAN).is_none());
    }

    #[test]
    fn unknown_family_maker_write_side_dispatch_fails_closed() {
        let payout = BinarySettlementPayout::new(1.0, 0.0).expect("terminal payout");

        assert!(maker_quote_targets_for_family("missing_family", fixture_quote_inputs()).is_none());
        assert!(maker_settlement_payout_for_family("missing_family", payout, Leg::Yes).is_none());
        assert!(
            maker_settlement_payout_from_reference_prices_for_family("missing_family", 100.0, 99.0)
                .is_none()
        );
        assert!(maker_binary_fee_curve_for_family("missing_family", 0.02, 0.5).is_none());
    }

    #[test]
    fn selected_market_requirement_uses_injected_family_binding_without_parent_family_branch() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();
        let selected = fake_selected_binary_option_market();

        let production_error = selected_market_requirement_from_target(&target, &selected, 123)
            .expect_err("production registry should not know the test family");
        assert!(
            production_error
                .to_string()
                .contains("not supported by this build"),
            "production registry should not know the test family: {production_error}"
        );

        let requirement = selected_market_requirement_from_target_with_bindings(
            &target,
            &selected,
            123,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own requirement extraction");

        assert_eq!(requirement.configured_target_id, "fixture-target");
        assert_eq!(requirement.family_key, "fixture_family");
        assert_eq!(requirement.selected_at_ms, 123);
        assert_eq!(
            requirement.instrument_ids,
            vec!["fixture-down.FIXTURE", "fixture-up.FIXTURE"]
        );
    }

    #[test]
    fn selected_market_key_excludes_selected_at_and_tracks_identity_fields() {
        let first = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            123,
        ))
        .expect("fixture requirement should build");
        let later = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            456,
        ))
        .expect("later fixture requirement should build");
        assert_eq!(first.selected_market_key, later.selected_market_key);

        let changed = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market-2",
            123,
        ))
        .expect("changed fixture requirement should build");
        assert_ne!(first.selected_market_key, changed.selected_market_key);
    }

    #[test]
    fn selected_market_requirement_rejects_unsorted_or_duplicate_instrument_ids() {
        let mut unsorted = fixture_requirement_parts("fixture-market", 123);
        unsorted.instrument_ids = vec![
            "fixture-up.FIXTURE".to_string(),
            "fixture-down.FIXTURE".to_string(),
        ];
        let error = selected_market_requirement_from_parts(unsorted)
            .expect_err("unsorted instrument ids should fail closed");
        assert!(
            error.to_string().contains("sorted and unique"),
            "expected sorted ids rejection, got: {error}"
        );

        let mut duplicate = fixture_requirement_parts("fixture-market", 123);
        duplicate.instrument_ids = vec![
            "fixture-up.FIXTURE".to_string(),
            "fixture-up.FIXTURE".to_string(),
        ];
        let error = selected_market_requirement_from_parts(duplicate)
            .expect_err("duplicate instrument ids should fail closed");
        assert!(
            error.to_string().contains("sorted and unique"),
            "expected unique ids rejection, got: {error}"
        );
    }

    #[test]
    fn selected_market_requirement_rejects_empty_metadata_provenance_strings() {
        let mut parts = fixture_requirement_parts("fixture-market", 123);
        parts.metadata_provenance_fields = selected_market_metadata_provenance_fields([
            ("family_key", "fixture_family"),
            ("source_kind", ""),
        ]);

        let error = selected_market_requirement_from_parts(parts)
            .expect_err("empty provenance string should fail closed");
        assert!(
            error.to_string().contains("metadata_provenance"),
            "expected provenance string rejection, got: {error}"
        );
    }

    #[test]
    fn selected_market_key_uses_lexicographically_sorted_canonical_json() {
        use std::collections::BTreeMap;

        let requirement = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            123,
        ))
        .expect("fixture requirement should build");

        let expected_input = BTreeMap::from([
            (
                SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD.to_string(),
                serde_json::json!("target-a"),
            ),
            (
                SELECTED_MARKET_FAMILY_KEY_FIELD.to_string(),
                serde_json::json!("fixture_family"),
            ),
            (
                SELECTED_MARKET_INSTRUMENT_IDS_FIELD.to_string(),
                serde_json::json!(["fixture-down.FIXTURE", "fixture-up.FIXTURE"]),
            ),
            (
                SELECTED_MARKET_MARKET_CLASS_FIELD.to_string(),
                serde_json::json!("fixture_market"),
            ),
            (
                SELECTED_MARKET_MARKET_ID_FIELD.to_string(),
                serde_json::json!("fixture-market"),
            ),
            (
                SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD.to_string(),
                serde_json::json!(requirement.metadata_provenance_sha256),
            ),
            (
                SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD.to_string(),
                serde_json::json!("fixture-resolution-1"),
            ),
            (
                SELECTED_MARKET_RESOLUTION_KIND_FIELD.to_string(),
                serde_json::json!("fixture_resolution"),
            ),
            (
                SELECTED_MARKET_VALUE_KIND_FIELD.to_string(),
                serde_json::json!("fixture_value"),
            ),
            (
                SELECTED_MARKET_VENUE_FIELD.to_string(),
                serde_json::json!("FIXTURE"),
            ),
        ]);
        assert_eq!(
            requirement.selected_market_key,
            canonical_json_sha256(&expected_input).expect("canonical BTreeMap should hash")
        );
    }

    #[test]
    fn validate_strategy_target_wraps_target_block_errors_as_typed_target_validation_failure() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "CONFIGURED_ASSET"
            cadence_secs = -1
            cadence_slug_token = "configuredwindow"
            market_selection_rule = "active_or_next"
            retry_interval_secs = 1
            blocked_after_secs = 1
        }
        .into();

        let (_, errors) = validate_strategy_target("strategy `alpha`", &target);
        let cadence_failure = errors.iter().find(|e| {
            matches!(
                e,
                InstrumentFilterError::TargetValidationFailure { message, .. }
                    if message.contains("target.cadence_secs")
            )
        });
        assert!(
            cadence_failure.is_some(),
            "expected TargetValidationFailure for cadence_secs: {errors:#?}"
        );
    }

    #[test]
    fn validate_strategy_target_emits_typed_unsupported_family_for_unknown_key() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "unicorn"
        }
        .into();

        let (_, errors) = validate_strategy_target("strategy `alpha`", &target);
        let unsupported = errors.iter().find_map(|e| match e {
            InstrumentFilterError::UnsupportedFamily {
                context,
                family_key,
                ..
            } => Some((context.clone(), family_key.clone())),
            _ => None,
        });
        assert_eq!(
            unsupported,
            Some((Some("strategy `alpha`".to_string()), "unicorn".to_string()))
        );
    }

    #[test]
    fn target_runtime_fields_returns_typed_unsupported_family_for_unknown_key() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "unicorn"
        }
        .into();

        let error = target_runtime_fields_from_target(&target).expect_err("unknown family");
        match error {
            InstrumentFilterError::UnsupportedFamily {
                context,
                family_key,
                supported,
            } => {
                assert_eq!(context, None);
                assert_eq!(family_key, "unicorn");
                assert!(
                    supported.contains(&updown::KEY),
                    "supported list should include the registered family keys: {supported:?}"
                );
            }
            other => panic!("expected UnsupportedFamily, got {other:?}"),
        }
    }
}
