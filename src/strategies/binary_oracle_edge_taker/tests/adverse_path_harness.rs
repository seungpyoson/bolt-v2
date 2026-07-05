#![cfg(test)]

use super::*;
use crate::{
    bolt_v3_maker_runtime_settlement::{
        MakerRuntimeSettlementInput, settle_maker_runtime_reference_prices,
    },
    bolt_v3_maker_settlement::{BinarySettlementLot, BinarySettlementResult},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
};
use nautilus_model::{
    events::{OrderAccepted, OrderEventAny, OrderSubmitted},
    identifiers::{AccountId, VenueOrderId},
};
use nautilus_trading::Strategy;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub(super) const PRECISION_REJECT_REASON: &str = "invalid amounts, the market buy orders maker amount supports a max accuracy of 2 decimals, taker amount a max of 4 decimals";
pub(super) const BALANCE_REJECT_REASON: &str =
    "not enough balance / allowance: the balance is not enough -> balance: 0";
pub(super) const MIN_SIZE_REJECT_REASON: &str =
    "invalid amount for a marketable BUY order ($0.84), min size: 1";
// guard matches failure CLASS, not assertion instance — never reuse a constant in a new assertion.
const DROPPED_TERMINAL_PINNED_FAILURE: &str =
    "accepted-with-no-terminal entry replay reached the boundary with no terminal event";
const PARTIAL_FILL_PINNED_FAILURE: &str =
    "partial-fill residual must be re-managed with the exact unfilled quantity";
const RESTART_OPEN_EXIT_PINNED_FAILURE: &str =
    "restart replay must adopt the recovered exit before attributing fills";
const SETTLEMENT_PINNED_FAILURE: &str = "hold-to-resolution must close exposure to Flat, book realized cash, and record settlement evidence";

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
fn dropped_terminal_event_after_accepted_entry_is_not_left_pending() {
    assert_reality_fixtures();

    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let entry_client_order_id = ClientOrderId::from("ENTRY-ACCEPTED-NO-TERMINAL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let sequence = accepted_without_terminal_sequence(entry_client_order_id, instrument_id);
    // The venue replay fixture documents the incident sequence; the strategy state is set below.
    assert_event_types(&sequence, &["Initialized", "Submitted", "Accepted"]);

    set_pending_entry(&mut strategy, pending);
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-NEXT", 2_000));

    assert_managed_or_halted_loud(&strategy, DROPPED_TERMINAL_PINNED_FAILURE);
}

#[test]
fn partial_fill_then_expire_exit_residual_is_remanaged_or_reexited() {
    assert_reality_fixtures();

    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
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
        open_position.clone(),
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

    // halt-loudly is deliberately NOT an acceptable terminal for partial-fill
    // residuals — the residual is known, so it must be re-managed; spec
    // decision recorded on #1179.
    assert!(
        partial_fill_residual_is_managed_or_fresh_reexit(
            &strategy,
            &cache,
            exit_client_order_id,
            &open_position,
            Quantity::new(6.0, 2),
        ),
        "{PARTIAL_FILL_PINNED_FAILURE}; expected Managed residual quantity 6.00 or a fresh non-terminal re-exit with a new client_order_id, got {:?}",
        strategy.exposure,
    );
}

#[test]
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
        "{RESTART_OPEN_EXIT_PINNED_FAILURE}: bootstrap must adopt the open exit order before a subsequent fill can be attributed"
    );

    strategy
        .on_order_filled(&order_filled_event_with_details(
            exit_client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Sell,
        ))
        .unwrap_or_else(|error| {
            panic!(
                "{RESTART_OPEN_EXIT_PINNED_FAILURE}: replayed exit fill should be attributable after bootstrap: {error:?}"
            )
        });
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true),
        "{RESTART_OPEN_EXIT_PINNED_FAILURE}: replayed fill must mark the recovered exit order as filled"
    );
}

