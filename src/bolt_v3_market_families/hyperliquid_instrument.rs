//! Static Hyperliquid instrument target identity.
//!
//! This family lets strategy config name a concrete Hyperliquid
//! instrument for route validation without reusing the up/down rotating
//! market shape. Binary market selection remains unsupported here.

use std::{collections::BTreeMap, sync::Arc};

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::{LoadedStrategy, NO_RESOLUTION_KIND, NO_RESOLUTION_VALUE_KIND},
    bolt_v3_instrument_filters::InstrumentFilterError,
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketIdentityPlan, MarketIdentityTarget,
        MarketSelectionCandidateWindow, MarketSelectionTarget, SelectedBinaryOptionMarket,
        SelectedMarketRequirement, SelectedMarketRequirementParts, TargetRuntimeFields,
        selected_market_metadata_provenance_fields, selected_market_requirement_from_parts,
    },
    bolt_v3_numeric::Probability,
    bolt_v3_target_identity::ConfiguredTargetId,
};

pub const KEY: &str = "hyperliquid_instrument";

const BINARY_MARKET_UNSUPPORTED: &str = "hyperliquid_instrument targets are static instruments and do not support binary rotating-market selection";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: ConfiguredTargetId,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub product_surface: ProductSurface,
    pub instrument_id: InstrumentId,
    pub quantity_step: Decimal,
    pub notional_step: Option<Decimal>,
    pub min_quantity: Option<Decimal>,
    pub min_notional: Option<Decimal>,
    pub gate_subscriptions: Option<BTreeMap<String, super::updown::TargetGateSubscription>>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    StaticInstrument,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotatingMarketFamily {
    HyperliquidInstrument,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProductSurface {
    StandardPerps,
    Spot,
    Hip3BuilderPerps,
    Hip4Outcomes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidInstrumentTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: ConfiguredTargetId,
    pub execution_client_id: String,
    pub product_surface: ProductSurface,
    pub instrument_id: InstrumentId,
    pub quantity_step: Decimal,
    pub notional_step: Option<Decimal>,
    pub min_quantity: Option<Decimal>,
    pub min_notional: Option<Decimal>,
}

impl MarketIdentityTarget for HyperliquidInstrumentTargetPlan {
    fn family_key(&self) -> &'static str {
        KEY
    }

    fn configured_target_id(&self) -> &ConfiguredTargetId {
        &self.configured_target_id
    }

    fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn target_plans(
    plan: &MarketIdentityPlan,
) -> impl Iterator<Item = &HyperliquidInstrumentTargetPlan> {
    plan.targets().filter_map(|target| {
        target
            .as_any()
            .downcast_ref::<HyperliquidInstrumentTargetPlan>()
    })
}

pub fn deserialize_target_block(target: &toml::Value) -> Result<TargetBlock, String> {
    target
        .clone()
        .try_into::<TargetBlock>()
        .map_err(|error| error.to_string())
}

pub fn validate_target_block(context: &str, target: &toml::Value) -> Vec<String> {
    let block = match deserialize_target_block(target) {
        Ok(value) => value,
        Err(message) => return vec![format!("{context}: target: {message}")],
    };

    let mut errors = Vec::new();
    let instrument_id = block.instrument_id.to_string();
    if instrument_id.is_empty() {
        errors.push(format!("{context}: target.instrument_id must not be empty"));
    }
    if block.quantity_step <= Decimal::ZERO {
        errors.push(format!(
            "{context}: target.quantity_step must be a positive decimal"
        ));
    }
    if block
        .notional_step
        .is_some_and(|notional_step| notional_step <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: target.notional_step must be a positive decimal when configured"
        ));
    }
    if block
        .min_quantity
        .is_some_and(|min_quantity| min_quantity <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: target.min_quantity must be a positive decimal when configured"
        ));
    }
    if block
        .min_notional
        .is_some_and(|min_notional| min_notional <= Decimal::ZERO)
    {
        errors.push(format!(
            "{context}: target.min_notional must be a positive decimal when configured"
        ));
    }
    errors
}

