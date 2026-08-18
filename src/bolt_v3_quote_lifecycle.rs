//! Single-leg quote-lifecycle state machine for the binary-oracle maker
//! (W2 — Quote Lifecycle / Execution Control).
//!
//! It models each resting-quote leg as one private governed transaction whose
//! state variants own their generation, budget reservation, prepaid capacity,
//! rollback obligation, and route settlement. Per-order terminal truth lives in
//! the tracked-order retention authority rather than this per-leg occupancy
//! machine. Public
//! projections expose [`LegState`], while reducer commits emit
//! [`LifecycleAction`]s for shared execution to translate into NT order calls.
//!
//! The machine deliberately holds no NautilusTrader type: NT remains the single
//! owner of order submission/cancellation/fills (NT-FIRST, NO DUAL PATHS), and
//! this module only names the *intent*. The quote authority seals the typed NT
//! instrument IDs that define its lifecycle scope but owns no NT cache, order,
//! or routing behavior. Keeping the reducer pure makes the lifecycle exhaustively
//! unit-testable without a runtime, and lets the same machine be reused unchanged
//! behind the execution-adapter seam when a second venue arrives. It is
//! venue-agnostic by construction: the requote path is selected by the
//! `supports_modify` capability fact (a `bool` sourced from the venue contract),
//! never by a venue name.
//!
//! Scope (W2 slices 1–3): both requote paths per leg — cancel+resubmit
//! (venues without order-modify support) and modify-in-place (modify-capable
//! venues, with a modify-reject degrade) — plus the two-leg
//! (YES/NO) market controller with explicit cancel scope (single-leg, both-leg
//! drain, one-side). The requote throttle, reconnect resync, and the NT handler
//! translation arrive in later W2 slices.

use std::sync::{Arc, Mutex};

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
use std::sync::atomic::{AtomicU64, Ordering};

use nautilus_model::identifiers::InstrumentId;

use crate::bolt_v3_requote_budget::{
    RequoteBudgetPair, RequoteBudgetReservation, RequoteBudgetReservationProposal,
};

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
    /// route again until authoritative tracked-order reconciliation reports a
    /// terminal venue disposition, which applies the sealed recovery state.
    PoisonedReconciliationHold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteLegTransactionObligation {
    FreshSubmit,
    ReplacementSubmit,
    RequoteCancel,
    PlainCancel,
    Modify,
}

impl QuoteLegTransactionObligation {
    const fn from_proposal(proposal: QuoteLegTransitionProposal) -> Self {
        match proposal.action {
            LifecycleAction::Submit => match proposal.prior_state {
                LegState::ReplacementPendingBackoff => Self::ReplacementSubmit,
                LegState::Idle
                | LegState::SubmitPending
                | LegState::Resting
                | LegState::RequotePending
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::PoisonedReconciliationHold => Self::FreshSubmit,
            },
            LifecycleAction::Cancel => match proposal.pending_state {
                LegState::RequotePending => Self::RequoteCancel,
                LegState::Idle
                | LegState::SubmitPending
                | LegState::Resting
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::ReplacementPendingBackoff
                | LegState::PoisonedReconciliationHold => Self::PlainCancel,
            },
            LifecycleAction::Modify => match proposal.pending_state {
                LegState::Idle
                | LegState::SubmitPending
                | LegState::Resting
                | LegState::RequotePending
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::ReplacementPendingBackoff
                | LegState::PoisonedReconciliationHold => Self::Modify,
            },
        }
    }

    const fn route_success(self) -> QuoteRouteSuccess {
        match self {
            Self::FreshSubmit | Self::ReplacementSubmit => QuoteRouteSuccess::Submitted,
            Self::RequoteCancel | Self::PlainCancel | Self::Modify => {
                QuoteRouteSuccess::CommandIssued
            }
        }
    }
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

/// Authoritative terminal classification sourced from the tracked NT order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MakerQuoteTerminalDisposition {
    Denied,
    Rejected,
    Canceled,
    Expired,
    Filled,
    Voided,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerQuoteLifecycleIdentity {
    client_order_id: Box<str>,
    generation: u64,
}

impl MakerQuoteLifecycleIdentity {
    pub fn new(client_order_id: impl Into<Box<str>>, generation: u64) -> Self {
        Self {
            client_order_id: client_order_id.into(),
            generation,
        }
    }

    pub fn client_order_id(&self) -> &str {
        &self.client_order_id
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MakerQuoteLifecycleRefinement {
    Terminal {
        stable_effect: Option<MakerQuoteTerminalDisposition>,
        closes_reopened: bool,
    },
    Reopened,
    RetentionHorizon,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MakerQuoteLifecycleRefinementEvent {
    identity: MakerQuoteLifecycleIdentity,
    refinement: MakerQuoteLifecycleRefinement,
}

impl MakerQuoteLifecycleRefinementEvent {
    pub(crate) const fn new(
        identity: MakerQuoteLifecycleIdentity,
        refinement: MakerQuoteLifecycleRefinement,
    ) -> Self {
        Self {
            identity,
            refinement,
        }
    }
}

#[must_use = "maker quote refinement outcomes govern association retention"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MakerQuoteLifecycleRefinementOutcome {
    Applied,
    Unaffected {
        event: MakerQuoteLifecycleIdentity,
        active: Option<MakerQuoteLifecycleIdentity>,
    },
    Invalid {
        event: MakerQuoteLifecycleIdentity,
        active: Option<MakerQuoteLifecycleIdentity>,
    },
}

impl MakerQuoteTerminalDisposition {
    /// Refine a previously observed terminal truth using the transitions the
    /// pinned NT order machine permits after a terminal status. Conflicting or
    /// stale dispositions cannot rewrite the retained authoritative truth.
    pub(crate) const fn refine_terminal_with(self, authoritative: Self) -> Self {
        use MakerQuoteTerminalDisposition::{Canceled, Denied, Expired, Filled, Rejected, Voided};

        match (self, authoritative) {
            (Canceled, Filled) => Filled,
            (Filled, Voided) => Voided,
            (Denied, Denied | Rejected | Canceled | Expired | Filled | Voided) => Denied,
            (Rejected, Denied | Rejected | Canceled | Expired | Filled | Voided) => Rejected,
            (Canceled, Denied | Rejected | Canceled | Expired | Voided) => Canceled,
            (Expired, Denied | Rejected | Canceled | Expired | Filled | Voided) => Expired,
            (Filled, Denied | Rejected | Canceled | Expired | Filled) => Filled,
            (Voided, Denied | Rejected | Canceled | Expired | Filled | Voided) => Voided,
        }
    }

    pub(crate) const fn can_refine(self) -> bool {
        match self {
            Self::Canceled | Self::Filled => true,
            Self::Denied | Self::Rejected | Self::Expired | Self::Voided => false,
        }
    }
}

/// Read-only transaction capability derived from the governed state variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuoteTransactionRegistrationPhase {
    PreSink,
    SinkInvoked,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerQuoteBudgetProposal {
    Reserve(RequoteBudgetReservationProposal),
    Prepaid { generation: u64, now_ms: u64 },
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

#[derive(Debug)]
struct QuoteTransactionArm {
    identity: MakerQuoteLifecycleIdentity,
    prior_state: LegState,
    pending_state: LegState,
    obligation: QuoteLegTransactionObligation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteRouteSettlement {
    Submitted,
    CommandIssued,
    SinkRejected,
    CallbackRetired,
    PreSinkAbort,
    PreSinkInvariantFailure,
    PostSinkInvariantFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteRouteSuccess {
    Submitted,
    CommandIssued,
}

impl QuoteRouteSuccess {
    const fn settlement(self) -> QuoteRouteSettlement {
        match self {
            Self::Submitted => QuoteRouteSettlement::Submitted,
            Self::CommandIssued => QuoteRouteSettlement::CommandIssued,
        }
    }

    const fn illegal_outcome_message(self) -> &'static str {
        match self {
            Self::Submitted => "submitted outcome is not legal for this quote transaction",
            Self::CommandIssued => "command-issued outcome is not legal for this quote transaction",
        }
    }
}

#[derive(Debug)]
enum QuoteTransactionState {
    Idle,
    SubmitPending,
    Resting,
    RequotePending {
        prepaid: RequoteBudgetReservation,
    },
    ModifyPending,
    CancelPending,
    ReplacementPendingBackoff,
    ReplacementPendingBackoffPrepaid {
        prepaid: RequoteBudgetReservation,
    },
    PoisonedReconciliationHold {
        obligation: QuoteLegTransactionObligation,
    },
    PoisonedReconciliationHoldPrepaid {
        obligation: QuoteLegTransactionObligation,
        prepaid: RequoteBudgetReservation,
    },
    ArmedReserved {
        arm: QuoteTransactionArm,
        reservation: RequoteBudgetReservation,
    },
    ArmedPrepaid {
        arm: QuoteTransactionArm,
        prepaid: RequoteBudgetReservation,
    },
    SinkInvokedCharged {
        arm: QuoteTransactionArm,
    },
    SinkInvokedPrepaid {
        arm: QuoteTransactionArm,
        prepaid: RequoteBudgetReservation,
    },
    WindingDown(WindDownQuoteTransactionState),
    Settled {
        generation: u64,
        route: Option<QuoteRouteSettlement>,
        reopened: bool,
        stable: Box<QuoteTransactionState>,
    },
}

#[derive(Debug)]
enum QuoteTransactionGenerationFence {
    Current(QuoteTransactionState),
    Stale(QuoteTransactionState),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteTransactionMode {
    Active,
    WindingDown,
}

#[derive(Debug)]
enum ArmedQuoteBudget {
    Reserved(RequoteBudgetReservation),
    Prepaid(RequoteBudgetReservation),
}

#[derive(Debug)]
enum SinkInvokedQuoteBudget {
    Charged,
    Prepaid(RequoteBudgetReservation),
}

#[derive(Debug)]
enum ClassifiedSinkInvokedState {
    Invoked {
        mode: QuoteTransactionMode,
        arm: QuoteTransactionArm,
        budget: SinkInvokedQuoteBudget,
    },
    Settled(QuoteTransactionState),
    Inactive(QuoteTransactionState),
}

impl ArmedQuoteBudget {
    fn mark_sink_invoked_at(
        mut self,
        obligation: QuoteLegTransactionObligation,
        actor_now_ms: u64,
    ) -> std::result::Result<SinkInvokedQuoteBudget, (Self, anyhow::Error)> {
        let accounting = match &mut self {
            Self::Reserved(reservation) | Self::Prepaid(reservation) => {
                reservation.mark_sink_invoked_at(actor_now_ms)
            }
        };
        match accounting {
            Err(error) => Err((
                self,
                anyhow::anyhow!("maker quote budget sink accounting failed: {error:?}"),
            )),
            Ok(()) => match (self, obligation) {
                (Self::Reserved(reservation), QuoteLegTransactionObligation::RequoteCancel) => {
                    Ok(SinkInvokedQuoteBudget::Prepaid(reservation))
                }
                (
                    Self::Reserved(reservation),
                    QuoteLegTransactionObligation::FreshSubmit
                    | QuoteLegTransactionObligation::ReplacementSubmit
                    | QuoteLegTransactionObligation::PlainCancel
                    | QuoteLegTransactionObligation::Modify,
                )
                | (
                    Self::Prepaid(reservation),
                    QuoteLegTransactionObligation::FreshSubmit
                    | QuoteLegTransactionObligation::ReplacementSubmit
                    | QuoteLegTransactionObligation::RequoteCancel
                    | QuoteLegTransactionObligation::PlainCancel
                    | QuoteLegTransactionObligation::Modify,
                ) => {
                    drop(reservation);
                    Ok(SinkInvokedQuoteBudget::Charged)
                }
            },
        }
    }
}

impl QuoteTransactionMode {
    fn armed_state(
        self,
        arm: QuoteTransactionArm,
        budget: ArmedQuoteBudget,
    ) -> QuoteTransactionState {
        match (self, budget) {
            (Self::Active, ArmedQuoteBudget::Reserved(reservation)) => {
                QuoteTransactionState::ArmedReserved { arm, reservation }
            }
            (Self::Active, ArmedQuoteBudget::Prepaid(prepaid)) => {
                QuoteTransactionState::ArmedPrepaid { arm, prepaid }
            }
            (Self::WindingDown, ArmedQuoteBudget::Reserved(reservation)) => {
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                    arm,
                    reservation,
                })
            }
            (Self::WindingDown, ArmedQuoteBudget::Prepaid(prepaid)) => {
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                    arm,
                    prepaid,
                })
            }
        }
    }

    fn sink_invoked_state(
        self,
        arm: QuoteTransactionArm,
        budget: SinkInvokedQuoteBudget,
    ) -> QuoteTransactionState {
        match (self, budget) {
            (Self::Active, SinkInvokedQuoteBudget::Charged) => {
                QuoteTransactionState::SinkInvokedCharged { arm }
            }
            (Self::Active, SinkInvokedQuoteBudget::Prepaid(prepaid)) => {
                QuoteTransactionState::SinkInvokedPrepaid { arm, prepaid }
            }
            (Self::WindingDown, SinkInvokedQuoteBudget::Charged) => {
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::SinkInvokedCharged { arm },
                )
            }
            (Self::WindingDown, SinkInvokedQuoteBudget::Prepaid(prepaid)) => {
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, prepaid },
                )
            }
        }
    }
}