#[test]
fn hold_to_resolution_books_realized_cash_and_settlement_evidence() {
    assert_reality_fixtures();

    let winning_yes = hold_to_resolution_case(
        "winning YES",
        Leg::Yes,
        0.45,
        3_101.0,
        5.5,
        PositionId::from("P-HOLD-TO-RESOLUTION-WIN"),
    );
    let losing_no = hold_to_resolution_case(
        "losing NO",
        Leg::No,
        0.50,
        3_101.0,
        -5.0,
        PositionId::from("P-HOLD-TO-RESOLUTION-LOSS"),
    );
    let observations = [winning_yes, losing_no];
    let failed_cases = observations
        .iter()
        .filter(|case| !(case.exposure_is_flat && case.settlement_evidence_matches_expected))
        .map(|case| {
            format!(
                "{} expected_realized_pnl={} exposure={:?} evidence_events={:?}",
                case.name, case.expected_realized_pnl, case.exposure, case.evidence_events,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        failed_cases.is_empty(),
        "{SETTLEMENT_PINNED_FAILURE}; failed_cases={failed_cases:?}"
    );
}

#[test]
fn feed_outage_at_resolution_records_booking_error_without_booking_settlement() {
    assert_reality_fixtures();

    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-HOLD-TO-RESOLUTION-FEED-OUTAGE"),
        Quantity::new(10.0, 2),
        0.45,
    );

    let close_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure market close");
    strategy
        .check_resolution_feed_outage_at_market_end(close_ms)
        .expect("feed outage check should record booking-error evidence");

    let events = evidence.events();
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && !matches!(strategy.exposure, ExposureState::Flat),
        "resolution feed outage must fail closed: no settlement booking, one loud booking-error record, exposure preserved for venue-truth fence; exposure={:?}, events={events:?}",
        strategy.exposure
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    let events = evidence.events();
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && !matches!(strategy.exposure, ExposureState::Flat),
        "late resolution feed after a recorded outage must remain fail-closed with no booking; exposure={:?}, events={events:?}",
        strategy.exposure
    );
}

#[test]
fn terminal_after_settlement_stays_flat_and_does_not_double_book() {
    assert_reality_fixtures();

    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-TERMINAL-AFTER-SETTLEMENT"),
        Quantity::new(10.0, 2),
        0.45,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-AFTER-SETTLEMENT");
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    assert!(matches!(strategy.exposure, ExposureState::Flat));
    strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

    let events = evidence.events();
    assert_eq!(settlement_evidence_count(&events), 1);
    assert_eq!(settlement_booking_error_count(&events), 0);
    assert!(matches!(strategy.exposure, ExposureState::Flat));
}

#[test]
fn terminal_before_settlement_remanages_residual_then_books_residual_settlement() {
    assert_reality_fixtures();

    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-TERMINAL-BEFORE-SETTLEMENT"),
        Quantity::new(10.0, 2),
        0.45,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-BEFORE-SETTLEMENT");
    set_exit_pending_with_filled_quantity(
        &mut strategy,
        position,
        exit_client_order_id,
        Quantity::new(4.0, 2),
    );

    strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));
    assert!(
        matches!(
            &strategy.exposure,
            ExposureState::Managed(managed) if managed.position.quantity == Quantity::new(6.0, 2)
        ),
        "terminal before settlement must re-manage the known residual before resolution; exposure={:?}",
        strategy.exposure
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    let expected =
        expected_hold_to_resolution_settlement_for_quantity(Leg::Yes, 0.45, 3_101.0, 6.0);
    let events = evidence.events();
    assert!(
        matches!(strategy.exposure, ExposureState::Flat)
            && settlement_evidence_matches(&events, expected.realized_pnl)
            && settlement_evidence_count(&events) == 1,
        "residual settlement should book exactly the residual quantity after terminal-before-settlement; expected_realized_pnl={}, exposure={:?}, events={events:?}",
        expected.realized_pnl,
        strategy.exposure
    );
}

#[derive(Debug)]
struct SettlementCaseObservation {
    name: &'static str,
    expected_realized_pnl: f64,
    exposure_is_flat: bool,
    settlement_evidence_matches_expected: bool,
    exposure: ExposureState,
    evidence_events: Vec<RecordedDecisionEvidenceEvent>,
}

