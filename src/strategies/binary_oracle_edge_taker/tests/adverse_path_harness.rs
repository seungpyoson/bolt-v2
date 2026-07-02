#![cfg(test)]

use super::*;
use nautilus_model::{
    events::{OrderAccepted, OrderEventAny, OrderSubmitted},
    identifiers::{AccountId, VenueOrderId},
};
use nautilus_trading::Strategy;
use serde_json::{Value, json};
use std::{panic, sync::Arc};

const PRECISION_REJECT_REASON: &str = "invalid amounts, the market buy orders maker amount supports a max accuracy of 2 decimals, taker amount a max of 4 decimals";
const BALANCE_REJECT_REASON: &str =
    "not enough balance / allowance: the balance is not enough -> balance: 0";
const MIN_SIZE_REJECT_REASON: &str =
    "invalid amount for a marketable BUY order ($0.84), min size: 1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IncidentLifecycleCounts {
    initialized: u16,
    submitted: u16,
    rejected: u16,
    accepted: u16,
    filled: u16,
    accepted_no_terminal: u16,
}

const INCIDENT_LIFECYCLE_COUNTS: IncidentLifecycleCounts = IncidentLifecycleCounts {
    initialized: 490,
    submitted: 449,
    rejected: 485,
    accepted: 5,
    filled: 4,
    accepted_no_terminal: 1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueEventFixture {
    event_type: &'static str,
    payload_json: Value,
}

#[test]
#[ignore = "red until #1179 Lane 4 acceptance bar 1: dropped terminal event"]
fn dropped_terminal_event_after_accepted_entry_is_not_left_pending() {
    assert_reality_fixtures();

    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ACCEPTED-NO-TERMINAL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let sequence = accepted_without_terminal_sequence(entry_client_order_id, instrument_id);
    assert_event_types(&sequence, &["Initialized", "Submitted", "Accepted"]);

    set_pending_entry(&mut strategy, pending);
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-NEXT", 2_000));

    assert_managed_or_halted_loud(
        &strategy,
        "accepted-with-no-terminal entry replay reached the boundary with no terminal event",
    );
}

#[test]
#[ignore = "red until #1179 Lane 4 acceptance bar 1: partial-fill-then-expire exit"]
fn partial_fill_then_expire_exit_residual_is_remanaged_or_halted_loud() {
    assert_reality_fixtures();

    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-PARTIAL-FILL-EXPIRE-NO-POSITION-UPDATE");
    let exit_client_order_id = ClientOrderId::from("EXIT-PARTIAL-FILL-EXPIRE");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    let sequence = partial_fill_then_expire_sequence(exit_client_order_id, instrument_id);
    assert_event_types(&sequence, &["Filled", "Expired"]);

    let mut fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    strategy
        .on_order_filled(&fill)
        .expect("partial exit fill bookkeeping should not error");
    strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

    assert_managed_or_halted_loud(
        &strategy,
        "partial exit fill followed by Expired without a position update left residual exposure unmanaged",
    );
    assert_eq!(
        managed_position_ref(&strategy).map(|position| position.quantity),
        Some(Quantity::new(6.0, 2)),
        "the unfilled residual must be the managed position quantity after a 4/10 partial exit"
    );
}

#[test]
#[ignore = "red until #1179 Lane 4 acceptance bar 1: restart with open order+position"]
fn restart_with_open_exit_order_and_position_adopts_order_before_fill_replay() {
    assert_reality_fixtures();

    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-RESTART-OPEN-ORDER");
    let exit_client_order_id = ClientOrderId::from("EXIT-OPEN-AT-RESTART");
    strategy.config.exit_order.order_type = OrderType::Limit;
    strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "restart-market",
        "restart-market",
        "Up",
        1_000,
        1_300,
    );
    let position = Position::new(
        &instrument,
        order_filled_event_with_details(
            ClientOrderId::from("ENTRY-BEFORE-RESTART"),
            instrument_id,
            Some(position_id),
            OrderSide::Buy,
        ),
    );
    let exit_order = strategy
        .build_configured_exit_order(
            instrument_id,
            OrderSide::Sell,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            exit_client_order_id,
        )
        .expect("restart fixture exit order should build");
    let (submitted, accepted) = submitted_and_accepted_events(&exit_order, "V-RESTART-EXIT-001");

    {
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(instrument)
            .expect("test cache should accept restart instrument");
        cache
            .add_position(&position, NtOmsType::Netting)
            .expect("test cache should accept restart position");
        cache
            .add_order(
                exit_order,
                Some(position_id),
                Some(ClientId::from(strategy.config.client_id.as_str())),
                true,
            )
            .expect("test cache should accept restart open order");
        cache
            .update_order(&submitted)
            .expect("test cache should replay restart order Submitted");
        cache
            .update_order(&accepted)
            .expect("test cache should replay restart order Accepted");
    }
    assert_eq!(
        cache
            .borrow()
            .orders_open(
                Some(&fixture_execution_venue()),
                Some(&instrument_id),
                Some(&StrategyId::from("BINARYORACLEEDGETAKER-001")),
                None,
                Some(OrderSide::Sell),
            )
            .len(),
        1,
        "fixture must seed the open exit order that bootstrap is expected to adopt"
    );

    strategy.bootstrap_recovery_from_cache();

    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id),
        "bootstrap must adopt the open exit order before a subsequent fill can be attributed"
    );

    strategy
        .on_order_filled(&order_filled_event_with_details(
            exit_client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Sell,
        ))
        .expect("replayed exit fill should be attributable after bootstrap");
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true),
        "replayed fill must mark the recovered exit order as filled"
    );
}

