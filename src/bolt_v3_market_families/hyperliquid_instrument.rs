//! Static Hyperliquid instrument target identity.
//!
//! This family lets strategy config name a concrete Hyperliquid
//! instrument for route validation without reusing the up/down rotating
//! market shape. Binary market selection remains unsupported here.

use std::sync::Arc;

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::LoadedStrategy,
    bolt_v3_instrument_filters::InstrumentFilterError,
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketIdentityPlan, MarketIdentityTarget,
        MarketSelectionCandidateWindow, MarketSelectionTarget, SelectedBinaryOptionMarket,
        SelectedMarketRequirement, TargetRuntimeFields,
    },
};

pub const KEY: &str = "hyperliquid_instrument";

const BINARY_MARKET_UNSUPPORTED: &str = "hyperliquid_instrument targets are static instruments and do not support binary rotating-market selection";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub product_surface: ProductSurface,
    pub instrument_id: InstrumentId,
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
    pub configured_target_id: String,
    pub execution_client_id: String,
    pub product_surface: ProductSurface,
    pub instrument_id: InstrumentId,
}

impl MarketIdentityTarget for HyperliquidInstrumentTargetPlan {
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
    if block.configured_target_id.is_empty() {
        errors.push(format!(
            "{context}: target.configured_target_id must not be empty"
        ));
    }
    let instrument_id = block.instrument_id.to_string();
    if instrument_id.is_empty() {
        errors.push(format!("{context}: target.instrument_id must not be empty"));
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
    })))
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

pub fn fair_probability_up(_inputs: &FairProbabilityInputs) -> Option<f64> {
    None
}

fn binary_market_unsupported() -> InstrumentFilterError {
    InstrumentFilterError::Other {
        message: BINARY_MARKET_UNSUPPORTED.to_string(),
    }
}