fn hold_to_resolution_case(
    name: &'static str,
    held_leg: Leg,
    entry_price: f64,
    reference_close_price: f64,
    expected_realized_pnl: f64,
    position_id: PositionId,
) -> SettlementCaseObservation {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, held_leg);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        entry_price,
    );
    let expected =
        expected_hold_to_resolution_settlement(held_leg, entry_price, reference_close_price);
    assert!(
        (expected.realized_pnl - expected_realized_pnl).abs() <= f64::EPSILON,
        "hold-to-resolution fixture {name} must pin expected realized_pnl numerically; expected {}, got {}",
        expected_realized_pnl,
        expected.realized_pnl,
    );

    let close_report_ts_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure the interval close boundary");
    let resolution_update = IndexPriceUpdate::new(
        strategy
            .resolution_instrument_id()
            .expect("fixture should configure the resolution instrument"),
        Price::new(reference_close_price, 1),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
    );
    DataActor::on_index_price(&mut strategy, &resolution_update)
        .expect("resolution index price should route through the strategy handler");

    let evidence_events = evidence.events();
    // For this harness, the Settlement evidence record is the booking record
    // being asserted. The durable cash-surface pin (loss-governor accumulator /
    // venue-truth balance) is owned by PR-D acceptance on #1179.
    let settlement_evidence_matches_expected =
        settlement_evidence_matches(&evidence_events, expected.realized_pnl);

    SettlementCaseObservation {
        name,
        expected_realized_pnl: expected.realized_pnl,
        exposure_is_flat: matches!(strategy.exposure, ExposureState::Flat),
        settlement_evidence_matches_expected,
        exposure: strategy.exposure.clone(),
        evidence_events,
    }
}

fn emit_resolution_update(strategy: &mut BinaryOracleEdgeTaker, reference_close_price: f64) {
    let close_report_ts_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure the interval close boundary");
    let resolution_update = IndexPriceUpdate::new(
        strategy
            .resolution_instrument_id()
            .expect("fixture should configure the resolution instrument"),
        Price::new(reference_close_price, 1),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
    );
    DataActor::on_index_price(strategy, &resolution_update)
        .expect("resolution index price should route through the strategy handler");
}

fn partial_fill_residual_is_managed_or_fresh_reexit(
    strategy: &BinaryOracleEdgeTaker,
    cache: &Rc<RefCell<Cache>>,
    expired_client_order_id: ClientOrderId,
    original_position: &OpenPositionState,
    expected_residual_quantity: Quantity,
) -> bool {
    match &strategy.exposure {
        ExposureState::Managed(managed) => {
            position_matches_expected_residual(
                &managed.position,
                original_position,
                expected_residual_quantity,
            ) && open_sell_exit_order_count(cache, original_position) == 0
        }
        ExposureState::ExitPending(exit) => {
            let Some(managed) = &exit.position else {
                return false;
            };
            position_matches_expected_residual(
                &managed.position,
                original_position,
                expected_residual_quantity,
            ) && exit.pending_exit.client_order_id != expired_client_order_id
                && exit.pending_exit.position_id == Some(original_position.position_id)
                && !exit.pending_exit.fill_received
                && !exit.pending_exit.close_received
                && !exit.pending_exit.terminal_received
                && !exit.pending_exit.residual_position_observed_after_fill
                && fresh_exit_order_matches_residual(
                    cache,
                    &exit.pending_exit,
                    original_position,
                    expected_residual_quantity,
                    // A residual re-exit after partial fill is a normal-path exit of remaining exposure; the normal exit_order template is the spec.
                    strategy.config.exit_order.order_type,
                    strategy.config.exit_order.time_in_force,
                    strategy.config.exit_order.is_reduce_only,
                )
        }
        _ => false,
    }
}

fn position_matches_expected_residual(
    actual: &OpenPositionState,
    original: &OpenPositionState,
    expected_residual_quantity: Quantity,
) -> bool {
    actual.quantity == expected_residual_quantity
        && (
            &actual.instrument_id,
            &actual.position_id,
            actual.side,
            // avg_px_open is invariant under partial close; on this single-entry fixture the pin is convention-independent.
            actual.avg_px_open.to_bits(),
        ) == (
            &original.instrument_id,
            &original.position_id,
            original.side,
            original.avg_px_open.to_bits(),
        )
}

