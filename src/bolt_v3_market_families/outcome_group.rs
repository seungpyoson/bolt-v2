//! Static outcome-group target identity.
//!
//! This family is the config-owned bridge from strategy `[target]`
//! blocks to root-level `outcome_group_sources`. It registers no
//! provider client and performs no discovery itself; provider mapping
//! resolves the selected source ids from the shared market-identity
//! plan.

use std::{collections::BTreeSet, sync::Arc};

use nautilus_model::instruments::InstrumentAny;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::LoadedStrategy,
    bolt_v3_instrument_filters::InstrumentFilterError,
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketIdentityPlan, MarketIdentityTarget,
        MarketSelectionCandidateWindow, MarketSelectionTarget, SelectedBinaryOptionMarket,
        SelectedMarketRequirement, TargetRuntimeFields,
    },
    bolt_v3_numeric::Probability,
};

pub const KEY: &str = "outcome_group";

const BINARY_MARKET_UNSUPPORTED: &str = "outcome_group targets use static group sources and do not support binary rotating-market selection";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub group_sources: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    StaticOutcomeGroup,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotatingMarketFamily {
    OutcomeGroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeGroupTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub execution_client_id: String,
    pub group_sources: Vec<String>,
}

impl MarketIdentityTarget for OutcomeGroupTargetPlan {
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

pub fn target_plans(plan: &MarketIdentityPlan) -> impl Iterator<Item = &OutcomeGroupTargetPlan> {
    plan.targets()
        .filter_map(|target| target.as_any().downcast_ref::<OutcomeGroupTargetPlan>())
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
    if block.configured_target_id.trim().is_empty()
        || block.configured_target_id.trim() != block.configured_target_id
    {
        errors.push(format!(
            "{context}: target.configured_target_id must be non-empty without surrounding whitespace"
        ));
    }
    if block.group_sources.is_empty() {
        errors.push(format!("{context}: target.group_sources must not be empty"));
    }
    let mut seen = BTreeSet::new();
    for source_id in &block.group_sources {
        if source_id.trim().is_empty() || source_id.trim() != source_id {
            errors.push(format!(
                "{context}: target.group_sources entries must be non-empty without surrounding whitespace"
            ));
        }
        if !seen.insert(source_id.as_str()) {
            errors.push(format!(
                "{context}: target.group_sources source_id `{source_id}` is duplicated"
            ));
        }
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

    let TargetKind::StaticOutcomeGroup = target.kind;
    let RotatingMarketFamily::OutcomeGroup = target.rotating_market_family;

    Ok(Some(Arc::new(OutcomeGroupTargetPlan {
        strategy_instance_id,
        configured_target_id: target.configured_target_id,
        execution_client_id: strategy.config.execution_client_id.to_string(),
        group_sources: target.group_sources,
    })))
}

pub fn target_runtime_fields(
    _target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    Err(InstrumentFilterError::Other {
        message: BINARY_MARKET_UNSUPPORTED.to_string(),
    })
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
    Err(InstrumentFilterError::Other {
        message: BINARY_MARKET_UNSUPPORTED.to_string(),
    })
}

pub fn selected_market_requirement(
    _target: &toml::Value,
    _selected: &SelectedBinaryOptionMarket,
    _selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    Err(InstrumentFilterError::Other {
        message: BINARY_MARKET_UNSUPPORTED.to_string(),
    })
}

pub fn fair_probability_up(_inputs: &FairProbabilityInputs) -> Option<Probability> {
    None
}
