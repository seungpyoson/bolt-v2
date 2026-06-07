use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    subscription::{IvRuntimeOperation, IvSubscriptionPlan},
};

pub trait IvRuntimeBindingAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRuntimeBindingError {
    pub profile_id: String,
    pub source_id: String,
    pub operation: IvRuntimeOperation,
    pub reason: IvRejectReason,
    pub message: String,
}

impl IvRuntimeBindingError {
    pub fn subscription_failed(plan: &IvSubscriptionPlan, message: String) -> Self {
        Self {
            profile_id: plan.profile_id.clone(),
            source_id: plan.source_id.clone(),
            operation: plan.operation,
            reason: IvRejectReason::SubscriptionFailed,
            message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvRuntimePlanOutcome {
    pub plan: IvSubscriptionPlan,
    pub source_health: IvSourceHealth,
    pub error: Option<IvRuntimeBindingError>,
}

pub fn apply_subscription_plans<A: IvRuntimeBindingAdapter>(
    adapter: &mut A,
    plans: &[IvSubscriptionPlan],
) -> Vec<IvRuntimePlanOutcome> {
    plans
        .iter()
        .map(|plan| match adapter.apply_subscription_plan(plan) {
            Ok(()) => IvRuntimePlanOutcome {
                plan: plan.clone(),
                source_health: source_health(plan, success_state(plan.operation), None),
                error: None,
            },
            Err(error) => IvRuntimePlanOutcome {
                plan: plan.clone(),
                source_health: source_health(
                    plan,
                    IvSourceHealthState::SubscriptionFailed,
                    Some(error.reason),
                ),
                error: Some(error),
            },
        })
        .collect()
}

fn success_state(operation: IvRuntimeOperation) -> IvSourceHealthState {
    match operation {
        IvRuntimeOperation::SubscribeOptionGreeks
        | IvRuntimeOperation::SubscribeOptionChain
        | IvRuntimeOperation::SubscribeAggregateGreeks
        | IvRuntimeOperation::SubscribeCustomData => IvSourceHealthState::Active,
        IvRuntimeOperation::UnsubscribeOptionGreeks
        | IvRuntimeOperation::UnsubscribeOptionChain
        | IvRuntimeOperation::UnsubscribeAggregateGreeks
        | IvRuntimeOperation::UnsubscribeCustomData => IvSourceHealthState::Unsubscribing,
        IvRuntimeOperation::RemoveSource => IvSourceHealthState::Removed,
    }
}

fn source_health(
    plan: &IvSubscriptionPlan,
    subscription_state: IvSourceHealthState,
    reject_reason: Option<IvRejectReason>,
) -> IvSourceHealth {
    let mut reject_counts = BTreeMap::new();
    if let Some(reason) = reject_reason {
        reject_counts.insert(reason, 1);
    }

    IvSourceHealth {
        profile_id: plan.profile_id.clone(),
        source_id: plan.source_id.clone(),
        subscription_state,
        last_event_ts_ns: None,
        last_reject_reason: reject_reason,
        reject_counts,
        stale_state: false,
        retention_state: false,
        subscription_generation: plan.subscription_generation,
    }
}