fn fresh_exit_order_matches_residual(
    cache: &Rc<RefCell<Cache>>,
    pending_exit: &PendingExitState,
    original_position: &OpenPositionState,
    expected_residual_quantity: Quantity,
    expected_order_type: OrderType,
    expected_time_in_force: TimeInForce,
    expected_is_reduce_only: bool,
) -> bool {
    let cache = cache.borrow();
    let cache_position_id_matches =
        cache.position_id(&pending_exit.client_order_id) == Some(&original_position.position_id);
    let open_sell_orders = cache.orders(
        Some(&fixture_execution_venue()),
        Some(&original_position.instrument_id),
        Some(&StrategyId::from("BINARYORACLEEDGETAKER-001")),
        None,
        Some(OrderSide::Sell),
    );
    let open_sell_orders = open_sell_orders
        .into_iter()
        .filter(|order| !order.is_closed())
        .collect::<Vec<_>>();
    open_sell_orders.len() == 1
        && open_sell_orders.first().is_some_and(|order| {
            order.client_order_id() == pending_exit.client_order_id
                && order.instrument_id() == original_position.instrument_id
                && order.quantity() == expected_residual_quantity
                && order.order_type() == expected_order_type
                && order.time_in_force() == expected_time_in_force
                && order.is_reduce_only() == expected_is_reduce_only
                && cache_position_id_matches
        })
}

fn open_sell_exit_order_count(
    cache: &Rc<RefCell<Cache>>,
    original_position: &OpenPositionState,
) -> usize {
    cache
        .borrow()
        .orders(
            Some(&fixture_execution_venue()),
            Some(&original_position.instrument_id),
            Some(&StrategyId::from("BINARYORACLEEDGETAKER-001")),
            None,
            Some(OrderSide::Sell),
        )
        .into_iter()
        .filter(|order| !order.is_closed())
        .count()
}

fn set_exit_pending_with_filled_quantity(
    strategy: &mut BinaryOracleEdgeTaker,
    position: OpenPositionState,
    client_order_id: ClientOrderId,
    filled_quantity: Quantity,
) {
    strategy.exposure = ExposureState::ExitPending(ExitPendingState {
        pending_exit: PendingExitState {
            client_order_id,
            market_id: position.market_id.clone(),
            position_id: Some(position.position_id),
            fill_received: true,
            filled_quantity: Some(filled_quantity),
            close_received: false,
            terminal_received: false,
            residual_position_observed_after_fill: false,
        },
        position: Some(ManagedPositionState {
            position,
            origin: ManagedPositionOrigin::StrategyEntry,
            pending_entry: None,
        }),
    });
}

fn held_instrument_id(strategy: &BinaryOracleEdgeTaker, held_leg: Leg) -> InstrumentId {
    match held_leg {
        Leg::Yes => strategy
            .active
            .books
            .up
            .instrument_id
            .expect("ready-to-trade fixture should configure the YES instrument"),
        Leg::No => strategy
            .active
            .books
            .down
            .instrument_id
            .expect("ready-to-trade fixture should configure the NO instrument"),
    }
}

fn expected_hold_to_resolution_settlement(
    held_leg: Leg,
    entry_price: f64,
    reference_close_price: f64,
) -> BinarySettlementResult {
    expected_hold_to_resolution_settlement_for_quantity(
        held_leg,
        entry_price,
        reference_close_price,
        10.0,
    )
}

fn expected_hold_to_resolution_settlement_for_quantity(
    held_leg: Leg,
    entry_price: f64,
    reference_close_price: f64,
    quantity: f64,
) -> BinarySettlementResult {
    let lot = BinarySettlementLot {
        leg: held_leg,
        side: QuoteSide::Buy,
        quantity,
        entry_price,
    };
    settle_maker_runtime_reference_prices(MakerRuntimeSettlementInput {
        family_key: crate::bolt_v3_market_families::updown::KEY,
        reference_close_price,
        strike_price: 3_100.0,
        lots: &[lot],
    })
    .result
    .expect("fixture payout should settle the held lot")
}

fn settlement_evidence_count(events: &[RecordedDecisionEvidenceEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, RecordedDecisionEvidenceEvent::Settlement(_)))
        .count()
}

fn settlement_booking_error_count(events: &[RecordedDecisionEvidenceEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RecordedDecisionEvidenceEvent::SettlementBookingError(_)
            )
        })
        .count()
}

fn settlement_evidence_matches(
    events: &[RecordedDecisionEvidenceEvent],
    expected_realized_pnl: f64,
) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            RecordedDecisionEvidenceEvent::Settlement(evidence)
                if (evidence.realized_pnl - expected_realized_pnl).abs() <= f64::EPSILON
        )
    })
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
                | ExposureState::EntryReconcilePending { .. }
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
