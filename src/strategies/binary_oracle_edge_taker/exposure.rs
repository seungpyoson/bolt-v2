use nautilus_model::{
    enums::{OrderSide, PositionSide},
    identifiers::{ClientOrderId, InstrumentId, PositionId, Venue},
    types::Quantity,
};

use crate::{
    bolt_v3_book_sizing::OutcomeBookState,
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_position_contract::{
        BoltV3PositionMarketLifecycle, expected_exit_order_side_for_position,
        expected_position_side_for_entry_order, is_observed_open_side,
    },
};

use super::OutcomeFeeState;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct OpenPositionState {
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) position_id: PositionId,
    pub(super) outcome_fees: OutcomeFeeState,
    pub(super) historical_entry_fee_bps: Option<f64>,
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
    pub(super) outcome_fees: OutcomeFeeState,
    pub(super) historical_entry_fee_bps: Option<f64>,
    pub(super) book: OutcomeBookState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingExitState {
    pub(super) client_order_id: ClientOrderId,
    pub(super) submitted_at_ms: Option<u64>,
    pub(super) market_id: Option<String>,
    pub(super) position_id: Option<PositionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedPositionOrigin {
    StrategyEntry,
    RecoveryBootstrap,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionContext {
    pub(super) lifecycle: BoltV3PositionMarketLifecycle,
    pub(super) instrument_id: InstrumentId,
    pub(super) position_id: PositionId,
    pub(super) outcome_fees: OutcomeFeeState,
    pub(super) historical_entry_fee_bps: Option<f64>,
    pub(super) book: OutcomeBookState,
    pub(super) origin: ManagedPositionOrigin,
    pub(super) pending_entry: Option<PendingEntryState>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ManagedPositionState {
    pub(super) position: OpenPositionState,
    pub(super) origin: ManagedPositionOrigin,
    pub(super) pending_entry: Option<PendingEntryState>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitPendingState {
    pub(super) position: Option<ManagedPositionContext>,
    pub(super) pending_exit: PendingExitState,
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
    AmbiguousRestartOpenExitOrders {
        instrument_id: InstrumentId,
        count: usize,
    },
    UnattributedRestartOpenExitOrder {
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
pub(super) enum ExposureState {
    Flat,
    PendingEntry(PendingEntryState),
    EntryReconcilePending {
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    },
    Managed(ManagedPositionContext),
    ExitPending(ExitPendingState),
    UnsupportedObserved(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
}

impl ExposureState {
    pub(super) fn pending_entry(&self) -> Option<&PendingEntryState> {
        match self {
            Self::PendingEntry(pending) | Self::EntryReconcilePending { pending, .. } => {
                Some(pending)
            }
            Self::Managed(position) => position.pending_entry.as_ref(),
            Self::ExitPending(exit) => exit
                .position
                .as_ref()
                .and_then(|position| position.pending_entry.as_ref()),
            _ => None,
        }
    }

    pub(super) fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        match self {
            Self::PendingEntry(pending) | Self::EntryReconcilePending { pending, .. } => {
                Some(pending)
            }
            Self::Managed(position) => position.pending_entry.as_mut(),
            Self::ExitPending(exit) => exit
                .position
                .as_mut()
                .and_then(|position| position.pending_entry.as_mut()),
            _ => None,
        }
    }

    pub(super) fn managed_position_context(&self) -> Option<&ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitPending(exit) => exit.position.as_ref(),
            _ => None,
        }
    }

    pub(super) fn managed_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitPending(exit) => exit.position.as_mut(),
            _ => None,
        }
    }

    pub(super) fn tracked_position_context(&self) -> Option<&ManagedPositionContext> {
        self.managed_position_context().or(match self {
            Self::UnsupportedObserved(observed) => Some(&observed.context),
            _ => None,
        })
    }

    pub(super) fn tracked_position_context_mut(&mut self) -> Option<&mut ManagedPositionContext> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitPending(exit) => exit.position.as_mut(),
            Self::UnsupportedObserved(observed) => Some(&mut observed.context),
            _ => None,
        }
    }

    pub(super) fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.tracked_position_context()
            .map(|position| position.instrument_id)
            .or_else(|| self.pending_entry().map(|pending| pending.instrument_id))
    }

    pub(super) fn exit_pending(&self) -> Option<&ExitPendingState> {
        match self {
            Self::ExitPending(exit) => Some(exit),
            _ => None,
        }
    }

    pub(super) fn occupancy(&self) -> Option<ExposureOccupancy> {
        match self {
            Self::Flat => None,
            Self::PendingEntry(_) => Some(ExposureOccupancy::PendingEntry),
            Self::EntryReconcilePending { .. } => Some(ExposureOccupancy::EntryReconcilePending),
            Self::Managed(_) => Some(ExposureOccupancy::ManagedPosition),
            Self::ExitPending(_) => Some(ExposureOccupancy::ExitPending),
            Self::UnsupportedObserved(_) => Some(ExposureOccupancy::UnsupportedObserved),
            Self::BlindRecovery(_) => Some(ExposureOccupancy::BlindRecovery),
        }
    }

    #[cfg(test)]
    pub(super) fn blocks_new_entries(&self) -> bool {
        !matches!(self, Self::Flat)
    }

    pub(super) fn is_recovering(&self) -> bool {
        match self {
            Self::Managed(position) => position.origin == ManagedPositionOrigin::RecoveryBootstrap,
            Self::ExitPending(exit) => exit.position.as_ref().is_some_and(|position| {
                position.origin == ManagedPositionOrigin::RecoveryBootstrap
            }),
            Self::EntryReconcilePending { .. }
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => true,
            Self::Flat | Self::PendingEntry(_) => false,
        }
    }

    pub(super) fn current_position_market_id(&self) -> Option<String> {
        self.managed_position_context()
            .and_then(|position| position.lifecycle.market_id_owned())
            .or_else(|| {
                self.exit_pending()
                    .and_then(|exit| exit.pending_exit.market_id.clone())
            })
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