#[derive(Debug)]
enum WindDownQuoteTransactionState {
    Idle,
    CancelPending,
    PoisonedReconciliationHold,
    PoisonedReconciliationHoldPrepaid {
        prepaid: RequoteBudgetReservation,
    },
    ArmedReserved {
        arm: QuoteTransactionArm,
        reservation: RequoteBudgetReservation,
    },
    ArmedPrepaid {
        arm: QuoteTransactionArm,
        prepaid: RequoteBudgetReservation,
    },
    SinkInvokedCharged {
        arm: QuoteTransactionArm,
    },
    SinkInvokedPrepaid {
        arm: QuoteTransactionArm,
        prepaid: RequoteBudgetReservation,
    },
}

impl WindDownQuoteTransactionState {
    const fn leg_state(&self) -> LegState {
        match self {
            Self::Idle => LegState::Idle,
            Self::CancelPending => LegState::CancelPending,
            Self::PoisonedReconciliationHold | Self::PoisonedReconciliationHoldPrepaid { .. } => {
                LegState::PoisonedReconciliationHold
            }
            Self::ArmedReserved { arm, .. }
            | Self::ArmedPrepaid { arm, .. }
            | Self::SinkInvokedCharged { arm }
            | Self::SinkInvokedPrepaid { arm, .. } => arm.pending_state,
        }
    }

    fn prepaid_generation(&self) -> Option<u64> {
        match self {
            Self::PoisonedReconciliationHoldPrepaid { prepaid, .. }
            | Self::ArmedPrepaid { prepaid, .. }
            | Self::SinkInvokedPrepaid { prepaid, .. } => Some(prepaid.generation()),
            Self::Idle
            | Self::CancelPending
            | Self::PoisonedReconciliationHold
            | Self::ArmedReserved { .. }
            | Self::SinkInvokedCharged { .. } => None,
        }
    }

    fn retire_on_terminal(self) -> Self {
        match self {
            Self::Idle
            | Self::CancelPending
            | Self::PoisonedReconciliationHold
            | Self::SinkInvokedCharged { .. } => Self::Idle,
            Self::PoisonedReconciliationHoldPrepaid { prepaid, .. }
            | Self::ArmedPrepaid { prepaid, .. }
            | Self::SinkInvokedPrepaid { prepaid, .. } => {
                drop(prepaid);
                Self::Idle
            }
            Self::ArmedReserved { reservation, .. } => {
                drop(reservation);
                Self::Idle
            }
        }
    }
}

impl QuoteTransactionState {
    fn fence_generation(self, generation: u64) -> QuoteTransactionGenerationFence {
        match self.armed_identity() {
            Some(identity) if identity.generation() != generation => {
                QuoteTransactionGenerationFence::Stale(self)
            }
            Some(_) | None => QuoteTransactionGenerationFence::Current(self),
        }
    }

    fn classify_sink_invoked(self) -> ClassifiedSinkInvokedState {
        match self {
            Self::SinkInvokedCharged { arm } => ClassifiedSinkInvokedState::Invoked {
                mode: QuoteTransactionMode::Active,
                arm,
                budget: SinkInvokedQuoteBudget::Charged,
            },
            Self::SinkInvokedPrepaid { arm, prepaid } => ClassifiedSinkInvokedState::Invoked {
                mode: QuoteTransactionMode::Active,
                arm,
                budget: SinkInvokedQuoteBudget::Prepaid(prepaid),
            },
            Self::WindingDown(WindDownQuoteTransactionState::SinkInvokedCharged { arm }) => {
                ClassifiedSinkInvokedState::Invoked {
                    mode: QuoteTransactionMode::WindingDown,
                    arm,
                    budget: SinkInvokedQuoteBudget::Charged,
                }
            }
            Self::WindingDown(WindDownQuoteTransactionState::SinkInvokedPrepaid {
                arm,
                prepaid,
            }) => ClassifiedSinkInvokedState::Invoked {
                mode: QuoteTransactionMode::WindingDown,
                arm,
                budget: SinkInvokedQuoteBudget::Prepaid(prepaid),
            },
            state @ Self::Settled { .. } => ClassifiedSinkInvokedState::Settled(state),
            state @ (Self::Idle
            | Self::SubmitPending
            | Self::Resting
            | Self::RequotePending { .. }
            | Self::ModifyPending
            | Self::CancelPending
            | Self::ReplacementPendingBackoff
            | Self::ReplacementPendingBackoffPrepaid { .. }
            | Self::PoisonedReconciliationHold { .. }
            | Self::PoisonedReconciliationHoldPrepaid { .. }
            | Self::ArmedReserved { .. }
            | Self::ArmedPrepaid { .. }
            | Self::WindingDown(
                WindDownQuoteTransactionState::Idle
                | WindDownQuoteTransactionState::CancelPending
                | WindDownQuoteTransactionState::PoisonedReconciliationHold
                | WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
                | WindDownQuoteTransactionState::ArmedReserved { .. }
                | WindDownQuoteTransactionState::ArmedPrepaid { .. },
            )) => ClassifiedSinkInvokedState::Inactive(state),
        }
    }

    fn armed_identity(&self) -> Option<&MakerQuoteLifecycleIdentity> {
        match self {
            Self::ArmedReserved { arm, .. }
            | Self::ArmedPrepaid { arm, .. }
            | Self::SinkInvokedCharged { arm }
            | Self::SinkInvokedPrepaid { arm, .. }
            | Self::WindingDown(
                WindDownQuoteTransactionState::ArmedReserved { arm, .. }
                | WindDownQuoteTransactionState::ArmedPrepaid { arm, .. }
                | WindDownQuoteTransactionState::SinkInvokedCharged { arm }
                | WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, .. },
            ) => Some(&arm.identity),
            Self::Idle
            | Self::SubmitPending
            | Self::Resting
            | Self::RequotePending { .. }
            | Self::ModifyPending
            | Self::CancelPending
            | Self::ReplacementPendingBackoff
            | Self::ReplacementPendingBackoffPrepaid { .. }
            | Self::PoisonedReconciliationHold { .. }
            | Self::PoisonedReconciliationHoldPrepaid { .. }
            | Self::WindingDown(
                WindDownQuoteTransactionState::Idle
                | WindDownQuoteTransactionState::CancelPending
                | WindDownQuoteTransactionState::PoisonedReconciliationHold
                | WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. },
            )
            | Self::Settled { .. } => None,
        }
    }

    const fn leg_state(&self) -> LegState {
        match self {
            Self::Idle => LegState::Idle,
            Self::SubmitPending => LegState::SubmitPending,
            Self::Resting => LegState::Resting,
            Self::RequotePending { .. } => LegState::RequotePending,
            Self::ModifyPending => LegState::ModifyPending,
            Self::CancelPending => LegState::CancelPending,
            Self::ReplacementPendingBackoff | Self::ReplacementPendingBackoffPrepaid { .. } => {
                LegState::ReplacementPendingBackoff
            }
            Self::PoisonedReconciliationHold { .. }
            | Self::PoisonedReconciliationHoldPrepaid { .. } => {
                LegState::PoisonedReconciliationHold
            }
            Self::ArmedReserved { arm, .. }
            | Self::ArmedPrepaid { arm, .. }
            | Self::SinkInvokedCharged { arm }
            | Self::SinkInvokedPrepaid { arm, .. } => arm.pending_state,
            Self::WindingDown(state) => state.leg_state(),
            Self::Settled { reopened: true, .. } => LegState::CancelPending,
            Self::Settled { stable, .. } => stable.leg_state(),
        }
    }

    fn prepaid_generation(&self) -> Option<u64> {
        match self {
            Self::RequotePending { prepaid }
            | Self::ReplacementPendingBackoffPrepaid { prepaid }
            | Self::PoisonedReconciliationHoldPrepaid { prepaid, .. }
            | Self::ArmedPrepaid { prepaid, .. }
            | Self::SinkInvokedPrepaid { prepaid, .. } => Some(prepaid.generation()),
            Self::WindingDown(state) => state.prepaid_generation(),
            Self::Settled { stable, .. } => stable.prepaid_generation(),
            Self::Idle
            | Self::SubmitPending
            | Self::Resting
            | Self::ModifyPending
            | Self::CancelPending
            | Self::ReplacementPendingBackoff
            | Self::PoisonedReconciliationHold { .. }
            | Self::ArmedReserved { .. }
            | Self::SinkInvokedCharged { .. } => None,
        }
    }

    fn is_winding_down(&self) -> bool {
        match self {
            Self::WindingDown(_) => true,
            Self::Settled { stable, .. } => stable.is_winding_down(),
            Self::Idle
            | Self::SubmitPending
            | Self::Resting
            | Self::RequotePending { .. }
            | Self::ModifyPending
            | Self::CancelPending
            | Self::ReplacementPendingBackoff
            | Self::ReplacementPendingBackoffPrepaid { .. }
            | Self::PoisonedReconciliationHold { .. }
            | Self::PoisonedReconciliationHoldPrepaid { .. }
            | Self::ArmedReserved { .. }
            | Self::ArmedPrepaid { .. }
            | Self::SinkInvokedCharged { .. }
            | Self::SinkInvokedPrepaid { .. } => false,
        }
    }
}

#[derive(Debug)]
struct GovernedQuoteTransactionInner {
    state: QuoteTransactionState,
    supports_modify: bool,
    terminal_owner: Option<MakerQuoteLifecycleIdentity>,
    retention_scope_closed: bool,
}

#[derive(Debug)]
enum QuoteTransactionEvent {
    Lifecycle(LegEvent),
    WindDown,
    MissingMoneyMovingTruth,
    Arm {
        proposal: QuoteLegTransitionProposal,
        budget: RequoteBudgetPair,
        budget_proposal: MakerQuoteBudgetProposal,
        identity: MakerQuoteLifecycleIdentity,
    },
    PreSinkAbort {
        generation: u64,
    },
    PreSinkInvariantFailure {
        generation: u64,
    },
    Unwind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinkCapableQuoteTransactionEvent {
    SinkInvoked { generation: u64, actor_now_ms: u64 },
    Submitted { generation: u64 },
    CommandIssued { generation: u64 },
    SinkRejected { generation: u64 },
    CallbackRetired { generation: u64 },
    PostSinkUnwind { generation: u64 },
}

enum QuoteTransactionReductionEvent {
    PreSink(QuoteTransactionEvent),
    SinkCapable(SinkCapableQuoteTransactionEvent),
}

#[must_use = "quote transaction commits must route their lifecycle action"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GovernedQuoteTransactionCommit {
    action: Option<LifecycleAction>,
}

impl GovernedQuoteTransactionCommit {
    const fn no_action() -> Self {
        Self { action: None }
    }
}

struct QuoteTransactionReductionRequest {
    event: QuoteTransactionReductionEvent,
}

impl From<QuoteTransactionEvent> for QuoteTransactionReductionRequest {
    fn from(event: QuoteTransactionEvent) -> Self {
        Self {
            event: QuoteTransactionReductionEvent::PreSink(event),
        }
    }
}

impl From<SinkCapableQuoteTransactionEvent> for QuoteTransactionReductionRequest {
    fn from(event: SinkCapableQuoteTransactionEvent) -> Self {
        Self {
            event: QuoteTransactionReductionEvent::SinkCapable(event),
        }
    }
}

impl QuoteTransactionReductionRequest {
    fn apply(
        self,
        inner: &mut GovernedQuoteTransactionInner,
    ) -> anyhow::Result<GovernedQuoteTransactionCommit> {
        let prior = std::mem::replace(&mut inner.state, QuoteTransactionState::Idle);
        let (settled_owner, poison_only) = match &self.event {
            QuoteTransactionReductionEvent::SinkCapable(
                SinkCapableQuoteTransactionEvent::Submitted { .. }
                | SinkCapableQuoteTransactionEvent::CommandIssued { .. }
                | SinkCapableQuoteTransactionEvent::PostSinkUnwind { .. },
            ) => (prior.armed_identity().cloned(), false),
            QuoteTransactionReductionEvent::PreSink(
                QuoteTransactionEvent::PreSinkAbort { .. }
                | QuoteTransactionEvent::PreSinkInvariantFailure { .. }
                | QuoteTransactionEvent::Unwind,
            ) => (prior.armed_identity().cloned(), true),
            QuoteTransactionReductionEvent::PreSink(
                QuoteTransactionEvent::Lifecycle(_)
                | QuoteTransactionEvent::WindDown
                | QuoteTransactionEvent::MissingMoneyMovingTruth
                | QuoteTransactionEvent::Arm { .. },
            )
            | QuoteTransactionReductionEvent::SinkCapable(
                SinkCapableQuoteTransactionEvent::SinkInvoked { .. }
                | SinkCapableQuoteTransactionEvent::SinkRejected { .. }
                | SinkCapableQuoteTransactionEvent::CallbackRetired { .. },
            ) => (None, false),
        };
        let reduced = match self.event {
            QuoteTransactionReductionEvent::PreSink(event) => {
                GovernedQuoteTransactionInner::reduce_pre_sink(prior, event, inner.supports_modify)
            }
            QuoteTransactionReductionEvent::SinkCapable(event) => {
                GovernedQuoteTransactionInner::reduce_sink_capable(prior, event)
            }
        };
        let result = match reduced {
            Ok((next, commit)) => {
                inner.state = next;
                Ok(commit)
            }
            Err((restored, error)) => {
                inner.state = restored;
                Err(error)
            }
        };
        match (settled_owner, poison_only, inner.state.leg_state()) {
            (Some(owner), false, _) | (Some(owner), true, LegState::PoisonedReconciliationHold) => {
                inner.terminal_owner = Some(owner);
            }
            (None, _, _)
            | (
                Some(_),
                true,
                LegState::Idle
                | LegState::SubmitPending
                | LegState::Resting
                | LegState::RequotePending
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::ReplacementPendingBackoff,
            ) => {}
        }
        result
    }
}

