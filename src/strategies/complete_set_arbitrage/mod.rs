//! Thin complete-set strategy shell.
//!
//! The shell is intentionally limited to basket intent and NT-event forwarding.
//! Admission, venue mutation, fillability, sizing, rounding, and repair/unwind
//! remain in shared outcome-group execution modules.

pub mod archetype;

use std::{cell::RefCell, collections::BTreeMap, fmt, rc::Rc};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component};
use nautilus_model::{
    enums::{OmsType as NtOmsType, TimeInForce},
    events::{OrderAccepted, OrderCancelRejected, OrderFilled},
    identifiers::{InstrumentId, StrategyId},
};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore};
use rust_decimal::Decimal;
use serde::Deserialize;
use toml::Value;

use crate::{
    bolt_v3_basket_execution::{
        BoltV3BasketExecutionError, BoltV3BasketExecutionEvent, BoltV3BasketExecutionState,
        BoltV3BasketSettlementSignal,
    },
    bolt_v3_complete_set_contract::{
        COMPLETE_SET_ARBITRAGE_KEY, CompleteSetSubmitMode, submit_mode_contract,
    },
    bolt_v3_strategy_context::StrategyBuildContext,
    strategies::{
        nautilus_strategy_with_fill_void_guard,
        registry::{BoxedStrategy, StrategyBuilder, ValidationError},
    },
};

pub const KEY: &str = COMPLETE_SET_ARBITRAGE_KEY;

const CONFIG_FIELD_OMS_TYPE: &str = stringify!(oms_type);
const WRONG_TYPE_CODE: &str = stringify!(wrong_type);
const INVALID_CONFIG_CODE: &str = stringify!(invalid_config);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSetArbitrageShell {
    strategy_id: String,
    forwarded_event_count: u64,
    failed_event_count: u64,
    last_failure: Option<CompleteSetForwardingError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompleteSetForwardingError {
    UnknownExecutorLegIdentity {
        execution_client_id: String,
        client_order_id: String,
    },
    DuplicateBasketHandle(String),
    DuplicateExecutorLegIdentity(String),
    FillCostOverflow,
    Executor(BoltV3BasketExecutionError),
}

impl fmt::Display for CompleteSetForwardingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownExecutorLegIdentity {
                execution_client_id,
                client_order_id,
            } => write!(
                f,
                "complete-set event has no basket for executor identity `{execution_client_id}` and client order `{client_order_id}`"
            ),
            Self::DuplicateExecutorLegIdentity(client_order_id) => write!(
                f,
                "complete-set client order identity `{client_order_id}` maps to more than one basket"
            ),
            Self::DuplicateBasketHandle(basket_handle) => write!(
                f,
                "complete-set basket handle `{basket_handle}` is already indexed"
            ),
            Self::FillCostOverflow => write!(f, "complete-set fill cost overflowed decimal range"),
            Self::Executor(error) => write!(f, "complete-set executor event failed: {error}"),
        }
    }
}

impl std::error::Error for CompleteSetForwardingError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompleteSetNtEventForwarder {
    execution_client_id: String,
    shell: CompleteSetArbitrageShell,
    baskets_by_handle: BTreeMap<String, BoltV3BasketExecutionState>,
    basket_handle_by_client_order_id: BTreeMap<String, String>,
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
    event_forwarder: CompleteSetNtEventForwarder,
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
            event_forwarder: CompleteSetNtEventForwarder::new(
                config.strategy_id.clone(),
                config.client_id.clone(),
            ),
            config,
            context,
        })
    }

    pub fn complete_set_config(&self) -> &CompleteSetArbitrageConfig {
        &self.config
    }

    pub fn context(&self) -> &StrategyBuildContext {
        &self.context
    }

    pub fn shell(&self) -> &CompleteSetArbitrageShell {
        &self.event_forwarder.shell
    }
}

impl DataActor for CompleteSetArbitrage {}

nautilus_strategy_with_fill_void_guard!(CompleteSetArbitrage, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        self.event_forwarder
            .forward_order_filled(event)
            .expect("complete-set fill event forwarding must succeed");
    }

    fn on_order_accepted(&mut self, event: OrderAccepted) {
        if let Err(error) = self.event_forwarder.forward_order_accepted(&event) {
            log::error!(
                "complete-set accepted event forwarding failed: strategy_id={} error={error}",
                self.config.strategy_id
            );
        }
    }

    fn on_order_cancel_rejected(&mut self, event: OrderCancelRejected) {
        if let Err(error) = self.event_forwarder.forward_order_cancel_rejected(&event) {
            log::error!(
                "complete-set cancel-rejected event forwarding failed: strategy_id={} error={error}",
                self.config.strategy_id
            );
        }
    }
});

impl CompleteSetNtEventForwarder {
    fn new(strategy_id: impl Into<String>, execution_client_id: impl Into<String>) -> Self {
        Self {
            execution_client_id: execution_client_id.into(),
            shell: CompleteSetArbitrageShell::new(strategy_id),
            // Task 11 only installs the forwarding substrate. Production does not
            // synthesize or activate basket state in this strategy-local lookup.
            baskets_by_handle: BTreeMap::new(),
            basket_handle_by_client_order_id: BTreeMap::new(),
        }
    }

