//! Pure, NautilusTrader-free single-leg quote-lifecycle state machine for the
//! binary-oracle maker (W2 — Quote Lifecycle / Execution Control).
//!
//! It models one resting-quote leg as a [`LegState`] plus a transition function
//! ([`QuoteLeg::on_event`]) that consumes [`LegEvent`]s and emits
//! [`LifecycleAction`]s the strategy layer translates into NT order calls.
//!
//! The machine deliberately holds no NautilusTrader type: NT remains the single
//! owner of order submission/cancellation/fills (NT-FIRST, NO DUAL PATHS), and
//! this module only names the *intent*. Keeping it pure makes the lifecycle
//! exhaustively unit-testable without a runtime, and lets the same machine be
//! reused unchanged behind the execution-adapter seam when a second venue
//! arrives. It is venue-agnostic by construction: the requote path is selected
//! by the `supports_modify` capability fact (a `bool` sourced from the venue
//! contract), never by a venue name.
//!
//! Scope (W2 slices 1–3): both requote paths per leg — cancel+resubmit
//! (venues without order-modify support) and modify-in-place (modify-capable
//! venues, with a modify-reject degrade) — plus the two-leg
//! (YES/NO) market controller with explicit cancel scope (single-leg, both-leg
//! drain, one-side). The requote throttle, reconnect resync, and the NT handler
//! translation arrive in later W2 slices.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::bolt_v3_requote_budget::RequoteBudgetReservation;

/// Lifecycle state of a single quote leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegState {
    /// No resting order and nothing in flight.
    Idle,
    /// A submit has been emitted; awaiting the venue's accept/reject.
    SubmitPending,
    /// A live resting quote is on the book.
    Resting,
    /// A requote cancel has been emitted; awaiting the cancel confirmation
    /// before the replacement submit (cancel+resubmit venues).
    RequotePending,
    /// An in-place modify has been emitted; awaiting the modify confirmation
    /// (modify-capable venues). A modify reject degrades to cancel+resubmit.
    ModifyPending,
    /// An unconditional (governor / wind-down) cancel has been emitted; the leg
    /// will NOT resubmit when it confirms — distinct from `RequotePending`.
    CancelPending,
    /// The requote cancel is confirmed and no order remains at the venue. The
    /// next timed quote drive must submit a replacement; failures return here
    /// instead of restoring the pre-cancel resting state.
    ReplacementPendingBackoff,
    /// A sink-reaching attempt unwound with an unknown outcome. The leg cannot
    /// route again until governed reconciliation proves the venue disposition.
    PoisonedReconciliationHold,
}

/// Events that drive a leg.
///
/// The caller derives `requote_needed` by comparing the freshly-computed quote
/// price against the resting price using the configured requote threshold; the
/// pure machine never computes prices itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegEvent {
    /// A pricing tick. `requote_needed` is true when the desired quote has moved
    /// beyond the requote threshold from the currently resting one.
    QuoteTrigger { requote_needed: bool },
    /// The venue accepted the in-flight submit.
    Accepted,
    /// The venue rejected the in-flight submit.
    Rejected,
    /// The venue confirmed the in-flight cancel.
    Canceled,
    /// The venue confirmed the in-flight in-place modify.
    Modified,
    /// The venue rejected the in-flight in-place modify.
    ModifyRejected,
    /// The venue rejected the in-flight cancel (order already gone, a duplicate
    /// cancel, or a transient venue error). The leg cannot assume the order is
    /// gone, so it keeps hunting rather than wait forever for a `Canceled` that
    /// may never arrive.
    CancelRejected,
    /// The resting (or in-flight) order **fully** filled and left the book.
    ///
    /// A *partial* fill — a remainder still rests — is deliberately NOT a
    /// lifecycle event: it does not change whether an order is live, so it is
    /// booked into the maker's inventory/position accounting separately (this
    /// machine owns order *liveness*, not size). The NT handler raises `Filled`
    /// only when a fill leaves zero quantity working.
    Filled,
}

/// The order intent the strategy layer must execute against NautilusTrader.
///
/// The pure machine never calls NT; it only emits this intent, which the maker's
/// NT event handlers (a later slice) translate into the corresponding NT call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    /// Submit a fresh post-only limit quote for this leg.
    Submit,
    /// Cancel the resting quote (the cancel+resubmit requote path).
    Cancel,
    /// Modify the resting quote in place (modify-capable venues).
    Modify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLegTransitionProposal {
    leg: Leg,
    action: LifecycleAction,
    prior_state: LegState,
    pending_state: LegState,
}

impl QuoteLegTransitionProposal {
    #[must_use]
    pub const fn leg(self) -> Leg {
        self.leg
    }

    #[must_use]
    pub const fn action(self) -> LifecycleAction {
        self.action
    }

    #[must_use]
    pub const fn prior_state(self) -> LegState {
        self.prior_state
    }

