#![cfg(test)]

use super::*;
use crate::{
    bolt_v3_binary_settlement::{BinarySettlementLot, BinarySettlementResult},
    bolt_v3_binary_settlement_runtime::{
        BinaryRuntimeSettlementInput, settle_binary_runtime_reference_prices,
    },
    bolt_v3_prediction_market_instrument::prediction_market_product_id_from_instrument_id,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
};
use nautilus_model::{
    enums::PositionSideSpecified,
    events::{OrderAccepted, OrderEventAny, OrderSubmitted},
    identifiers::{AccountId, TradeId, VenueOrderId},
    position::Position,
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
const POSITION_MARKET_LIFECYCLE_PINNED_FAILURE: &str =
    "managed position must own its market lifecycle across active-market roll";
const TEST_LOSS_STATE_MAX_BYTES: u64 = 65_536;
const TEST_LOSS_ACTION_RETRY_INTERVAL_MS: u64 = 250;
const TEST_LOSS_ACTION_RETRY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Default)]
struct NoopLossActionSink;

impl crate::bolt_v3_loss_protection::KillSwitchLossActionSink for NoopLossActionSink {
    fn emit(&self, _action: crate::bolt_v3_loss_protection::KillSwitchLossAction) -> Result<()> {
        Ok(())
    }
}

struct DurableLossSettlementRuntimeSink {
    loss_protection: RefCell<crate::bolt_v3_loss_protection::KillSwitchLossProtection>,
}

impl std::fmt::Debug for DurableLossSettlementRuntimeSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableLossSettlementRuntimeSink").finish()
    }
}

impl DurableLossSettlementRuntimeSink {
    fn new(loss_protection: crate::bolt_v3_loss_protection::KillSwitchLossProtection) -> Self {
        Self {
            loss_protection: RefCell::new(loss_protection),
        }
    }

    fn loss_snapshot(&self) -> crate::bolt_v3_kill_switch_store::KillSwitchLossProtectionSnapshot {
        self.loss_protection
            .borrow()
            .store()
            .load_recovery_record()
            .expect("durable loss-governor state should be readable")
            .loss_protection
            .expect("loss-governor snapshot should be persisted")
    }
}

impl crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSink
    for DurableLossSettlementRuntimeSink
{
    fn record_loss_governor_position_realized_pnl(
        &self,
        observation: crate::bolt_v3_loss_protection::PositionRealizedPnlObservation,
    ) -> Result<()> {
        self.loss_protection
            .borrow_mut()
            .record_position_realized_pnl(observation)?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct LossFailingSettlementRuntimeSink {
    loss_observations: RefCell<Vec<crate::bolt_v3_loss_protection::PositionRealizedPnlObservation>>,
}

impl LossFailingSettlementRuntimeSink {
    fn loss_observation_count(&self) -> usize {
        self.loss_observations.borrow().len()
    }
}

impl crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSink
    for LossFailingSettlementRuntimeSink
{
    fn record_loss_governor_position_realized_pnl(
        &self,
        observation: crate::bolt_v3_loss_protection::PositionRealizedPnlObservation,
    ) -> Result<()> {
        self.loss_observations.borrow_mut().push(observation);
        anyhow::bail!("synthetic loss-governor reducer failure")
    }
}

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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position.clone(),
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let sequence = partial_fill_then_expire_sequence(exit_client_order_id, instrument_id);
    assert_event_types(&sequence, &["Filled", "Expired"]);

    let mut fill = order_filled_event(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    fill.trade_id = TradeId::from("TRADE-PROJECTED-PARTIAL-ADVERSE");
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Filled(fill.clone()));
    strategy.on_order_filled(&fill);
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        open_position.avg_px_open,
        OrderSide::Buy,
    );
    let expired = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Expired(expired.clone()));
    strategy.on_order_expired(expired);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Long,
        Quantity::new(6.0, 2),
        2_000,
    );
    emit_time_event_at(&mut strategy, 2);

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
        order_filled_event(
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
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id()),
        Some(exit_client_order_id),
        "{RESTART_OPEN_EXIT_PINNED_FAILURE}: bootstrap must adopt the open exit order before a subsequent fill can be attributed"
    );

    let mut terminal_fill = order_filled_event(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    terminal_fill.trade_id = TradeId::from("TRADE-RESTART-PROJECTED-TERMINAL");
    terminal_fill.ts_event = UnixNanos::from(1_002_u64);
    terminal_fill.ts_init = UnixNanos::from(1_002_u64);
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Filled(terminal_fill.clone()));
    strategy.on_order_filled(&terminal_fill);
    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id()),
        Some(exit_client_order_id),
        "{RESTART_OPEN_EXIT_PINNED_FAILURE}: a fill event must not replace NT position truth or lose exit-order correlation"
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));
    assert_eq!(
        strategy
            .context
            .position_authority()
            .expect("projected terminal requires position authority")
            .canonical_position(position_id, instrument_id)
            .expect("projected terminal position read should succeed")
            .expect("projected terminal must retain the stale cached position")
            .signed_quantity(),
        Decimal::new(10, 0),
        "order-only reconciliation must not masquerade as position causality"
    );

    close_nt_position(&mut strategy, position_id);
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Flat,
        Quantity::zero(2),
        1_100,
    );
    let mut authority = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("recovered terminal exit remains tracked before timer reconciliation")
        .authority;
    assert_eq!(
        authority
            .release(
                strategy
                    .context
                    .position_authority()
                    .expect("converged flat release requires position authority"),
            )
            .expect("exact flat cache/report convergence should be evaluable"),
        crate::bolt_v3_order_execution::BoltV3PositionReductionRelease::Flat
    );
    let cached_exit = strategy
        .cache()
        .order(&exit_client_order_id)
        .expect("recovered terminal order should remain cached");
    assert!(
        cached_exit.is_closed(),
        "cached status={:?}",
        cached_exit.status()
    );
    assert!(matches!(
        classify_cached_exit_order_lifecycle(cached_exit.status()),
        CachedExitOrderLifecycle::Terminal { .. }
    ));
    assert_eq!(
        cached_exit.ts_last(),
        terminal_fill.ts_event,
        "the repeated cache observation must represent the same terminal event"
    );
    assert_eq!(
        authority
            .observe_order(
                &cached_exit,
                cached_exit.ts_last().as_u64(),
                BoltV3ExitOrderCorrection::Unchanged,
            )
            .expect("repeated terminal observation should remain valid"),
        crate::bolt_v3_order_execution::BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition
    );
    assert_eq!(
        authority
            .release(
                strategy
                    .context
                    .position_authority()
                    .expect("repeated terminal release requires position authority"),
            )
            .expect("repeated terminal observation must preserve established proof"),
        crate::bolt_v3_order_execution::BoltV3PositionReductionRelease::Flat
    );
    assert!(strategy.event_instrument_matches_held_exposure(instrument_id));
    strategy.reconcile_cached_exit_order_on_timer();

    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat),
        "recovered projected terminal should release after exact flat cache/report convergence; exposure={:?}",
        strategy.exposure
    );
    assert!(pending_exit_snapshot(&strategy).is_none());
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
fn feed_outage_at_resolution_records_booking_error_after_close_fetch_retry_budget_exhausted() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        OrderSide::Buy,
        PositionSide::Long,
    );

    let close_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure market close");
    strategy
        .check_resolution_feed_outage_at_market_end(close_ms)
        .expect("feed outage check should dispatch the first close fetch");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 0
            && close_fetch_count == 1
            && !matches!(strategy.exposure.state(), ExposureState::Flat),
        "resolution feed outage must first request a close-boundary fetch before terminal booking-error; exposure={:?}, close_fetch_count={close_fetch_count}, events={events:?}",
        strategy.exposure
    );

    emit_settlement_close_retry_budget_events(&mut strategy, close_ms);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && close_fetch_count == strategy.config.market_exit_max_attempts as usize
            // #1349: terminal booking-error releases exposure (Flat) so the
            // single-exposure strategy is not parked forever. Venue residual may
            // still exist in NT cache; occupancy is strategy-local.
            && matches!(strategy.exposure.state(), ExposureState::Flat)
            && terminal_settlement_lifecycle_count(&events) == 1,
        "resolution feed outage must fail closed after close-fetch retry exhaustion: no settlement booking, one loud booking-error record, exposure released to Flat; exposure={:?}, close_fetch_count={close_fetch_count}, events={events:?}",
        strategy.exposure
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && matches!(strategy.exposure.state(), ExposureState::Flat),
        "late resolution feed after a recorded outage must remain fail-closed with no booking and Flat exposure; exposure={:?}, events={events:?}",
        strategy.exposure
    );
}

