use crate::support;

use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use bolt_v2::bolt_v3_decision_evidence::{BoltV3OrderRejectReason, BoltV3RejectSource};
use bolt_v2::bolt_v3_order_reject_observer_feed::{
    BoltV3OrderRejectObserverFeed, subscribe_order_reject_observer_feed,
};
use nautilus_common::msgbus::{publish_order_event, switchboard};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderType},
    events::{
        OrderAccepted, OrderDenied, OrderEventAny, OrderFilled, OrderRejected, OrderSubmitted,
    },
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId, TraderId,
        VenueOrderId,
    },
    types::{Currency, Price, Quantity},
};
use ustr::Ustr;

#[test]
#[should_panic(expected = "order reject observer order-event feed lock poisoned")]
fn subscribed_order_reject_event_panics_on_poisoned_feed_lock() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let feed = Arc::new(Mutex::new(BoltV3OrderRejectObserverFeed::new(
        writer,
        AccountId::from("ACCOUNT-001"),
    )));
    poison_lock(&feed);
    let _subscription = subscribe_order_reject_observer_feed(feed);

    publish_order_event(
        switchboard::get_event_order_topic(StrategyId::from("strategy-a")),
        &OrderEventAny::Rejected(order_rejected_event(
            "client-order-1",
            "instrument-yes.VENUE-A",
            AccountId::from("ACCOUNT-001"),
            "maker amount precision exceeds venue precision",
            1_000,
        )),
    );
}

#[test]
fn rejected_event_for_configured_account_records_venue_precision_reject_evidence() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));
    let reason = "maker amount precision exceeds venue precision";

    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        reason,
        1_000,
    )));

    let records = writer.order_rejects();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.reject_source, BoltV3RejectSource::Venue);
    assert_eq!(
        record.reject_reason,
        BoltV3OrderRejectReason::PrecisionRejected
    );
    assert_eq!(record.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(record.client_order_id, "client-order-1");
    assert_eq!(record.raw_reason_text.as_deref(), Some(reason));
    assert_eq!(record.admission_outcome, None);
    assert_eq!(record.order_side, None);
    assert_eq!(record.raw_price, None);
    assert_eq!(record.raw_quantity, None);
    assert_eq!(record.raw_maker_amount, None);
    assert_eq!(record.raw_taker_amount, None);
    assert_eq!(record.normalized_price, None);
    assert_eq!(record.normalized_quantity, None);
    assert_eq!(record.normalized_maker_amount, None);
    assert_eq!(record.normalized_taker_amount, None);
    assert_eq!(record.venue_price_precision, None);
    assert_eq!(record.venue_size_precision, None);
    assert_eq!(record.venue_min_notional, None);
    assert_eq!(record.backoff_cooldown_state, None);
}

#[test]
fn reject_observer_snapshot_renders_active_episode_state() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));

    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        "maker amount precision exceeds venue precision",
        1_000,
    )));
    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-2",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        "maker amount precision exceeds venue precision",
        1_050,
    )));

    let snapshot = feed.health_snapshot();

    assert_eq!(snapshot.active_episode_count, 1);
    assert_eq!(snapshot.total_retry_count, 2);
    assert_eq!(
        snapshot.latest_client_order_id.as_deref(),
        Some("client-order-2")
    );
    assert_eq!(snapshot.oldest_episode_first_ns, Some(1_000));
}

#[test]
fn rejected_event_for_different_account_is_not_recorded() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));

    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-002"),
        "maker amount precision exceeds venue precision",
        1_000,
    )));

    assert!(writer.order_rejects().is_empty());
}

#[test]
fn denied_event_without_account_records_nt_execution_reject_evidence() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));
    let reason = "duplicate client order id denied by NT";

    feed.on_order_event(&OrderEventAny::Denied(order_denied_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        reason,
        1_000,
    )));

    let records = writer.order_rejects();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.reject_source, BoltV3RejectSource::NtExecution);
    assert_eq!(
        record.reject_reason,
        BoltV3OrderRejectReason::DuplicateClientOrderId
    );
    assert_eq!(record.raw_reason_text.as_deref(), Some(reason));
    assert_eq!(record.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(record.client_order_id, "client-order-1");
}

#[test]
fn submitted_accepted_and_filled_events_are_not_recorded() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));

    feed.on_order_event(&OrderEventAny::Submitted(order_submitted_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        1_000,
    )));
    feed.on_order_event(&OrderEventAny::Accepted(order_accepted_event(
        "client-order-2",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        1_100,
    )));
    feed.on_order_event(&OrderEventAny::Filled(order_filled_event(
        "client-order-3",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        1_200,
    )));

    assert!(writer.order_rejects().is_empty());
}