#[test]
#[ignore = "red until #1179 Lane 3 acceptance bar 1: hold-to-resolution settlement"]
fn hold_to_resolution_books_realized_cash_and_settlement_evidence() {
    assert_reality_fixtures();

    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-HOLD-TO-RESOLUTION"),
        Quantity::new(10.0, 2),
        0.45,
    );
    let lot = crate::bolt_v3_maker_settlement::BinarySettlementLot {
        leg: crate::bolt_v3_quote_lifecycle::Leg::Yes,
        side: crate::bolt_v3_quoting::QuoteSide::Buy,
        quantity: 10.0,
        entry_price: 0.45,
    };
    let settlement = crate::bolt_v3_maker_runtime_settlement::settle_maker_runtime_reference_prices(
        crate::bolt_v3_maker_runtime_settlement::MakerRuntimeSettlementInput {
            family_key: crate::bolt_v3_market_families::updown::KEY,
            reference_close_price: 3_101.0,
            strike_price: 3_100.0,
            lots: &[lot],
        },
    );
    let expected = settlement
        .result
        .expect("fixture payout should settle the held YES lot");

    strategy
        .active
        .observe_resolution_strike(3_101.0, 1_000, 1_300);

    let evidence_events = evidence.events();
    let settlement_evidence_present = evidence_events.iter().any(|event| {
        format!("{event:?}")
            .to_ascii_lowercase()
            .contains("settlement")
    });
    assert!(
        strategy.managed_position().is_none() && settlement_evidence_present,
        "hold-to-resolution must close exposure, book realized cash {}, and record settlement evidence; exposure={:?} evidence_events={:?}",
        expected.realized_pnl,
        strategy.exposure,
        evidence_events,
    );
}

#[test]
fn ignored_adverse_path_harness_tests_still_fail_red() {
    let mut previous_hook = Some(panic::take_hook());
    panic::set_hook(Box::new(|_| {}));

    for (name, test) in [
        (
            "dropped_terminal_event_after_accepted_entry_is_not_left_pending",
            dropped_terminal_event_after_accepted_entry_is_not_left_pending as fn(),
        ),
        (
            "partial_fill_then_expire_exit_residual_is_remanaged_or_halted_loud",
            partial_fill_then_expire_exit_residual_is_remanaged_or_halted_loud as fn(),
        ),
        (
            "restart_with_open_exit_order_and_position_adopts_order_before_fill_replay",
            restart_with_open_exit_order_and_position_adopts_order_before_fill_replay as fn(),
        ),
        (
            "hold_to_resolution_books_realized_cash_and_settlement_evidence",
            hold_to_resolution_books_realized_cash_and_settlement_evidence as fn(),
        ),
    ] {
        if panic::catch_unwind(test).is_ok() {
            panic::set_hook(
                previous_hook
                    .take()
                    .expect("panic hook should still be available for restore"),
            );
            panic!(
                "{name} unexpectedly passed; remove the ignore and land the corresponding production fix"
            );
        }
    }

    panic::set_hook(
        previous_hook
            .take()
            .expect("panic hook should still be available for restore"),
    );
}