#[test]
fn position_market_lifecycle_books_settlement_at_its_own_interval_end() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-ROLLED-OWN-END-SETTLES"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_resolution_update_at(&mut strategy, 3_101.0, position_interval_end_ms);

    let expected = expected_hold_to_resolution_settlement(Leg::Yes, 0.45, 3_101.0);
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat)
            && settlement_evidence_count(&events) == 1
            && settlement_booking_error_count(&events) == 0
            && settlement_evidence_matches(&events, expected.realized_pnl)
            && settlement_market_ids(&events) == vec!["MKT-1".to_string()],
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: old-position resolution tick after roll must book with old strike/market; expected_realized_pnl={} exposure={:?} events={events:?}",
        expected.realized_pnl,
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_new_active_boundary_tick_does_not_settle_old_position() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-ROLLED-NEW-END-NO-SETTLE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    let new_active_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure next interval end");
    emit_resolution_update_at(&mut strategy, 3_201.0, new_active_interval_end_ms);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 0
            && matches!(strategy.exposure.state(),
                ExposureState::Managed(managed)
                    if managed.position_id == position.position_id
            ),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: next boundary tick must not book old position against new strike; exposure={:?} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_same_instrument_sync_preserves_captured_lifecycle() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-SAME-INSTRUMENT-SYNC"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");
    let captured_position = managed_position_snapshot(&strategy)
        .expect("position should be managed after materialization");
    let captured_lifecycle = captured_position.lifecycle.clone();
    let captured_book = captured_position.book.clone();

    strategy.active.price_to_beat = Some(3_200.0);
    strategy.active.interval_open = Some(3_200.0);
    strategy.active.interval_end_ms = Some(position_interval_end_ms.saturating_add(60_000));
    strategy.active.selection_published_at_ms = captured_lifecycle
        .selection_published_at_ms()
        .map(|published_at_ms| published_at_ms.saturating_add(1));
    strategy.active.seconds_to_expiry_at_selection = captured_lifecycle
        .seconds_to_expiry_at_selection()
        .map(|seconds_to_expiry| seconds_to_expiry.saturating_add(60));
    strategy.active.books.up.best_bid = Some(0.51);
    strategy.active.books.up.best_ask = Some(0.52);
    strategy.active.books.up.liquidity_available = Some(25.0);
    strategy.sync_exposure_context_from_active();

    let managed = managed_position_snapshot(&strategy).expect("position should remain managed");
    assert_eq!(
        managed.lifecycle.market_id(),
        captured_lifecycle.market_id(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must preserve captured market id"
    );
    assert_eq!(
        managed.lifecycle.outcome_side(),
        captured_lifecycle.outcome_side(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must preserve captured outcome side"
    );
    assert_eq!(
        managed.lifecycle.settlement_strike(),
        captured_lifecycle.settlement_strike(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must not erase captured strike"
    );
    assert_eq!(
        managed.lifecycle.interval_end_ms(),
        captured_lifecycle.interval_end_ms(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must not overwrite captured interval end"
    );
    assert_eq!(
        managed.lifecycle.selection_published_at_ms(),
        captured_lifecycle.selection_published_at_ms(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must preserve captured selection publish time"
    );
    assert_eq!(
        managed.lifecycle.seconds_to_expiry_at_selection(),
        captured_lifecycle.seconds_to_expiry_at_selection(),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must preserve captured selection expiry"
    );
    assert_eq!(
        managed.book.best_bid, captured_book.best_bid,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must not refresh the held book from a mismatched interval"
    );
    assert_eq!(
        managed.book.best_ask, captured_book.best_ask,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must not refresh the held book from a mismatched interval"
    );
    assert_eq!(
        managed.book.liquidity_available, captured_book.liquidity_available,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument active sync must not refresh held liquidity from a mismatched interval"
    );

    emit_resolution_update_at(&mut strategy, 3_101.0, position_interval_end_ms);

    let expected = expected_hold_to_resolution_settlement(Leg::Yes, 0.45, 3_101.0);
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        settlement_evidence_count(&events) == 1
            && settlement_booking_error_count(&events) == 0
            && settlement_evidence_matches(&events, expected.realized_pnl),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: settlement must still use captured lifecycle after same-instrument active sync; exposure={:?} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_same_instrument_sync_does_not_repair_missing_lifecycle_from_mismatched_interval()
 {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-SAME-INSTRUMENT-PARTIAL-LIFECYCLE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let captured_lifecycle = managed_position_snapshot(&strategy)
        .expect("position should be managed after materialization")
        .lifecycle
        .clone();
    let position_interval_end_ms = captured_lifecycle
        .interval_end_ms()
        .expect("fixture should configure position interval end");
    let partial_lifecycle = BoltV3PositionMarketLifecycle::from_entry_context(
        captured_lifecycle.market_id_owned(),
        captured_lifecycle.outcome_side(),
        None,
        None,
        Some(position_interval_end_ms),
        None,
        None,
    );
    let mut managed = strategy
        .exposure
        .managed_position_context()
        .expect("position should remain managed");
    managed.lifecycle = partial_lifecycle;
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::RefreshContext(managed),
    ));

    strategy.active.price_to_beat = Some(3_200.0);
    strategy.active.interval_open = Some(3_200.0);
    strategy.active.interval_end_ms = Some(position_interval_end_ms.saturating_add(60_000));
    strategy.active.selection_published_at_ms = captured_lifecycle
        .selection_published_at_ms()
        .map(|published_at_ms| published_at_ms.saturating_add(1));
    strategy.active.seconds_to_expiry_at_selection = captured_lifecycle
        .seconds_to_expiry_at_selection()
        .map(|seconds_to_expiry| seconds_to_expiry.saturating_add(60));
    strategy.sync_exposure_context_from_active();

    let managed = managed_position_snapshot(&strategy).expect("position should remain managed");
    assert_eq!(
        managed.lifecycle.settlement_strike(),
        None,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument sync must not repair missing strike from a mismatched interval"
    );
    assert_eq!(
        managed.lifecycle.selection_published_at_ms(),
        None,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument sync must not repair selection timing from a mismatched interval"
    );
    assert_eq!(
        managed.lifecycle.seconds_to_expiry_at_selection(),
        None,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: same-instrument sync must not repair expiry timing from a mismatched interval"
    );
}

#[test]
fn position_market_lifecycle_expired_book_deltas_do_not_submit_exits_after_roll() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );
    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );
    let (exec_handler, exec_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::exec_engine_queue_execute(),
        exec_handler,
    );
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-ROLLED-BOOK-DELTA-NO-EXIT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    clock.borrow_mut().set_time(UnixNanos::from(
        position_interval_end_ms.saturating_add(1) * NANOS_PER_MILLI_U64,
    ));
    strategy
        .on_book_deltas(&book_deltas(
            instrument_id,
            &[(BookAction::Update, OrderSide::Buy, 0.44, 500.0)],
        ))
        .expect("post-expiry book delta should not escape the actor loop");

    assert!(
        open_sell_exit_order_count(&cache, &position) == 0
            && risk_messages.get_messages().is_empty()
            && exec_messages.get_messages().is_empty()
            && matches!(strategy.exposure.state(),
                ExposureState::Managed(managed)
                    if managed.position_id == position.position_id
            ),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: book deltas after position expiry must not submit exits; exposure={:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_feed_outage_records_after_close_fetch_retry_budget_exhausted() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-ROLLED-FEED-OUTAGE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_settlement_close_retry_budget_events(&mut strategy, position_interval_end_ms);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && settlement_booking_error_reasons(&events)
                == vec![SettlementBookingErrorReason::ResolutionFeedMissing]
            && close_fetch_count == strategy.config.market_exit_max_attempts as usize,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: old-position feed outage must be recorded only after close-fetch retry budget exhaustion; exposure={:?} close_fetch_count={close_fetch_count} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_unroutable_close_fetch_records_terminal_booking_error() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-CLOSE-FETCH-NO-ROUTE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    strategy.config.resolution_client_id = None;
    strategy.config.resolution_instrument_id = None;
    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && settlement_booking_error_reasons(&events)
                == vec![SettlementBookingErrorReason::ResolutionFeedMissing]
            && close_fetch_count == 0,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: an unroutable settlement-close fetch must fail loud instead of retrying forever; exposure={:?} close_fetch_count={close_fetch_count} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_close_fetch_retry_waits_for_retry_interval() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-CLOSE-FETCH-RETRY-PACING"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(2));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        close_fetch_count == 1 && settlement_booking_error_count(&events) == 0,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: close-fetch retries must be paced by retry_interval_seconds, not by every time event; close_fetch_count={close_fetch_count} events={events:?}",
    );
}

#[test]
fn position_market_lifecycle_close_fetch_exhaustion_waits_for_retry_interval() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    strategy.config.market_exit_max_attempts = 1;
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-CLOSE-FETCH-TERMINAL-PACING"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(2));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        close_fetch_count == 1 && settlement_booking_error_count(&events) == 0,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: close-fetch exhaustion must wait one retry interval after the final fetch; close_fetch_count={close_fetch_count} events={events:?}",
    );
}