type QuoteReduction = std::result::Result<
    (QuoteTransactionState, GovernedQuoteTransactionCommit),
    (QuoteTransactionState, anyhow::Error),
>;

struct MakerQuoteLifecycleRefinementRequest {
    event: MakerQuoteLifecycleRefinementEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MakerQuoteRefinementAuthority {
    Exact,
    ArmedByAnother,
    Inactive,
}

impl MakerQuoteLifecycleRefinementRequest {
    fn apply(
        self,
        inner: &mut GovernedQuoteTransactionInner,
    ) -> MakerQuoteLifecycleRefinementOutcome {
        let prior = std::mem::replace(&mut inner.state, QuoteTransactionState::Idle);
        let active = prior
            .armed_identity()
            .cloned()
            .or_else(|| inner.terminal_owner.clone());
        let identity = self.event.identity;
        let authority = match prior.armed_identity() {
            Some(armed) if armed == &identity => MakerQuoteRefinementAuthority::Exact,
            Some(_) => MakerQuoteRefinementAuthority::ArmedByAnother,
            None if inner.terminal_owner.as_ref() == Some(&identity) => {
                MakerQuoteRefinementAuthority::Exact
            }
            None => MakerQuoteRefinementAuthority::Inactive,
        };
        match (self.event.refinement, authority) {
            (
                MakerQuoteLifecycleRefinement::Terminal {
                    stable_effect,
                    closes_reopened,
                },
                MakerQuoteRefinementAuthority::Exact,
            ) => {
                let generation = identity.generation();
                let had_in_flight_route = prior.armed_identity().is_some();
                let next = match prior {
                    QuoteTransactionState::Settled {
                        generation,
                        route,
                        reopened,
                        stable,
                    } => {
                        let (stable, commit) = match stable_effect {
                            Some(disposition) => {
                                GovernedQuoteTransactionInner::reduce_terminal(*stable, disposition)
                            }
                            None => (*stable, GovernedQuoteTransactionCommit::no_action()),
                        };
                        debug_assert!(commit.action.is_none());
                        QuoteTransactionState::Settled {
                            generation,
                            route,
                            reopened: reopened && !closes_reopened,
                            stable: Box::new(stable),
                        }
                    }
                    state => {
                        let (stable, commit) = match stable_effect {
                            Some(disposition) => {
                                GovernedQuoteTransactionInner::reduce_terminal(state, disposition)
                            }
                            None => (state, GovernedQuoteTransactionCommit::no_action()),
                        };
                        debug_assert!(commit.action.is_none());
                        QuoteTransactionState::Settled {
                            generation,
                            route: (!had_in_flight_route)
                                .then_some(QuoteRouteSettlement::CallbackRetired),
                            reopened: false,
                            stable: Box::new(stable),
                        }
                    }
                };
                inner.state = next;
                inner.terminal_owner = Some(identity);
                MakerQuoteLifecycleRefinementOutcome::Applied
            }
            (
                MakerQuoteLifecycleRefinement::Reopened,
                MakerQuoteRefinementAuthority::Exact | MakerQuoteRefinementAuthority::Inactive,
            ) => match GovernedQuoteTransactionInner::reduce_reopened(prior) {
                Ok((next, commit)) => {
                    debug_assert!(commit.action.is_none());
                    inner.state = next;
                    inner.terminal_owner = Some(identity);
                    MakerQuoteLifecycleRefinementOutcome::Applied
                }
                Err((restored, _)) => {
                    inner.state = restored;
                    MakerQuoteLifecycleRefinementOutcome::Invalid {
                        event: identity,
                        active,
                    }
                }
            },
            (
                MakerQuoteLifecycleRefinement::RetentionHorizon,
                MakerQuoteRefinementAuthority::Exact,
            ) => {
                let next = match prior {
                    QuoteTransactionState::Settled {
                        generation,
                        route,
                        stable,
                        ..
                    } => {
                        let (stable, commit) =
                            GovernedQuoteTransactionInner::reduce_wind_down(*stable);
                        debug_assert!(commit.action.is_none());
                        QuoteTransactionState::Settled {
                            generation,
                            route,
                            reopened: false,
                            stable: Box::new(stable),
                        }
                    }
                    state => state,
                };
                inner.state = next;
                MakerQuoteLifecycleRefinementOutcome::Applied
            }
            (
                MakerQuoteLifecycleRefinement::Terminal { .. }
                | MakerQuoteLifecycleRefinement::RetentionHorizon,
                MakerQuoteRefinementAuthority::ArmedByAnother
                | MakerQuoteRefinementAuthority::Inactive,
            )
            | (
                MakerQuoteLifecycleRefinement::Reopened,
                MakerQuoteRefinementAuthority::ArmedByAnother,
            ) => {
                inner.state = prior;
                MakerQuoteLifecycleRefinementOutcome::Unaffected {
                    event: identity,
                    active,
                }
            }
        }
    }
}

impl GovernedQuoteTransactionInner {
    fn reduce_pre_sink(
        state: QuoteTransactionState,
        event: QuoteTransactionEvent,
        supports_modify: bool,
    ) -> QuoteReduction {
        match event {
            QuoteTransactionEvent::Lifecycle(event) => {
                Ok(Self::reduce_lifecycle(state, event, supports_modify))
            }
            QuoteTransactionEvent::WindDown => Ok(Self::reduce_wind_down(state)),
            QuoteTransactionEvent::MissingMoneyMovingTruth => {
                Ok(Self::reduce_missing_money_moving_truth(state))
            }
            QuoteTransactionEvent::Arm {
                proposal,
                budget,
                budget_proposal,
                identity,
            } => Self::reduce_arm(state, proposal, budget, budget_proposal, identity),
            QuoteTransactionEvent::PreSinkAbort { generation } => {
                Self::reduce_pre_sink_abort(state, generation)
            }
            QuoteTransactionEvent::PreSinkInvariantFailure { generation } => {
                Self::reduce_pre_sink_invariant_failure(state, generation)
            }
            QuoteTransactionEvent::Unwind => Self::reduce_unwind(state),
        }
    }

    fn reduce_sink_capable(
        state: QuoteTransactionState,
        event: SinkCapableQuoteTransactionEvent,
    ) -> QuoteReduction {
        match event {
            SinkCapableQuoteTransactionEvent::SinkInvoked {
                generation,
                actor_now_ms,
            } => Self::reduce_sink_invoked(state, generation, actor_now_ms),
            SinkCapableQuoteTransactionEvent::Submitted { generation } => {
                Self::reduce_route_success(state, generation, QuoteRouteSuccess::Submitted)
            }
            SinkCapableQuoteTransactionEvent::CommandIssued { generation } => {
                Self::reduce_route_success(state, generation, QuoteRouteSuccess::CommandIssued)
            }
            SinkCapableQuoteTransactionEvent::SinkRejected { generation } => {
                Self::reduce_sink_rejected(state, generation)
            }
            SinkCapableQuoteTransactionEvent::CallbackRetired { generation } => {
                Self::reduce_callback_retired(state, generation)
            }
            SinkCapableQuoteTransactionEvent::PostSinkUnwind { generation } => {
                Self::reduce_post_sink_failure(state, generation)
            }
        }
    }

