use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};

use anyhow::Result;
use nautilus_core::UUID4;

use nautilus_model::{
    enums::{OrderSide, PositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId, TradeId, Venue},
    types::Quantity,
};

use crate::{
    bolt_v3_book_sizing::OutcomeBookState,
    bolt_v3_config::ExposureObligationLimits,
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_order_execution::{
        BoltV3ExitAuthorityRecoveryHandle, BoltV3ExitOrderAuthorityHandle,
        BoltV3ExitOrderCorrection, BoltV3PositionEpisodeFingerprint, BoltV3RecoveredExitCause,
        BoltV3RouteAttemptCompletion, BoltV3RouteAttemptParticipant,
    },
    bolt_v3_position_contract::{
        BoltV3PositionMarketLifecycle, expected_exit_order_side_for_position,
        expected_position_side_for_entry_order, is_observed_open_side,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ExposureStateKind {
    Flat,
    PendingEntry,
    EntryReconcilePending,
    Managed,
    ExitAttempting,
    ExitPending,
    TerminalExitAwaitingPosition,
    ExitAuthorityRecoveryHold,
    UnsupportedObserved,
    BlindRecovery,
    OperationSinkUnknown,
    ObligationSaturated,
    ReplacementConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureTransitionOutcome {
    Applied {
        from: ExposureStateKind,
        to: ExposureStateKind,
    },
    Preserved {
        state: ExposureStateKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureOperationKind {
    EntryRoute,
    ExitRoute,
    Bootstrap,
    Recovery,
    Correction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExposureOperationBlockedReason {
    Unoccupied,
    PendingEntryOccupied,
    EntryReconcileOccupied,
    ManagedOccupied,
    ExitAttemptOccupied,
    ExitPendingOccupied,
    RecoveryHoldOccupied,
    UnsupportedOccupied,
    BlindRecoveryOccupied,
    ReplacementConflictOccupied,
    SinkUnknownOccupied,
    ObligationSaturated,
    OperationAlreadyArmed,
    StaleGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExposureOperationRejection {
    pub(super) operation: ExposureOperationKind,
    pub(super) reason: ExposureOperationBlockedReason,
    pub(super) state: ExposureStateKind,
    pub(super) requested_generation: u64,
    pub(super) current_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExposureOperationDecision {
    pub(super) generation: u64,
    pub(super) rejection: Option<ExposureOperationBlockedReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum EntryLifecycleEvent {
    #[cfg(test)]
    RestorePending(PendingEntryState),
    Reconcile {
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    },
    ReleaseFlat,
    ClearManagedPending {
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    },
    RefreshPending(PendingEntryState),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExitLifecycleEvent {
    Pending(ExitPendingState),
    Working {
        expected_generation: u64,
        observation: ExitWorkingObservation,
        pending: ExitPendingState,
    },
    TerminalAwaitingPosition(ExitPendingState),
    RecoveryHold(ExitAuthorityRecoveryHoldState),
    RefreshAuthority(BoltV3ExitOrderAuthorityHandle),
    Residual(ManagedPositionContext),
    ReleaseFlat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitWorkingObservation {
    Lifecycle,
    Correction,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PositionTruthEvent {
    EntryTerminalMaterialization {
        client_order_id: ClientOrderId,
        managed: ManagedPositionContext,
    },
    RefreshContext(ManagedPositionContext),
    Unsupported(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum AdoptionCapablePositionTruthEvent {
    Canonical(CanonicalPositionProjection),
    AuthorizedRecovery(FreshCanonicalPositionProjection),
    AuthenticatedEpisodeRebase {
        before: PositionEpisodeFingerprint,
        authenticated_order_id: ClientOrderId,
        authenticated_fill_id: TradeId,
        rebased: Option<Box<ManagedPositionContext>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum CanonicalPositionProjection {
    None,
    ExactlyOne(Box<ManagedPositionContext>),
    Multiple {
        count: usize,
        recovery: BlindRecoveryState,
    },
    ProbeFailed {
        diagnostic: String,
        recovery: BlindRecoveryState,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PositionClosedEvent {
    ObservedWithFreshProjection {
        expected_generation: u64,
        episode: PositionEpisodeFingerprint,
        projection: FreshCanonicalPositionProjection,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum FreshCanonicalPositionProjection {
    None,
    ExactlyOne(Box<ClassifiedOpenPosition>),
    Multiple { count: usize },
    ProbeFailed { diagnostic: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ClassifiedOpenPosition {
    Managed(ManagedPositionContext),
    Unsupported(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum TimerReconciliationEvent {
    Pending(ExitPendingState),
    TerminalAwaitingPosition(ExitPendingState),
    RecoveryHold(ExitAuthorityRecoveryHoldState),
    ReleaseFlat,
    BlindRecovery(BlindRecoveryState),
    SinkUnknown(SinkUnknownResolution),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum SinkUnknownResolution {
    Submitted,
    Terminal {
        residual: Option<ManagedPositionContext>,
    },
    Filled {
        managed: ManagedPositionContext,
    },
    ProvenAbsent,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum BootstrapAdoptionEvent {
    Flat,
    Managed(ManagedPositionContext),
    Unsupported(UnsupportedObservedState),
    ExitPending(ExitPendingState),
    BlindRecovery(BlindRecoveryState),
}

impl From<ClassifiedOpenPosition> for BootstrapAdoptionEvent {
    fn from(classified: ClassifiedOpenPosition) -> Self {
        match classified {
            ClassifiedOpenPosition::Managed(managed) => Self::Managed(managed),
            ClassifiedOpenPosition::Unsupported(unsupported) => Self::Unsupported(unsupported),
            ClassifiedOpenPosition::BlindRecovery(recovery) => Self::BlindRecovery(recovery),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum SettlementEffectEvent {
    ReleaseFlat { episode: PositionEpisodeFingerprint },
    BlindRecovery(BlindRecoveryState),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExposureEvent {
    EntryLifecycle(EntryLifecycleEvent),
    ExitLifecycle(ExitLifecycleEvent),
    UntrackedOrder(UntrackedOrderEvent),
    PositionTruth(PositionTruthEvent),
    TimerReconciliation(TimerReconciliationEvent),
    BootstrapAdoption(BootstrapAdoptionEvent),
    SettlementEffect(SettlementEffectEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum AdoptionCapableExposureEvent {
    PositionTruth(AdoptionCapablePositionTruthEvent),
    PositionClosed(PositionClosedEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum UntrackedOrderEvent {
    Quarantine {
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    },
    HistoricalExitCorrection(HistoricalExitCorrection),
    HistoricalExitObservation(HistoricalExitObservation),
    ResolveHistoricalExitCorrection {
        client_order_id: ClientOrderId,
    },
    ObligationSaturated {
        client_order_id: ClientOrderId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HistoricalExitCorrection {
    pub(super) client_order_id: ClientOrderId,
    pub(super) instrument_id: InstrumentId,
    pub(super) trade_id: TradeId,
    pub(super) voided_quantity: Quantity,
    pub(super) ts_event_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum HistoricalExitObservationKey {
    Fill(TradeId),
    Lifecycle { ts_event_ns: u64, terminal: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct HistoricalExitObservation {
    pub(super) client_order_id: ClientOrderId,
    pub(super) instrument_id: InstrumentId,
    pub(super) key: HistoricalExitObservationKey,
    pub(super) observation: ExitRecoveryObservation,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReleasedExitProvenance {
    pub(super) client_order_id: ClientOrderId,
    pub(super) episode: PositionEpisodeFingerprint,
    pub(super) position: ManagedPositionContext,
    pub(super) observed_fill_ids: BTreeSet<TradeId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct DeferredExitObligation {
    pub(super) provenance: ReleasedExitProvenance,
    pub(super) history: BTreeMap<TradeId, HistoricalExitCorrection>,
    pub(super) observations: BTreeMap<HistoricalExitObservationKey, ExitRecoveryObservation>,
}

pub(super) type PositionEpisodeFingerprint = BoltV3PositionEpisodeFingerprint;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OpenPositionState {
    pub(super) episode: PositionEpisodeFingerprint,
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
pub(super) struct PendingExitState {
    pub(super) submitted_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedPositionOrigin {
    StrategyEntry,
    RecoveryBootstrap,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PositionReplayFragmentIdentity {
    pub(super) event_id: UUID4,
    pub(super) causation_id: Option<UUID4>,
    pub(super) client_order_id: ClientOrderId,
    pub(super) trade_id: TradeId,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionContext {
    pub(super) episode: PositionEpisodeFingerprint,
    pub(super) episode_fill_ids: BTreeSet<TradeId>,
    pub(super) replay_segment: Vec<PositionReplayFragmentIdentity>,
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) position_id: PositionId,
    pub(super) book: OutcomeBookState,
    pub(super) origin: ManagedPositionOrigin,
    pub(super) pending_entry: Option<PendingEntryState>,
    pub(super) episode_close_seen: bool,
    pub(super) canonical_none_seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionState {
    pub(super) position: OpenPositionState,
    pub(super) origin: ManagedPositionOrigin,
    pub(super) pending_entry: Option<PendingEntryState>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitPendingState {
    pub(super) position: ManagedPositionContext,
    pub(super) pending_exit: PendingExitState,
    pub(super) authority: BoltV3ExitOrderAuthorityHandle,
}

impl ExitPendingState {
    pub(super) fn client_order_id(&self) -> ClientOrderId {
        self.authority.client_order_id()
    }

    pub(super) fn instrument_id(&self) -> InstrumentId {
        self.authority.instrument_id()
    }

    pub(super) fn position_id(&self) -> PositionId {
        self.authority.position_id()
    }

    pub(super) fn episode(&self) -> PositionEpisodeFingerprint {
        self.authority.episode()
    }

    pub(super) fn market_id(&self) -> Option<String> {
        self.position.lifecycle.market_id_owned()
    }
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
            position: self.managed.clone(),
            pending_exit: self.pending_exit.clone(),
            authority: self.authority.clone(),
        }
    }

    pub(super) fn into_pending(self) -> ExitPendingState {
        ExitPendingState {
            position: self.managed,
            pending_exit: self.pending_exit,
            authority: self.authority,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitAuthorityRecoveryHoldState {
    pub(super) position: ManagedPositionContext,
    pub(super) pending_exit: PendingExitState,
    pub(super) plan: ExitAuthorityRecoveryPlan,
    pub(super) flat_recovery: ExitAuthorityFlatRecovery,
    pub(super) observations: BTreeMap<HistoricalExitObservationKey, ExitRecoveryObservation>,
}

impl ExitAuthorityRecoveryHoldState {
    pub(super) fn client_order_id(&self) -> ClientOrderId {
        self.plan.client_order_id()
    }

    pub(super) fn instrument_id(&self) -> InstrumentId {
        self.position.episode.instrument_id
    }

    pub(super) fn position_id(&self) -> PositionId {
        self.position.episode.position_id
    }

    pub(super) fn episode(&self) -> PositionEpisodeFingerprint {
        self.position.episode.clone()
    }

    pub(super) fn market_id(&self) -> Option<String> {
        self.position.lifecycle.market_id_owned()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExitAuthorityRecoveryPlan {
    Reconstruct {
        cause: BoltV3RecoveredExitCause,
        client_order_id: ClientOrderId,
    },
    Resume(BoltV3ExitOrderAuthorityHandle),
}

impl ExitAuthorityRecoveryPlan {
    pub(super) fn client_order_id(&self) -> ClientOrderId {
        match self {
            Self::Reconstruct {
                client_order_id, ..
            } => *client_order_id,
            Self::Resume(authority) => authority.client_order_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitRecoveryObservation {
    pub(super) ts_event_ns: u64,
    pub(super) trade_ids: BTreeSet<TradeId>,
    pub(super) effective_filled_quantity: Quantity,
    pub(super) terminal: bool,
    pub(super) correction: BoltV3ExitOrderCorrection,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExitAuthorityFlatRecovery {
    AwaitingLease,
    Armed(BoltV3ExitAuthorityRecoveryHandle),
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
    BootstrappedUnsupportedContract,
    LiveUnsupportedContract,
}

#[cfg(test)]
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
    DivergentUnsupportedPosition,
    SettlementEvidenceRecoveryFailed,
    AmbiguousRestartOpenExitOrders {
        instrument_id: InstrumentId,
        count: usize,
    },
    UnattributedRestartOpenExitOrder {
        instrument_id: InstrumentId,
    },
    ForeignVenuePosition {
        instrument_id: InstrumentId,
        instrument_venue: Venue,
        execution_venue: Venue,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct UnsupportedObservedState {
    pub(super) context: ManagedPositionContext,
    pub(super) reason: UnsupportedObservedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlindRecoveryProbeReason {
    CacheProbeFailed,
    MultipleOpenPositions { count: usize },
    SettlementEvidenceRecoveryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlindRecoveryIdentityReason {
    InvalidBootstrap {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
    InvalidLive {
        entry_order_side: OrderSide,
        side: Option<PositionSide>,
    },
    DivergentUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlindRecoveryRestartReason {
    AmbiguousOpenExitOrders { count: usize },
    UnattributedOpenExitOrder,
}

#[derive(Debug, Clone, PartialEq)]
enum BlindRecoveryAuthority {
    AuthorityFree,
    Retained(Box<ExposureState>),
}

impl BlindRecoveryAuthority {
    fn retain(&mut self, state: ExposureState) {
        match (self, state) {
            (Self::AuthorityFree, ExposureState::Flat) | (Self::Retained(_), _) => {}
            (slot @ Self::AuthorityFree, retained) => {
                *slot = Self::Retained(Box::new(retained));
            }
        }
    }

    fn retained(&self) -> Option<&ExposureState> {
        match self {
            Self::AuthorityFree => None,
            Self::Retained(retained) => Some(retained),
        }
    }

    fn retained_mut(&mut self) -> Option<&mut Box<ExposureState>> {
        match self {
            Self::AuthorityFree => None,
            Self::Retained(retained) => Some(retained),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct NonEmptyRestartOrderIds {
    first: ClientOrderId,
    remaining: Vec<ClientOrderId>,
}

impl NonEmptyRestartOrderIds {
    fn new(first: ClientOrderId, remaining: Vec<ClientOrderId>) -> Self {
        Self { first, remaining }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        std::iter::once(&self.first)
            .chain(self.remaining.iter())
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum BlindRecoveryCause {
    Probe(BlindRecoveryProbeReason),
    IdentityBearing {
        reason: BlindRecoveryIdentityReason,
        recorded_episode: PositionEpisodeFingerprint,
    },
    RestartAdoption {
        reason: BlindRecoveryRestartReason,
        instrument_id: InstrumentId,
        order_ids: NonEmptyRestartOrderIds,
    },
    ForeignVenue {
        instrument_id: InstrumentId,
        instrument_venue: Venue,
        execution_venue: Venue,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BlindRecoveryState {
    cause: BlindRecoveryCause,
    authority: BlindRecoveryAuthority,
}

impl BlindRecoveryState {
    pub(super) fn probe(reason: BlindRecoveryProbeReason) -> Self {
        Self {
            cause: BlindRecoveryCause::Probe(reason),
            authority: BlindRecoveryAuthority::AuthorityFree,
        }
    }

    pub(super) fn identity_bearing(
        reason: BlindRecoveryIdentityReason,
        recorded_episode: PositionEpisodeFingerprint,
    ) -> Self {
        Self {
            cause: BlindRecoveryCause::IdentityBearing {
                reason,
                recorded_episode,
            },
            authority: BlindRecoveryAuthority::AuthorityFree,
        }
    }

    pub(super) fn restart_adoption(
        reason: BlindRecoveryRestartReason,
        instrument_id: InstrumentId,
        first_order_id: ClientOrderId,
        remaining_order_ids: Vec<ClientOrderId>,
    ) -> Self {
        Self {
            cause: BlindRecoveryCause::RestartAdoption {
                reason,
                instrument_id,
                order_ids: NonEmptyRestartOrderIds::new(first_order_id, remaining_order_ids),
            },
            authority: BlindRecoveryAuthority::AuthorityFree,
        }
    }

    pub(super) fn foreign_venue(
        instrument_id: InstrumentId,
        instrument_venue: Venue,
        execution_venue: Venue,
    ) -> Self {
        Self {
            cause: BlindRecoveryCause::ForeignVenue {
                instrument_id,
                instrument_venue,
                execution_venue,
            },
            authority: BlindRecoveryAuthority::AuthorityFree,
        }
    }

    #[cfg(test)]
    pub(super) fn reason(&self) -> BlindRecoveryReason {
        match &self.cause {
            BlindRecoveryCause::Probe(BlindRecoveryProbeReason::CacheProbeFailed) => {
                BlindRecoveryReason::CacheProbeFailed
            }
            BlindRecoveryCause::Probe(BlindRecoveryProbeReason::MultipleOpenPositions {
                count,
            }) => BlindRecoveryReason::MultipleOpenPositions { count: *count },
            BlindRecoveryCause::Probe(
                BlindRecoveryProbeReason::SettlementEvidenceRecoveryFailed,
            ) => BlindRecoveryReason::SettlementEvidenceRecoveryFailed,
            BlindRecoveryCause::IdentityBearing {
                reason:
                    BlindRecoveryIdentityReason::InvalidBootstrap {
                        entry_order_side,
                        side,
                    },
                ..
            } => BlindRecoveryReason::InvalidBootstrappedPosition {
                entry_order_side: *entry_order_side,
                side: *side,
            },
            BlindRecoveryCause::IdentityBearing {
                reason:
                    BlindRecoveryIdentityReason::InvalidLive {
                        entry_order_side,
                        side,
                    },
                ..
            } => BlindRecoveryReason::InvalidLivePosition {
                entry_order_side: *entry_order_side,
                side: *side,
            },
            BlindRecoveryCause::IdentityBearing {
                reason: BlindRecoveryIdentityReason::DivergentUnsupported,
                ..
            } => BlindRecoveryReason::DivergentUnsupportedPosition,
            BlindRecoveryCause::RestartAdoption {
                reason: BlindRecoveryRestartReason::AmbiguousOpenExitOrders { count },
                instrument_id,
                ..
            } => BlindRecoveryReason::AmbiguousRestartOpenExitOrders {
                instrument_id: *instrument_id,
                count: *count,
            },
            BlindRecoveryCause::RestartAdoption {
                reason: BlindRecoveryRestartReason::UnattributedOpenExitOrder,
                instrument_id,
                ..
            } => BlindRecoveryReason::UnattributedRestartOpenExitOrder {
                instrument_id: *instrument_id,
            },
            BlindRecoveryCause::ForeignVenue {
                instrument_id,
                instrument_venue,
                execution_venue,
            } => BlindRecoveryReason::ForeignVenuePosition {
                instrument_id: *instrument_id,
                instrument_venue: *instrument_venue,
                execution_venue: *execution_venue,
            },
        }
    }

    pub(super) fn retain_authority(&mut self, state: ExposureState) {
        self.authority.retain(state);
    }

    pub(super) fn retained_authority(&self) -> Option<&ExposureState> {
        self.authority.retained()
    }

    fn retained_authority_mut(&mut self) -> Option<&mut Box<ExposureState>> {
        self.authority.retained_mut()
    }

    fn authorizes_exactly_one(&self, managed: &ManagedPositionContext) -> bool {
        match &self.authority {
            BlindRecoveryAuthority::Retained(retained) => match retained.as_ref() {
                ExposureState::ReplacementConflict(conflict) => {
                    conflict.retained.episode == managed.episode
                        || (conflict.candidate.episode == managed.episode
                            && conflict.retained_is_closed())
                }
                retained => {
                    retained
                        .tracked_position_context()
                        .is_some_and(|context| context.episode == managed.episode)
                        || retained.pending_entry().is_some_and(|pending| {
                            pending.instrument_id == managed.instrument_id
                                && pending.client_order_id == managed.episode.opening_order_id
                        })
                }
            },
            BlindRecoveryAuthority::AuthorityFree => match &self.cause {
                BlindRecoveryCause::Probe(_) => true,
                BlindRecoveryCause::IdentityBearing {
                    recorded_episode, ..
                } => recorded_episode == &managed.episode,
                BlindRecoveryCause::RestartAdoption { instrument_id, .. } => {
                    instrument_id == &managed.instrument_id
                }
                BlindRecoveryCause::ForeignVenue { instrument_id, .. } => {
                    instrument_id != &managed.instrument_id
                }
            },
        }
    }

    pub(super) fn authorizes_none(&self) -> bool {
        match &self.authority {
            BlindRecoveryAuthority::Retained(retained) => match retained.as_ref() {
                ExposureState::Flat => true,
                ExposureState::Managed(context) => context.episode_close_seen,
                ExposureState::UnsupportedObserved(observed) => observed.context.episode_close_seen,
                ExposureState::ReplacementConflict(conflict) => conflict.retained_is_closed(),
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_) => false,
            },
            BlindRecoveryAuthority::AuthorityFree => matches!(
                &self.cause,
                BlindRecoveryCause::Probe(_) | BlindRecoveryCause::ForeignVenue { .. }
            ),
        }
    }

    fn is_restart_adoption(&self) -> bool {
        matches!(&self.cause, BlindRecoveryCause::RestartAdoption { .. })
    }

    #[cfg(test)]
    pub(super) fn restart_order_count(&self) -> Option<usize> {
        match &self.cause {
            BlindRecoveryCause::RestartAdoption { order_ids, .. } => Some(order_ids.len()),
            BlindRecoveryCause::Probe(_)
            | BlindRecoveryCause::IdentityBearing { .. }
            | BlindRecoveryCause::ForeignVenue { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum RouteOperationPayload {
    Entry(Box<PendingEntryState>),
    Exit(Box<ExitAttemptingState>),
}

impl RouteOperationPayload {
    fn operation(&self) -> ExposureOperationKind {
        match self {
            Self::Entry(_) => ExposureOperationKind::EntryRoute,
            Self::Exit(_) => ExposureOperationKind::ExitRoute,
        }
    }

    fn client_order_id(&self) -> ClientOrderId {
        match self {
            Self::Entry(pending) => pending.client_order_id,
            Self::Exit(attempt) => attempt.authority.client_order_id(),
        }
    }

    fn instrument_id(&self) -> InstrumentId {
        match self {
            Self::Entry(pending) => pending.instrument_id,
            Self::Exit(attempt) => attempt.managed.instrument_id,
        }
    }

    fn position_context(&self) -> Option<&ManagedPositionContext> {
        match self {
            Self::Entry(_) => None,
            Self::Exit(attempt) => Some(&attempt.managed),
        }
    }

    fn market_id(&self) -> Option<String> {
        match self {
            Self::Entry(pending) => pending.lifecycle.market_id_owned(),
            Self::Exit(attempt) => attempt.managed.lifecycle.market_id_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OperationSinkUnknownState {
    pub(super) operation: ExposureOperationKind,
    pub(super) generation: u64,
    pub(super) client_order_id: ClientOrderId,
    pub(super) attempted: RouteOperationPayload,
    pub(super) prior: Box<ExposureState>,
}

impl OperationSinkUnknownState {
    pub(super) fn instrument_id(&self) -> InstrumentId {
        self.attempted.instrument_id()
    }

    pub(super) fn position_context(&self) -> Option<&ManagedPositionContext> {
        self.attempted.position_context()
    }

    pub(super) fn market_id(&self) -> Option<String> {
        self.attempted.market_id()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ObligationSaturatedState {
    pub(super) retained: Box<ExposureState>,
    pub(super) client_order_id: ClientOrderId,
    pub(super) obligation_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReplacementConflictState {
    pub(super) retained: ManagedPositionContext,
    pub(super) candidate: ManagedPositionContext,
    pub(super) retained_close: ReplacementRetainedCloseProof,
    pub(super) candidate_projection: ReplacementCandidateProjection,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ReplacementRetainedCloseProof {
    Awaiting,
    Observed,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ReplacementCandidateProjection {
    Matching,
    None,
    Divergent { episode: PositionEpisodeFingerprint },
    Multiple { count: usize },
    ProbeFailed { diagnostic: String },
    RecoveryHeld,
}

enum ReplacementProjectionObservation {
    None,
    Managed(Box<ManagedPositionContext>),
    Divergent(PositionEpisodeFingerprint),
    Multiple(usize),
    ProbeFailed(String),
    RecoveryHeld,
}

enum ReplacementConflictEvent {
    Canonical(ReplacementProjectionObservation),
    RetainedClosed {
        episode: PositionEpisodeFingerprint,
        projection: ReplacementProjectionObservation,
    },
}

impl From<FreshCanonicalPositionProjection> for ReplacementProjectionObservation {
    fn from(projection: FreshCanonicalPositionProjection) -> Self {
        match projection {
            FreshCanonicalPositionProjection::None => Self::None,
            FreshCanonicalPositionProjection::ExactlyOne(classified) => match *classified {
                ClassifiedOpenPosition::Managed(managed) => Self::Managed(Box::new(managed)),
                ClassifiedOpenPosition::Unsupported(unsupported) => {
                    Self::Divergent(unsupported.context.episode)
                }
                ClassifiedOpenPosition::BlindRecovery(_) => Self::RecoveryHeld,
            },
            FreshCanonicalPositionProjection::Multiple { count } => Self::Multiple(count),
            FreshCanonicalPositionProjection::ProbeFailed { diagnostic } => {
                Self::ProbeFailed(diagnostic)
            }
        }
    }
}

#[derive(Debug)]
enum ReplacementConflictResolution {
    Unresolved(Box<ReplacementConflictState>),
    Retained(ManagedPositionContext),
    Adopted(ReplacementAdoption),
    Released(Option<PendingEntryState>),
}

enum ReplacementConflictContainer {
    Direct,
    BlindRecovery(BlindRecoveryState),
}

impl ReplacementConflictResolution {
    fn into_transition(
        self,
        container: ReplacementConflictContainer,
    ) -> (ExposureState, Option<ReplacementAdoption>) {
        match (self, container) {
            (Self::Unresolved(conflict), ReplacementConflictContainer::Direct) => {
                (ExposureState::ReplacementConflict(conflict), None)
            }
            (
                Self::Unresolved(conflict),
                ReplacementConflictContainer::BlindRecovery(mut recovery),
            ) => {
                recovery.authority = BlindRecoveryAuthority::Retained(Box::new(
                    ExposureState::ReplacementConflict(conflict),
                ));
                (ExposureState::BlindRecovery(recovery), None)
            }
            (Self::Retained(retained), _) => (ExposureState::Managed(retained), None),
            (Self::Adopted(adoption), _) => (
                ExposureState::Managed(adoption.adopted.clone()),
                Some(adoption),
            ),
            (Self::Released(pending), _) => (
                pending.map_or(ExposureState::Flat, ExposureState::PendingEntry),
                None,
            ),
        }
    }
}

impl ReplacementConflictState {
    fn retained_is_closed(&self) -> bool {
        matches!(self.retained_close, ReplacementRetainedCloseProof::Observed)
    }

    fn observe_retained_close(&mut self, episode: &PositionEpisodeFingerprint) -> bool {
        match (episode == &self.retained.episode, self.retained_is_closed()) {
            (false, _) | (true, true) => false,
            (true, false) => {
                self.retained_close = ReplacementRetainedCloseProof::Observed;
                true
            }
        }
    }

    fn observe_projection(&mut self, projection: ReplacementCandidateProjection) {
        self.candidate_projection = projection;
    }

    fn transition(mut self, event: ReplacementConflictEvent) -> ReplacementConflictResolution {
        let projection = match event {
            ReplacementConflictEvent::Canonical(projection) => projection,
            ReplacementConflictEvent::RetainedClosed {
                episode,
                projection,
            } => match episode == self.retained.episode {
                false => return ReplacementConflictResolution::Unresolved(Box::new(self)),
                true => {
                    self.retained_close = ReplacementRetainedCloseProof::Observed;
                    projection
                }
            },
        };
        match projection {
            ReplacementProjectionObservation::None => {
                self.observe_projection(ReplacementCandidateProjection::None);
            }
            ReplacementProjectionObservation::Managed(managed) => match (
                managed.episode == self.retained.episode,
                managed.episode == self.candidate.episode,
            ) {
                (true, _) => {
                    return ReplacementConflictResolution::Retained(refresh_replacement_candidate(
                        self.retained,
                        *managed,
                    ));
                }
                (false, true) => {
                    self.candidate = refresh_replacement_candidate(self.candidate, *managed);
                    self.observe_projection(ReplacementCandidateProjection::Matching);
                }
                (false, false) => {
                    self.observe_projection(ReplacementCandidateProjection::Divergent {
                        episode: managed.episode,
                    })
                }
            },
            ReplacementProjectionObservation::Divergent(episode) => {
                self.observe_projection(ReplacementCandidateProjection::Divergent { episode })
            }
            ReplacementProjectionObservation::Multiple(count) => {
                self.observe_projection(ReplacementCandidateProjection::Multiple { count });
            }
            ReplacementProjectionObservation::ProbeFailed(diagnostic) => {
                self.observe_projection(ReplacementCandidateProjection::ProbeFailed { diagnostic });
            }
            ReplacementProjectionObservation::RecoveryHeld => {
                self.observe_projection(ReplacementCandidateProjection::RecoveryHeld);
            }
        }
        self.resolve()
    }

    fn resolve(self) -> ReplacementConflictResolution {
        match (self.retained_is_closed(), &self.candidate_projection) {
            (false, _) => ReplacementConflictResolution::Unresolved(Box::new(self)),
            (true, ReplacementCandidateProjection::Matching) => {
                let adoption = ReplacementAdoption {
                    retained_episode: self.retained.episode.clone(),
                    adopted: self.candidate.clone(),
                    cause: ReplacementAdoptionCause::CanonicalCloseConjunction,
                };
                ReplacementConflictResolution::Adopted(adoption)
            }
            (true, ReplacementCandidateProjection::None) => {
                ReplacementConflictResolution::Released(self.retained.pending_entry.clone())
            }
            (
                true,
                ReplacementCandidateProjection::Divergent { .. }
                | ReplacementCandidateProjection::Multiple { .. }
                | ReplacementCandidateProjection::ProbeFailed { .. }
                | ReplacementCandidateProjection::RecoveryHeld,
            ) => ReplacementConflictResolution::Unresolved(Box::new(self)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExposureState {
    Flat,
    PendingEntry(PendingEntryState),
    EntryReconcilePending {
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    },
    Managed(ManagedPositionContext),
    ExitAttempting(ExitAttemptingState),
    ExitPending(ExitPendingState),
    TerminalExitAwaitingPosition(ExitPendingState),
    ExitAuthorityRecoveryHold(ExitAuthorityRecoveryHoldState),
    UnsupportedObserved(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
    OperationSinkUnknown(OperationSinkUnknownState),
    ObligationSaturated(ObligationSaturatedState),
    ReplacementConflict(Box<ReplacementConflictState>),
}

/// The strategy's sole exposure mutation authority.
///
/// `state` is deliberately private and there is no mutable state projection.
/// Every variant change and context refresh enters through [`Self::reduce`].
#[derive(Debug, Clone, PartialEq)]
struct GovernedExposureInner {
    state: ExposureState,
    generation: u64,
    limits: ExposureObligationLimits,
    quarantined_orders: BTreeMap<ClientOrderId, UntrackedOrderEvent>,
    released_exits: BTreeMap<ClientOrderId, ReleasedExitProvenance>,
    obligations: BTreeMap<ClientOrderId, DeferredExitObligation>,
    identity_conflict: Option<IdentityConflict>,
    last_outcome: ExposureTransitionOutcome,
    operation_arm: Option<OperationArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct IdentityConflict {
    pub(super) retained: ExposureStateKind,
    pub(super) retained_episode: Option<PositionEpisodeFingerprint>,
    pub(super) candidate: ManagedPositionContext,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReplacementAdoption {
    pub(super) retained_episode: PositionEpisodeFingerprint,
    pub(super) adopted: ManagedPositionContext,
    pub(super) cause: ReplacementAdoptionCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplacementAdoptionCause {
    CanonicalCloseConjunction,
    AuthenticatedCorrection,
}

#[must_use = "adoption-capable exposure transitions must handle replacement adoption"]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExposureAdoptionCommit {
    pub(super) outcome: ExposureTransitionOutcome,
    pub(super) replacement_adoption: Option<ReplacementAdoption>,
}

enum ExposureReductionEvent {
    NonAdopting(Box<ExposureEvent>),
    AdoptionCapable(AdoptionCapableExposureEvent),
}

pub(super) struct ExposureReductionRequest<Output> {
    event: ExposureReductionEvent,
    project: fn(ExposureTransitionOutcome, Option<ReplacementAdoption>) -> Output,
}

impl From<ExposureEvent> for ExposureReductionRequest<ExposureTransitionOutcome> {
    fn from(event: ExposureEvent) -> Self {
        Self {
            event: ExposureReductionEvent::NonAdopting(Box::new(event)),
            project: |outcome, replacement_adoption| {
                debug_assert!(replacement_adoption.is_none());
                outcome
            },
        }
    }
}

impl From<AdoptionCapableExposureEvent> for ExposureReductionRequest<ExposureAdoptionCommit> {
    fn from(event: AdoptionCapableExposureEvent) -> Self {
        Self {
            event: ExposureReductionEvent::AdoptionCapable(event),
            project: |outcome, replacement_adoption| ExposureAdoptionCommit {
                outcome,
                replacement_adoption,
            },
        }
    }
}

impl<Output> ExposureReductionRequest<Output> {
    fn apply(self, inner: &mut GovernedExposureInner) -> Output {
        match self.event {
            ExposureReductionEvent::NonAdopting(event) => {
                (self.project)(inner.reduce(*event), None)
            }
            ExposureReductionEvent::AdoptionCapable(event) => {
                let ExposureAdoptionCommit {
                    outcome,
                    replacement_adoption,
                } = inner.reduce_adoption_capable(event);
                (self.project)(outcome, replacement_adoption)
            }
        }
    }

    fn preserved(self, state: ExposureStateKind) -> Output {
        (self.project)(ExposureTransitionOutcome::Preserved { state }, None)
    }
}

impl GovernedExposureInner {
    fn new(limits: ExposureObligationLimits) -> Self {
        Self {
            state: ExposureState::Flat,
            generation: 0,
            limits,
            quarantined_orders: BTreeMap::new(),
            released_exits: BTreeMap::new(),
            obligations: BTreeMap::new(),
            identity_conflict: None,
            last_outcome: ExposureTransitionOutcome::Preserved {
                state: ExposureStateKind::Flat,
            },
            operation_arm: None,
        }
    }

    fn reduce(&mut self, event: ExposureEvent) -> ExposureTransitionOutcome {
        let from = self.state.kind();
        let changed = match event {
            ExposureEvent::EntryLifecycle(event) => self.reduce_entry_lifecycle(event),
            ExposureEvent::ExitLifecycle(event) => self.reduce_exit_lifecycle(event),
            ExposureEvent::UntrackedOrder(event) => match event {
                UntrackedOrderEvent::Quarantine {
                    client_order_id,
                    instrument_id,
                } => {
                    self.quarantined_orders.insert(
                        client_order_id,
                        UntrackedOrderEvent::Quarantine {
                            client_order_id,
                            instrument_id,
                        },
                    );
                    false
                }
                UntrackedOrderEvent::HistoricalExitCorrection(correction) => {
                    self.reduce_historical_exit_correction(correction)
                }
                UntrackedOrderEvent::HistoricalExitObservation(observation) => {
                    self.reduce_historical_exit_observation(observation)
                }
                UntrackedOrderEvent::ResolveHistoricalExitCorrection { client_order_id } => {
                    self.obligations.remove(&client_order_id).is_some()
                }
                UntrackedOrderEvent::ObligationSaturated { client_order_id } => {
                    self.enter_obligation_saturation(client_order_id)
                }
            },
            ExposureEvent::PositionTruth(event) => self.reduce_position_truth(event),
            ExposureEvent::TimerReconciliation(event) => self.reduce_timer_reconciliation(event),
            ExposureEvent::BootstrapAdoption(event) => self.reduce_bootstrap_adoption(event),
            ExposureEvent::SettlementEffect(event) => self.reduce_settlement_effect(event),
        };
        self.finish_reduction(from, changed)
    }

    fn reduce_adoption_capable(
        &mut self,
        event: AdoptionCapableExposureEvent,
    ) -> ExposureAdoptionCommit {
        let from = self.state.kind();
        let mut replacement_adoption = None;
        let changed = match event {
            AdoptionCapableExposureEvent::PositionTruth(event) => {
                self.reduce_adoption_capable_position_truth(event, &mut replacement_adoption)
            }
            AdoptionCapableExposureEvent::PositionClosed(event) => {
                self.reduce_position_closed(event, &mut replacement_adoption)
            }
        };
        ExposureAdoptionCommit {
            outcome: self.finish_reduction(from, changed),
            replacement_adoption,
        }
    }

    fn finish_reduction(
        &mut self,
        from: ExposureStateKind,
        changed: bool,
    ) -> ExposureTransitionOutcome {
        let to = self.state.kind();
        let outcome = if changed {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("validated exposure generation space exhausted");
            self.operation_arm = None;
            ExposureTransitionOutcome::Applied { from, to }
        } else {
            ExposureTransitionOutcome::Preserved { state: to }
        };
        self.last_outcome = outcome;
        outcome
    }

    fn replace(&mut self, next: ExposureState) -> bool {
        if self.state == next {
            return false;
        }
        if matches!(
            next,
            ExposureState::Flat | ExposureState::Managed(_) | ExposureState::PendingEntry(_)
        ) && let Some(provenance) = released_exit_provenance(&self.state)
        {
            if !self
                .released_exits
                .contains_key(&provenance.client_order_id)
                && self.released_exits.len()
                    >= self.limits.max_released_exit_provenance_count.get() as usize
            {
                let client_order_id = provenance.client_order_id;
                let retained = self.state.clone();
                self.state = ExposureState::ObligationSaturated(ObligationSaturatedState {
                    retained: Box::new(retained),
                    client_order_id,
                    obligation_count: self.released_exits.len(),
                });
                return true;
            }
            self.released_exits
                .insert(provenance.client_order_id, provenance);
        }
        self.state = next;
        true
    }

    fn replace_from_operation(&mut self, next: ExposureState) {
        let from = self.state.kind();
        let changed = self.state != next;
        self.state = next;
        let to = self.state.kind();
        self.last_outcome = if changed {
            ExposureTransitionOutcome::Applied { from, to }
        } else {
            ExposureTransitionOutcome::Preserved { state: to }
        };
    }

    fn enter_obligation_saturation(&mut self, client_order_id: ClientOrderId) -> bool {
        if matches!(self.state, ExposureState::ObligationSaturated(_)) {
            return false;
        }
        let retained = self.state.clone();
        self.state = ExposureState::ObligationSaturated(ObligationSaturatedState {
            retained: Box::new(retained),
            client_order_id,
            obligation_count: self.obligations.len(),
        });
        true
    }

    fn reduce_historical_exit_correction(&mut self, correction: HistoricalExitCorrection) -> bool {
        let Some(provenance) = self
            .released_exits
            .get(&correction.client_order_id)
            .filter(|provenance| provenance.episode.instrument_id == correction.instrument_id)
            .cloned()
        else {
            self.quarantined_orders.insert(
                correction.client_order_id,
                UntrackedOrderEvent::Quarantine {
                    client_order_id: correction.client_order_id,
                    instrument_id: correction.instrument_id,
                },
            );
            return false;
        };
        if self
            .obligations
            .get(&correction.client_order_id)
            .is_some_and(|obligation| obligation.history.contains_key(&correction.trade_id))
        {
            return false;
        }
        let max_count = self.limits.max_count.get() as usize;
        let max_history = self.limits.max_history_events_per_obligation.get() as usize;
        let creates_obligation = !self.obligations.contains_key(&correction.client_order_id);
        let history_full = self
            .obligations
            .get(&correction.client_order_id)
            .is_some_and(|obligation| {
                obligation.history.len() + obligation.observations.len() >= max_history
            });
        if (creates_obligation && self.obligations.len() >= max_count) || history_full {
            return self.enter_obligation_saturation(correction.client_order_id);
        }
        self.obligations
            .entry(correction.client_order_id)
            .or_insert_with(|| DeferredExitObligation {
                provenance,
                history: BTreeMap::new(),
                observations: BTreeMap::new(),
            })
            .history
            .insert(correction.trade_id, correction);
        true
    }

    fn reduce_historical_exit_observation(
        &mut self,
        observation: HistoricalExitObservation,
    ) -> bool {
        let historically_attributed = self
            .released_exits
            .get(&observation.client_order_id)
            .filter(|provenance| provenance.episode.instrument_id == observation.instrument_id)
            .is_some();
        if !historically_attributed {
            self.quarantined_orders.insert(
                observation.client_order_id,
                UntrackedOrderEvent::Quarantine {
                    client_order_id: observation.client_order_id,
                    instrument_id: observation.instrument_id,
                },
            );
            return false;
        }
        if !self.obligations.contains_key(&observation.client_order_id) {
            return false;
        }
        let existing = self
            .obligations
            .get(&observation.client_order_id)
            .and_then(|obligation| obligation.observations.get(&observation.key));
        if existing.is_some_and(|current| current == &observation.observation) {
            return false;
        }
        let replaces_existing = existing.is_some();
        let max_history = self.limits.max_history_events_per_obligation.get() as usize;
        let history_full = self
            .obligations
            .get(&observation.client_order_id)
            .is_some_and(|obligation| {
                obligation.history.len() + obligation.observations.len() >= max_history
            });
        if !replaces_existing && history_full {
            return self.enter_obligation_saturation(observation.client_order_id);
        }
        self.obligations
            .get_mut(&observation.client_order_id)
            .expect("historical observation requires an existing obligation")
            .observations
            .insert(observation.key, observation.observation);
        true
    }

    fn enter_blind_recovery(&mut self, mut recovery: BlindRecoveryState) -> bool {
        if matches!(self.state, ExposureState::BlindRecovery(_)) {
            return false;
        }
        self.identity_conflict = None;
        recovery.retain_authority(self.state.clone());
        self.replace(ExposureState::BlindRecovery(recovery))
    }

    fn reduce_entry_lifecycle(&mut self, event: EntryLifecycleEvent) -> bool {
        match &mut self.state {
            ExposureState::BlindRecovery(recovery) => {
                return recovery
                    .retained_authority_mut()
                    .is_some_and(|retained| reduce_retained_entry_lifecycle(retained, event));
            }
            ExposureState::ObligationSaturated(saturated) => {
                return reduce_retained_entry_lifecycle(&mut saturated.retained, event);
            }
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ReplacementConflict(_) => {}
        }
        match event {
            #[cfg(test)]
            EntryLifecycleEvent::RestorePending(pending) => match &self.state {
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. } => {
                    self.replace(ExposureState::PendingEntry(pending))
                }
                ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            EntryLifecycleEvent::Reconcile { pending, reason } => match &self.state {
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. } => {
                    self.replace(ExposureState::EntryReconcilePending { pending, reason })
                }
                ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            EntryLifecycleEvent::ReleaseFlat => match &self.state {
                ExposureState::PendingEntry(_) | ExposureState::EntryReconcilePending { .. } => {
                    let next = self
                        .identity_conflict
                        .take()
                        .filter(|conflict| {
                            matches!(
                                conflict.retained,
                                ExposureStateKind::PendingEntry
                                    | ExposureStateKind::EntryReconcilePending
                            )
                        })
                        .map_or(ExposureState::Flat, |conflict| {
                            ExposureState::Managed(conflict.candidate)
                        });
                    self.replace(next)
                }
                ExposureState::Flat
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            EntryLifecycleEvent::ClearManagedPending {
                client_order_id,
                instrument_id,
            } => self.clear_managed_pending(client_order_id, instrument_id),
            EntryLifecycleEvent::RefreshPending(pending) => match &mut self.state {
                ExposureState::PendingEntry(current)
                | ExposureState::EntryReconcilePending {
                    pending: current, ..
                } if current.client_order_id == pending.client_order_id
                    && current.instrument_id == pending.instrument_id =>
                {
                    *current = pending;
                    true
                }
                ExposureState::Managed(context)
                | ExposureState::ExitAttempting(ExitAttemptingState {
                    managed: context, ..
                }) => {
                    let Some(current) = context.pending_entry.as_mut() else {
                        return false;
                    };
                    if current.client_order_id != pending.client_order_id
                        || current.instrument_id != pending.instrument_id
                    {
                        return false;
                    }
                    *current = pending;
                    true
                }
                ExposureState::ExitPending(exit)
                | ExposureState::TerminalExitAwaitingPosition(exit) => {
                    let Some(current) = exit.position.pending_entry.as_mut() else {
                        return false;
                    };
                    if current.client_order_id != pending.client_order_id
                        || current.instrument_id != pending.instrument_id
                    {
                        return false;
                    }
                    *current = pending;
                    true
                }
                ExposureState::ExitAuthorityRecoveryHold(hold) => {
                    let Some(current) = hold.position.pending_entry.as_mut() else {
                        return false;
                    };
                    if current.client_order_id != pending.client_order_id
                        || current.instrument_id != pending.instrument_id
                    {
                        return false;
                    }
                    *current = pending;
                    true
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
        }
    }

    fn clear_managed_pending(
        &mut self,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    ) -> bool {
        let context = match &mut self.state {
            ExposureState::Managed(context) => Some(context),
            ExposureState::ExitAttempting(attempt) => Some(&mut attempt.managed),
            ExposureState::ExitPending(exit)
            | ExposureState::TerminalExitAwaitingPosition(exit) => Some(&mut exit.position),
            ExposureState::ExitAuthorityRecoveryHold(hold) => Some(&mut hold.position),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => None,
        };
        let Some(context) = context else {
            return false;
        };
        if context.instrument_id != instrument_id
            || !context
                .pending_entry
                .as_ref()
                .is_some_and(|pending| pending.client_order_id == client_order_id)
        {
            return false;
        }
        context.pending_entry = None;
        true
    }

    fn reduce_exit_lifecycle(&mut self, event: ExitLifecycleEvent) -> bool {
        let prior_state = self.state.clone();
        if let ExposureState::BlindRecovery(recovery) = &mut self.state {
            let Some(retained) = recovery.retained_authority_mut() else {
                return false;
            };
            let (changed, provenance) =
                reduce_retained_exit_lifecycle_with_provenance(retained, event);
            if let Some(provenance) = provenance {
                if !self
                    .released_exits
                    .contains_key(&provenance.client_order_id)
                    && self.released_exits.len()
                        >= self.limits.max_released_exit_provenance_count.get() as usize
                {
                    self.state = ExposureState::ObligationSaturated(ObligationSaturatedState {
                        retained: Box::new(prior_state),
                        client_order_id: provenance.client_order_id,
                        obligation_count: self.released_exits.len(),
                    });
                    return true;
                }
                self.released_exits
                    .insert(provenance.client_order_id, provenance);
            }
            return changed;
        }
        if let ExposureState::ObligationSaturated(saturated) = &mut self.state {
            let (changed, provenance) =
                reduce_retained_exit_lifecycle_with_provenance(&mut saturated.retained, event);
            if let Some(provenance) = provenance {
                if !self
                    .released_exits
                    .contains_key(&provenance.client_order_id)
                    && self.released_exits.len()
                        >= self.limits.max_released_exit_provenance_count.get() as usize
                {
                    self.state = prior_state;
                    return false;
                }
                self.released_exits
                    .insert(provenance.client_order_id, provenance);
            }
            return changed;
        }
        match event {
            ExitLifecycleEvent::Pending(pending) => match &self.state {
                ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::Flat => self.replace(ExposureState::ExitPending(pending)),
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            ExitLifecycleEvent::Working {
                expected_generation,
                observation,
                pending,
            } => {
                if expected_generation != self.generation {
                    return false;
                }
                match &self.state {
                    ExposureState::ExitAttempting(attempt)
                        if attempt.generation == expected_generation
                            && attempt.authority.client_order_id() == pending.client_order_id() =>
                    {
                        self.replace(ExposureState::ExitPending(pending))
                    }
                    ExposureState::ExitPending(current)
                        if current.client_order_id() == pending.client_order_id() =>
                    {
                        self.replace(ExposureState::ExitPending(pending))
                    }
                    ExposureState::TerminalExitAwaitingPosition(current)
                        if observation == ExitWorkingObservation::Correction
                            && current.client_order_id() == pending.client_order_id() =>
                    {
                        self.replace(ExposureState::ExitPending(pending))
                    }
                    ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::Managed(_)
                    | ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::BlindRecovery(_)
                    | ExposureState::OperationSinkUnknown(_)
                    | ExposureState::ObligationSaturated(_)
                    | ExposureState::ReplacementConflict(_) => false,
                }
            }
            ExitLifecycleEvent::TerminalAwaitingPosition(pending) => match &self.state {
                ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_) => {
                    self.replace(ExposureState::TerminalExitAwaitingPosition(pending))
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            ExitLifecycleEvent::RecoveryHold(hold) => match &self.state {
                ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::Flat
                | ExposureState::Managed(_) => {
                    self.replace(ExposureState::ExitAuthorityRecoveryHold(hold))
                }
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            ExitLifecycleEvent::RefreshAuthority(authority) => match &mut self.state {
                ExposureState::ExitPending(exit)
                | ExposureState::TerminalExitAwaitingPosition(exit)
                    if exit.client_order_id() == authority.client_order_id() =>
                {
                    if exit.authority == authority {
                        false
                    } else {
                        exit.authority = authority;
                        true
                    }
                }
                ExposureState::ExitAuthorityRecoveryHold(hold)
                    if hold.client_order_id() == authority.client_order_id() =>
                {
                    let ExitAuthorityRecoveryPlan::Resume(current) = &mut hold.plan else {
                        return false;
                    };
                    if *current == authority {
                        false
                    } else {
                        *current = authority;
                        true
                    }
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            ExitLifecycleEvent::Residual(managed) => match &self.state {
                ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_) => {
                    self.replace(ExposureState::Managed(managed))
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            ExitLifecycleEvent::ReleaseFlat => match &self.state {
                ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_) => self.replace(ExposureState::Flat),
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
        }
    }

    fn reduce_position_truth(&mut self, event: PositionTruthEvent) -> bool {
        match event {
            PositionTruthEvent::EntryTerminalMaterialization {
                client_order_id,
                managed,
            } => self.reduce_entry_terminal_materialization(client_order_id, managed),
            PositionTruthEvent::RefreshContext(context) => self.refresh_context(context),
            PositionTruthEvent::Unsupported(mut observed) => match &self.state {
                ExposureState::Flat => self.replace(ExposureState::UnsupportedObserved(observed)),
                ExposureState::Managed(current) if current.episode == observed.context.episode => {
                    observed.context.episode_close_seen = current.episode_close_seen;
                    observed.context.canonical_none_seen = current.canonical_none_seen;
                    observed.context.pending_entry = current.pending_entry.clone();
                    self.replace(ExposureState::UnsupportedObserved(observed))
                }
                ExposureState::UnsupportedObserved(current)
                    if current.context.episode == observed.context.episode =>
                {
                    observed.context.episode_close_seen = current.context.episode_close_seen;
                    observed.context.canonical_none_seen = current.context.canonical_none_seen;
                    observed.context.pending_entry = current.context.pending_entry.clone();
                    self.replace(ExposureState::UnsupportedObserved(observed))
                }
                ExposureState::BlindRecovery(_) => self.record_identity_conflict(observed.context),
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => {
                    let candidate = observed.context;
                    let retained_kind = self.state.kind();
                    let retained_episode = self
                        .state
                        .tracked_position_context()
                        .map(|context| context.episode.clone());
                    let retained = self.state.clone();
                    let mut recovery = BlindRecoveryState::identity_bearing(
                        BlindRecoveryIdentityReason::DivergentUnsupported,
                        candidate.episode.clone(),
                    );
                    recovery.retain_authority(retained);
                    self.state = ExposureState::BlindRecovery(recovery);
                    self.identity_conflict = Some(IdentityConflict {
                        retained: retained_kind,
                        retained_episode,
                        candidate,
                    });
                    true
                }
            },
            PositionTruthEvent::BlindRecovery(recovery) => self.enter_blind_recovery(recovery),
        }
    }

    fn reduce_adoption_capable_position_truth(
        &mut self,
        event: AdoptionCapablePositionTruthEvent,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        match event {
            AdoptionCapablePositionTruthEvent::Canonical(projection) => {
                self.reduce_canonical_projection(projection, replacement_adoption)
            }
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(projection) => {
                self.reduce_authorized_recovery(projection, replacement_adoption)
            }
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before,
                authenticated_order_id,
                authenticated_fill_id,
                rebased,
            } => self.reduce_authenticated_episode_rebase(
                before,
                authenticated_order_id,
                authenticated_fill_id,
                rebased.map(|rebased| *rebased),
                replacement_adoption,
            ),
        }
    }

    fn reduce_canonical_projection(
        &mut self,
        projection: CanonicalPositionProjection,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        match projection {
            CanonicalPositionProjection::None => match &mut self.state {
                ExposureState::Managed(context) => {
                    context.canonical_none_seen = true;
                    if context.episode_close_seen {
                        self.state = context
                            .pending_entry
                            .clone()
                            .map_or(ExposureState::Flat, ExposureState::PendingEntry);
                    }
                    true
                }
                ExposureState::UnsupportedObserved(observed) => {
                    observed.context.canonical_none_seen = true;
                    if observed.context.episode_close_seen {
                        self.state = ExposureState::Flat;
                    }
                    true
                }
                ExposureState::ReplacementConflict(conflict) => {
                    let resolution =
                        (**conflict)
                            .clone()
                            .transition(ReplacementConflictEvent::Canonical(
                                ReplacementProjectionObservation::None,
                            ));
                    let (state, adoption) =
                        resolution.into_transition(ReplacementConflictContainer::Direct);
                    self.state = state;
                    *replacement_adoption = adoption;
                    true
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_) => false,
            },
            CanonicalPositionProjection::ExactlyOne(managed) => {
                let managed = *managed;
                match &mut self.state {
                    ExposureState::ReplacementConflict(conflict) => {
                        let resolution =
                            (**conflict)
                                .clone()
                                .transition(ReplacementConflictEvent::Canonical(
                                    ReplacementProjectionObservation::Managed(Box::new(managed)),
                                ));
                        let (state, adoption) =
                            resolution.into_transition(ReplacementConflictContainer::Direct);
                        self.state = state;
                        *replacement_adoption = adoption;
                        true
                    }
                    ExposureState::BlindRecovery(_) => false,
                    ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::Managed(_)
                    | ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::OperationSinkUnknown(_)
                    | ExposureState::ObligationSaturated(_) => {
                        self.apply_managed_truth(managed, replacement_adoption)
                    }
                }
            }
            CanonicalPositionProjection::Multiple { count, recovery } => {
                if let ExposureState::ReplacementConflict(conflict) = &mut self.state {
                    conflict.observe_projection(ReplacementCandidateProjection::Multiple { count });
                }
                self.enter_blind_recovery(recovery)
            }
            CanonicalPositionProjection::ProbeFailed {
                diagnostic,
                recovery,
            } => {
                if let ExposureState::ReplacementConflict(conflict) = &mut self.state {
                    conflict.observe_projection(ReplacementCandidateProjection::ProbeFailed {
                        diagnostic,
                    });
                }
                self.enter_blind_recovery(recovery)
            }
        }
    }

    fn reduce_authorized_recovery(
        &mut self,
        projection: FreshCanonicalPositionProjection,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        match projection {
            FreshCanonicalPositionProjection::None => match &self.state {
                ExposureState::BlindRecovery(recovery) if recovery.authorizes_none() => {
                    self.identity_conflict = None;
                    self.state = ExposureState::Flat;
                    true
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            FreshCanonicalPositionProjection::ExactlyOne(classified) => match *classified {
                ClassifiedOpenPosition::Managed(managed) => match &self.state {
                    ExposureState::BlindRecovery(recovery)
                        if recovery.authorizes_exactly_one(&managed) =>
                    {
                        let changed = self.apply_managed_truth(managed, replacement_adoption);
                        if changed {
                            self.identity_conflict = None;
                        }
                        changed
                    }
                    ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::Managed(_)
                    | ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::BlindRecovery(_)
                    | ExposureState::OperationSinkUnknown(_)
                    | ExposureState::ObligationSaturated(_)
                    | ExposureState::ReplacementConflict(_) => false,
                },
                ClassifiedOpenPosition::Unsupported(unsupported) => match &self.state {
                    ExposureState::BlindRecovery(recovery)
                        if recovery.authorizes_exactly_one(&unsupported.context) =>
                    {
                        self.identity_conflict = None;
                        self.state = ExposureState::UnsupportedObserved(unsupported);
                        true
                    }
                    ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::Managed(_)
                    | ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::BlindRecovery(_)
                    | ExposureState::OperationSinkUnknown(_)
                    | ExposureState::ObligationSaturated(_)
                    | ExposureState::ReplacementConflict(_) => false,
                },
                ClassifiedOpenPosition::BlindRecovery(mut classified) => match &self.state {
                    ExposureState::BlindRecovery(current) => {
                        if let Some(retained) = current.retained_authority().cloned() {
                            classified.retain_authority(retained);
                        }
                        self.identity_conflict = None;
                        self.state = ExposureState::BlindRecovery(classified);
                        true
                    }
                    ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
                    | ExposureState::Managed(_)
                    | ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::UnsupportedObserved(_)
                    | ExposureState::OperationSinkUnknown(_)
                    | ExposureState::ObligationSaturated(_)
                    | ExposureState::ReplacementConflict(_) => false,
                },
            },
            FreshCanonicalPositionProjection::Multiple { count } => {
                self.enter_blind_recovery(BlindRecoveryState::probe(
                    BlindRecoveryProbeReason::MultipleOpenPositions { count },
                ))
            }
            FreshCanonicalPositionProjection::ProbeFailed { .. } => self.enter_blind_recovery(
                BlindRecoveryState::probe(BlindRecoveryProbeReason::CacheProbeFailed),
            ),
        }
    }

    fn refresh_context(&mut self, mut refreshed: ManagedPositionContext) -> bool {
        let current = match &mut self.state {
            ExposureState::Managed(current) => Some(current),
            ExposureState::ExitAttempting(attempt) => Some(&mut attempt.managed),
            ExposureState::ExitPending(exit)
            | ExposureState::TerminalExitAwaitingPosition(exit) => Some(&mut exit.position),
            ExposureState::ExitAuthorityRecoveryHold(hold) => Some(&mut hold.position),
            ExposureState::UnsupportedObserved(observed) => Some(&mut observed.context),
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::BlindRecovery(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => None,
        };
        let Some(current) = current else {
            return false;
        };
        if current.episode != refreshed.episode {
            return false;
        }
        refreshed.episode_close_seen = current.episode_close_seen;
        refreshed.canonical_none_seen = current.canonical_none_seen;
        *current = refreshed;
        true
    }

    fn reduce_entry_terminal_materialization(
        &mut self,
        client_order_id: ClientOrderId,
        managed: ManagedPositionContext,
    ) -> bool {
        let matching_pending = match &self.state {
            ExposureState::PendingEntry(pending)
            | ExposureState::EntryReconcilePending { pending, .. } => {
                pending.client_order_id == client_order_id
                    && pending.instrument_id == managed.instrument_id
            }
            ExposureState::BlindRecovery(recovery) => recovery
                .retained_authority()
                .and_then(ExposureState::pending_entry)
                .is_some_and(|pending| {
                    pending.client_order_id == client_order_id
                        && pending.instrument_id == managed.instrument_id
                }),
            ExposureState::Flat
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => false,
        };
        if !matching_pending {
            return match &self.state {
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::BlindRecovery(_) => self.record_identity_conflict(managed),
                ExposureState::Flat
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            };
        }
        self.identity_conflict = None;
        match &mut self.state {
            ExposureState::PendingEntry(_) | ExposureState::EntryReconcilePending { .. } => {
                self.state = ExposureState::Managed(managed);
            }
            ExposureState::BlindRecovery(recovery) => {
                if let Some(retained) = recovery.retained_authority_mut() {
                    **retained = ExposureState::Managed(managed);
                }
            }
            ExposureState::Flat
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => unreachable!("matching state checked"),
        }
        true
    }

    fn reduce_authenticated_episode_rebase(
        &mut self,
        before: PositionEpisodeFingerprint,
        authenticated_order_id: ClientOrderId,
        authenticated_fill_id: TradeId,
        rebased: Option<ManagedPositionContext>,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        if authenticated_order_id != before.opening_order_id {
            return false;
        }
        let retained_fill_ids = self.episode_fill_ids(&before);
        if !retained_fill_ids.contains(&authenticated_fill_id) {
            return false;
        }

        if rebased
            .as_ref()
            .is_some_and(|rebased| self.replay_segment_continuous(&before, rebased))
        {
            let Some(mut rebased) = rebased else {
                return false;
            };
            if rebased.episode.instrument_id != before.instrument_id
                || rebased.episode.position_id != before.position_id
            {
                return false;
            }
            rebased.episode_close_seen = false;
            rebased.canonical_none_seen = false;
            let after = rebased.episode.clone();
            let mut changed = rebase_state_episode(&mut self.state, &before, &rebased);
            for provenance in self.released_exits.values_mut() {
                if provenance.episode == before {
                    provenance.episode = after.clone();
                    provenance.position = rebased.clone();
                    changed = true;
                }
            }
            for obligation in self.obligations.values_mut() {
                if obligation.provenance.episode == before {
                    obligation.provenance.episode = after.clone();
                    obligation.provenance.position = rebased.clone();
                    changed = true;
                }
            }
            changed
        } else {
            let retained_episode = before.clone();
            let adopted = rebased.clone();
            let mut changed = correction_close_state_episode(&mut self.state, &before);
            let released_before = self.released_exits.len();
            self.released_exits
                .retain(|_, provenance| provenance.episode != before);
            changed |= self.released_exits.len() != released_before;
            let obligations_before = self.obligations.len();
            self.obligations
                .retain(|_, obligation| obligation.provenance.episode != before);
            changed |= self.obligations.len() != obligations_before;
            if let Some(adopted) = adopted
                && adopted.episode != retained_episode
                && matches!(self.state, ExposureState::Flat)
            {
                changed |= self.apply_managed_truth(adopted.clone(), replacement_adoption);
                *replacement_adoption = Some(ReplacementAdoption {
                    retained_episode,
                    adopted,
                    cause: ReplacementAdoptionCause::AuthenticatedCorrection,
                });
            }
            changed
        }
    }

    fn episode_fill_ids(&self, episode: &PositionEpisodeFingerprint) -> BTreeSet<TradeId> {
        let mut fill_ids = BTreeSet::new();
        collect_state_episode_fill_ids(&self.state, episode, &mut fill_ids);
        for provenance in self.released_exits.values() {
            if &provenance.episode == episode {
                fill_ids.extend(provenance.position.episode_fill_ids.iter().copied());
            }
        }
        for obligation in self.obligations.values() {
            if &obligation.provenance.episode == episode {
                fill_ids.extend(
                    obligation
                        .provenance
                        .position
                        .episode_fill_ids
                        .iter()
                        .copied(),
                );
            }
        }
        fill_ids
    }

    fn replay_segment_continuous(
        &self,
        before: &PositionEpisodeFingerprint,
        rebased: &ManagedPositionContext,
    ) -> bool {
        if rebased.episode.instrument_id != before.instrument_id
            || rebased.episode.position_id != before.position_id
        {
            return false;
        }
        let retained = self.replay_segment(before);
        retained
            .iter()
            .any(|fragment| rebased.replay_segment.contains(fragment))
    }

    fn replay_segment(
        &self,
        episode: &PositionEpisodeFingerprint,
    ) -> Vec<PositionReplayFragmentIdentity> {
        let mut segment = Vec::new();
        collect_state_replay_segment(&self.state, episode, &mut segment);
        for provenance in self.released_exits.values() {
            if &provenance.episode == episode {
                extend_unique_replay_segment(&mut segment, &provenance.position.replay_segment);
            }
        }
        for obligation in self.obligations.values() {
            if &obligation.provenance.episode == episode {
                extend_unique_replay_segment(
                    &mut segment,
                    &obligation.provenance.position.replay_segment,
                );
            }
        }
        segment
    }

    fn record_identity_conflict(&mut self, candidate: ManagedPositionContext) -> bool {
        let retained_episode = self
            .state
            .tracked_position_context()
            .map(|context| context.episode.clone());
        self.identity_conflict = Some(IdentityConflict {
            retained: self.state.kind(),
            retained_episode,
            candidate,
        });
        true
    }

    fn apply_managed_truth(
        &mut self,
        managed: ManagedPositionContext,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        match &mut self.state {
            ExposureState::Flat => {
                self.identity_conflict = None;
                self.state = ExposureState::Managed(managed);
                true
            }
            ExposureState::PendingEntry(_) | ExposureState::EntryReconcilePending { .. } => {
                self.record_identity_conflict(managed)
            }
            ExposureState::Managed(current) if current.episode == managed.episode => {
                let mut managed = managed;
                managed.episode_close_seen = current.episode_close_seen;
                managed.canonical_none_seen = current.canonical_none_seen;
                *current = managed;
                self.identity_conflict = None;
                true
            }
            ExposureState::ExitAttempting(attempt)
                if attempt.managed.episode == managed.episode =>
            {
                let mut managed = managed;
                managed.episode_close_seen = attempt.managed.episode_close_seen;
                managed.canonical_none_seen = attempt.managed.canonical_none_seen;
                attempt.managed = managed;
                self.identity_conflict = None;
                true
            }
            ExposureState::ExitPending(exit)
            | ExposureState::TerminalExitAwaitingPosition(exit)
                if exit.episode() == managed.episode =>
            {
                let mut managed = managed;
                managed.episode_close_seen = exit.position.episode_close_seen;
                managed.canonical_none_seen = exit.position.canonical_none_seen;
                exit.position = managed;
                self.identity_conflict = None;
                true
            }
            ExposureState::ExitAuthorityRecoveryHold(hold)
                if hold.position.episode == managed.episode =>
            {
                let mut managed = managed;
                managed.episode_close_seen = hold.position.episode_close_seen;
                managed.canonical_none_seen = hold.position.canonical_none_seen;
                hold.position = managed;
                self.identity_conflict = None;
                true
            }
            ExposureState::UnsupportedObserved(observed)
                if observed.context.episode == managed.episode =>
            {
                let mut managed = managed;
                managed.episode_close_seen = observed.context.episode_close_seen;
                managed.canonical_none_seen = observed.context.canonical_none_seen;
                observed.context = managed;
                self.identity_conflict = None;
                true
            }
            ExposureState::BlindRecovery(recovery) if recovery.authorizes_exactly_one(&managed) => {
                if let Some(retained) = recovery.retained_authority().cloned() {
                    if let ExposureState::ReplacementConflict(conflict) = retained {
                        if conflict.retained.episode == managed.episode {
                            let mut retained = managed;
                            retained.episode_close_seen = false;
                            retained.canonical_none_seen = false;
                            self.state = ExposureState::Managed(retained);
                            self.identity_conflict = None;
                            return true;
                        }
                        if conflict.candidate.episode == managed.episode
                            && conflict.retained_is_closed()
                        {
                            *replacement_adoption = Some(ReplacementAdoption {
                                retained_episode: conflict.retained.episode.clone(),
                                adopted: managed.clone(),
                                cause: ReplacementAdoptionCause::CanonicalCloseConjunction,
                            });
                            self.state = ExposureState::Managed(managed);
                            self.identity_conflict = None;
                            return true;
                        }
                        return false;
                    }
                    self.state = retained;
                    if self.refresh_context(managed.clone()) {
                        true
                    } else if self.state.pending_entry().is_some_and(|pending| {
                        pending.instrument_id == managed.instrument_id
                            && pending.client_order_id == managed.episode.opening_order_id
                    }) {
                        self.state = ExposureState::Managed(managed);
                        true
                    } else {
                        false
                    }
                } else {
                    self.state = ExposureState::Managed(managed);
                    true
                }
            }
            ExposureState::Managed(current) => {
                self.state = if current.episode_close_seen {
                    self.identity_conflict = Some(IdentityConflict {
                        retained: ExposureStateKind::Managed,
                        retained_episode: Some(current.episode.clone()),
                        candidate: managed.clone(),
                    });
                    *replacement_adoption = Some(ReplacementAdoption {
                        retained_episode: current.episode.clone(),
                        adopted: managed.clone(),
                        cause: ReplacementAdoptionCause::CanonicalCloseConjunction,
                    });
                    ExposureState::Managed(managed)
                } else {
                    ExposureState::ReplacementConflict(Box::new(ReplacementConflictState {
                        retained: current.clone(),
                        candidate: managed,
                        retained_close: ReplacementRetainedCloseProof::Awaiting,
                        candidate_projection: ReplacementCandidateProjection::Matching,
                    }))
                };
                true
            }
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_) => self.record_identity_conflict(managed),
            ExposureState::BlindRecovery(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => false,
        }
    }

    fn reduce_position_closed(
        &mut self,
        event: PositionClosedEvent,
        replacement_adoption: &mut Option<ReplacementAdoption>,
    ) -> bool {
        let PositionClosedEvent::ObservedWithFreshProjection {
            expected_generation,
            episode,
            projection,
        } = event;
        if expected_generation != self.generation {
            return false;
        }

        if let ExposureState::ReplacementConflict(conflict) = self.state.clone() {
            let resolution = (*conflict).transition(ReplacementConflictEvent::RetainedClosed {
                episode,
                projection: projection.into(),
            });
            let (state, adoption) =
                resolution.into_transition(ReplacementConflictContainer::Direct);
            let changed = state != self.state;
            self.state = state;
            *replacement_adoption = adoption;
            return changed;
        }

        if let ExposureState::BlindRecovery(recovery) = self.state.clone()
            && let Some(ExposureState::ReplacementConflict(conflict)) =
                recovery.retained_authority().cloned()
        {
            let resolution = (*conflict).transition(ReplacementConflictEvent::RetainedClosed {
                episode,
                projection: projection.into(),
            });
            let (state, adoption) =
                resolution.into_transition(ReplacementConflictContainer::BlindRecovery(recovery));
            let changed = state != self.state;
            self.state = state;
            *replacement_adoption = adoption;
            return changed;
        }

        let close_changed = match &mut self.state {
            ExposureState::Managed(context) if context.episode == episode => {
                context.episode_close_seen = true;
                true
            }
            ExposureState::UnsupportedObserved(observed) if observed.context.episode == episode => {
                observed.context.episode_close_seen = true;
                true
            }
            ExposureState::ExitAttempting(attempt) if attempt.managed.episode == episode => {
                attempt.managed.episode_close_seen = true;
                true
            }
            ExposureState::ExitPending(exit)
            | ExposureState::TerminalExitAwaitingPosition(exit)
                if exit.episode() == episode =>
            {
                exit.position.episode_close_seen = true;
                true
            }
            ExposureState::ExitAuthorityRecoveryHold(hold) if hold.episode() == episode => {
                hold.position.episode_close_seen = true;
                true
            }
            ExposureState::BlindRecovery(recovery) => recovery
                .retained_authority_mut()
                .is_some_and(|retained| observe_position_close(retained, &episode)),
            ExposureState::ObligationSaturated(saturated) => {
                observe_position_close(&mut saturated.retained, &episode)
            }
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ReplacementConflict(_) => false,
        };
        let projection_changed = match projection {
            FreshCanonicalPositionProjection::None => self.reduce_canonical_projection(
                CanonicalPositionProjection::None,
                replacement_adoption,
            ),
            FreshCanonicalPositionProjection::ExactlyOne(classified) => match *classified {
                ClassifiedOpenPosition::Managed(managed) => {
                    if self.state.pending_entry().is_some_and(|pending| {
                        pending.instrument_id == managed.instrument_id
                            && pending.client_order_id == managed.episode.opening_order_id
                    }) {
                        self.reduce_position_truth(
                            PositionTruthEvent::EntryTerminalMaterialization {
                                client_order_id: managed.episode.opening_order_id,
                                managed,
                            },
                        )
                    } else {
                        self.reduce_canonical_projection(
                            CanonicalPositionProjection::ExactlyOne(Box::new(managed)),
                            replacement_adoption,
                        )
                    }
                }
                ClassifiedOpenPosition::Unsupported(unsupported) => {
                    self.reduce_position_truth(PositionTruthEvent::Unsupported(unsupported))
                }
                ClassifiedOpenPosition::BlindRecovery(recovery) => {
                    self.enter_blind_recovery(recovery)
                }
            },
            FreshCanonicalPositionProjection::Multiple { count } => self
                .reduce_canonical_projection(
                    CanonicalPositionProjection::Multiple {
                        count,
                        recovery: BlindRecoveryState::probe(
                            BlindRecoveryProbeReason::MultipleOpenPositions { count },
                        ),
                    },
                    replacement_adoption,
                ),
            FreshCanonicalPositionProjection::ProbeFailed { diagnostic } => self
                .reduce_canonical_projection(
                    CanonicalPositionProjection::ProbeFailed {
                        diagnostic,
                        recovery: BlindRecoveryState::probe(
                            BlindRecoveryProbeReason::CacheProbeFailed,
                        ),
                    },
                    replacement_adoption,
                ),
        };
        projection_changed || close_changed
    }

    fn reduce_timer_reconciliation(&mut self, event: TimerReconciliationEvent) -> bool {
        match event {
            TimerReconciliationEvent::Pending(state) => {
                self.reduce_exit_lifecycle(ExitLifecycleEvent::Pending(state))
            }
            TimerReconciliationEvent::TerminalAwaitingPosition(state) => {
                self.reduce_exit_lifecycle(ExitLifecycleEvent::TerminalAwaitingPosition(state))
            }
            TimerReconciliationEvent::RecoveryHold(state) => {
                self.reduce_exit_lifecycle(ExitLifecycleEvent::RecoveryHold(state))
            }
            TimerReconciliationEvent::ReleaseFlat => {
                self.reduce_exit_lifecycle(ExitLifecycleEvent::ReleaseFlat)
            }
            TimerReconciliationEvent::BlindRecovery(state) => self.enter_blind_recovery(state),
            TimerReconciliationEvent::SinkUnknown(resolution) => match &mut self.state {
                ExposureState::OperationSinkUnknown(unknown) => {
                    let next = sink_unknown_resolution_state(unknown, resolution);
                    self.replace(next)
                }
                ExposureState::BlindRecovery(recovery) => {
                    let Some(retained) = recovery.retained_authority_mut() else {
                        return false;
                    };
                    let ExposureState::OperationSinkUnknown(unknown) = &**retained else {
                        return false;
                    };
                    **retained = sink_unknown_resolution_state(unknown, resolution);
                    true
                }
                ExposureState::ObligationSaturated(saturated) => {
                    let ExposureState::OperationSinkUnknown(unknown) = &*saturated.retained else {
                        return false;
                    };
                    *saturated.retained = sink_unknown_resolution_state(unknown, resolution);
                    true
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
        }
    }

    fn reduce_bootstrap_adoption(&mut self, event: BootstrapAdoptionEvent) -> bool {
        match event {
            BootstrapAdoptionEvent::Flat => self.replace(ExposureState::Flat),
            BootstrapAdoptionEvent::Managed(state) => match &self.state {
                ExposureState::Flat | ExposureState::BlindRecovery(_) => {
                    self.replace(ExposureState::Managed(state))
                }
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            BootstrapAdoptionEvent::Unsupported(state) => match &self.state {
                ExposureState::Flat | ExposureState::BlindRecovery(_) => {
                    self.replace(ExposureState::UnsupportedObserved(state))
                }
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            BootstrapAdoptionEvent::ExitPending(state) => match &self.state {
                ExposureState::Flat
                | ExposureState::Managed(_)
                | ExposureState::BlindRecovery(_) => {
                    self.replace(ExposureState::ExitPending(state))
                }
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            BootstrapAdoptionEvent::BlindRecovery(state) => self.enter_blind_recovery(state),
        }
    }

    fn reduce_settlement_effect(&mut self, event: SettlementEffectEvent) -> bool {
        match event {
            SettlementEffectEvent::ReleaseFlat { episode } => match &self.state {
                ExposureState::Managed(managed) if managed.episode == episode => {
                    self.replace(ExposureState::Flat)
                }
                ExposureState::UnsupportedObserved(unsupported)
                    if unsupported.context.episode == episode =>
                {
                    self.replace(ExposureState::Flat)
                }
                ExposureState::Flat
                | ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::Managed(_)
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            },
            SettlementEffectEvent::BlindRecovery(state) => self.enter_blind_recovery(state),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationArmPhase {
    Provisional,
    Consumed,
    SinkInvoked,
}

#[derive(Debug, Clone, PartialEq)]
struct OperationArm {
    operation: ExposureOperationKind,
    generation: u64,
    prior_generation: u64,
    prior: ExposureState,
    payload: Option<RouteOperationPayload>,
    phase: OperationArmPhase,
}

pub(super) struct GovernedExposure {
    inner: Rc<RefCell<GovernedExposureInner>>,
}

impl std::fmt::Debug for GovernedExposure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("GovernedExposure")
            .field(&*self.inner.borrow())
            .finish()
    }
}

impl Clone for GovernedExposure {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::new(RefCell::new(self.inner.borrow().clone())),
        }
    }
}

impl PartialEq for GovernedExposure {
    fn eq(&self, other: &Self) -> bool {
        *self.inner.borrow() == *other.inner.borrow()
    }
}

impl GovernedExposure {
    pub(super) fn new(limits: ExposureObligationLimits) -> Self {
        Self {
            inner: Rc::new(RefCell::new(GovernedExposureInner::new(limits))),
        }
    }

    pub(super) fn state(&self) -> ExposureState {
        self.inner.borrow().state.clone()
    }

    pub(super) fn generation(&self) -> u64 {
        self.inner.borrow().generation
    }

    #[cfg(test)]
    pub(super) fn last_outcome(&self) -> ExposureTransitionOutcome {
        self.inner.borrow().last_outcome
    }

    #[cfg(test)]
    pub(super) fn quarantined_order(&self, client_order_id: &ClientOrderId) -> bool {
        self.inner
            .borrow()
            .quarantined_orders
            .contains_key(client_order_id)
    }

    pub(super) fn limits(&self) -> ExposureObligationLimits {
        self.inner.borrow().limits
    }

    pub(super) fn identity_conflict(&self) -> Option<IdentityConflict> {
        self.inner.borrow().identity_conflict.clone()
    }

    pub(super) fn released_exit(
        &self,
        client_order_id: &ClientOrderId,
    ) -> Option<ReleasedExitProvenance> {
        self.inner
            .borrow()
            .released_exits
            .get(client_order_id)
            .cloned()
    }

    pub(super) fn released_exit_for_episode(
        &self,
        episode: &PositionEpisodeFingerprint,
    ) -> Option<ReleasedExitProvenance> {
        self.inner
            .borrow()
            .released_exits
            .values()
            .find(|provenance| &provenance.episode == episode)
            .cloned()
    }

    #[cfg(test)]
    pub(super) fn released_exit_count(&self) -> usize {
        self.inner.borrow().released_exits.len()
    }

    pub(super) fn authenticated_episode_for_fill_void(
        &self,
        client_order_id: ClientOrderId,
        trade_id: TradeId,
    ) -> Option<PositionEpisodeFingerprint> {
        let inner = self.inner.borrow();
        let mut episodes = Vec::new();
        collect_state_episode_identities(&inner.state, client_order_id, trade_id, &mut episodes);
        for provenance in inner.released_exits.values() {
            collect_context_episode_identity(
                &provenance.position,
                client_order_id,
                trade_id,
                &mut episodes,
            );
        }
        for obligation in inner.obligations.values() {
            collect_context_episode_identity(
                &obligation.provenance.position,
                client_order_id,
                trade_id,
                &mut episodes,
            );
        }
        (episodes.len() == 1).then(|| episodes.remove(0))
    }

    pub(super) fn ready_historical_exit_obligation(&self) -> Option<DeferredExitObligation> {
        let inner = self.inner.borrow();
        inner
            .obligations
            .values()
            .find(|obligation| match &inner.state {
                ExposureState::Flat => true,
                ExposureState::Managed(managed) => managed.episode == obligation.provenance.episode,
                ExposureState::PendingEntry(_)
                | ExposureState::EntryReconcilePending { .. }
                | ExposureState::ExitAttempting(_)
                | ExposureState::ExitPending(_)
                | ExposureState::TerminalExitAwaitingPosition(_)
                | ExposureState::ExitAuthorityRecoveryHold(_)
                | ExposureState::UnsupportedObserved(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::OperationSinkUnknown(_)
                | ExposureState::ObligationSaturated(_)
                | ExposureState::ReplacementConflict(_) => false,
            })
            .cloned()
    }

    pub(super) fn deferred_obligation(
        &self,
        client_order_id: &ClientOrderId,
    ) -> Option<DeferredExitObligation> {
        self.inner
            .borrow()
            .obligations
            .get(client_order_id)
            .cloned()
    }

    pub(super) fn reduce<Output>(
        &self,
        event: impl Into<ExposureReductionRequest<Output>>,
    ) -> Output {
        event.into().apply(&mut self.inner.borrow_mut())
    }

    pub(super) fn saturate_with_current_state(
        &self,
        client_order_id: ClientOrderId,
    ) -> ExposureTransitionOutcome {
        self.reduce(ExposureEvent::UntrackedOrder(
            UntrackedOrderEvent::ObligationSaturated { client_order_id },
        ))
    }

    pub(super) fn pending_entry(&self) -> Option<PendingEntryState> {
        self.inner.borrow().state.pending_entry().cloned()
    }

    pub(super) fn managed_position_context(&self) -> Option<ManagedPositionContext> {
        self.inner
            .borrow()
            .state
            .managed_position_context()
            .cloned()
    }

    pub(super) fn tracked_position_context(&self) -> Option<ManagedPositionContext> {
        self.inner
            .borrow()
            .state
            .tracked_position_context()
            .cloned()
    }

    pub(super) fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.inner.borrow().state.held_instrument_id()
    }

    pub(super) fn exit_pending_snapshot(&self) -> Option<ExitPendingState> {
        self.inner.borrow().state.exit_pending_snapshot()
    }

    pub(super) fn exit_lifecycle(&self) -> Option<(ExitLifecyclePhase, ExitPendingState)> {
        self.inner.borrow().state.exit_lifecycle()
    }

    pub(super) fn exit_authority_recovery_hold(&self) -> Option<ExitAuthorityRecoveryHoldState> {
        self.inner
            .borrow()
            .state
            .exit_authority_recovery_hold()
            .cloned()
    }

    pub(super) fn operation_sink_unknown(&self) -> Option<OperationSinkUnknownState> {
        self.inner.borrow().state.operation_sink_unknown().cloned()
    }

    pub(super) fn occupancy(&self) -> Option<ExposureOccupancy> {
        let inner = self.inner.borrow();
        if inner.operation_arm.is_some() {
            return Some(ExposureOccupancy::BlindRecovery);
        }
        inner.state.occupancy()
    }

    #[cfg(test)]
    pub(super) fn blocks_new_entries(&self) -> bool {
        let inner = self.inner.borrow();
        inner.operation_arm.is_some() || inner.state.blocks_new_entries()
    }

    pub(super) fn is_recovering(&self) -> bool {
        self.inner.borrow().state.is_recovering()
    }

    pub(super) fn current_position_market_id(&self) -> Option<String> {
        self.inner.borrow().state.current_position_market_id()
    }

    pub(super) fn request_entry_operation(
        &self,
        expected_generation: u64,
    ) -> Result<EntryOperationGrant, ExposureOperationRejection> {
        self.request_operation(ExposureOperationKind::EntryRoute, expected_generation)
            .map(EntryOperationGrant)
    }

    pub(super) fn inspect_entry_operation(&self) -> ExposureOperationDecision {
        self.inspect_operation(ExposureOperationKind::EntryRoute)
    }

    pub(super) fn request_exit_operation(
        &self,
        expected_generation: u64,
    ) -> Result<ExitOperationGrant, ExposureOperationRejection> {
        self.request_operation(ExposureOperationKind::ExitRoute, expected_generation)
            .map(ExitOperationGrant)
    }

    pub(super) fn inspect_exit_operation(&self) -> ExposureOperationDecision {
        self.inspect_operation(ExposureOperationKind::ExitRoute)
    }

    pub(super) fn request_bootstrap_operation(
        &self,
        expected_generation: u64,
    ) -> Result<BootstrapOperationGrant, ExposureOperationRejection> {
        self.request_operation(ExposureOperationKind::Bootstrap, expected_generation)
            .map(BootstrapOperationGrant)
    }

    pub(super) fn request_recovery_operation(
        &self,
        expected_generation: u64,
    ) -> Result<RecoveryOperationGrant, ExposureOperationRejection> {
        let restart_adoption = matches!(
            &self.inner.borrow().state,
            ExposureState::BlindRecovery(recovery) if recovery.is_restart_adoption()
        );
        self.request_operation(ExposureOperationKind::Recovery, expected_generation)
            .map(|grant| RecoveryOperationGrant {
                grant,
                restart_adoption,
            })
    }

    pub(super) fn request_correction_operation(
        &self,
        expected_generation: u64,
    ) -> Result<CorrectionOperationGrant, ExposureOperationRejection> {
        self.request_operation(ExposureOperationKind::Correction, expected_generation)
            .map(CorrectionOperationGrant)
    }

    fn inspect_operation(&self, operation: ExposureOperationKind) -> ExposureOperationDecision {
        let inner = self.inner.borrow();
        classify_operation(&inner, operation, inner.generation)
    }

    fn request_operation(
        &self,
        operation: ExposureOperationKind,
        expected_generation: u64,
    ) -> Result<ExposureOperationGrant, ExposureOperationRejection> {
        let mut inner = self.inner.borrow_mut();
        let state = inner.state.kind();
        let rejection = |reason, current_generation| ExposureOperationRejection {
            operation,
            reason,
            state,
            requested_generation: expected_generation,
            current_generation,
        };
        if let Some(reason) = classify_operation(&inner, operation, expected_generation).rejection {
            return Err(rejection(reason, inner.generation));
        }
        let prior_generation = inner.generation;
        let generation = prior_generation
            .checked_add(1)
            .expect("validated exposure generation space exhausted");
        inner.generation = generation;
        inner.operation_arm = Some(OperationArm {
            operation,
            generation,
            prior_generation,
            prior: inner.state.clone(),
            payload: None,
            phase: OperationArmPhase::Provisional,
        });
        Ok(ExposureOperationGrant {
            inner: Rc::downgrade(&self.inner),
            operation,
            generation,
            complete: false,
        })
    }
}

fn classify_operation(
    inner: &GovernedExposureInner,
    operation: ExposureOperationKind,
    expected_generation: u64,
) -> ExposureOperationDecision {
    let rejection = match (
        inner.operation_arm.is_some(),
        expected_generation == inner.generation,
    ) {
        (true, _) => Some(ExposureOperationBlockedReason::OperationAlreadyArmed),
        (false, false) => Some(ExposureOperationBlockedReason::StaleGeneration),
        (false, true) => (!inner.state.allows_operation(operation))
            .then(|| blocked_reason_for_state(&inner.state)),
    };
    ExposureOperationDecision {
        generation: expected_generation,
        rejection,
    }
}

fn blocked_reason_for_state(state: &ExposureState) -> ExposureOperationBlockedReason {
    match state {
        ExposureState::Flat => ExposureOperationBlockedReason::Unoccupied,
        ExposureState::PendingEntry(_) => ExposureOperationBlockedReason::PendingEntryOccupied,
        ExposureState::EntryReconcilePending { .. } => {
            ExposureOperationBlockedReason::EntryReconcileOccupied
        }
        ExposureState::Managed(_) => ExposureOperationBlockedReason::ManagedOccupied,
        ExposureState::ExitAttempting(_) => ExposureOperationBlockedReason::ExitAttemptOccupied,
        ExposureState::ExitPending(_) | ExposureState::TerminalExitAwaitingPosition(_) => {
            ExposureOperationBlockedReason::ExitPendingOccupied
        }
        ExposureState::ExitAuthorityRecoveryHold(_) => {
            ExposureOperationBlockedReason::RecoveryHoldOccupied
        }
        ExposureState::UnsupportedObserved(_) => {
            ExposureOperationBlockedReason::UnsupportedOccupied
        }
        ExposureState::BlindRecovery(_) => ExposureOperationBlockedReason::BlindRecoveryOccupied,
        ExposureState::OperationSinkUnknown(_) => {
            ExposureOperationBlockedReason::SinkUnknownOccupied
        }
        ExposureState::ObligationSaturated(_) => {
            ExposureOperationBlockedReason::ObligationSaturated
        }
        ExposureState::ReplacementConflict(_) => {
            ExposureOperationBlockedReason::ReplacementConflictOccupied
        }
    }
}

#[derive(Debug)]
struct ExposureOperationGrant {
    inner: Weak<RefCell<GovernedExposureInner>>,
    operation: ExposureOperationKind,
    generation: u64,
    complete: bool,
}

impl ExposureOperationGrant {
    fn bind_route_payload(&mut self, payload: RouteOperationPayload) -> Result<()> {
        anyhow::ensure!(
            payload.operation() == self.operation,
            "exposure route grant operation does not match its payload"
        );
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("exposure authority no longer exists"))?;
        let mut inner = inner.borrow_mut();
        let Some(arm) = inner.operation_arm.as_mut() else {
            anyhow::bail!("exposure route grant is stale");
        };
        anyhow::ensure!(
            arm.generation == self.generation && arm.operation == self.operation,
            "exposure route grant generation is stale"
        );
        anyhow::ensure!(
            arm.payload.is_none(),
            "exposure route grant is already bound"
        );
        arm.payload = Some(payload);
        Ok(())
    }

    fn rollback(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut inner = inner.borrow_mut();
        let Some(arm) = inner.operation_arm.as_ref() else {
            return;
        };
        if arm.generation != self.generation || arm.operation != self.operation {
            return;
        }
        let arm = inner
            .operation_arm
            .take()
            .expect("matching exposure operation arm must still exist");
        match arm.phase {
            OperationArmPhase::Provisional | OperationArmPhase::Consumed => {
                inner.replace_from_operation(arm.prior);
                inner.generation = arm.prior_generation;
            }
            OperationArmPhase::SinkInvoked => {
                let payload = arm
                    .payload
                    .expect("sink-invoked exposure operation must carry its route payload");
                inner.replace_from_operation(ExposureState::OperationSinkUnknown(
                    OperationSinkUnknownState {
                        operation: arm.operation,
                        generation: arm.generation,
                        client_order_id: payload.client_order_id(),
                        attempted: payload,
                        prior: Box::new(arm.prior),
                    },
                ));
                inner.generation = arm
                    .generation
                    .checked_add(1)
                    .expect("validated exposure generation space exhausted");
            }
        }
    }

    fn commit_reducer_event<Output>(
        &mut self,
        event: impl Into<ExposureReductionRequest<Output>>,
    ) -> Output {
        let request = event.into();
        let inner = self
            .inner
            .upgrade()
            .expect("exposure authority must outlive reducer operation grant");
        let mut inner = inner.borrow_mut();
        if !inner.operation_arm.as_ref().is_some_and(|arm| {
            arm.generation == self.generation
                && arm.operation == self.operation
                && arm.phase == OperationArmPhase::Provisional
        }) {
            self.complete = true;
            return request.preserved(inner.state.kind());
        }
        let outcome = request.apply(&mut inner);
        if inner
            .operation_arm
            .as_ref()
            .is_some_and(|arm| arm.generation == self.generation && arm.operation == self.operation)
        {
            let arm = inner
                .operation_arm
                .take()
                .expect("matching reducer operation arm must still exist");
            inner.replace_from_operation(arm.prior);
            inner.generation = arm.prior_generation;
        }
        self.complete = true;
        outcome
    }
}

impl Drop for ExposureOperationGrant {
    fn drop(&mut self) {
        if !self.complete {
            self.rollback();
        }
    }
}

impl BoltV3RouteAttemptParticipant for ExposureOperationGrant {
    fn consume_at_pre_sink(&mut self) -> Result<()> {
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("exposure authority no longer exists"))?;
        let mut inner = inner.borrow_mut();
        let Some(arm) = inner.operation_arm.as_mut() else {
            anyhow::bail!("exposure route grant is stale");
        };
        anyhow::ensure!(
            arm.generation == self.generation && arm.operation == self.operation,
            "exposure route grant generation is stale"
        );
        anyhow::ensure!(
            arm.phase == OperationArmPhase::Provisional,
            "exposure route grant was consumed more than once"
        );
        let payload = arm
            .payload
            .clone()
            .ok_or_else(|| anyhow::anyhow!("exposure route grant has no bound payload"))?;
        arm.phase = OperationArmPhase::Consumed;
        let next = match payload {
            RouteOperationPayload::Entry(pending) => ExposureState::PendingEntry(*pending),
            RouteOperationPayload::Exit(attempt) => ExposureState::ExitAttempting(*attempt),
        };
        inner.replace_from_operation(next);
        Ok(())
    }

    fn mark_sink_invoked(&mut self, _actor_now_ns: u64) -> Result<()> {
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        let mut inner = inner.borrow_mut();
        if let Some(arm) = inner.operation_arm.as_mut()
            && arm.generation == self.generation
            && arm.operation == self.operation
            && arm.phase == OperationArmPhase::Consumed
        {
            arm.phase = OperationArmPhase::SinkInvoked;
        }
        Ok(())
    }

    fn complete(&mut self, completion: BoltV3RouteAttemptCompletion) {
        let Some(inner) = self.inner.upgrade() else {
            self.complete = true;
            return;
        };
        let mut inner = inner.borrow_mut();
        let Some(arm) = inner.operation_arm.as_ref() else {
            self.complete = true;
            return;
        };
        if arm.generation != self.generation || arm.operation != self.operation {
            self.complete = true;
            return;
        }
        let arm = inner
            .operation_arm
            .take()
            .expect("matching exposure operation arm must still exist");
        match completion {
            BoltV3RouteAttemptCompletion::Submitted => {
                if let RouteOperationPayload::Exit(attempt) = arm
                    .payload
                    .expect("submitted exposure operation must carry its route payload")
                {
                    inner.replace_from_operation(ExposureState::ExitPending(
                        (*attempt).into_pending(),
                    ));
                }
                inner.generation = arm
                    .generation
                    .checked_add(1)
                    .expect("validated exposure generation space exhausted");
            }
            BoltV3RouteAttemptCompletion::SinkRejected => {
                let payload = arm
                    .payload
                    .expect("sink-rejected exposure operation must carry its route payload");
                inner.replace_from_operation(ExposureState::OperationSinkUnknown(
                    OperationSinkUnknownState {
                        operation: arm.operation,
                        generation: arm.generation,
                        client_order_id: payload.client_order_id(),
                        attempted: payload,
                        prior: Box::new(arm.prior),
                    },
                ));
                inner.generation = arm
                    .generation
                    .checked_add(1)
                    .expect("validated exposure generation space exhausted");
            }
        }
        self.complete = true;
    }
}

#[derive(Debug)]
pub(super) struct EntryOperationGrant(ExposureOperationGrant);

impl EntryOperationGrant {
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.0.generation
    }

    pub(super) fn bind(
        mut self,
        pending: PendingEntryState,
    ) -> Result<Box<dyn BoltV3RouteAttemptParticipant>> {
        self.0
            .bind_route_payload(RouteOperationPayload::Entry(Box::new(pending)))?;
        Ok(Box::new(self.0))
    }
}

#[derive(Debug)]
pub(super) struct ExitOperationGrant(ExposureOperationGrant);

impl ExitOperationGrant {
    pub(super) fn generation(&self) -> u64 {
        self.0.generation
    }

    pub(super) fn bind(
        mut self,
        attempt: ExitAttemptingState,
    ) -> Result<Box<dyn BoltV3RouteAttemptParticipant>> {
        self.0
            .bind_route_payload(RouteOperationPayload::Exit(Box::new(attempt)))?;
        Ok(Box::new(self.0))
    }
}

#[derive(Debug)]
pub(super) struct BootstrapOperationGrant(ExposureOperationGrant);

impl BootstrapOperationGrant {
    pub(super) fn commit(mut self, event: BootstrapAdoptionEvent) -> ExposureTransitionOutcome {
        self.0
            .commit_reducer_event(ExposureEvent::BootstrapAdoption(event))
    }
}

#[derive(Debug)]
pub(super) struct RecoveryOperationGrant {
    grant: ExposureOperationGrant,
    restart_adoption: bool,
}

#[must_use = "recovery commits must handle replacement and restart adoption"]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RecoveryOperationCommit {
    pub(super) outcome: ExposureTransitionOutcome,
    pub(super) replacement_adoption: Option<ReplacementAdoption>,
    pub(super) restart_adoption: bool,
}

impl RecoveryOperationGrant {
    pub(super) fn commit(
        mut self,
        projection: FreshCanonicalPositionProjection,
    ) -> RecoveryOperationCommit {
        let ExposureAdoptionCommit {
            outcome,
            replacement_adoption,
        } = self
            .grant
            .commit_reducer_event(AdoptionCapableExposureEvent::PositionTruth(
                AdoptionCapablePositionTruthEvent::AuthorizedRecovery(projection),
            ));
        RecoveryOperationCommit {
            outcome,
            replacement_adoption,
            restart_adoption: self.restart_adoption,
        }
    }
}

#[derive(Debug)]
pub(super) struct CorrectionOperationGrant(ExposureOperationGrant);

impl CorrectionOperationGrant {
    pub(super) fn commit(mut self, event: ExitLifecycleEvent) -> ExposureTransitionOutcome {
        self.0
            .commit_reducer_event(ExposureEvent::ExitLifecycle(event))
    }
}

fn refresh_replacement_candidate(
    retained: ManagedPositionContext,
    fresh: ManagedPositionContext,
) -> ManagedPositionContext {
    let ManagedPositionContext {
        episode,
        episode_fill_ids: mut fresh_fill_ids,
        replay_segment,
        lifecycle: _,
        instrument_id,
        position_id,
        book: _,
        origin: _,
        pending_entry: _,
        episode_close_seen: _,
        canonical_none_seen: _,
    } = fresh;
    let ManagedPositionContext {
        episode: _,
        episode_fill_ids,
        replay_segment: _,
        lifecycle,
        instrument_id: _,
        position_id: _,
        book,
        origin,
        pending_entry,
        episode_close_seen: _,
        canonical_none_seen: _,
    } = retained;
    fresh_fill_ids.extend(episode_fill_ids);
    ManagedPositionContext {
        episode,
        episode_fill_ids: fresh_fill_ids,
        replay_segment,
        lifecycle,
        instrument_id,
        position_id,
        book,
        origin,
        pending_entry,
        episode_close_seen: false,
        canonical_none_seen: false,
    }
}

fn observe_position_close(state: &mut ExposureState, episode: &PositionEpisodeFingerprint) -> bool {
    match state {
        ExposureState::Managed(context) if &context.episode == episode => {
            context.episode_close_seen = true;
            true
        }
        ExposureState::UnsupportedObserved(observed) if &observed.context.episode == episode => {
            observed.context.episode_close_seen = true;
            true
        }
        ExposureState::ExitAttempting(attempt) if &attempt.managed.episode == episode => {
            attempt.managed.episode_close_seen = true;
            true
        }
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit)
            if &exit.position.episode == episode =>
        {
            exit.position.episode_close_seen = true;
            true
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) if &hold.position.episode == episode => {
            hold.position.episode_close_seen = true;
            true
        }
        ExposureState::ReplacementConflict(conflict) => conflict.observe_retained_close(episode),
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. }
        | ExposureState::Managed(_)
        | ExposureState::ExitAttempting(_)
        | ExposureState::ExitPending(_)
        | ExposureState::TerminalExitAwaitingPosition(_)
        | ExposureState::ExitAuthorityRecoveryHold(_)
        | ExposureState::UnsupportedObserved(_)
        | ExposureState::BlindRecovery(_)
        | ExposureState::OperationSinkUnknown(_)
        | ExposureState::ObligationSaturated(_) => false,
    }
}

fn collect_state_episode_fill_ids(
    state: &ExposureState,
    episode: &PositionEpisodeFingerprint,
    fill_ids: &mut BTreeSet<TradeId>,
) {
    let mut collect = |context: &ManagedPositionContext| {
        if &context.episode == episode {
            fill_ids.extend(context.episode_fill_ids.iter().copied());
        }
    };
    match state {
        ExposureState::Managed(context) => collect(context),
        ExposureState::ExitAttempting(attempt) => collect(&attempt.managed),
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            collect(&exit.position);
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => collect(&hold.position),
        ExposureState::UnsupportedObserved(observed) => collect(&observed.context),
        ExposureState::BlindRecovery(recovery) => {
            if let Some(retained) = recovery.retained_authority() {
                collect_state_episode_fill_ids(retained, episode, fill_ids);
            }
        }
        ExposureState::OperationSinkUnknown(unknown) => {
            collect_state_episode_fill_ids(&unknown.prior, episode, fill_ids);
        }
        ExposureState::ObligationSaturated(saturated) => {
            collect_state_episode_fill_ids(&saturated.retained, episode, fill_ids);
        }
        ExposureState::ReplacementConflict(conflict) => {
            collect(&conflict.retained);
            collect(&conflict.candidate);
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. } => {}
    }
}

fn collect_state_replay_segment(
    state: &ExposureState,
    episode: &PositionEpisodeFingerprint,
    segment: &mut Vec<PositionReplayFragmentIdentity>,
) {
    let mut collect = |context: &ManagedPositionContext| {
        if &context.episode == episode {
            extend_unique_replay_segment(segment, &context.replay_segment);
        }
    };
    match state {
        ExposureState::Managed(context) => collect(context),
        ExposureState::ExitAttempting(attempt) => collect(&attempt.managed),
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            collect(&exit.position);
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => collect(&hold.position),
        ExposureState::UnsupportedObserved(observed) => collect(&observed.context),
        ExposureState::BlindRecovery(recovery) => {
            if let Some(retained) = recovery.retained_authority() {
                collect_state_replay_segment(retained, episode, segment);
            }
        }
        ExposureState::OperationSinkUnknown(unknown) => {
            collect_state_replay_segment(&unknown.prior, episode, segment);
        }
        ExposureState::ObligationSaturated(saturated) => {
            collect_state_replay_segment(&saturated.retained, episode, segment);
        }
        ExposureState::ReplacementConflict(conflict) => {
            collect(&conflict.retained);
            collect(&conflict.candidate);
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. } => {}
    }
}

fn extend_unique_replay_segment(
    target: &mut Vec<PositionReplayFragmentIdentity>,
    source: &[PositionReplayFragmentIdentity],
) {
    for fragment in source {
        if !target.contains(fragment) {
            target.push(fragment.clone());
        }
    }
}

fn collect_context_episode_identity(
    context: &ManagedPositionContext,
    client_order_id: ClientOrderId,
    trade_id: TradeId,
    episodes: &mut Vec<PositionEpisodeFingerprint>,
) {
    if context.episode.opening_order_id == client_order_id
        && context.episode_fill_ids.contains(&trade_id)
        && !episodes.contains(&context.episode)
    {
        episodes.push(context.episode.clone());
    }
}

fn collect_state_episode_identities(
    state: &ExposureState,
    client_order_id: ClientOrderId,
    trade_id: TradeId,
    episodes: &mut Vec<PositionEpisodeFingerprint>,
) {
    match state {
        ExposureState::Managed(context) => {
            collect_context_episode_identity(context, client_order_id, trade_id, episodes);
        }
        ExposureState::ExitAttempting(attempt) => {
            collect_context_episode_identity(&attempt.managed, client_order_id, trade_id, episodes)
        }
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            collect_context_episode_identity(&exit.position, client_order_id, trade_id, episodes);
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => {
            collect_context_episode_identity(&hold.position, client_order_id, trade_id, episodes)
        }
        ExposureState::UnsupportedObserved(observed) => {
            collect_context_episode_identity(&observed.context, client_order_id, trade_id, episodes)
        }
        ExposureState::BlindRecovery(recovery) => {
            if let Some(retained) = recovery.retained_authority() {
                collect_state_episode_identities(retained, client_order_id, trade_id, episodes);
            }
        }
        ExposureState::OperationSinkUnknown(unknown) => {
            collect_state_episode_identities(&unknown.prior, client_order_id, trade_id, episodes)
        }
        ExposureState::ObligationSaturated(saturated) => collect_state_episode_identities(
            &saturated.retained,
            client_order_id,
            trade_id,
            episodes,
        ),
        ExposureState::ReplacementConflict(conflict) => {
            collect_context_episode_identity(
                &conflict.retained,
                client_order_id,
                trade_id,
                episodes,
            );
            collect_context_episode_identity(
                &conflict.candidate,
                client_order_id,
                trade_id,
                episodes,
            );
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. } => {}
    }
}

fn rebase_managed_context(
    context: &mut ManagedPositionContext,
    before: &PositionEpisodeFingerprint,
    rebased: &ManagedPositionContext,
) -> bool {
    if &context.episode != before {
        return false;
    }
    let pending_entry = context.pending_entry.clone();
    *context = rebased.clone();
    context.pending_entry = pending_entry;
    context.episode_close_seen = false;
    context.canonical_none_seen = false;
    true
}

fn reduce_retained_entry_lifecycle(state: &mut ExposureState, event: EntryLifecycleEvent) -> bool {
    match event {
        #[cfg(test)]
        EntryLifecycleEvent::RestorePending(pending) => {
            if matches!(
                state,
                ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
            ) {
                *state = ExposureState::PendingEntry(pending);
                true
            } else {
                false
            }
        }
        EntryLifecycleEvent::Reconcile { pending, reason } => {
            if matches!(
                state,
                ExposureState::Flat
                    | ExposureState::PendingEntry(_)
                    | ExposureState::EntryReconcilePending { .. }
            ) {
                *state = ExposureState::EntryReconcilePending { pending, reason };
                true
            } else {
                false
            }
        }
        EntryLifecycleEvent::ReleaseFlat => {
            if matches!(
                state,
                ExposureState::PendingEntry(_) | ExposureState::EntryReconcilePending { .. }
            ) {
                *state = ExposureState::Flat;
                true
            } else {
                false
            }
        }
        EntryLifecycleEvent::ClearManagedPending {
            client_order_id,
            instrument_id,
        } => clear_managed_pending_in_state(state, client_order_id, instrument_id),
        EntryLifecycleEvent::RefreshPending(pending) => refresh_pending_in_state(state, pending),
    }
}

fn clear_managed_pending_in_state(
    state: &mut ExposureState,
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> bool {
    let context = match state {
        ExposureState::Managed(context) => Some(context),
        ExposureState::ExitAttempting(attempt) => Some(&mut attempt.managed),
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            Some(&mut exit.position)
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => Some(&mut hold.position),
        ExposureState::BlindRecovery(recovery) => {
            return recovery.retained_authority_mut().is_some_and(|retained| {
                clear_managed_pending_in_state(retained, client_order_id, instrument_id)
            });
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. }
        | ExposureState::UnsupportedObserved(_)
        | ExposureState::OperationSinkUnknown(_)
        | ExposureState::ObligationSaturated(_)
        | ExposureState::ReplacementConflict(_) => None,
    };
    let Some(context) = context else {
        return false;
    };
    if context.instrument_id != instrument_id
        || !context
            .pending_entry
            .as_ref()
            .is_some_and(|pending| pending.client_order_id == client_order_id)
    {
        return false;
    }
    context.pending_entry = None;
    true
}

fn refresh_pending_in_state(state: &mut ExposureState, pending: PendingEntryState) -> bool {
    match state {
        ExposureState::PendingEntry(current)
        | ExposureState::EntryReconcilePending {
            pending: current, ..
        } if current.client_order_id == pending.client_order_id
            && current.instrument_id == pending.instrument_id =>
        {
            *current = pending;
            true
        }
        ExposureState::Managed(context)
        | ExposureState::ExitAttempting(ExitAttemptingState {
            managed: context, ..
        }) => refresh_context_pending_entry(context, pending),
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            refresh_context_pending_entry(&mut exit.position, pending)
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => {
            refresh_context_pending_entry(&mut hold.position, pending)
        }
        ExposureState::BlindRecovery(recovery) => recovery
            .retained_authority_mut()
            .is_some_and(|retained| refresh_pending_in_state(retained, pending)),
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. }
        | ExposureState::UnsupportedObserved(_)
        | ExposureState::OperationSinkUnknown(_)
        | ExposureState::ObligationSaturated(_)
        | ExposureState::ReplacementConflict(_) => false,
    }
}

fn refresh_context_pending_entry(
    context: &mut ManagedPositionContext,
    pending: PendingEntryState,
) -> bool {
    let Some(current) = context.pending_entry.as_mut() else {
        return false;
    };
    if current.client_order_id != pending.client_order_id
        || current.instrument_id != pending.instrument_id
    {
        return false;
    }
    *current = pending;
    true
}

fn reduce_retained_exit_lifecycle(state: &mut ExposureState, event: ExitLifecycleEvent) -> bool {
    match event {
        ExitLifecycleEvent::Pending(pending) => {
            if matches!(
                state,
                ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
                    | ExposureState::Flat
            ) {
                *state = ExposureState::ExitPending(pending);
                true
            } else {
                false
            }
        }
        ExitLifecycleEvent::Working {
            expected_generation,
            observation,
            pending,
        } => match state {
            ExposureState::ExitAttempting(attempt)
                if attempt.generation == expected_generation
                    && attempt.authority.client_order_id() == pending.client_order_id() =>
            {
                *state = ExposureState::ExitPending(pending);
                true
            }
            ExposureState::ExitPending(current)
                if current.client_order_id() == pending.client_order_id() =>
            {
                *state = ExposureState::ExitPending(pending);
                true
            }
            ExposureState::TerminalExitAwaitingPosition(current)
                if observation == ExitWorkingObservation::Correction
                    && current.client_order_id() == pending.client_order_id() =>
            {
                *state = ExposureState::ExitPending(pending);
                true
            }
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => false,
        },
        ExitLifecycleEvent::TerminalAwaitingPosition(pending) => {
            if matches!(
                state,
                ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
            ) {
                *state = ExposureState::TerminalExitAwaitingPosition(pending);
                true
            } else {
                false
            }
        }
        ExitLifecycleEvent::RecoveryHold(hold) => {
            if state
                .tracked_position_context()
                .is_some_and(|context| context.episode == hold.position.episode)
            {
                *state = ExposureState::ExitAuthorityRecoveryHold(hold);
                true
            } else {
                false
            }
        }
        ExitLifecycleEvent::RefreshAuthority(authority) => match state {
            ExposureState::ExitPending(exit)
            | ExposureState::TerminalExitAwaitingPosition(exit)
                if exit.client_order_id() == authority.client_order_id() =>
            {
                if exit.authority == authority {
                    false
                } else {
                    exit.authority = authority;
                    true
                }
            }
            ExposureState::ExitAuthorityRecoveryHold(hold)
                if hold.client_order_id() == authority.client_order_id() =>
            {
                let ExitAuthorityRecoveryPlan::Resume(current) = &mut hold.plan else {
                    return false;
                };
                if *current == authority {
                    false
                } else {
                    *current = authority;
                    true
                }
            }
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_)
            | ExposureState::OperationSinkUnknown(_)
            | ExposureState::ObligationSaturated(_)
            | ExposureState::ReplacementConflict(_) => false,
        },
        ExitLifecycleEvent::Residual(managed) => {
            if matches!(
                state,
                ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
            ) {
                *state = ExposureState::Managed(managed);
                true
            } else {
                false
            }
        }
        ExitLifecycleEvent::ReleaseFlat => {
            if matches!(
                state,
                ExposureState::ExitAttempting(_)
                    | ExposureState::ExitPending(_)
                    | ExposureState::TerminalExitAwaitingPosition(_)
                    | ExposureState::ExitAuthorityRecoveryHold(_)
            ) {
                *state = ExposureState::Flat;
                true
            } else {
                false
            }
        }
    }
}

fn reduce_retained_exit_lifecycle_with_provenance(
    state: &mut ExposureState,
    event: ExitLifecycleEvent,
) -> (bool, Option<ReleasedExitProvenance>) {
    let provenance = released_exit_provenance(state);
    let changed = reduce_retained_exit_lifecycle(state, event);
    let released = changed
        && matches!(
            state,
            ExposureState::Flat | ExposureState::Managed(_) | ExposureState::PendingEntry(_)
        );
    (changed, released.then_some(provenance).flatten())
}

fn sink_unknown_resolution_state(
    unknown: &OperationSinkUnknownState,
    resolution: SinkUnknownResolution,
) -> ExposureState {
    match resolution {
        SinkUnknownResolution::Submitted => match &unknown.attempted {
            RouteOperationPayload::Entry(pending) => {
                ExposureState::PendingEntry((**pending).clone())
            }
            RouteOperationPayload::Exit(attempt) => {
                ExposureState::ExitPending((**attempt).clone().into_pending())
            }
        },
        SinkUnknownResolution::Terminal { residual } => {
            residual.map_or(ExposureState::Flat, ExposureState::Managed)
        }
        SinkUnknownResolution::Filled { managed } => ExposureState::Managed(managed),
        SinkUnknownResolution::ProvenAbsent => (*unknown.prior).clone(),
    }
}

fn rebase_state_episode(
    state: &mut ExposureState,
    before: &PositionEpisodeFingerprint,
    rebased: &ManagedPositionContext,
) -> bool {
    match state {
        ExposureState::Managed(context) => rebase_managed_context(context, before, rebased),
        ExposureState::ExitAttempting(attempt) => {
            if &attempt.managed.episode != before || attempt.authority.episode() != *before {
                return false;
            }
            if !attempt
                .authority
                .rebase_episode(before, rebased.episode.clone())
            {
                return false;
            }
            rebase_managed_context(&mut attempt.managed, before, rebased)
        }
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => {
            if &exit.position.episode != before || exit.authority.episode() != *before {
                return false;
            }
            if !exit
                .authority
                .rebase_episode(before, rebased.episode.clone())
            {
                return false;
            }
            rebase_managed_context(&mut exit.position, before, rebased)
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) => {
            if &hold.position.episode != before {
                return false;
            }
            if let ExitAuthorityRecoveryPlan::Resume(authority) = &mut hold.plan
                && (authority.episode() != *before
                    || !authority.rebase_episode(before, rebased.episode.clone()))
            {
                return false;
            }
            rebase_managed_context(&mut hold.position, before, rebased)
        }
        ExposureState::UnsupportedObserved(observed) => {
            rebase_managed_context(&mut observed.context, before, rebased)
        }
        ExposureState::BlindRecovery(recovery) => recovery
            .retained_authority_mut()
            .is_some_and(|retained| rebase_state_episode(retained, before, rebased)),
        ExposureState::OperationSinkUnknown(unknown) => {
            rebase_state_episode(&mut unknown.prior, before, rebased)
        }
        ExposureState::ObligationSaturated(saturated) => {
            rebase_state_episode(&mut saturated.retained, before, rebased)
        }
        ExposureState::ReplacementConflict(conflict) => {
            let retained = rebase_managed_context(&mut conflict.retained, before, rebased);
            let candidate = rebase_managed_context(&mut conflict.candidate, before, rebased);
            if retained {
                conflict.retained_close = ReplacementRetainedCloseProof::Awaiting;
            }
            retained || candidate
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. } => false,
    }
}

fn correction_close_state_episode(
    state: &mut ExposureState,
    before: &PositionEpisodeFingerprint,
) -> bool {
    match state {
        ExposureState::Managed(context) if &context.episode == before => {
            let next = context
                .pending_entry
                .clone()
                .map_or(ExposureState::Flat, ExposureState::PendingEntry);
            *state = next;
            true
        }
        ExposureState::ExitAttempting(attempt) if &attempt.managed.episode == before => {
            let next = attempt
                .managed
                .pending_entry
                .clone()
                .map_or(ExposureState::Flat, ExposureState::PendingEntry);
            *state = next;
            true
        }
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit)
            if &exit.position.episode == before =>
        {
            let next = exit
                .position
                .pending_entry
                .clone()
                .map_or(ExposureState::Flat, ExposureState::PendingEntry);
            *state = next;
            true
        }
        ExposureState::ExitAuthorityRecoveryHold(hold) if &hold.position.episode == before => {
            let next = hold
                .position
                .pending_entry
                .clone()
                .map_or(ExposureState::Flat, ExposureState::PendingEntry);
            *state = next;
            true
        }
        ExposureState::UnsupportedObserved(observed) if &observed.context.episode == before => {
            *state = ExposureState::Flat;
            true
        }
        ExposureState::BlindRecovery(recovery) => recovery
            .retained_authority_mut()
            .is_some_and(|retained| correction_close_state_episode(retained, before)),
        ExposureState::OperationSinkUnknown(unknown) => {
            correction_close_state_episode(&mut unknown.prior, before)
        }
        ExposureState::ObligationSaturated(saturated) => {
            correction_close_state_episode(&mut saturated.retained, before)
        }
        ExposureState::ReplacementConflict(conflict) if &conflict.retained.episode == before => {
            conflict.observe_retained_close(before)
        }
        ExposureState::ReplacementConflict(conflict) if &conflict.candidate.episode == before => {
            conflict.candidate_projection = ReplacementCandidateProjection::None;
            true
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. }
        | ExposureState::Managed(_)
        | ExposureState::ExitAttempting(_)
        | ExposureState::ExitPending(_)
        | ExposureState::TerminalExitAwaitingPosition(_)
        | ExposureState::ExitAuthorityRecoveryHold(_)
        | ExposureState::UnsupportedObserved(_)
        | ExposureState::ReplacementConflict(_) => false,
    }
}

fn released_exit_provenance(state: &ExposureState) -> Option<ReleasedExitProvenance> {
    let (position, client_order_id, observed_fill_ids) = match state {
        ExposureState::ExitAttempting(attempt) => (
            &attempt.managed,
            attempt.authority.client_order_id(),
            attempt.authority.observed_fill_ids(),
        ),
        ExposureState::ExitPending(exit) | ExposureState::TerminalExitAwaitingPosition(exit) => (
            &exit.position,
            exit.client_order_id(),
            exit.authority.observed_fill_ids(),
        ),
        ExposureState::ExitAuthorityRecoveryHold(hold) => {
            let observed_fill_ids = match &hold.plan {
                ExitAuthorityRecoveryPlan::Resume(authority) => authority.observed_fill_ids(),
                ExitAuthorityRecoveryPlan::Reconstruct { .. } => BTreeSet::new(),
            };
            (&hold.position, hold.client_order_id(), observed_fill_ids)
        }
        ExposureState::Flat
        | ExposureState::PendingEntry(_)
        | ExposureState::EntryReconcilePending { .. }
        | ExposureState::Managed(_)
        | ExposureState::UnsupportedObserved(_)
        | ExposureState::BlindRecovery(_)
        | ExposureState::OperationSinkUnknown(_)
        | ExposureState::ObligationSaturated(_)
        | ExposureState::ReplacementConflict(_) => return None,
    };
    Some(ReleasedExitProvenance {
        client_order_id,
        episode: position.episode.clone(),
        position: position.clone(),
        observed_fill_ids,
    })
}

#[derive(Default)]
struct ExposureProjection<'a> {
    pending_entry: Option<&'a PendingEntryState>,
    managed: Option<&'a ManagedPositionContext>,
    tracked: Option<&'a ManagedPositionContext>,
    exit: Option<ExitProjection<'a>>,
    recovery_hold: Option<&'a ExitAuthorityRecoveryHoldState>,
    sink_unknown: Option<&'a OperationSinkUnknownState>,
    occupancy: Option<ExposureOccupancy>,
    operation_permissions: ExposureOperationPermissions,
}

#[derive(Clone, Copy, Default)]
enum ExposureOperationPermissions {
    #[default]
    None,
    Flat,
    Managed,
    BlindRecovery,
}

impl ExposureOperationPermissions {
    const fn allows(self, operation: ExposureOperationKind) -> bool {
        match (self, operation) {
            (
                Self::Flat,
                ExposureOperationKind::EntryRoute
                | ExposureOperationKind::Bootstrap
                | ExposureOperationKind::Correction,
            )
            | (
                Self::Managed,
                ExposureOperationKind::ExitRoute
                | ExposureOperationKind::Bootstrap
                | ExposureOperationKind::Correction,
            )
            | (
                Self::BlindRecovery,
                ExposureOperationKind::Bootstrap | ExposureOperationKind::Recovery,
            ) => true,
            (
                Self::None,
                ExposureOperationKind::EntryRoute
                | ExposureOperationKind::ExitRoute
                | ExposureOperationKind::Bootstrap
                | ExposureOperationKind::Recovery
                | ExposureOperationKind::Correction,
            )
            | (Self::Flat, ExposureOperationKind::ExitRoute | ExposureOperationKind::Recovery)
            | (
                Self::Managed,
                ExposureOperationKind::EntryRoute | ExposureOperationKind::Recovery,
            )
            | (
                Self::BlindRecovery,
                ExposureOperationKind::EntryRoute
                | ExposureOperationKind::ExitRoute
                | ExposureOperationKind::Correction,
            ) => false,
        }
    }
}

#[derive(Clone, Copy)]
enum ExitProjection<'a> {
    Attempting(&'a ExitAttemptingState),
    Working(&'a ExitPendingState),
    TerminalAwaitingPosition(&'a ExitPendingState),
}

impl ExitProjection<'_> {
    fn lifecycle(self) -> (ExitLifecyclePhase, ExitPendingState) {
        match self {
            Self::Attempting(attempt) => (ExitLifecyclePhase::Attempting, attempt.snapshot()),
            Self::Working(exit) => (ExitLifecyclePhase::Working, exit.clone()),
            Self::TerminalAwaitingPosition(exit) => {
                (ExitLifecyclePhase::TerminalAwaitingPosition, exit.clone())
            }
        }
    }

    fn snapshot(self) -> ExitPendingState {
        self.lifecycle().1
    }
}

impl ExposureState {
    pub(super) const fn kind(&self) -> ExposureStateKind {
        match self {
            Self::Flat => ExposureStateKind::Flat,
            Self::PendingEntry(_) => ExposureStateKind::PendingEntry,
            Self::EntryReconcilePending { .. } => ExposureStateKind::EntryReconcilePending,
            Self::Managed(_) => ExposureStateKind::Managed,
            Self::ExitAttempting(_) => ExposureStateKind::ExitAttempting,
            Self::ExitPending(_) => ExposureStateKind::ExitPending,
            Self::TerminalExitAwaitingPosition(_) => {
                ExposureStateKind::TerminalExitAwaitingPosition
            }
            Self::ExitAuthorityRecoveryHold(_) => ExposureStateKind::ExitAuthorityRecoveryHold,
            Self::UnsupportedObserved(_) => ExposureStateKind::UnsupportedObserved,
            Self::BlindRecovery(_) => ExposureStateKind::BlindRecovery,
            Self::OperationSinkUnknown(_) => ExposureStateKind::OperationSinkUnknown,
            Self::ObligationSaturated(_) => ExposureStateKind::ObligationSaturated,
            Self::ReplacementConflict(_) => ExposureStateKind::ReplacementConflict,
        }
    }

    pub(super) fn pending_entry(&self) -> Option<&PendingEntryState> {
        self.projection().pending_entry
    }

    pub(super) fn managed_position_context(&self) -> Option<&ManagedPositionContext> {
        self.projection().managed
    }

    pub(super) fn tracked_position_context(&self) -> Option<&ManagedPositionContext> {
        self.projection().tracked
    }

    pub(super) fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.tracked_position_context()
            .map(|position| position.instrument_id)
            .or_else(|| self.pending_entry().map(|pending| pending.instrument_id))
    }

    pub(super) fn exit_pending_snapshot(&self) -> Option<ExitPendingState> {
        self.projection().exit.map(ExitProjection::snapshot)
    }

    pub(super) fn exit_lifecycle(&self) -> Option<(ExitLifecyclePhase, ExitPendingState)> {
        self.projection().exit.map(ExitProjection::lifecycle)
    }

    pub(super) fn exit_authority_recovery_hold(&self) -> Option<&ExitAuthorityRecoveryHoldState> {
        self.projection().recovery_hold
    }

    pub(super) fn operation_sink_unknown(&self) -> Option<&OperationSinkUnknownState> {
        self.projection().sink_unknown
    }

    pub(super) fn occupancy(&self) -> Option<ExposureOccupancy> {
        self.projection().occupancy
    }

    fn allows_operation(&self, operation: ExposureOperationKind) -> bool {
        self.projection().operation_permissions.allows(operation)
    }

    fn projection(&self) -> ExposureProjection<'_> {
        let mut projection = ExposureProjection::default();
        match self {
            Self::Flat => {
                projection.operation_permissions = ExposureOperationPermissions::Flat;
            }
            Self::PendingEntry(pending) => {
                projection.pending_entry = Some(pending);
                projection.occupancy = Some(ExposureOccupancy::PendingEntry);
            }
            Self::EntryReconcilePending { pending, .. } => {
                projection.pending_entry = Some(pending);
                projection.occupancy = Some(ExposureOccupancy::EntryReconcilePending);
            }
            Self::Managed(position) => {
                projection.pending_entry = position.pending_entry.as_ref();
                projection.managed = Some(position);
                projection.tracked = Some(position);
                projection.occupancy = Some(ExposureOccupancy::ManagedPosition);
                projection.operation_permissions = ExposureOperationPermissions::Managed;
            }
            Self::ExitAttempting(attempt) => {
                projection.pending_entry = attempt.managed.pending_entry.as_ref();
                projection.managed = Some(&attempt.managed);
                projection.tracked = Some(&attempt.managed);
                projection.exit = Some(ExitProjection::Attempting(attempt));
                projection.occupancy = Some(ExposureOccupancy::ExitPending);
            }
            Self::ExitPending(exit) => {
                projection.pending_entry = exit.position.pending_entry.as_ref();
                projection.managed = Some(&exit.position);
                projection.tracked = Some(&exit.position);
                projection.exit = Some(ExitProjection::Working(exit));
                projection.occupancy = Some(ExposureOccupancy::ExitPending);
            }
            Self::TerminalExitAwaitingPosition(exit) => {
                projection.pending_entry = exit.position.pending_entry.as_ref();
                projection.managed = Some(&exit.position);
                projection.tracked = Some(&exit.position);
                projection.exit = Some(ExitProjection::TerminalAwaitingPosition(exit));
                projection.occupancy = Some(ExposureOccupancy::ExitPending);
            }
            Self::ExitAuthorityRecoveryHold(hold) => {
                projection.pending_entry = hold.position.pending_entry.as_ref();
                projection.managed = Some(&hold.position);
                projection.tracked = Some(&hold.position);
                projection.recovery_hold = Some(hold);
                projection.occupancy = Some(ExposureOccupancy::ExitPending);
            }
            Self::UnsupportedObserved(observed) => {
                projection.tracked = Some(&observed.context);
                projection.occupancy = Some(ExposureOccupancy::UnsupportedObserved);
            }
            Self::BlindRecovery(recovery) => {
                projection = recovery
                    .retained_authority()
                    .map_or_else(ExposureProjection::default, ExposureState::projection);
                projection.occupancy = Some(ExposureOccupancy::BlindRecovery);
                projection.operation_permissions = ExposureOperationPermissions::BlindRecovery;
            }
            Self::OperationSinkUnknown(unknown) => {
                projection.sink_unknown = Some(unknown);
                projection.occupancy = Some(ExposureOccupancy::BlindRecovery);
            }
            Self::ObligationSaturated(saturated) => {
                projection = saturated.retained.projection();
                projection.occupancy = Some(ExposureOccupancy::BlindRecovery);
                projection.operation_permissions = ExposureOperationPermissions::None;
            }
            Self::ReplacementConflict(conflict) => {
                projection.pending_entry = conflict.retained.pending_entry.as_ref();
                projection.tracked = Some(&conflict.retained);
                projection.occupancy = Some(ExposureOccupancy::BlindRecovery);
            }
        }
        projection
    }

    #[cfg(test)]
    pub(super) fn blocks_new_entries(&self) -> bool {
        !matches!(self, Self::Flat)
    }

    pub(super) fn is_recovering(&self) -> bool {
        match self {
            Self::Managed(position) => position.origin == ManagedPositionOrigin::RecoveryBootstrap,
            Self::ExitAttempting(attempt) => {
                attempt.managed.origin == ManagedPositionOrigin::RecoveryBootstrap
            }
            Self::ExitPending(exit) | Self::TerminalExitAwaitingPosition(exit) => {
                exit.position.origin == ManagedPositionOrigin::RecoveryBootstrap
            }
            Self::ExitAuthorityRecoveryHold(hold) => {
                hold.position.origin == ManagedPositionOrigin::RecoveryBootstrap
            }
            Self::EntryReconcilePending { .. }
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_)
            | Self::OperationSinkUnknown(_)
            | Self::ObligationSaturated(_)
            | Self::ReplacementConflict(_) => true,
            Self::Flat | Self::PendingEntry(_) => false,
        }
    }

    pub(super) fn current_position_market_id(&self) -> Option<String> {
        self.tracked_position_context()
            .and_then(|position| position.lifecycle.market_id_owned())
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
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}