    #[must_use]
    pub const fn pending_state(self) -> LegState {
        self.pending_state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuoteLegTransactionArm {
    generation: u64,
    prior_state: LegState,
}

/// A single quote leg, its lifecycle state, and the one venue-capability fact the
/// requote path depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLeg {
    state: LegState,
    /// Whether the venue supports in-place order modification (from the venue
    /// capability contract). When false, a requote cancels then resubmits.
    supports_modify: bool,
    transaction_arm: Option<QuoteLegTransactionArm>,
    callback_retired_generation: Option<u64>,
}

impl QuoteLeg {
    /// A fresh leg with no resting order. Constructed explicitly (no `Default`):
    /// the bolt-v3 legacy-default fence forbids a `Default` impl on the
    /// production surface, so callers must name the starting state and pass the
    /// `supports_modify` venue-capability fact.
    pub fn new(supports_modify: bool) -> Self {
        Self {
            state: LegState::Idle,
            supports_modify,
            transaction_arm: None,
            callback_retired_generation: None,
        }
    }

    /// The leg's current lifecycle state.
    pub fn state(&self) -> LegState {
        self.state
    }

    /// Drive the leg with one event, advancing its state and returning the order
    /// intent to execute (if any).
    ///
    /// Fail-closed by construction: an event that does not apply to the current
    /// state is a no-op (no action, no state change). In particular a second
    /// `QuoteTrigger` while a submit, cancel, or modify is already in flight emits
    /// nothing, so a leg can never have two commands outstanding at once.
    pub fn on_event(&mut self, event: LegEvent) -> Option<LifecycleAction> {
        match (self.state, event) {
            // T1: Idle + trigger -> submit a fresh quote.
            (LegState::Idle, LegEvent::QuoteTrigger { .. }) => {
                self.state = LegState::SubmitPending;
                Some(LifecycleAction::Submit)
            }
            // T2: in-flight submit accepted -> the quote is resting.
            (LegState::SubmitPending, LegEvent::Accepted) => {
                self.state = LegState::Resting;
                None
            }
            // T3: in-flight submit rejected -> drop the leg. The governor decides
            // whether/when to re-quote; there is no automatic resubmit here.
            (LegState::SubmitPending, LegEvent::Rejected) => {
                self.state = LegState::Idle;
                None
            }
            // T4-modify: a resting quote that has moved, on a modify-capable
            // venue, is amended in place — one Modify, no cancel/resubmit cycle.
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) if self.supports_modify => {
                self.state = LegState::ModifyPending;
                Some(LifecycleAction::Modify)
            }
            // T4-cancel: same trigger on a modify-unsupported venue cancels
            // first; the replacement submit is emitted once the cancel
            // confirms (T5).
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) => {
                self.state = LegState::RequotePending;
                Some(LifecycleAction::Cancel)
            }
            // T5: the requote cancel confirmed. The replacement is proposed by
            // the next timed quote drive so an exact pre-sink abort can retry
            // without depending on a second `Canceled` event.
            (LegState::RequotePending, LegEvent::Canceled) => {
                self.state = LegState::ReplacementPendingBackoff;
                None
            }
            (LegState::ReplacementPendingBackoff, LegEvent::QuoteTrigger { .. }) => {
                self.state = LegState::SubmitPending;
                Some(LifecycleAction::Submit)
            }
            // T5-modify: the in-place modify confirmed -> the quote rests again at
            // the new price; no resubmit needed.
            (LegState::ModifyPending, LegEvent::Modified) => {
                self.state = LegState::Resting;
                None
            }
            // T6: the modify was rejected -> degrade to cancel+resubmit (cancel
            // now, resubmit on the cancel confirmation via T5).
            (LegState::ModifyPending, LegEvent::ModifyRejected) => {
                self.state = LegState::RequotePending;
                Some(LifecycleAction::Cancel)
            }
            // T8-confirm: an unconditional (wind-down) cancel confirmed -> Idle.
            // Unlike T5 there is no resubmit.
            (LegState::CancelPending, LegEvent::Canceled) => {
                self.state = LegState::Idle;
                None
            }
            // T8-orphan-guard: a wind-down cancel is outstanding, but the venue
            // confirms it actually CREATED or still holds an order — a late
            // `Accepted` (the cancel raced ahead of the accept and no-op'd, so
            // the submit then rested), a `Modified` (the order rests at the new
            // price), or a `ModifyRejected` (the original order is still
            // resting). The first cancel hit nothing, so re-emit a Cancel
            // against the now-existing order and stay in CancelPending until it
            // confirms gone. Without this arm the order is orphaned live while
            // the leg sits in CancelPending forever (no exit event), the exact
            // stuck-state / ghost-order hazard.
            (
                LegState::CancelPending,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            ) => Some(LifecycleAction::Cancel),
            // T8-reject: the in-flight submit that the wind-down cancel chased
            // was rejected — nothing was ever created at the venue, so there is
            // nothing to cancel and the leg is done.
            (LegState::CancelPending, LegEvent::Rejected) => {
                self.state = LegState::Idle;
                None
            }
            // The shared tracked-order cancellation coordinator owns retry timing.
            // Preserve the lifecycle state, but never create a second retry route.
            (LegState::RequotePending | LegState::CancelPending, LegEvent::CancelRejected) => None,
            // Fill: a full fill removes the order from the book entirely, from
            // any state that holds or is working an order, so the leg returns to
            // Idle — there is no resting quote left to cancel or modify. This is
            // the class fix for the ghost-order hazard: without it a filled
            // resting quote would stay Resting and the next requote would cancel
            // an order that already filled and is gone. An in-flight cancel or
            // modify chasing this order is answered by the venue with a
            // reject/cancel that is a harmless no-op once the leg is Idle. There
            // is no automatic resubmit — the governor decides whether to requote.
            // (Idle + Filled is a stale/duplicate fill and falls through below.)
            (
                LegState::SubmitPending
                | LegState::Resting
                | LegState::RequotePending
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::ReplacementPendingBackoff,
                LegEvent::Filled,
            ) => {
                self.state = LegState::Idle;
                None
            }
            // Idle-orphan-guard: the leg believes it holds no order, yet the venue
            // reports one live — a late `Accepted` (a submit we treated as dropped
            // actually rested), a `Modified`, or a `ModifyRejected` (the original
            // still rests). Hunt it down rather than leave it resting untracked.
            // Stay Idle: the resulting `Canceled` is a no-op here, and if the event
            // was merely stale the Cancel hits nothing. Mirrors the CancelPending
            // orphan guard. (`Filled` in Idle is a stale/duplicate fill and stays a
            // no-op below.)
            (
                LegState::Idle,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            ) => Some(LifecycleAction::Cancel),
            (
                LegState::SubmitPending | LegState::Resting | LegState::ModifyPending,
                LegEvent::Canceled,
            ) => {
                self.state = LegState::Idle;
                None
            }
            // Everything else is a no-op: a no-move trigger while Resting, any
            // trigger while a command is in flight, or an event that does not
            // match the current state.
            _ => None,
        }
    }

    fn propose(&self, leg: Leg, event: LegEvent) -> Option<QuoteLegTransitionProposal> {
        let mut candidate = *self;
        let action = candidate.on_event(event)?;
        Some(QuoteLegTransitionProposal {
            leg,
            action,
            prior_state: self.state,
            pending_state: candidate.state,
        })
    }

    fn arm(&mut self, proposal: QuoteLegTransitionProposal, generation: u64) -> anyhow::Result<()> {
        anyhow::ensure!(
            generation > 0,
            "quote lifecycle transaction generation must be positive"
        );
        anyhow::ensure!(
            self.transaction_arm.is_none(),
            "quote lifecycle already has an armed transaction"
        );
        anyhow::ensure!(
            self.callback_retired_generation.is_none(),
            "quote lifecycle has an unsettled callback disposition"
        );
        anyhow::ensure!(
            self.state == proposal.prior_state,
            "quote lifecycle proposal is stale"
        );
        self.transaction_arm = Some(QuoteLegTransactionArm {
            generation,
            prior_state: self.state,
        });
        self.state = proposal.pending_state;
        Ok(())
    }

    fn commit(&mut self, generation: u64) -> bool {
        if self
            .transaction_arm
            .is_some_and(|arm| arm.generation == generation)
        {
            self.transaction_arm = None;
            return true;
        }
        if self.callback_retired_generation == Some(generation) {
            self.callback_retired_generation = None;
            return true;
        }
        false
    }

    fn abort(&mut self, generation: u64) -> bool {
        if self.callback_retired_generation == Some(generation) {
            self.callback_retired_generation = None;
            return true;
        }
        let Some(arm) = self
            .transaction_arm
            .filter(|arm| arm.generation == generation)
        else {
            return false;
        };
        self.state = arm.prior_state;
        self.transaction_arm = None;
        true
    }

    fn retire_callback(&mut self, generation: u64) -> bool {
        if self.callback_retired_generation == Some(generation) {
            self.callback_retired_generation = None;
            return true;
        }
        if !self
            .transaction_arm
            .is_some_and(|arm| arm.generation == generation)
        {
            return false;
        }
        self.state = LegState::Idle;
        self.transaction_arm = None;
        true
    }

    fn poison_if_armed(&mut self, generation: u64) -> bool {
        if self.callback_retired_generation == Some(generation) {
            self.callback_retired_generation = None;
            return true;
        }
        if !self
            .transaction_arm
            .is_some_and(|arm| arm.generation == generation)
        {
            return false;
        }
        self.state = LegState::PoisonedReconciliationHold;
        self.transaction_arm = None;
        true
    }

    fn tracked_terminal_callback(&mut self) {
        if let Some(arm) = self.transaction_arm.take() {
            self.callback_retired_generation = Some(arm.generation);
        }
        self.state = match self.state {
            LegState::RequotePending => LegState::ReplacementPendingBackoff,
            LegState::PoisonedReconciliationHold => LegState::PoisonedReconciliationHold,
            _ => LegState::Idle,
        };
    }

    /// Request an unconditional cancel of this leg's working order (governor /
    /// wind-down driven), with NO resubmit when it confirms — distinct from the
    /// requote cancel (T4), which resubmits at a fresh price. A leg with no
    /// working order (Idle), or one already cancelling, is a no-op.
    pub fn request_cancel(&mut self) -> Option<LifecycleAction> {
        match self.state {
            LegState::Idle | LegState::CancelPending | LegState::PoisonedReconciliationHold => None,
            LegState::ReplacementPendingBackoff => {
                self.state = LegState::Idle;
                None
            }
            LegState::RequotePending => {
                self.state = LegState::CancelPending;
                None
            }
            // A resting or otherwise in-flight leg is cancelled and will not
            // resubmit; the wind-down supersedes any in-flight submit/requote.
            _ => {
                self.state = LegState::CancelPending;
                Some(LifecycleAction::Cancel)
            }
        }
    }
}