#[test]
fn position_market_lifecycle_selection_blocked_issues_own_settlement_close_fetch() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-SELECTION-BLOCKED-CLOSE-FETCH"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    strategy.active = ActiveMarketState::idle();
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_events = settlement_close_fetch_events(&strategy);
    assert!(
        close_events.len() == 1
            && close_events[0].trigger == ResolutionStrikeFetchTrigger::CustomFetch
            && close_events[0].boundary_unix_seconds
                == position_interval_end_ms / MILLIS_PER_SECOND_U64
            && settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 0
            && matches!(strategy.exposure.state(), ExposureState::Managed(_)),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: selection-blocked held position must issue its own WindowCloseSettlement fetch without terminal outage evidence; exposure={:?} close_events={close_events:?} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_close_and_open_fetches_use_boundary_scoped_durable_slots() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-CLOSE-OPEN-BOUNDARY-SLOTS"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    let event_start = strategy.resolution_strike_subscribe_events.len();
    strategy.active = ActiveMarketState::idle();
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));
    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);

    let events = strategy.resolution_strike_subscribe_events[event_start..].to_vec();
    let close_events = events
        .iter()
        .filter(|event| event.report_boundary == ResolutionStrikeReportBoundary::WindowClose)
        .collect::<Vec<_>>();
    let open_events = events
        .iter()
        .filter(|event| event.report_boundary == ResolutionStrikeReportBoundary::WindowOpen)
        .collect::<Vec<_>>();
    assert!(
        close_events.len() == 1
            && close_events[0].trigger == ResolutionStrikeFetchTrigger::CustomFetch
            && open_events.len() == 1
            && open_events[0].trigger == ResolutionStrikeFetchTrigger::DurableIndex
            && close_events[0].boundary_unix_seconds
                == position_interval_end_ms / MILLIS_PER_SECOND_U64
            && open_events[0].boundary_unix_seconds
                == position_interval_end_ms / MILLIS_PER_SECOND_U64,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: window-close and window-open resolution fetches must not share a single durable subscription slot; events={events:?}",
    );
}

#[test]
fn position_market_lifecycle_late_matching_resolution_tick_after_watchdog_books_settlement() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-ROLLED-LATE-RESOLUTION"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");

    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));
    emit_resolution_update_at(&mut strategy, 3_200.0, position_interval_end_ms);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 1
            && settlement_booking_error_count(&events) == 0
            && matches!(strategy.exposure.state(), ExposureState::Flat)
            && close_fetch_count == 1,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: late matching resolution tick after watchdog must still book settlement; exposure={:?} close_fetch_count={close_fetch_count} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn position_market_lifecycle_recovered_expired_cache_position_records_terminal_booking_error_after_roll()
 {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let cache = register_test_strategy(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "expired-recovery-market",
        "MKT-1",
        "Up",
        1_000,
        position_interval_end_ms,
    );
    let position_id = PositionId::from("P-RECOVERED-EXPIRED-CACHE");
    let fill = order_filled_event(
        ClientOrderId::from("RECOVERED-EXPIRED-CACHE-ORDER"),
        instrument.id(),
        Some(position_id),
        OrderSide::Buy,
    );
    let cache_position = Position::new(&instrument, fill);
    {
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(instrument)
            .expect("test cache should accept expired recovery instrument");
        cache
            .add_position(&cache_position, NtOmsType::Netting)
            .expect("test cache should accept expired recovery position");
    }
    let scope_position = OpenPositionState {
        episode: position_episode_for_test(instrument_id, position_id),
        lifecycle: BoltV3PositionMarketLifecycle::missing(),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.45,
        book: OutcomeBookState::from_instrument_id(instrument_id),
    };
    let settlement_key = settlement_key_for_position(&scope_position)
        .expect("fixture cache position should derive settlement key");

    strategy.bootstrap_recovery_from_cache();
    roll_active_to_next_interval(&mut strategy, position_interval_end_ms, 3_200.0);
    emit_settlement_close_retry_budget_events(&mut strategy, position_interval_end_ms);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let close_fetch_count = settlement_close_fetch_event_count(&strategy);
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && close_fetch_count == strategy.config.market_exit_max_attempts as usize
            && strategy
                .settlement_booking_error_keys
                .contains(&settlement_key)
            // #1349: terminal booking-error releases exposure (Flat).
            && matches!(strategy.exposure.state(), ExposureState::Flat)
            && terminal_settlement_lifecycle_count(&events) == 1,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: recovered expired cache position must record a terminal booking-error after close-fetch retry exhaustion and release exposure to Flat; exposure={:?} close_fetch_count={close_fetch_count} events={events:?}",
        strategy.exposure,
    );
    let transitions = health_transitions
        .lock()
        .expect("recording settlement health transition mutex poisoned");
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].reason, "market_expired");
}

#[test]
fn position_market_lifecycle_recovered_position_missing_instrument_records_terminal_booking_error()
{
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let cache = register_test_strategy(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position_interval_end_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure position interval end");
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "expired-recovery-market-missing-instrument",
        "MKT-1",
        "Up",
        1_000,
        position_interval_end_ms,
    );
    let position_id = PositionId::from("P-RECOVERED-MISSING-INSTRUMENT");
    let fill = order_filled_event(
        ClientOrderId::from("RECOVERED-MISSING-INSTRUMENT-ORDER"),
        instrument.id(),
        Some(position_id),
        OrderSide::Buy,
    );
    let cache_position = Position::new(&instrument, fill);
    {
        let mut cache = cache.borrow_mut();
        cache
            .add_position(&cache_position, NtOmsType::Netting)
            .expect("test cache should accept position without instrument metadata");
    }
    let scope_position = OpenPositionState {
        episode: position_episode_for_test(instrument_id, position_id),
        lifecycle: BoltV3PositionMarketLifecycle::missing(),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.45,
        book: OutcomeBookState::from_instrument_id(instrument_id),
    };
    let settlement_key = settlement_key_for_position(&scope_position)
        .expect("fixture cache position should derive settlement key");

    strategy.bootstrap_recovery_from_cache();
    emit_time_event_at(&mut strategy, position_interval_end_ms.saturating_add(1));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && settlement_booking_error_reasons(&events)
                == vec![SettlementBookingErrorReason::SettlementInputInvalid]
            && strategy
                .settlement_booking_error_keys
                .contains(&settlement_key)
            // #1349: terminal booking-error releases exposure (Flat).
            && matches!(strategy.exposure.state(), ExposureState::Flat)
            && terminal_settlement_lifecycle_count(&events) == 1,
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: recovered cache position with missing instrument metadata must record a terminal booking-error and release exposure to Flat; exposure={:?} events={events:?}",
        strategy.exposure,
    );
    let transitions = health_transitions
        .lock()
        .expect("recording settlement health transition mutex poisoned");
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].reason, "recovery_unknown_interval");
}

