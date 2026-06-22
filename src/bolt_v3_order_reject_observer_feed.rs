use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use nautilus_common::msgbus::{TypedHandler, subscribe_order_events, unsubscribe_order_events};
use nautilus_model::{events::OrderEventAny, identifiers::AccountId};

use crate::{
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceWriter, BoltV3OrderRejectEvidence, BoltV3OrderRejectReason,
        BoltV3RejectSource,
    },
    nt_runtime_capture::order_events_pattern,
};

const REJECT_SOURCE_SUBMIT_ADMISSION_KEY: &str = "submit_admission";
const REJECT_SOURCE_VENUE_KEY: &str = "venue";
const REJECT_SOURCE_NT_EXECUTION_KEY: &str = "nt_execution";
const REJECT_SOURCE_INTERNAL_KEY: &str = "internal";
const REJECT_REASON_ADMISSION_REJECTED_KEY: &str = "admission_rejected";
const REJECT_REASON_PRECISION_REJECTED_KEY: &str = "precision_rejected";
const REJECT_REASON_MIN_SIZE_REJECTED_KEY: &str = "min_size_rejected";
const REJECT_REASON_MIN_NOTIONAL_REJECTED_KEY: &str = "min_notional_rejected";
const REJECT_REASON_INSUFFICIENT_BALANCE_KEY: &str = "insufficient_balance";
const REJECT_REASON_DUPLICATE_CLIENT_ORDER_ID_KEY: &str = "duplicate_client_order_id";
const REJECT_REASON_OTHER_KEY: &str = "other";
const REJECT_REASON_PRECISION_NEEDLE: &str = "precision";
const REJECT_REASON_MIN_NEEDLE: &str = "min";
const REJECT_REASON_NOTIONAL_NEEDLE: &str = "notional";
const REJECT_REASON_SIZE_NEEDLE: &str = "size";
const REJECT_REASON_TOO_SMALL_NEEDLE: &str = "too small";
const REJECT_REASON_INSUFFICIENT_NEEDLE: &str = "insufficient";
const REJECT_REASON_BALANCE_NEEDLE: &str = "balance";
const REJECT_REASON_DUPLICATE_NEEDLE: &str = "duplicate";
const REJECT_OBSERVER_INITIAL_EPISODE_COUNT: u32 = 0;
const REJECT_OBSERVER_EPISODE_INCREMENT: u32 = 1;

#[derive(Debug, Clone)]
struct RejectObserverEpisode {
    count: u32,
    first_ns: u64,
    last_client_order_id: String,
}

pub struct OrderRejectObserverFeedSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
}

#[must_use]
pub fn subscribe_order_reject_observer_feed(
    feed: Arc<Mutex<BoltV3OrderRejectObserverFeed>>,
) -> OrderRejectObserverFeedSubscription {
    let order_feed = Arc::clone(&feed);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        order_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_order_event(event);
    });
    subscribe_order_events(order_events_pattern(), order_events.clone(), None);
    OrderRejectObserverFeedSubscription {
        order_events: Some(order_events),
    }
}

impl OrderRejectObserverFeedSubscription {
    pub fn unsubscribe_all(&mut self) {
        if let Some(order_events) = self.order_events.take() {
            unsubscribe_order_events(order_events_pattern(), &order_events);
        }
    }
}

impl Drop for OrderRejectObserverFeedSubscription {
    fn drop(&mut self) {
        self.unsubscribe_all();
    }
}

pub struct BoltV3OrderRejectObserverFeed {
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    account_id: AccountId,
    episodes: BTreeMap<String, RejectObserverEpisode>,
}

impl BoltV3OrderRejectObserverFeed {
    #[must_use]
    pub fn new(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        account_id: AccountId,
    ) -> Self {
        Self {
            decision_evidence,
            account_id,
            episodes: BTreeMap::new(),
        }
    }