    fn reduce_lifecycle(
        state: QuoteTransactionState,
        event: LegEvent,
        supports_modify: bool,
    ) -> (QuoteTransactionState, GovernedQuoteTransactionCommit) {
        let state = match state {
            QuoteTransactionState::Settled { stable, .. } => *stable,
            state => state,
        };
        let action = |action| GovernedQuoteTransactionCommit {
            action: Some(action),
        };
        match (state, event) {
            (
                QuoteTransactionState::WindingDown(state),
                event @ (LegEvent::QuoteTrigger { .. }
                | LegEvent::Accepted
                | LegEvent::Rejected
                | LegEvent::Canceled
                | LegEvent::Modified
                | LegEvent::ModifyRejected
                | LegEvent::CancelRejected
                | LegEvent::Filled),
            ) => Self::reduce_winding_down_lifecycle(state, event),
            (QuoteTransactionState::Idle, LegEvent::QuoteTrigger { .. }) => (
                QuoteTransactionState::SubmitPending,
                action(LifecycleAction::Submit),
            ),
            (QuoteTransactionState::SubmitPending, LegEvent::Accepted) => (
                QuoteTransactionState::Resting,
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (
                QuoteTransactionState::SubmitPending | QuoteTransactionState::CancelPending,
                LegEvent::Rejected,
            ) => (
                QuoteTransactionState::Idle,
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (
                state @ QuoteTransactionState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) => (
                state,
                action(if supports_modify {
                    LifecycleAction::Modify
                } else {
                    LifecycleAction::Cancel
                }),
            ),
            (
                state @ QuoteTransactionState::ReplacementPendingBackoff,
                LegEvent::QuoteTrigger { .. },
            )
            | (
                state @ QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. },
                LegEvent::QuoteTrigger { .. },
            ) => (state, action(LifecycleAction::Submit)),
            (QuoteTransactionState::RequotePending { prepaid }, LegEvent::Canceled) => (
                QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (QuoteTransactionState::ModifyPending, LegEvent::Modified) => (
                QuoteTransactionState::Resting,
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (QuoteTransactionState::ModifyPending, LegEvent::ModifyRejected) => (
                QuoteTransactionState::Resting,
                action(LifecycleAction::Cancel),
            ),
            (
                state @ QuoteTransactionState::CancelPending,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            )
            | (
                state @ QuoteTransactionState::Idle,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            ) => (state, action(LifecycleAction::Cancel)),
            (
                QuoteTransactionState::CancelPending
                | QuoteTransactionState::SubmitPending
                | QuoteTransactionState::Resting
                | QuoteTransactionState::ModifyPending,
                LegEvent::Canceled,
            ) => (
                QuoteTransactionState::Idle,
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (
                QuoteTransactionState::SubmitPending
                | QuoteTransactionState::Resting
                | QuoteTransactionState::ModifyPending
                | QuoteTransactionState::CancelPending
                | QuoteTransactionState::ReplacementPendingBackoff,
                LegEvent::Filled,
            ) => (
                QuoteTransactionState::Idle,
                GovernedQuoteTransactionCommit::no_action(),
            ),
            (QuoteTransactionState::RequotePending { prepaid }, LegEvent::Filled)
            | (
                QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                LegEvent::Filled,
            ) => {
                drop(prepaid);
                (
                    QuoteTransactionState::Idle,
                    GovernedQuoteTransactionCommit::no_action(),
                )
            }
            (
                state @ (QuoteTransactionState::PoisonedReconciliationHold { .. }
                | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
                | QuoteTransactionState::ArmedReserved { .. }
                | QuoteTransactionState::ArmedPrepaid { .. }
                | QuoteTransactionState::SinkInvokedCharged { .. }
                | QuoteTransactionState::SinkInvokedPrepaid { .. }
                | QuoteTransactionState::Settled { .. }),
                _,
            ) => (state, GovernedQuoteTransactionCommit::no_action()),
            (
                state @ (QuoteTransactionState::Idle
                | QuoteTransactionState::SubmitPending
                | QuoteTransactionState::Resting
                | QuoteTransactionState::RequotePending { .. }
                | QuoteTransactionState::ModifyPending
                | QuoteTransactionState::CancelPending
                | QuoteTransactionState::ReplacementPendingBackoff
                | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }),
                _,
            ) => (state, GovernedQuoteTransactionCommit::no_action()),
        }
    }

    fn reduce_winding_down_lifecycle(
        state: WindDownQuoteTransactionState,
        event: LegEvent,
    ) -> (QuoteTransactionState, GovernedQuoteTransactionCommit) {
        let no_action = GovernedQuoteTransactionCommit::no_action();
        let cancel = GovernedQuoteTransactionCommit {
            action: Some(LifecycleAction::Cancel),
        };
        match (state, event) {
            (
                WindDownQuoteTransactionState::Idle,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            ) => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                cancel,
            ),
            (
                WindDownQuoteTransactionState::Idle,
                LegEvent::QuoteTrigger { .. }
                | LegEvent::Rejected
                | LegEvent::Canceled
                | LegEvent::CancelRejected
                | LegEvent::Filled,
            ) => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                no_action,
            ),
            (
                WindDownQuoteTransactionState::CancelPending,
                LegEvent::Accepted | LegEvent::Modified | LegEvent::ModifyRejected,
            ) => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending),
                cancel,
            ),
            (
                WindDownQuoteTransactionState::CancelPending,
                LegEvent::Rejected | LegEvent::Canceled | LegEvent::Filled,
            ) => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                no_action,
            ),
            (
                WindDownQuoteTransactionState::CancelPending,
                LegEvent::QuoteTrigger { .. } | LegEvent::CancelRejected,
            ) => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending),
                no_action,
            ),
            (
                state @ (WindDownQuoteTransactionState::PoisonedReconciliationHold
                | WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                    ..
                }
                | WindDownQuoteTransactionState::ArmedReserved { .. }
                | WindDownQuoteTransactionState::ArmedPrepaid { .. }
                | WindDownQuoteTransactionState::SinkInvokedCharged { .. }
                | WindDownQuoteTransactionState::SinkInvokedPrepaid { .. }),
                LegEvent::QuoteTrigger { .. }
                | LegEvent::Accepted
                | LegEvent::Rejected
                | LegEvent::Canceled
                | LegEvent::Modified
                | LegEvent::ModifyRejected
                | LegEvent::CancelRejected
                | LegEvent::Filled,
            ) => (QuoteTransactionState::WindingDown(state), no_action),
        }
    }

    fn reduce_wind_down(
        state: QuoteTransactionState,
    ) -> (QuoteTransactionState, GovernedQuoteTransactionCommit) {
        let cancel = GovernedQuoteTransactionCommit {
            action: Some(LifecycleAction::Cancel),
        };
        match state {
            QuoteTransactionState::Idle => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                GovernedQuoteTransactionCommit::no_action(),
            ),
            QuoteTransactionState::CancelPending => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending),
                GovernedQuoteTransactionCommit::no_action(),
            ),
            QuoteTransactionState::ReplacementPendingBackoff => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                GovernedQuoteTransactionCommit::no_action(),
            ),
            QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid } => {
                drop(prepaid);
                (
                    QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                    GovernedQuoteTransactionCommit::no_action(),
                )
            }
            QuoteTransactionState::RequotePending { prepaid } => {
                drop(prepaid);
                (
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::CancelPending,
                    ),
                    GovernedQuoteTransactionCommit::no_action(),
                )
            }
            QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::ModifyPending => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending),
                cancel,
            ),
            QuoteTransactionState::PoisonedReconciliationHold { .. } => (
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::PoisonedReconciliationHold,
                ),
                cancel,
            ),
            QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                obligation: _,
                prepaid,
            } => (
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { prepaid },
                ),
                cancel,
            ),
            QuoteTransactionState::ArmedReserved { arm, reservation } => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                    arm,
                    reservation,
                }),
                cancel,
            ),
            QuoteTransactionState::ArmedPrepaid { arm, prepaid } => (
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                    arm,
                    prepaid,
                }),
                cancel,
            ),
            QuoteTransactionState::SinkInvokedCharged { arm } => (
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::SinkInvokedCharged { arm },
                ),
                cancel,
            ),
            QuoteTransactionState::SinkInvokedPrepaid { arm, prepaid } => (
                QuoteTransactionState::WindingDown(
                    WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, prepaid },
                ),
                cancel,
            ),
            state @ QuoteTransactionState::WindingDown(_) => {
                (state, GovernedQuoteTransactionCommit::no_action())
            }
            QuoteTransactionState::Settled {
                generation,
                route,
                reopened,
                stable,
            } => {
                let (stable, commit) = Self::reduce_wind_down(*stable);
                (
                    QuoteTransactionState::Settled {
                        generation,
                        route,
                        reopened,
                        stable: Box::new(stable),
                    },
                    commit,
                )
            }
        }
    }

    fn reduce_missing_money_moving_truth(
        state: QuoteTransactionState,
    ) -> (QuoteTransactionState, GovernedQuoteTransactionCommit) {
        let hold = |obligation| QuoteTransactionState::PoisonedReconciliationHold { obligation };
        let hold_prepaid =
            |obligation, prepaid| QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                obligation,
                prepaid,
            };
        let no_action = GovernedQuoteTransactionCommit::no_action();
        let next = match state {
            QuoteTransactionState::Settled {
                generation,
                route,
                stable,
                ..
            } => {
                let (stable, commit) = Self::reduce_missing_money_moving_truth(*stable);
                debug_assert!(commit.action.is_none());
                QuoteTransactionState::Settled {
                    generation,
                    route,
                    reopened: false,
                    stable: Box::new(stable),
                }
            }
            QuoteTransactionState::RequotePending { prepaid }
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { prepaid, .. } => {
                hold_prepaid(QuoteLegTransactionObligation::PlainCancel, prepaid)
            }
            QuoteTransactionState::ArmedPrepaid { prepaid, .. }
            | QuoteTransactionState::SinkInvokedPrepaid { prepaid, .. } => {
                hold_prepaid(QuoteLegTransactionObligation::PlainCancel, prepaid)
            }
            QuoteTransactionState::ArmedReserved { reservation, .. } => {
                drop(reservation);
                hold(QuoteLegTransactionObligation::PlainCancel)
            }
            QuoteTransactionState::WindingDown(state) => {
                let state = match state {
                    WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                        prepaid,
                    }
                    | WindDownQuoteTransactionState::ArmedPrepaid { prepaid, .. }
                    | WindDownQuoteTransactionState::SinkInvokedPrepaid { prepaid, .. } => {
                        WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { prepaid }
                    }
                    WindDownQuoteTransactionState::ArmedReserved { reservation, .. } => {
                        drop(reservation);
                        WindDownQuoteTransactionState::PoisonedReconciliationHold
                    }
                    WindDownQuoteTransactionState::Idle
                    | WindDownQuoteTransactionState::CancelPending
                    | WindDownQuoteTransactionState::PoisonedReconciliationHold
                    | WindDownQuoteTransactionState::SinkInvokedCharged { .. } => {
                        WindDownQuoteTransactionState::PoisonedReconciliationHold
                    }
                };
                QuoteTransactionState::WindingDown(state)
            }
            QuoteTransactionState::PoisonedReconciliationHold { obligation } => {
                QuoteTransactionState::PoisonedReconciliationHold { obligation }
            }
            QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::SinkInvokedCharged { .. } => {
                hold(QuoteLegTransactionObligation::PlainCancel)
            }
        };
        (next, no_action)
    }

    fn reduce_arm(
        state: QuoteTransactionState,
        proposal: QuoteLegTransitionProposal,
        budget: RequoteBudgetPair,
        budget_proposal: MakerQuoteBudgetProposal,
        identity: MakerQuoteLifecycleIdentity,
    ) -> QuoteReduction {
        let fail = |state, message: &'static str| (state, anyhow::anyhow!(message));
        let generation = identity.generation();
        if generation == 0 {
            return Err(fail(
                state,
                "quote lifecycle transaction generation must be positive",
            ));
        }
        let state = match state {
            QuoteTransactionState::Settled { stable, .. } => *stable,
            state => state,
        };
        if state.is_winding_down() {
            return Err(fail(state, "quote lifecycle is winding down"));
        }
        if state.leg_state() != proposal.prior_state {
            return Err(fail(state, "quote lifecycle proposal is stale"));
        }
        let arm = QuoteTransactionArm {
            identity,
            prior_state: proposal.prior_state,
            pending_state: proposal.pending_state,
            obligation: QuoteLegTransactionObligation::from_proposal(proposal),
        };
        match (state, budget_proposal) {
            (
                QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                MakerQuoteBudgetProposal::Prepaid {
                    generation: prepaid_generation,
                    ..
                },
            ) if prepaid.generation() == prepaid_generation => Ok((
                QuoteTransactionState::ArmedPrepaid { arm, prepaid },
                GovernedQuoteTransactionCommit::no_action(),
            )),
            (state, MakerQuoteBudgetProposal::Prepaid { .. }) => Err(fail(
                state,
                "replacement prepaid reservation generation is stale",
            )),
            (state, MakerQuoteBudgetProposal::Reserve(reservation_proposal)) => {
                match budget.reserve(reservation_proposal) {
                    Ok(reservation) => Ok((
                        QuoteTransactionState::ArmedReserved { arm, reservation },
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    Err(error) => Err((
                        state,
                        anyhow::anyhow!("maker quote budget reservation denied: {error:?}"),
                    )),
                }
            }
        }
    }

    fn restore_prior(arm: QuoteTransactionArm) -> QuoteTransactionState {
        match arm.prior_state {
            LegState::Idle => QuoteTransactionState::Idle,
            LegState::SubmitPending => QuoteTransactionState::SubmitPending,
            LegState::Resting => QuoteTransactionState::Resting,
            LegState::RequotePending => QuoteTransactionState::CancelPending,
            LegState::ModifyPending => QuoteTransactionState::ModifyPending,
            LegState::CancelPending => QuoteTransactionState::CancelPending,
            LegState::ReplacementPendingBackoff => QuoteTransactionState::ReplacementPendingBackoff,
            LegState::PoisonedReconciliationHold => {
                QuoteTransactionState::PoisonedReconciliationHold {
                    obligation: arm.obligation,
                }
            }
        }
    }

    fn settled(
        generation: u64,
        route: QuoteRouteSettlement,
        stable: QuoteTransactionState,
    ) -> QuoteTransactionState {
        QuoteTransactionState::Settled {
            generation,
            route: Some(route),
            reopened: false,
            stable: Box::new(stable),
        }
    }

    fn terminal_before_route_settlement(
        generation: u64,
        stable: QuoteTransactionState,
    ) -> QuoteTransactionState {
        QuoteTransactionState::Settled {
            generation,
            route: None,
            reopened: false,
            stable: Box::new(stable),
        }
    }

    fn reduce_settlement_replay(
        state: QuoteTransactionState,
        generation: u64,
        route: QuoteRouteSettlement,
    ) -> QuoteReduction {
        match state {
            QuoteTransactionState::Settled {
                generation: settled_generation,
                route: None,
                reopened,
                stable,
            } if settled_generation == generation => Ok((
                QuoteTransactionState::Settled {
                    generation,
                    route: Some(route),
                    reopened,
                    stable,
                },
                GovernedQuoteTransactionCommit::no_action(),
            )),
            state @ QuoteTransactionState::Settled {
                generation: settled_generation,
                route: Some(settled_route),
                ..
            } if settled_generation == generation && settled_route == route => {
                Ok((state, GovernedQuoteTransactionCommit::no_action()))
            }
            state @ QuoteTransactionState::Settled { .. } => Err((
                state,
                anyhow::anyhow!("conflicting or stale quote transaction settlement"),
            )),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::ArmedReserved { .. }
            | QuoteTransactionState::ArmedPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)) => {
                Err((state, anyhow::anyhow!("quote transaction is not settled")))
            }
        }
    }

    fn reduce_pre_sink_abort(state: QuoteTransactionState, generation: u64) -> QuoteReduction {
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Ok((state, GovernedQuoteTransactionCommit::no_action()));
            }
        };
        match state {
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                arm: _,
                reservation,
            }) => match reservation.abort() {
                Ok(()) => Ok((
                    Self::settled(
                        generation,
                        QuoteRouteSettlement::PreSinkAbort,
                        QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                    ),
                    GovernedQuoteTransactionCommit::no_action(),
                )),
                Err(error) => Err((
                    Self::settled(
                        generation,
                        QuoteRouteSettlement::PreSinkAbort,
                        QuoteTransactionState::WindingDown(
                            WindDownQuoteTransactionState::PoisonedReconciliationHold,
                        ),
                    ),
                    anyhow::anyhow!("maker quote budget abort failed: {error:?}"),
                )),
            },
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                arm: _,
                prepaid,
            }) => {
                drop(prepaid);
                Ok((
                    Self::settled(
                        generation,
                        QuoteRouteSettlement::PreSinkAbort,
                        QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                    ),
                    GovernedQuoteTransactionCommit::no_action(),
                ))
            }
            QuoteTransactionState::ArmedReserved { arm, reservation } => {
                match reservation.abort() {
                    Ok(()) => Ok((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            Self::restore_prior(arm),
                        ),
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    Err(error) => Err((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            QuoteTransactionState::PoisonedReconciliationHold {
                                obligation: arm.obligation,
                            },
                        ),
                        anyhow::anyhow!("maker quote budget abort failed: {error:?}"),
                    )),
                }
            }
            QuoteTransactionState::ArmedPrepaid { arm: _, prepaid } => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PreSinkAbort,
                    QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            state @ QuoteTransactionState::Settled { .. } => Self::reduce_settlement_replay(
                state,
                generation,
                QuoteRouteSettlement::PreSinkAbort,
            ),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)) => {
                Ok((state, GovernedQuoteTransactionCommit::no_action()))
            }
        }
    }

    fn reduce_pre_sink_invariant_failure(
        state: QuoteTransactionState,
        generation: u64,
    ) -> QuoteReduction {
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Ok((state, GovernedQuoteTransactionCommit::no_action()));
            }
        };
        match state {
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                arm: _,
                reservation,
            }) => match reservation.commit() {
                Ok(()) => Ok((
                    Self::settled(
                        generation,
                        QuoteRouteSettlement::PreSinkInvariantFailure,
                        QuoteTransactionState::WindingDown(
                            WindDownQuoteTransactionState::PoisonedReconciliationHold,
                        ),
                    ),
                    GovernedQuoteTransactionCommit::no_action(),
                )),
                Err(error) => Err((
                    Self::settled(
                        generation,
                        QuoteRouteSettlement::PreSinkInvariantFailure,
                        QuoteTransactionState::WindingDown(
                            WindDownQuoteTransactionState::PoisonedReconciliationHold,
                        ),
                    ),
                    anyhow::anyhow!("maker quote budget commit failed: {error:?}"),
                )),
            },
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                arm: _,
                prepaid,
            }) => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PreSinkInvariantFailure,
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                            prepaid,
                        },
                    ),
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::ArmedReserved { arm, reservation } => {
                match reservation.commit() {
                    Ok(()) => Ok((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkInvariantFailure,
                            QuoteTransactionState::PoisonedReconciliationHold {
                                obligation: arm.obligation,
                            },
                        ),
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    Err(error) => Err((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkInvariantFailure,
                            QuoteTransactionState::PoisonedReconciliationHold {
                                obligation: arm.obligation,
                            },
                        ),
                        anyhow::anyhow!("maker quote budget commit failed: {error:?}"),
                    )),
                }
            }
            QuoteTransactionState::ArmedPrepaid { arm, prepaid } => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PreSinkInvariantFailure,
                    QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                        obligation: arm.obligation,
                        prepaid,
                    },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            state @ QuoteTransactionState::Settled { .. } => Self::reduce_settlement_replay(
                state,
                generation,
                QuoteRouteSettlement::PreSinkInvariantFailure,
            ),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)) => {
                Ok((state, GovernedQuoteTransactionCommit::no_action()))
            }
        }
    }

    fn reduce_unwind(state: QuoteTransactionState) -> QuoteReduction {
        match state {
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                arm,
                reservation,
            }) => {
                let generation = arm.identity.generation();
                match reservation.abort() {
                    Ok(()) => Ok((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                        ),
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    Err(error) => Err((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            QuoteTransactionState::WindingDown(
                                WindDownQuoteTransactionState::PoisonedReconciliationHold,
                            ),
                        ),
                        anyhow::anyhow!("maker quote budget abort failed: {error:?}"),
                    )),
                }
            }
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                arm,
                prepaid,
            }) => {
                drop(prepaid);
                Ok((
                    Self::settled(
                        arm.identity.generation(),
                        QuoteRouteSettlement::PreSinkAbort,
                        QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                    ),
                    GovernedQuoteTransactionCommit::no_action(),
                ))
            }
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::SinkInvokedCharged { arm },
            ) => Ok((
                Self::settled(
                    arm.identity.generation(),
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::PoisonedReconciliationHold,
                    ),
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, prepaid },
            ) => Ok((
                Self::settled(
                    arm.identity.generation(),
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                            prepaid,
                        },
                    ),
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::ArmedReserved { arm, reservation } => {
                let generation = arm.identity.generation();
                match reservation.abort() {
                    Ok(()) => Ok((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            Self::restore_prior(arm),
                        ),
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    Err(error) => Err((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::PreSinkAbort,
                            QuoteTransactionState::PoisonedReconciliationHold {
                                obligation: arm.obligation,
                            },
                        ),
                        anyhow::anyhow!("maker quote budget abort failed: {error:?}"),
                    )),
                }
            }
            QuoteTransactionState::ArmedPrepaid { arm, prepaid } => Ok((
                Self::settled(
                    arm.identity.generation(),
                    QuoteRouteSettlement::PreSinkAbort,
                    QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::SinkInvokedCharged { arm } => Ok((
                Self::settled(
                    arm.identity.generation(),
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::PoisonedReconciliationHold {
                        obligation: arm.obligation,
                    },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::SinkInvokedPrepaid { arm, prepaid } => Ok((
                Self::settled(
                    arm.identity.generation(),
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                        obligation: arm.obligation,
                        prepaid,
                    },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::Idle
                | WindDownQuoteTransactionState::CancelPending
                | WindDownQuoteTransactionState::PoisonedReconciliationHold
                | WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. },
            )
            | QuoteTransactionState::Settled { .. }) => {
                Ok((state, GovernedQuoteTransactionCommit::no_action()))
            }
        }
    }

    fn reduce_sink_invoked(
        state: QuoteTransactionState,
        generation: u64,
        actor_now_ms: u64,
    ) -> QuoteReduction {
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Ok((state, GovernedQuoteTransactionCommit::no_action()));
            }
        };
        match state {
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedReserved {
                arm,
                reservation,
            }) => Self::account_sink_invocation(
                QuoteTransactionMode::WindingDown,
                arm,
                ArmedQuoteBudget::Reserved(reservation),
                actor_now_ms,
            ),
            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::ArmedPrepaid {
                arm,
                prepaid,
            }) => Self::account_sink_invocation(
                QuoteTransactionMode::WindingDown,
                arm,
                ArmedQuoteBudget::Prepaid(prepaid),
                actor_now_ms,
            ),
            QuoteTransactionState::ArmedReserved { arm, reservation } => {
                Self::account_sink_invocation(
                    QuoteTransactionMode::Active,
                    arm,
                    ArmedQuoteBudget::Reserved(reservation),
                    actor_now_ms,
                )
            }
            QuoteTransactionState::ArmedPrepaid { arm, prepaid } => Self::account_sink_invocation(
                QuoteTransactionMode::Active,
                arm,
                ArmedQuoteBudget::Prepaid(prepaid),
                actor_now_ms,
            ),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)
            | QuoteTransactionState::Settled { .. }) => {
                Ok((state, GovernedQuoteTransactionCommit::no_action()))
            }
        }
    }

    fn account_sink_invocation(
        mode: QuoteTransactionMode,
        arm: QuoteTransactionArm,
        budget: ArmedQuoteBudget,
        actor_now_ms: u64,
    ) -> QuoteReduction {
        match budget.mark_sink_invoked_at(arm.obligation, actor_now_ms) {
            Ok(budget) => Ok((
                mode.sink_invoked_state(arm, budget),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            Err((budget, error)) => Err((mode.armed_state(arm, budget), error)),
        }
    }

    fn committed_pending(arm: QuoteTransactionArm) -> QuoteTransactionState {
        match arm.pending_state {
            LegState::Idle => QuoteTransactionState::Idle,
            LegState::SubmitPending => QuoteTransactionState::SubmitPending,
            LegState::Resting => QuoteTransactionState::Resting,
            LegState::RequotePending => QuoteTransactionState::CancelPending,
            LegState::ModifyPending => QuoteTransactionState::ModifyPending,
            LegState::CancelPending => QuoteTransactionState::CancelPending,
            LegState::ReplacementPendingBackoff => QuoteTransactionState::ReplacementPendingBackoff,
            LegState::PoisonedReconciliationHold => {
                QuoteTransactionState::PoisonedReconciliationHold {
                    obligation: arm.obligation,
                }
            }
        }
    }

    fn reduce_route_success(
        state: QuoteTransactionState,
        generation: u64,
        success: QuoteRouteSuccess,
    ) -> QuoteReduction {
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Err((state, anyhow::anyhow!(success.illegal_outcome_message())));
            }
        };
        match state.classify_sink_invoked() {
            ClassifiedSinkInvokedState::Invoked { mode, arm, budget } => {
                match (arm.obligation.route_success(), success) {
                    (QuoteRouteSuccess::Submitted, QuoteRouteSuccess::Submitted)
                    | (QuoteRouteSuccess::CommandIssued, QuoteRouteSuccess::CommandIssued) => {
                        let stable = Self::route_success_state(mode, arm, budget);
                        Ok((
                            Self::settled(generation, success.settlement(), stable),
                            GovernedQuoteTransactionCommit::no_action(),
                        ))
                    }
                    (QuoteRouteSuccess::Submitted, QuoteRouteSuccess::CommandIssued)
                    | (QuoteRouteSuccess::CommandIssued, QuoteRouteSuccess::Submitted) => Err((
                        mode.sink_invoked_state(arm, budget),
                        anyhow::anyhow!(success.illegal_outcome_message()),
                    )),
                }
            }
            ClassifiedSinkInvokedState::Settled(state) => {
                Self::reduce_settlement_replay(state, generation, success.settlement())
            }
            ClassifiedSinkInvokedState::Inactive(state) => {
                Err((state, anyhow::anyhow!(success.illegal_outcome_message())))
            }
        }
    }

    fn route_success_state(
        mode: QuoteTransactionMode,
        arm: QuoteTransactionArm,
        budget: SinkInvokedQuoteBudget,
    ) -> QuoteTransactionState {
        match (mode, budget) {
            (QuoteTransactionMode::Active, SinkInvokedQuoteBudget::Charged) => {
                Self::committed_pending(arm)
            }
            (QuoteTransactionMode::Active, SinkInvokedQuoteBudget::Prepaid(prepaid)) => {
                QuoteTransactionState::RequotePending { prepaid }
            }
            (QuoteTransactionMode::WindingDown, SinkInvokedQuoteBudget::Charged) => {
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending)
            }
            (QuoteTransactionMode::WindingDown, SinkInvokedQuoteBudget::Prepaid(prepaid)) => {
                drop(prepaid);
                QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::CancelPending)
            }
        }
    }

    fn reduce_sink_rejected(state: QuoteTransactionState, generation: u64) -> QuoteReduction {
        const ILLEGAL_OUTCOME: &str =
            "sink-rejected outcome is not legal for this quote transaction";
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Err((state, anyhow::anyhow!(ILLEGAL_OUTCOME)));
            }
        };
        match state.classify_sink_invoked() {
            ClassifiedSinkInvokedState::Invoked { mode, arm, budget } => {
                match (mode, arm, budget) {
                    (
                        QuoteTransactionMode::WindingDown,
                        QuoteTransactionArm {
                            obligation:
                                QuoteLegTransactionObligation::FreshSubmit
                                | QuoteLegTransactionObligation::ReplacementSubmit,
                            ..
                        },
                        SinkInvokedQuoteBudget::Charged,
                    ) => Ok((
                        Self::settled(
                            generation,
                            QuoteRouteSettlement::SinkRejected,
                            QuoteTransactionState::WindingDown(WindDownQuoteTransactionState::Idle),
                        ),
                        GovernedQuoteTransactionCommit::no_action(),
                    )),
                    (
                        QuoteTransactionMode::Active,
                        arm @ QuoteTransactionArm {
                            obligation:
                                QuoteLegTransactionObligation::FreshSubmit
                                | QuoteLegTransactionObligation::ReplacementSubmit,
                            ..
                        },
                        SinkInvokedQuoteBudget::Charged,
                    ) => {
                        let stable = Self::restore_prior(arm);
                        Ok((
                            Self::settled(generation, QuoteRouteSettlement::SinkRejected, stable),
                            GovernedQuoteTransactionCommit::no_action(),
                        ))
                    }
                    (mode, arm, budget) => Err((
                        mode.sink_invoked_state(arm, budget),
                        anyhow::anyhow!(ILLEGAL_OUTCOME),
                    )),
                }
            }
            ClassifiedSinkInvokedState::Settled(state) => Self::reduce_settlement_replay(
                state,
                generation,
                QuoteRouteSettlement::SinkRejected,
            ),
            ClassifiedSinkInvokedState::Inactive(state) => {
                Err((state, anyhow::anyhow!(ILLEGAL_OUTCOME)))
            }
        }
    }

    fn reduce_callback_retired(state: QuoteTransactionState, generation: u64) -> QuoteReduction {
        match state {
            state @ QuoteTransactionState::Settled { .. } => Self::reduce_settlement_replay(
                state,
                generation,
                QuoteRouteSettlement::CallbackRetired,
            ),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::ArmedReserved { .. }
            | QuoteTransactionState::ArmedPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)) => Err((
                state,
                anyhow::anyhow!("callback retirement is not legal for this quote transaction"),
            )),
        }
    }

    fn reduce_post_sink_failure(state: QuoteTransactionState, generation: u64) -> QuoteReduction {
        let state = match state.fence_generation(generation) {
            QuoteTransactionGenerationFence::Current(state) => state,
            QuoteTransactionGenerationFence::Stale(state) => {
                return Err((
                    state,
                    anyhow::anyhow!("post-sink failure is not legal for this quote transaction"),
                ));
            }
        };
        match state {
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::SinkInvokedCharged { arm: _ },
            ) => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::PoisonedReconciliationHold,
                    ),
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::SinkInvokedPrepaid { arm: _, prepaid },
            ) => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::WindingDown(
                        WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                            prepaid,
                        },
                    ),
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::SinkInvokedCharged { arm } => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::PoisonedReconciliationHold {
                        obligation: arm.obligation,
                    },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            QuoteTransactionState::SinkInvokedPrepaid { arm, prepaid } => Ok((
                Self::settled(
                    generation,
                    QuoteRouteSettlement::PostSinkInvariantFailure,
                    QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                        obligation: arm.obligation,
                        prepaid,
                    },
                ),
                GovernedQuoteTransactionCommit::no_action(),
            )),
            state @ QuoteTransactionState::Settled { .. } => Self::reduce_settlement_replay(
                state,
                generation,
                QuoteRouteSettlement::PostSinkInvariantFailure,
            ),
            state @ (QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::ArmedReserved { .. }
            | QuoteTransactionState::ArmedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)) => Err((
                state,
                anyhow::anyhow!("post-sink failure is not legal for this quote transaction"),
            )),
        }
    }

    fn terminal_state(
        obligation: QuoteLegTransactionObligation,
        prepaid: Option<RequoteBudgetReservation>,
        disposition: MakerQuoteTerminalDisposition,
    ) -> QuoteTransactionState {
        match disposition {
            MakerQuoteTerminalDisposition::Filled => {
                drop(prepaid);
                QuoteTransactionState::Idle
            }
            MakerQuoteTerminalDisposition::Denied
            | MakerQuoteTerminalDisposition::Rejected
            | MakerQuoteTerminalDisposition::Canceled
            | MakerQuoteTerminalDisposition::Expired
            | MakerQuoteTerminalDisposition::Voided => match (obligation, prepaid) {
                (
                    QuoteLegTransactionObligation::ReplacementSubmit
                    | QuoteLegTransactionObligation::RequoteCancel,
                    Some(prepaid),
                ) => QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid },
                (
                    QuoteLegTransactionObligation::ReplacementSubmit
                    | QuoteLegTransactionObligation::RequoteCancel,
                    None,
                ) => QuoteTransactionState::ReplacementPendingBackoff,
                (
                    QuoteLegTransactionObligation::FreshSubmit
                    | QuoteLegTransactionObligation::PlainCancel
                    | QuoteLegTransactionObligation::Modify,
                    prepaid,
                ) => {
                    drop(prepaid);
                    QuoteTransactionState::Idle
                }
            },
        }
    }

    fn reduce_terminal(
        state: QuoteTransactionState,
        disposition: MakerQuoteTerminalDisposition,
    ) -> (QuoteTransactionState, GovernedQuoteTransactionCommit) {
        let no_action = GovernedQuoteTransactionCommit::no_action();
        match state {
            QuoteTransactionState::WindingDown(state) => {
                let retired = match disposition {
                    MakerQuoteTerminalDisposition::Denied
                    | MakerQuoteTerminalDisposition::Rejected
                    | MakerQuoteTerminalDisposition::Canceled
                    | MakerQuoteTerminalDisposition::Expired
                    | MakerQuoteTerminalDisposition::Filled
                    | MakerQuoteTerminalDisposition::Voided => state.retire_on_terminal(),
                };
                (QuoteTransactionState::WindingDown(retired), no_action)
            }
            QuoteTransactionState::RequotePending { prepaid } => (
                Self::terminal_state(
                    QuoteLegTransactionObligation::RequoteCancel,
                    Some(prepaid),
                    disposition,
                ),
                no_action,
            ),
            QuoteTransactionState::PoisonedReconciliationHold { obligation } => (
                Self::terminal_state(obligation, None, disposition),
                no_action,
            ),
            QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                obligation,
                prepaid,
            } => (
                Self::terminal_state(obligation, Some(prepaid), disposition),
                no_action,
            ),
            QuoteTransactionState::ArmedReserved { arm, reservation } => {
                drop(reservation);
                let generation = arm.identity.generation();
                (
                    Self::terminal_before_route_settlement(
                        generation,
                        Self::terminal_state(arm.obligation, None, disposition),
                    ),
                    no_action,
                )
            }
            QuoteTransactionState::ArmedPrepaid { arm, prepaid }
            | QuoteTransactionState::SinkInvokedPrepaid { arm, prepaid } => {
                let generation = arm.identity.generation();
                (
                    Self::terminal_before_route_settlement(
                        generation,
                        Self::terminal_state(arm.obligation, Some(prepaid), disposition),
                    ),
                    no_action,
                )
            }
            QuoteTransactionState::SinkInvokedCharged { arm } => {
                let generation = arm.identity.generation();
                (
                    Self::terminal_before_route_settlement(
                        generation,
                        Self::terminal_state(arm.obligation, None, disposition),
                    ),
                    no_action,
                )
            }
            QuoteTransactionState::Settled {
                generation,
                route,
                reopened: _,
                stable,
            } => {
                let (stable, commit) = Self::reduce_terminal(*stable, disposition);
                (
                    QuoteTransactionState::Settled {
                        generation,
                        route,
                        reopened: false,
                        stable: Box::new(stable),
                    },
                    commit,
                )
            }
            QuoteTransactionState::ReplacementPendingBackoffPrepaid { prepaid } => {
                drop(prepaid);
                (QuoteTransactionState::Idle, no_action)
            }
            QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff => {
                (QuoteTransactionState::Idle, no_action)
            }
        }
    }

    fn reduce_reopened(state: QuoteTransactionState) -> QuoteReduction {
        let no_action = GovernedQuoteTransactionCommit::no_action();
        match state {
            QuoteTransactionState::Settled {
                generation,
                route,
                reopened,
                stable,
            } if reopened
                || matches!(
                    stable.leg_state(),
                    LegState::Idle | LegState::CancelPending | LegState::ReplacementPendingBackoff
                ) =>
            {
                Ok((
                    QuoteTransactionState::Settled {
                        generation,
                        route,
                        reopened: true,
                        stable,
                    },
                    no_action,
                ))
            }
            state => Err((
                state,
                anyhow::anyhow!("maker quote reopening conflicts with current leg occupancy"),
            )),
        }
    }
}