#[test]
fn position_market_lifecycle_recovered_missing_interval_book_delta_records_error_not_exit() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    let (risk_handler, risk_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::risk_engine_queue_execute(),
        risk_handler,
    );
    let (exec_handler, exec_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::exec_engine_queue_execute(),
        exec_handler,
    );

    let instrument_id =
        InstrumentId::from("condition-RECOVERED-BOOK-DELTA-RECOVERED-BOOK-DELTA-UP.POLYMARKET");
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "recovered-book-delta-missing-interval",
        "RECOVERED-BOOK-DELTA",
        "Up",
        1_000,
        0,
    );
    let position_id = PositionId::from("P-RECOVERED-MISSING-INTERVAL-BOOK-DELTA");
    let fill = order_filled_event(
        ClientOrderId::from("RECOVERED-MISSING-INTERVAL-ORDER"),
        instrument.id(),
        Some(position_id),
        OrderSide::Buy,
    );
    let cache_position = Position::new(&instrument, fill);
    {
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(instrument.clone())
            .expect("test cache should accept malformed recovered instrument");
        cache
            .add_position(&cache_position, NtOmsType::Netting)
            .expect("test cache should accept recovered position");
    }
    let scope_position = OpenPositionState {
        episode: position_episode_for_test(instrument_id, position_id),
        lifecycle: BoltV3PositionMarketLifecycle::recover_from_instrument(Some(&instrument)),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.45,
        book: OutcomeBookState::from_instrument_id(instrument_id),
    };
    let settlement_key = settlement_key_for_position(&scope_position)
        .expect("fixture cache position should derive settlement key");

    strategy.bootstrap_recovery_from_cache();
    strategy.active.phase = SelectionPhase::Freeze;
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(1_200 * NANOS_PER_MILLI_U64));
    strategy
        .on_book_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.44, 500.0),
                (BookAction::Add, OrderSide::Sell, 0.46, 500.0),
            ],
        ))
        .expect("book delta should not escape the actor loop");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        settlement_evidence_count(&events) == 0
            && settlement_booking_error_count(&events) == 1
            && settlement_booking_error_reasons(&events)
                == vec![SettlementBookingErrorReason::SettlementInputInvalid]
            && strategy
                .settlement_booking_error_keys
                .contains(&settlement_key)
            && open_sell_exit_order_count(&cache, &scope_position) == 0
            && risk_messages.get_messages().is_empty()
            && exec_messages.get_messages().is_empty()
            // #1349: terminal booking-error releases exposure (Flat) so the
            // single-exposure strategy is not parked; exit path stays blocked
            // by booking-error key, not Managed occupancy.
            && matches!(strategy.exposure.state(), ExposureState::Flat),
        "{POSITION_MARKET_LIFECYCLE_PINNED_FAILURE}: recovered position with missing interval must record terminal booking-error, release exposure to Flat, and block forced-flat book-delta exit; exposure={:?} events={events:?}",
        strategy.exposure,
    );
}

#[test]
fn settlement_preserves_live_exit_until_late_terminal_then_stays_flat_without_double_booking() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-AFTER-SETTLEMENT");
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));
    let expired = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Expired(expired.clone()));
    strategy.on_order_expired(expired);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_evidence_count(&events), 1);
    assert_eq!(settlement_booking_error_count(&events), 0);
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn settlement_during_sink_unknown_releases_only_on_correlated_denial_proof() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-SETTLED-SINK-UNKNOWN-DENIAL"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-SETTLED-SINK-UNKNOWN-DENIAL");
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create sealed exit authority");
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));
    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("managed exposure should grant the exit route");
    let generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation,
            managed: exit.position,
            pending_exit: exit.pending_exit,
            authority: exit.authority,
        })
        .expect("exit route should bind its sealed payload");
    participant
        .consume_at_pre_sink()
        .expect("exit route should consume before the sink");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));

    emit_resolution_update(&mut strategy, 3_101.0);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));
    strategy.on_order_denied(order_denied_event_with_reason(
        ClientOrderId::from("EXIT-SETTLED-SINK-UNKNOWN-FOREIGN-DENIAL"),
        instrument_id,
        "foreign denial",
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));

    strategy.on_order_denied(order_denied_event_with_reason(
        exit_client_order_id,
        instrument_id,
        "correlated denial proves the exit did not reach the venue",
    ));

    let facts = evidence
        .recorded_facts()
        .expect("settled sink-unknown evidence should decode");
    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat),
        "correlated denial should apply the recorded settlement after restoring the prior exposure; exposure={:?} settled_keys={:?} terminal_keys={:?} facts={facts:?}",
        strategy.exposure,
        strategy.settled_position_keys,
        strategy.terminal_settlement_keys,
    );
    assert_eq!(settlement_evidence_count(&facts), 1);
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownResolved
                && record.outcome == OrderLifecycleOutcome::Flat
                && record.client_order_id.as_deref() == Some(exit_client_order_id.as_str())
    )));
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownResolved
                && record.client_order_id.as_deref()
                    == Some("EXIT-SETTLED-SINK-UNKNOWN-FOREIGN-DENIAL")
    )));
}

#[test]
fn terminal_booking_error_during_sink_unknown_releases_on_correlated_denial_proof() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-TERMINAL-ERROR-SINK-UNKNOWN"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-ERROR-SINK-UNKNOWN");
    set_exit_pending(
        &mut strategy,
        position.clone(),
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create sealed exit authority");
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));
    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("managed exposure should grant the exit route");
    let generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation,
            managed: exit.position,
            pending_exit: exit.pending_exit,
            authority: exit.authority,
        })
        .expect("exit route should bind its sealed payload");
    participant
        .consume_at_pre_sink()
        .expect("exit route should consume before the sink");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);

    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive its settlement key");
    let terminal_ns = position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);
    strategy
        .record_settlement_booking_error(
            &position,
            settlement_key.clone(),
            SettlementBookingErrorReason::SettlementInputInvalid,
            "terminal booking error arrived while exit dispatch was unknown".to_string(),
            terminal_ns,
        )
        .expect("terminal booking error should persist behind sink-unknown authority");

    assert!(
        matches!(
            strategy.exposure.state(),
            ExposureState::OperationSinkUnknown(_)
        ) && strategy.terminal_settlement_keys.contains(&settlement_key)
    );
    let terminal_facts = evidence
        .recorded_facts()
        .expect("sink-unknown terminal booking evidence should decode");
    assert!(terminal_facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::TerminalSettlement(record)
            if record.lifecycle.outcome == OrderLifecycleOutcome::ExitPending
                && record.lifecycle.client_order_id.is_none()
                && record.lifecycle.position_id.as_deref() == Some(position.position_id.as_str())
    )));

    strategy.on_order_denied(order_denied_event_with_reason(
        ClientOrderId::from("EXIT-TERMINAL-ERROR-SINK-UNKNOWN-FOREIGN"),
        instrument_id,
        "foreign denial",
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));

    strategy.on_order_denied(order_denied_event_with_reason(
        exit_client_order_id,
        instrument_id,
        "correlated denial proves the exit did not reach the venue",
    ));

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    let released_facts = evidence
        .recorded_facts()
        .expect("sink-unknown settlement release evidence should decode");
    assert!(released_facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownResolved
                && record.outcome == OrderLifecycleOutcome::Flat
                && record.client_order_id.as_deref() == Some(exit_client_order_id.as_str())
    )));
}

#[test]
fn terminal_booking_error_retains_exit_evidence_until_late_terminal_release() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        PositionId::from("P-TERMINAL-BOOKING-ERROR-LATE-EXIT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-BOOKING-ERROR-LATE");
    set_exit_pending(
        &mut strategy,
        position.clone(),
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive its settlement key");
    let terminal_ns = position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);

    strategy
        .record_settlement_booking_error(
            &position,
            settlement_key,
            SettlementBookingErrorReason::SettlementInputInvalid,
            "terminal booking error arrived before exit terminal proof".to_string(),
            terminal_ns,
        )
        .expect("terminal booking error should persist");

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));
    let before_terminal = evidence
        .recorded_facts()
        .expect("terminal booking-error evidence should decode");
    assert!(before_terminal.iter().any(|fact| matches!(
        fact,
        CurrentFact::TerminalSettlement(record)
            if record.lifecycle.transition
                == OrderLifecycleTransition::SettlementBookingTerminal
                && record.lifecycle.outcome == OrderLifecycleOutcome::ExitPending
                && record.lifecycle.position_id.as_deref() == Some(position.position_id.as_str())
    )));

    let expired = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Expired(expired.clone()));
    strategy.on_order_expired(expired);

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    let after_terminal = evidence
        .recorded_facts()
        .expect("late terminal release evidence should decode");
    assert!(after_terminal.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OrderExpired
                && record.outcome == OrderLifecycleOutcome::Flat
                && record.client_order_id.as_deref() == Some(exit_client_order_id.as_str())
    )));
}