pub fn plan_strategy_target(
    strategy: &LoadedStrategy,
) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError> {
    let strategy_instance_id = strategy.config.strategy_instance_id.clone();
    let target = deserialize_target_block(&strategy.config.target).map_err(|message| {
        InstrumentFilterError::TargetParseFailed {
            strategy_instance_id: strategy_instance_id.clone(),
            message,
        }
    })?;
    let TargetKind::StaticInstrument = target.kind;
    let RotatingMarketFamily::HyperliquidInstrument = target.rotating_market_family;

    Ok(Some(Arc::new(HyperliquidInstrumentTargetPlan {
        strategy_instance_id,
        configured_target_id: target.configured_target_id,
        execution_client_id: strategy.config.execution_client_id.to_string(),
        product_surface: target.product_surface,
        instrument_id: target.instrument_id,
        quantity_step: target.quantity_step,
        notional_step: target.notional_step,
        min_quantity: target.min_quantity,
        min_notional: target.min_notional,
    })))
}

pub fn selected_static_instrument_requirement(
    target: &HyperliquidInstrumentTargetPlan,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    let instrument_id = target.instrument_id.to_string();
    selected_market_requirement_from_parts(SelectedMarketRequirementParts {
        configured_target_id: &target.configured_target_id,
        venue: target.instrument_id.venue.as_str(),
        family_key: KEY,
        market_id: instrument_id.as_str(),
        instrument_ids: vec![instrument_id.clone()],
        market_class: product_surface_key(target.product_surface),
        resolution_kind: NO_RESOLUTION_KIND,
        resolution_identity: instrument_id.as_str(),
        value_kind: NO_RESOLUTION_VALUE_KIND,
        metadata_provenance_fields: selected_market_metadata_provenance_fields([
            (
                "product_surface",
                product_surface_key(target.product_surface),
            ),
            ("instrument_id", instrument_id.as_str()),
        ]),
        selected_at_ms,
    })
}

pub fn target_runtime_fields(
    _target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    Err(binary_market_unsupported())
}

pub fn select_binary_option_market(
    _target: MarketSelectionTarget<'_>,
    _instruments: &[InstrumentAny],
    _now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    None
}

pub fn market_selection_candidate_windows(
    _target: MarketSelectionTarget<'_>,
    _now_milliseconds: u64,
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    Err(binary_market_unsupported())
}

pub fn selected_market_requirement(
    _target: &toml::Value,
    _selected: &SelectedBinaryOptionMarket,
    _selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    Err(binary_market_unsupported())
}

pub fn fair_probability_up(_inputs: &FairProbabilityInputs) -> Option<Probability> {
    None
}

fn binary_market_unsupported() -> InstrumentFilterError {
    InstrumentFilterError::Other {
        message: BINARY_MARKET_UNSUPPORTED.to_string(),
    }
}

fn product_surface_key(surface: ProductSurface) -> &'static str {
    match surface {
        ProductSurface::StandardPerps => "standard_perps",
        ProductSurface::Spot => "spot",
        ProductSurface::Hip3BuilderPerps => "hip3_builder_perps",
        ProductSurface::Hip4Outcomes => "hip4_outcomes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_whitespace_padded_configured_target_identity() {
        let target: toml::Value = toml::toml! {
            configured_target_id = " hl-spot-btc-usdc "
            kind = "static_instrument"
            rotating_market_family = "hyperliquid_instrument"
            product_surface = "spot"
            instrument_id = "BTC/USDC.HYPERLIQUID"
            quantity_step = "0.001"
        }
        .into();

        let errors = validate_target_block("strategy `fixture`", &target);
        assert!(errors.iter().any(|error| {
            error.contains(
                "target.configured_target_id must be non-empty without surrounding whitespace",
            )
        }));
    }
}