#[derive(Clone)]
struct GovernedQuoteTransaction {
    inner: Arc<Mutex<GovernedQuoteTransactionInner>>,
}

impl GovernedQuoteTransaction {
    fn new(supports_modify: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GovernedQuoteTransactionInner {
                state: QuoteTransactionState::Idle,
                supports_modify,
                terminal_owner: None,
                retention_scope_closed: false,
            })),
        }
    }

    fn reduce(
        &self,
        event: impl Into<QuoteTransactionReductionRequest>,
    ) -> anyhow::Result<GovernedQuoteTransactionCommit> {
        event
            .into()
            .apply(&mut self.inner.lock().expect("quote transaction lock poisoned"))
    }

    fn refine(
        &self,
        event: MakerQuoteLifecycleRefinementEvent,
    ) -> MakerQuoteLifecycleRefinementOutcome {
        MakerQuoteLifecycleRefinementRequest { event }
            .apply(&mut self.inner.lock().expect("quote transaction lock poisoned"))
    }

    fn shares_authority_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn retention_scope_is_closed(&self) -> bool {
        self.inner
            .lock()
            .expect("quote transaction lock poisoned")
            .retention_scope_closed
    }

    fn close_retention_scope(&self) -> anyhow::Result<GovernedQuoteTransactionCommit> {
        let mut inner = self.inner.lock().expect("quote transaction lock poisoned");
        inner.retention_scope_closed = true;
        QuoteTransactionReductionRequest::from(QuoteTransactionEvent::WindDown).apply(&mut inner)
    }

    fn leg_state(&self) -> LegState {
        self.inner
            .lock()
            .expect("quote transaction lock poisoned")
            .state
            .leg_state()
    }

    fn supports_modify(&self) -> bool {
        self.inner
            .lock()
            .expect("quote transaction lock poisoned")
            .supports_modify
    }

    fn prepaid_generation(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("quote transaction lock poisoned")
            .state
            .prepaid_generation()
    }

    fn propose(&self, leg: Leg, event: LegEvent) -> Option<QuoteLegTransitionProposal> {
        let inner = self.inner.lock().expect("quote transaction lock poisoned");
        if inner.state.is_winding_down() {
            return None;
        }
        let prior_state = inner.state.leg_state();
        let (action, pending_state) = match (prior_state, event) {
            (LegState::Idle, LegEvent::QuoteTrigger { .. }) => {
                (LifecycleAction::Submit, LegState::SubmitPending)
            }
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) if inner.supports_modify => (LifecycleAction::Modify, LegState::ModifyPending),
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) => (LifecycleAction::Cancel, LegState::RequotePending),
            (LegState::ReplacementPendingBackoff, LegEvent::QuoteTrigger { .. }) => {
                (LifecycleAction::Submit, LegState::SubmitPending)
            }
            (LegState::ModifyPending, LegEvent::ModifyRejected) => {
                (LifecycleAction::Cancel, LegState::RequotePending)
            }
            (
                LegState::Idle
                | LegState::SubmitPending
                | LegState::Resting
                | LegState::RequotePending
                | LegState::ModifyPending
                | LegState::CancelPending
                | LegState::ReplacementPendingBackoff
                | LegState::PoisonedReconciliationHold,
                LegEvent::QuoteTrigger { .. }
                | LegEvent::Accepted
                | LegEvent::Rejected
                | LegEvent::Canceled
                | LegEvent::Modified
                | LegEvent::ModifyRejected
                | LegEvent::CancelRejected
                | LegEvent::Filled,
            ) => return None,
        };
        Some(QuoteLegTransitionProposal {
            leg,
            action,
            prior_state,
            pending_state,
        })
    }

    fn generation(&self) -> Option<u64> {
        let inner = self.inner.lock().expect("quote transaction lock poisoned");
        match &inner.state {
            QuoteTransactionState::ArmedReserved { arm, .. }
            | QuoteTransactionState::ArmedPrepaid { arm, .. }
            | QuoteTransactionState::SinkInvokedCharged { arm }
            | QuoteTransactionState::SinkInvokedPrepaid { arm, .. } => {
                Some(arm.identity.generation())
            }
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::ArmedReserved { arm, .. }
                | WindDownQuoteTransactionState::ArmedPrepaid { arm, .. }
                | WindDownQuoteTransactionState::SinkInvokedCharged { arm }
                | WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, .. },
            ) => Some(arm.identity.generation()),
            QuoteTransactionState::Settled { generation, .. } => Some(*generation),
            QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::Idle
                | WindDownQuoteTransactionState::CancelPending
                | WindDownQuoteTransactionState::PoisonedReconciliationHold
                | WindDownQuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. },
            ) => None,
        }
    }

    fn registration_phase(&self, generation: u64) -> QuoteTransactionRegistrationPhase {
        let inner = self.inner.lock().expect("quote transaction lock poisoned");
        match &inner.state {
            QuoteTransactionState::SinkInvokedCharged { arm }
            | QuoteTransactionState::SinkInvokedPrepaid { arm, .. }
                if arm.identity.generation() == generation =>
            {
                QuoteTransactionRegistrationPhase::SinkInvoked
            }
            QuoteTransactionState::WindingDown(
                WindDownQuoteTransactionState::SinkInvokedCharged { arm }
                | WindDownQuoteTransactionState::SinkInvokedPrepaid { arm, .. },
            ) if arm.identity.generation() == generation => {
                QuoteTransactionRegistrationPhase::SinkInvoked
            }
            QuoteTransactionState::Settled {
                generation: settled_generation,
                ..
            } if *settled_generation == generation => QuoteTransactionRegistrationPhase::Settled,
            QuoteTransactionState::Idle
            | QuoteTransactionState::SubmitPending
            | QuoteTransactionState::Resting
            | QuoteTransactionState::RequotePending { .. }
            | QuoteTransactionState::ModifyPending
            | QuoteTransactionState::CancelPending
            | QuoteTransactionState::ReplacementPendingBackoff
            | QuoteTransactionState::ReplacementPendingBackoffPrepaid { .. }
            | QuoteTransactionState::PoisonedReconciliationHold { .. }
            | QuoteTransactionState::PoisonedReconciliationHoldPrepaid { .. }
            | QuoteTransactionState::ArmedReserved { .. }
            | QuoteTransactionState::ArmedPrepaid { .. }
            | QuoteTransactionState::SinkInvokedCharged { .. }
            | QuoteTransactionState::SinkInvokedPrepaid { .. }
            | QuoteTransactionState::WindingDown(_)
            | QuoteTransactionState::Settled { .. } => QuoteTransactionRegistrationPhase::PreSink,
        }
    }
}