#[test]
fn same_state_rejects_reach_the_production_authority_without_producer_sampling() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));

    for attempt in 1_u64..=9 {
        feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
            &format!("client-order-{attempt}"),
            "instrument-yes.VENUE-A",
            AccountId::from("ACCOUNT-001"),
            "maker amount precision exceeds venue precision",
            1_000 + attempt,
        )));
    }

    let records = writer.order_rejects();
    assert_eq!(
        records
            .iter()
            .map(|record| record.retry_count)
            .collect::<Vec<_>>(),
        (1..=9).collect::<Vec<_>>()
    );
    assert_eq!(
        records
            .iter()
            .map(|record| record.prior_client_order_id.as_deref())
            .collect::<Vec<_>>(),
        vec![
            None,
            Some("client-order-1"),
            Some("client-order-2"),
            Some("client-order-3"),
            Some("client-order-4"),
            Some("client-order-5"),
            Some("client-order-6"),
            Some("client-order-7"),
            Some("client-order-8")
        ]
    );
    assert!(records.iter().all(
        |record| record.stable_episode_key == "instrument-yes.VENUE-A/venue/precision_rejected"
    ));
    assert_eq!(
        records
            .iter()
            .map(|record| record.elapsed_ns)
            .collect::<Vec<_>>(),
        vec![0, 1, 3, 7]
    );
}

#[test]
fn order_reject_evidence_write_failure_is_swallowed() {
    // FIX 3b: the observer's record_order_reject path is swallow-on-error. A writer
    // that errors on record_order_reject must leave on_order_event returning
    // normally with no panic.
    let writer = Arc::new(support::OrderRejectFailingDecisionEvidenceWriter::default());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));

    // The first reject has retry_count == 1 (a power of two), so the sink is
    // attempted; the error must be swallowed rather than propagated/panicking.
    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        "maker amount precision exceeds venue precision",
        1_000,
    )));

    assert_eq!(
        writer.order_reject_attempts(),
        1,
        "the order-reject sink must have been attempted exactly once"
    );
}

#[test]
fn recorded_raw_reason_text_redacts_address_and_long_digit_run() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));
    let reason = "insufficient balance for 0xABCDEF0123456789 holding 123456789012345 units";

    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        reason,
        1_000,
    )));

    let records = writer.order_rejects();
    assert_eq!(records.len(), 1);
    let recorded = records[0]
        .raw_reason_text
        .as_deref()
        .expect("reason recorded");
    assert!(
        !recorded.contains("0xABCDEF0123456789"),
        "raw address must not persist: {recorded}"
    );
    assert!(
        !recorded.contains("123456789012345"),
        "raw long number must not persist: {recorded}"
    );
    assert!(
        recorded.contains("[redacted-addr]"),
        "address placeholder must persist: {recorded}"
    );
    assert!(
        recorded.contains("[redacted-num]"),
        "number placeholder must persist: {recorded}"
    );
    // Diagnostic words around the redaction are retained, so classification and
    // the `Other`-bucket diagnostics still work.
    assert!(recorded.contains("insufficient balance"));
}

#[test]
fn recorded_raw_reason_text_is_truncated_to_cap() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let mut feed =
        BoltV3OrderRejectObserverFeed::new(writer.clone(), AccountId::from("ACCOUNT-001"));
    // 600 ASCII chars, well over the 256-char cap, with no redactable runs.
    let reason = "z".repeat(600);

    feed.on_order_event(&OrderEventAny::Rejected(order_rejected_event(
        "client-order-1",
        "instrument-yes.VENUE-A",
        AccountId::from("ACCOUNT-001"),
        &reason,
        1_000,
    )));

    let records = writer.order_rejects();
    assert_eq!(records.len(), 1);
    let recorded = records[0]
        .raw_reason_text
        .as_deref()
        .expect("reason recorded");
    assert!(
        recorded.ends_with("..."),
        "over-long reason must be marked truncated: {recorded}"
    );
    // 256-char body + the 3-char truncation marker.
    assert_eq!(recorded.chars().count(), 256 + 3);
}

fn order_rejected_event(
    client_order_id: &str,
    instrument_id: &str,
    account_id: AccountId,
    reason: &str,
    ts_event: u64,
) -> OrderRejected {
    OrderRejected::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        account_id,
        Ustr::from(reason),
        UUID4::new(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        false,
    )
}

fn poison_lock<T>(lock: &Arc<Mutex<T>>) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _g = lock.lock().unwrap();
        panic!("seed poison");
    }));
}

fn order_denied_event(
    client_order_id: &str,
    instrument_id: &str,
    reason: &str,
    ts_event: u64,
) -> OrderDenied {
    OrderDenied::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        Ustr::from(reason),
        UUID4::new(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}

fn order_submitted_event(
    client_order_id: &str,
    instrument_id: &str,
    account_id: AccountId,
    ts_event: u64,
) -> OrderSubmitted {
    OrderSubmitted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        account_id,
        UUID4::new(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
    )
}

fn order_accepted_event(
    client_order_id: &str,
    instrument_id: &str,
    account_id: AccountId,
    ts_event: u64,
) -> OrderAccepted {
    OrderAccepted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from("venue-order-1"),
        account_id,
        UUID4::new(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
    )
}

fn order_filled_event(
    client_order_id: &str,
    instrument_id: &str,
    account_id: AccountId,
    ts_event: u64,
) -> OrderFilled {
    OrderFilled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("strategy-a"),
        InstrumentId::from(instrument_id),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from("venue-order-1"),
        account_id,
        TradeId::from("trade-1"),
        OrderSide::Buy,
        OrderType::Limit,
        Quantity::from("1"),
        Price::from("0.40"),
        Currency::USD(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::from(ts_event),
        UnixNanos::from(ts_event),
        false,
        Some(PositionId::from("position-1")),
        None,
    )
}