fn assert_incident_lifecycle_counts() {
    let counts = INCIDENT_LIFECYCLE_COUNTS;
    assert_eq!(
        counts.rejected + counts.accepted,
        counts.initialized,
        "rejected plus accepted terminal outcomes must account for initialized incident orders"
    );
    assert_eq!(
        counts.filled + counts.accepted_no_terminal,
        counts.accepted,
        "filled plus accepted-with-no-terminal orders must account for accepted incident orders"
    );
    assert_eq!(
        counts.accepted_no_terminal, 1,
        "the incident precedent for this harness is exactly one accepted order with no terminal event"
    );
    assert!(
        counts.submitted < counts.initialized,
        "the captured incident stream had fewer submitted responses than initialized orders"
    );
}

fn assert_reality_fixtures() {
    assert_incident_lifecycle_counts();
    for reason in [
        PRECISION_REJECT_REASON,
        BALANCE_REJECT_REASON,
        MIN_SIZE_REJECT_REASON,
    ] {
        let event = rejected_response(reason);
        assert_eq!(event.event_type, "Rejected");
        assert_eq!(event.payload_json["Rejected"]["reason"], reason);
    }
}

fn assert_managed_or_halted_loud(strategy: &BinaryOracleEdgeTaker, context: &str) {
    assert!(
        matches!(
            strategy.exposure,
            ExposureState::Managed(_)
                | ExposureState::BlindRecovery(_)
                | ExposureState::UnsupportedObserved(_)
        ),
        "{context}; expected Managed or fail-closed halt/recovery state, got {:?}",
        strategy.exposure,
    );
}

fn assert_event_types(events: &[VenueEventFixture], expected: &[&str]) {
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        events
            .iter()
            .all(|event| event.payload_json.get("event_type") == Some(&json!(event.event_type))),
        "every venue fixture must carry event_type in payload_json too: {events:?}"
    );
}

fn accepted_without_terminal_sequence(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> Vec<VenueEventFixture> {
    vec![
        order_response("Initialized", client_order_id, instrument_id),
        order_response("Submitted", client_order_id, instrument_id),
        order_response("Accepted", client_order_id, instrument_id),
    ]
}

fn partial_fill_then_expire_sequence(
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> Vec<VenueEventFixture> {
    vec![
        order_response("Filled", client_order_id, instrument_id),
        order_response("Expired", client_order_id, instrument_id),
    ]
}

fn order_response(
    event_type: &'static str,
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
) -> VenueEventFixture {
    VenueEventFixture {
        event_type,
        payload_json: json!({
            "event_type": event_type,
            "client_order_id": client_order_id.to_string(),
            "instrument_id": instrument_id.to_string(),
        }),
    }
}

fn rejected_response(reason: &'static str) -> VenueEventFixture {
    VenueEventFixture {
        event_type: "Rejected",
        payload_json: json!({
            "event_type": "Rejected",
            "Rejected": {
                "reason": reason,
            },
        }),
    }
}

fn submitted_and_accepted_events(
    order: &OrderAny,
    venue_order_id: &str,
) -> (OrderEventAny, OrderEventAny) {
    let trader_id = nautilus_model::identifiers::TraderId::from("TRADER-001");
    let strategy_id = StrategyId::from("BINARYORACLEEDGETAKER-001");
    let instrument_id = order.instrument_id();
    let client_order_id = order.client_order_id();
    let account_id = AccountId::from("TEST-ACCOUNT");
    (
        OrderEventAny::Submitted(OrderSubmitted::new(
            trader_id,
            strategy_id,
            instrument_id,
            client_order_id,
            account_id,
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_000_u64),
            UnixNanos::from(1_000_u64),
        )),
        OrderEventAny::Accepted(OrderAccepted::new(
            trader_id,
            strategy_id,
            instrument_id,
            client_order_id,
            VenueOrderId::from(venue_order_id),
            account_id,
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_001_u64),
            UnixNanos::from(1_001_u64),
            false,
        )),
    )
}