    pub fn on_order_event(&mut self, event: &OrderEventAny) {
        let (reject_source, raw_reason_text) = match event {
            OrderEventAny::Rejected(rejected) => {
                if event.account_id() != Some(self.account_id) {
                    return;
                }
                (BoltV3RejectSource::Venue, rejected.reason.to_string())
            }
            OrderEventAny::Denied(denied) => {
                (BoltV3RejectSource::NtExecution, denied.reason.to_string())
            }
            _ => return,
        };
        let reject_reason = classify_reject_reason(&raw_reason_text);
        let instrument_id = event.instrument_id().to_string();
        let client_order_id = event.client_order_id().to_string();
        let ts_event_ns = event.ts_event().as_u64();
        let stable_episode_key = format!(
            "{}/{}/{}",
            instrument_id,
            reject_source_key(reject_source),
            reject_reason_key(reject_reason)
        );
        let (prior_client_order_id, retry_count, elapsed_ns) = {
            let episode = self
                .episodes
                .entry(stable_episode_key.clone())
                .or_insert_with(|| RejectObserverEpisode {
                    count: REJECT_OBSERVER_INITIAL_EPISODE_COUNT,
                    first_ns: ts_event_ns,
                    last_client_order_id: String::new(),
                });
            let prior_client_order_id = if episode.count > REJECT_OBSERVER_INITIAL_EPISODE_COUNT {
                Some(episode.last_client_order_id.clone())
            } else {
                None
            };
            episode.count = episode
                .count
                .saturating_add(REJECT_OBSERVER_EPISODE_INCREMENT);
            episode.last_client_order_id = client_order_id.clone();
            (
                prior_client_order_id,
                episode.count,
                ts_event_ns.saturating_sub(episode.first_ns),
            )
        };
        if !retry_count.is_power_of_two() {
            return;
        }

        let evidence = BoltV3OrderRejectEvidence {
            reject_source,
            reject_reason,
            admission_outcome: None,
            raw_reason_text: Some(raw_reason_text),
            instrument_id,
            order_side: None,
            raw_price: None,
            raw_quantity: None,
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: None,
            normalized_quantity: None,
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: None,
            venue_size_precision: None,
            venue_min_notional: None,
            prior_client_order_id,
            client_order_id,
            retry_count,
            backoff_cooldown_state: None,
            stable_episode_key,
            elapsed_ns,
        };
        if let Err(err) = self.decision_evidence.record_order_reject(&evidence) {
            log::warn!(
                "bolt-v3 order-reject evidence write failed: source={:?} reason={:?} instrument_id={} client_order_id={} raw_reason_text={:?} err={err}",
                evidence.reject_source,
                evidence.reject_reason,
                evidence.instrument_id,
                evidence.client_order_id,
                evidence.raw_reason_text
            );
        }
    }
}

fn classify_reject_reason(raw_reason_text: &str) -> BoltV3OrderRejectReason {
    let lowercased = raw_reason_text.to_lowercase();
    if lowercased.contains(REJECT_REASON_PRECISION_NEEDLE) {
        BoltV3OrderRejectReason::PrecisionRejected
    } else if lowercased.contains(REJECT_REASON_MIN_NEEDLE)
        && lowercased.contains(REJECT_REASON_NOTIONAL_NEEDLE)
    {
        BoltV3OrderRejectReason::MinNotionalRejected
    } else if lowercased.contains(REJECT_REASON_MIN_NEEDLE)
        && lowercased.contains(REJECT_REASON_SIZE_NEEDLE)
        || lowercased.contains(REJECT_REASON_TOO_SMALL_NEEDLE)
    {
        BoltV3OrderRejectReason::MinSizeRejected
    } else if lowercased.contains(REJECT_REASON_INSUFFICIENT_NEEDLE)
        || lowercased.contains(REJECT_REASON_BALANCE_NEEDLE)
    {
        BoltV3OrderRejectReason::InsufficientBalance
    } else if lowercased.contains(REJECT_REASON_DUPLICATE_NEEDLE) {
        BoltV3OrderRejectReason::DuplicateClientOrderId
    } else {
        BoltV3OrderRejectReason::Other
    }
}

fn reject_source_key(reject_source: BoltV3RejectSource) -> &'static str {
    match reject_source {
        BoltV3RejectSource::SubmitAdmission => REJECT_SOURCE_SUBMIT_ADMISSION_KEY,
        BoltV3RejectSource::Venue => REJECT_SOURCE_VENUE_KEY,
        BoltV3RejectSource::NtExecution => REJECT_SOURCE_NT_EXECUTION_KEY,
        BoltV3RejectSource::Internal => REJECT_SOURCE_INTERNAL_KEY,
    }
}

fn reject_reason_key(reject_reason: BoltV3OrderRejectReason) -> &'static str {
    match reject_reason {
        BoltV3OrderRejectReason::AdmissionRejected => REJECT_REASON_ADMISSION_REJECTED_KEY,
        BoltV3OrderRejectReason::PrecisionRejected => REJECT_REASON_PRECISION_REJECTED_KEY,
        BoltV3OrderRejectReason::MinSizeRejected => REJECT_REASON_MIN_SIZE_REJECTED_KEY,
        BoltV3OrderRejectReason::MinNotionalRejected => REJECT_REASON_MIN_NOTIONAL_REJECTED_KEY,
        BoltV3OrderRejectReason::InsufficientBalance => REJECT_REASON_INSUFFICIENT_BALANCE_KEY,
        BoltV3OrderRejectReason::DuplicateClientOrderId => {
            REJECT_REASON_DUPLICATE_CLIENT_ORDER_ID_KEY
        }
        BoltV3OrderRejectReason::Other => REJECT_REASON_OTHER_KEY,
    }
}