/// The two sides of a binary market the maker quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// The YES (e.g. up) outcome token.
    Yes,
    /// The NO (e.g. down) outcome token.
    No,
}

/// Aggregate quoting state across a market's two legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketState {
    /// Neither leg has a working or resting order.
    Idle,
    /// At least one leg is actively quoting (working toward, at, or requoting a
    /// resting order); the market is not purely winding down.
    Quoting,
    /// Every still-working leg is being cancelled with no resubmit — the market
    /// is winding down (a drain / both-leg cancel).
    Draining,
    /// At least one leg is held non-routing after a sink-unknown unwind.
    ReconciliationHold,
}

/// A market-level action the strategy layer executes against NautilusTrader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketAction {
    /// Drive one leg's single order (submit / cancel / modify that leg). Maps to
    /// a per-order NT call for that leg's client order id.
    Leg { leg: Leg, action: LifecycleAction },
    /// Cancel every tracked working order for both legs through the shared
    /// per-order cancellation coordinator.
    CancelAllBothLegs,
    /// Cancel every tracked working order on one side through the shared
    /// per-order cancellation coordinator.
    CancelAllOneSide { leg: Leg },
}

/// A market's two quote legs (YES/NO) and the cancel-scope controller over them.
///
/// Per-leg pricing/order events are routed to the matching [`QuoteLeg`]; governor
/// and wind-down decisions act at the market level with explicit cancel scope.
struct MarketQuoteState {
    yes: QuoteLeg,
    no: QuoteLeg,
    yes_prepaid: Option<RequoteBudgetReservation>,
    no_prepaid: Option<RequoteBudgetReservation>,
}