#[test]
fn terminal_before_settlement_remanages_residual_then_books_residual_settlement() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-TERMINAL-BEFORE-SETTLEMENT");
    set_exit_pending(
        &mut strategy,
        position.clone(),
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position.position_id,
        Quantity::new(6.0, 2),
        position.avg_px_open,
        OrderSide::Buy,
    );

    let expired = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(&mut strategy, OrderEventAny::Expired(expired.clone()));
    strategy.on_order_expired(expired);
    assert!(
        matches!(strategy.exposure.state(), ExposureState::Managed(_))
            && managed_position_snapshot(&strategy)
                .is_some_and(|managed| managed.quantity == Quantity::new(6.0, 2)),
        "terminal before settlement must re-manage the known residual before resolution; exposure={:?}",
        strategy.exposure
    );

    emit_resolution_update(&mut strategy, 3_101.0);
    let expected =
        expected_hold_to_resolution_settlement_for_quantity(Leg::Yes, 0.45, 3_101.0, 6.0);
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat)
            && settlement_evidence_matches(&events, expected.realized_pnl)
            && settlement_evidence_count(&events) == 1,
        "residual settlement should book exactly the residual quantity after terminal-before-settlement; expected_realized_pnl={}, exposure={:?}, events={events:?}",
        expected.realized_pnl,
        strategy.exposure
    );
}

#[test]
fn booked_settlement_routes_to_runtime_sink_and_flattening() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let sink = Rc::new(RecordingSettlementRuntimeSink::default());
    attach_settlement_runtime_sink(&mut strategy, sink.clone());
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RUNTIME-WIN"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );

    emit_resolution_update(&mut strategy, 3_101.0);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let loss_observations = sink.loss_observations();
    assert_eq!(settlement_evidence_count(&events), 1);
    assert_eq!(loss_observations.len(), 1);
    let settlement_key = events
        .iter()
        .find_map(|event| match event {
            CurrentFact::Settlement(fact) => Some(fact.settlement_key.as_str()),
            _ => None,
        })
        .expect("settlement fact should carry the reducer idempotency key");
    assert_eq!(
        loss_observations[0].event_id.as_deref(),
        Some(settlement_key)
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    emit_resolution_update(&mut strategy, 3_101.0);
    assert_eq!(
        evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .len(),
        events.len(),
        "same settlement key must suppress duplicate booking after runtime calls"
    );
    assert_eq!(sink.loss_observations().len(), 1);
}

#[test]
fn loss_reducer_failure_after_settled_key_insert_enters_blind_recovery() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let sink = Rc::new(LossFailingSettlementRuntimeSink::default());
    let sink_handle: crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkHandle =
        sink.clone();
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_runtime_sink(Some(sink_handle));
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RUNTIME-SINK-FAIL"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive settlement key");
    let position_id = position.position_id.to_string();

    try_emit_resolution_update(&mut strategy, 3_101.0)
        .expect("post-evidence loss-reducer failure should fail closed without bubbling");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_evidence_count(&events), 1);
    assert_eq!(settlement_booking_error_count(&events), 0);
    assert!(strategy.settled_position_keys.contains(&settlement_key));
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                CurrentFact::OrderLifecycle(evidence)
                    if evidence.transition
                        == OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked
                        && evidence.outcome == OrderLifecycleOutcome::BlindRecovery
                        && evidence.position_id.as_deref() == Some(position_id.as_str())
            )
        }),
        "post-settled-key loss-reducer failure must write durable blind-recovery lifecycle evidence: {events:?}"
    );
    assert_eq!(sink.loss_observation_count(), 1);
    assert!(
        matches!(
            strategy.exposure.state(),
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::SettlementEvidenceRecoveryFailed,
                ..
            })
        ),
        "post-settled-key loss-reducer failure must enter blind settlement recovery; exposure={:?}",
        strategy.exposure
    );

    try_emit_resolution_update(&mut strategy, 3_101.0)
        .expect("blind recovery must not retry the committed settlement");
    let replayed_events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(
        settlement_evidence_count(&replayed_events),
        1,
        "post-commit reducer failure must never re-append the settlement fact"
    );
}

fn assert_settlement_evidence_failure_precedes_runtime_effects(
    evidence: Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) {
    assert_reality_fixtures();

    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );
    let sink = Rc::new(RecordingSettlementRuntimeSink::default());
    attach_settlement_runtime_sink(&mut strategy, sink.clone());
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EVIDENCE-BEFORE-RUNTIME"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive settlement key");

    try_emit_resolution_update(&mut strategy, 3_101.0)
        .expect_err("settlement evidence failure must stop before runtime reducers");

    assert!(sink.loss_observations().is_empty());
    assert!(
        !strategy.settled_position_keys.contains(&settlement_key),
        "an uncommitted settlement must not latch its idempotency key"
    );
}

#[test]
fn settlement_write_failure_precedes_loss_runtime_effects() {
    assert_settlement_evidence_failure_precedes_runtime_effects(failing_decision_evidence());
}

#[test]
fn settlement_sync_failure_precedes_loss_runtime_effects() {
    assert_settlement_evidence_failure_precedes_runtime_effects(sync_failing_decision_evidence());
}

#[test]
fn losing_settlement_moves_durable_loss_governor() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission.clone(),
    );
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let settlement_account_id = strategy
        .context
        .settlement_account_id()
        .expect("fixture strategy should derive settlement account id")
        .to_string();
    let temp = tempfile::tempdir().expect("loss-governor tempdir should create");
    let loss_protection = crate::bolt_v3_loss_protection::KillSwitchLossProtection::new(
        settlement_loss_config(instrument_id, &settlement_account_id),
        submit_admission,
        crate::bolt_v3_kill_switch_store::KillSwitchStore::new(
            temp.path().join("kill-switch.json"),
            TEST_LOSS_STATE_MAX_BYTES,
        ),
        Rc::new(NoopLossActionSink),
    )
    .expect("loss protection should initialize");
    let sink = Rc::new(DurableLossSettlementRuntimeSink::new(loss_protection));
    let sink_handle: crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkHandle =
        sink.clone();
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_runtime_sink(Some(sink_handle));
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RUNTIME-LOSS"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive settlement key");

    emit_resolution_update(&mut strategy, 3_099.0);

    let loss_snapshot = sink.loss_snapshot();
    assert!(
        loss_snapshot.daily_realized_pnl < Decimal::ZERO,
        "losing settlement must move the durable loss-governor accumulator: {loss_snapshot:?}"
    );
    assert!(
        loss_snapshot
            .adjusted_position_pnl
            .contains_key(&settlement_key),
        "settlement-key dedupe entry should persist with the realized-PnL snapshot: {loss_snapshot:?}"
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn missing_settlement_currency_records_booking_error_from_config_derived_fixture() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let fixture_account_id = fixture_settlement_account_id();
    assert_eq!(
        strategy.context.settlement_account_id(),
        Some(fixture_account_id.as_str())
    );
    assert_eq!(
        strategy.context.settlement_currency(),
        Some(fixture_settlement_currency())
    );
    strategy.context = strategy.context.clone().with_settlement_currency(None);
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-MISSING-SETTLEMENT-CURRENCY"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );

    emit_resolution_update(&mut strategy, 3_101.0);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_evidence_count(&events), 0);
    assert_eq!(settlement_booking_error_count(&events), 1);
    // #1349: terminal booking-error releases single-exposure occupancy.
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert_eq!(terminal_settlement_lifecycle_count(&events), 1);
}

