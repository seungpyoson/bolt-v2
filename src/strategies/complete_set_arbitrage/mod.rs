//! Thin complete-set strategy shell.
//!
//! The shell is intentionally limited to basket intent and NT-event forwarding.
//! Admission, venue mutation, fillability, sizing, rounding, and repair/unwind
//! remain in shared outcome-group execution modules.

use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    enums::{OmsType as NtOmsType, TimeInForce},
    identifiers::{InstrumentId, StrategyId},
};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::Decimal;
use serde::Deserialize;
use toml::Value;

use crate::{
    bolt_v3_archetypes::complete_set_arbitrage::{
        CompleteSetSubmitMode, raw_complete_set_config, submit_mode_contract,
    },
    bolt_v3_basket_execution::{
        BoltV3BasketExecutionError, BoltV3BasketExecutionEvent, BoltV3BasketExecutionState,
        BoltV3BasketSettlementSignal,
    },
    bolt_v3_outcome_group_sources::COMPLETE_SET_ARBITRAGE_KEY,
    bolt_v3_providers::resolve_fee_provider,
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
    },
    strategies::{
        production_strategy_registry,
        registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError},
    },
};

pub const KEY: &str = COMPLETE_SET_ARBITRAGE_KEY;

const CONFIG_FIELD_OMS_TYPE: &str = stringify!(oms_type);
const WRONG_TYPE_CODE: &str = stringify!(wrong_type);
const INVALID_CONFIG_CODE: &str = stringify!(invalid_config);

pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: CompleteSetArbitrageBuilder::kind,
    register: register_runtime_strategy,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSetArbitrageShell {
    strategy_id: String,
    forwarded_event_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteSetMechanicsPolicy {
    pub shared_basket_execution_owns_admission: bool,
    pub shared_basket_execution_owns_venue_mutation: bool,
    pub shared_basket_execution_owns_fillability: bool,
    pub shared_basket_execution_owns_repair_unwind: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteSetSettlementPolicy {
    RejectUntilReachableNtSignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteSetNtSubmitContract {
    pub order_list_type: &'static str,
    pub submit_order_list_type: &'static str,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetArbitrageConfig {
    pub strategy_id: String,
    pub order_id_tag: String,
    pub oms_type: String,
    pub use_uuid_client_order_ids: bool,
    pub use_hyphens_in_client_order_ids: bool,
    pub external_order_claims: Vec<String>,
    pub manage_contingent_orders: bool,
    pub manage_gtd_expiry: bool,
    pub manage_stop: bool,
    pub market_exit_interval_ms: u64,
    pub market_exit_max_attempts: u64,
    pub market_exit_reduce_only: bool,
    pub log_events: bool,
    pub log_commands: bool,
    pub log_rejected_due_post_only_as_warning: bool,
    pub client_id: String,
    pub min_edge_bps: i64,
    pub max_basket_notional: String,
    pub max_open_baskets: u32,
    pub submit_mode: String,
    pub vwap_depth_limit_bps: u64,
    pub slippage_buffer_bps: u64,
    pub max_repair_attempts: u32,
    pub max_unwind_attempts: u32,
}

pub struct CompleteSetArbitrage {
    core: StrategyCore,
    config: CompleteSetArbitrageConfig,
    context: StrategyBuildContext,
    shell: CompleteSetArbitrageShell,
}

impl std::fmt::Debug for CompleteSetArbitrage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompleteSetArbitrage")
            .field("strategy_id", &self.config.strategy_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct CompleteSetArbitrageBuilder;

impl CompleteSetArbitrage {
    fn new(config: CompleteSetArbitrageConfig, context: StrategyBuildContext) -> Result<Self> {
        let oms_type = parse_configured_oms_type(CONFIG_FIELD_OMS_TYPE, &config.oms_type)?;
        let market_exit_time_in_force = submit_mode_time_in_force(&config.submit_mode)?;
        let external_order_claims = config
            .external_order_claims
            .iter()
            .map(|instrument_id| InstrumentId::from(instrument_id.as_str()))
            .collect::<Vec<_>>();
        Ok(Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                use_uuid_client_order_ids: config.use_uuid_client_order_ids,
                use_hyphens_in_client_order_ids: config.use_hyphens_in_client_order_ids,
                oms_type: Some(oms_type),
                external_order_claims: Some(external_order_claims),
                manage_contingent_orders: config.manage_contingent_orders,
                manage_gtd_expiry: config.manage_gtd_expiry,
                manage_stop: config.manage_stop,
                market_exit_interval_ms: config.market_exit_interval_ms,
                market_exit_max_attempts: config.market_exit_max_attempts,
                market_exit_time_in_force,
                market_exit_reduce_only: config.market_exit_reduce_only,
                log_events: config.log_events,
                log_commands: config.log_commands,
                log_rejected_due_post_only_as_warning: config.log_rejected_due_post_only_as_warning,
            }),
            shell: CompleteSetArbitrageShell::new(config.strategy_id.clone()),
            config,
            context,
        })
    }

    pub fn config(&self) -> &CompleteSetArbitrageConfig {
        &self.config
    }

    pub fn context(&self) -> &StrategyBuildContext {
        &self.context
    }

    pub fn shell(&self) -> &CompleteSetArbitrageShell {
        &self.shell
    }
}

impl DataActor for CompleteSetArbitrage {}

nautilus_strategy!(CompleteSetArbitrage);

impl CompleteSetArbitrageShell {
    pub fn new(strategy_id: impl Into<String>) -> Self {
        Self {
            strategy_id: strategy_id.into(),
            forwarded_event_count: u64::MIN,
        }
    }

    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    pub fn mechanics_policy(&self) -> CompleteSetMechanicsPolicy {
        CompleteSetMechanicsPolicy {
            shared_basket_execution_owns_admission: true,
            shared_basket_execution_owns_venue_mutation: true,
            shared_basket_execution_owns_fillability: true,
            shared_basket_execution_owns_repair_unwind: true,
        }
    }

    pub fn forwarded_event_count(&self) -> u64 {
        self.forwarded_event_count
    }

    pub fn forward_executor_event(
        &mut self,
        basket: &mut BoltV3BasketExecutionState,
        event: BoltV3BasketExecutionEvent,
    ) -> Result<(), BoltV3BasketExecutionError> {
        basket.apply_event(event)?;
        self.forwarded_event_count = self.forwarded_event_count.saturating_add(1);
        Ok(())
    }
}

impl CompleteSetArbitrageBuilder {
    pub fn parse_config(raw: &Value) -> Result<CompleteSetArbitrageConfig> {
        let config: CompleteSetArbitrageConfig = raw
            .clone()
            .try_into()
            .context("complete_set_arbitrage builder requires a valid config table")?;
        anyhow::ensure!(
            config.min_edge_bps.is_positive(),
            "min_edge_bps must be positive"
        );
        let max_basket_notional = config
            .max_basket_notional
            .parse::<Decimal>()
            .context("max_basket_notional must parse as a decimal")?;
        anyhow::ensure!(
            max_basket_notional > Decimal::ZERO,
            "max_basket_notional must be positive"
        );
        for (field, value) in [
            (stringify!(max_open_baskets), config.max_open_baskets),
            (stringify!(max_repair_attempts), config.max_repair_attempts),
            (stringify!(max_unwind_attempts), config.max_unwind_attempts),
        ] {
            anyhow::ensure!(value > u32::MIN, "{field} must be positive");
        }
        for (field, value) in [
            (
                stringify!(vwap_depth_limit_bps),
                config.vwap_depth_limit_bps,
            ),
            (stringify!(slippage_buffer_bps), config.slippage_buffer_bps),
        ] {
            anyhow::ensure!(value > u64::MIN, "{field} must be positive");
        }
        parse_submit_mode(&config.submit_mode)?;
        Ok(config)
    }
}

impl StrategyBuilder for CompleteSetArbitrageBuilder {
    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        if !raw.is_table() {
            errors.push(ValidationError {
                field: field_prefix.to_string(),
                code: WRONG_TYPE_CODE,
                message: "must be a TOML table".to_string(),
            });
            return;
        }
        if let Err(error) = Self::parse_config(raw) {
            errors.push(ValidationError {
                field: field_prefix.to_string(),
                code: INVALID_CONFIG_CODE,
                message: error.to_string(),
            });
        }
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(CompleteSetArbitrage::new(
            Self::parse_config(raw)?,
            context.clone(),
        )?))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = CompleteSetArbitrage::new(Self::parse_config(raw)?, context.clone())?;
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

pub fn register_runtime_strategy(
    node: &mut LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw = raw_complete_set_config(context.strategy, context.loaded)
        .map_err(|error| binding_message(&context, error.to_string()))?;
    let fee_provider = resolve_fee_provider(
        context.loaded,
        context.strategy.config.execution_client_id.as_str(),
        context.resolved,
    )
    .map_err(|error| binding_message(&context, error.to_string()))?;
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let execution_venue = context
        .loaded
        .root
        .clients
        .get(execution_client_id)
        .map(|client| client.venue)
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
                ),
            )
        })?;
    let build_context = StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        context.order_execution_policy,
        execution_venue,
    )
    .with_realized_volatility_runtime(context.realized_volatility_runtime.clone());
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            context.strategy_kind,
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

