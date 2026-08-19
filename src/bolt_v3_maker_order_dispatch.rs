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
    bolt_v3_maker_quote_control::MakerQuoteCommandProposal,
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
    bolt_v3_order_execution::{
        BoltV3PreparedRestingRegistrationCommit, BoltV3RestingRegistrationCapability,
        BoltV3RestingRegistrationCommitParticipant, BoltV3RestingSubmitTransactionOutcome,
        RestingOrderCancelHandled,
    },
    bolt_v3_order_intent::build_nt_order,
    bolt_v3_quote_lifecycle::{
        Leg, LifecycleAction, MakerQuoteLifecycleHandle, MakerQuoteLifecycleIdentity, MarketAction,
        MarketQuote, PreparedMakerQuoteSinkInvocation, QuoteLegTransitionProposal,
        QuoteTransactionRegistrationPhase,
    },
    bolt_v3_requote_budget::RequoteBudgetPair,
};

#[derive(Debug, Clone, PartialEq)]
struct MakerQuoteLegAuthority {
    market: MarketQuote,
    budget: RequoteBudgetPair,
    proposal: MakerQuoteCommandProposal,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerQuoteTransactionContext {
    authority: MakerQuoteLegAuthority,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MakerOrderCommandAuthority {
    Quote(MakerQuoteTransactionContext),
    ScopeCancelAll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerOrderDispatchInput<'a> {
    pub command: &'a MakerCompiledOrderCommand,
    pub submit_order_prefix: &'a str,
    pub authority: MakerOrderCommandAuthority,
}

#[derive(Debug)]
struct MakerQuoteTransactionParticipant {
    market: MarketQuote,
    budget: RequoteBudgetPair,
    proposal: MakerQuoteCommandProposal,
}

impl MakerQuoteTransactionParticipant {
    fn new(context: MakerQuoteTransactionContext) -> Self {
        let MakerQuoteLegAuthority {
            market,
            budget,
            proposal,
            instrument_id: _,
        } = context.authority;
        Self {
            market,
            budget,
            proposal,
        }
    }

    const fn lifecycle(&self) -> QuoteLegTransitionProposal {
        self.proposal.lifecycle()
    }

    fn require_settled(settled: bool) -> Result<()> {
        anyhow::ensure!(settled, "maker quote transaction settlement was rejected");
        Ok(())
    }

    #[cfg(test)]
    fn commit_sink_invoked(&mut self, generation: u64, actor_now_ns: u64) {
        self.preflight_sink_invocation(generation, actor_now_ns)
            .expect("test participant should prepare the sink boundary")
            .commit();
    }
}

impl BoltV3PreparedRestingRegistrationCommit for PreparedMakerQuoteSinkInvocation<'_> {
    fn commit(&mut self) {
        PreparedMakerQuoteSinkInvocation::commit(self);
    }
}

#[cfg(test)]
pub(crate) fn maker_quote_transaction_participant_for_test(
    context: MakerQuoteTransactionContext,
) -> Box<dyn BoltV3RestingRegistrationCommitParticipant> {
    Box::new(MakerQuoteTransactionParticipant::new(context))
}

impl BoltV3RestingRegistrationCommitParticipant for MakerQuoteTransactionParticipant {
    fn requote_budget(&self) -> Option<RequoteBudgetPair> {
        Some(self.budget.clone())
    }

    fn maker_lifecycle(&self) -> MakerQuoteLifecycleHandle {
        MakerQuoteLifecycleHandle::new(self.market.clone(), self.lifecycle().leg())
    }

    fn arm_at_identity(&mut self, identity: MakerQuoteLifecycleIdentity) -> Result<()> {
        self.market.arm_leg_transaction(
            self.lifecycle(),
            self.budget.clone(),
            self.proposal.budget(),
            identity,
        )
    }

    fn preflight_sink_invocation(
        &mut self,
        generation: u64,
        actor_now_ns: u64,
    ) -> Result<Box<dyn BoltV3PreparedRestingRegistrationCommit + '_>> {
        self.market
            .prepare_leg_transaction_sink_invoked(
                self.lifecycle(),
                generation,
                actor_now_ns / NANOS_PER_MILLI_U64,
            )
            .map(|prepared| Box::new(prepared) as Box<dyn BoltV3PreparedRestingRegistrationCommit>)
    }

