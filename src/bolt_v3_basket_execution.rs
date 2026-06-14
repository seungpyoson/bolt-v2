use std::{any::type_name, collections::BTreeMap, fmt};

use nautilus_common::messages::execution::{
    BatchCancelOrders, CancelAllOrders, CancelOrder, ModifyOrder, SubmitOrderList,
};
use nautilus_model::{enums::OrderSide, orders::OrderList};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_kill_switch::{
        KillSwitchEvent, KillSwitchHaltTrigger, KillSwitchState, KillSwitchTransitionContext,
        transition_kill_switch_state,
    },
    bolt_v3_kill_switch_store::{KillSwitchStore, KillSwitchStoreError},
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};

pub const REPAIR_EDGE_INEQUALITY: &str = "min(M * (filled_qty + repair_qty)) - (filled_cost + repair_cost) preserves admitted absolute edge floor and normalized edge_bps floor";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketExecutionConfig {
    pub repair: BoltV3BasketRepairPolicy,
    pub unwind: BoltV3BasketUnwindPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketRepairPolicy {
    pub max_retries: u32,
    pub max_book_age_ms: u64,
    pub max_slippage_bps: u32,
    pub max_depth_levels: u32,
    pub allow_unwind_when_repair_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketUnwindPolicy {
    pub max_retries: u32,
    pub max_book_age_ms: u64,
    pub max_slippage_bps: u32,
    pub max_depth_levels: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoltV3BasketExecutionStatus {
    Candidate,
    Reserved,
    Submitting,
    Partial,
    Complete,
    Repair,
    Unwind,
    Stuck,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoltV3BasketFillSource {
    Strategy,
    Hip4SyntheticSettlement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3BasketExecutionSubmitDisposition {
    ReuseNtSubmitOrderList,
    RejectForNow,
    ReviewedFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketExecutionLegIntent {
    pub leg_id: String,
    pub instrument_id: String,
    pub venue: String,
    pub client_order_id: String,
    pub side: OrderSide,
    pub quantity: Decimal,
    pub notional: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketExecutionState {
    basket_id: String,
    strategy_id: String,
    execution_client_id: String,
    status: BoltV3BasketExecutionStatus,
    legs: Vec<BoltV3BasketExecutionLegState>,
    payout_matrix: Vec<Vec<Decimal>>,
    admitted_absolute_edge_floor: Decimal,
    admitted_edge_bps_floor: Decimal,
    config: BoltV3BasketExecutionConfig,
    order_list_id: Option<String>,
    reservation_held: bool,
    unresolved_real_exposure: bool,
    settled: bool,
    fill_sources: Vec<BoltV3BasketFillSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BoltV3BasketExecutionLegState {
    leg_id: String,
    instrument_id: String,
    target_quantity: Decimal,
    filled_quantity: Decimal,
    filled_cost: Decimal,
    client_order_id: Option<String>,
    venue_order_id: Option<String>,
}

impl BoltV3BasketExecutionState {
    #[allow(clippy::too_many_arguments)]
    pub fn candidate(
        basket_id: &str,
        strategy_id: &str,
        execution_client_id: &str,
        legs: Vec<(&str, &str, Decimal)>,
        payout_matrix: Vec<Vec<Decimal>>,
        admitted_absolute_edge_floor: Decimal,
        admitted_edge_bps_floor: Decimal,
        config: BoltV3BasketExecutionConfig,
    ) -> Result<Self, BoltV3BasketExecutionError> {
        if legs.is_empty() {
            return Err(BoltV3BasketExecutionError::MissingLegs);
        }
        if payout_matrix.iter().any(|row| row.len() != legs.len()) {
            return Err(BoltV3BasketExecutionError::InvalidPayoutMatrix);
        }
        Ok(Self {
            basket_id: basket_id.to_string(),
            strategy_id: strategy_id.to_string(),
            execution_client_id: execution_client_id.to_string(),
            status: BoltV3BasketExecutionStatus::Candidate,
            legs: legs
                .into_iter()
                .map(
                    |(leg_id, instrument_id, target_quantity)| BoltV3BasketExecutionLegState {
                        leg_id: leg_id.to_string(),
                        instrument_id: instrument_id.to_string(),
                        target_quantity,
                        filled_quantity: Decimal::ZERO,
                        filled_cost: Decimal::ZERO,
                        client_order_id: None,
                        venue_order_id: None,
                    },
                )
                .collect(),
            payout_matrix,
            admitted_absolute_edge_floor,
            admitted_edge_bps_floor,
            config,
            order_list_id: None,
            reservation_held: false,
            unresolved_real_exposure: false,
            settled: false,
            fill_sources: Vec::new(),
        })
    }

    pub fn status(&self) -> BoltV3BasketExecutionStatus {
        self.status
    }

    pub fn order_list_id(&self) -> Option<&str> {
        self.order_list_id.as_deref()
    }

    pub fn client_order_ids(&self) -> Vec<String> {
        self.legs
            .iter()
            .filter_map(|leg| leg.client_order_id.clone())
            .collect()
    }

    pub fn reservation_held(&self) -> bool {
        self.reservation_held
    }

    pub fn unresolved_real_exposure(&self) -> bool {
        self.unresolved_real_exposure
    }

    pub fn settled(&self) -> bool {
        self.settled
    }

    pub fn fill_sources(&self) -> Vec<BoltV3BasketFillSource> {
        self.fill_sources.clone()
    }

    pub fn build_same_venue_submit_command(
        &mut self,
        disposition: BoltV3BasketExecutionSubmitDisposition,
        order_list_id: &str,
        leg_intents: Vec<BoltV3BasketExecutionLegIntent>,
    ) -> Result<BoltV3BasketNtSubmitCommand, BoltV3BasketExecutionError> {
        if disposition != BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList {
            return Err(BoltV3BasketExecutionError::SubmitModeRejected);
        }
        if self.status != BoltV3BasketExecutionStatus::Reserved {
            return Err(BoltV3BasketExecutionError::InvalidStateTransition);
        }
        let Some(first_venue) = leg_intents.first().map(|leg| leg.venue.clone()) else {
            return Err(BoltV3BasketExecutionError::MissingLegs);
        };
        if leg_intents
            .iter()
            .any(|leg| leg.venue.as_str() != first_venue.as_str())
        {
            return Err(BoltV3BasketExecutionError::MixedVenueBasket);
        }
        if leg_intents.len() != self.legs.len() {
            return Err(BoltV3BasketExecutionError::LegShapeMismatch);
        }

        let mut client_order_ids = Vec::with_capacity(leg_intents.len());
        for intent in leg_intents {
            let leg = self
                .legs
                .iter_mut()
                .find(|leg| leg.leg_id == intent.leg_id)
                .ok_or(BoltV3BasketExecutionError::LegShapeMismatch)?;
            leg.client_order_id = Some(intent.client_order_id.clone());
            client_order_ids.push(intent.client_order_id);
        }

        self.order_list_id = Some(order_list_id.to_string());
        self.status = BoltV3BasketExecutionStatus::Submitting;

        Ok(BoltV3BasketNtSubmitCommand {
            order_list_id: order_list_id.to_string(),
            client_order_ids,
            venue: first_venue,
        })
    }

    pub fn apply_event(
        &mut self,
        event: BoltV3BasketExecutionEvent,
    ) -> Result<(), BoltV3BasketExecutionError> {
        match event {
            BoltV3BasketExecutionEvent::ReservationPersisted => {
                if self.status != BoltV3BasketExecutionStatus::Candidate {
                    return Err(BoltV3BasketExecutionError::InvalidStateTransition);
                }
                self.status = BoltV3BasketExecutionStatus::Reserved;
                self.reservation_held = true;
            }
            BoltV3BasketExecutionEvent::VenueOrderId {
                client_order_id,
                venue_order_id,
            } => {
                self.leg_for_client_order_mut(&client_order_id)?
                    .venue_order_id = Some(venue_order_id);
            }
            BoltV3BasketExecutionEvent::LegFill {
                client_order_id,
                venue_order_id,
                quantity,
                cost,
                source,
            } => {
                let leg = self.leg_for_client_order_mut(&client_order_id)?;
                if let Some(venue_order_id) = venue_order_id {
                    leg.venue_order_id = Some(venue_order_id);
                }
                leg.filled_quantity += quantity;
                leg.filled_cost += cost;
                self.fill_sources.push(source);
                if source == BoltV3BasketFillSource::Hip4SyntheticSettlement {
                    self.settled = true;
                    self.status = BoltV3BasketExecutionStatus::Stuck;
                    self.unresolved_real_exposure = true;
                    self.reservation_held = true;
                    return Ok(());
                }
                self.refresh_fill_status();
            }
            BoltV3BasketExecutionEvent::CancelRejected { .. }
            | BoltV3BasketExecutionEvent::RetryBudgetExhausted { .. } => {
                self.status = BoltV3BasketExecutionStatus::Stuck;
                self.unresolved_real_exposure = true;
                self.reservation_held = true;
            }
            BoltV3BasketExecutionEvent::SettlementSignal(
                BoltV3BasketSettlementSignal::LiveSettlementRejectedUntilReachableNtSignal,
            ) => {
                return Err(BoltV3BasketExecutionError::LiveSettlementSignalUnreachable);
            }
            BoltV3BasketExecutionEvent::SettlementSignal(
                BoltV3BasketSettlementSignal::ReachableNtClose,
            ) => {
                self.settled = true;
                if self.unresolved_real_exposure {
                    self.status = BoltV3BasketExecutionStatus::Stuck;
                    self.reservation_held = true;
                }
            }
            BoltV3BasketExecutionEvent::TerminalClose => {
                if self.unresolved_real_exposure
                    || self.status == BoltV3BasketExecutionStatus::Stuck
                {
                    return Err(BoltV3BasketExecutionError::UnresolvedExposure);
                }
                self.status = BoltV3BasketExecutionStatus::Closed;
                self.reservation_held = false;
            }
        }
        Ok(())
    }

    pub fn reconcile_restart(
        &mut self,
        reports: &[BoltV3BasketRestartReport],
    ) -> Result<(), BoltV3BasketExecutionError> {
        for report in reports {
            let Some(index) = self.leg_index_for_report(report) else {
                self.status = BoltV3BasketExecutionStatus::Stuck;
                self.unresolved_real_exposure = true;
                self.reservation_held = true;
                return Ok(());
            };
            let leg = self
                .legs
                .get_mut(index)
                .ok_or(BoltV3BasketExecutionError::LegShapeMismatch)?;
            leg.filled_quantity = report.filled_quantity;
            leg.filled_cost = report.filled_cost;
            if leg.venue_order_id.is_none() {
                leg.venue_order_id = report.venue_order_id.clone();
            }
        }
        self.refresh_fill_status();
        Ok(())
    }

    fn leg_for_client_order_mut(
        &mut self,
        client_order_id: &str,
    ) -> Result<&mut BoltV3BasketExecutionLegState, BoltV3BasketExecutionError> {
        self.legs
            .iter_mut()
            .find(|leg| leg.client_order_id.as_deref() == Some(client_order_id))
            .ok_or(BoltV3BasketExecutionError::UnknownClientOrderId)
    }

    fn leg_index_for_report(&self, report: &BoltV3BasketRestartReport) -> Option<usize> {
        if let Some(client_order_id) = report.client_order_id.as_deref()
            && let Some(index) = self
                .legs
                .iter()
                .position(|leg| leg.client_order_id.as_deref() == Some(client_order_id))
        {
            return Some(index);
        }
        if let Some(venue_order_id) = report.venue_order_id.as_deref() {
            return self
                .legs
                .iter()
                .position(|leg| leg.venue_order_id.as_deref() == Some(venue_order_id));
        }
        None
    }

    fn refresh_fill_status(&mut self) {
        let any_fill = self
            .legs
            .iter()
            .any(|leg| leg.filled_quantity > Decimal::ZERO);
        let complete = self
            .legs
            .iter()
            .all(|leg| leg.filled_quantity >= leg.target_quantity);
        if complete {
            self.status = BoltV3BasketExecutionStatus::Complete;
            self.unresolved_real_exposure = false;
        } else if any_fill {
            self.status = BoltV3BasketExecutionStatus::Partial;
            self.unresolved_real_exposure = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketNtSubmitCommand {
    pub order_list_id: String,
    pub client_order_ids: Vec<String>,
    pub venue: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketExecutionEvent {
    ReservationPersisted,
    VenueOrderId {
        client_order_id: String,
        venue_order_id: String,
    },
    LegFill {
        client_order_id: String,
        venue_order_id: Option<String>,
        quantity: Decimal,
        cost: Decimal,
        source: BoltV3BasketFillSource,
    },
    CancelRejected {
        reason: String,
    },
    RetryBudgetExhausted {
        reason: String,
    },
    SettlementSignal(BoltV3BasketSettlementSignal),
    TerminalClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3BasketSettlementSignal {
    LiveSettlementRejectedUntilReachableNtSignal,
    ReachableNtClose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketRepairInput {
    pub admitted_target_quantities: Vec<(String, Decimal)>,
    pub filled_quantities: Vec<(String, Decimal)>,
    pub filled_cost: Decimal,
    pub payout_matrix: Vec<Vec<Decimal>>,
    pub executable_repair_legs: Vec<BoltV3ExecutableRepairLeg>,
    pub admitted_absolute_edge_floor: Decimal,
    pub admitted_edge_bps_floor: Decimal,
    pub remaining_retry_budget: u32,
    pub now_unix_ms: u64,
}

impl BoltV3BasketRepairInput {
    pub fn plan_repair(&self, policy: &BoltV3BasketRepairPolicy) -> BoltV3BasketRepairOutcome {
        if self.remaining_retry_budget == 0 || self.remaining_retry_budget > policy.max_retries {
            return BoltV3BasketRepairOutcome::Stuck {
                reason: "repair retry budget is not available".to_string(),
            };
        }
        if !self
            .executable_repair_legs
            .iter()
            .all(|leg| executable_leg_is_fresh_and_bounded(self.now_unix_ms, leg, policy))
        {
            return BoltV3BasketRepairOutcome::Stuck {
                reason: "fresh executable repair books are required".to_string(),
            };
        }

        let filled = quantity_map(&self.filled_quantities);
        let mut repair = BTreeMap::new();
        let mut residuals = Vec::new();
        let mut repair_cost = Decimal::ZERO;
        for (leg_id, target_quantity) in &self.admitted_target_quantities {
            let filled_quantity = filled.get(leg_id).copied().unwrap_or(Decimal::ZERO);
            if filled_quantity >= *target_quantity {
                continue;
            }
            let residual_quantity = *target_quantity - filled_quantity;
            let Some(executable_leg) = self
                .executable_repair_legs
                .iter()
                .find(|leg| leg.leg_id == *leg_id && leg.quantity >= residual_quantity)
            else {
                return BoltV3BasketRepairOutcome::Stuck {
                    reason: "fresh executable repair books are required".to_string(),
                };
            };
            repair.insert(leg_id.clone(), residual_quantity);
            residuals.push((leg_id.clone(), residual_quantity));
            repair_cost += executable_leg.cost;
        }

        let projected_quantities =
            projected_quantities(&self.admitted_target_quantities, &filled, &repair);
        let guaranteed_payout = guaranteed_payout(&self.payout_matrix, &projected_quantities);
        let total_cost = self.filled_cost + repair_cost;
        let projected_absolute_edge = guaranteed_payout - total_cost;
        let projected_edge_bps = edge_bps(projected_absolute_edge, total_cost);

        if projected_absolute_edge >= self.admitted_absolute_edge_floor
            && projected_edge_bps >= self.admitted_edge_bps_floor
        {
            return BoltV3BasketRepairOutcome::Repair {
                residuals,
                projected_absolute_edge,
                projected_edge_bps,
            };
        }

        if policy.allow_unwind_when_repair_denied {
            BoltV3BasketRepairOutcome::Unwind {
                reason: "repair cannot preserve admitted edge floors".to_string(),
            }
        } else {
            BoltV3BasketRepairOutcome::Stuck {
                reason: "repair cannot preserve admitted edge floors".to_string(),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketRepairOutcome {
    Repair {
        residuals: Vec<(String, Decimal)>,
        projected_absolute_edge: Decimal,
        projected_edge_bps: Decimal,
    },
    Unwind {
        reason: String,
    },
    Stuck {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3ExecutableRepairLeg {
    pub leg_id: String,
    pub quantity: Decimal,
    pub cost: Decimal,
    pub observed_unix_ms: u64,
    pub slippage_bps: u32,
    pub depth_levels: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketUnwindInput {
    pub filled_quantities: Vec<(String, Decimal)>,
    pub executable_unwind_legs: Vec<BoltV3ExecutableRepairLeg>,
    pub now_unix_ms: u64,
    pub settled: bool,
    pub remaining_retry_budget: u32,
}

impl BoltV3BasketUnwindInput {
    pub fn plan_unwind(&self, policy: &BoltV3BasketUnwindPolicy) -> BoltV3BasketUnwindOutcome {
        if self.settled {
            return BoltV3BasketUnwindOutcome::Stuck {
                reason: "unwind is forbidden after durable settlement".to_string(),
            };
        }
        if self.remaining_retry_budget == 0 || self.remaining_retry_budget > policy.max_retries {
            return BoltV3BasketUnwindOutcome::Stuck {
                reason: "unwind retry budget is not available".to_string(),
            };
        }
        if !self
            .executable_unwind_legs
            .iter()
            .all(|leg| executable_unwind_leg_is_fresh_and_bounded(self.now_unix_ms, leg, policy))
        {
            return BoltV3BasketUnwindOutcome::Stuck {
                reason: "fresh executable unwind books are required".to_string(),
            };
        }
        let executable = quantity_map(
            &self
                .executable_unwind_legs
                .iter()
                .map(|leg| (leg.leg_id.clone(), leg.quantity))
                .collect::<Vec<_>>(),
        );
        let reductions = self
            .filled_quantities
            .iter()
            .filter_map(|(leg_id, filled_quantity)| {
                executable.get(leg_id).map(|quantity| {
                    let reduction = if quantity < filled_quantity {
                        *quantity
                    } else {
                        *filled_quantity
                    };
                    (leg_id.clone(), reduction)
                })
            })
            .collect();
        BoltV3BasketUnwindOutcome::Unwind { reductions }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketUnwindOutcome {
    Unwind { reductions: Vec<(String, Decimal)> },
    Stuck { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketRestartReport {
    pub instrument_id: String,
    pub client_order_id: Option<String>,
    pub venue_order_id: Option<String>,
    pub filled_quantity: Decimal,
    pub filled_cost: Decimal,
    pub report_class: BoltV3ExternalReportClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3ExternalReportClass {
    StrategyOwned,
    Unclaimed,
    EngineClassifiedExternal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NtOrderManagementContract {
    pub order_list_type: &'static str,
    pub submit_order_list_type: &'static str,
    pub cancel_order_type: &'static str,
    pub batch_cancel_orders_type: &'static str,
    pub cancel_all_orders_type: &'static str,
    pub modify_order_type: &'static str,
}

pub fn nt_order_management_contract() -> BoltV3NtOrderManagementContract {
    BoltV3NtOrderManagementContract {
        order_list_type: type_name::<OrderList>(),
        submit_order_list_type: type_name::<SubmitOrderList>(),
        cancel_order_type: type_name::<CancelOrder>(),
        batch_cancel_orders_type: type_name::<BatchCancelOrders>(),
        cancel_all_orders_type: type_name::<CancelAllOrders>(),
        modify_order_type: type_name::<ModifyOrder>(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3ExecutorEventIntegrationContract {
    pub forwarded_nt_events: Vec<&'static str>,
    pub strategy_shell_may_call_submit_admission: bool,
    pub strategy_shell_may_mutate_venue: bool,
    pub shared_executor_owns_state_transitions: bool,
}

pub fn executor_event_integration_contract() -> BoltV3ExecutorEventIntegrationContract {
    BoltV3ExecutorEventIntegrationContract {
        forwarded_nt_events: vec!["order", "fill", "cancel", "instrument_status", "settlement"],
        strategy_shell_may_call_submit_admission: false,
        strategy_shell_may_mutate_venue: false,
        shared_executor_owns_state_transitions: true,
    }
}

pub fn trip_stuck_basket_kill_switch(
    basket: &BoltV3BasketExecutionState,
    kill_switch_store: &KillSwitchStore,
    submit_admission: &BoltV3SubmitAdmissionState,
    source_timestamp_unix_nanos: u64,
) -> Result<KillSwitchState, BoltV3BasketExecutionError> {
    if basket.status != BoltV3BasketExecutionStatus::Stuck || !basket.unresolved_real_exposure {
        return Err(BoltV3BasketExecutionError::NoStuckExposure);
    }

    let trigger = KillSwitchHaltTrigger::basket_execution_stuck(
        basket.strategy_id.clone(),
        source_timestamp_unix_nanos,
        basket.basket_id.clone(),
    );
    let halting = transition_kill_switch_state(
        KillSwitchState::Armed,
        KillSwitchEvent::HaltTriggered(trigger),
        kill_switch_transition_context(),
    )?;
    kill_switch_store.write_state(&halting)?;
    let halted = transition_kill_switch_state(
        halting,
        KillSwitchEvent::DurableHaltEvidenceRecorded,
        kill_switch_transition_context(),
    )?;
    kill_switch_store.write_state(&halted)?;
    submit_admission.replace_kill_switch_state(halted.clone());
    Ok(halted)
}

fn kill_switch_transition_context() -> KillSwitchTransitionContext {
    KillSwitchTransitionContext {
        state_write_succeeded: true,
        durable_halt_evidence_recorded: true,
        operator_authorized: false,
        manual_reset_evidence_valid: false,
        mandatory_proof_streams_fresh: false,
        no_outstanding_order_risk: false,
        no_open_positions: false,
        no_pending_entry_risk: false,
    }
}

fn executable_leg_is_fresh_and_bounded(
    now_unix_ms: u64,
    leg: &BoltV3ExecutableRepairLeg,
    policy: &BoltV3BasketRepairPolicy,
) -> bool {
    now_unix_ms.saturating_sub(leg.observed_unix_ms) <= policy.max_book_age_ms
        && leg.slippage_bps <= policy.max_slippage_bps
        && leg.depth_levels <= policy.max_depth_levels
}

fn executable_unwind_leg_is_fresh_and_bounded(
    now_unix_ms: u64,
    leg: &BoltV3ExecutableRepairLeg,
    policy: &BoltV3BasketUnwindPolicy,
) -> bool {
    now_unix_ms.saturating_sub(leg.observed_unix_ms) <= policy.max_book_age_ms
        && leg.slippage_bps <= policy.max_slippage_bps
        && leg.depth_levels <= policy.max_depth_levels
}

fn quantity_map(values: &[(String, Decimal)]) -> BTreeMap<String, Decimal> {
    values.iter().cloned().collect()
}

fn projected_quantities(
    targets: &[(String, Decimal)],
    filled: &BTreeMap<String, Decimal>,
    repair: &BTreeMap<String, Decimal>,
) -> Vec<Decimal> {
    targets
        .iter()
        .map(|(leg_id, _)| {
            filled.get(leg_id).copied().unwrap_or(Decimal::ZERO)
                + repair.get(leg_id).copied().unwrap_or(Decimal::ZERO)
        })
        .collect()
}

fn guaranteed_payout(payout_matrix: &[Vec<Decimal>], quantities: &[Decimal]) -> Decimal {
    payout_matrix
        .iter()
        .map(|row| {
            row.iter()
                .zip(quantities.iter())
                .fold(Decimal::ZERO, |total, (payout, quantity)| {
                    total + (*payout * *quantity)
                })
        })
        .min()
        .unwrap_or(Decimal::ZERO)
}

fn edge_bps(edge: Decimal, total_cost: Decimal) -> Decimal {
    if total_cost <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    ((edge / total_cost) * Decimal::from(10_000u32)).trunc()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketExecutionError {
    MissingLegs,
    InvalidPayoutMatrix,
    SubmitModeRejected,
    InvalidStateTransition,
    MixedVenueBasket,
    LegShapeMismatch,
    UnknownClientOrderId,
    LiveSettlementSignalUnreachable,
    UnresolvedExposure,
    NoStuckExposure,
    KillSwitchTransition(String),
    KillSwitchStore(String),
}

impl fmt::Display for BoltV3BasketExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLegs => write!(f, "basket execution requires at least one leg"),
            Self::InvalidPayoutMatrix => {
                write!(f, "basket execution payout matrix shape is invalid")
            }
            Self::SubmitModeRejected => write!(f, "basket execution submit mode rejected"),
            Self::InvalidStateTransition => {
                write!(f, "basket execution state transition is invalid")
            }
            Self::MixedVenueBasket => write!(
                f,
                "basket execution requires one venue for NT OrderList submit"
            ),
            Self::LegShapeMismatch => write!(
                f,
                "basket execution leg shape does not match durable basket"
            ),
            Self::UnknownClientOrderId => write!(
                f,
                "basket execution event references an unknown client order id"
            ),
            Self::LiveSettlementSignalUnreachable => {
                write!(f, "live settlement signal is not reachable")
            }
            Self::UnresolvedExposure => {
                write!(f, "basket execution cannot close unresolved exposure")
            }
            Self::NoStuckExposure => write!(
                f,
                "basket execution kill switch requires stuck real exposure"
            ),
            Self::KillSwitchTransition(reason) => write!(
                f,
                "basket execution kill switch transition failed: {reason}"
            ),
            Self::KillSwitchStore(reason) => {
                write!(f, "basket execution kill switch store failed: {reason}")
            }
        }
    }
}

impl std::error::Error for BoltV3BasketExecutionError {}

impl From<crate::bolt_v3_kill_switch::KillSwitchTransitionError> for BoltV3BasketExecutionError {
    fn from(error: crate::bolt_v3_kill_switch::KillSwitchTransitionError) -> Self {
        Self::KillSwitchTransition(format!("{error:?}"))
    }
}

impl From<KillSwitchStoreError> for BoltV3BasketExecutionError {
    fn from(error: KillSwitchStoreError) -> Self {
        Self::KillSwitchStore(format!("{error:?}"))
    }
}