/// The two sides of a binary market the maker quotes.
#[repr(usize)]
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

/// The cadence and both NT instruments that define one governed order-lifecycle
/// scope. Venue evidence metadata may change without changing this identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerOrderLifecycleScopeIdentity {
    start_timestamp_milliseconds: u64,
    instrument_ids: [InstrumentId; 2],
}

impl MakerOrderLifecycleScopeIdentity {
    #[must_use]
    pub const fn new(
        start_timestamp_milliseconds: u64,
        yes_instrument_id: InstrumentId,
        no_instrument_id: InstrumentId,
    ) -> Self {
        Self {
            start_timestamp_milliseconds,
            instrument_ids: [yes_instrument_id, no_instrument_id],
        }
    }

    #[must_use]
    pub(crate) const fn instrument_id(self, leg: Leg) -> InstrumentId {
        self.instrument_ids[leg as usize]
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
static NEXT_TEST_LIFECYCLE_SCOPE: AtomicU64 = AtomicU64::new(0);

/// A market's two governed quote transactions and cancel-scope controller,
/// sealed to the cadence and NT instruments whose orders it may govern.
#[derive(Clone)]
pub struct MarketQuote {
    scope_identity: MakerOrderLifecycleScopeIdentity,
    yes: GovernedQuoteTransaction,
    no: GovernedQuoteTransaction,
}

impl std::fmt::Debug for MarketQuote {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MarketQuote")
            .field("scope_identity", &self.scope_identity)
            .field("yes_state", &self.yes.leg_state())
            .field("no_state", &self.no.leg_state())
            .field("yes_prepaid_generation", &self.yes.prepaid_generation())
            .field("no_prepaid_generation", &self.no.prepaid_generation())
            .finish()
    }
}

impl PartialEq for MarketQuote {
    fn eq(&self, other: &Self) -> bool {
        self.scope_identity == other.scope_identity && self.snapshot() == other.snapshot()
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

    pub(crate) fn refine(
        &self,
        event: MakerQuoteLifecycleRefinementEvent,
    ) -> MakerQuoteLifecycleRefinementOutcome {
        self.market.transaction(self.leg).refine(event)
    }

    pub(crate) fn shares_authority_with(&self, other: &Self) -> bool {
        self.leg == other.leg
            && self.market.scope_identity == other.market.scope_identity
            && self
                .market
                .transaction(self.leg)
                .shares_authority_with(other.market.transaction(other.leg))
    }

    pub(crate) const fn scope_identity(&self) -> MakerOrderLifecycleScopeIdentity {
        self.market.scope_identity
    }

    pub(crate) fn shares_lifecycle_scope_with(&self, other: &Self) -> bool {
        self.leg == other.leg && self.scope_identity() == other.scope_identity()
    }

    pub(crate) fn retention_scope_is_closed(&self) -> bool {
        self.market
            .transaction(self.leg)
            .retention_scope_is_closed()
    }

    pub(crate) fn hold_missing_money_moving_truth(&self) -> bool {
        self.market
            .transaction(self.leg)
            .reduce(QuoteTransactionEvent::MissingMoneyMovingTruth)
            .is_ok()
    }
}

impl MarketQuote {
    /// A fresh market with both legs idle, sealed to one typed order-lifecycle
    /// scope. `supports_modify` is the venue capability fact shared by both legs.
    #[must_use]
    pub fn new(scope_identity: MakerOrderLifecycleScopeIdentity, supports_modify: bool) -> Self {
        Self {
            scope_identity,
            yes: GovernedQuoteTransaction::new(supports_modify),
            no: GovernedQuoteTransaction::new(supports_modify),
        }
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[must_use]
    pub fn new_for_test(supports_modify: bool) -> Self {
        let scope_generation = NEXT_TEST_LIFECYCLE_SCOPE.fetch_add(1, Ordering::Relaxed);
        Self::new(
            MakerOrderLifecycleScopeIdentity::new(
                scope_generation,
                InstrumentId::from("TEST-SCOPE-YES.SIM"),
                InstrumentId::from("TEST-SCOPE-NO.SIM"),
            ),
            supports_modify,
        )
    }

    #[must_use]
    pub const fn scope_identity(&self) -> MakerOrderLifecycleScopeIdentity {
        self.scope_identity
    }

    fn transaction(&self, leg: Leg) -> &GovernedQuoteTransaction {
        match leg {
            Leg::Yes => &self.yes,
            Leg::No => &self.no,
        }
    }

    pub(crate) fn shares_authority_with(&self, other: &Self) -> bool {
        self.scope_identity == other.scope_identity
            && self.yes.shares_authority_with(&other.yes)
            && self.no.shares_authority_with(&other.no)
    }

    /// The lifecycle state of one leg.
    pub fn leg_state(&self, leg: Leg) -> LegState {
        self.transaction(leg).leg_state()
    }

    pub fn leg_supports_modify(&self, leg: Leg) -> bool {
        self.transaction(leg).supports_modify()
    }

    /// The aggregate market quoting state.
    pub fn market_state(&self) -> MarketState {
        let states = [self.yes.leg_state(), self.no.leg_state()];
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
        self.transaction(leg)
            .reduce(QuoteTransactionEvent::Lifecycle(event))
            .expect("quote lifecycle event reduction must be total")
            .action
            .map(|action| MarketAction::Leg { leg, action })
    }

    pub fn propose_leg_event(
        &self,
        leg: Leg,
        event: LegEvent,
    ) -> Option<QuoteLegTransitionProposal> {
        self.transaction(leg).propose(leg, event)
    }

    pub(crate) fn arm_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        budget: RequoteBudgetPair,
        budget_proposal: MakerQuoteBudgetProposal,
        identity: MakerQuoteLifecycleIdentity,
    ) -> anyhow::Result<()> {
        let commit = self
            .transaction(proposal.leg)
            .reduce(QuoteTransactionEvent::Arm {
                proposal,
                budget,
                budget_proposal,
                identity,
            })?;
        anyhow::ensure!(
            commit.action.is_none(),
            "arming cannot emit a lifecycle action"
        );
        Ok(())
    }