pub fn nt_submit_contract() -> CompleteSetNtSubmitContract {
    let contract = crate::bolt_v3_order_execution::nt_order_management_contract();
    CompleteSetNtSubmitContract {
        order_list_type: contract.order_list_type,
        submit_order_list_type: contract.submit_order_list_type,
    }
}

pub fn live_settlement_policy() -> CompleteSetSettlementPolicy {
    CompleteSetSettlementPolicy::RejectUntilReachableNtSignal
}

/// Always returns an error while Task 0 classifies live settlement signals as
/// unreachable through the allowed NT Polymarket subscription path.
pub fn forward_settlement_signal(
    basket: &mut BoltV3BasketExecutionState,
) -> Result<(), BoltV3BasketExecutionError> {
    basket.apply_event(BoltV3BasketExecutionEvent::SettlementSignal(
        BoltV3BasketSettlementSignal::LiveSettlementRejectedUntilReachableNtSignal,
    ))
}

fn parse_configured_oms_type(field: &str, value: &str) -> Result<NtOmsType> {
    value
        .parse::<NtOmsType>()
        .with_context(|| format!("{field} must be a NautilusTrader OmsType, got `{value}`"))
}

fn submit_mode_time_in_force(value: &str) -> Result<TimeInForce> {
    Ok(submit_mode_contract(parse_submit_mode(value)?)
        .order_template
        .time_in_force)
}

fn parse_submit_mode(value: &str) -> Result<CompleteSetSubmitMode> {
    CompleteSetSubmitMode::from_config(value)
        .with_context(|| "submit_mode is not supported by the complete-set archetype")
}

fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
    }
}

#[cfg(test)]
mod tests;
