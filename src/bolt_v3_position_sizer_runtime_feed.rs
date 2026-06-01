use std::sync::{Arc, Mutex};

use nautilus_common::msgbus::{TypedHandler, subscribe_order_events, unsubscribe_order_events};
use nautilus_model::{events::OrderEventAny, identifiers::AccountId};

use crate::{
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionState, BoltV3SubmitPositionSizingLifecycleDecision,
    },
    nt_runtime_capture::order_events_pattern,
};

const POSITION_SIZER_ORDER_TERMINAL_SOURCE: &str = stringify!(nt_order_terminal_event);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSizerRuntimeFeedConfig {
    pub account_id: AccountId,
}

#[derive(Debug)]
pub struct PositionSizerRuntimeFeed {
    config: PositionSizerRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    latest_terminal_observed_at_ns: Option<u64>,
}

pub struct PositionSizerRuntimeFeedSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
}

#[must_use]
pub fn subscribe_position_sizer_runtime_feed(
    feed: Arc<Mutex<PositionSizerRuntimeFeed>>,
) -> PositionSizerRuntimeFeedSubscription {
    let order_feed = Arc::clone(&feed);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        order_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_order_event(event);
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);

    PositionSizerRuntimeFeedSubscription {
        order_events: Some(order_events),
    }
}

impl PositionSizerRuntimeFeedSubscription {
    pub fn unsubscribe_all(&mut self) {
        if let Some(order_events) = self.order_events.take() {
            unsubscribe_order_events(order_events_pattern(), &order_events);
        }
    }
}

impl Drop for PositionSizerRuntimeFeedSubscription {
    fn drop(&mut self) {
        self.unsubscribe_all();
    }
}

impl PositionSizerRuntimeFeed {
    #[must_use]
    pub fn new(
        config: PositionSizerRuntimeFeedConfig,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
    ) -> Self {
        Self {
            config,
            submit_admission,
            latest_terminal_observed_at_ns: None,
        }
    }

    pub fn on_order_event(
        &mut self,
        event: &OrderEventAny,
    ) -> Option<BoltV3SubmitPositionSizingLifecycleDecision> {
        if !is_terminal_order_event(event) {
            return None;
        }
        if let Some(account_id) = event.account_id()
            && account_id != self.config.account_id
        {
            return None;
        }

        let observed_at_ns = event.ts_event().as_u64();
        let decision = self
            .submit_admission
            .apply_position_sizing_terminal_order_event(
                event.client_order_id().to_string(),
                observed_at_ns,
                POSITION_SIZER_ORDER_TERMINAL_SOURCE.to_string(),
            );
        if decision.unknown_reservation {
            return None;
        }
        self.latest_terminal_observed_at_ns = Some(observed_at_ns);
        Some(decision)
    }

    #[must_use]
    pub const fn latest_terminal_observed_at_ns(&self) -> Option<u64> {
        self.latest_terminal_observed_at_ns
    }
}

fn is_terminal_order_event(event: &OrderEventAny) -> bool {
    matches!(
        event,
        OrderEventAny::Denied(_)
            | OrderEventAny::Rejected(_)
            | OrderEventAny::Canceled(_)
            | OrderEventAny::Expired(_)
    )
}