impl std::fmt::Debug for MarketQuoteState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MarketQuoteState")
            .field("yes", &self.yes)
            .field("no", &self.no)
            .field(
                "yes_prepaid_generation",
                &self
                    .yes_prepaid
                    .as_ref()
                    .map(RequoteBudgetReservation::generation),
            )
            .field(
                "no_prepaid_generation",
                &self
                    .no_prepaid
                    .as_ref()
                    .map(RequoteBudgetReservation::generation),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct MarketQuote {
    state: Arc<Mutex<MarketQuoteState>>,
}

impl std::fmt::Debug for MarketQuote {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.lock().fmt(formatter)
    }
}

impl PartialEq for MarketQuote {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.state, &other.state) {
            return true;
        }
        self.snapshot() == other.snapshot()
    }
}

impl Eq for MarketQuote {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerQuoteLifecycleHandle {
    market: MarketQuote,
    leg: Leg,
}

impl MakerQuoteLifecycleHandle {
    pub(crate) fn new(market: MarketQuote, leg: Leg) -> Self {
        Self { market, leg }
    }

    pub(crate) fn terminal_callback(&self) {
        self.market
            .with_leg_mut(self.leg, QuoteLeg::tracked_terminal_callback);
    }
}

impl MarketQuote {
    /// A fresh market with both legs idle. `supports_modify` is the venue
    /// capability fact shared by both legs.
    pub fn new(supports_modify: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(MarketQuoteState {
                yes: QuoteLeg::new(supports_modify),
                no: QuoteLeg::new(supports_modify),
                yes_prepaid: None,
                no_prepaid: None,
            })),
        }
    }

    fn with_leg_mut<T>(&self, leg: Leg, apply: impl FnOnce(&mut QuoteLeg) -> T) -> T {
        let mut state = self.lock();
        match leg {
            Leg::Yes => apply(&mut state.yes),
            Leg::No => apply(&mut state.no),
        }
    }

    /// The lifecycle state of one leg.
    pub fn leg_state(&self, leg: Leg) -> LegState {
        let state = self.lock();
        match leg {
            Leg::Yes => state.yes.state(),
            Leg::No => state.no.state(),
        }
    }

    pub fn leg_supports_modify(&self, leg: Leg) -> bool {
        let state = self.lock();
        match leg {
            Leg::Yes => state.yes.supports_modify,
            Leg::No => state.no.supports_modify,
        }
    }

    /// The aggregate market quoting state.
    pub fn market_state(&self) -> MarketState {
        let state = self.lock();
        let states = [state.yes.state(), state.no.state()];
        let any_active = states.iter().any(|state| {
            matches!(
                state,
                LegState::SubmitPending
                    | LegState::Resting
                    | LegState::RequotePending
                    | LegState::ModifyPending
                    | LegState::ReplacementPendingBackoff
            )
        });
        if states
            .iter()
            .any(|state| matches!(state, LegState::PoisonedReconciliationHold))
        {
            MarketState::ReconciliationHold
        } else if any_active {
            MarketState::Quoting
        } else if states
            .iter()
            .any(|state| matches!(state, LegState::CancelPending))
        {
            MarketState::Draining
        } else {
            MarketState::Idle
        }
    }

    /// Route a per-leg pricing/order event to one leg, wrapping the leg's intent
    /// with its leg id.
    pub fn on_leg_event(&mut self, leg: Leg, event: LegEvent) -> Option<MarketAction> {
        self.with_leg_mut(leg, |quote_leg| quote_leg.on_event(event))
            .map(|action| MarketAction::Leg { leg, action })
    }

    pub fn propose_leg_event(
        &self,
        leg: Leg,
        event: LegEvent,
    ) -> Option<QuoteLegTransitionProposal> {
        let state = self.lock();
        match leg {
            Leg::Yes => state.yes.propose(leg, event),
            Leg::No => state.no.propose(leg, event),
        }
    }

    pub(crate) fn arm_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> anyhow::Result<()> {
        self.with_leg_mut(proposal.leg, |leg| leg.arm(proposal, generation))
    }

    pub(crate) fn commit_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.with_leg_mut(proposal.leg, |leg| leg.commit(generation))
    }

    pub(crate) fn abort_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.with_leg_mut(proposal.leg, |leg| leg.abort(generation))
    }

    pub(crate) fn retire_leg_transaction_from_callback(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.with_leg_mut(proposal.leg, |leg| leg.retire_callback(generation))
    }

    pub(crate) fn poison_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.with_leg_mut(proposal.leg, |leg| leg.poison_if_armed(generation))
    }

    pub(crate) fn prepaid_generation(&self, leg: Leg) -> Option<u64> {
        let state = self.lock();
        match leg {
            Leg::Yes => state.yes_prepaid.as_ref(),
            Leg::No => state.no_prepaid.as_ref(),
        }
        .map(RequoteBudgetReservation::generation)
    }

    pub(crate) fn take_prepaid_reservation(
        &self,
        leg: Leg,
        generation: u64,
    ) -> anyhow::Result<RequoteBudgetReservation> {
        let mut state = self.lock();
        let slot = match leg {
            Leg::Yes => &mut state.yes_prepaid,
            Leg::No => &mut state.no_prepaid,
        };
        anyhow::ensure!(
            slot.as_ref().map(RequoteBudgetReservation::generation) == Some(generation),
            "replacement prepaid reservation generation is stale"
        );
        Ok(slot.take().expect("prepaid generation was present"))
    }

    pub(crate) fn retain_prepaid_reservation(
        &self,
        leg: Leg,
        reservation: RequoteBudgetReservation,
    ) -> anyhow::Result<()> {
        let mut state = self.lock();
        let slot = match leg {
            Leg::Yes => &mut state.yes_prepaid,
            Leg::No => &mut state.no_prepaid,
        };
        anyhow::ensure!(
            slot.is_none(),
            "quote leg already retains a prepaid reservation"
        );
        *slot = Some(reservation);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, MarketQuoteState> {
        self.state
            .lock()
            .expect("maker quote lifecycle state lock poisoned")
    }

    fn snapshot(&self) -> (QuoteLeg, QuoteLeg, Option<u64>, Option<u64>) {
        let state = self.lock();
        (
            state.yes,
            state.no,
            state
                .yes_prepaid
                .as_ref()
                .map(RequoteBudgetReservation::generation),
            state
                .no_prepaid
                .as_ref()
                .map(RequoteBudgetReservation::generation),
        )
    }

    /// T8 — cancel exactly one leg (e.g. a one-sided inventory/skew breach),
    /// leaving the other leg resting. Per-order `cancel_order` for that leg only.
    pub fn cancel_leg(&mut self, leg: Leg) -> Option<MarketAction> {
        let action = self.with_leg_mut(leg, QuoteLeg::request_cancel);
        self.release_prepaid_if_idle(leg);
        action.map(|action| MarketAction::Leg { leg, action })
    }

    /// T9 — drain the whole market: request coordinated per-order cancellation
    /// for every tracked working order on both legs, with no resubmit.
    pub fn drain(&mut self) -> Option<MarketAction> {
        let yes = self
            .with_leg_mut(Leg::Yes, QuoteLeg::request_cancel)
            .is_some();
        let no = self
            .with_leg_mut(Leg::No, QuoteLeg::request_cancel)
            .is_some();
        self.release_prepaid_if_idle(Leg::Yes);
        self.release_prepaid_if_idle(Leg::No);
        (yes || no).then_some(MarketAction::CancelAllBothLegs)
    }

    /// T9a — cancel one side of the market (e.g. a one-sided exposure cap),
    /// leaving the other side, through the per-order cancellation coordinator.
    pub fn cancel_one_side(&mut self, leg: Leg) -> Option<MarketAction> {
        let action = self.with_leg_mut(leg, QuoteLeg::request_cancel);
        self.release_prepaid_if_idle(leg);
        action.map(|_| MarketAction::CancelAllOneSide { leg })
    }

    fn release_prepaid_if_idle(&self, leg: Leg) {
        let mut state = self.lock();
        match leg {
            Leg::Yes if state.yes.state() == LegState::Idle => {
                state.yes_prepaid.take();
            }
            Leg::No if state.no.state() == LegState::Idle => {
                state.no_prepaid.take();
            }
            Leg::Yes | Leg::No => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a leg that already holds a resting quote, for the requote tests.
    fn resting_leg(supports_modify: bool) -> QuoteLeg {
        let mut leg = QuoteLeg::new(supports_modify);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        assert_eq!(leg.state(), LegState::Resting);
        leg
    }

    #[test]
    fn idle_trigger_submits_and_pends() {
        let mut leg = QuoteLeg::new(false);
        assert_eq!(leg.state(), LegState::Idle);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn submit_pending_accepted_rests() {
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Accepted);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn submit_pending_rejected_returns_to_idle() {
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Rejected);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn cancel_resubmit_requote_when_modify_unsupported() {
        let mut leg = resting_leg(false);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::RequotePending);
        // Confirmation enters a durable replacement state. The next timed quote
        // drive emits the replacement, so a pre-sink abort can retry without a
        // second Canceled event.
        let action = leg.on_event(LegEvent::Canceled);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::ReplacementPendingBackoff);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn cancel_rejected_retains_requote_pending_without_routing() {
        let mut leg = resting_leg(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::RequotePending);
        let action = leg.on_event(LegEvent::CancelRejected);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::RequotePending);
        // A later Canceled still creates the durable replacement state (T5).
        assert_eq!(leg.on_event(LegEvent::Canceled), None);
        assert_eq!(leg.state(), LegState::ReplacementPendingBackoff);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            Some(LifecycleAction::Submit)
        );
    }

    #[test]
    fn cancel_rejected_retains_cancel_pending_without_routing() {
        let mut leg = resting_leg(false);
        leg.request_cancel();
        assert_eq!(leg.state(), LegState::CancelPending);
        let action = leg.on_event(LegEvent::CancelRejected);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::CancelPending);
        // The eventual Canceled winds the leg down with no resubmit.
        assert_eq!(leg.on_event(LegEvent::Canceled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn cancel_rejected_without_an_outstanding_cancel_is_a_noop() {
        // Resting, no in-flight cancel: a stale CancelRejected changes nothing.
        let mut leg = resting_leg(false);
        assert_eq!(leg.on_event(LegEvent::CancelRejected), None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn external_cancel_of_active_leg_clears_to_idle() {
        let mut submitting = QuoteLeg::new(false);
        submitting.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(submitting.state(), LegState::SubmitPending);
        assert_eq!(submitting.on_event(LegEvent::Canceled), None);
        assert_eq!(submitting.state(), LegState::Idle);

        let mut resting = resting_leg(false);
        assert_eq!(resting.on_event(LegEvent::Canceled), None);
        assert_eq!(resting.state(), LegState::Idle);

        let mut modifying = resting_leg(true);
        modifying.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(modifying.state(), LegState::ModifyPending);
        assert_eq!(modifying.on_event(LegEvent::Canceled), None);
        assert_eq!(modifying.state(), LegState::Idle);
    }

    #[test]
    fn idle_orphan_guard_cancels_an_unexpected_live_order() {
        // An Idle leg that learns the venue holds a live order (late Accepted /
        // Modified / ModifyRejected) hunts it down instead of abandoning it.
        for event in [
            LegEvent::Accepted,
            LegEvent::Modified,
            LegEvent::ModifyRejected,
        ] {
            let mut leg = QuoteLeg::new(false);
            assert_eq!(leg.state(), LegState::Idle);
            assert_eq!(leg.on_event(event), Some(LifecycleAction::Cancel));
            assert_eq!(leg.state(), LegState::Idle, "stays Idle and re-hunts");
        }
        // A stale/duplicate Filled in Idle remains a no-op (not an orphan).
        let mut leg = QuoteLeg::new(false);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn modify_in_place_requote_when_modify_supported() {
        let mut leg = resting_leg(true);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        // One Modify, NOT a Cancel.
        assert_eq!(action, Some(LifecycleAction::Modify));
        assert_eq!(leg.state(), LegState::ModifyPending);
        // The modify confirmation returns the leg to Resting with no resubmit.
        let action = leg.on_event(LegEvent::Modified);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn modify_reject_degrades_to_cancel_resubmit() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        // A modify reject degrades to cancel+resubmit.
        let action = leg.on_event(LegEvent::ModifyRejected);
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::RequotePending);
        let action = leg.on_event(LegEvent::Canceled);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::ReplacementPendingBackoff);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn second_trigger_while_in_flight_emits_no_duplicate_command() {
        // Cancel+resubmit path.
        let mut leg = QuoteLeg::new(false);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: false
            }),
            Some(LifecycleAction::Submit)
        );
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::SubmitPending);
        leg.on_event(LegEvent::Accepted);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::RequotePending);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::RequotePending);
    }

    #[test]
    fn second_trigger_while_modify_in_flight_emits_no_duplicate_command() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::ModifyPending);
    }

    #[test]
    fn resting_no_move_trigger_is_a_noop() {
        for supports_modify in [false, true] {
            let mut leg = resting_leg(supports_modify);
            assert_eq!(
                leg.on_event(LegEvent::QuoteTrigger {
                    requote_needed: false
                }),
                None
            );
            assert_eq!(leg.state(), LegState::Resting);
        }
    }

    #[test]
    fn cancel_pending_late_accept_recancels_the_orphan() {
        // A wind-down cancel is requested while the submit is still in flight;
        // the cancel races ahead and no-ops, then the venue accepts the submit.
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(leg.request_cancel(), Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::CancelPending);
        // The late Accept means an order now rests: re-emit Cancel against it,
        // staying in CancelPending — NOT a silent no-op that orphans the order.
        assert_eq!(
            leg.on_event(LegEvent::Accepted),
            Some(LifecycleAction::Cancel)
        );
        assert_eq!(leg.state(), LegState::CancelPending);
        // The re-issued cancel confirms: the leg is finally clean.
        assert_eq!(leg.on_event(LegEvent::Canceled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn cancel_pending_late_modify_confirm_recancels_the_orphan() {
        // Wind-down cancel requested mid-modify; the modify confirms first, so
        // the order rests at the new price and must still be cancelled.
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        assert_eq!(leg.request_cancel(), Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::CancelPending);
        assert_eq!(
            leg.on_event(LegEvent::Modified),
            Some(LifecycleAction::Cancel)
        );
        assert_eq!(leg.state(), LegState::CancelPending);
    }

    #[test]
    fn cancel_pending_modify_reject_recancels_the_still_resting_order() {
        // Wind-down cancel requested mid-modify; the modify is rejected, so the
        // ORIGINAL order is still resting and must be cancelled.
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        leg.request_cancel();
        assert_eq!(leg.state(), LegState::CancelPending);
        assert_eq!(
            leg.on_event(LegEvent::ModifyRejected),
            Some(LifecycleAction::Cancel)
        );
        assert_eq!(leg.state(), LegState::CancelPending);
    }

    #[test]
    fn cancel_pending_rejected_submit_returns_to_idle() {
        // Wind-down cancel requested while submitting; the submit is rejected,
        // so nothing was ever created at the venue — nothing to cancel, done.
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.request_cancel();
        assert_eq!(leg.state(), LegState::CancelPending);
        assert_eq!(leg.on_event(LegEvent::Rejected), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn wind_down_requested_during_requote_does_not_emit_duplicate_cancel() {
        let mut leg = resting_leg(false);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            Some(LifecycleAction::Cancel)
        );
        assert_eq!(leg.state(), LegState::RequotePending);

        assert_eq!(leg.request_cancel(), None);
        assert_eq!(leg.state(), LegState::CancelPending);
    }

    // --- W2 slice 3: two-leg market controller + cancel scope ---

    fn resting_market(supports_modify: bool) -> MarketQuote {
        let mut market = MarketQuote::new(supports_modify);
        for leg in [Leg::Yes, Leg::No] {
            market.on_leg_event(
                leg,
                LegEvent::QuoteTrigger {
                    requote_needed: false,
                },
            );
            market.on_leg_event(leg, LegEvent::Accepted);
            assert_eq!(market.leg_state(leg), LegState::Resting);
        }
        market
    }

    #[test]
    fn fresh_market_is_idle() {
        let market = MarketQuote::new(false);
        assert_eq!(market.market_state(), MarketState::Idle);
    }

    #[test]
    fn both_legs_resting_is_quoting() {
        let market = resting_market(false);
        assert_eq!(market.market_state(), MarketState::Quoting);
    }

    #[test]
    fn on_leg_event_wraps_action_with_leg_id_and_isolates_legs() {
        let mut market = MarketQuote::new(false);
        assert_eq!(
            market.on_leg_event(
                Leg::Yes,
                LegEvent::QuoteTrigger {
                    requote_needed: false
                }
            ),
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Submit,
            })
        );
        // The other leg is untouched.
        assert_eq!(market.leg_state(Leg::No), LegState::Idle);
    }

    #[test]
    fn cancel_one_leg_leaves_the_other_resting() {
        let mut market = resting_market(false);
        let action = market.cancel_leg(Leg::Yes);
        assert_eq!(
            action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
        // The NO leg is left resting; the market still quotes one side.
        assert_eq!(market.leg_state(Leg::No), LegState::Resting);
        assert_eq!(market.market_state(), MarketState::Quoting);
        // The single-leg cancel does not resubmit on confirmation.
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Canceled), None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
    }

    #[test]
    fn drain_cancels_both_legs_and_is_draining() {
        let mut market = resting_market(false);
        assert_eq!(market.drain(), Some(MarketAction::CancelAllBothLegs));
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
        assert_eq!(market.leg_state(Leg::No), LegState::CancelPending);
        assert_eq!(market.market_state(), MarketState::Draining);
        // Neither leg resubmits; both wind down to Idle on confirmation.
        market.on_leg_event(Leg::Yes, LegEvent::Canceled);
        market.on_leg_event(Leg::No, LegEvent::Canceled);
        assert_eq!(market.market_state(), MarketState::Idle);
    }

    #[test]
    fn drain_with_no_working_orders_emits_nothing() {
        let mut market = MarketQuote::new(false);
        assert_eq!(market.drain(), None);
    }

    #[test]
    fn cancel_one_side_maps_to_scoped_cancel_all() {
        let mut market = resting_market(false);
        assert_eq!(
            market.cancel_one_side(Leg::No),
            Some(MarketAction::CancelAllOneSide { leg: Leg::No })
        );
        assert_eq!(market.leg_state(Leg::No), LegState::CancelPending);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(market.market_state(), MarketState::Quoting);
    }

    // --- W2/W3 hardening: full-fill handling (a maker's primary event) ---

    #[test]
    fn resting_full_fill_returns_to_idle() {
        let mut leg = resting_leg(false);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn submit_pending_full_fill_returns_to_idle() {
        // A submitted quote that fills before (or with) the accept is gone.
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(leg.state(), LegState::SubmitPending);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn requote_pending_full_fill_returns_to_idle() {
        // The resting order fills before the requote cancel lands: the order is
        // gone, so do NOT auto-resubmit — return to Idle and let the governor
        // decide. The chasing cancel becomes a venue-side no-op.
        let mut leg = resting_leg(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::RequotePending);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn modify_pending_full_fill_returns_to_idle() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn cancel_pending_full_fill_returns_to_idle() {
        // A wind-down cancel is outstanding when the order fills: it is gone, the
        // leg is Idle, and the chasing cancel is a no-op at the venue.
        let mut leg = resting_leg(false);
        leg.request_cancel();
        assert_eq!(leg.state(), LegState::CancelPending);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn idle_fill_is_a_noop() {
        // A stale/duplicate fill after the leg already went Idle changes nothing.
        let mut leg = QuoteLeg::new(false);
        assert_eq!(leg.on_event(LegEvent::Filled), None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn filled_leg_requotes_clean_with_no_ghost_cancel() {
        // The hazard the fill event closes: after a full fill the next pricing
        // trigger SUBMITS a fresh quote (the Idle path) instead of CANCELLING an
        // order that already filled and is gone.
        let mut leg = resting_leg(false);
        leg.on_event(LegEvent::Filled);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn market_full_fill_idles_one_leg_and_isolates_the_other() {
        let mut market = resting_market(false);
        assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Filled), None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        // The NO leg is untouched — still resting (leg isolation holds for fills).
        assert_eq!(market.leg_state(Leg::No), LegState::Resting);
    }

    #[test]
    fn requote_path_selection_follows_the_supports_modify_capability_at_market_level() {
        // DISPATCH-1 structural guard at the controller the maker drives: the SAME
        // requote trigger on a resting market yields a Modify on a modify-capable
        // venue and a Cancel on a no-modify venue, selected purely by the
        // `supports_modify` capability fact threaded into `MarketQuote::new` — never
        // by a venue name. This is the leg-level path choice that DISPATCH-1's NT
        // modify route depends on: a no-modify venue must produce a Cancel that
        // routes through the cancel path, and only a modify-capable venue produces a
        // Modify that reaches the new NT modify route. Differential by construction:
        // the two arms emit different actions from identical input, so a regression
        // that ignored `supports_modify` (always Cancel, or always Modify) fails one
        // arm.
        let trigger = LegEvent::QuoteTrigger {
            requote_needed: true,
        };

        let mut modify_capable = resting_market(true);
        assert_eq!(
            modify_capable.on_leg_event(Leg::Yes, trigger),
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Modify,
            }),
            "a modify-capable venue amends in place"
        );
        assert_eq!(modify_capable.leg_state(Leg::Yes), LegState::ModifyPending);

        let mut no_modify = resting_market(false);
        assert_eq!(
            no_modify.on_leg_event(Leg::Yes, trigger),
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            }),
            "a no-modify venue cancels then resubmits, never a Modify"
        );
        assert_eq!(no_modify.leg_state(Leg::Yes), LegState::RequotePending);
    }
}