    pub(crate) fn mark_leg_transaction_sink_invoked(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
        actor_now_ms: u64,
    ) -> anyhow::Result<()> {
        let commit = self.transaction(proposal.leg).reduce(
            SinkCapableQuoteTransactionEvent::SinkInvoked {
                generation,
                actor_now_ms,
            },
        )?;
        anyhow::ensure!(
            commit.action.is_none(),
            "sink invocation cannot emit a lifecycle action"
        );
        Ok(())
    }

    pub(crate) fn transaction_generation(
        &self,
        proposal: QuoteLegTransitionProposal,
    ) -> Option<u64> {
        self.transaction(proposal.leg).generation()
    }

    pub(crate) fn leg_transaction_registration_phase(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> QuoteTransactionRegistrationPhase {
        self.transaction(proposal.leg)
            .registration_phase(generation)
    }

    pub(crate) fn unwind_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
    ) -> anyhow::Result<()> {
        let commit = self
            .transaction(proposal.leg)
            .reduce(QuoteTransactionEvent::Unwind)?;
        anyhow::ensure!(
            commit.action.is_none(),
            "unwind cannot emit a lifecycle action"
        );
        Ok(())
    }

    pub(crate) fn commit_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        let event = match proposal.action {
            LifecycleAction::Submit => SinkCapableQuoteTransactionEvent::Submitted { generation },
            LifecycleAction::Cancel | LifecycleAction::Modify => {
                SinkCapableQuoteTransactionEvent::CommandIssued { generation }
            }
        };
        self.transaction(proposal.leg).reduce(event).is_ok()
    }

    pub(crate) fn abort_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.transaction(proposal.leg)
            .reduce(QuoteTransactionEvent::PreSinkAbort { generation })
            .is_ok()
    }

    pub(crate) fn retire_leg_transaction_from_callback(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.transaction(proposal.leg)
            .reduce(SinkCapableQuoteTransactionEvent::CallbackRetired { generation })
            .is_ok()
    }

