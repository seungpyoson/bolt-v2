use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use nautilus_common::msgbus::{TypedHandler, subscribe_order_events, unsubscribe_order_events};
use nautilus_model::{events::OrderEventAny, identifiers::AccountId};

use crate::{
    bolt_v3_current_evidence::{
        NonBlockingRecordOutcome, OrderRejectFact, OrderRejectObserverEvidence, OrderRejectReason,
        OrderRejectSource,
    },
    bolt_v3_evidence_sampling::{EpisodeFirstNs, evict_oldest_episodes_over_cap},
    bolt_v3_operator_health::BoltV3OperatorHealthTransitionEmitter,
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
/// Placeholder substituted for a wallet/proxy address run (a `0x`-prefixed hex
/// run or a bare run of >= `REJECT_REASON_ADDR_HEX_MIN_LEN` hex characters) in
/// the verbatim venue reason before it is recorded or logged.
const REJECT_REASON_ADDR_PLACEHOLDER: &str = "[redacted-addr]";
/// Placeholder substituted for a long digit run (>= `REJECT_REASON_NUM_MIN_LEN`)
/// in the verbatim venue reason.
const REJECT_REASON_NUM_PLACEHOLDER: &str = "[redacted-num]";
/// Minimum length of a bare hex run (no `0x` prefix) treated as an address.
const REJECT_REASON_ADDR_HEX_MIN_LEN: usize = 40;
/// Minimum length of a digit run treated as a number worth redacting.
const REJECT_REASON_NUM_MIN_LEN: usize = 12;
/// Maximum retained length (in chars) of the redacted reason text.
const REJECT_REASON_MAX_LEN: usize = 256;
/// Appended when the redacted reason text is truncated at the length cap.
const REJECT_REASON_TRUNCATION_MARKER: &str = "...";
const OPERATOR_HEALTH_REASON_ORDER_REJECT_OBSERVER: &str = stringify!(order_reject_observer);

#[derive(Debug, Clone)]
struct RejectObserverEpisode {
    count: u32,
    first_ns: u64,
    last_client_order_id: String,
}

impl EpisodeFirstNs for RejectObserverEpisode {
    fn first_ns(&self) -> u64 {
        self.first_ns
    }
}

pub struct OrderRejectObserverFeedSubscription {
    order_events: Option<TypedHandler<OrderEventAny>>,
}

#[must_use]
pub fn subscribe_order_reject_observer_feed(
    feed: Arc<Mutex<BoltV3OrderRejectObserverFeed>>,
) -> OrderRejectObserverFeedSubscription {
    subscribe_order_reject_observer_feed_internal(feed, None)
}

#[must_use]
pub fn subscribe_order_reject_observer_feed_with_health_emitter(
    feed: Arc<Mutex<BoltV3OrderRejectObserverFeed>>,
    health_emitter: BoltV3OperatorHealthTransitionEmitter,
) -> OrderRejectObserverFeedSubscription {
    subscribe_order_reject_observer_feed_internal(feed, Some(health_emitter))
}

fn subscribe_order_reject_observer_feed_internal(
    feed: Arc<Mutex<BoltV3OrderRejectObserverFeed>>,
    health_emitter: Option<BoltV3OperatorHealthTransitionEmitter>,
) -> OrderRejectObserverFeedSubscription {
    let order_feed = Arc::clone(&feed);
    let order_events = TypedHandler::from(move |event: &OrderEventAny| {
        let health_updated = order_feed
            .lock()
            .expect("order reject observer order-event feed lock poisoned")
            .on_order_event(event);
        if health_updated && let Some(health_emitter) = health_emitter.as_ref() {
            health_emitter(OPERATOR_HEALTH_REASON_ORDER_REJECT_OBSERVER);
        }
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
    decision_evidence: OrderRejectObserverEvidence,
    account_id: AccountId,
    episodes: BTreeMap<String, RejectObserverEpisode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3OrderRejectObserverHealthSnapshot {
    pub active_episode_count: usize,
    pub total_retry_count: u32,
    pub oldest_episode_first_ns: Option<u64>,
    pub latest_client_order_id: Option<String>,
}

impl BoltV3OrderRejectObserverFeed {
    #[must_use]
    pub fn new(
        decision_evidence: impl Into<OrderRejectObserverEvidence>,
        account_id: AccountId,
    ) -> Self {
        Self {
            decision_evidence: decision_evidence.into(),
            account_id,
            episodes: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn health_snapshot(&self) -> BoltV3OrderRejectObserverHealthSnapshot {
        let active_episode_count = self.episodes.len();
        let total_retry_count = self
            .episodes
            .values()
            .fold(0_u32, |total, episode| total.saturating_add(episode.count));
        let oldest_episode_first_ns = self.episodes.values().map(|episode| episode.first_ns).min();
        let latest_client_order_id = self
            .episodes
            .values()
            .max_by_key(|episode| episode.first_ns)
            .map(|episode| episode.last_client_order_id.clone());
        BoltV3OrderRejectObserverHealthSnapshot {
            active_episode_count,
            total_retry_count,
            oldest_episode_first_ns,
            latest_client_order_id,
        }
    }

    pub fn on_order_event(&mut self, event: &OrderEventAny) -> bool {
        let (reject_source, venue_reason_text) = match event {
            OrderEventAny::Rejected(rejected) => {
                if event.account_id() != Some(self.account_id) {
                    return false;
                }
                (OrderRejectSource::Venue, rejected.reason.as_str())
            }
            OrderEventAny::Denied(denied) => {
                // scope: Denied is an NT-execution-level reject (tagged NtExecution) and
                // OrderDenied carries no account_id. Per-trader filtering (OrderDenied
                // exposes trader_id()) is deferred: this feed is constructed in live_node
                // with only an account_id, and threading a TraderId would require new
                // construction params through the live_node wiring (#885 minor item [14]).
                (OrderRejectSource::NtExecution, denied.reason.as_str())
            }
            _ => return false,
        };
        // Classify on the full, unredacted reason (the classification only selects
        // an enum arm and never persists the raw text); the redacted-and-capped
        // copy is what is recorded and logged.
        let reject_reason = classify_reject_reason(venue_reason_text);
        let raw_reason_text = redact_and_cap_reject_reason(venue_reason_text);
        let instrument_id = event.instrument_id().to_string();
        let client_order_id = event.client_order_id().to_string();
        let ts_event_ns = event.ts_event().as_u64();
        let stable_episode_key = format!(
            "{}/{}/{}",
            instrument_id,
            reject_source_key(reject_source),
            reject_reason_key(reject_reason)
        );
        let mut episode_inserted = false;
        let (prior_client_order_id, retry_count, elapsed_ns) = if let Some(episode) =
            self.episodes.get_mut(&stable_episode_key)
        {
            // Existing episode: prior id comes from the pre-increment count.
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
        } else {
            // First reject for this key: insert at the post-increment count and
            // flag so eviction runs (the map only grows on this branch).
            episode_inserted = true;
            let first_count =
                REJECT_OBSERVER_INITIAL_EPISODE_COUNT + REJECT_OBSERVER_EPISODE_INCREMENT;
            let first_ns = ts_event_ns;
            self.episodes.insert(
                stable_episode_key.clone(),
                RejectObserverEpisode {
                    count: first_count,
                    first_ns,
                    last_client_order_id: client_order_id.clone(),
                },
            );
            (None, first_count, ts_event_ns.saturating_sub(first_ns))
        };
        if episode_inserted {
            self.evict_oldest_episodes_over_cap();
        }
        if !retry_count.is_power_of_two() {
            return true;
        }

        let evidence = OrderRejectFact {
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
        if let NonBlockingRecordOutcome::Failed(err) = self
            .decision_evidence
            .record_observed_order_reject(evidence.clone())
        {
            log::error!(
                "bolt-v3 order-reject evidence write failed: source={:?} reason={:?} instrument_id={} client_order_id={} raw_reason_text={:?} err={err}",
                evidence.reject_source,
                evidence.reject_reason,
                evidence.instrument_id,
                evidence.client_order_id,
                evidence.raw_reason_text
            );
        }
        true
    }

    /// Bound the episode map: while it exceeds the cap, drop the entry with the
    /// smallest `first_ns` (oldest episode). Eviction only discards an
    /// evidence-sampling counter, so a later reject for the same instrument simply
    /// re-starts its episode; no trading state is touched. Delegates to the shared
    /// `evict_oldest_episodes_over_cap` helper so the bound lives in one place.
    fn evict_oldest_episodes_over_cap(&mut self) {
        evict_oldest_episodes_over_cap(
            &mut self.episodes,
            self.decision_evidence.reject_episode_max_count(),
        );
    }
}

/// Redact identifying substrings from a verbatim venue/NT reject reason and cap
/// its length before it is persisted to the S3-synced catalog tree or logged.
/// The diagnostic value of the free-form text is retained for the `Other` bucket,
/// but wallet/proxy addresses and long numeric runs (which can leak account or
/// order identity) are replaced with fixed placeholders, and the result is bounded
/// so a pathological venue message cannot bloat a record.
///
/// Redaction passes, in order:
/// 1. A `0x`-prefixed hex run, or a bare run of >= `REJECT_REASON_ADDR_HEX_MIN_LEN`
///    hex characters, becomes `REJECT_REASON_ADDR_PLACEHOLDER`.
/// 2. A remaining digit run of >= `REJECT_REASON_NUM_MIN_LEN` digits becomes
///    `REJECT_REASON_NUM_PLACEHOLDER`.
/// 3. The result is truncated to `REJECT_REASON_MAX_LEN` chars, appending
///    `REJECT_REASON_TRUNCATION_MARKER` when truncation occurs.
fn redact_and_cap_reject_reason(raw: &str) -> String {
    let address_redacted = redact_address_runs(raw);
    let number_redacted = redact_long_digit_runs(&address_redacted);
    cap_reason_length(&number_redacted)
}

/// Replace `0x`-prefixed hex runs and bare hex runs of at least
/// `REJECT_REASON_ADDR_HEX_MIN_LEN` characters with the address placeholder.
///
/// Scans with a `peekable` char cursor (no index arithmetic): at each position it
/// consumes either a `0x`/`0X`-prefixed hex run (always an address) or a maximal
/// bare hex run (an address only when long enough), passing every other character
/// through verbatim.
fn redact_address_runs(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(&first) = chars.peek() {
        // A `0`/`x` opener followed by a hex run is an address regardless of length.
        if first == '0' {
            let mut lookahead = chars.clone();
            lookahead.next();
            if matches!(lookahead.peek(), Some('x') | Some('X')) {
                lookahead.next();
                if lookahead.peek().is_some_and(char::is_ascii_hexdigit) {
                    output.push_str(REJECT_REASON_ADDR_PLACEHOLDER);
                    chars = lookahead;
                    consume_run(&mut chars, char::is_ascii_hexdigit, None);
                    continue;
                }
            }
        }
        if first.is_ascii_hexdigit() {
            let mut run = String::new();
            consume_run(&mut chars, char::is_ascii_hexdigit, Some(&mut run));
            if run.chars().count() >= REJECT_REASON_ADDR_HEX_MIN_LEN {
                output.push_str(REJECT_REASON_ADDR_PLACEHOLDER);
            } else {
                output.push_str(&run);
            }
            continue;
        }
        output.push(first);
        chars.next();
    }
    output
}

/// Replace digit runs of at least `REJECT_REASON_NUM_MIN_LEN` characters with the
/// numeric placeholder. Uses the same `peekable` run-scan as the address pass.
fn redact_long_digit_runs(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(&first) = chars.peek() {
        if first.is_ascii_digit() {
            let mut run = String::new();
            consume_run(&mut chars, char::is_ascii_digit, Some(&mut run));
            if run.chars().count() >= REJECT_REASON_NUM_MIN_LEN {
                output.push_str(REJECT_REASON_NUM_PLACEHOLDER);
            } else {
                output.push_str(&run);
            }
            continue;
        }
        output.push(first);
        chars.next();
    }
    output
}

/// Advance the cursor over a maximal run of characters matching `predicate`,
/// optionally collecting the consumed characters into `sink`.
fn consume_run(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    predicate: fn(&char) -> bool,
    mut sink: Option<&mut String>,
) {
    while let Some(&next) = chars.peek() {
        if !predicate(&next) {
            break;
        }
        if let Some(sink) = sink.as_deref_mut() {
            sink.push(next);
        }
        chars.next();
    }
}

/// Truncate to `REJECT_REASON_MAX_LEN` chars on a char boundary, appending the
/// truncation marker when the input is longer than the cap.
fn cap_reason_length(raw: &str) -> String {
    if raw.chars().count() <= REJECT_REASON_MAX_LEN {
        return raw.to_string();
    }
    let mut capped: String = raw.chars().take(REJECT_REASON_MAX_LEN).collect();
    capped.push_str(REJECT_REASON_TRUNCATION_MARKER);
    capped
}

fn classify_reject_reason(raw_reason_text: &str) -> OrderRejectReason {
    let lowercased = raw_reason_text.to_lowercase();
    if lowercased.contains(REJECT_REASON_PRECISION_NEEDLE) {
        OrderRejectReason::PrecisionRejected
    } else if lowercased.contains(REJECT_REASON_MIN_NEEDLE)
        && lowercased.contains(REJECT_REASON_NOTIONAL_NEEDLE)
    {
        OrderRejectReason::MinNotionalRejected
    } else if (lowercased.contains(REJECT_REASON_MIN_NEEDLE)
        && lowercased.contains(REJECT_REASON_SIZE_NEEDLE))
        || lowercased.contains(REJECT_REASON_TOO_SMALL_NEEDLE)
    {
        OrderRejectReason::MinSizeRejected
    } else if lowercased.contains(REJECT_REASON_INSUFFICIENT_NEEDLE)
        || lowercased.contains(REJECT_REASON_BALANCE_NEEDLE)
    {
        OrderRejectReason::InsufficientBalance
    } else if lowercased.contains(REJECT_REASON_DUPLICATE_NEEDLE) {
        OrderRejectReason::DuplicateClientOrderId
    } else {
        OrderRejectReason::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reject_reason_covers_every_arm_and_precedence() {
        assert_eq!(
            classify_reject_reason("maker amount precision exceeds venue precision"),
            OrderRejectReason::PrecisionRejected
        );
        assert_eq!(
            classify_reject_reason("minimum notional not met"),
            OrderRejectReason::MinNotionalRejected
        );
        // The MinSize arm parses as `(min && size) || too_small`: pin both the
        // `min`+`size` conjunction and the `too small` disjunct independently.
        assert_eq!(
            classify_reject_reason("order size below minimum"),
            OrderRejectReason::MinSizeRejected
        );
        assert_eq!(
            classify_reject_reason("order too small"),
            OrderRejectReason::MinSizeRejected
        );
        assert_eq!(
            classify_reject_reason("insufficient balance"),
            OrderRejectReason::InsufficientBalance
        );
        assert_eq!(
            classify_reject_reason("duplicate client order id denied by NT"),
            OrderRejectReason::DuplicateClientOrderId
        );
        assert_eq!(
            classify_reject_reason("unrecognized venue error"),
            OrderRejectReason::Other
        );
        // Precedence: precision wins over a message that also matches min+notional.
        assert_eq!(
            classify_reject_reason("precision exceeds minimum notional"),
            OrderRejectReason::PrecisionRejected
        );
    }

    #[test]
    fn redact_replaces_addresses_and_long_digit_runs() {
        let raw = "rejected for 0xABCDEF0123456789 with balance 123456789012345 left";
        let redacted = redact_and_cap_reject_reason(raw);
        assert!(
            redacted.contains(REJECT_REASON_ADDR_PLACEHOLDER),
            "0x address should be redacted: {redacted}"
        );
        assert!(
            redacted.contains(REJECT_REASON_NUM_PLACEHOLDER),
            "long digit run should be redacted: {redacted}"
        );
        assert!(
            !redacted.contains("0xABCDEF0123456789"),
            "raw address must not survive: {redacted}"
        );
        assert!(
            !redacted.contains("123456789012345"),
            "raw long number must not survive: {redacted}"
        );
        // Diagnostic words are retained.
        assert!(redacted.contains("rejected for"));
        assert!(redacted.contains("left"));
    }

    #[test]
    fn redact_replaces_bare_forty_char_hex_address() {
        let bare_address = "a".repeat(REJECT_REASON_ADDR_HEX_MIN_LEN);
        let raw = format!("denied wallet {bare_address} blocked");
        let redacted = redact_and_cap_reject_reason(&raw);
        assert!(
            redacted.contains(REJECT_REASON_ADDR_PLACEHOLDER),
            "bare 40-hex run should be redacted: {redacted}"
        );
        assert!(
            !redacted.contains(&bare_address),
            "raw bare address must not survive: {redacted}"
        );
    }

    #[test]
    fn redact_keeps_short_hex_and_short_digit_runs() {
        // A short hex token (< address threshold) and a short number (< num
        // threshold) are diagnostic and must be preserved verbatim.
        let raw = "code abc123 size 42 rejected";
        let redacted = redact_and_cap_reject_reason(raw);
        assert_eq!(redacted, raw);
    }

    #[test]
    fn redact_caps_overlong_reason_with_marker() {
        let raw = "z".repeat(REJECT_REASON_MAX_LEN + 50);
        let redacted = redact_and_cap_reject_reason(&raw);
        assert!(redacted.ends_with(REJECT_REASON_TRUNCATION_MARKER));
        let body_len = redacted.chars().count() - REJECT_REASON_TRUNCATION_MARKER.chars().count();
        assert_eq!(body_len, REJECT_REASON_MAX_LEN);
    }

    #[test]
    fn redact_leaves_within_cap_reason_unmarked() {
        let raw = "x".repeat(REJECT_REASON_MAX_LEN);
        let redacted = redact_and_cap_reject_reason(&raw);
        assert_eq!(redacted, raw);
        assert!(!redacted.ends_with(REJECT_REASON_TRUNCATION_MARKER));
    }

    #[test]
    fn evict_oldest_episodes_bounds_the_map() {
        let mut episodes: BTreeMap<String, RejectObserverEpisode> = BTreeMap::new();
        // Insert one past the cap, with strictly increasing first_ns so the oldest
        // (first_ns == 0) is unambiguous.
        let cap = 3;
        let over_cap = cap + 1;
        for index in 0..over_cap {
            episodes.insert(
                format!("instrument-{index}/venue/other"),
                RejectObserverEpisode {
                    count: REJECT_OBSERVER_EPISODE_INCREMENT,
                    first_ns: index as u64,
                    last_client_order_id: String::new(),
                },
            );
        }
        assert_eq!(episodes.len(), over_cap);
        let oldest_key = "instrument-0/venue/other".to_string();
        assert!(episodes.contains_key(&oldest_key));

        evict_oldest_episodes_over_cap(&mut episodes, cap);

        assert_eq!(episodes.len(), cap);
        assert!(
            !episodes.contains_key(&oldest_key),
            "the oldest episode (smallest first_ns) must be evicted first"
        );
    }
}

fn reject_source_key(reject_source: OrderRejectSource) -> &'static str {
    match reject_source {
        OrderRejectSource::SubmitAdmission => REJECT_SOURCE_SUBMIT_ADMISSION_KEY,
        OrderRejectSource::Venue => REJECT_SOURCE_VENUE_KEY,
        OrderRejectSource::NtExecution => REJECT_SOURCE_NT_EXECUTION_KEY,
        OrderRejectSource::Internal => REJECT_SOURCE_INTERNAL_KEY,
    }
}

fn reject_reason_key(reject_reason: OrderRejectReason) -> &'static str {
    match reject_reason {
        OrderRejectReason::AdmissionRejected => REJECT_REASON_ADMISSION_REJECTED_KEY,
        OrderRejectReason::PrecisionRejected => REJECT_REASON_PRECISION_REJECTED_KEY,
        OrderRejectReason::MinSizeRejected => REJECT_REASON_MIN_SIZE_REJECTED_KEY,
        OrderRejectReason::MinNotionalRejected => REJECT_REASON_MIN_NOTIONAL_REJECTED_KEY,
        OrderRejectReason::InsufficientBalance => REJECT_REASON_INSUFFICIENT_BALANCE_KEY,
        OrderRejectReason::DuplicateClientOrderId => REJECT_REASON_DUPLICATE_CLIENT_ORDER_ID_KEY,
        OrderRejectReason::Other => REJECT_REASON_OTHER_KEY,
    }
}