    fn registration_capability(&self, generation: u64) -> BoltV3RestingRegistrationCapability {
        match self
            .market
            .leg_transaction_registration_phase(self.lifecycle(), generation)
        {
            QuoteTransactionRegistrationPhase::PreSink => {
                BoltV3RestingRegistrationCapability::PreSink
            }
            QuoteTransactionRegistrationPhase::SinkInvoked => {
                BoltV3RestingRegistrationCapability::SinkInvoked
            }
            QuoteTransactionRegistrationPhase::Settled => {
                BoltV3RestingRegistrationCapability::Settled
            }
        }
    }

    fn settle_submitted(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .commit_leg_transaction(self.lifecycle(), generation),
        )
    }

    fn settle_nt_mutation_invoked(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .commit_leg_transaction(self.lifecycle(), generation),
        )
    }

    fn settle_sink_rejected(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .reject_leg_transaction_at_sink(self.lifecycle(), generation),
        )
    }

    fn settle_callback_retired(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .retire_leg_transaction_from_callback(self.lifecycle(), generation),
        )
    }

    fn abort_pre_sink(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .abort_leg_transaction(self.lifecycle(), generation),
        )
    }

    fn fail_pre_sink_invariant(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .fail_pre_sink_leg_transaction(self.lifecycle(), generation),
        )
    }

    fn fail_post_sink_invariant(&mut self, generation: u64) -> Result<()> {
        Self::require_settled(
            self.market
                .unwind_post_sink_leg_transaction(self.lifecycle(), generation),
        )
    }
}

