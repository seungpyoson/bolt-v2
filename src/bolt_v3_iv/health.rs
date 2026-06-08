use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{error::IvRejectReason, time::UnixNanos};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvSourceHealthState {
    Configured,
    Subscribing,
    Active,
    Stale,
    Unsubscribing,
    Removed,
    SubscriptionFailed,
    Rejected,
}

impl IvSourceHealthState {
    pub fn can_transition_to(self, next: Self) -> bool {
        use IvSourceHealthState::*;

        if next == Rejected {
            return self != Removed && self != Rejected;
        }

        matches!(
            (self, next),
            (Configured, Subscribing)
                | (Subscribing, Active)
                | (Subscribing, SubscriptionFailed)
                | (Active, Stale)
                | (Active, Unsubscribing)
                | (Stale, Active)
                | (Stale, Unsubscribing)
                | (Unsubscribing, Removed)
                | (SubscriptionFailed, Subscribing)
        )
    }

    pub fn can_satisfy_current_query(self) -> bool {
        self == Self::Active
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Subscribing => "subscribing",
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Unsubscribing => "unsubscribing",
            Self::Removed => "removed",
            Self::SubscriptionFailed => "subscription_failed",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvSourceHealth {
    pub profile_id: String,
    pub source_id: String,
    pub subscription_state: IvSourceHealthState,
    pub last_event_ts_ns: Option<UnixNanos>,
    pub last_reject_reason: Option<IvRejectReason>,
    pub reject_counts: BTreeMap<IvRejectReason, u64>,
    pub stale_state: bool,
    pub retention_state: bool,
    pub subscription_generation: u64,
}

impl IvSourceHealth {
    pub fn can_satisfy_current_query(&self) -> bool {
        self.subscription_state.can_satisfy_current_query()
            && !self.stale_state
            && !self.retention_state
    }
}