#[test]
fn missing_settlement_account_records_booking_error_from_config_derived_fixture() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let fixture_account_id = fixture_settlement_account_id();
    assert_eq!(
        strategy.context.settlement_account_id(),
        Some(fixture_account_id.as_str())
    );
    assert_eq!(
        strategy.context.settlement_currency(),
        Some(fixture_settlement_currency())
    );
    let sink = Rc::new(RecordingSettlementRuntimeSink::default());
    attach_settlement_runtime_sink(&mut strategy, sink.clone());
    strategy.context = strategy.context.clone().with_settlement_account_id(None);
    let (_cache, _clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-MISSING-SETTLEMENT-ACCOUNT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );

    emit_resolution_update(&mut strategy, 3_101.0);

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_evidence_count(&events), 0);
    assert_eq!(settlement_booking_error_count(&events), 1);
    assert!(sink.loss_observations().is_empty());
    // #1349: terminal booking-error releases single-exposure occupancy.
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert_eq!(terminal_settlement_lifecycle_count(&events), 1);
}

#[test]
fn distinct_terminal_booking_error_keys_each_record_lifecycle_and_release_exposure() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let first_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-TERMINAL-BOOKING-ERROR-FIRST"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let first_key = settlement_key_for_position(&first_position)
        .expect("first fixture position should derive a settlement key");
    let first_terminal_ns = first_position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);

    strategy
        .record_settlement_booking_error(
            &first_position,
            first_key.clone(),
            SettlementBookingErrorReason::SettlementInputInvalid,
            "first distinct terminal booking error".to_string(),
            first_terminal_ns,
        )
        .expect("first terminal booking error should be recorded");
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    close_nt_position(&mut strategy, first_position.position_id);

    let second_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-TERMINAL-BOOKING-ERROR-SECOND"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let second_key = settlement_key_for_position(&second_position)
        .expect("second fixture position should derive a settlement key");
    let second_terminal_ns = second_position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);
    assert_ne!(first_key, second_key);

    strategy
        .record_settlement_booking_error(
            &second_position,
            second_key.clone(),
            SettlementBookingErrorReason::SettlementInputInvalid,
            "second distinct terminal booking error".to_string(),
            second_terminal_ns,
        )
        .expect("second terminal booking error should be recorded");
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    let terminal_position_ids = events
        .iter()
        .filter_map(|event| match event {
            CurrentFact::TerminalSettlement(evidence)
                if evidence.lifecycle.transition
                    == OrderLifecycleTransition::SettlementBookingTerminal =>
            {
                evidence.lifecycle.position_id.clone()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(settlement_booking_error_count(&events), 2);
    let recorded_health_transitions = health_transitions
        .lock()
        .expect("recording settlement health transition mutex poisoned")
        .clone();
    assert_eq!(recorded_health_transitions.len(), 2);
    assert_eq!(recorded_health_transitions[0].settlement_key, first_key);
    assert_eq!(recorded_health_transitions[1].settlement_key, second_key);
    assert_eq!(
        terminal_position_ids,
        vec![
            first_position.position_id.to_string(),
            second_position.position_id.to_string(),
        ],
        "each distinct settlement booking-error key must emit terminal lifecycle evidence"
    );

    strategy
        .record_settlement_booking_error(
            &second_position,
            recorded_health_transitions[1].settlement_key.clone(),
            SettlementBookingErrorReason::SettlementInputInvalid,
            "duplicate terminal booking error".to_string(),
            second_terminal_ns,
        )
        .expect("duplicate terminal booking error should be idempotent");
    assert_eq!(
        settlement_booking_error_count(
            &evidence
                .recorded_facts()
                .expect("recorded current evidence must decode")
        ),
        2
    );
    assert_eq!(
        health_transitions
            .lock()
            .expect("recording settlement health transition mutex poisoned")
            .len(),
        2,
        "health reporting must follow canonical terminal transitions"
    );
}

#[test]
fn terminal_settlement_uses_one_canonical_durable_event() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-ATOMIC-TERMINAL-SETTLEMENT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive settlement key");
    let terminal_ns = position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);

    strategy
        .record_settlement_booking_error(
            &position,
            settlement_key,
            SettlementBookingErrorReason::SettlementInputInvalid,
            "terminal evidence must use one durable append".to_string(),
            terminal_ns,
        )
        .expect("canonical terminal settlement evidence must append once");

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_booking_error_count(&events), 1);
    assert_eq!(terminal_settlement_lifecycle_count(&events), 1);
    assert_eq!(
        health_transitions
            .lock()
            .expect("recording settlement health transition mutex poisoned")
            .len(),
        1
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn health_emitter_failure_cannot_park_exposure_or_duplicate_terminal_evidence() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded_attempts = health_attempts.clone();
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_health_transition_emitter(Some(Arc::new(move |_| {
            recorded_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("injected settlement health emitter failure")
        })));
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-HEALTH-EMITTER-FAILURE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive settlement key");
    let terminal_ns = position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64);

    for _ in 0..2 {
        strategy
            .record_settlement_booking_error(
                &position,
                settlement_key.clone(),
                SettlementBookingErrorReason::SettlementInputInvalid,
                "health failure must follow durable release".to_string(),
                terminal_ns,
            )
            .expect("health reporting failure must not fail terminal release");
    }

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert_eq!(
        settlement_booking_error_count(
            &evidence
                .recorded_facts()
                .expect("recorded current evidence must decode")
        ),
        1
    );
    assert_eq!(health_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn lifecycle_write_failure_preserves_transition_and_source_context() {
    let evidence = failing_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence,
        submit_admission,
    );

    let error = strategy
        .persist_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition: OrderLifecycleTransition::SettlementBookingTerminal,
            outcome: OrderLifecycleOutcome::Flat,
            source: ORDER_LIFECYCLE_SOURCE_SETTLEMENT_BOOKING_TERMINAL,
            market_id: Some("MKT-LIFECYCLE-FAILURE".to_string()),
            instrument_id: None,
            position_id: None,
            client_order_id: None,
            prior_client_order_id: None,
            raw_reason_text: None,
            order_side: None,
            filled_quantity: None,
            residual_quantity: None,
            ts_event_ns: None,
        })
        .expect_err("fixture lifecycle writer should fail");
    let rendered = format!("{error:#}");
    assert!(rendered.contains("transition=SettlementBookingTerminal"));
    assert!(rendered.contains("source=SettlementBookingTerminal"));
}

#[test]
fn live_manageable_nonterminal_position_cannot_enter_terminal_settlement_transition() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-NONTERMINAL-SETTLEMENT-GUARD"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let settlement_key = settlement_key_for_position(&position)
        .expect("fixture position should derive a settlement key");
    let before_expiry_ns = position
        .lifecycle
        .interval_end_ms()
        .expect("live fixture position must retain an interval end")
        .saturating_mul(NANOS_PER_MILLI_U64)
        .saturating_sub(1);

    let error = strategy
        .record_settlement_booking_error(
            &position,
            settlement_key,
            SettlementBookingErrorReason::SettlementInputInvalid,
            "nonterminal position must not release".to_string(),
            before_expiry_ns,
        )
        .expect_err("nonterminal position must be ineligible for terminal settlement");

    assert!(error.to_string().contains("ineligible"));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));
    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert_eq!(settlement_booking_error_count(&events), 0);
    assert_eq!(terminal_settlement_lifecycle_count(&events), 0);
    assert!(
        health_transitions
            .lock()
            .expect("recording settlement health transition mutex poisoned")
            .is_empty()
    );
}

