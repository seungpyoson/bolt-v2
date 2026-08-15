//! Shared maker order-command dispatcher.
//!
//! The maker compile layer produces typed commands. This module binds those
//! commands to the existing NT order-construction path and a caller-provided
//! runtime sink, so strategies do not own maker submit/cancel/modify mechanics.

use std::cell::RefMut;

use anyhow::Result;
use nautilus_common::factories::OrderFactory;
use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientOrderId, InstrumentId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};

use crate::{
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_quote_control::{MakerQuoteBudgetProposal, MakerQuoteCommandProposal},
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
    bolt_v3_order_execution::{
        BoltV3RestingCommitDisposition, BoltV3RestingRegistrationCommitParticipant,
        BoltV3RestingSubmitTransactionOutcome,
    },
    bolt_v3_order_intent::build_nt_order,
    bolt_v3_quote_lifecycle::{
        Leg, LifecycleAction, MakerQuoteLifecycleHandle, MarketAction, MarketQuote,
    },
    bolt_v3_requote_budget::{RequoteBudgetPair, RequoteBudgetReservation},
};

#[derive(Debug, Clone, PartialEq)]
pub struct MakerQuoteTransactionContext {
    pub market: MarketQuote,
    pub budget: RequoteBudgetPair,
    pub proposal: MakerQuoteCommandProposal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerOrderDispatchInput<'a> {
    pub command: &'a MakerCompiledOrderCommand,
    pub submit_order_prefix: &'a str,
    pub quote_transaction: Option<MakerQuoteTransactionContext>,
}

#[derive(Debug)]
struct MakerQuoteTransactionParticipant {
    market: MarketQuote,
    budget: RequoteBudgetPair,
    proposal: MakerQuoteCommandProposal,
    generation: Option<u64>,
    reservation: Option<RequoteBudgetReservation>,
    reservation_was_prepaid: bool,
    phase: MakerQuoteTransactionPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MakerQuoteTransactionPhase {
    Proposed,
    Armed,
    SinkInvoked,
    Settled,
}

impl MakerQuoteTransactionParticipant {
    fn new(context: MakerQuoteTransactionContext) -> Self {
        Self {
            market: context.market,
            budget: context.budget,
            proposal: context.proposal,
            generation: None,
            reservation: None,
            reservation_was_prepaid: false,
            phase: MakerQuoteTransactionPhase::Proposed,
        }
    }

    fn take_reservation(&mut self) -> Result<RequoteBudgetReservation> {
        self.reservation
            .take()
            .ok_or_else(|| anyhow::anyhow!("maker quote transaction lost its budget reservation"))
    }
}

impl BoltV3RestingRegistrationCommitParticipant for MakerQuoteTransactionParticipant {
    fn requote_budget(&self) -> Option<RequoteBudgetPair> {
        Some(self.budget.clone())
    }

    fn maker_lifecycle(&self) -> Option<MakerQuoteLifecycleHandle> {
        Some(MakerQuoteLifecycleHandle::new(
            self.market.clone(),
            self.proposal.lifecycle.leg(),
        ))
    }

    fn arm_at_generation(&mut self, generation: u64) -> Result<()> {
        anyhow::ensure!(
            self.phase == MakerQuoteTransactionPhase::Proposed,
            "maker quote transaction was armed more than once"
        );
        self.market
            .arm_leg_transaction(self.proposal.lifecycle, generation)?;
        let reservation = match self.proposal.budget {
            MakerQuoteBudgetProposal::Reserve(proposal) => self.budget.reserve(proposal),
            MakerQuoteBudgetProposal::Prepaid { generation, .. } => self
                .market
                .take_prepaid_reservation(self.proposal.lifecycle.leg(), generation)
                .map_err(|_| {
                    crate::bolt_v3_requote_budget::RequoteBudgetReservationDenied::StaleReservation
                }),
        };
        let reservation = match reservation {
            Ok(reservation) => reservation,
            Err(error) => {
                let restored = self
                    .market
                    .abort_leg_transaction(self.proposal.lifecycle, generation);
                anyhow::ensure!(restored, "budget denial failed to restore lifecycle arm");
                return Err(anyhow::anyhow!(
                    "maker quote budget reservation denied: {error:?}"
                ));
            }
        };
        self.reservation_was_prepaid = matches!(
            self.proposal.budget,
            MakerQuoteBudgetProposal::Prepaid { .. }
        );
        self.generation = Some(generation);
        self.reservation = Some(reservation);
        self.phase = MakerQuoteTransactionPhase::Armed;
        Ok(())
    }

