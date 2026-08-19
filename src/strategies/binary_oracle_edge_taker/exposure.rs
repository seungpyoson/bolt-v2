use nautilus_model::{
    enums::{OrderSide, PositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId, Venue},
    types::Quantity,
};

use crate::{
    bolt_v3_book_sizing::OutcomeBookState,
    bolt_v3_current_evidence::OrderLifecycleOutcome,
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_order_execution::BoltV3ExitOrderAuthorityHandle,
    bolt_v3_position_contract::{
        BoltV3PositionMarketLifecycle, expected_exit_order_side_for_position,
        expected_position_side_for_entry_order, is_observed_open_side,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OpenPositionState {
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) position_id: PositionId,
    pub(super) entry_order_side: OrderSide,
    pub(super) side: PositionSide,
    pub(super) quantity: Quantity,
    pub(super) avg_px_open: f64,
    pub(super) book: OutcomeBookState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingEntryState {
    pub(super) client_order_id: ClientOrderId,
    pub(super) submitted_at_ms: Option<u64>,
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) book: OutcomeBookState,
}

#[derive(Debug, Clone, PartialEq)]
struct PendingEntryArmState {
    generation: u64,
    pending: PendingEntryState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingExitState {
    pub(super) client_order_id: ClientOrderId,
    pub(super) submitted_at_ms: Option<u64>,
    pub(super) market_id: Option<String>,
    pub(super) position_id: Option<PositionId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionContext {
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) position_id: PositionId,
    pub(super) book: OutcomeBookState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionState {
    pub(super) position: OpenPositionState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitPendingState {
    pub(super) position: Option<ManagedPositionContext>,
    pub(super) pending_exit: PendingExitState,
    pub(super) authority: BoltV3ExitOrderAuthorityHandle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitAttemptingState {
    pub(super) generation: u64,
    pub(super) managed: ManagedPositionContext,
    pub(super) pending_exit: PendingExitState,
    pub(super) authority: BoltV3ExitOrderAuthorityHandle,
}

impl ExitAttemptingState {
    pub(super) fn snapshot(&self) -> ExitPendingState {
        ExitPendingState {
            position: Some(self.managed.clone()),
            pending_exit: self.pending_exit.clone(),
            authority: self.authority.clone(),
        }
    }

    pub(super) fn into_pending(self) -> ExitPendingState {
        ExitPendingState {
            position: Some(self.managed),
            pending_exit: self.pending_exit,
            authority: self.authority,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitLifecyclePhase {
    Attempting,
    Working,
    TerminalAwaitingPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryReconcileReason {
    AwaitingPositionMaterialization,
    UnresolvedAtSelectionBoundary,
    UnsupportedEntryFillSide {
        order_side: OrderSide,
    },
    InvalidObservedPosition {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnsupportedObservedReason {
    LiveUnsupportedContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlindRecoveryReason {
    CacheProbeFailed,
    MultipleOpenPositions {
        count: usize,
    },
    InvalidBootstrappedPosition {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
    InvalidLivePosition {
        entry_order_side: OrderSide,
        side: Option<PositionSide>,
    },
    SettlementEvidenceRecoveryFailed,
    RestartOpenPosition {
        instrument_id: InstrumentId,
        position_id: PositionId,
    },
    UnretainedExitCorrection {
        instrument_id: InstrumentId,
    },
    ForeignVenuePosition {
        instrument_venue: Venue,
        execution_venue: Venue,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct UnsupportedObservedState {
    pub(super) context: ManagedPositionContext,
    pub(super) reason: UnsupportedObservedReason,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BlindRecoveryState {
    pub(super) reason: BlindRecoveryReason,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum EntryRemainderPosition {
    Supported(ManagedPositionContext),
    Unsupported(UnsupportedObservedState),
    CanonicallyFlat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryCancellation {
    Working,
    Pending,
    Refused,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryRemainderState {
    pub(super) pending_entry: PendingEntryState,
    pub(super) position: EntryRemainderPosition,
    pub(super) cancellation: EntryCancellation,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum EntryTerminalPositionTruth {
    ZeroFill,
    Supported(ManagedPositionContext),
    Unsupported(UnsupportedObservedState),
    CanonicallyFlat,
    Unresolved(EntryReconcileReason),
}

impl EntryRemainderState {
    fn position_matches(&self, position_id: PositionId, instrument_id: InstrumentId) -> bool {
        match &self.position {
            EntryRemainderPosition::Supported(position) => {
                position.position_id == position_id && position.instrument_id == instrument_id
            }
            EntryRemainderPosition::Unsupported(observed) => {
                observed.context.position_id == position_id
                    && observed.context.instrument_id == instrument_id
            }
            EntryRemainderPosition::CanonicallyFlat => {
                self.pending_entry.instrument_id == instrument_id
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EntryCancellationToken {
    client_order_id: ClientOrderId,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EntryCancellationEffect {
    None,
    Route {
        client_order_id: ClientOrderId,
        token: EntryCancellationToken,
    },
    AwaitingLifecycle,
    Refused,
}

#[derive(Debug, Clone, PartialEq)]
enum ExposureState {
    Flat,
    PendingEntry(PendingEntryArmState),
    EntryReconcilePending {
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    },
    EntryRemainder(Box<EntryRemainderState>),
    Managed(ManagedPositionContext),
    ExitAttempting(ExitAttemptingState),
    ExitPending(ExitPendingState),
    TerminalExitAwaitingPosition(ExitPendingState),
    UnsupportedObserved(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
}

impl ExposureState {
    fn entry_cancellation_effect(remainder: &mut EntryRemainderState) -> EntryCancellationEffect {
        match remainder.cancellation {
            EntryCancellation::Working => {
                remainder.cancellation = EntryCancellation::Pending;
                let client_order_id = remainder.pending_entry.client_order_id;
                EntryCancellationEffect::Route {
                    client_order_id,
                    token: EntryCancellationToken { client_order_id },
                }
            }
            EntryCancellation::Pending => EntryCancellationEffect::AwaitingLifecycle,
            EntryCancellation::Refused => EntryCancellationEffect::Refused,
        }
    }

    pub(super) fn pending_entry(&self) -> Option<&PendingEntryState> {
        match self {
            Self::PendingEntry(arm) => Some(&arm.pending),
            Self::EntryReconcilePending { pending, .. } => Some(pending),
            Self::EntryRemainder(remainder) => Some(&remainder.pending_entry),
            Self::Flat
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        match self {
            Self::PendingEntry(arm) => Some(&mut arm.pending),
            Self::EntryReconcilePending { pending, .. } => Some(pending),
            Self::EntryRemainder(remainder) => Some(&mut remainder.pending_entry),
            Self::Flat
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn entry_remainder(&self) -> Option<&EntryRemainderState> {
        match self {
            Self::EntryRemainder(remainder) => Some(remainder),
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    fn resolve_entry_terminal(
        self,
        client_order_id: ClientOrderId,
        truth: EntryTerminalPositionTruth,
    ) -> (Self, Option<PendingEntryState>) {
        match self {
            Self::PendingEntry(arm) if arm.pending.client_order_id == client_order_id => {
                Self::resolve_unmaterialized_entry_terminal(arm.pending, truth)
            }
            Self::EntryReconcilePending { pending, .. }
                if pending.client_order_id == client_order_id =>
            {
                Self::resolve_unmaterialized_entry_terminal(pending, truth)
            }
            Self::EntryRemainder(remainder)
                if remainder.pending_entry.client_order_id == client_order_id =>
            {
                let next = match truth {
                    EntryTerminalPositionTruth::Supported(position)
                        if remainder
                            .position_matches(position.position_id, position.instrument_id) =>
                    {
                        Some(Self::Managed(position))
                    }
                    EntryTerminalPositionTruth::Unsupported(observed)
                        if remainder.position_matches(
                            observed.context.position_id,
                            observed.context.instrument_id,
                        ) =>
                    {
                        Some(Self::UnsupportedObserved(observed))
                    }
                    EntryTerminalPositionTruth::CanonicallyFlat => Some(Self::Flat),
                    EntryTerminalPositionTruth::ZeroFill
                    | EntryTerminalPositionTruth::Supported(_)
                    | EntryTerminalPositionTruth::Unsupported(_)
                    | EntryTerminalPositionTruth::Unresolved(_) => None,
                };
                match next {
                    Some(next) => (next, Some(remainder.pending_entry)),
                    None => (Self::EntryRemainder(remainder), None),
                }
            }
            state @ (Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::EntryRemainder(_)
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_)) => (state, None),
        }
    }

    fn resolve_unmaterialized_entry_terminal(
        pending: PendingEntryState,
        truth: EntryTerminalPositionTruth,
    ) -> (Self, Option<PendingEntryState>) {
        match truth {
            EntryTerminalPositionTruth::ZeroFill => (Self::Flat, Some(pending)),
            EntryTerminalPositionTruth::Supported(position)
                if pending.instrument_id == position.instrument_id =>
            {
                (Self::Managed(position), Some(pending))
            }
            EntryTerminalPositionTruth::Unsupported(observed)
                if pending.instrument_id == observed.context.instrument_id =>
            {
                (Self::UnsupportedObserved(observed), Some(pending))
            }
            EntryTerminalPositionTruth::CanonicallyFlat => (
                Self::EntryReconcilePending {
                    pending,
                    reason: EntryReconcileReason::AwaitingPositionMaterialization,
                },
                None,
            ),
            EntryTerminalPositionTruth::Unresolved(reason) => {
                (Self::EntryReconcilePending { pending, reason }, None)
            }
            EntryTerminalPositionTruth::Supported(_)
            | EntryTerminalPositionTruth::Unsupported(_) => (
                Self::EntryReconcilePending {
                    pending,
                    reason: EntryReconcileReason::AwaitingPositionMaterialization,
                },
                None,
            ),
        }
    }

    pub(super) fn materialize_supported_position(
        &mut self,
        managed: ManagedPositionContext,
        keep_entry_remainder: bool,
        unlineaged_reason: BlindRecoveryReason,
    ) -> bool {
        let position_id = managed.position_id;
        let instrument_id = managed.instrument_id;
        let (next, materialized) = match self.clone() {
            Self::PendingEntry(PendingEntryArmState { pending, .. })
            | Self::EntryReconcilePending { pending, .. }
                if pending.instrument_id == instrument_id =>
            {
                let next = match keep_entry_remainder {
                    true => Self::EntryRemainder(Box::new(EntryRemainderState {
                        pending_entry: pending,
                        position: EntryRemainderPosition::Supported(managed),
                        cancellation: EntryCancellation::Working,
                    })),
                    false => Self::Managed(managed),
                };
                (next, true)
            }
            Self::PendingEntry(_) | Self::EntryReconcilePending { .. } => (
                Self::BlindRecovery(BlindRecoveryState {
                    reason: unlineaged_reason,
                }),
                false,
            ),
            Self::EntryRemainder(mut remainder) => {
                let same_position = match &remainder.position {
                    EntryRemainderPosition::Supported(position) => {
                        position.position_id == position_id
                            && position.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::Unsupported(observed) => {
                        observed.context.position_id == position_id
                            && observed.context.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::CanonicallyFlat => {
                        remainder.pending_entry.instrument_id == instrument_id
                    }
                };
                match same_position {
                    true => {
                        remainder.position = EntryRemainderPosition::Supported(managed);
                        (Self::EntryRemainder(remainder), true)
                    }
                    false => (
                        Self::BlindRecovery(BlindRecoveryState {
                            reason: unlineaged_reason,
                        }),
                        false,
                    ),
                }
            }
            Self::Managed(current)
                if current.position_id == position_id && current.instrument_id == instrument_id =>
            {
                (Self::Managed(managed), true)
            }
            Self::ExitAttempting(mut attempt)
                if attempt.authority.position_id() == position_id
                    && attempt.authority.instrument_id() == instrument_id =>
            {
                attempt.managed = managed;
                (Self::ExitAttempting(attempt), true)
            }
            Self::ExitPending(mut exit)
                if exit.authority.position_id() == position_id
                    && exit.authority.instrument_id() == instrument_id =>
            {
                exit.position = Some(managed);
                (Self::ExitPending(exit), true)
            }
            Self::TerminalExitAwaitingPosition(mut exit)
                if exit.authority.position_id() == position_id
                    && exit.authority.instrument_id() == instrument_id =>
            {
                exit.position = Some(managed);
                (Self::TerminalExitAwaitingPosition(exit), true)
            }
            Self::UnsupportedObserved(current)
                if current.context.position_id == position_id
                    && current.context.instrument_id == instrument_id =>
            {
                (Self::Managed(managed), true)
            }
            Self::BlindRecovery(recovery) => (Self::BlindRecovery(recovery), false),
            Self::Flat
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_) => (
                Self::BlindRecovery(BlindRecoveryState {
                    reason: unlineaged_reason,
                }),
                false,
            ),
        };
        *self = next;
        materialized
    }

    fn reduce_position_closed(
        self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> (Self, EntryCancellationEffect) {
        match self {
            Self::EntryRemainder(mut remainder) => {
                let position_matches = match &remainder.position {
                    EntryRemainderPosition::Supported(position) => {
                        position.position_id == position_id
                            && position.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::Unsupported(observed) => {
                        observed.context.position_id == position_id
                            && observed.context.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::CanonicallyFlat => false,
                };
                match position_matches {
                    true => {
                        remainder.position = EntryRemainderPosition::CanonicallyFlat;
                        let effect = Self::entry_cancellation_effect(&mut remainder);
                        (Self::EntryRemainder(remainder), effect)
                    }
                    false => (
                        Self::EntryRemainder(remainder),
                        EntryCancellationEffect::None,
                    ),
                }
            }
            Self::EntryReconcilePending { pending, .. }
                if pending.instrument_id == instrument_id =>
            {
                let mut remainder = EntryRemainderState {
                    pending_entry: pending,
                    position: EntryRemainderPosition::CanonicallyFlat,
                    cancellation: EntryCancellation::Working,
                };
                let effect = Self::entry_cancellation_effect(&mut remainder);
                (Self::EntryRemainder(Box::new(remainder)), effect)
            }
            Self::Managed(position)
                if position.position_id == position_id
                    && position.instrument_id == instrument_id =>
            {
                (Self::Flat, EntryCancellationEffect::None)
            }
            Self::UnsupportedObserved(observed)
                if observed.context.position_id == position_id
                    && observed.context.instrument_id == instrument_id =>
            {
                (Self::Flat, EntryCancellationEffect::None)
            }
            Self::Flat => (Self::Flat, EntryCancellationEffect::None),
            Self::PendingEntry(arm) => (Self::PendingEntry(arm), EntryCancellationEffect::None),
            Self::EntryReconcilePending { pending, reason } => (
                Self::EntryReconcilePending { pending, reason },
                EntryCancellationEffect::None,
            ),
            Self::Managed(position) => (Self::Managed(position), EntryCancellationEffect::None),
            Self::ExitAttempting(attempt) => {
                (Self::ExitAttempting(attempt), EntryCancellationEffect::None)
            }
            Self::ExitPending(exit) => (Self::ExitPending(exit), EntryCancellationEffect::None),
            Self::TerminalExitAwaitingPosition(exit) => (
                Self::TerminalExitAwaitingPosition(exit),
                EntryCancellationEffect::None,
            ),
            Self::UnsupportedObserved(observed) => (
                Self::UnsupportedObserved(observed),
                EntryCancellationEffect::None,
            ),
            Self::BlindRecovery(recovery) => {
                (Self::BlindRecovery(recovery), EntryCancellationEffect::None)
            }
        }
    }

    pub(super) fn request_entry_remainder_cancellation(&mut self) -> EntryCancellationEffect {
        match self {
            Self::EntryRemainder(remainder) => Self::entry_cancellation_effect(remainder),
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::Managed(_)
            | Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => EntryCancellationEffect::None,
        }
    }

    pub(super) fn settle_entry_cancellation(
        &mut self,
        token: EntryCancellationToken,
        settlement: EntryCancellationSettlement,
    ) {
        match (self, settlement) {
            (Self::EntryRemainder(remainder), EntryCancellationSettlement::RestoreWorking)
                if remainder.pending_entry.client_order_id == token.client_order_id
                    && remainder.cancellation == EntryCancellation::Pending =>
            {
                remainder.cancellation = EntryCancellation::Working;
            }
            (Self::EntryRemainder(remainder), EntryCancellationSettlement::RetainPending)
                if remainder.pending_entry.client_order_id == token.client_order_id
                    && remainder.cancellation == EntryCancellation::Pending => {}
            (
                Self::Flat
                | Self::PendingEntry(_)
                | Self::EntryReconcilePending { .. }
                | Self::EntryRemainder(_)
                | Self::Managed(_)
                | Self::ExitAttempting(_)
                | Self::ExitPending(_)
                | Self::TerminalExitAwaitingPosition(_)
                | Self::UnsupportedObserved(_)
                | Self::BlindRecovery(_),
                EntryCancellationSettlement::RestoreWorking
                | EntryCancellationSettlement::RetainPending,
            ) => {}
        }
    }

    pub(super) fn observe_cancel_rejected(&mut self, client_order_id: ClientOrderId) {
        let Self::EntryRemainder(remainder) = self else {
            return;
        };
        if remainder.pending_entry.client_order_id == client_order_id
            && remainder.cancellation == EntryCancellation::Pending
        {
            remainder.cancellation = EntryCancellation::Refused;
        }
    }

    pub(super) fn managed_position_context(&self) -> Option<&ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitAttempting(attempt) => Some(&attempt.managed),
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                exit.position.as_ref()
            }
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::EntryRemainder(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn managed_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitAttempting(attempt) => Some(&mut attempt.managed),
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                exit.position.as_mut()
            }
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::EntryRemainder(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn tracked_position_context(&self) -> Option<&ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitAttempting(attempt) => Some(&attempt.managed),
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                exit.position.as_ref()
            }
            Self::UnsupportedObserved(observed) => Some(&observed.context),
            Self::EntryRemainder(remainder) => match &remainder.position {
                EntryRemainderPosition::Supported(position) => Some(position),
                EntryRemainderPosition::Unsupported(observed) => Some(&observed.context),
                EntryRemainderPosition::CanonicallyFlat => None,
            },
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn tracked_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitAttempting(attempt) => Some(&mut attempt.managed),
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                exit.position.as_mut()
            }
            Self::UnsupportedObserved(observed) => Some(&mut observed.context),
            Self::EntryRemainder(remainder) => match &mut remainder.position {
                EntryRemainderPosition::Supported(position) => Some(position),
                EntryRemainderPosition::Unsupported(observed) => Some(&mut observed.context),
                EntryRemainderPosition::CanonicallyFlat => None,
            },
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.tracked_position_context()
            .map(|position| position.instrument_id)
            .or_else(|| self.pending_entry().map(|pending| pending.instrument_id))
    }

    pub(super) fn exit_pending_snapshot(&self) -> Option<ExitPendingState> {
        match self {
            Self::ExitAttempting(attempt) => Some(attempt.snapshot()),
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                Some(exit.clone())
            }
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::EntryRemainder(_)
            | Self::Managed(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn exit_lifecycle(&self) -> Option<(ExitLifecyclePhase, ExitPendingState)> {
        match self {
            Self::ExitAttempting(attempt) => {
                Some((ExitLifecyclePhase::Attempting, attempt.snapshot()))
            }
            Self::ExitPending(exit) => Some((ExitLifecyclePhase::Working, exit.clone())),
            Self::TerminalExitAwaitingPosition(exit) => {
                Some((ExitLifecyclePhase::TerminalAwaitingPosition, exit.clone()))
            }
            Self::Flat
            | Self::PendingEntry(_)
            | Self::EntryReconcilePending { .. }
            | Self::EntryRemainder(_)
            | Self::Managed(_)
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => None,
        }
    }

    pub(super) fn occupancy(&self) -> Option<ExposureOccupancy> {
        match self {
            Self::Flat => None,
            Self::PendingEntry(_) => Some(ExposureOccupancy::PendingEntry),
            Self::EntryReconcilePending { .. } => Some(ExposureOccupancy::EntryReconcilePending),
            Self::EntryRemainder(_) => Some(ExposureOccupancy::EntryRemainder),
            Self::Managed(_) => Some(ExposureOccupancy::ManagedPosition),
            Self::ExitAttempting(_)
            | Self::ExitPending(_)
            | Self::TerminalExitAwaitingPosition(_) => Some(ExposureOccupancy::ExitPending),
            Self::UnsupportedObserved(_) => Some(ExposureOccupancy::UnsupportedObserved),
            Self::BlindRecovery(_) => Some(ExposureOccupancy::BlindRecovery),
        }
    }

    pub(super) fn current_position_market_id(&self) -> Option<String> {
        self.tracked_position_context()
            .and_then(|position| position.lifecycle.market_id_owned())
            .or_else(|| {
                self.exit_pending_snapshot()
                    .and_then(|exit| exit.pending_exit.market_id)
            })
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureKind {
    Flat,
    PendingEntry,
    EntryReconcilePending,
    EntryRemainder,
    Managed,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitRouteAvailability {
    Managed,
    EntryRemainder,
    ExitPending,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureEntryGate {
    Open,
    Occupied(ExposureOccupancy),
    Recovering(ExposureOccupancy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureMarketDataRoute {
    Entry,
    Exit,
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EntryArmCapability {
    generation: u64,
    client_order_id: ClientOrderId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryArmError {
    Occupied(ExposureOccupancy),
    GenerationExhausted,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExitAttemptCapability {
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitAttemptSettlement {
    Retain,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryArmSettlement {
    Retain,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryCancellationSettlement {
    RestoreWorking,
    RetainPending,
}

#[derive(Debug)]
enum ExposureReleaseObservation {
    EntryArmAborted(EntryArmCapability),
    EntryTerminal {
        client_order_id: ClientOrderId,
        truth: Box<EntryTerminalPositionTruth>,
    },
    PositionClosed {
        position_id: PositionId,
        instrument_id: InstrumentId,
    },
    ExitFlat,
    Settlement,
    CancelRejected(ClientOrderId),
}

#[derive(Debug)]
struct ExposureReleaseEffects {
    retired_entry: Option<PendingEntryState>,
    cancellation: EntryCancellationEffect,
}

impl ExposureReleaseEffects {
    fn none() -> Self {
        Self {
            retired_entry: None,
            cancellation: EntryCancellationEffect::None,
        }
    }
}

#[derive(Debug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub(super) struct ExposureOwner {
    state: ExposureState,
    next_entry_generation: u64,
    next_exit_generation: u64,
}

impl ExposureOwner {
    pub(super) const fn new() -> Self {
        Self {
            state: ExposureState::Flat,
            next_entry_generation: 0,
            next_exit_generation: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn kind(&self) -> ExposureKind {
        match self.state {
            ExposureState::Flat => ExposureKind::Flat,
            ExposureState::PendingEntry(_) => ExposureKind::PendingEntry,
            ExposureState::EntryReconcilePending { .. } => ExposureKind::EntryReconcilePending,
            ExposureState::EntryRemainder(_) => ExposureKind::EntryRemainder,
            ExposureState::Managed(_) => ExposureKind::Managed,
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_) => ExposureKind::ExitPending,
            ExposureState::UnsupportedObserved(_) => ExposureKind::UnsupportedObserved,
            ExposureState::BlindRecovery(_) => ExposureKind::BlindRecovery,
        }
    }

    pub(super) fn lifecycle_outcome(&self) -> OrderLifecycleOutcome {
        match self.state {
            ExposureState::Flat => OrderLifecycleOutcome::Flat,
            ExposureState::PendingEntry(_) | ExposureState::EntryRemainder(_) => {
                OrderLifecycleOutcome::PendingEntry
            }
            ExposureState::EntryReconcilePending { .. } => {
                OrderLifecycleOutcome::EntryReconcilePending
            }
            ExposureState::Managed(_) => OrderLifecycleOutcome::Managed,
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_) => OrderLifecycleOutcome::ExitPending,
            ExposureState::UnsupportedObserved(_) => OrderLifecycleOutcome::UnsupportedObserved,
            ExposureState::BlindRecovery(_) => OrderLifecycleOutcome::BlindRecovery,
        }
    }

    #[cfg(test)]
    pub(super) fn occupancy(&self) -> Option<ExposureOccupancy> {
        self.state.occupancy()
    }

    pub(super) fn pending_entry(&self) -> Option<&PendingEntryState> {
        self.state.pending_entry()
    }

    pub(super) fn pending_entry_arm(&self) -> Option<&PendingEntryState> {
        match &self.state {
            ExposureState::PendingEntry(arm) => Some(&arm.pending),
            ExposureState::Flat
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::EntryRemainder(_)
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => None,
        }
    }

    pub(super) fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        self.state.pending_entry_mut()
    }

    pub(super) fn entry_remainder(&self) -> Option<&EntryRemainderState> {
        self.state.entry_remainder()
    }

    pub(super) fn managed_position_context(&self) -> Option<&ManagedPositionContext> {
        self.state.managed_position_context()
    }

    pub(super) fn tracked_position_context(&self) -> Option<&ManagedPositionContext> {
        self.state.tracked_position_context()
    }

    pub(super) fn tracked_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        self.state.tracked_position_context_mut()
    }

    pub(super) fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.state.held_instrument_id()
    }

    pub(super) fn exit_pending_snapshot(&self) -> Option<ExitPendingState> {
        self.state.exit_pending_snapshot()
    }

    pub(super) fn exit_lifecycle(&self) -> Option<(ExitLifecyclePhase, ExitPendingState)> {
        self.state.exit_lifecycle()
    }

    pub(super) fn current_position_market_id(&self) -> Option<String> {
        self.state.current_position_market_id()
    }

    #[cfg(test)]
    pub(super) fn is_flat(&self) -> bool {
        self.kind() == ExposureKind::Flat
    }

    #[cfg(test)]
    pub(super) fn is_managed(&self) -> bool {
        self.kind() == ExposureKind::Managed
    }

    pub(super) fn is_entry_reconcile_pending(&self) -> bool {
        matches!(&self.state, ExposureState::EntryReconcilePending { .. })
    }

    pub(super) fn is_blind_recovery(&self) -> bool {
        matches!(&self.state, ExposureState::BlindRecovery(_))
    }

    pub(super) fn exit_route_availability(&self) -> ExitRouteAvailability {
        match self.state {
            ExposureState::Managed(_) => ExitRouteAvailability::Managed,
            ExposureState::EntryRemainder(_) => ExitRouteAvailability::EntryRemainder,
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_) => ExitRouteAvailability::ExitPending,
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => ExitRouteAvailability::Unavailable,
        }
    }

    pub(super) fn entry_gate(&self) -> ExposureEntryGate {
        match &self.state {
            ExposureState::Flat => ExposureEntryGate::Open,
            ExposureState::PendingEntry(_) => {
                ExposureEntryGate::Occupied(ExposureOccupancy::PendingEntry)
            }
            ExposureState::EntryReconcilePending { .. } => {
                ExposureEntryGate::Recovering(ExposureOccupancy::EntryReconcilePending)
            }
            ExposureState::EntryRemainder(remainder) => match remainder.position {
                EntryRemainderPosition::Supported(_) => {
                    ExposureEntryGate::Occupied(ExposureOccupancy::EntryRemainder)
                }
                EntryRemainderPosition::Unsupported(_)
                | EntryRemainderPosition::CanonicallyFlat => {
                    ExposureEntryGate::Recovering(ExposureOccupancy::EntryRemainder)
                }
            },
            ExposureState::Managed(_) => {
                ExposureEntryGate::Occupied(ExposureOccupancy::ManagedPosition)
            }
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_) => {
                ExposureEntryGate::Occupied(ExposureOccupancy::ExitPending)
            }
            ExposureState::UnsupportedObserved(_) => {
                ExposureEntryGate::Recovering(ExposureOccupancy::UnsupportedObserved)
            }
            ExposureState::BlindRecovery(_) => {
                ExposureEntryGate::Recovering(ExposureOccupancy::BlindRecovery)
            }
        }
    }

    pub(super) fn market_data_route(&self) -> ExposureMarketDataRoute {
        match self.state {
            ExposureState::Flat => ExposureMarketDataRoute::Entry,
            ExposureState::Managed(_) => ExposureMarketDataRoute::Exit,
            ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::EntryRemainder(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => ExposureMarketDataRoute::None,
        }
    }

    pub(super) fn terminal_exit_snapshot(&self) -> Option<ExitPendingState> {
        match &self.state {
            ExposureState::TerminalExitAwaitingPosition(exit) => Some(exit.clone()),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::EntryRemainder(_)
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => None,
        }
    }

    fn reduce_release(
        &mut self,
        observation: ExposureReleaseObservation,
    ) -> ExposureReleaseEffects {
        let prior = self.state.clone();
        let (next, effects) = match observation {
            ExposureReleaseObservation::EntryArmAborted(capability) => {
                let exact_generation = matches!(
                    &prior,
                    ExposureState::PendingEntry(arm)
                        if arm.generation == capability.generation
                            && arm.pending.client_order_id == capability.client_order_id
                );
                let next = match exact_generation {
                    true => ExposureState::Flat,
                    false => prior,
                };
                (next, ExposureReleaseEffects::none())
            }
            ExposureReleaseObservation::EntryTerminal {
                client_order_id,
                truth,
            } => {
                let (next, retired_entry) = prior.resolve_entry_terminal(client_order_id, *truth);
                (
                    next,
                    ExposureReleaseEffects {
                        retired_entry,
                        ..ExposureReleaseEffects::none()
                    },
                )
            }
            ExposureReleaseObservation::PositionClosed {
                position_id,
                instrument_id,
            } => {
                let (next, cancellation) = prior.reduce_position_closed(position_id, instrument_id);
                (
                    next,
                    ExposureReleaseEffects {
                        cancellation,
                        ..ExposureReleaseEffects::none()
                    },
                )
            }
            ExposureReleaseObservation::ExitFlat => {
                let next = match prior {
                    ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_) => ExposureState::Flat,
                    state @ (ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::EntryRemainder(_)
                    | ExposureState::Managed(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::BlindRecovery(_)) => state,
                };
                (next, ExposureReleaseEffects::none())
            }
            ExposureReleaseObservation::Settlement => match prior {
                ExposureState::EntryRemainder(mut remainder) => {
                    remainder.position = EntryRemainderPosition::CanonicallyFlat;
                    let cancellation = ExposureState::entry_cancellation_effect(&mut remainder);
                    (
                        ExposureState::EntryRemainder(remainder),
                        ExposureReleaseEffects {
                            cancellation,
                            ..ExposureReleaseEffects::none()
                        },
                    )
                }
                ExposureState::Managed(_) | ExposureState::UnsupportedObserved(_) => {
                    (ExposureState::Flat, ExposureReleaseEffects::none())
                }
                state @ (ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::BlindRecovery(_)) => (state, ExposureReleaseEffects::none()),
            },
            ExposureReleaseObservation::CancelRejected(client_order_id) => {
                let mut next = prior;
                next.observe_cancel_rejected(client_order_id);
                (next, ExposureReleaseEffects::none())
            }
        };
        self.state = next;
        effects
    }

    pub(super) fn arm_entry(
        &mut self,
        pending: PendingEntryState,
    ) -> Result<EntryArmCapability, EntryArmError> {
        if let Some(occupancy) = self.state.occupancy() {
            return Err(EntryArmError::Occupied(occupancy));
        }
        let Some(generation) = self.next_entry_generation.checked_add(1) else {
            return Err(EntryArmError::GenerationExhausted);
        };
        self.next_entry_generation = generation;
        let client_order_id = pending.client_order_id;
        self.state = ExposureState::PendingEntry(PendingEntryArmState {
            generation,
            pending,
        });
        Ok(EntryArmCapability {
            generation,
            client_order_id,
        })
    }

    pub(super) fn settle_entry_arm(
        &mut self,
        capability: EntryArmCapability,
        settlement: EntryArmSettlement,
    ) {
        match settlement {
            EntryArmSettlement::Abort => {
                self.reduce_release(ExposureReleaseObservation::EntryArmAborted(capability));
            }
            EntryArmSettlement::Retain => {}
        }
    }

    pub(super) fn enter_entry_reconcile(
        &mut self,
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    ) {
        self.state = ExposureState::EntryReconcilePending { pending, reason };
    }

    pub(super) fn reclassify_pending_entry(
        &mut self,
        reason: EntryReconcileReason,
    ) -> Option<PendingEntryState> {
        let ExposureState::PendingEntry(arm) = self.state.clone() else {
            return None;
        };
        self.state = ExposureState::EntryReconcilePending {
            pending: arm.pending.clone(),
            reason,
        };
        Some(arm.pending)
    }

    pub(super) fn enter_blind_recovery(&mut self, reason: BlindRecoveryReason) {
        self.state = ExposureState::BlindRecovery(BlindRecoveryState { reason });
    }

    pub(super) fn observe_clean_start(&mut self) {
        self.state = ExposureState::Flat;
    }

    pub(super) fn observe_entry_terminal(
        &mut self,
        client_order_id: ClientOrderId,
        truth: EntryTerminalPositionTruth,
    ) -> Option<PendingEntryState> {
        self.reduce_release(ExposureReleaseObservation::EntryTerminal {
            client_order_id,
            truth: Box::new(truth),
        })
        .retired_entry
    }

    pub(super) fn materialize_supported_position(
        &mut self,
        managed: ManagedPositionContext,
        keep_entry_remainder: bool,
        unlineaged_reason: BlindRecoveryReason,
    ) -> bool {
        self.state
            .materialize_supported_position(managed, keep_entry_remainder, unlineaged_reason)
    }

    pub(super) fn set_unsupported_observed(
        &mut self,
        unsupported: UnsupportedObservedState,
        keep_entry_remainder: bool,
        unlineaged_reason: BlindRecoveryReason,
    ) {
        let position_id = unsupported.context.position_id;
        let instrument_id = unsupported.context.instrument_id;
        let next = match self.state.clone() {
            ExposureState::PendingEntry(PendingEntryArmState { pending, .. })
            | ExposureState::EntryReconcilePending { pending, .. }
                if pending.instrument_id == instrument_id && keep_entry_remainder =>
            {
                ExposureState::EntryRemainder(Box::new(EntryRemainderState {
                    pending_entry: pending,
                    position: EntryRemainderPosition::Unsupported(unsupported),
                    cancellation: EntryCancellation::Working,
                }))
            }
            ExposureState::PendingEntry(PendingEntryArmState { pending, .. })
            | ExposureState::EntryReconcilePending { pending, .. }
                if pending.instrument_id == instrument_id =>
            {
                ExposureState::UnsupportedObserved(unsupported)
            }
            ExposureState::EntryRemainder(mut remainder) => {
                let same_position = match &remainder.position {
                    EntryRemainderPosition::Supported(position) => {
                        position.position_id == position_id
                            && position.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::Unsupported(observed) => {
                        observed.context.position_id == position_id
                            && observed.context.instrument_id == instrument_id
                    }
                    EntryRemainderPosition::CanonicallyFlat => {
                        remainder.pending_entry.instrument_id == instrument_id
                    }
                };
                match (same_position, keep_entry_remainder) {
                    (true, true) => {
                        remainder.position = EntryRemainderPosition::Unsupported(unsupported);
                        ExposureState::EntryRemainder(remainder)
                    }
                    (true, false) => ExposureState::UnsupportedObserved(unsupported),
                    (false, _) => ExposureState::BlindRecovery(BlindRecoveryState {
                        reason: unlineaged_reason,
                    }),
                }
            }
            ExposureState::Managed(current)
                if current.position_id == position_id && current.instrument_id == instrument_id =>
            {
                ExposureState::UnsupportedObserved(unsupported)
            }
            ExposureState::UnsupportedObserved(current)
                if current.context.position_id == position_id
                    && current.context.instrument_id == instrument_id =>
            {
                ExposureState::UnsupportedObserved(unsupported)
            }
            ExposureState::ExitAttempting(attempt)
                if attempt.authority.position_id() == position_id
                    && attempt.authority.instrument_id() == instrument_id =>
            {
                ExposureState::ExitAttempting(attempt)
            }
            ExposureState::ExitPending(exit)
                if exit.authority.position_id() == position_id
                    && exit.authority.instrument_id() == instrument_id =>
            {
                ExposureState::ExitPending(exit)
            }
            ExposureState::TerminalExitAwaitingPosition(exit)
                if exit.authority.position_id() == position_id
                    && exit.authority.instrument_id() == instrument_id =>
            {
                ExposureState::TerminalExitAwaitingPosition(exit)
            }
            ExposureState::BlindRecovery(recovery) => ExposureState::BlindRecovery(recovery),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::UnsupportedObserved(_) => {
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: unlineaged_reason,
                })
            }
        };
        self.state = next;
    }

    pub(super) fn observe_position_closed(
        &mut self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> EntryCancellationEffect {
        self.reduce_release(ExposureReleaseObservation::PositionClosed {
            position_id,
            instrument_id,
        })
        .cancellation
    }

    pub(super) fn request_entry_remainder_cancellation(&mut self) -> EntryCancellationEffect {
        self.state.request_entry_remainder_cancellation()
    }

    pub(super) fn settle_entry_cancellation(
        &mut self,
        token: EntryCancellationToken,
        settlement: EntryCancellationSettlement,
    ) {
        self.state.settle_entry_cancellation(token, settlement);
    }

    pub(super) fn observe_cancel_rejected(&mut self, client_order_id: ClientOrderId) {
        self.reduce_release(ExposureReleaseObservation::CancelRejected(client_order_id));
    }

    pub(super) fn begin_exit(
        &mut self,
        pending_exit: PendingExitState,
        authority: BoltV3ExitOrderAuthorityHandle,
    ) -> anyhow::Result<ExitAttemptCapability> {
        let ExposureState::Managed(managed) = self.state.clone() else {
            anyhow::bail!("exit attempt requires managed exposure");
        };
        let generation = self
            .next_exit_generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("exit attempt generation overflow"))?;
        self.next_exit_generation = generation;
        self.state = ExposureState::ExitAttempting(ExitAttemptingState {
            generation,
            managed,
            pending_exit,
            authority,
        });
        Ok(ExitAttemptCapability { generation })
    }

    pub(super) fn settle_exit_attempt(
        &mut self,
        capability: ExitAttemptCapability,
        settlement: ExitAttemptSettlement,
    ) {
        let transition = match (&self.state, settlement) {
            (ExposureState::ExitAttempting(attempt), ExitAttemptSettlement::Retain)
                if attempt.generation == capability.generation =>
            {
                Some(ExposureState::ExitPending(attempt.clone().into_pending()))
            }
            (ExposureState::ExitAttempting(attempt), ExitAttemptSettlement::Abort)
                if attempt.generation == capability.generation =>
            {
                Some(ExposureState::Managed(attempt.managed.clone()))
            }
            (
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::EntryRemainder(_)
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_),
                ExitAttemptSettlement::Retain | ExitAttemptSettlement::Abort,
            ) => None,
        };
        let Some(next) = transition else {
            return;
        };
        self.state = next;
    }

    pub(super) fn observe_exit_working(&mut self, exit: ExitPendingState) {
        self.state = ExposureState::ExitPending(exit);
    }

    pub(super) fn observe_exit_terminal(&mut self, exit: ExitPendingState) {
        self.state = ExposureState::TerminalExitAwaitingPosition(exit);
    }

    pub(super) fn observe_exit_residual(&mut self, managed: ManagedPositionContext) {
        self.state = ExposureState::Managed(managed);
    }

    pub(super) fn observe_exit_flat(&mut self) {
        self.reduce_release(ExposureReleaseObservation::ExitFlat);
    }

    pub(super) fn observe_settlement(&mut self) -> EntryCancellationEffect {
        self.reduce_release(ExposureReleaseObservation::Settlement)
            .cancellation
    }

    #[cfg(test)]
    pub(super) fn set_pending_entry_for_test(&mut self, pending: PendingEntryState) {
        let generation = self
            .next_entry_generation
            .checked_add(1)
            .expect("test entry generation must remain available");
        self.next_entry_generation = generation;
        self.state = ExposureState::PendingEntry(PendingEntryArmState {
            generation,
            pending,
        });
    }

    #[cfg(test)]
    pub(super) fn set_entry_reconcile_for_test(
        &mut self,
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    ) {
        self.state = ExposureState::EntryReconcilePending { pending, reason };
    }

    #[cfg(test)]
    pub(super) fn set_managed_for_test(&mut self, managed: ManagedPositionContext) {
        self.state = ExposureState::Managed(managed);
    }

    #[cfg(test)]
    pub(super) fn set_entry_remainder_for_test(
        &mut self,
        pending_entry: PendingEntryState,
        position: EntryRemainderPosition,
    ) {
        self.state = ExposureState::EntryRemainder(Box::new(EntryRemainderState {
            pending_entry,
            position,
            cancellation: EntryCancellation::Working,
        }));
    }

    #[cfg(test)]
    pub(super) fn set_exit_pending_for_test(&mut self, exit: ExitPendingState) {
        self.state = ExposureState::ExitPending(exit);
    }

    #[cfg(test)]
    pub(super) fn set_terminal_exit_for_test(&mut self, exit: ExitPendingState) {
        self.state = ExposureState::TerminalExitAwaitingPosition(exit);
    }

    #[cfg(test)]
    pub(super) fn set_blind_recovery_for_test(&mut self, reason: BlindRecoveryReason) {
        self.enter_blind_recovery(reason);
    }

    #[cfg(test)]
    pub(super) fn set_unsupported_for_test(&mut self, observed: UnsupportedObservedState) {
        self.state = ExposureState::UnsupportedObserved(observed);
    }

    #[cfg(test)]
    pub(super) fn set_flat_for_test(&mut self) {
        self.state = ExposureState::Flat;
    }

    #[cfg(test)]
    pub(super) fn managed_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        self.state.managed_position_context_mut()
    }

    #[cfg(test)]
    pub(super) fn blind_recovery_reason(&self) -> Option<&BlindRecoveryReason> {
        match &self.state {
            ExposureState::BlindRecovery(recovery) => Some(&recovery.reason),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::EntryRemainder(_)
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::UnsupportedObserved(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn unsupported_observation(&self) -> Option<&UnsupportedObservedState> {
        match &self.state {
            ExposureState::UnsupportedObserved(observed) => Some(observed),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::EntryRemainder(_)
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::BlindRecovery(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn entry_reconcile_for_test(
        &self,
    ) -> Option<(&PendingEntryState, &EntryReconcileReason)> {
        match &self.state {
            ExposureState::EntryReconcilePending { pending, reason } => Some((pending, reason)),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryRemainder(_)
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_next_exit_generation_for_test(&mut self, generation: u64) {
        self.next_exit_generation = generation;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConfiguredPositionContract {
    pub(super) entry_order_side: OrderSide,
    pub(super) entry_position_side: PositionSide,
    pub(super) exit_order_side: OrderSide,
    pub(super) exit_position_side: PositionSide,
}

pub(super) fn supports_strategy_managed_position(
    entry_order_side: OrderSide,
    side: PositionSide,
    contract: ConfiguredPositionContract,
) -> bool {
    supports_strategy_position_contract(contract)
        && entry_order_side == contract.entry_order_side
        && side == contract.entry_position_side
        && is_observed_open_side(side)
}

pub(super) fn supports_strategy_position_contract(contract: ConfiguredPositionContract) -> bool {
    if contract.entry_position_side == PositionSide::Short
        || contract.exit_position_side == PositionSide::Short
    {
        return false;
    }
    expected_position_side_for_entry_order(contract.entry_order_side)
        .is_some_and(|side| side == contract.entry_position_side)
        && expected_exit_order_side_for_position(contract.exit_position_side)
            .is_some_and(|side| side == contract.exit_order_side)
        && contract.entry_position_side == contract.exit_position_side
        && is_observed_open_side(contract.entry_position_side)
}

pub(super) fn infer_strategy_position_side_from_entry_fill(
    entry_order_side: OrderSide,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<PositionSide> {
    (entry_order_side == configured_entry_order_side).then_some(configured_position_side)
}

pub(super) fn managed_position_effective_entry_cost(
    position: &OpenPositionState,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<f64> {
    (position.entry_order_side == configured_entry_order_side
        && position.side == configured_position_side)
        .then_some(position.avg_px_open)
        .filter(|effective_cost| is_positive_finite(*effective_cost))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureOccupancy {
    PendingEntry,
    EntryReconcilePending,
    EntryRemainder,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}
