mod support;

use std::sync::Arc;

use bolt_v2::bolt_v3_decision_evidence::{BoltV3OrderRejectReason, BoltV3RejectSource};
use bolt_v2::bolt_v3_order_reject_observer_feed::BoltV3OrderRejectObserverFeed;
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
fn same_episode_rejects_emit_exponential_samples_with_previous_client_order_id() {
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
        vec![1, 2, 4, 8]
    );
    assert_eq!(
        records
            .iter()
            .map(|record| record.prior_client_order_id.as_deref())
            .collect::<Vec<_>>(),
        vec![
            None,
            Some("client-order-1"),
            Some("client-order-3"),
            Some("client-order-7")
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