    fn forward_order_accepted(
        &mut self,
        event: &OrderAccepted,
    ) -> Result<(), CompleteSetForwardingError> {
        let client_order_id = event.client_order_id.to_string();
        self.forward_event(
            &client_order_id,
            BoltV3BasketExecutionEvent::VenueOrderId {
                client_order_id: client_order_id.clone(),
                venue_order_id: event.venue_order_id.to_string(),
            },
        )
    }

    fn forward_order_filled(
        &mut self,
        event: &OrderFilled,
    ) -> Result<(), CompleteSetForwardingError> {
        let quantity = event.last_qty.as_decimal();
        let Some(cost) = quantity.checked_mul(event.last_px.as_decimal()) else {
            let error = CompleteSetForwardingError::FillCostOverflow;
            self.shell.record_failure(error.clone());
            return Err(error);
        };
        let client_order_id = event.client_order_id.to_string();
        self.forward_event(
            &client_order_id,
            BoltV3BasketExecutionEvent::LegFill {
                client_order_id: client_order_id.clone(),
                venue_order_id: Some(event.venue_order_id.to_string()),
                quantity,
                cost,
                source: crate::bolt_v3_basket_execution::BoltV3BasketFillSource::Strategy,
            },
        )
    }

    fn forward_order_cancel_rejected(
        &mut self,
        event: &OrderCancelRejected,
    ) -> Result<(), CompleteSetForwardingError> {
        let client_order_id = event.client_order_id.to_string();
        self.forward_event(
            &client_order_id,
            BoltV3BasketExecutionEvent::CancelRejected {
                reason: event.reason.to_string(),
            },
        )
    }

    fn forward_event(
        &mut self,
        client_order_id: &str,
        event: BoltV3BasketExecutionEvent,
    ) -> Result<(), CompleteSetForwardingError> {
        let Some(basket_handle) = self
            .basket_handle_by_client_order_id
            .get(client_order_id)
            .cloned()
        else {
            let error = CompleteSetForwardingError::UnknownExecutorLegIdentity {
                execution_client_id: self.execution_client_id.clone(),
                client_order_id: client_order_id.to_string(),
            };
            self.shell.record_failure(error.clone());
            return Err(error);
        };
        let Some(basket) = self.baskets_by_handle.get_mut(&basket_handle) else {
            let error = CompleteSetForwardingError::UnknownExecutorLegIdentity {
                execution_client_id: self.execution_client_id.clone(),
                client_order_id: client_order_id.to_string(),
            };
            self.shell.record_failure(error.clone());
            return Err(error);
        };
        self.shell.forward_executor_event(basket, event)
    }

    #[cfg(test)]
    fn insert_test_basket(
        &mut self,
        basket_handle: &str,
        basket: BoltV3BasketExecutionState,
    ) -> Result<(), CompleteSetForwardingError> {
        let client_order_ids = basket.client_order_ids();
        if self.baskets_by_handle.contains_key(basket_handle) {
            return Err(CompleteSetForwardingError::DuplicateBasketHandle(
                basket_handle.to_string(),
            ));
        }
        if let Some(client_order_id) = client_order_ids
            .iter()
            .find(|client_order_id| {
                self.basket_handle_by_client_order_id
                    .contains_key(client_order_id.as_str())
            })
            .cloned()
        {
            return Err(CompleteSetForwardingError::DuplicateExecutorLegIdentity(
                client_order_id,
            ));
        }
        for client_order_id in client_order_ids {
            self.basket_handle_by_client_order_id
                .insert(client_order_id, basket_handle.to_string());
        }
        self.baskets_by_handle
            .insert(basket_handle.to_string(), basket);
        Ok(())
    }

    #[cfg(test)]
    fn test_basket(&self, client_order_id: &str) -> Option<&BoltV3BasketExecutionState> {
        self.basket_handle_by_client_order_id
            .get(client_order_id)
            .and_then(|basket_handle| self.baskets_by_handle.get(basket_handle))
    }
}

impl CompleteSetArbitrageShell {
    pub fn new(strategy_id: impl Into<String>) -> Self {
        Self {
            strategy_id: strategy_id.into(),
            forwarded_event_count: u64::MIN,
            failed_event_count: u64::MIN,
            last_failure: None,
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

    pub fn failed_event_count(&self) -> u64 {
        self.failed_event_count
    }

    pub fn last_failure(&self) -> Option<&CompleteSetForwardingError> {
        self.last_failure.as_ref()
    }

    pub fn forward_executor_event(
        &mut self,
        basket: &mut BoltV3BasketExecutionState,
        event: BoltV3BasketExecutionEvent,
    ) -> Result<(), CompleteSetForwardingError> {
        match basket.apply_event(event) {
            Ok(()) => {
                self.forwarded_event_count = self.forwarded_event_count.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                let error = CompleteSetForwardingError::Executor(error);
                self.record_failure(error.clone());
                Err(error)
            }
        }
    }

    fn record_failure(&mut self, error: CompleteSetForwardingError) {
        self.failed_event_count = self.failed_event_count.saturating_add(1);
        self.last_failure = Some(error);
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

#[cfg(test)]
mod tests;
