//! Thin complete-set strategy shell.
//!
//! The shell is intentionally limited to basket intent and NT-event forwarding.
//! Admission, venue mutation, fillability, sizing, rounding, and repair/unwind
//! remain in shared outcome-group execution modules.

use std::any::type_name;

use nautilus_common::messages::execution::SubmitOrderList;
use nautilus_model::orders::OrderList;

use crate::{
    bolt_v3_basket_execution::{
        BoltV3BasketExecutionError, BoltV3BasketExecutionEvent, BoltV3BasketExecutionState,
        BoltV3BasketSettlementSignal,
    },
    bolt_v3_outcome_group_sources::COMPLETE_SET_ARBITRAGE_KEY,
};

pub const KEY: &str = COMPLETE_SET_ARBITRAGE_KEY;

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

    pub fn nt_submit_contract(&self) -> CompleteSetNtSubmitContract {
        nt_submit_contract()
    }

    pub fn forward_executor_event(
        &mut self,
        basket: &mut BoltV3BasketExecutionState,
        event: BoltV3BasketExecutionEvent,
    ) -> Result<(), BoltV3BasketExecutionError> {
        basket.apply_event(event)?;
        self.forwarded_event_count = self.forwarded_event_count.saturating_add(u64::from(true));
        Ok(())
    }
}

pub fn nt_submit_contract() -> CompleteSetNtSubmitContract {
    CompleteSetNtSubmitContract {
        order_list_type: type_name::<OrderList>(),
        submit_order_list_type: type_name::<SubmitOrderList>(),
    }
}

pub fn live_settlement_policy() -> CompleteSetSettlementPolicy {
    CompleteSetSettlementPolicy::RejectUntilReachableNtSignal
}

pub fn forward_settlement_signal(
    basket: &mut BoltV3BasketExecutionState,
) -> Result<(), BoltV3BasketExecutionError> {
    basket.apply_event(BoltV3BasketExecutionEvent::SettlementSignal(
        BoltV3BasketSettlementSignal::LiveSettlementRejectedUntilReachableNtSignal,
    ))
}