    fn mark_sink_invoked(&mut self, actor_now_ns: u64) -> Result<()> {
        anyhow::ensure!(
            self.phase == MakerQuoteTransactionPhase::Armed,
            "maker quote transaction reached the sink outside its armed phase"
        );
        let reservation = self
            .reservation
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("maker quote transaction lost its reservation"))?;
        reservation
            .mark_sink_invoked_at(actor_now_ns / NANOS_PER_MILLI_U64)
            .map_err(|error| {
                anyhow::anyhow!("maker quote budget sink accounting failed: {error:?}")
            })?;
        self.phase = MakerQuoteTransactionPhase::SinkInvoked;
        Ok(())
    }

    fn settle_at_generation(
        &mut self,
        generation: u64,
        disposition: BoltV3RestingCommitDisposition,
    ) -> Result<()> {
        if self.phase == MakerQuoteTransactionPhase::Proposed {
            anyhow::ensure!(
                matches!(disposition, BoltV3RestingCommitDisposition::PreSinkAborted),
                "maker quote transaction received a routed disposition before provisional arming"
            );
            self.phase = MakerQuoteTransactionPhase::Settled;
            return Ok(());
        }
        anyhow::ensure!(
            self.generation == Some(generation),
            "maker quote transaction generation is stale"
        );
        let lifecycle_settled = match disposition {
            BoltV3RestingCommitDisposition::Submitted => self
                .market
                .commit_leg_transaction(self.proposal.lifecycle, generation),
            BoltV3RestingCommitDisposition::CommandIssued => self
                .market
                .commit_leg_transaction(self.proposal.lifecycle, generation),
            BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid => self
                .market
                .commit_leg_transaction(self.proposal.lifecycle, generation),
            BoltV3RestingCommitDisposition::SinkRejected
            | BoltV3RestingCommitDisposition::PreSinkAborted => self
                .market
                .abort_leg_transaction(self.proposal.lifecycle, generation),
            BoltV3RestingCommitDisposition::CallbackRetired => self
                .market
                .retire_leg_transaction_from_callback(self.proposal.lifecycle, generation),
            BoltV3RestingCommitDisposition::RollbackInvariantFailed
            | BoltV3RestingCommitDisposition::PostSinkUnwind => self
                .market
                .poison_leg_transaction(self.proposal.lifecycle, generation),
        };
        anyhow::ensure!(
            lifecycle_settled,
            "maker quote lifecycle lost exact transaction generation"
        );
        let reservation = self.take_reservation()?;
        match disposition {
            BoltV3RestingCommitDisposition::PreSinkAborted if self.reservation_was_prepaid => self
                .market
                .retain_prepaid_reservation(self.proposal.lifecycle.leg(), reservation)?,
            BoltV3RestingCommitDisposition::PreSinkAborted => reservation
                .abort()
                .map_err(|error| anyhow::anyhow!("maker quote budget abort failed: {error:?}"))?,
            BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid => self
                .market
                .retain_prepaid_reservation(self.proposal.lifecycle.leg(), reservation)?,
            BoltV3RestingCommitDisposition::RollbackInvariantFailed
                if self.reservation_was_prepaid =>
            {
                self.market
                    .retain_prepaid_reservation(self.proposal.lifecycle.leg(), reservation)?
            }
            BoltV3RestingCommitDisposition::Submitted
            | BoltV3RestingCommitDisposition::CommandIssued
            | BoltV3RestingCommitDisposition::SinkRejected
            | BoltV3RestingCommitDisposition::CallbackRetired
            | BoltV3RestingCommitDisposition::RollbackInvariantFailed
            | BoltV3RestingCommitDisposition::PostSinkUnwind => reservation
                .commit()
                .map_err(|error| anyhow::anyhow!("maker quote budget commit failed: {error:?}"))?,
        }
        self.phase = MakerQuoteTransactionPhase::Settled;
        Ok(())
    }
}