#[test]
fn restart_reconstructs_expired_terminal_transition_from_durable_booking_error() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let health_transitions = attach_recording_settlement_health_transitions(&mut strategy);
    let (cache, clock) = register_test_strategy_with_clock(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "terminal-restart-market",
        "terminal-restart-market",
        "Up",
        1_000,
        2_000,
    );
    let position_id = PositionId::from("P-TERMINAL-RESTART");
    let fill = order_filled_event(
        ClientOrderId::from("TERMINAL-RESTART-ORDER"),
        instrument.id(),
        Some(position_id),
        OrderSide::Buy,
    );
    let position = Position::new(&instrument, fill);
    {
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(instrument)
            .expect("test cache should accept recovery instrument");
        cache
            .add_position(&position, NtOmsType::Netting)
            .expect("test cache should accept recovery position");
    }
    clock.borrow_mut().set_time(UnixNanos::from(
        2_500_u64.saturating_mul(NANOS_PER_MILLI_U64),
    ));
    let recovered_position = OpenPositionState {
        episode: position_episode_for_test(instrument_id, position_id),
        lifecycle: BoltV3PositionMarketLifecycle::recover_from_instrument(
            cache.borrow().instrument(&instrument_id),
        ),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.45,
        book: OutcomeBookState::from_instrument_id(instrument_id),
    };
    let settlement_key = settlement_key_for_position(&recovered_position)
        .expect("recovered position should derive settlement key");
    evidence
        .record_terminal_settlement(crate::bolt_v3_current_evidence::TerminalSettlementFact {
            settlement_key: settlement_key.clone(),
            booking_error: crate::bolt_v3_current_evidence::SettlementBookingErrorFact {
                strategy_id: strategy.config.strategy_id.clone(),
                settlement_key: settlement_key.clone(),
                market_id: recovered_position.lifecycle.market_id_owned(),
                position_id: Some(position_id.to_string()),
                instrument_id: Some(instrument_id.to_string()),
                resolution_instrument_id: strategy
                    .resolution_instrument_id()
                    .map(|instrument_id| instrument_id.to_string()),
                reason: SettlementBookingErrorReason::SettlementInputInvalid,
                detail: "durable terminal booking error".to_string(),
                observed_at_ns: 2_000_u64.saturating_mul(NANOS_PER_MILLI_U64),
            },
            lifecycle: crate::bolt_v3_current_evidence::OrderLifecycleFact {
                strategy_id: strategy.config.strategy_id.clone(),
                transition: OrderLifecycleTransition::SettlementBookingTerminal,
                outcome: OrderLifecycleOutcome::Flat,
                source: OrderLifecycleSource::SettlementBookingTerminal,
                market_id: recovered_position.lifecycle.market_id_owned(),
                instrument_id: Some(instrument_id.to_string()),
                position_id: Some(position_id.to_string()),
                client_order_id: None,
                prior_client_order_id: None,
                raw_reason_text: Some("durable terminal booking error".to_string()),
                order_side: Some(EvidenceOrderSide::Buy),
                filled_quantity: None,
                residual_quantity: Some("0".to_string()),
                ts_event_ns: Some(2_000_u64.saturating_mul(NANOS_PER_MILLI_U64)),
            },
        })
        .expect("current terminal-settlement evidence should append");
    let (_, settlement_recovery, booking_recovery) = evidence
        .startup_recovery_projections(
            crate::bolt_v3_current_evidence::PositiveFiniteEvidenceReadCap::new(100_000)
                .expect("test recovery cap must be positive and finite"),
        )
        .expect("current booking-error evidence should reconstruct");
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_recovery(Some(Arc::new(settlement_recovery)))
        .with_booking_recovery(Some(Arc::new(booking_recovery)));

    strategy.bootstrap_recovery_from_cache();

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert_eq!(
        terminal_settlement_lifecycle_count(
            &evidence
                .recorded_facts()
                .expect("current evidence should decode"),
        ),
        1
    );
    let transitions = health_transitions
        .lock()
        .expect("recording settlement health transition mutex poisoned");
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].settlement_key, settlement_key);
    assert_eq!(transitions[0].reason, "market_expired");
    assert_eq!(
        settlement_booking_error_count(
            &evidence
                .recorded_facts()
                .expect("current evidence should decode"),
        ),
        1,
        "restart must not append a parallel booking-error record"
    );

    drop(transitions);
    strategy.bootstrap_recovery_from_cache();
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert_eq!(
        terminal_settlement_lifecycle_count(
            &evidence
                .recorded_facts()
                .expect("current evidence should decode"),
        ),
        1,
        "restart must not append duplicate canonical terminal evidence"
    );
}

#[test]
fn startup_settlement_recovery_replays_evidence_from_real_cache_positions() {
    assert_reality_fixtures();

    let evidence = recording_decision_evidence();
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
    );
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    let sink = Rc::new(RecordingSettlementRuntimeSink::default());
    let sink_handle: crate::bolt_v3_settlement_runtime::BoltV3SettlementRuntimeSinkHandle =
        sink.clone();
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_runtime_sink(Some(sink_handle));
    let cache = register_test_strategy(&mut strategy);
    let instrument_id = held_instrument_id(&strategy, Leg::Yes);
    let instrument = updown_binary_option(
        instrument_id.to_string().as_str(),
        "settlement-recovery-market",
        "settlement-recovery-market",
        "Up",
        1_000,
        2_000,
    );
    let position_id = PositionId::from("P-SETTLEMENT-RECOVERY-CACHE");
    let fill = order_filled_event(
        ClientOrderId::from("SETTLEMENT-RECOVERY-ORDER"),
        instrument.id(),
        Some(position_id),
        OrderSide::Buy,
    );
    let position = Position::new(&instrument, fill);
    {
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(instrument)
            .expect("test cache should accept recovery instrument");
        cache
            .add_position(&position, NtOmsType::Netting)
            .expect("test cache should accept recovery position");
    }
    let scope_position = OpenPositionState {
        episode: position_episode_for_test(instrument_id, position_id),
        lifecycle: BoltV3PositionMarketLifecycle::missing(),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.45,
        book: OutcomeBookState::from_instrument_id(instrument_id),
    };
    let settlement_key = settlement_key_for_position(&scope_position)
        .expect("fixture cache position should derive settlement key");
    let _committed = evidence
        .record_settlement(crate::bolt_v3_current_evidence::SettlementFact {
            strategy_id: strategy.config.strategy_id.clone(),
            settlement_key: settlement_key.clone(),
            market_id: "settlement-recovery-market".to_string(),
            position_id: position_id.to_string(),
            instrument_id: instrument_id.to_string(),
            product_id: prediction_market_product_id_from_instrument_id(&instrument_id)
                .expect("fixture instrument id should derive product id"),
            outcome_side: crate::bolt_v3_current_evidence::OutcomeSide::Up,
            entry_order_side: EvidenceOrderSide::Buy,
            quantity: "10".to_string(),
            entry_price: "0.45".to_string(),
            family_key: crate::bolt_v3_market_families::updown::KEY.to_string(),
            strike_price: "3100".to_string(),
            resolution_instrument_id: "RESOLUTION.SOURCE".to_string(),
            resolution_ts_event_ns: 1_300_000_000,
            reference_close_price: "3101".to_string(),
            payout_per_share: "1".to_string(),
            terminal_value: "10".to_string(),
            realized_pnl: "5.5".to_string(),
            settlement_currency: fixture_settlement_currency().to_string(),
        })
        .expect("current settlement evidence should append");
    let (_, settlement_recovery, booking_recovery) = evidence
        .startup_recovery_projections(
            crate::bolt_v3_current_evidence::PositiveFiniteEvidenceReadCap::new(100_000)
                .expect("test recovery cap must be positive and finite"),
        )
        .expect("current settlement evidence should reconstruct");
    strategy.context = strategy
        .context
        .clone()
        .with_settlement_recovery(Some(Arc::new(settlement_recovery)))
        .with_booking_recovery(Some(Arc::new(booking_recovery)));

    strategy.bootstrap_recovery_from_cache();

    let loss_observations = sink.loss_observations();
    assert_eq!(
        loss_observations.len(),
        1,
        "recovery state: exposure={:?} settled_keys={:?}",
        strategy.exposure,
        strategy.settled_position_keys
    );
    assert_eq!(
        loss_observations[0].event_id.as_deref(),
        Some(settlement_key.as_str()),
        "recovered settlement must replay the loss reducer with the durable settlement key"
    );
    assert!(strategy.settled_position_keys.contains(&settlement_key));
    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat),
        "a recovered successful settlement must reconstruct terminal Flat exposure"
    );
}

fn settlement_loss_config(
    instrument_id: InstrumentId,
    account_id: &str,
) -> crate::bolt_v3_loss_protection::KillSwitchLossProtectionConfig {
    crate::bolt_v3_loss_protection::KillSwitchLossProtectionConfig {
        max_utc_daily_realized_loss: Decimal::new(100, 0),
        action_retry_interval_ms: TEST_LOSS_ACTION_RETRY_INTERVAL_MS,
        action_retry_timeout_ms: TEST_LOSS_ACTION_RETRY_TIMEOUT_MS,
        account_ids: vec![account_id.to_string()],
        instrument_ids: vec![instrument_id.to_string()],
    }
}