impl Drop for MakerQuoteTransactionParticipant {
    fn drop(&mut self) {
        if let Err(error) = self.market.unwind_leg_transaction(self.lifecycle()) {
            log::error!(
                "maker quote transaction unwind failed: leg={:?} error={error:#}",
                self.lifecycle().leg(),
            );
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
    CancelIntentHandled {
        leg: Leg,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        disposition: RestingOrderCancelHandled,
    },
    CancelScopeHandled {
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
        dispositions: Vec<RestingOrderCancelHandled>,
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
    LifecycleScope,
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

impl MakerQuoteTransactionContext {
    #[must_use]
    pub fn new(
        market: MarketQuote,
        budget: RequoteBudgetPair,
        proposal: MakerQuoteCommandProposal,
    ) -> Self {
        let instrument_id = market
            .scope_identity()
            .instrument_id(proposal.lifecycle().leg());
        Self {
            authority: MakerQuoteLegAuthority {
                market,
                budget,
                proposal,
                instrument_id,
            },
        }
    }

    pub(crate) const fn market(&self) -> &MarketQuote {
        &self.authority.market
    }

    #[cfg(test)]
    const fn proposal(&self) -> MakerQuoteCommandProposal {
        self.authority.proposal
    }

    fn bind_to(
        &self,
        action: MarketAction,
        instrument_id: InstrumentId,
    ) -> std::result::Result<(), MakerOrderCommandFailure> {
        let sealed = (
            self.authority.proposal.action(),
            self.authority.instrument_id,
        );
        let command = (action, instrument_id);
        if sealed != command {
            return Err(MakerOrderCommandFailure::new(
                MakerOrderCommandFailureKind::LifecycleScope,
                format_args!(
                    "maker quote transaction does not match the compiled command: sealed={sealed:?} command={command:?}",
                ),
            ));
        }
        Ok(())
    }

    fn into_participant(self) -> MakerQuoteTransactionParticipant {
        MakerQuoteTransactionParticipant::new(self)
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
    ) -> Result<RestingOrderCancelHandled>;

    fn cancel_all_maker_orders(
        &mut self,
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<Vec<RestingOrderCancelHandled>>;

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
    let MakerOrderDispatchInput {
        command,
        submit_order_prefix,
        authority,
    } = input;
    match (command, authority) {
        (
            MakerCompiledOrderCommand::Submit {
                leg,
                template,
                inputs,
                fallback_price,
            },
            MakerOrderCommandAuthority::Quote(quote_transaction),
        ) => {
            let action = MarketAction::Leg {
                leg: *leg,
                action: LifecycleAction::Submit,
            };
            quote_transaction.bind_to(action, inputs.instrument_id)?;
            let order = {
                // `order_factory()` now yields a `RefMut` guard (NT moved the strategy
                // `OrderFactory` behind `Rc<RefCell<_>>`). Scope it so the borrow of `sink`
                // is released before the `submit_maker_order` call below.
                let mut order_factory = sink.order_factory();
                build_nt_order(&mut order_factory, submit_order_prefix, template, *inputs).map_err(
                    |error| {
                        MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Build, error)
                    },
                )?
            };
            let prepared_client_order_id = order.client_order_id();
            let instrument_id = order.instrument_id();
            quote_transaction.bind_to(action, instrument_id)?;
            let price = order.price().unwrap_or(*fallback_price);
            let quantity = order.quantity();
            let prepared = sink.prepare_maker_order(order).map_err(|error| {
                MakerOrderCommandFailure::new(
                    MakerOrderCommandFailureKind::SubmitPreparation,
                    error,
                )
            })?;
            let transaction =
                sink.submit_maker_order(prepared, Box::new(quote_transaction.into_participant()));
            Ok(MakerOrderDispatchOutcome::SubmitAttempt {
                leg: *leg,
                instrument_id,
                prepared_client_order_id,
                price,
                quantity,
                transaction,
            })
        }
        (
            MakerCompiledOrderCommand::Cancel {
                leg,
                instrument_id,
                client_order_id,
            },
            MakerOrderCommandAuthority::Quote(quote_transaction),
        ) => {
            quote_transaction.bind_to(
                MarketAction::Leg {
                    leg: *leg,
                    action: LifecycleAction::Cancel,
                },
                *instrument_id,
            )?;
            let disposition = sink
                .cancel_maker_order(
                    *leg,
                    *instrument_id,
                    *client_order_id,
                    Box::new(quote_transaction.into_participant()),
                )
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::Cancel, error)
                })?;
            Ok(MakerOrderDispatchOutcome::CancelIntentHandled {
                leg: *leg,
                instrument_id: *instrument_id,
                client_order_id: *client_order_id,
                disposition,
            })
        }
        (
            MakerCompiledOrderCommand::CancelAll {
                leg,
                instrument_id,
                order_side,
            },
            MakerOrderCommandAuthority::ScopeCancelAll,
        ) => {
            let dispositions = sink
                .cancel_all_maker_orders(*leg, *instrument_id, *order_side)
                .map_err(|error| {
                    MakerOrderCommandFailure::new(MakerOrderCommandFailureKind::CancelAll, error)
                })?;
            Ok(MakerOrderDispatchOutcome::CancelScopeHandled {
                leg: *leg,
                instrument_id: *instrument_id,
                order_side: *order_side,
                dispositions,
            })
        }
        (
            MakerCompiledOrderCommand::Modify {
                leg,
                instrument_id,
                client_order_id,
                price,
                quantity,
            },
            MakerOrderCommandAuthority::Quote(quote_transaction),
        ) => {
            quote_transaction.bind_to(
                MarketAction::Leg {
                    leg: *leg,
                    action: LifecycleAction::Modify,
                },
                *instrument_id,
            )?;
            sink.modify_maker_order(
                *leg,
                *instrument_id,
                *client_order_id,
                *price,
                *quantity,
                Box::new(quote_transaction.into_participant()),
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
        (
            MakerCompiledOrderCommand::Submit { .. }
            | MakerCompiledOrderCommand::Cancel { .. }
            | MakerCompiledOrderCommand::Modify { .. },
            MakerOrderCommandAuthority::ScopeCancelAll,
        ) => Err(MakerOrderCommandFailure::new(
            MakerOrderCommandFailureKind::Lifecycle,
            "quote-bearing maker command requires quote lifecycle authority",
        )),
        (MakerCompiledOrderCommand::CancelAll { .. }, MakerOrderCommandAuthority::Quote(_)) => {
            Err(MakerOrderCommandFailure::new(
                MakerOrderCommandFailureKind::Lifecycle,
                "maker cancel-all requires scope cancellation authority",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_maker_quote_control::{QuoteControlInput, drive_quote_leg},
        bolt_v3_quote_lifecycle::{
            LegEvent, LegState, MakerQuoteBudgetProposal, MakerQuoteLifecycleRefinement,
            MakerQuoteLifecycleRefinementEvent, MakerQuoteTerminalDisposition,
        },
        bolt_v3_requote_budget::RequoteBudget,
    };

    const NOW_MS: u64 = 1_000;
    const WINDOW_MS: u64 = 60_000;

    fn lifecycle_identity(generation: u64) -> MakerQuoteLifecycleIdentity {
        MakerQuoteLifecycleIdentity::new("TEST-MAKER-ORDER", generation)
    }

    fn terminal_event(
        generation: u64,
        disposition: MakerQuoteTerminalDisposition,
    ) -> MakerQuoteLifecycleRefinementEvent {
        MakerQuoteLifecycleRefinementEvent::new(
            lifecycle_identity(generation),
            MakerQuoteLifecycleRefinement::Terminal {
                stable_effect: Some(disposition),
                closes_reopened: false,
            },
        )
    }

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
        MakerQuoteTransactionContext::new(
            market.clone(),
            budget.clone(),
            decision.proposal.expect("fresh quote must be proposed"),
        )
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
        MakerQuoteTransactionContext::new(
            market.clone(),
            budget.clone(),
            decision.proposal.expect("requote cancel must be proposed"),
        )
    }

    fn settle_fresh(
        mark_sink: bool,
        settle: impl FnOnce(&mut MakerQuoteTransactionParticipant) -> Result<()>,
    ) -> (MarketQuote, RequoteBudgetPair) {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        if mark_sink {
            participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        }
        settle(&mut participant).unwrap();
        (market, budget)
    }

    #[test]
    fn submitted_outcome_is_rejected_before_sink_without_mutation() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));

        participant
            .settle_submitted(1)
            .expect_err("a submitted outcome cannot settle a pre-sink participant");

        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(budget.outstanding_submit_cost(), 0);
    }

    #[test]
    fn settled_outcome_is_idempotent_but_a_different_outcome_is_rejected() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        participant
            .settle_submitted(1)
            .expect("the matching settled outcome must be idempotent");

        participant
            .settle_sink_rejected(1)
            .expect_err("a different settled outcome must be rejected");
        assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
    }

    #[test]
    fn pre_sink_abort_restores_lifecycle_and_budget_while_submit_commits_both() {
        let (aborted_market, aborted_budget) =
            settle_fresh(false, |participant| participant.abort_pre_sink(1));
        assert_eq!(aborted_market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(aborted_budget.outstanding_submit_cost(), 0);
        assert_eq!(aborted_budget.submit_commands_in_window(), 0);
        assert_eq!(aborted_budget.rest_cost_in_window(), 0);

        let (submitted_market, submitted_budget) =
            settle_fresh(true, |participant| participant.settle_submitted(1));
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
        let (market, budget) =
            settle_fresh(true, |participant| participant.settle_sink_rejected(1));
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
        assert!(budget.propose_fresh_submit(NOW_MS).is_err());
    }

    #[test]
    fn drop_guard_restores_before_sink_and_poisons_with_charge_after_sink() {
        let before_market = MarketQuote::new_for_test(false);
        let before_budget = budget_pair(8, 8);
        let mut before = MakerQuoteTransactionParticipant::new(fresh_context(
            &before_market,
            &before_budget,
            Leg::Yes,
        ));
        before.arm_at_identity(lifecycle_identity(1)).unwrap();
        drop(before);
        assert_eq!(before_market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(before_budget.rest_cost_in_window(), 0);

        let after_market = MarketQuote::new_for_test(false);
        let after_budget = budget_pair(8, 8);
        let mut after = MakerQuoteTransactionParticipant::new(fresh_context(
            &after_market,
            &after_budget,
            Leg::Yes,
        ));
        after.arm_at_identity(lifecycle_identity(2)).unwrap();
        after.commit_sink_invoked(2, NOW_MS * NANOS_PER_MILLI_U64);
        drop(after);
        assert_eq!(
            after_market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert_eq!(after_budget.submit_commands_in_window(), 1);
        assert_eq!(after_budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn post_sink_cancel_unwind_retains_prepaid_replacement_capacity() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(1, 2);
        let mut participant =
            MakerQuoteTransactionParticipant::new(resting_context(&market, &budget));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);

        drop(participant);

        assert!(market.prepaid_generation(Leg::Yes).is_some());
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn poisoned_requote_cancel_recovers_to_the_prepaid_replacement_obligation() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(1, 2);
        let mut participant =
            MakerQuoteTransactionParticipant::new(resting_context(&market, &budget));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);

        drop(participant);

        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("post-sink unwind must retain replacement capacity");
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );

        let _ = MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes)
            .refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));

        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(
            market.prepaid_generation(Leg::Yes),
            Some(prepaid_generation)
        );
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
    }

    #[test]
    fn drop_after_external_settlement_is_idempotent() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let context = fresh_context(&market, &budget, Leg::Yes);
        let lifecycle = context.proposal().lifecycle();
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(11)).unwrap();
        assert!(market.abort_leg_transaction(lifecycle, 11));

        drop(participant);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(budget.outstanding_submit_cost(), 0);
    }

    #[test]
    fn sink_callback_then_unwind_preserves_callback_retirement_and_charge() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let context = fresh_context(&market, &budget, Leg::Yes);
        let proposal = context.proposal().lifecycle();
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(7)).unwrap();
        participant.commit_sink_invoked(7, NOW_MS * NANOS_PER_MILLI_U64);
        let _ = MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes)
            .refine(terminal_event(7, MakerQuoteTerminalDisposition::Rejected));
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
            let market = MarketQuote::new_for_test(false);
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
            submitted.arm_at_identity(lifecycle_identity(1)).unwrap();
            rejected.arm_at_identity(lifecycle_identity(2)).unwrap();
            submitted.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
            submitted.settle_submitted(1).unwrap();
            rejected.abort_pre_sink(2).unwrap();
            assert_eq!(market.leg_state(submitted_leg), LegState::SubmitPending);
            assert_eq!(market.leg_state(rejected_leg), LegState::Idle);
            assert_eq!(budget.submit_commands_in_window(), 1);
            assert_eq!(budget.rest_cost_in_window(), 1);

            let market = MarketQuote::new_for_test(false);
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
            submitted.arm_at_identity(lifecycle_identity(3)).unwrap();
            rejected.arm_at_identity(lifecycle_identity(4)).unwrap();
            submitted.commit_sink_invoked(3, NOW_MS * NANOS_PER_MILLI_U64);
            rejected.commit_sink_invoked(4, NOW_MS * NANOS_PER_MILLI_U64);
            submitted.settle_submitted(3).unwrap();
            rejected.settle_sink_rejected(4).unwrap();
            assert_eq!(market.leg_state(submitted_leg), LegState::SubmitPending);
            assert_eq!(market.leg_state(rejected_leg), LegState::Idle);
            assert_eq!(budget.submit_commands_in_window(), 2);
            assert_eq!(budget.rest_cost_in_window(), 2);
        }
    }

    fn invoked_cancel_with_prepaid(
        submit_cap: u64,
        rest_cap: u64,
    ) -> (MarketQuote, RequoteBudgetPair) {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(submit_cap, rest_cap);
        let context = resting_context(&market, &budget);
        let lifecycle = context.proposal().lifecycle();
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        participant.settle_nt_mutation_invoked(1).unwrap();
        assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
        assert!(market.prepaid_generation(Leg::Yes).is_some());
        let _ = MakerQuoteLifecycleHandle::new(market.clone(), lifecycle.leg())
            .refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));
        (market, budget)
    }

    fn invoked_cancel_before_terminal() -> (
        MarketQuote,
        RequoteBudgetPair,
        MakerQuoteLifecycleHandle,
        MakerQuoteTransactionParticipant,
    ) {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let context = resting_context(&market, &budget);
        let lifecycle =
            MakerQuoteLifecycleHandle::new(market.clone(), context.proposal().lifecycle().leg());
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        participant.settle_nt_mutation_invoked(1).unwrap();
        (market, budget, lifecycle, participant)
    }

    #[test]
    fn typed_terminal_disposition_distinguishes_cancel_from_fill() {
        let (canceled_market, _canceled_budget, canceled_lifecycle, canceled_participant) =
            invoked_cancel_before_terminal();
        let _ =
            canceled_lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));
        assert_eq!(
            canceled_market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert!(canceled_market.prepaid_generation(Leg::Yes).is_some());
        drop(canceled_participant);

        let (filled_market, _filled_budget, filled_lifecycle, filled_participant) =
            invoked_cancel_before_terminal();
        let _ = filled_lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Filled));
        assert_eq!(filled_market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(filled_market.prepaid_generation(Leg::Yes), None);
        drop(filled_participant);
    }

    #[test]
    fn canceled_terminal_refines_to_filled_and_retires_prepaid_replacement() {
        let (market, budget, lifecycle, participant) = invoked_cancel_before_terminal();

        let _ = lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert!(market.prepaid_generation(Leg::Yes).is_some());

        let _ = lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Filled));

        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
        drop(participant);
    }

    #[test]
    fn filled_terminal_releases_the_owned_prepaid_capacity() {
        let (market, budget, lifecycle, participant) = invoked_cancel_before_terminal();
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);

        let _ = lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Filled));

        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
        drop(participant);
    }

    #[test]
    fn poisoned_wind_down_emits_scoped_cancel_without_clearing_authority() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let context = resting_context(&market, &budget);
        let lifecycle =
            MakerQuoteLifecycleHandle::new(market.clone(), context.proposal().lifecycle().leg());
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        participant.fail_post_sink_invariant(1).unwrap();
        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("poisoned requote cancel must retain prepaid capacity");

        let mut wind_down = market.clone();
        assert_eq!(
            wind_down.cancel_one_side(Leg::Yes),
            Some(MarketAction::CancelAllOneSide { leg: Leg::Yes })
        );
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::PoisonedReconciliationHold
        );
        assert_eq!(
            market.prepaid_generation(Leg::Yes),
            Some(prepaid_generation)
        );

        let _ = lifecycle.refine(terminal_event(1, MakerQuoteTerminalDisposition::Filled));
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        drop(participant);
    }

    fn assert_only_emitted_cancel_remains_charged(budget: &RequoteBudgetPair) {
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn cancel_one_side_after_requote_confirmation_charges_cancel_and_releases_replacement() {
        let (mut market, budget) = invoked_cancel_with_prepaid(8, 8);

        assert_eq!(market.cancel_one_side(Leg::Yes), None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_only_emitted_cancel_remains_charged(&budget);
    }

    #[test]
    fn drain_after_requote_confirmation_charges_cancel_and_releases_replacement() {
        let (mut market, budget) = invoked_cancel_with_prepaid(8, 8);

        assert_eq!(market.drain(), None);
        assert_eq!(
            market.market_state(),
            crate::bolt_v3_quote_lifecycle::MarketState::Idle
        );
        assert_only_emitted_cancel_remains_charged(&budget);
    }

    #[test]
    fn both_leg_wind_down_after_confirmation_charges_both_emitted_cancels_only() {
        let market = MarketQuote::new_for_test(false);
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
            let context = MakerQuoteTransactionContext::new(
                market.clone(),
                budget.clone(),
                decision.proposal.expect("requote cancel must be proposed"),
            );
            let generation = u64::try_from(offset + 1).expect("test generation fits u64");
            let mut participant = MakerQuoteTransactionParticipant::new(context);
            participant
                .arm_at_identity(lifecycle_identity(generation))
                .unwrap();
            participant.commit_sink_invoked(generation, NOW_MS * NANOS_PER_MILLI_U64);
            participant.settle_nt_mutation_invoked(generation).unwrap();
            let _ = MakerQuoteLifecycleHandle::new(market.clone(), leg).refine(terminal_event(
                generation,
                MakerQuoteTerminalDisposition::Canceled,
            ));
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
    fn cancel_failure_before_invocation_restores_resting_state_and_releases_prepaid_capacity() {
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(1, 2);
        let mut participant =
            MakerQuoteTransactionParticipant::new(resting_context(&market, &budget));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.abort_pre_sink(1).unwrap();

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
        MakerQuoteTransactionContext::new(market.clone(), budget.clone(), proposal)
    }

    #[test]
    fn modify_rolls_back_before_invocation_and_commits_pending_state_at_invocation() {
        let aborted_market = MarketQuote::new_for_test(true);
        let aborted_budget = budget_pair(1, 1);
        let mut aborted =
            MakerQuoteTransactionParticipant::new(modify_context(&aborted_market, &aborted_budget));
        aborted.arm_at_identity(lifecycle_identity(1)).unwrap();
        aborted.abort_pre_sink(1).unwrap();
        assert_eq!(aborted_market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(aborted_budget.rest_cost_in_window(), 0);

        let invoked_market = MarketQuote::new_for_test(true);
        let invoked_budget = budget_pair(1, 1);
        let mut invoked =
            MakerQuoteTransactionParticipant::new(modify_context(&invoked_market, &invoked_budget));
        invoked.arm_at_identity(lifecycle_identity(2)).unwrap();
        invoked.commit_sink_invoked(2, NOW_MS * NANOS_PER_MILLI_U64);
        invoked.settle_nt_mutation_invoked(2).unwrap();
        assert_eq!(invoked_market.leg_state(Leg::Yes), LegState::ModifyPending);
        assert_eq!(invoked_budget.submit_commands_in_window(), 0);
        assert_eq!(invoked_budget.rest_cost_in_window(), 1);
    }

    #[test]
    fn delayed_fresh_submit_charges_the_actor_sink_timestamp() {
        let market = MarketQuote::new_for_test(false);
        let budget = short_window_budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(fresh_context(&market, &budget, Leg::Yes));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, 1_900 * NANOS_PER_MILLI_U64);
        participant.settle_submitted(1).unwrap();

        assert!(budget.propose_fresh_submit(2_001).is_err());
    }

    #[test]
    fn delayed_modify_charges_the_actor_sink_timestamp() {
        let market = MarketQuote::new_for_test(true);
        let budget = short_window_budget_pair(1, 1);
        let mut participant =
            MakerQuoteTransactionParticipant::new(modify_context(&market, &budget));
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, 1_900 * NANOS_PER_MILLI_U64);
        participant.settle_nt_mutation_invoked(1).unwrap();

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
        MakerQuoteTransactionContext::new(
            market.clone(),
            budget.clone(),
            decision.proposal.expect("replacement must be proposed"),
        )
    }

    #[test]
    fn cancel_invocation_retains_one_prepaid_token_and_pre_sink_replacement_reuses_it() {
        let (market, budget) = invoked_cancel_with_prepaid(8, 8);
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(budget.outstanding_submit_cost(), 1);
        assert_eq!(budget.outstanding_rest_cost(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);

        let context = replacement_context(&market, &budget);
        assert!(matches!(
            context.proposal().budget(),
            MakerQuoteBudgetProposal::Prepaid { .. }
        ));
        let mut replacement = MakerQuoteTransactionParticipant::new(context);
        replacement.arm_at_identity(lifecycle_identity(2)).unwrap();
        replacement.abort_pre_sink(2).unwrap();
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
        let market = MarketQuote::new_for_test(false);
        let budget = budget_pair(8, 8);
        let context = resting_context(&market, &budget);
        let mut participant = MakerQuoteTransactionParticipant::new(context);
        participant.arm_at_identity(lifecycle_identity(1)).unwrap();
        participant.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        let _ = MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes)
            .refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));
        participant.settle_nt_mutation_invoked(1).unwrap();

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
        let (market, budget) = invoked_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_identity(lifecycle_identity(2)).unwrap();
        replacement.commit_sink_invoked(2, NOW_MS * NANOS_PER_MILLI_U64);
        replacement.settle_sink_rejected(2).unwrap();
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(market.prepaid_generation(Leg::Yes), None);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 2);
        assert!(matches!(
            replacement_context(&market, &budget).proposal().budget(),
            MakerQuoteBudgetProposal::Reserve(_)
        ));
    }

    #[test]
    fn repeated_sink_rejected_replacements_take_fresh_tokens_until_the_cap_blocks_routing() {
        let (market, budget) = invoked_cancel_with_prepaid(2, 3);
        let mut first =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        first.arm_at_identity(lifecycle_identity(2)).unwrap();
        first.commit_sink_invoked(2, NOW_MS * NANOS_PER_MILLI_U64);
        first.settle_sink_rejected(2).unwrap();

        let second_context = replacement_context(&market, &budget);
        assert!(matches!(
            second_context.proposal().budget(),
            MakerQuoteBudgetProposal::Reserve(_)
        ));
        let mut second = MakerQuoteTransactionParticipant::new(second_context);
        second.arm_at_identity(lifecycle_identity(3)).unwrap();
        second.commit_sink_invoked(3, NOW_MS * NANOS_PER_MILLI_U64);
        second.settle_sink_rejected(3).unwrap();
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
    fn authoritative_terminal_recovers_poisoned_replacement_with_its_prepaid_token() {
        let (market, budget) = invoked_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_identity(lifecycle_identity(2)).unwrap();
        replacement.fail_pre_sink_invariant(2).unwrap();
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

        let prepaid_generation = market
            .prepaid_generation(Leg::Yes)
            .expect("poisoned replacement must retain its prepaid generation");
        let _ = MakerQuoteLifecycleHandle::new(market.clone(), Leg::Yes)
            .refine(terminal_event(2, MakerQuoteTerminalDisposition::Canceled));
        assert_eq!(
            market.leg_state(Leg::Yes),
            LegState::ReplacementPendingBackoff
        );
        assert_eq!(
            market.prepaid_generation(Leg::Yes),
            Some(prepaid_generation)
        );

        let recovered = drive_quote_leg(
            &mut market_handle,
            &mut budget_handle,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.6,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW_MS + 3,
            },
        );
        assert_eq!(
            recovered.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Submit,
            })
        );
        assert!(matches!(
            recovered.proposal.map(MakerQuoteCommandProposal::budget),
            Some(MakerQuoteBudgetProposal::Prepaid {
                generation,
                ..
            }) if generation == prepaid_generation
        ));
        assert_eq!(budget.outstanding_submit_cost(), 1);
    }

    #[test]
    fn post_sink_rollback_cannot_reuse_an_exhausted_prepaid_token() {
        let (market, budget) = invoked_cancel_with_prepaid(8, 8);
        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_identity(lifecycle_identity(2)).unwrap();
        replacement.commit_sink_invoked(2, (NOW_MS + 1) * NANOS_PER_MILLI_U64);
        replacement.fail_post_sink_invariant(2).unwrap();

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
        let market = MarketQuote::new_for_test(false);
        let budget = short_window_budget_pair(1, 2);
        let context = resting_context(&market, &budget);
        let lifecycle = context.proposal().lifecycle();
        let mut cancel = MakerQuoteTransactionParticipant::new(context);
        cancel.arm_at_identity(lifecycle_identity(1)).unwrap();
        cancel.commit_sink_invoked(1, NOW_MS * NANOS_PER_MILLI_U64);
        cancel.settle_nt_mutation_invoked(1).unwrap();
        let _ = MakerQuoteLifecycleHandle::new(market.clone(), lifecycle.leg())
            .refine(terminal_event(1, MakerQuoteTerminalDisposition::Canceled));

        let mut replacement =
            MakerQuoteTransactionParticipant::new(replacement_context(&market, &budget));
        replacement.arm_at_identity(lifecycle_identity(2)).unwrap();
        replacement.commit_sink_invoked(2, 1_900 * NANOS_PER_MILLI_U64);
        replacement.settle_submitted(2).unwrap();

        assert!(budget.propose_fresh_submit(2_001).is_err());
    }
}