impl Drop for MakerQuoteTransactionParticipant {
    fn drop(&mut self) {
        let Some(generation) = self.generation else {
            return;
        };
        match self.phase {
            MakerQuoteTransactionPhase::Proposed | MakerQuoteTransactionPhase::Settled => {}
            MakerQuoteTransactionPhase::Armed => {
                let _ = self
                    .market
                    .abort_leg_transaction(self.proposal.lifecycle, generation);
            }
            MakerQuoteTransactionPhase::SinkInvoked => {
                let _ = self
                    .market
                    .poison_leg_transaction(self.proposal.lifecycle, generation);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MakerOrderDispatchOutcome {
    SubmitAttempt {
        leg: Leg,
        instrument_id: InstrumentId,
        prepared_client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
        transaction: BoltV3RestingSubmitTransactionOutcome,
    },
    Canceled {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    },
    CanceledAll {
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    },
    Modified {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    },
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl MakerOrderDispatchOutcome {
    #[must_use]
    pub fn submitted_for_test(
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Self {
        Self::SubmitAttempt {
            leg,
            instrument_id,
            prepared_client_order_id: client_order_id,
            price,
            quantity,
            transaction: BoltV3RestingSubmitTransactionOutcome::submitted_with_linkage_for_test(
                instrument_id,
                OrderSide::Buy,
                price,
                quantity,
                client_order_id,
            ),
        }
    }

    #[must_use]
    pub fn policy_skipped_for_test(
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
    ) -> Self {
        Self::SubmitAttempt {
            leg,
            instrument_id,
            prepared_client_order_id: client_order_id,
            price,
            quantity,
            transaction: BoltV3RestingSubmitTransactionOutcome::policy_skipped_for_test(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerOrderCommandFailureKind {
    Lifecycle,
    Build,
    SubmitPreparation,
    Cancel,
    CancelAll,
    Modify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerOrderCommandFailure {
    kind: MakerOrderCommandFailureKind,
    diagnostic: String,
}

impl MakerOrderCommandFailure {
    pub(crate) fn new(kind: MakerOrderCommandFailureKind, error: impl std::fmt::Display) -> Self {
        Self {
            kind,
            diagnostic: error.to_string(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> MakerOrderCommandFailureKind {
        self.kind
    }

    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn for_test(kind: MakerOrderCommandFailureKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
        }
    }
}

impl std::fmt::Display for MakerOrderCommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "maker command {:?} failure: {}",
            self.kind, self.diagnostic
        )
    }
}

impl std::error::Error for MakerOrderCommandFailure {}

pub trait MakerOrderCommandSink {
    type PreparedSubmit;

    fn order_factory(&mut self) -> RefMut<'_, OrderFactory>;

    fn prepare_maker_order(&mut self, order: OrderAny) -> Result<Self::PreparedSubmit>;

    fn submit_maker_order(
        &mut self,
        prepared: Self::PreparedSubmit,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> BoltV3RestingSubmitTransactionOutcome;

    fn cancel_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> Result<()>;

    fn cancel_all_maker_orders(
        &mut self,
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()>;

    fn modify_maker_order(
        &mut self,
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        price: Price,
        quantity: Quantity,
        participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> Result<()>;
}

pub fn dispatch_maker_order_command(
    input: MakerOrderDispatchInput<'_>,
    sink: &mut impl MakerOrderCommandSink,
) -> std::result::Result<MakerOrderDispatchOutcome, MakerOrderCommandFailure> {
    match input.command {
        MakerCompiledOrderCommand::Submit {
            leg,
            template,
            inputs,
            fallback_price,
        } => {
            let quote_transaction = input.quote_transaction.ok_or_else(|| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker submit is missing its quote transaction proposal",
                )
            })?;
            if quote_transaction.proposal.action
                != (MarketAction::Leg {
                    leg: *leg,
                    action: LifecycleAction::Submit,
                })
            {
                return Err(MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker submit quote transaction does not match the compiled command",
                ));
            }
            let order = {
                // `order_factory()` now yields a `RefMut` guard (NT moved the strategy
                // `OrderFactory` behind `Rc<RefCell<_>>`). Scope it so the borrow of `sink`
                // is released before the `submit_maker_order` call below.
                let mut order_factory = sink.order_factory();
                build_nt_order(
                    &mut order_factory,
                    input.submit_order_prefix,
                    template,
                    *inputs,
                )
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Build, error)
                })?
            };
            let prepared_client_order_id = order.client_order_id();
            let instrument_id = order.instrument_id();
            let price = order.price().unwrap_or(*fallback_price);
            let quantity = order.quantity();
            let prepared = sink.prepare_maker_order(order).map_err(|error| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::SubmitPreparation,
                    error,
                )
            })?;
            let transaction = sink.submit_maker_order(
                prepared,
                Box::new(MakerQuoteTransactionParticipant::new(quote_transaction)),
            );
            Ok(MakerOrderDispatchOutcome::SubmitAttempt {
                leg: *leg,
                instrument_id,
                prepared_client_order_id,
                price,
                quantity,
                transaction,
            })
        }
        MakerCompiledOrderCommand::Cancel {
            leg,
            instrument_id,
            client_order_id,
        } => {
            let quote_transaction = input.quote_transaction.ok_or_else(|| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker cancel is missing its quote transaction proposal",
                )
            })?;
            if quote_transaction.proposal.action
                != (MarketAction::Leg {
                    leg: *leg,
                    action: LifecycleAction::Cancel,
                })
            {
                return Err(MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker cancel quote transaction does not match the compiled command",
                ));
            }
            sink.cancel_maker_order(
                *leg,
                *instrument_id,
                *client_order_id,
                Box::new(MakerQuoteTransactionParticipant::new(quote_transaction)),
            )
            .map_err(|error| {
                MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Cancel, error)
            })?;
            Ok(MakerOrderDispatchOutcome::Canceled {
                leg: *leg,
                instrument_id: *instrument_id,
                client_order_id: *client_order_id,
            })
        }
        MakerCompiledOrderCommand::CancelAll {
            leg,
            instrument_id,
            order_side,
        } => {
            sink.cancel_all_maker_orders(*leg, *instrument_id, *order_side)
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::CancelAll, error)
                })?;
            Ok(MakerOrderDispatchOutcome::CanceledAll {
                leg: *leg,
                instrument_id: *instrument_id,
                order_side: *order_side,
            })
        }
        MakerCompiledOrderCommand::Modify {
            leg,
            instrument_id,
            client_order_id,
            price,
            quantity,
        } => {
            let quote_transaction = input.quote_transaction.ok_or_else(|| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker modify is missing its quote transaction proposal",
                )
            })?;
            if quote_transaction.proposal.action
                != (MarketAction::Leg {
                    leg: *leg,
                    action: LifecycleAction::Modify,
                })
            {
                return Err(MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::Lifecycle,
                    "maker modify quote transaction does not match the compiled command",
                ));
            }
            sink.modify_maker_order(
                *leg,
                *instrument_id,
                *client_order_id,
                *price,
                *quantity,
                Box::new(MakerQuoteTransactionParticipant::new(quote_transaction)),
            )
            .map_err(|error| {
                MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Modify, error)
            })?;
            Ok(MakerOrderDispatchOutcome::Modified {
                leg: *leg,
                instrument_id: *instrument_id,
                client_order_id: *client_order_id,
                price: *price,
                quantity: *quantity,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_maker_quote_control::{
            MakerQuoteBudgetProposal, QuoteControlInput, drive_quote_leg,
        },
        bolt_v3_quote_lifecycle::{LegEvent, LegState},
        bolt_v3_requote_budget::RequoteBudget,
    };

    const NOW_MS: u64 = 1_000;
    const WINDOW_MS: u64 = 60_000;

    fn budget_pair(submit_cap: u64, rest_cap: u64) -> RequoteBudgetPair {
        RequoteBudgetPair::new(
            RequoteBudget::new(submit_cap, WINDOW_MS, 0),
            RequoteBudget::new(rest_cap, WINDOW_MS, 0),
        )
    }

    fn short_window_budget_pair(submit_cap: u64, rest_cap: u64) -> RequoteBudgetPair {
        RequoteBudgetPair::new(
            RequoteBudget::new(submit_cap, 1_000, 0),
            RequoteBudget::new(rest_cap, 1_000, 0),
        )
    }

    fn fresh_context(
        market: &MarketQuote,
        budget: &RequoteBudgetPair,
        leg: Leg,
    ) -> MakerQuoteTransactionContext {
        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg,
                desired_price: 0.5,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS,
            },
        );
        MakerQuoteTransactionContext {
            market: market.clone(),
            budget: budget.clone(),
            proposal: decision.proposal.expect("fresh quote must be proposed"),
        }
    }

    fn resting_context(
        market: &MarketQuote,
        budget: &RequoteBudgetPair,
    ) -> MakerQuoteTransactionContext {
        let mut market_handle = market.clone();
        market_handle.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market_handle.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: Some(0.5),
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS,
            },
        );
        MakerQuoteTransactionContext {
            market: market.clone(),
            budget: budget.clone(),
            proposal: decision.proposal.expect("requote cancel must be proposed"),
        }
    }

    fn settle_fresh(
        disposition: BoltV3RestingCommitDisposition,
        mark_sink: bool,
    ) -> (MarketQuote, RequoteBudgetPair) {
        let market = MarketQuote::new(false);
        let budget = budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));
        participant.arm_at_generation(1).unwrap();
        if mark_sink {
            participant
                .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
                .expect("participant should reach the sink");
        }
        participant.settle_at_generation(1, disposition).unwrap();
        (market, budget)
    }

    #[test]
    fn pre_sink_abort_restores_lifecycle_and_budget_while_submit_commits_both() {
        let (aborted_market, aborted_budget) =
            settle_fresh(BoltV3RestingCommitDisposition::PreSinkAborted, false);
        assert_eq!(aborted_market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(aborted_budget.outstanding_submit_cost(), 0);
        assert_eq!(aborted_budget.submit_commands_in_window(), 0);
        assert_eq!(aborted_budget.rest_cost_in_window(), 0);

        let (submitted_market, submitted_budget) =
            settle_fresh(BoltV3RestingCommitDisposition::Submitted, true);
        assert_eq!(
            submitted_market.leg_state(Leg::Yes),
            LegState::SubmitPending
        );
        assert_eq!(submitted_budget.outstanding_submit_cost(), 0);
        assert_eq!(submitted_budget.submit_commands_in_window(), 1);
        assert_eq!(submitted_budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn sink_rejected_submit_aborts_lifecycle_but_never_refunds_the_charge() {
        let (market, budget) = settle_fresh(BoltV3RestingCommitDisposition::SinkRejected, true);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
        assert!(budget.propose_fresh_submit(NOW_MS).is_err());
    }

    #[test]
    fn drop_guard_restores_before_sink_and_poisons_with_charge_after_sink() {
        let before_market = MarketQuote::new(false);
        let before_budget = budget_pair(8, 8);
        let mut before = MakerQuoteTransactionParticipant::new(fresh_context(
            &before_market,
            &before_budget,
            Leg::Yes,
        ));
        before.arm_at_generation(1).unwrap();
        drop(before);
        assert_eq!(before_market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(before_budget.rest_cost_in_window(), 0);

        let after_market = MarketQuote::new(false);
        let after_budget = budget_pair(8, 8);
        let mut after = MakerQuoteTransactionParticipant::new(fresh_context(
            &after_market,
            &after_budget,
            Leg::Yes,
        ));
        after.arm_at_generation(2).unwrap();
        after
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        drop(after);
        assert_eq!(
            after_market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert_eq!(after_budget.submit_commands_in_window(), 1);
        assert_eq!(after_budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn sink_callback_then_unwind_preserves_callback_retirement_and_charge() {
        let market = MarketQuote::new(false);
        let budget = budget_pair(8, 8);
        let context = fresh_context(&market, &budget, Leg::Yes);
        let proposal = context.proposal.lifecycle;
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_generation(7).unwrap();
        participant
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        assert!(market.retire_leg_transaction_from_callback(proposal, 7));
        drop(participant);

        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn two_leg_settlement_is_independent_for_pre_sink_and_sink_rejection() {
        for rejected_leg in [Leg::Yes, Leg::No] {
            let submitted_leg = match rejected_leg {
                Leg::Yes => Leg::No,
                Leg::No => Leg::Yes,
            };
            let market = MarketQuote::new(false);
            let budget = budget_pair(8, 8);
            let mut submitted = MakerQuoteTransactionParticipant::new(fresh_context(
                &market,
                &budget,
                submitted_leg,
            ));
            let mut rejected = MakerQuoteTransactionParticipant::new(fresh_context(
                &market,
                &budget,
                rejected_leg,
            ));
            submitted.arm_at_generation(1).unwrap();
            rejected.arm_at_generation(2).unwrap();
            submitted
                .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
                .expect("participant should reach the sink");
            submitted
                .settle_at_generation(1, BoltV3RestingCommitDisposition::Submitted)
                .unwrap();
            rejected
                .settle_at_generation(2, BoltV3RestingCommitDisposition::PreSinkAborted)
                .unwrap();
            assert_eq!(market.leg_state(submitted_leg), LegState::SubmitPending);
            assert_eq!(market.leg_state(rejected_leg), LegState::Idle);
            assert_eq!(budget.submit_commands_in_window(), 1);
            assert_eq!(budget.rest_cost_in_window(), 1);

            let market = MarketQuote::new(false);
            let budget = budget_pair(8, 8);
            let mut submitted = MakerQuoteTransactionParticipant::new(fresh_context(
                &market,
                &budget,
                submitted_leg,
            ));
            let mut rejected = MakerQuoteTransactionParticipant::new(fresh_context(
                &market,
                &budget,
                rejected_leg,
            ));
            submitted.arm_at_generation(3).unwrap();
            rejected.arm_at_generation(4).unwrap();
            submitted
                .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
                .expect("participant should reach the sink");
            rejected
                .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
                .expect("participant should reach the sink");
            submitted
                .settle_at_generation(3, BoltV3RestingCommitDisposition::Submitted)
                .unwrap();
            rejected
                .settle_at_generation(4, BoltV3RestingCommitDisposition::SinkRejected)
                .unwrap();
            assert_eq!(market.leg_state(submitted_leg), LegState::SubmitPending);
            assert_eq!(market.leg_state(rejected_leg), LegState::Idle);
            assert_eq!(budget.submit_commands_in_window(), 2);
            assert_eq!(budget.rest_cost_in_window(), 2);
        }
    }

    fn issued_cancel_with_prepaid(
        submit_cap: u64,
        rest_cap: u64,
    ) -> (MarketQuote, RequoteBudgetPair) {
        let market = MarketQuote::new(false);
        let budget = budget_pair(submit_cap, rest_cap);
        let context = resting_context(&market, &budget);
        let lifecycle = context.proposal.lifecycle;
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_generation(1).unwrap();
        participant
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        participant
            .settle_at_generation(
                1,
                BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid,
            )
            .unwrap();
        assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        MakerQuoteLifecycleHandle::new(market.clone(), lifecycle.leg()).terminal_callback();
        (market, budget)
    }

    fn assert_only_emitted_cancel_remains_charged(budget: &RequoteBudgetPair) {
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn cancel_one_side_after_requote_confirmation_charges_cancel_and_releases_replacement() {
        let (mut market, budget) = issued_cancel_with_prepaid(8, 8);

        assert_eq!(market.cancel_one_side(Leg::Yes), None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_only_emitted_cancel_remains_charged(&budget);
    }

    #[test]
    fn drain_after_requote_confirmation_charges_cancel_and_releases_replacement() {
        let (mut market, budget) = issued_cancel_with_prepaid(8, 8);

        assert_eq!(market.drain(), None);
        assert_eq!(
            market.market_state(),
            crate::bolt_v3_quote_lifecycle::MarketState::Idle
        );
        assert_only_emitted_cancel_remains_charged(&budget);
    }

    #[test]
    fn both_leg_wind_down_after_confirmation_charges_both_emitted_cancels_only() {
        let market = MarketQuote::new(false);
        let budget = budget_pair(8, 8);
        let mut participants = Vec::new();
        for (offset, leg) in [Leg::Yes, Leg::No].into_iter().enumerate() {
            let mut market_handle = market.clone();
            market_handle.on_leg_event(
                leg,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            );
            market_handle.on_leg_event(leg, LegEvent::Accepted);
            let mut budget_handle = budget.clone();
            let decision = drive_quote_leg(
                &mut market_handle,
                &mut budget_handle,
                QuoteControlInput {
                    leg,
                    desired_price: 0.6,
                    resting_price: Some(0.5),
                    requote_threshold: 0.01,
                    eps: 1e-9,
                    now_ms: NOW_MS,
                },
            );
            let context = MakerQuoteTransactionContext {
                market: market.clone(),
                budget: budget.clone(),
                proposal: decision.proposal.expect("requote cancel must be proposed"),
            };
            let generation = u64::try_from(offset + 1).expect("test generation fits u64");
            let mut participant = MakerQuoteTransactionParticipant::new(context);
            participant.arm_at_generation(generation).unwrap();
            participant
                .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
                .expect("participant should reach the sink");
            participant
                .settle_at_generation(
                    generation,
                    BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid,
                )
                .unwrap();
            MakerQuoteLifecycleHandle::new(market.clone(), leg).terminal_callback();
            participants.push(participant);
        }
        drop(participants);

        let mut market_handle = market.clone();
        assert_eq!(market_handle.drain(), None);
        assert_eq!(
            market_handle.market_state(),
            crate::bolt_v3_quote_lifecycle::MarketState::Idle
        );
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 2);
    }

    #[test]
    fn cancel_failure_before_issuance_restores_resting_state_and_releases_prepaid_capacity() {
        let market = MarketQuote::new(false);
        let budget = budget_pair(1, 2);
        let mut participant =
            MakerQuoteTransactionParticipant::new(resting_context(&market, &budget));
        participant.arm_at_generation(1).unwrap();
        participant
            .settle_at_generation(1, BoltV3RestingCommitDisposition::PreSinkAborted)
            .unwrap();

        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert!(budget.propose_cancel_resubmit(NOW_MS).is_ok());
    }

    fn modify_context(
        market: &MarketQuote,
        budget: &RequoteBudgetPair,
    ) -> MakerQuoteTransactionContext {
        let mut market_handle = market.clone();
        market_handle.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market_handle.on_leg_event(Leg::Yes, LegEvent::Accepted);
        let mut budget_handle = budget.clone();
        let proposal = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: Some(0.5),
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS,
            },
        )
        .proposal
        .expect("modify must be proposed");
        MakerQuoteTransactionContext {
            market: market.clone(),
            budget: budget.clone(),
            proposal,
        }
    }

    #[test]
    fn modify_rolls_back_before_issuance_and_commits_pending_state_at_issuance() {
        let aborted_market = MarketQuote::new(true);
        let aborted_budget = budget_pair(1, 1);
        let mut aborted =
            MakerQuoteTransactionParticipant::new(modify_context(&aborted_market, &aborted_budget));
        aborted.arm_at_generation(1).unwrap();
        aborted
            .settle_at_generation(1, BoltV3RestingCommitDisposition::PreSinkAborted)
            .unwrap();
        assert_eq!(aborted_market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(aborted_budget.rest_cost_in_window(), 0);

        let issued_market = MarketQuote::new(true);
        let issued_budget = budget_pair(1, 1);
        let mut issued =
            MakerQuoteTransactionParticipant::new(modify_context(&issued_market, &issued_budget));
        issued.arm_at_generation(2).unwrap();
        issued
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        issued
            .settle_at_generation(2, BoltV3RestingCommitDisposition::CommandIssued)
            .unwrap();
        assert_eq!(issued_market.leg_state(Leg::Yes), LegState::ModifyPending);
        assert_eq!(issued_budget.submit_commands_in_window(), 0);
        assert_eq!(issued_budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn delayed_fresh_submit_charges_the_actor_sink_timestamp() {
        let market = MarketQuote::new(false);
        let budget = short_window_budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));
        participant.arm_at_generation(1).unwrap();
        participant
            .mark_sink_invoked(1_900 * NANOS_PER_MILLI_U64)
            .unwrap();
        participant
            .settle_at_generation(1, BoltV3RestingCommitDisposition::Submitted)
            .unwrap();

        assert!(budget.propose_fresh_submit(2_001).is_err());
    }

    #[test]
    fn delayed_modify_charges_the_actor_sink_timestamp() {
        let market = MarketQuote::new(true);
        let budget = short_window_budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(modify_context(&market, &budget));
        participant.arm_at_generation(1).unwrap();
        participant
            .mark_sink_invoked(1_900 * NANOS_PER_MILLI_U64)
            .unwrap();
        participant
            .settle_at_generation(1, BoltV3RestingCommitDisposition::CommandIssued)
            .unwrap();

        assert!(budget.propose_rest(2_001).is_err());
    }

    fn replacement_context(
        market: &MarketQuote,
        budget: &RequoteBudgetPair,
    ) -> MakerQuoteTransactionContext {
        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS + 1,
            },
        );
        MakerQuoteTransactionContext {
            market: market.clone(),
            budget: budget.clone(),
            proposal: decision.proposal.expect("replacement must be proposed"),
        }
    }

    #[test]
    fn cancel_issuance_retains_one_prepaid_token_and_pre_sink_replacement_reuses_it() {
        let (market, budget) = issued_cancel_with_prepaid(8, 8);
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);

        let context = replacement_context(&market, &budget);
        assert!(matches!(
            context.proposal.budget,
            MakerQuoteBudgetProposal::Prepaid { .. }
        ));
        let mut replacement = MakerQuoteTransactionParticipant::new(context);
        replacement.arm_at_generation(2).unwrap();
        replacement
            .settle_at_generation(2, BoltV3RestingCommitDisposition::PreSinkAborted)
            .unwrap();
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn synchronous_cancel_terminal_callback_wins_without_poisoning_the_leg() {
        let market = MarketQuote::new(false);
        let budget = budget_pair(8, 8);
        let context = resting_context(&market, &budget);
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_generation(1).unwrap();
        participant
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes).terminal_callback();
        participant
            .settle_at_generation(
                1,
                BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid,
            )
            .unwrap();

        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn sink_rejected_replacement_consumes_prepaid_and_next_retry_reserves_fresh() {
        let (market, budget) = issued_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_generation(2).unwrap();
        replacement
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        replacement
            .settle_at_generation(2, BoltV3RestingCommitDisposition::SinkRejected)
            .unwrap();
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 2);
        assert!(matches!(
            replacement_context(&market, &budget).proposal.budget,
            MakerQuoteBudgetProposal::Reserve(_)
        ));
    }

    #[test]
    fn repeated_sink_rejected_replacements_take_fresh_tokens_until_the_cap_blocks_routing() {
        let (market, budget) = issued_cancel_with_prepaid(2, 3);
        let mut first =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        first.arm_at_generation(2).unwrap();
        first
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        first
            .settle_at_generation(2, BoltV3RestingCommitDisposition::SinkRejected)
            .unwrap();

        let second_context = replacement_context(&market, &budget);
        assert!(matches!(
            second_context.proposal.budget,
            MakerQuoteBudgetProposal::Reserve(_)
        ));
        let mut second = MakerQuoteTransactionParticipant::new(second_context);
        second.arm_at_generation(3).unwrap();
        second
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .expect("participant should reach the sink");
        second
            .settle_at_generation(3, BoltV3RestingCommitDisposition::SinkRejected)
            .unwrap();
        assert_eq!(budget.submit_commands_in_window(), 2);
        assert_eq!(budget.rest_cost_in_window(), 3);

        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let blocked = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS + 2,
            },
        );
        assert_eq!(blocked.action, None);
        assert_eq!(
            blocked.blocked_by,
            Some(
                crate::bolt_v3_maker_quote_control::QuoteControlBlockReason::RequoteBudgetExhausted
            )
        );
        assert_eq!(budget.outstanding_submit_cost(), 0);
    }

    #[test]
    fn rollback_invariant_failure_poison_holds_the_prepaid_token_without_retry() {
        let (market, budget) = issued_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_generation(2).unwrap();
        replacement
            .settle_at_generation(2, BoltV3RestingCommitDisposition::RollbackInvariantFailed)
            .unwrap();
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        assert_eq!(budget.outstanding_submit_cost(), 1);
        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS + 2,
            },
        );
        assert_eq!(decision.action, None);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn post_sink_rollback_cannot_reuse_an_exhausted_prepaid_token() {
        let (market, budget) = issued_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_generation(2).unwrap();
        replacement
            .mark_sink_invoked((NOW_MS + 1) * NANOS_PER_MILLI_U64)
            .expect("replacement should reach the sink");
        replacement
            .settle_at_generation(2, BoltV3RestingCommitDisposition::RollbackInvariantFailed)
            .unwrap();

        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 2);

        let mut market_handle = market.clone();
        let mut budget_handle = budget.clone();
        let decision = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS + 2,
            },
        );
        assert_eq!(decision.action, None);
        assert_eq!(decision.proposal, None);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 2);
    }

    #[test]
    fn delayed_prepaid_replacement_charges_the_actor_sink_timestamp() {
        let market = MarketQuote::new(false);
        let budget = short_window_budget_pair(1, 2);
        let context = resting_context(&market, &budget);
        let lifecycle = context.proposal.lifecycle;
        let mut cancel = MakerQuoteTransactionParticipant::new(context);
        cancel.arm_at_generation(1).unwrap();
        cancel
            .mark_sink_invoked(NOW_MS * NANOS_PER_MILLI_U64)
            .unwrap();
        cancel
            .settle_at_generation(
                1,
                BoltV3RestingCommitDisposition::CommandIssuedRetainPrepaid,
            )
            .unwrap();
        MakerQuoteLifecycleHandle::new(market.clone(), lifecycle.leg()).terminal_callback();

        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_generation(2).unwrap();
        replacement
            .mark_sink_invoked(1_900 * NANOS_PER_MILLI_U64)
            .unwrap();
        replacement
            .settle_at_generation(2, BoltV3RestingCommitDisposition::Submitted)
            .unwrap();

        assert!(budget.propose_fresh_submit(2_001).is_err());
    }
}