#[derive(Debug)]
struct SettlementCaseObservation {
    name: &'static str,
    expected_realized_pnl: f64,
    exposure_is_flat: bool,
    settlement_evidence_matches_expected: bool,
    exposure: ExposureState,
    evidence_events: Vec<CurrentFact>,
}

fn hold_to_resolution_case(
    name: &'static str,
    held_leg: Leg,
    entry_price: f64,
    reference_close_price: f64,
    expected_realized_pnl: f64,
    position_id: PositionId,
) -> SettlementCaseObservation {
    let evidence = recording_decision_evidence();
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
        OrderSide::Buy,
        PositionSide::Long,
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

    let evidence_events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    // For this harness, the Settlement evidence record is the booking record
    // being asserted. The durable cash-surface pin (loss-governor accumulator /
    // provider-allowance balance) is owned by PR-D acceptance on #1179.
    let settlement_evidence_matches_expected =
        settlement_evidence_matches(&evidence_events, expected.realized_pnl);

    SettlementCaseObservation {
        name,
        expected_realized_pnl: expected.realized_pnl,
        exposure_is_flat: matches!(strategy.exposure.state(), ExposureState::Flat),
        settlement_evidence_matches_expected,
        exposure: strategy.exposure.state().clone(),
        evidence_events,
    }
}

fn emit_resolution_update(strategy: &mut BinaryOracleEdgeTaker, reference_close_price: f64) {
    let close_report_ts_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure the interval close boundary");
    try_emit_resolution_update_at(strategy, reference_close_price, close_report_ts_ms)
        .expect("resolution index price should route through the strategy handler");
}

fn try_emit_resolution_update(
    strategy: &mut BinaryOracleEdgeTaker,
    reference_close_price: f64,
) -> Result<()> {
    let close_report_ts_ms = strategy
        .active
        .interval_end_ms
        .expect("fixture should configure the interval close boundary");
    try_emit_resolution_update_at(strategy, reference_close_price, close_report_ts_ms)
}

fn emit_resolution_update_at(
    strategy: &mut BinaryOracleEdgeTaker,
    reference_close_price: f64,
    close_report_ts_ms: u64,
) {
    try_emit_resolution_update_at(strategy, reference_close_price, close_report_ts_ms)
        .expect("resolution index price should route through the strategy handler");
}

fn try_emit_resolution_update_at(
    strategy: &mut BinaryOracleEdgeTaker,
    reference_close_price: f64,
    close_report_ts_ms: u64,
) -> Result<()> {
    let resolution_update = IndexPriceUpdate::new(
        strategy
            .resolution_instrument_id()
            .expect("fixture should configure the resolution instrument"),
        Price::new(reference_close_price, 1),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(close_report_ts_ms * NANOS_PER_MILLI_U64),
    );
    DataActor::on_index_price(strategy, &resolution_update)
}

fn emit_time_event_at(strategy: &mut BinaryOracleEdgeTaker, event_ts_ms: u64) {
    let event = TimeEvent::new(
        ustr::Ustr::from("position-market-lifecycle-test"),
        nautilus_core::UUID4::new(),
        UnixNanos::from(event_ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(event_ts_ms * NANOS_PER_MILLI_U64),
    );
    DataActor::on_time_event(strategy, &event)
        .expect("time event should route through the strategy handler");
}

fn emit_settlement_close_retry_budget_events(
    strategy: &mut BinaryOracleEdgeTaker,
    position_interval_end_ms: u64,
) {
    let retry_interval_ms = strategy
        .config
        .retry_interval_seconds
        .saturating_mul(MILLIS_PER_SECOND_U64);
    for attempt in INITIAL_COUNTER_U64..=strategy.config.market_exit_max_attempts {
        emit_time_event_at(
            strategy,
            position_interval_end_ms
                .saturating_add(1)
                .saturating_add(attempt.saturating_mul(retry_interval_ms)),
        );
    }
}

fn settlement_close_fetch_events(
    strategy: &BinaryOracleEdgeTaker,
) -> Vec<&ResolutionStrikeSubscribeEvent> {
    strategy
        .resolution_strike_subscribe_events
        .iter()
        .filter(|event| event.report_boundary == ResolutionStrikeReportBoundary::WindowClose)
        .collect()
}

fn settlement_close_fetch_event_count(strategy: &BinaryOracleEdgeTaker) -> usize {
    settlement_close_fetch_events(strategy).len()
}

fn roll_active_to_next_interval(
    strategy: &mut BinaryOracleEdgeTaker,
    next_interval_start_ms: u64,
    next_strike_price: f64,
) {
    strategy.apply_selection_snapshot(active_snapshot_with_start(
        "MKT-NEXT",
        next_interval_start_ms,
    ));
    strategy.active.price_to_beat = Some(next_strike_price);
    strategy.active.interval_open = Some(next_strike_price);
    strategy.active.warmup_count = strategy.config.warmup_tick_count;
    strategy.active.last_reference_ts_ms = Some(next_interval_start_ms.saturating_add(1));
}

fn partial_fill_residual_is_managed_or_fresh_reexit(
    strategy: &BinaryOracleEdgeTaker,
    cache: &Rc<RefCell<Cache>>,
    expired_client_order_id: ClientOrderId,
    original_position: &OpenPositionState,
    expected_residual_quantity: Quantity,
) -> bool {
    match strategy.exposure.state() {
        ExposureState::Managed(_) => {
            let Some(managed) = strategy.managed_position() else {
                return false;
            };
            position_matches_expected_residual(
                &managed.position,
                original_position,
                expected_residual_quantity,
            ) && open_sell_exit_order_count(cache, original_position) == 0
        }
        ExposureState::ExitPending(exit) => {
            let Some(managed) = strategy.managed_position() else {
                return false;
            };
            position_matches_expected_residual(
                &managed.position,
                original_position,
                expected_residual_quantity,
            ) && exit.client_order_id() != expired_client_order_id
                && exit.position_id() == original_position.position_id
                && fresh_exit_order_matches_residual(
                    cache,
                    exit.client_order_id(),
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
    client_order_id: ClientOrderId,
    original_position: &OpenPositionState,
    expected_residual_quantity: Quantity,
    expected_order_type: OrderType,
    expected_time_in_force: TimeInForce,
    expected_is_reduce_only: bool,
) -> bool {
    let cache = cache.borrow();
    let cache_position_id_matches =
        cache.position_id(&client_order_id) == Some(&original_position.position_id);
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
            order.client_order_id() == client_order_id
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
    settle_binary_runtime_reference_prices(BinaryRuntimeSettlementInput {
        family_key: crate::bolt_v3_market_families::updown::KEY,
        reference_close_price,
        strike_price: 3_100.0,
        lots: &[lot],
    })
    .result
    .expect("fixture payout should settle the held lot")
}

fn settlement_evidence_count(events: &[CurrentFact]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CurrentFact::Settlement(_)))
        .count()
}

fn settlement_market_ids(events: &[CurrentFact]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            CurrentFact::Settlement(evidence) => Some(evidence.market_id.clone()),
            _ => None,
        })
        .collect()
}

fn settlement_booking_error_count(events: &[CurrentFact]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CurrentFact::TerminalSettlement(_)))
        .count()
}

fn terminal_settlement_lifecycle_count(events: &[CurrentFact]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CurrentFact::OrderLifecycle(evidence)
                    if evidence.transition
                        == OrderLifecycleTransition::SettlementBookingTerminal
            ) || matches!(
                event,
                CurrentFact::TerminalSettlement(evidence)
                    if evidence.lifecycle.transition
                        == OrderLifecycleTransition::SettlementBookingTerminal
            )
        })
        .count()
}

fn settlement_booking_error_reasons(events: &[CurrentFact]) -> Vec<SettlementBookingErrorReason> {
    events
        .iter()
        .filter_map(|event| match event {
            CurrentFact::TerminalSettlement(evidence) => Some(evidence.booking_error.reason),
            _ => None,
        })
        .collect()
}

fn settlement_evidence_matches(events: &[CurrentFact], expected_realized_pnl: f64) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            CurrentFact::Settlement(evidence)
                if evidence.realized_pnl.parse::<f64>().is_ok_and(|realized_pnl|
                    (realized_pnl - expected_realized_pnl).abs() <= f64::EPSILON
                )
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
            strategy.exposure.state(),
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