    pub(crate) fn fail_pre_sink_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.transaction(proposal.leg)
            .reduce(QuoteTransactionEvent::PreSinkInvariantFailure { generation })
            .is_ok()
    }

    pub(crate) fn unwind_post_sink_leg_transaction(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.transaction(proposal.leg)
            .reduce(SinkCapableQuoteTransactionEvent::PostSinkUnwind { generation })
            .is_ok()
    }

    pub(crate) fn reject_leg_transaction_at_sink(
        &self,
        proposal: QuoteLegTransitionProposal,
        generation: u64,
    ) -> bool {
        self.transaction(proposal.leg)
            .reduce(SinkCapableQuoteTransactionEvent::SinkRejected { generation })
            .is_ok()
    }

    pub(crate) fn prepaid_generation(&self, leg: Leg) -> Option<u64> {
        self.transaction(leg).prepaid_generation()
    }

    fn snapshot(&self) -> (LegState, LegState, Option<u64>, Option<u64>) {
        (
            self.yes.leg_state(),
            self.no.leg_state(),
            self.yes.prepaid_generation(),
            self.no.prepaid_generation(),
        )
    }

    /// T8 — cancel exactly one leg (e.g. a one-sided inventory/skew breach),
    /// leaving the other leg resting. Per-order `cancel_order` for that leg only.
    pub fn cancel_leg(&mut self, leg: Leg) -> Option<MarketAction> {
        self.transaction(leg)
            .reduce(QuoteTransactionEvent::WindDown)
            .expect("wind-down quote transition must be total")
            .action
            .map(|action| MarketAction::Leg { leg, action })
    }

    /// T9 — drain the whole market: request coordinated per-order cancellation
    /// for every tracked working order on both legs, with no resubmit.
    pub fn drain(&mut self) -> Option<MarketAction> {
        let yes = self
            .yes
            .reduce(QuoteTransactionEvent::WindDown)
            .expect("YES wind-down quote transition must be total")
            .action
            .is_some();
        let no = self
            .no
            .reduce(QuoteTransactionEvent::WindDown)
            .expect("NO wind-down quote transition must be total")
            .action
            .is_some();
        (yes || no).then_some(MarketAction::CancelAllBothLegs)
    }

    pub(crate) fn close_retention_scope(&mut self) -> Option<MarketAction> {
        let yes = self
            .yes
            .close_retention_scope()
            .expect("YES retention-scope close transition must be total")
            .action
            .is_some();
        let no = self
            .no
            .close_retention_scope()
            .expect("NO retention-scope close transition must be total")
            .action
            .is_some();
        (yes || no).then_some(MarketAction::CancelAllBothLegs)
    }

    /// T9a — cancel one side of the market (e.g. a one-sided exposure cap),
    /// leaving the other side, through the per-order cancellation coordinator.
    pub fn cancel_one_side(&mut self, leg: Leg) -> Option<MarketAction> {
        self.transaction(leg)
            .reduce(QuoteTransactionEvent::WindDown)
            .expect("one-side wind-down quote transition must be total")
            .action
            .map(|_| MarketAction::CancelAllOneSide { leg })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_requote_budget::RequoteBudget;

    /// Leg-scoped test driver over the production governed authority.
    struct LegHarness {
        market: MarketQuote,
        budget: RequoteBudgetPair,
        generation: u64,
    }

    impl LegHarness {
        fn new(supports_modify: bool) -> Self {
            Self {
                market: MarketQuote::new_for_test(supports_modify),
                budget: RequoteBudgetPair::new(
                    RequoteBudget::new(100, 60_000, 0),
                    RequoteBudget::new(100, 60_000, 0),
                ),
                generation: 1,
            }
        }

        fn state(&self) -> LegState {
            self.market.leg_state(Leg::Yes)
        }

        fn on_event(&mut self, event: LegEvent) -> Option<LifecycleAction> {
            if let LegEvent::QuoteTrigger { .. } = event {
                let proposal = self.market.propose_leg_event(Leg::Yes, event)?;
                let budget_proposal = match proposal.action {
                    LifecycleAction::Submit => {
                        if let Some(generation) = self.market.prepaid_generation(Leg::Yes) {
                            MakerQuoteBudgetProposal::Prepaid {
                                generation,
                                now_ms: 1,
                            }
                        } else {
                            MakerQuoteBudgetProposal::Reserve(
                                self.budget
                                    .propose_fresh_submit(1)
                                    .expect("test submit budget should be available"),
                            )
                        }
                    }
                    LifecycleAction::Cancel
                        if proposal.pending_state == LegState::RequotePending =>
                    {
                        MakerQuoteBudgetProposal::Reserve(
                            self.budget
                                .propose_cancel_resubmit(1)
                                .expect("test requote budget should be available"),
                        )
                    }
                    LifecycleAction::Cancel | LifecycleAction::Modify => {
                        MakerQuoteBudgetProposal::Reserve(
                            self.budget
                                .propose_rest(1)
                                .expect("test REST budget should be available"),
                        )
                    }
                };
                let generation = self.generation;
                self.generation = self
                    .generation
                    .checked_add(1)
                    .expect("test transaction generation should not overflow");
                self.market
                    .arm_leg_transaction(
                        proposal,
                        self.budget.clone(),
                        budget_proposal,
                        MakerQuoteLifecycleIdentity::new("TEST-LEG-ORDER", generation),
                    )
                    .expect("test transaction should arm");
                self.market
                    .mark_leg_transaction_sink_invoked(proposal, generation, 1)
                    .expect("test transaction should reach the sink");
                assert!(
                    self.market.commit_leg_transaction(proposal, generation),
                    "test transaction should settle"
                );
                return Some(proposal.action);
            }
            match self.market.on_leg_event(Leg::Yes, event) {
                Some(MarketAction::Leg {
                    leg: Leg::Yes,
                    action,
                }) => Some(action),
                None => None,
                Some(
                    MarketAction::Leg { leg: Leg::No, .. }
                    | MarketAction::CancelAllBothLegs
                    | MarketAction::CancelAllOneSide { .. },
                ) => {
                    panic!("leg-scoped event emitted a non-leg action")
                }
            }
        }

        fn request_cancel(&mut self) -> Option<LifecycleAction> {
            match self.market.cancel_leg(Leg::Yes) {
                Some(MarketAction::Leg {
                    leg: Leg::Yes,
                    action,
                }) => Some(action),
                None => None,
                Some(
                    MarketAction::Leg { leg: Leg::No, .. }
                    | MarketAction::CancelAllBothLegs
                    | MarketAction::CancelAllOneSide { .. },
                ) => {
                    panic!("leg-scoped wind-down emitted a non-leg action")
                }
            }
        }
    }

    /// Build a leg that already holds a resting quote, for the requote tests.
    fn resting_leg(supports_modify: bool) -> LegHarness {
        let mut leg = LegHarness::new(supports_modify);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        assert_eq!(leg.state(), LegState::Resting);
        leg
    }

    #[test]
    fn idle_trigger_submits_and_pends() {
        let mut leg = LegHarness::new(false);
        assert_eq!(leg.state(), LegState::Idle);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn submit_pending_accepted_rests() {
        let mut leg = LegHarness::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Accepted);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn submit_pending_rejected_returns_to_idle() {
        let mut leg = LegHarness::new(false);
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
        let mut submitting = LegHarness::new(false);
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
            let mut leg = LegHarness::new(false);
            assert_eq!(leg.state(), LegState::Idle);
            assert_eq!(leg.on_event(event), Some(LifecycleAction::Cancel));
            assert_eq!(leg.state(), LegState::Idle, "stays Idle and re-hunts");
        }
        // A stale/duplicate Filled in Idle remains a no-op (not an orphan).
        let mut leg = LegHarness::new(false);
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
    fn modify_reject_emits_cancel_without_manufacturing_prepaid_state() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        // The callback can request a cancel, but it cannot manufacture the
        // separately admitted replacement capacity.
        let action = leg.on_event(LegEvent::ModifyRejected);
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::Resting);
        assert_eq!(leg.market.prepaid_generation(Leg::Yes), None);
    }

    #[test]
    fn second_trigger_while_in_flight_emits_no_duplicate_command() {
        // Cancel+resubmit path.
        let mut leg = LegHarness::new(false);
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
        let mut leg = LegHarness::new(false);
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
        let mut leg = LegHarness::new(false);
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
        let mut market = MarketQuote::new_for_test(supports_modify);
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
        let market = MarketQuote::new_for_test(false);
        assert_eq!(market.market_state(), MarketState::Idle);
    }

    #[test]
    fn both_legs_resting_is_quoting() {
        let market = resting_market(false);
        assert_eq!(market.market_state(), MarketState::Quoting);
    }

    #[test]
    fn on_leg_event_wraps_action_with_leg_id_and_isolates_legs() {
        let mut market = MarketQuote::new_for_test(false);
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
        let mut market = MarketQuote::new_for_test(false);
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
        let mut leg = LegHarness::new(false);
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
        let mut leg = LegHarness::new(false);
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

        let mut modify_capable = resting_leg(true);
        assert_eq!(
            modify_capable.on_event(trigger),
            Some(LifecycleAction::Modify),
            "a modify-capable venue amends in place"
        );
        assert_eq!(modify_capable.state(), LegState::ModifyPending);

        let mut no_modify = resting_leg(false);
        assert_eq!(
            no_modify.on_event(trigger),
            Some(LifecycleAction::Cancel),
            "a no-modify venue cancels then resubmits, never a Modify"
        );
        assert_eq!(no_modify.state(), LegState::RequotePending);
    }

    #[test]
    fn governed_transaction_state_event_table_is_total_and_replay_safe() {
        let mut invalid = GovernedQuoteTransactionInner {
            state: QuoteTransactionState::Idle,
            supports_modify: false,
            terminal_owner: None,
            retention_scope_closed: false,
        };
        assert!(
            QuoteTransactionReductionRequest::from(SinkCapableQuoteTransactionEvent::Submitted {
                generation: 1
            })
            .apply(&mut invalid)
            .is_err()
        );
        assert!(matches!(invalid.state, QuoteTransactionState::Idle));

        let mut mismatched_success = GovernedQuoteTransactionInner {
            state: QuoteTransactionState::SinkInvokedCharged {
                arm: QuoteTransactionArm {
                    identity: MakerQuoteLifecycleIdentity::new("TEST-MISMATCHED-SUCCESS", 1),
                    prior_state: LegState::Resting,
                    pending_state: LegState::CancelPending,
                    obligation: QuoteLegTransactionObligation::PlainCancel,
                },
            },
            supports_modify: false,
            terminal_owner: None,
            retention_scope_closed: false,
        };
        assert!(
            QuoteTransactionReductionRequest::from(SinkCapableQuoteTransactionEvent::Submitted {
                generation: 1,
            })
            .apply(&mut mismatched_success)
            .is_err()
        );
        assert!(matches!(
            mismatched_success.state,
            QuoteTransactionState::SinkInvokedCharged {
                arm: QuoteTransactionArm {
                    obligation: QuoteLegTransactionObligation::PlainCancel,
                    ..
                }
            }
        ));

        let mut replay = GovernedQuoteTransactionInner {
            state: QuoteTransactionState::Settled {
                generation: 2,
                route: Some(QuoteRouteSettlement::Submitted),
                reopened: false,
                stable: Box::new(QuoteTransactionState::SubmitPending),
            },
            supports_modify: false,
            terminal_owner: None,
            retention_scope_closed: false,
        };
        assert!(
            QuoteTransactionReductionRequest::from(SinkCapableQuoteTransactionEvent::Submitted {
                generation: 2
            })
            .apply(&mut replay)
            .is_ok()
        );
        assert!(matches!(
            replay.state,
            QuoteTransactionState::Settled {
                generation: 2,
                route: Some(QuoteRouteSettlement::Submitted),
                reopened: false,
                ..
            }
        ));
        assert!(
            QuoteTransactionReductionRequest::from(
                SinkCapableQuoteTransactionEvent::SinkRejected { generation: 2 }
            )
            .apply(&mut replay)
            .is_err()
        );
        assert!(matches!(
            replay.state,
            QuoteTransactionState::Settled {
                generation: 2,
                route: Some(QuoteRouteSettlement::Submitted),
                reopened: false,
                ..
            }
        ));

        for (generation, obligation, pending_state) in [
            (
                3,
                QuoteLegTransactionObligation::PlainCancel,
                LegState::CancelPending,
            ),
            (
                4,
                QuoteLegTransactionObligation::Modify,
                LegState::ModifyPending,
            ),
        ] {
            let mut inner = GovernedQuoteTransactionInner {
                state: QuoteTransactionState::SinkInvokedCharged {
                    arm: QuoteTransactionArm {
                        identity: MakerQuoteLifecycleIdentity::new("TEST-MATRIX-ORDER", generation),
                        prior_state: LegState::Resting,
                        pending_state,
                        obligation,
                    },
                },
                supports_modify: obligation == QuoteLegTransactionObligation::Modify,
                terminal_owner: None,
                retention_scope_closed: false,
            };
            assert!(matches!(
                MakerQuoteLifecycleRefinementRequest {
                    event: MakerQuoteLifecycleRefinementEvent::new(
                        MakerQuoteLifecycleIdentity::new("TEST-MATRIX-ORDER", generation),
                        MakerQuoteLifecycleRefinement::Terminal {
                            stable_effect: Some(MakerQuoteTerminalDisposition::Canceled),
                            closes_reopened: false,
                        },
                    ),
                }
                .apply(&mut inner),
                MakerQuoteLifecycleRefinementOutcome::Applied
            ));
            assert_eq!(inner.state.leg_state(), LegState::Idle);
            assert!(
                QuoteTransactionReductionRequest::from(
                    SinkCapableQuoteTransactionEvent::CommandIssued { generation }
                )
                .apply(&mut inner)
                .is_ok()
            );
            assert!(matches!(
                inner.state,
                QuoteTransactionState::Settled {
                    route: Some(QuoteRouteSettlement::CommandIssued),
                    reopened: false,
                    ..
                }
            ));
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum TerminalMatrixMode {
        Active,
        WindingDown,
    }

    #[derive(Clone, Copy, Debug)]
    struct TerminalMatrixExpectation {
        state: LegState,
        outstanding_submit_cost: u64,
        outstanding_rest_cost: u64,
    }

    impl TerminalMatrixMode {
        fn prepare(self, inner: &mut GovernedQuoteTransactionInner) {
            match self {
                Self::Active => {}
                Self::WindingDown => {
                    let commit =
                        QuoteTransactionReductionRequest::from(QuoteTransactionEvent::WindDown)
                            .apply(inner)
                            .expect("wind-down reduction must be total");
                    assert_eq!(commit.action, Some(LifecycleAction::Cancel));
                }
            }
        }

        const fn expectation(
            self,
            obligation: QuoteLegTransactionObligation,
            disposition: MakerQuoteTerminalDisposition,
        ) -> TerminalMatrixExpectation {
            use MakerQuoteTerminalDisposition::{
                Canceled, Denied, Expired, Filled, Rejected, Voided,
            };
            use QuoteLegTransactionObligation::{
                FreshSubmit, Modify, PlainCancel, ReplacementSubmit, RequoteCancel,
            };

            match (self, obligation, disposition) {
                (
                    Self::WindingDown,
                    _,
                    Denied | Rejected | Canceled | Expired | Filled | Voided,
                )
                | (Self::Active, _, Filled)
                | (
                    Self::Active,
                    FreshSubmit | PlainCancel | Modify,
                    Denied | Rejected | Canceled | Expired | Voided,
                ) => TerminalMatrixExpectation {
                    state: LegState::Idle,
                    outstanding_submit_cost: 0,
                    outstanding_rest_cost: 0,
                },
                (
                    Self::Active,
                    ReplacementSubmit,
                    Denied | Rejected | Canceled | Expired | Voided,
                ) => TerminalMatrixExpectation {
                    state: LegState::ReplacementPendingBackoff,
                    outstanding_submit_cost: 0,
                    outstanding_rest_cost: 0,
                },
                (Self::Active, RequoteCancel, Denied | Rejected | Canceled | Expired | Voided) => {
                    TerminalMatrixExpectation {
                        state: LegState::ReplacementPendingBackoff,
                        outstanding_submit_cost: 1,
                        outstanding_rest_cost: 1,
                    }
                }
            }
        }

        fn assert_post_terminal(self, inner: &mut GovernedQuoteTransactionInner) {
            match self {
                Self::Active => {}
                Self::WindingDown => {
                    let commit = QuoteTransactionReductionRequest::from(
                        QuoteTransactionEvent::Lifecycle(LegEvent::QuoteTrigger {
                            requote_needed: true,
                        }),
                    )
                    .apply(inner)
                    .expect("post-wind-down quote trigger must be total");
                    assert_eq!(commit.action, None);
                    assert_eq!(inner.state.leg_state(), LegState::Idle);
                }
            }
        }
    }

    fn terminal_matrix_state(
        obligation: QuoteLegTransactionObligation,
        budget: &RequoteBudgetPair,
    ) -> QuoteTransactionState {
        match obligation {
            QuoteLegTransactionObligation::RequoteCancel => {
                let proposal = budget
                    .propose_cancel_resubmit(1)
                    .expect("matrix prepaid capacity should be available");
                let mut prepaid = budget
                    .reserve(proposal)
                    .expect("matrix prepaid reservation should succeed");
                prepaid
                    .mark_sink_invoked_at(1)
                    .expect("matrix cancel should reach the sink");
                QuoteTransactionState::PoisonedReconciliationHoldPrepaid {
                    obligation,
                    prepaid,
                }
            }
            QuoteLegTransactionObligation::FreshSubmit
            | QuoteLegTransactionObligation::ReplacementSubmit
            | QuoteLegTransactionObligation::PlainCancel
            | QuoteLegTransactionObligation::Modify => {
                QuoteTransactionState::PoisonedReconciliationHold { obligation }
            }
        }
    }

    #[test]
    fn terminal_matrix_latches_wind_down_and_retires_every_obligation() {
        let obligations = [
            QuoteLegTransactionObligation::FreshSubmit,
            QuoteLegTransactionObligation::ReplacementSubmit,
            QuoteLegTransactionObligation::RequoteCancel,
            QuoteLegTransactionObligation::PlainCancel,
            QuoteLegTransactionObligation::Modify,
        ];
        let dispositions = [
            MakerQuoteTerminalDisposition::Denied,
            MakerQuoteTerminalDisposition::Rejected,
            MakerQuoteTerminalDisposition::Canceled,
            MakerQuoteTerminalDisposition::Expired,
            MakerQuoteTerminalDisposition::Filled,
            MakerQuoteTerminalDisposition::Voided,
        ];

        for obligation in obligations {
            for disposition in dispositions {
                for mode in [TerminalMatrixMode::Active, TerminalMatrixMode::WindingDown] {
                    let budget = RequoteBudgetPair::new(
                        RequoteBudget::new(8, 60_000, 0),
                        RequoteBudget::new(8, 60_000, 0),
                    );
                    let mut inner = GovernedQuoteTransactionInner {
                        state: terminal_matrix_state(obligation, &budget),
                        supports_modify: false,
                        terminal_owner: Some(MakerQuoteLifecycleIdentity::new(
                            "TEST-TERMINAL-MATRIX",
                            1,
                        )),
                        retention_scope_closed: false,
                    };
                    mode.prepare(&mut inner);

                    assert!(matches!(
                        MakerQuoteLifecycleRefinementRequest {
                            event: MakerQuoteLifecycleRefinementEvent::new(
                                MakerQuoteLifecycleIdentity::new("TEST-TERMINAL-MATRIX", 1),
                                MakerQuoteLifecycleRefinement::Terminal {
                                    stable_effect: Some(disposition),
                                    closes_reopened: false,
                                },
                            ),
                        }
                        .apply(&mut inner),
                        MakerQuoteLifecycleRefinementOutcome::Applied
                    ));

                    let expected = mode.expectation(obligation, disposition);
                    assert_eq!(
                        inner.state.leg_state(),
                        expected.state,
                        "obligation={obligation:?} disposition={disposition:?} mode={mode:?}"
                    );
                    assert_eq!(
                        budget.outstanding_submit_cost(),
                        expected.outstanding_submit_cost,
                        "submit liability: obligation={obligation:?} disposition={disposition:?} mode={mode:?}"
                    );
                    assert_eq!(
                        budget.outstanding_rest_cost(),
                        expected.outstanding_rest_cost,
                        "REST liability: obligation={obligation:?} disposition={disposition:?} mode={mode:?}"
                    );
                    mode.assert_post_terminal(&mut inner);
                }
            }
        }
    }
}
