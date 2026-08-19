#![cfg(test)]

use super::*;
use nautilus_model::enums::PositionSideSpecified;
use nautilus_trading::Strategy;
use std::sync::Arc;

#[test]
fn position_events_update_live_position_state() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-001");
    let pending = pending_entry_state(&mut strategy, ClientOrderId::from("ENTRY-P-001"));
    let expected_lifecycle = pending.lifecycle.clone();
    let expected_book = pending.book.clone();
    set_pending_entry(&mut strategy, pending);

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert!(strategy.managed_position().is_some());
    let managed_position =
        managed_position_snapshot(&strategy).expect("position should be managed after open event");
    assert_eq!(managed_position.lifecycle, expected_lifecycle);
    assert_eq!(managed_position.instrument_id, instrument_id);
    assert_eq!(managed_position.position_id, position_id);
    assert_eq!(managed_position.entry_order_side, OrderSide::Buy);
    assert_eq!(managed_position.side, PositionSide::Long);
    assert_eq!(managed_position.quantity, Quantity::new(10.0, 2));
    assert_eq!(managed_position.avg_px_open, 0.450);
    let managed_book = managed_position.book.clone();
    assert_eq!(managed_book, expected_book);

    let recovered_position = managed_position_snapshot(&strategy)
        .expect("position should be managed before exit pending");
    set_exit_pending(
        &mut strategy,
        recovered_position,
        ClientOrderId::from("EXIT-001"),
        FixturePositionLineage::CurrentProcess,
    );
    let expired_event = order_expired_event(ClientOrderId::from("EXIT-001"), instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Expired(expired_event.clone()),
    );
    strategy.on_order_expired(expired_event);
    close_nt_position(&mut strategy, position_id);
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Flat,
        Quantity::zero(2),
        1_100,
    );
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(strategy.managed_position().is_none());
    assert!(pending_exit_snapshot(&strategy).is_none());
    assert!(!matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
}

#[test]
fn stale_same_id_position_close_keeps_nt_open_position_managed() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-REOPENED-SAME-ID");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(7.0, 2),
        0.475,
    );

    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    let retained = managed_position_snapshot(&strategy)
        .expect("a stale close callback cannot override the NT open-position cache");
    assert_eq!(retained.position_id, position_id);
    assert_eq!(retained.quantity, Quantity::new(7.0, 2));
}

#[test]
fn same_id_wrong_instrument_close_cannot_release_managed_exposure() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let other_instrument_id = configured_instrument_except(&strategy, instrument_id);
    let position_id = PositionId::from("P-CLOSE-WRONG-INSTRUMENT");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(7.0, 2),
        0.475,
    );
    close_nt_position(&mut strategy, position_id);

    strategy.on_position_closed(position_closed_event(other_instrument_id, position_id));

    let retained = strategy
        .exposure
        .managed_position_context()
        .expect("a close for another instrument cannot release managed exposure");
    assert_eq!(retained.instrument_id, instrument_id);
    assert_eq!(retained.position_id, position_id);
}

#[test]
fn stale_instrument_close_rematerializes_entry_reconcile_from_nt() {
    let mut strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-RECONCILE-STALE-CLOSE"),
    );
    let instrument_id = pending.instrument_id;
    let open_position_id = PositionId::from("P-RECONCILE-OPEN");
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        open_position_id,
        Quantity::new(6.0, 2),
        0.465,
    );

    strategy.on_position_closed(position_closed_event(
        instrument_id,
        PositionId::from("P-RECONCILE-STALE-CLOSED"),
    ));

    let retained = managed_position_snapshot(&strategy)
        .expect("instrument-scoped NT truth must dominate a stale close callback");
    assert_eq!(retained.position_id, open_position_id);
    assert_eq!(retained.quantity, Quantity::new(6.0, 2));
}

#[test]
fn exit_fill_keeps_pending_exit_until_position_closed() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-EXIT-001");
    let exit_client_order_id = ClientOrderId::from("EXIT-001");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );

    let mut fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-EXIT-FULL");
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill.clone()),
    );
    strategy.on_order_filled(&fill);

    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id)
    );
    assert!(strategy.managed_position().is_some());
    assert!(strategy.exposure.terminal_exit_snapshot().is_some());

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));
    assert!(strategy.exposure.terminal_exit_snapshot().is_some());
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Flat,
        Quantity::zero(2),
        1_100,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("exit-authority-reconcile"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("timer should release the causally proven flat position");

    assert!(strategy.managed_position().is_none());
    assert!(pending_exit_snapshot(&strategy).is_none());
}

#[test]
fn position_change_preserves_pending_exit_correlation() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-EXIT-CHANGE");
    let exit_client_order_id = ClientOrderId::from("EXIT-CHANGE");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(7.0, 2),
        0.470,
    );
    strategy.materialize_position_from_event(
        PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(7.0, 2),
            avg_px_open: 0.470,
        },
        0,
    );

    let exit_pending = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("position change should keep exit pending");
    assert_eq!(
        exit_pending.pending_exit.client_order_id,
        exit_client_order_id
    );
    assert_eq!(exit_pending.pending_exit.position_id, Some(position_id));

    assert!(exit_pending.position.is_some());
    let position =
        managed_position_snapshot(&strategy).expect("NT cache should project the changed position");
    assert_eq!(position.quantity, Quantity::new(7.0, 2));
    assert_eq!(position.avg_px_open, 0.470);
}

#[test]
fn unrelated_position_close_does_not_clear_pending_exit_before_fill() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        tracked_instrument,
        PositionId::from("P-TRACKED"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-001"),
        FixturePositionLineage::CurrentProcess,
    );

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id),
        Some(ClientOrderId::from("EXIT-001"))
    );
    assert!(strategy.managed_position().is_some());
}

#[test]
fn unrelated_position_close_does_not_clear_pending_exit_after_fill_event() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = selected_entry_instrument(&strategy);
    let open_position = materialize_configured_position(
        &mut strategy,
        tracked_instrument,
        PositionId::from("P-TRACKED"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-001"),
        FixturePositionLineage::CurrentProcess,
    );
    strategy.on_order_filled(&order_filled_event(
        ClientOrderId::from("EXIT-001"),
        tracked_instrument,
        PositionId::from("P-TRACKED"),
    ));

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id),
        Some(ClientOrderId::from("EXIT-001"))
    );
    assert!(strategy.managed_position().is_some());
}

#[test]
fn exit_pending_state_clears_on_cancel_reject_and_expire() {
    let exit_client_order_id = ClientOrderId::from("EXIT-001");

    let mut canceled = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&canceled);
    let canceled_position = materialize_configured_position(
        &mut canceled,
        instrument_id,
        PositionId::from("P-CANCEL"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut canceled,
        canceled_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let canceled_event = order_canceled_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut canceled,
        nautilus_model::events::OrderEventAny::Canceled(canceled_event.clone()),
    );
    canceled.on_order_canceled(&canceled_event);
    assert!(pending_exit_snapshot(&canceled).is_none());
    assert!(canceled.managed_position().is_some());

    let mut rejected = ready_to_trade_strategy();
    let rejected_position = materialize_configured_position(
        &mut rejected,
        instrument_id,
        PositionId::from("P-REJECT"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut rejected,
        rejected_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let rejected_event = order_rejected_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut rejected,
        nautilus_model::events::OrderEventAny::Rejected(rejected_event.clone()),
    );
    rejected.on_order_rejected(rejected_event);
    assert!(pending_exit_snapshot(&rejected).is_none());
    assert!(rejected.managed_position().is_some());

    let mut expired = ready_to_trade_strategy();
    let expired_position = materialize_configured_position(
        &mut expired,
        instrument_id,
        PositionId::from("P-EXPIRE"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut expired,
        expired_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let expired_event = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut expired,
        nautilus_model::events::OrderEventAny::Expired(expired_event.clone()),
    );
    expired.on_order_expired(expired_event);
    assert!(pending_exit_snapshot(&expired).is_none());
    assert!(expired.managed_position().is_some());
}

#[test]
fn partial_exit_fill_then_expire_restores_managed_residual_position() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-PARTIAL-EXIT-EXPIRE");
    let exit_client_order_id = ClientOrderId::from("EXIT-PARTIAL-EXPIRE");
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
        FixturePositionLineage::CurrentProcess,
    );

    let mut fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-EXIT-PARTIAL");
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill.clone()),
    );
    strategy.on_order_filled(&fill);
    assert!(matches!(
        strategy.exposure.exit_lifecycle(),
        Some((ExitLifecyclePhase::Working, _))
    ));
    assert_eq!(
        strategy
            .context
            .position_authority()
            .expect("projected fill requires position authority")
            .canonical_position(position_id, instrument_id)
            .expect("canonical projected-fill position read should succeed")
            .expect("projected-fill position should remain cached")
            .signed_quantity(),
        Decimal::new(10, 0),
        "an order-only projected fill must not pretend that NT position state advanced"
    );

    let expired_event = order_expired_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Expired(expired_event.clone()),
    );
    strategy.on_order_expired(expired_event);

    assert!(strategy.exposure.terminal_exit_snapshot().is_some());
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        0.45,
    );
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Long,
        Quantity::new(6.0, 2),
        1_100,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("exit-authority-reconcile"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("timer should reconcile callback-free cache convergence");

    assert!(pending_exit_snapshot(&strategy).is_none());
    assert_eq!(
        strategy.exposure_occupancy(),
        Some(ExposureOccupancy::ManagedPosition)
    );
    assert_eq!(
        tracked_position_snapshot(&strategy).map(|position| position.quantity),
        Some(Quantity::new(6.0, 2))
    );
}

#[test]
fn projected_partial_exit_fill_then_cancel_waits_for_timer_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-PROJECTED-PARTIAL-CANCEL");
    let exit_client_order_id = ClientOrderId::from("EXIT-PROJECTED-PARTIAL-CANCEL");
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
        FixturePositionLineage::CurrentProcess,
    );

    let mut fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-PROJECTED-CANCEL");
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill.clone()),
    );
    strategy.on_order_filled(&fill);
    assert!(matches!(
        strategy.exposure.exit_lifecycle(),
        Some((ExitLifecyclePhase::Working, _))
    ));

    let canceled_event = order_canceled_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled_event.clone()),
    );
    strategy.on_order_canceled(&canceled_event);
    assert!(strategy.exposure.terminal_exit_snapshot().is_some());

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        0.45,
    );
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Long,
        Quantity::new(6.0, 2),
        1_100,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("exit-authority-reconcile"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("timer should reconcile callback-free cache convergence");

    assert_eq!(
        tracked_position_snapshot(&strategy).map(|position| position.quantity),
        Some(Quantity::new(6.0, 2))
    );
    assert!(pending_exit_snapshot(&strategy).is_none());
}

#[test]
fn timer_reconciles_a_missed_fill_void_that_reopens_the_exit_order() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-MISSED-FILL-VOID");
    let client_order_id = ClientOrderId::from("EXIT-MISSED-FILL-VOID");
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
        client_order_id,
        FixturePositionLineage::CurrentProcess,
    );

    let trade_id = nautilus_model::identifiers::TradeId::from("TRADE-MISSED-FILL-VOID");
    let mut fill = order_filled_event_with_details(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.trade_id = trade_id;
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill.clone()),
    );
    strategy.on_order_filled(&fill);
    assert!(strategy.exposure.terminal_exit_snapshot().is_some());

    let fill_voided = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        trade_id,
        Quantity::new(10.0, 2),
        1_100,
    );
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::FillVoided(fill_voided),
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("missed-fill-void-reconcile"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("timer should reconcile the reopened cached order");

    assert!(
        matches!(
            strategy.exposure.exit_lifecycle(),
            Some((ExitLifecyclePhase::Working, _))
        ),
        "a missed fill-void callback must not leave a reopened order terminal-fenced: {:?}",
        strategy.exposure
    );
}

#[test]
fn timer_fences_a_cached_voided_exit_until_post_correction_position_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-CACHED-VOIDED");
    let client_order_id = ClientOrderId::from("EXIT-CACHED-VOIDED");
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
        client_order_id,
        FixturePositionLineage::CurrentProcess,
    );

    let trade_id = nautilus_model::identifiers::TradeId::from("TRADE-CACHED-VOIDED");
    let mut fill = order_filled_event_with_details(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(10.0, 2);
    fill.trade_id = trade_id;
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill),
    );
    let mut fill_voided = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        trade_id,
        Quantity::new(10.0, 2),
        1_100,
    );
    fill_voided.is_reopened = false;
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::FillVoided(fill_voided),
    );
    assert_eq!(
        strategy
            .cache()
            .order(&client_order_id)
            .expect("voided exit should remain cached")
            .status(),
        OrderStatus::Voided
    );

    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("cached-voided-reconcile"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("timer should classify cached Voided as a correction");
    assert!(
        strategy.exposure.terminal_exit_snapshot().is_some(),
        "a corrected zero-fill order cannot use the zero-fill shortcut"
    );

    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Long,
        Quantity::new(10.0, 2),
        1_200,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("cached-voided-authority"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_200_u64),
            UnixNanos::from(1_200_u64),
        ),
    )
    .expect("post-correction authority should release the exact residual");
    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.quantity),
        Some(Quantity::new(10.0, 2))
    );
}

#[test]
fn exit_fill_quarantines_foreign_venue_client_order_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-EXIT-FILL");
    let exit_client_order_id = ClientOrderId::from("EXIT-FOREIGN-FILL");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_filled(&order_filled_event(
        exit_client_order_id,
        foreign_instrument_id,
        position_id,
    ));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn managed_entry_fill_quarantines_foreign_venue_client_order_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL");
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-MANAGED-FILL");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    let mut pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
    pending_entry.instrument_id = instrument_id;
    set_managed_position_with_pending_entry(
        &mut strategy,
        open_position,
        FixturePositionLineage::CurrentProcess,
        pending_entry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_filled(&order_filled_event_with_details(
        entry_client_order_id,
        foreign_instrument_id,
        Some(PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL")),
        OrderSide::Buy,
    ));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn pending_entry_terminal_quarantines_foreign_venue_client_order_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-CANCEL");
    let pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending_entry.instrument_id;
    set_pending_entry(&mut strategy, pending_entry);
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_canceled(&order_canceled_event(
        entry_client_order_id,
        foreign_instrument_id,
    ));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn managed_pending_entry_terminal_quarantines_foreign_venue_client_order_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-MANAGED-ENTRY-CANCEL");
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-MANAGED-CANCEL");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    let mut pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
    pending_entry.instrument_id = instrument_id;
    set_managed_position_with_pending_entry(
        &mut strategy,
        open_position,
        FixturePositionLineage::CurrentProcess,
        pending_entry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_canceled(&order_canceled_event(
        entry_client_order_id,
        foreign_instrument_id,
    ));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn exit_terminal_quarantines_foreign_venue_client_order_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-EXIT-CANCEL");
    let exit_client_order_id = ClientOrderId::from("EXIT-FOREIGN-CANCEL");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_canceled(&order_canceled_event(
        exit_client_order_id,
        foreign_instrument_id,
    ));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn position_event_without_context_does_not_guess_side_from_suffix() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = InstrumentId::from("external-MKT-1-UP.POLYMARKET");
    let position_id = PositionId::from("P-SUFFIX-001");
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert!(managed_position_snapshot(&strategy).is_none());
    assert!(strategy.exposure.is_blind_recovery());
}

#[test]
fn production_outcome_side_inference_does_not_parse_instrument_suffixes() {
    let production = crate::bolt_v3_source_integrity::production_module_source_text(
        crate::bolt_v3_source_integrity::STRATEGY_KEY,
    );
    let up_suffix = format!("{}{}{}", "-", "UP", ".");
    let down_suffix = format!("{}{}{}", "-", "DOWN", ".");

    assert!(
        !production.contains(&up_suffix),
        "production strategy code must not infer outcome side from instrument-id text suffix"
    );
    assert!(
        !production.contains(&down_suffix),
        "production strategy code must not infer outcome side from instrument-id text suffix"
    );
}

#[test]
fn untracked_position_close_keeps_recovery_fail_closed() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);

    strategy.on_position_closed(position_closed_event(
        instrument_id,
        PositionId::from("P-X"),
    ));

    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
}

#[test]
fn fill_after_rotation_preserves_exitable_position_book_and_subscription() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let entry_client_order_id = ClientOrderId::from("ENTRY-A");
    let position_id = PositionId::from("P-A");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_a = pending.instrument_id;
    let original_book = pending.book.clone();
    set_pending_entry(&mut strategy, pending);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    seed_nt_open_position(
        &mut strategy,
        instrument_a,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_a,
        position_id,
    ));

    assert_eq!(
        managed_position_snapshot(&strategy).and_then(|p| p.book.best_bid),
        original_book.best_bid
    );
    assert_eq!(
        managed_position_snapshot(&strategy).and_then(|p| p.lifecycle.settlement_strike()),
        Some(3_100.0)
    );
    assert_eq!(
        managed_position_snapshot(&strategy).and_then(|p| p.lifecycle.selection_published_at_ms()),
        Some(1_000)
    );
    assert_eq!(
        managed_position_snapshot(&strategy)
            .and_then(|p| p.lifecycle.seconds_to_expiry_at_selection()),
        Some(300)
    );
    assert_eq!(
        strategy.book_subscriptions.tracked_position_instrument_id,
        Some(instrument_a)
    );
    let decision = strategy.exit_intent_decision_at(2_000);
    assert_eq!(decision.instrument_id, Some(instrument_a));
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert!(
        evidence.recorded_facts().expect("recorded current evidence must decode").into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::EntryFillMaterialized
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::Managed
                    && record.client_order_id.as_deref() == Some("ENTRY-A")
                    && record.position_id.as_deref() == Some("P-A")
        )),
        "late entry fill after selection rotation must write managed materialization lifecycle evidence"
    );
}

#[test]
fn maker_entry_partial_fills_keep_entry_fill_accounting_without_overwriting_position_event_quantity()
 {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.is_post_only = true;
    let entry_client_order_id = ClientOrderId::from("ENTRY-PARTIAL");
    let position_id = PositionId::from("P-PARTIAL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(4.0, 2),
        0.450,
    );
    let mut first_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    first_fill.last_qty = Quantity::new(4.0, 2);
    strategy.on_order_filled(&first_fill);
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(4.0, 2),
        0.450,
    ));

    let mut second_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    second_fill.last_qty = Quantity::new(6.0, 2);
    strategy.on_order_filled(&second_fill);

    assert_eq!(strategy.market_churn_count("MKT-1"), 2);
    assert_eq!(
        tracked_position_snapshot(&strategy).map(|position| position.quantity),
        Some(Quantity::new(4.0, 2)),
        "OrderFilled carries last fill quantity; NT position events remain authoritative for aggregate position quantity"
    );
}

#[test]
fn managed_partial_entry_blocks_normal_exit_until_entry_order_resolves() {
    let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy();
        configure_limit_base_entry_order(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        strategy.active.phase = SelectionPhase::Active;
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("POSITION-NORMAL-WORKING-{instrument_id}").as_str()),
            position_quantity,
        );

        let decision = strategy.exit_intent_decision_at(1_200);

        assert_eq!(
            decision.blocked_reason,
            Some(EvidenceExitBlockedReason::EntryOrderStillWorking),
            "{instrument_id}"
        );
        assert_eq!(
            decision.evaluation.blocked_reason,
            Some(EvidenceExitBlockedReason::EntryOrderStillWorking),
            "{instrument_id}"
        );
        assert_eq!(decision.instrument_id, None, "{instrument_id}");
        assert_eq!(decision.order_side, None, "{instrument_id}");
        assert_eq!(decision.quantity, None, "{instrument_id}");
        assert!(decision.forced_flat_reasons.is_empty(), "{instrument_id}");
    }
}

#[test]
fn forced_flat_exit_waits_for_resting_entry_terminal_proof() {
    let configured_instruments =
        configured_outcome_instruments(&ready_to_trade_strategy_with_bound_economics());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy_with_bound_economics();
        configure_limit_base_entry_order(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        strategy.active.phase = SelectionPhase::Freeze;
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("POSITION-FORCED-WORKING-{instrument_id}").as_str()),
            position_quantity,
        );
        let decision = strategy.exit_intent_decision_at(1_200);

        assert_eq!(
            decision.blocked_reason,
            Some(EvidenceExitBlockedReason::EntryOrderStillWorking),
            "{instrument_id}"
        );
        assert_eq!(
            decision.evaluation.blocked_reason,
            Some(EvidenceExitBlockedReason::EntryOrderStillWorking),
            "{instrument_id}"
        );
        assert_eq!(
            decision.forced_flat_reasons,
            vec![ForcedFlatReason::Freeze],
            "{instrument_id}"
        );
        assert_eq!(decision.order_type, None, "{instrument_id}");
        assert_eq!(decision.time_in_force, None, "{instrument_id}");
        assert_eq!(decision.order_side, None, "{instrument_id}");
        assert_eq!(decision.quantity, None, "{instrument_id}");
        assert_eq!(decision.price, None, "{instrument_id}");
        assert_eq!(decision.is_post_only, None, "{instrument_id}");
        assert_eq!(decision.is_reduce_only, None, "{instrument_id}");
    }
}

#[test]
fn forced_flat_submit_cancels_resting_entry_before_routing_exit() {
    let configured_instruments =
        configured_outcome_instruments(&ready_to_trade_strategy_with_bound_economics());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy_with_bound_economics();
        let yes_instrument_id = strategy
            .active
            .books
            .up
            .instrument_id
            .expect("fixture should bind the yes instrument");
        let no_instrument_id = strategy
            .active
            .books
            .down
            .instrument_id
            .expect("fixture should bind the no instrument");
        let canonical_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let (yes_position, no_position) = match instrument_id == yes_instrument_id {
            true => (canonical_quantity.as_decimal(), Decimal::ZERO),
            false => (Decimal::ZERO, canonical_quantity.as_decimal()),
        };
        let submit_admission = submit_admission_with_provider_cap_and_canonical_position(
            Decimal::new(10_000, 0),
            recording_decision_evidence(),
            yes_instrument_id,
            no_instrument_id,
            yes_position,
            no_position,
        );
        strategy.context = StrategyBuildContext::new(
            fixture_order_economics(),
            recording_decision_evidence(),
            submit_admission,
            crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
            fixture_execution_venue(),
        )
        .with_position_authority(fixture_position_authority_capability(&strategy));
        configure_limit_base_entry_order(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        strategy.active.phase = SelectionPhase::Freeze;
        let cache = register_test_strategy(&mut strategy);
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
        let position_id =
            PositionId::from(format!("POSITION-FORCED-RACE-{instrument_id}").as_str());
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let (instrument_id, entry_client_order_id) =
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                position_id,
                position_quantity,
            );
        let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
            .best_ask
            .expect("ready-to-trade fixture should expose an ask");
        let entry_order = strategy
            .build_configured_entry_order(
                instrument_id,
                strategy
                    .configured_entry_order_side()
                    .expect("test config should carry entry order side"),
                position_quantity,
                Price::new(entry_price, 2),
                entry_client_order_id,
            )
            .expect("resting entry order should build through NT factory");
        seed_nt_working_order(&mut strategy, entry_order, position_id);
        let mut partial_fill =
            order_filled_event(entry_client_order_id, instrument_id, position_id);
        partial_fill.last_qty = Quantity::new(1.0, 2);
        apply_exit_order_event_to_nt_cache(
            &mut strategy,
            nautilus_model::events::OrderEventAny::Filled(partial_fill),
        );

        let first_attempt = strategy
            .try_submit_exit_order_for_trigger(
                1_200,
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    1_200,
                )),
            )
            .expect("forced-flat cancellation should not fail");
        assert_eq!(first_attempt, None, "{instrument_id}");

        let exec_messages = exec_messages.get_messages();
        assert!(
            exec_messages.iter().any(|message| matches!(
                message,
                TradingCommand::CancelOrder(command)
                    if command.client_order_id == entry_client_order_id
            )),
            "forced-flat submit should cancel the resting entry before relying on exit: {instrument_id}"
        );
        let risk_messages_before_terminal = risk_messages.get_messages();
        assert!(
            !risk_messages_before_terminal
                .iter()
                .any(|message| matches!(message, TradingCommand::SubmitOrder(_))),
            "forced-flat must not submit an exit before exact entry terminal proof: {instrument_id}"
        );

        let canceled = order_canceled_event(entry_client_order_id, instrument_id);
        apply_exit_order_event_to_nt_cache(
            &mut strategy,
            nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
        );
        strategy.on_order_canceled(&canceled);

        let exit_client_order_id = strategy
            .try_submit_exit_order_for_trigger(
                1_200,
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    1_200,
                )),
            )
            .expect("forced-flat exit should route after terminal entry proof")
            .expect("forced-flat exit should be present after terminal entry proof");
        let risk_messages = risk_messages.get_messages();
        assert!(
            risk_messages.iter().any(|message| matches!(
                message,
                TradingCommand::SubmitOrder(command)
                    if command.client_order_id == exit_client_order_id
            )),
            "forced-flat exit should route only after the entry identity retires: {instrument_id}"
        );
    }
}

#[test]
fn non_resting_entry_fill_does_not_keep_pending_entry_from_cache_state() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
    let entry_client_order_id = ClientOrderId::from("ENTRY-IOC");
    let position_id = PositionId::from("P-IOC");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("IOC entry order should build");
    cache
        .borrow_mut()
        .add_order(order, None, Some(ClientId::from("POLYMARKET")), true)
        .expect("test cache should accept entry order");

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_id,
        position_id,
    ));

    assert!(strategy.exposure.entry_remainder().is_none());
    assert_eq!(
        strategy.exit_intent_decision_at(1_200).blocked_reason,
        Some(EvidenceExitBlockedReason::ExitHold)
    );
}

#[test]
fn entry_fill_without_position_id_stays_fail_closed_until_position_event_arrives() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-NO-POS");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let original_book = pending.book.clone();
    set_pending_entry(&mut strategy, pending);

    strategy.on_order_filled(&order_filled_event_with_details(
        entry_client_order_id,
        instrument_id,
        None,
        OrderSide::Buy,
    ));

    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(strategy.market_in_cooldown("MKT-1", 1_000));

    let late_position_id = PositionId::from("P-LATE");
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        late_position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        late_position_id,
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert!(strategy.managed_position().is_some());
    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.position_id),
        Some(PositionId::from("P-LATE"))
    );
    assert_eq!(
        managed_position_snapshot(&strategy)
            .and_then(|position| position.lifecycle.market_id_owned()),
        Some("MKT-1".to_string())
    );
    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.book.clone()),
        Some(original_book)
    );
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn late_zero_fill_entry_terminal_events_resolve_entry_reconcile_to_flat() {
    let evidence = recording_decision_evidence();
    let mut canceled = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut canceled);
    configure_limit_base_entry_order(&mut canceled);
    canceled.config.entry_order.time_in_force = TimeInForce::Gtc;
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-CANCEL");
    let canceled_pending = pending_entry_state(&mut canceled, entry_client_order_id);
    let canceled_instrument_id = canceled_pending.instrument_id;
    let canceled_order = canceled
        .build_configured_entry_order(
            canceled_instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(
        &mut canceled,
        canceled_order,
        PositionId::from("P-ZERO-FILL-CANCEL"),
    );
    set_entry_reconcile_pending(
        &mut canceled,
        canceled_pending,
        EntryReconcileReason::UnresolvedAtSelectionBoundary,
    );
    let canceled_event = order_canceled_event(entry_client_order_id, canceled_instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut canceled,
        nautilus_model::events::OrderEventAny::Canceled(canceled_event.clone()),
    );
    canceled.on_order_canceled(&canceled_event);
    assert!(canceled.exposure.is_flat());
    assert!(
        evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .into_iter()
            .any(|event| matches!(
                event,
                CurrentFact::OrderLifecycle(record)
                    if record.transition
                        == crate::bolt_v3_current_evidence::OrderLifecycleTransition::OrderCanceled
                        && record.client_order_id.as_deref() == Some("ENTRY-ZERO-FILL-CANCEL")
                        && record.outcome
                            == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::Flat
            )),
        "zero-fill cancel must record a Flat terminal lifecycle outcome"
    );

    let mut rejected = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-REJECT");
    let rejected_pending = pending_entry_state(&mut rejected, entry_client_order_id);
    let rejected_instrument_id = rejected_pending.instrument_id;
    set_entry_reconcile_pending_after_fill(
        &mut rejected,
        rejected_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    rejected.on_order_rejected(order_rejected_event(
        entry_client_order_id,
        rejected_instrument_id,
    ));
    assert!(rejected.exposure.is_flat());

    let mut denied = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-DENIED");
    let denied_pending = pending_entry_state(&mut denied, entry_client_order_id);
    let denied_instrument_id = denied_pending.instrument_id;
    set_entry_reconcile_pending_after_fill(
        &mut denied,
        denied_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    denied.on_order_denied(order_denied_event_with_reason(
        entry_client_order_id,
        denied_instrument_id,
        "DENIED",
    ));
    assert!(denied.exposure.is_flat());

    let mut expired = ready_to_trade_strategy();
    register_test_strategy_with_active_instruments(&mut expired);
    configure_limit_base_entry_order(&mut expired);
    expired.config.entry_order.time_in_force = TimeInForce::Gtc;
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-EXPIRE");
    let expired_pending = pending_entry_state(&mut expired, entry_client_order_id);
    let expired_instrument_id = expired_pending.instrument_id;
    let expired_order = expired
        .build_configured_entry_order(
            expired_instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(
        &mut expired,
        expired_order,
        PositionId::from("P-ZERO-FILL-EXPIRE"),
    );
    set_entry_reconcile_pending(
        &mut expired,
        expired_pending,
        EntryReconcileReason::UnresolvedAtSelectionBoundary,
    );
    let expired_event = order_expired_event(entry_client_order_id, expired_instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut expired,
        nautilus_model::events::OrderEventAny::Expired(expired_event.clone()),
    );
    expired.on_order_expired(expired_event);
    assert!(expired.exposure.is_flat());
}

#[test]
fn late_fill_observed_entry_cancel_or_expire_preserves_entry_reconcile_fail_closed_state() {
    let evidence = recording_decision_evidence();

    let mut canceled = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut canceled);
    let entry_client_order_id = ClientOrderId::from("ENTRY-FILL-SEEN-CANCEL");
    let canceled_pending = pending_entry_state(&mut canceled, entry_client_order_id);
    let canceled_instrument_id = canceled_pending.instrument_id;
    set_entry_reconcile_pending_after_fill(
        &mut canceled,
        canceled_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    canceled.on_order_canceled(&order_canceled_event(
        entry_client_order_id,
        canceled_instrument_id,
    ));
    assert!(matches!(
        canceled.exposure.entry_reconcile_for_test(),
        Some((_, EntryReconcileReason::AwaitingPositionMaterialization))
    ));

    let mut expired = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut expired);
    let entry_client_order_id = ClientOrderId::from("ENTRY-FILL-SEEN-EXPIRE");
    let expired_pending = pending_entry_state(&mut expired, entry_client_order_id);
    let expired_instrument_id = expired_pending.instrument_id;
    set_entry_reconcile_pending_after_fill(
        &mut expired,
        expired_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    expired.on_order_expired(order_expired_event(
        entry_client_order_id,
        expired_instrument_id,
    ));
    assert!(matches!(
        expired.exposure.entry_reconcile_for_test(),
        Some((_, EntryReconcileReason::AwaitingPositionMaterialization))
    ));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        events.iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::OrderCanceled
                    && record.raw_reason_text.as_deref()
                        == Some(ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON)
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "fill-observed cancel must record preserved fail-closed lifecycle evidence"
    );
    assert!(
        events.into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::OrderExpired
                    && record.raw_reason_text.as_deref()
                        == Some(ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON)
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "fill-observed expiry must record preserved fail-closed lifecycle evidence"
    );
}

#[test]
fn pending_entry_cancel_after_missed_partial_fill_does_not_create_false_vacancy() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    register_test_strategy_with_active_instruments(&mut strategy);

    let entry_client_order_id = ClientOrderId::from("ENTRY-MISSED-PARTIAL-FILL-CANCEL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(
        &mut strategy,
        entry_order,
        PositionId::from("P-MISSED-PARTIAL-FILL-CANCEL"),
    );

    let mut fill = order_filled_event(
        entry_client_order_id,
        instrument_id,
        PositionId::from("P-MISSED-PARTIAL-FILL-CANCEL"),
    );
    fill.last_qty = Quantity::new(4.0, 2);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill),
    );
    let canceled = order_canceled_event(entry_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );

    // Model a missed fill callback: the NT cache knows the order partially filled,
    // but the edge-taker sees only the later terminal callback.
    strategy.on_order_canceled(&canceled);

    assert!(matches!(
        strategy.exposure.entry_reconcile_for_test(),
        Some((pending, EntryReconcileReason::AwaitingPositionMaterialization))
            if pending.client_order_id == entry_client_order_id
    ));
    assert!(!strategy.exposure.is_flat());
}

#[test]
fn pending_entry_terminal_consumes_only_coherent_canonical_position_truth() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    register_test_strategy_with_active_instruments(&mut strategy);

    let entry_client_order_id = ClientOrderId::from("ENTRY-MISSED-FILL-CANONICAL-POSITION");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let position_id = PositionId::from("P-MISSED-FILL-CANONICAL-POSITION");
    set_pending_entry(&mut strategy, pending);

    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(&mut strategy, entry_order, position_id);
    let mut fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    fill.last_qty = Quantity::new(4.0, 2);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill),
    );
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(4.0, 2),
        0.45,
    );
    let canceled = order_canceled_event(entry_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );

    strategy.on_order_canceled(&canceled);

    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.position_id),
        Some(position_id)
    );
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn pending_entry_terminal_rejects_unrelated_same_instrument_position_truth() {
    let mut strategy = ready_to_trade_strategy();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    register_test_strategy_with_active_instruments(&mut strategy);

    let entry_client_order_id = ClientOrderId::from("ENTRY-UNRELATED-CANONICAL-POSITION");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let entry_position_id = PositionId::from("P-ENTRY-ORDER");
    let unrelated_position_id = PositionId::from("P-UNRELATED-SAME-INSTRUMENT");
    set_pending_entry(&mut strategy, pending);

    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(&mut strategy, entry_order, entry_position_id);
    let mut fill = order_filled_event(entry_client_order_id, instrument_id, entry_position_id);
    fill.last_qty = Quantity::new(4.0, 2);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(fill),
    );
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        unrelated_position_id,
        Quantity::new(4.0, 2),
        0.45,
    );
    let canceled = order_canceled_event(entry_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );

    strategy.on_order_canceled(&canceled);

    assert!(matches!(
        strategy.exposure.entry_reconcile_for_test(),
        Some((pending, EntryReconcileReason::AwaitingPositionMaterialization))
            if pending.client_order_id == entry_client_order_id
    ));
    assert!(managed_position_snapshot(&strategy).is_none());
    assert!(!strategy.exposure.is_flat());
}

#[test]
fn malformed_entry_reject_stops_same_instrument_entry_decisions() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-MALFORMED-AMOUNTS");
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure.set_pending_entry_for_test(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "invalid order amounts: maker amount exceeds allowed decimal precision",
    ));

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceEntrySkipReason::EntryMalformedRejected)
    );
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn order_denied_clears_matching_pending_entry_and_records_lifecycle_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    let entry_client_order_id = ClientOrderId::from("ENTRY-DENIED");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure.set_pending_entry_for_test(pending);

    strategy.on_order_denied(order_denied_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "RATE_LIMIT_EXCEEDED",
    ));

    assert!(strategy.pending_entry().is_none());
    assert_eq!(
        strategy.entry_submission_decision_at(1_200).blocked_reason,
        Some(EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook),
        "a local denial must not fall through to immediate resubmit"
    );
    assert!(
        evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .into_iter()
            .any(|event| matches!(
                event,
                CurrentFact::OrderLifecycle(record)
                    if record.transition
                        == crate::bolt_v3_current_evidence::OrderLifecycleTransition::OrderDenied
                        && record.client_order_id.as_deref() == Some("ENTRY-DENIED")
                        && record.outcome
                            == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::Flat
            )),
        "denial handling must write distinguishable lifecycle evidence"
    );
}

#[test]
fn selection_rotation_reclassifies_unresolved_pending_entry_and_records_lifecycle_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let entry_client_order_id = ClientOrderId::from("ENTRY-BOUNDARY-NO-TERMINAL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure.set_pending_entry_for_test(pending);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-NEXT", 2_000));

    assert!(matches!(
        strategy.exposure.entry_reconcile_for_test(),
        Some((pending, EntryReconcileReason::UnresolvedAtSelectionBoundary))
            if pending.instrument_id == instrument_id
    ));
    let instrument_id_text = instrument_id.to_string();
    assert!(
        evidence.recorded_facts().expect("recorded current evidence must decode").into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::BoundaryReclassification
                    && record.client_order_id.as_deref() == Some("ENTRY-BOUNDARY-NO-TERMINAL")
                    && record.instrument_id.as_deref() == Some(instrument_id_text.as_str())
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "selection-boundary recovery must write distinguishable lifecycle evidence"
    );
}

#[test]
fn unfillable_fok_entry_reject_waits_for_book_change_before_redeciding() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOK-NO-MATCH");
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let rejected_book = pending.book.clone();
    strategy.exposure.set_pending_entry_for_test(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "FOK order could not be matched against the current book",
    ));

    let unchanged_book_decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        unchanged_book_decision.blocked_reason,
        Some(EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook)
    );

    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    assert_ne!(
        configured_book_for_instrument(&mut strategy, instrument_id),
        rejected_book,
        "fixture must actually change the selected book for this replay"
    );
    let changed_book_decision = strategy.entry_submission_decision_at(1_201);
    assert_eq!(changed_book_decision.blocked_reason, None);
}

#[test]
fn incident_entry_reject_strings_pin_classifier_classes() {
    assert_eq!(
        classify_entry_reject_reason(super::adverse_path_harness::PRECISION_REJECT_REASON),
        Some(EntryRejectClass::Malformed)
    );
    assert_eq!(
        classify_entry_reject_reason(super::adverse_path_harness::BALANCE_REJECT_REASON),
        Some(EntryRejectClass::Balance)
    );
    assert_eq!(
        classify_entry_reject_reason(super::adverse_path_harness::MIN_SIZE_REJECT_REASON),
        Some(EntryRejectClass::Malformed)
    );
}

#[test]
fn balance_entry_reject_stops_same_instrument_entry_decisions() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-BALANCE-REJECTED");
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure.set_pending_entry_for_test(pending);
    let balance_reject_reason =
        "not enough balance / allowance: the balance is not enough -> balance: 0";

    assert!(
        classify_entry_reject_reason(balance_reject_reason).is_some(),
        "balance/allowance rejects must be an explicit entry reject class"
    );
    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        balance_reject_reason,
    ));

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceEntrySkipReason::EntryBalanceRejected)
    );
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    let changed_book_decision = strategy.entry_submission_decision_at(1_201);
    assert_eq!(
        changed_book_decision.blocked_reason,
        Some(EvidenceEntrySkipReason::EntryBalanceRejected)
    );
}

#[test]
fn unknown_entry_reject_waits_for_book_change_before_redeciding() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-UNKNOWN-REJECTED");
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let rejected_book = pending.book.clone();
    strategy.exposure.set_pending_entry_for_test(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "venue rejected entry for an unmodeled reason",
    ));

    let unchanged_book_decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        unchanged_book_decision.blocked_reason,
        Some(EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook)
    );

    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    assert_ne!(
        configured_book_for_instrument(&mut strategy, instrument_id),
        rejected_book,
        "fixture must actually change the selected book for this replay"
    );
    let changed_book_decision = strategy.entry_submission_decision_at(1_201);
    assert_eq!(changed_book_decision.blocked_reason, None);
}

#[test]
fn book_delta_entry_reconcile_pending_does_not_try_new_entry() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-RECONCILE-BOOK-DELTA"),
    );
    let instrument_id = pending.instrument_id;
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    let result = strategy.on_book_deltas(&book_deltas(
        instrument_id,
        &[(BookAction::Update, OrderSide::Sell, 0.43, 500.0)],
    ));

    assert!(
        result.is_ok(),
        "book-delta handling must not escape while entry reconciliation is pending: {result:#?}"
    );
    assert!(strategy.exposure.is_entry_reconcile_pending());
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
}

#[test]
fn position_closed_retains_entry_reconcile_until_entry_terminal_proof() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSED-BEFORE-OPEN");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            OrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            entry_client_order_id,
        )
        .expect("persistent entry order should build");
    seed_nt_working_order(
        &mut strategy,
        entry_order,
        PositionId::from("P-CLOSED-BEFORE-OPEN"),
    );
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    strategy.on_position_closed(position_closed_event(
        instrument_id,
        PositionId::from("P-CLOSED-BEFORE-OPEN"),
    ));

    assert!(!strategy.exposure.is_flat());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(
        evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .into_iter()
            .any(|event| matches!(
                event,
                CurrentFact::OrderLifecycle(record)
                    if record.transition
                        == crate::bolt_v3_current_evidence::OrderLifecycleTransition::PositionClosed
                        && record.client_order_id.as_deref() == Some("ENTRY-CLOSED-BEFORE-OPEN")
                        && record.position_id.as_deref() == Some("P-CLOSED-BEFORE-OPEN")
                        && record.outcome
                            == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::PendingEntry
            )),
        "position close must retain the unresolved entry lifecycle"
    );

    let canceled = order_canceled_event(entry_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );
    strategy.on_order_canceled(&canceled);
    assert!(strategy.exposure.is_flat());
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn position_closed_cancels_managed_resting_pending_entry_and_keeps_context() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.is_post_only = true;
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let (exec_handler, exec_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::exec_engine_queue_execute(),
        exec_handler,
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("POSITION-CLOSED-CANCELS-ENTRY");
    let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
    let (instrument_id, entry_client_order_id) =
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            position_id,
            position_quantity,
        );
    let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
        .best_ask
        .expect("ready-to-trade fixture should expose an ask");
    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            strategy
                .configured_entry_order_side()
                .expect("test config should carry entry order side"),
            position_quantity,
            Price::new(entry_price, 2),
            entry_client_order_id,
        )
        .expect("resting entry order should build through NT factory");
    seed_nt_working_order(&mut strategy, entry_order, position_id);
    let mut partial_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    partial_fill.last_qty = Quantity::new(1.0, 2);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(partial_fill),
    );

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    let exec_messages = exec_messages.get_messages();
    assert!(
        exec_messages.iter().any(|message| matches!(
            message,
            TradingCommand::CancelOrder(command)
                if command.client_order_id == entry_client_order_id
        )),
        "external position close should cancel the resting entry"
    );
    assert_eq!(
        strategy
            .exposure
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(strategy.pending_entry().is_some());

    let canceled = order_canceled_event(entry_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );
    strategy.on_order_canceled(&canceled);
    assert!(strategy.exposure.is_flat());
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn forced_flat_exit_in_shadow_mode_suppresses_resting_entry_cancel() {
    let configured_instruments =
        configured_outcome_instruments(&ready_to_trade_strategy_with_bound_economics());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy_with_bound_economics();
        let yes_instrument_id = strategy
            .active
            .books
            .up
            .instrument_id
            .expect("fixture should bind the yes instrument");
        let no_instrument_id = strategy
            .active
            .books
            .down
            .instrument_id
            .expect("fixture should bind the no instrument");
        let canonical_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let (yes_position, no_position) = match instrument_id == yes_instrument_id {
            true => (canonical_quantity.as_decimal(), Decimal::ZERO),
            false => (Decimal::ZERO, canonical_quantity.as_decimal()),
        };
        let submit_admission = submit_admission_with_provider_cap_and_canonical_position(
            Decimal::new(10_000, 0),
            recording_decision_evidence(),
            yes_instrument_id,
            no_instrument_id,
            yes_position,
            no_position,
        );
        strategy.context = StrategyBuildContext::new(
            fixture_order_economics(),
            recording_decision_evidence(),
            submit_admission,
            crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
            fixture_execution_venue(),
        )
        .with_position_authority(fixture_position_authority_capability(&strategy));
        configure_limit_base_entry_order(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        set_shadow_order_execution_policy(&mut strategy);
        strategy.active.phase = SelectionPhase::Freeze;
        let cache = register_test_strategy(&mut strategy);
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
        let position_id =
            PositionId::from(format!("POSITION-SHADOW-FORCED-{instrument_id}").as_str());
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let (instrument_id, entry_client_order_id) =
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                position_id,
                position_quantity,
            );
        let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
            .best_ask
            .expect("ready-to-trade fixture should expose an ask");
        let entry_order = strategy
            .build_configured_entry_order(
                instrument_id,
                strategy
                    .configured_entry_order_side()
                    .expect("test config should carry entry order side"),
                position_quantity,
                Price::new(entry_price, 2),
                entry_client_order_id,
            )
            .expect("resting entry order should build through NT factory");
        cache
            .borrow_mut()
            .add_order(
                entry_order,
                None,
                Some(ClientId::from(strategy.config.client_id.as_str())),
                true,
            )
            .expect("test cache should accept resting entry order");

        strategy
            .try_submit_exit_order_for_trigger(
                1_200,
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    1_200,
                )),
            )
            .expect("forced-flat exit must not error in shadow mode");

        let exec_messages = exec_messages.get_messages();
        assert!(
            !exec_messages
                .iter()
                .any(|message| matches!(message, TradingCommand::CancelOrder(_))),
            "shadow mode must not emit a venue CancelOrder on forced-flat exit: {instrument_id}"
        );
        let risk_messages = risk_messages.get_messages();
        assert!(
            !risk_messages
                .iter()
                .any(|message| matches!(message, TradingCommand::SubmitOrder(_))),
            "shadow mode must not emit a venue SubmitOrder on forced-flat exit: {instrument_id}"
        );
    }
}

#[test]
fn position_closed_in_shadow_mode_suppresses_resting_entry_cancel() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    configure_limit_base_entry_order(&mut strategy);
    strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
    strategy.config.entry_order.is_post_only = true;
    set_shadow_order_execution_policy(&mut strategy);
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let (exec_handler, exec_messages) =
        get_typed_into_message_saving_handler::<TradingCommand>(None);
    msgbus::register_trading_command_endpoint(
        MessagingSwitchboard::exec_engine_queue_execute(),
        exec_handler,
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("POSITION-SHADOW-CLOSED-ENTRY");
    let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
    let (instrument_id, entry_client_order_id) =
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            position_id,
            position_quantity,
        );
    let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
        .best_ask
        .expect("ready-to-trade fixture should expose an ask");
    let entry_order = strategy
        .build_configured_entry_order(
            instrument_id,
            strategy
                .configured_entry_order_side()
                .expect("test config should carry entry order side"),
            position_quantity,
            Price::new(entry_price, 2),
            entry_client_order_id,
        )
        .expect("resting entry order should build through NT factory");
    cache
        .borrow_mut()
        .add_order(
            entry_order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept resting entry order");

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    let exec_messages = exec_messages.get_messages();
    assert!(
        !exec_messages
            .iter()
            .any(|message| matches!(message, TradingCommand::CancelOrder(_))),
        "shadow mode must not emit a venue CancelOrder on external position close"
    );
    // The exposure still transitions to retain the pending-entry context; only
    // the venue cancel is suppressed in shadow mode.
    assert_eq!(
        strategy
            .exposure
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
}

#[test]
fn position_closed_keeps_entry_reconcile_pending_for_different_instrument() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSE-OTHER-INSTRUMENT");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let pending_instrument_id = pending.instrument_id;
    let other_instrument_id = configured_instrument_except(&strategy, pending_instrument_id);
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    strategy.on_position_closed(position_closed_event(
        other_instrument_id,
        PositionId::from("P-CLOSED-OTHER-INSTRUMENT"),
    ));

    assert!(strategy.exposure.is_entry_reconcile_pending());
    assert!(strategy.pending_entry().is_some());
}

#[test]
fn position_closed_quarantines_foreign_venue_managed_position_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-CLOSE-MANAGED");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn position_closed_quarantines_foreign_venue_exit_pending_position_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FOREIGN-CLOSE-EXIT");
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-FOREIGN-CLOSE"),
        FixturePositionLineage::CurrentProcess,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn position_closed_quarantines_foreign_venue_unsupported_position_id_collision() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let position_id = PositionId::from("P-FOREIGN-CLOSE-UNSUPPORTED");
    let book = strategy.active.books.up.clone();
    set_unsupported_observed(
        &mut strategy,
        OpenPositionState {
            lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
                Some("MKT-1".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Sell,
            side: PositionSide::Short,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.480,
            book,
        },
        UnsupportedObservedReason::LiveUnsupportedContract,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn position_closed_releases_unsupported_observed_for_same_position() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let position_id = PositionId::from("P-UNSUPPORTED-CLOSED");
    let book = strategy.active.books.up.clone();
    set_unsupported_observed(
        &mut strategy,
        OpenPositionState {
            lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
                Some("MKT-1".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Sell,
            side: PositionSide::Short,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.480,
            book,
        },
        UnsupportedObservedReason::LiveUnsupportedContract,
    );

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(strategy.exposure.is_flat());
}

#[test]
fn unsupported_position_observation_cannot_discard_current_exit_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-EXIT-UNSUPPORTED-OBSERVATION");
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-UNSUPPORTED-OBSERVATION");
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        FixturePositionLineage::CurrentProcess,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should retain current-process exit authority");
    let unsupported = UnsupportedObservedState {
        context: exit
            .position
            .clone()
            .expect("current-process exit should retain its position context"),
        reason: UnsupportedObservedReason::LiveUnsupportedContract,
    };

    strategy.exposure.set_unsupported_observed(
        unsupported,
        false,
        BlindRecoveryReason::InvalidLivePosition {
            entry_order_side: OrderSide::Sell,
            side: Some(PositionSide::Short),
        },
    );

    assert_eq!(
        strategy
            .exposure
            .exit_pending_snapshot()
            .map(|pending| pending.pending_exit.client_order_id),
        Some(exit_client_order_id),
        "position classification must not retire a still-working exit"
    );
}

#[test]
fn sell_fill_enters_recovery_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy.on_order_filled(&order_filled_event_with_details(
        entry_client_order_id,
        instrument_id,
        Some(PositionId::from("P-SHORT")),
        OrderSide::Sell,
    ));

    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.instrument_id),
        Some(instrument_id)
    );
}

#[test]
fn entry_fill_reconcile_branches_record_lifecycle_evidence() {
    let evidence = recording_decision_evidence();

    let mut awaiting = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut awaiting);
    let entry_client_order_id = ClientOrderId::from("ENTRY-FILL-AWAITING-POSITION");
    let pending = pending_entry_state(&mut awaiting, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut awaiting, pending);
    let mut fill =
        order_filled_event_with_details(entry_client_order_id, instrument_id, None, OrderSide::Buy);
    fill.last_qty = Quantity::new(2.0, 2);

    awaiting.on_order_filled(&fill);

    assert!(matches!(
        awaiting.exposure.entry_reconcile_for_test(),
        Some((_, EntryReconcileReason::AwaitingPositionMaterialization))
    ));

    let mut unsupported = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut unsupported);
    let entry_client_order_id = ClientOrderId::from("ENTRY-FILL-UNSUPPORTED-SIDE");
    let pending = pending_entry_state(&mut unsupported, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut unsupported, pending);
    let mut fill = order_filled_event_with_details(
        entry_client_order_id,
        instrument_id,
        Some(PositionId::from("P-FILL-UNSUPPORTED-SIDE")),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(3.0, 2);

    unsupported.on_order_filled(&fill);

    assert!(matches!(
        unsupported.exposure.entry_reconcile_for_test(),
        Some((
            _,
            EntryReconcileReason::UnsupportedEntryFillSide {
                order_side: OrderSide::Sell,
            }
        ))
    ));

    let events = evidence
        .recorded_facts()
        .expect("recorded current evidence must decode");
    assert!(
        events.iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::EntryReconcilePending
                    && record.client_order_id.as_deref() == Some("ENTRY-FILL-AWAITING-POSITION")
                    && record.position_id.is_none()
                    && record.filled_quantity.is_some()
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "awaiting-position entry fill must write lifecycle evidence"
    );
    assert!(
        events.into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::EntryReconcilePending
                    && record.client_order_id.as_deref() == Some("ENTRY-FILL-UNSUPPORTED-SIDE")
                    && record.position_id.as_deref() == Some("P-FILL-UNSUPPORTED-SIDE")
                    && record.order_side == Some(EvidenceOrderSide::Sell)
                    && record.filled_quantity.is_some()
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "unsupported-side entry fill must write lifecycle evidence"
    );
}

#[test]
fn unsupported_entry_fill_without_matching_context_keeps_unknown_side_absent() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-MISMATCHED-FILL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let pending_instrument_id = pending.instrument_id;
    let fill_instrument_id = configured_instrument_except(&strategy, pending_instrument_id);
    set_pending_entry(&mut strategy, pending);

    strategy.on_order_filled(&order_filled_event_with_details(
        entry_client_order_id,
        fill_instrument_id,
        Some(PositionId::from("P-MISMATCHED-FILL")),
        OrderSide::Sell,
    ));

    assert_eq!(
        strategy.exposure.blind_recovery_reason(),
        Some(&BlindRecoveryReason::InvalidLivePosition {
            entry_order_side: OrderSide::Sell,
            side: None,
        })
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn pending_entry_cannot_lineage_a_different_instrument_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-DIFFERENT-INSTRUMENT-POSITION");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let pending_instrument_id = pending.instrument_id;
    let observed_instrument_id = configured_instrument_except(&strategy, pending_instrument_id);
    let position_id = PositionId::from("P-DIFFERENT-INSTRUMENT");
    set_pending_entry(&mut strategy, pending);
    seed_nt_open_position(
        &mut strategy,
        observed_instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );

    strategy.on_position_opened(position_opened_event_with_details(
        observed_instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy.exposure.blind_recovery_reason(),
        Some(&BlindRecoveryReason::InvalidLivePosition {
            entry_order_side: OrderSide::Buy,
            side: Some(PositionSide::Long),
        })
    );
}

#[test]
fn entry_remainder_cannot_adopt_a_different_position_identity() {
    let mut strategy = ready_to_trade_strategy();
    let retained_instrument_id = selected_entry_instrument(&strategy);
    let retained_position_id = PositionId::from("P-RETAINED-ENTRY-REMAINDER");
    let retained_position = materialize_configured_position(
        &mut strategy,
        retained_instrument_id,
        retained_position_id,
        Quantity::new(5.0, 2),
        0.450,
    );
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-REMAINDER-DIFFERENT-POSITION"),
    );
    strategy.exposure.set_entry_remainder_for_test(
        pending,
        EntryRemainderPosition::Supported(managed_position_context(retained_position)),
    );
    let observed_instrument_id = retained_instrument_id;
    let observed_position_id = PositionId::from("P-CONFLICTING-ENTRY-REMAINDER");
    seed_nt_open_position(
        &mut strategy,
        observed_instrument_id,
        observed_position_id,
        Quantity::new(5.0, 2),
        0.450,
    );

    strategy.on_position_opened(position_opened_event(
        observed_instrument_id,
        observed_position_id,
        Quantity::new(5.0, 2),
        0.450,
    ));

    assert!(strategy.managed_position().is_none());
    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(ExposureOccupancy::BlindRecovery)
    ));
}

#[test]
fn pending_entry_short_position_event_stays_fail_closed_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let position_id = PositionId::from("P-SHORT");
    seed_nt_open_position_with_details(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Sell,
    );

    strategy.on_position_opened(position_opened_event_with_details(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Sell,
        PositionSide::Short,
    ));

    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
    assert!(strategy.managed_position().is_none());
    let quarantined = strategy
        .exposure
        .unsupported_observation()
        .expect("expected unsupported observed exposure");
    assert_eq!(quarantined.context.instrument_id, instrument_id);
    assert_eq!(quarantined.context.position_id, position_id);
    let observed = strategy
        .tracked_observed_position()
        .expect("unsupported context should project NT position truth");
    assert_eq!(observed.entry_order_side, OrderSide::Sell);
    assert_eq!(observed.side, PositionSide::Short);
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn live_position_event_quarantines_foreign_venue_position() {
    // Live-path regression lock (mirror of
    // `recovery_bootstrap_quarantines_foreign_venue_position`). The recovery path already
    // quarantines a foreign-venue cached position before its contract check; the LIVE
    // position-event path (`materialize_position_from_event`, driven by `on_position_opened`)
    // must do the same. This event carries a SUPPORTED side/contract shape (Buy / Long, the
    // exact shape the same-venue baseline `position_events_update_live_position_state` adopts
    // into Managed), so the ONLY thing making it foreign is the venue. Under the pre-guard
    // code this foreign event would pass the side + contract checks and become Managed (the
    // exit path then submits a real order against that foreign instrument_id); the new venue
    // guard is what diverts it to blind recovery instead.
    let mut strategy = ready_to_trade_strategy();
    let execution_venue = strategy.context.execution_venue();
    let execution_instrument = configured_outcome_instruments(&strategy)
        .into_iter()
        .next()
        .expect("ready-to-trade fixture should expose a configured instrument");
    assert_eq!(
        execution_instrument.venue, execution_venue,
        "fixture instrument must be on the execution venue so only the venue is changed",
    );
    // Same symbol, foreign venue: the venue is the ONLY difference from a managed position.
    let foreign_instrument =
        InstrumentId::new(execution_instrument.symbol, Venue::from("HYPERLIQUID"));
    assert_ne!(
        foreign_instrument.venue, execution_venue,
        "foreign instrument must be on a non-execution venue",
    );

    strategy.on_position_opened(position_opened_event_with_details(
        foreign_instrument,
        PositionId::from("P-FOREIGN-LIVE"),
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    // Observable exposure: quarantined to blind recovery, never adopted into Managed.
    assert!(
        matches!(
            strategy.exposure.blind_recovery_reason(),
            Some(BlindRecoveryReason::ForeignVenuePosition { .. })
        ),
        "foreign-venue live position event must be quarantined to blind recovery, got {:?}",
        strategy.exposure,
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn order_fill_entry_quarantines_foreign_venue_position() {
    // Live order-fill regression lock (sibling of
    // `live_position_event_quarantines_foreign_venue_position`). The entry-fill branch of
    // `on_order_filled` matches a fill to our pending entry by client_order_id ALONE, then
    // adopts the fill's instrument_id into Managed (origin StrategyEntry). A foreign-venue fill
    // that happens to carry our pending entry's client_order_id must NOT be adopted — the exit
    // path would otherwise submit a real order against the foreign instrument_id. The shared
    // venue-adoption guard must divert it to blind recovery, exactly as the position-event path
    // does. Pre-guard this fill (Some position_id + supported Buy/Long side) would become Managed.
    let mut strategy = ready_to_trade_strategy();
    let execution_venue = strategy.context.execution_venue();
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-FILL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let execution_instrument_id = pending.instrument_id;
    assert_eq!(
        execution_instrument_id.venue, execution_venue,
        "pending entry must be on the execution venue so only the fill venue differs",
    );
    set_pending_entry(&mut strategy, pending);

    // Same symbol, foreign venue: the venue is the ONLY difference from our pending entry.
    let foreign_instrument_id =
        InstrumentId::new(execution_instrument_id.symbol, Venue::from("HYPERLIQUID"));
    assert_ne!(
        foreign_instrument_id.venue, execution_venue,
        "foreign fill must be on a non-execution venue",
    );

    strategy.on_order_filled(&order_filled_event_with_details(
        entry_client_order_id,
        foreign_instrument_id,
        Some(PositionId::from("P-FOREIGN-FILL")),
        OrderSide::Buy,
    ));

    // Observable exposure: quarantined to blind recovery, never adopted into Managed.
    assert!(
        matches!(
            strategy.exposure.blind_recovery_reason(),
            Some(BlindRecoveryReason::ForeignVenuePosition { .. })
        ),
        "foreign-venue entry fill must be quarantined to blind recovery, got {:?}",
        strategy.exposure,
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn pending_entry_unknown_position_side_stays_fail_closed_without_materializing_position() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let entry_client_order_id = ClientOrderId::from("ENTRY-BAD-SIDE");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy.on_position_opened(position_opened_event_with_details(
        instrument_id,
        PositionId::from("P-BAD-SIDE"),
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Flat,
    ));

    assert!(matches!(
        strategy.exposure.entry_gate(),
        ExposureEntryGate::Recovering(_)
    ));
    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(
        evidence.recorded_facts().expect("recorded current evidence must decode").into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::EntryReconcilePending
                    && record.source == ORDER_LIFECYCLE_SOURCE_POSITION_EVENT
                    && record.client_order_id.as_deref() == Some("ENTRY-BAD-SIDE")
                    && record.position_id.as_deref() == Some("P-BAD-SIDE")
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::EntryReconcilePending
        )),
        "invalid observed position side must write lifecycle evidence"
    );
}

#[test]
fn position_opened_after_rotation_preserves_existing_position_context() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_a = selected_entry_instrument(&strategy);
    let preserved_book = configured_book_for_instrument(&mut strategy, instrument_a);
    let preserved_position = materialize_configured_position(
        &mut strategy,
        instrument_a,
        PositionId::from("P-A"),
        Quantity::new(10.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        preserved_position,
        FixturePositionLineage::CurrentProcess,
    );

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.active.interval_open = Some(3_200.0);
    strategy.on_position_opened(position_opened_event(
        instrument_a,
        PositionId::from("P-A"),
        Quantity::new(10.0, 2),
        0.450,
    ));

    let open_position =
        managed_position_snapshot(&strategy).expect("position should remain tracked");
    assert_eq!(open_position.lifecycle.market_id(), Some("MKT-1"));
    assert_eq!(open_position.lifecycle.settlement_strike(), Some(3_100.0));
    assert_eq!(
        open_position.lifecycle.selection_published_at_ms(),
        Some(1_000)
    );
    assert_eq!(
        open_position.lifecycle.seconds_to_expiry_at_selection(),
        Some(300)
    );
    assert_eq!(open_position.book.best_bid, preserved_book.best_bid);
}

#[test]
fn bootstrap_recovery_from_cache_ignores_foreign_venue_position() {
    // Recovery-path regression lock. The entry path scopes selection to the
    // execution venue; the recovery path must do the same. A foreign-venue cached position with
    // a supported contract shape must NOT be accepted into Managed state, because the exit
    // submission path uses the position's instrument_id directly with no additional venue gate.
    let mut strategy = test_strategy();
    assert_eq!(
        strategy.context.execution_venue(),
        fixture_execution_venue(),
        "harness precondition: production execution venue must be the POLYMARKET fixture",
    );
    let cache = register_test_strategy(&mut strategy);

    // Foreign-venue (HYPERLIQUID) instrument and position
    let foreign_instrument = updown_binary_option(
        "token-up.HYPERLIQUID",
        "foreign-market",
        "market-foreign",
        "Up",
        1_000,
        2_000,
    );
    let foreign_fill = order_filled_event_with_details(
        ClientOrderId::from("FOREIGN-ORDER-001"),
        foreign_instrument.id(),
        Some(PositionId::from("POS-FOREIGN-001")),
        OrderSide::Buy,
    );
    let foreign_position = Position::new(&foreign_instrument, foreign_fill);

    // Seed the cache with the foreign-venue instrument and position
    {
        let mut cache_mut = cache.borrow_mut();
        cache_mut
            .add_instrument(foreign_instrument.clone())
            .expect("test cache should accept the seeded instrument");
        cache_mut
            .add_position(&foreign_position, NtOmsType::Netting)
            .expect("test cache should accept the seeded position");
    }

    // Verify the position is present when querying WITHOUT venue scoping
    assert_eq!(
        cache
            .borrow()
            .positions_open(
                None,
                None,
                Some(&StrategyId::from("BINARYORACLEEDGETAKER-001")),
                None,
                None,
            )
            .len(),
        1,
        "foreign position must exist in the unscoped cache",
    );

    strategy.bootstrap_recovery_from_cache();

    // The foreign-venue position must be ignored; strategy stays Flat.
    assert!(
        strategy.exposure.is_flat(),
        "a foreign-venue cached position must NOT be recovered into Managed state: got {:?}",
        strategy.exposure,
    );
}

#[test]
fn bootstrap_recovery_from_cache_loads_execution_venue_position() {
    // Restart observations never mint current-process route authority.
    let mut strategy = test_strategy();
    assert_eq!(
        strategy.context.execution_venue(),
        fixture_execution_venue(),
        "harness precondition: production execution venue must be the POLYMARKET fixture",
    );
    let cache = register_test_strategy(&mut strategy);

    let execution_instrument = updown_binary_option(
        "token-up.POLYMARKET",
        "execution-market",
        "market-execution",
        "Up",
        1_000,
        2_000,
    );
    let execution_fill = order_filled_event_with_details(
        ClientOrderId::from("EXEC-ORDER-001"),
        execution_instrument.id(),
        Some(PositionId::from("POS-EXEC-001")),
        OrderSide::Buy,
    );
    let execution_position = Position::new(&execution_instrument, execution_fill);

    {
        let mut cache_mut = cache.borrow_mut();
        cache_mut
            .add_instrument(execution_instrument.clone())
            .expect("test cache should accept the seeded instrument");
        cache_mut
            .add_position(&execution_position, NtOmsType::Netting)
            .expect("test cache should accept the seeded position");
    }

    strategy.bootstrap_recovery_from_cache();

    assert!(
        matches!(
            strategy.exposure.blind_recovery_reason(),
            Some(BlindRecoveryReason::RestartOpenPosition {
                instrument_id,
                position_id,
            }) if instrument_id.to_string() == "token-up.POLYMARKET"
                && position_id.to_string() == "POS-EXEC-001"
        ),
        "a restart-observed position must remain non-routing: {:?}",
        strategy.exposure,
    );
}

#[test]
fn task5_entry_gate_reports_all_frozen_block_reasons_explicitly() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 1_000));
    strategy.market_lifecycle.insert(
        "MKT-1".to_string(),
        MarketLifecycleLedger {
            cooldown_expires_at_ms: Some(5_000),
            churn_count: 0,
        },
    );
    let pending = PendingEntryState {
        client_order_id: ClientOrderId::from("ENTRY-001"),
        submitted_at_ms: Some(1_000),
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            None,
            None,
            None,
            None,
            None,
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        book: strategy.active.books.up.clone(),
    };
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    let decision = strategy.entry_gate_decision_at(2_000);

    assert_eq!(
        decision.blocked_by,
        vec![
            EntryBlockReason::PhaseNotActive,
            EntryBlockReason::MetadataMismatch,
            EntryBlockReason::ActiveBookNotPriced,
            EntryBlockReason::IntervalOpenMissing,
            EntryBlockReason::WarmupIncomplete,
            EntryBlockReason::RecoveryMode,
            EntryBlockReason::MarketCoolingDown,
            EntryBlockReason::ForcedFlat(ForcedFlatReason::Freeze),
            EntryBlockReason::ForcedFlat(ForcedFlatReason::StaleReference),
            EntryBlockReason::ForcedFlat(ForcedFlatReason::ThinBook),
            EntryBlockReason::OnePositionInvariant(ExposureOccupancy::EntryReconcilePending),
        ]
    );
}

#[test]
fn entry_arm_rejects_occupied_exposure_without_panicking() {
    let mut strategy = ready_to_trade_strategy();
    let invariant_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-INVARIANT-1"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(5.0, 2),
        avg_px_open: 0.45,
        book: strategy.active.books.up.clone(),
    };
    set_exit_pending(
        &mut strategy,
        invariant_position,
        ClientOrderId::from("EXIT-001"),
        FixturePositionLineage::CurrentProcess,
    );

    let pending = PendingEntryState {
        client_order_id: ClientOrderId::from("ENTRY-BLOCKED-001"),
        submitted_at_ms: Some(1_000),
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            None,
            None,
            None,
            None,
            None,
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        book: strategy.active.books.up.clone(),
    };

    assert_eq!(
        strategy.exposure.arm_entry(pending),
        Err(EntryArmError::Occupied(ExposureOccupancy::ExitPending))
    );
}

#[test]
fn entry_gate_reports_one_position_invariant_only_on_occupancy_change() {
    let mut strategy = ready_to_trade_strategy();
    let invariant_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-INVARIANT-2"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(5.0, 2),
        avg_px_open: 0.45,
        book: strategy.active.books.up.clone(),
    };
    set_exit_pending(
        &mut strategy,
        invariant_position,
        ClientOrderId::from("EXIT-001"),
        FixturePositionLineage::CurrentProcess,
    );

    let first = strategy.entry_gate_decision_at(2_000);
    let second = strategy.entry_gate_decision_at(2_001);

    assert!(
        first
            .blocked_by
            .contains(&EntryBlockReason::OnePositionInvariant(
                ExposureOccupancy::ExitPending
            ))
    );
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
    assert_eq!(first.blocked_by, second.blocked_by);

    strategy.exposure.set_flat_for_test();
    let cleared = strategy.entry_gate_decision_at(2_002);
    assert!(
        !cleared
            .blocked_by
            .contains(&EntryBlockReason::OnePositionInvariant(
                ExposureOccupancy::ExitPending
            ))
    );
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
}

#[test]
fn entry_gate_reports_only_unexpected_occupancies_as_invariant_violations() {
    let mut strategy = ready_to_trade_strategy();
    set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);

    let decision = strategy.entry_gate_decision_at(2_000);

    assert!(
        decision
            .blocked_by
            .contains(&EntryBlockReason::OnePositionInvariant(
                ExposureOccupancy::BlindRecovery
            ))
    );
    assert_eq!(
        strategy.last_reported_exposure_occupancy.get(),
        Some(ExposureOccupancy::BlindRecovery)
    );
}

#[test]
fn taker_hardening_guards_are_entry_only_and_do_not_block_exits() {
    // Exits must always be able to fire (risk-off), even with a crossed book
    // and an armed spike cooldown. The exit path is structurally independent
    // of `entry_gate_decision_at`, so neither new gate reason can reach it.
    let configured_instruments =
        configured_outcome_instruments(&ready_to_trade_strategy_with_bound_economics());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy_with_bound_economics();
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        strategy.active.phase = SelectionPhase::Freeze;
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("POSITION-ENTRY-ONLY-{instrument_id}").as_str()),
            position_quantity,
        );

        // Cross both active books and arm the spike cooldown well into the future.
        strategy.active.books.up.best_bid = Some(0.46);
        strategy.active.books.up.best_ask = Some(0.45);
        strategy.active.books.down.best_bid = Some(0.46);
        strategy.active.books.down.best_ask = Some(0.45);
        strategy.pricing.spike_until_ms = Some(1_000_000);

        // The entry gate is blocked by both new guards...
        let gate = strategy.entry_gate_decision_at(1_200);
        assert!(
            gate.blocked_by.contains(&EntryBlockReason::BookCrossed),
            "{instrument_id}: crossed book must block entry"
        );
        assert!(
            gate.blocked_by
                .contains(&EntryBlockReason::SpotSpikeCooldown),
            "{instrument_id}: armed spike cooldown must block entry"
        );

        // ...but the exit still submits.
        let decision = strategy.exit_intent_decision_at(1_200);
        assert_eq!(decision.blocked_reason, None, "{instrument_id}");
        assert_eq!(
            decision.forced_flat_reasons,
            vec![ForcedFlatReason::Freeze],
            "{instrument_id}"
        );
        assert_eq!(
            decision.order_side,
            Some(OrderSide::Sell),
            "{instrument_id}"
        );
        assert!(decision.instrument_id.is_some(), "{instrument_id}");
        assert!(decision.quantity.is_some(), "{instrument_id}");
    }
}

#[test]
fn task5_entry_gate_blocks_on_active_phase_forced_flat_reasons() {
    let mut strategy = ready_to_trade_strategy();
    strategy.active.last_reference_ts_ms = Some(1_000);
    strategy.active.books.up.liquidity_available = Some(50.0);
    strategy.active.books.down.liquidity_available = Some(50.0);
    strategy.active.fast_venue_incoherent = true;

    let decision = strategy.entry_gate_decision_at(3_000);

    assert_eq!(
        decision.blocked_by,
        vec![
            EntryBlockReason::ForcedFlat(ForcedFlatReason::StaleReference),
            EntryBlockReason::ForcedFlat(ForcedFlatReason::ThinBook),
            EntryBlockReason::ForcedFlat(ForcedFlatReason::FastVenueIncoherent),
        ]
    );
}

#[test]
fn task5_cooldown_is_per_market_and_recovery_blocks_new_entries() {
    let mut strategy = ready_to_trade_strategy();
    strategy.arm_market_cooldown("MKT-1", 1_000);

    assert!(strategy.market_in_cooldown("MKT-1", 30_999));
    assert!(!strategy.market_in_cooldown("MKT-2", 30_999));

    set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);
    let decision = strategy.entry_gate_decision_at(2_000);

    assert!(
        decision
            .blocked_by
            .contains(&EntryBlockReason::RecoveryMode)
    );
}

#[test]
fn exit_evaluation_log_fields_use_position_context_after_rotation() {
    let mut strategy = test_strategy();
    strategy.config.warmup_tick_count = 2;
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    strategy.active.interval_open = Some(3_100.0);
    strategy.active.warmup_count = 2;
    strategy.active.last_reference_ts_ms = Some(2_000);
    let open_position = OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id: strategy.active.books.up.instrument_id.unwrap(),
        position_id: PositionId::from("P-UP-LOG-001"),
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity: Quantity::new(10.0, 2),
        avg_px_open: 0.450,
        book: strategy.active.books.up.clone(),
    };
    set_managed_position(
        &mut strategy,
        open_position,
        FixturePositionLineage::CurrentProcess,
    );

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.active.interval_open = Some(3_200.0);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_101.0, 2_000)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 2_000);

    let decision = strategy.exit_intent_decision_at(2_000);
    let fields = strategy.exit_evaluation_log_fields_at(
        2_000,
        ExitEvaluationTriggerContext::unknown(2_000),
        &decision,
    );

    assert_eq!(fields.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(fields.spot_price, None);
    assert_eq!(fields.spot_venue_name, None);
    assert_eq!(fields.interval_open, Some(3_100.0));
    assert_eq!(fields.seconds_to_expiry, Some(299));
    assert_eq!(fields.fair_probability_up, None);
    assert_eq!(fields.hold_ev_bps, None);
    assert_eq!(
        fields.realized_vol_source_venue.as_deref(),
        Some("<SOURCE_ID>"),
        "receive-fresh RV source evidence remains available after market rotation"
    );
    assert_eq!(fields.realized_vol_source_ts_ms, Some(2_000));
}

#[test]
fn unknown_recovered_position_lifecycle_blocks_instead_of_liquidating_by_default() {
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    let instrument_id = InstrumentId::from("0xcondition-222.POLYMARKET");
    let mut tracked_book = OutcomeBookState::from_instrument_id(instrument_id);
    tracked_book.last_observed_instrument_id = Some(instrument_id);
    tracked_book.best_bid = Some(0.520);
    tracked_book.best_ask = Some(0.530);
    tracked_book.liquidity_available = Some(100.0);
    set_managed_position(
        &mut strategy,
        OpenPositionState {
            lifecycle: BoltV3PositionMarketLifecycle::missing(),
            instrument_id,
            position_id: PositionId::from("P-UNKNOWN-001"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.480,
            book: tracked_book,
        },
        FixturePositionLineage::RestartObserved,
    );

    let decision = strategy.exit_intent_decision_at(2_000);

    assert_eq!(decision.evaluation.exit_decision, None);
    assert_eq!(decision.instrument_id, None);
    assert_eq!(decision.order_side, None);
    assert_eq!(decision.price, None);
    assert_eq!(decision.quantity, None);
    assert!(matches!(
        strategy.exposure.blind_recovery_reason(),
        Some(BlindRecoveryReason::RestartOpenPosition { .. })
    ));
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceExitBlockedReason::NoOpenPosition)
    );
}

#[test]
fn exposure_entry_reconcile_pending_preserves_context_and_blocks_new_entries() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let pending = PendingEntryState {
        client_order_id: ClientOrderId::from("ENTRY-RECONCILE-001"),
        submitted_at_ms: Some(1_000),
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id,
        book: strategy.active.books.up.clone(),
    };
    let mut exposure = ExposureOwner::new();
    exposure.set_entry_reconcile_for_test(
        pending.clone(),
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    assert_eq!(exposure.pending_entry(), Some(&pending));
    assert_eq!(
        exposure.occupancy(),
        Some(ExposureOccupancy::EntryReconcilePending)
    );
    assert!(!exposure.is_flat());
}

#[test]
fn exposure_exit_pending_stores_only_intent_correlation_and_bolt_context() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let client_order_id = ClientOrderId::from("EXIT-STATE-001");
    let position_id = PositionId::from("P-EXIT-STATE-001");
    let quantity = Quantity::new(10.0, 2);
    let lease = strategy
        .context
        .position_authority()
        .expect("fixture strategy should have position authority")
        .acquire_for_position(position_id, instrument_id)
        .expect("fixture exit authority lease should acquire");
    let authority = BoltV3ExitOrderAuthorityHandle::locally_submitted_for_test(
        client_order_id,
        instrument_id,
        position_id,
        quantity.as_decimal(),
        PositionSideSpecified::Long,
        quantity,
        lease,
    )
    .expect("fixture exit authority should build");
    let context = managed_position_context(OpenPositionState {
        lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
            Some("MKT-1".to_string()),
            Some(OutcomeSide::Up),
            Some(3_100.0),
            Some(3_100.0),
            Some(301_000),
            Some(1_000),
            Some(300),
        ),
        instrument_id,
        position_id,
        entry_order_side: OrderSide::Buy,
        side: PositionSide::Long,
        quantity,
        avg_px_open: 0.450,
        book: strategy.active.books.up.clone(),
    });
    let exit_pending = ExitPendingState {
        position: Some(context),
        pending_exit: PendingExitState {
            client_order_id,
            submitted_at_ms: Some(1_000),
            market_id: Some("MKT-1".to_string()),
            position_id: Some(position_id),
        },
        authority,
    };

    assert_eq!(
        exit_pending.pending_exit.client_order_id,
        ClientOrderId::from("EXIT-STATE-001")
    );
    assert_eq!(
        exit_pending
            .position
            .as_ref()
            .map(|state| state.position_id),
        Some(PositionId::from("P-EXIT-STATE-001"))
    );
}

#[test]
fn stale_exit_route_return_cannot_overwrite_a_synchronous_terminal_transition() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-ATTEMPT-GENERATION"),
        Quantity::new(10.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-ATTEMPT-GENERATION"),
        FixturePositionLineage::CurrentProcess,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create exit authority");
    let managed = exit
        .position
        .clone()
        .expect("local attempt must retain its managed position");
    strategy.exposure.set_managed_for_test(managed);
    let capability = strategy
        .exposure
        .begin_exit(exit.pending_exit.clone(), exit.authority.clone())
        .expect("managed exposure should begin one exit attempt");

    // Models the cache-first synchronous NT callback that advances the attempt
    // while the raw submit leaf is still on the stack.
    strategy.exposure.set_terminal_exit_for_test(exit.clone());
    strategy
        .exposure
        .settle_exit_attempt(capability, ExitAttemptSettlement::Abort);

    assert_eq!(
        strategy.exposure.terminal_exit_snapshot(),
        Some(exit),
        "the callback-owned terminal fence must win over the stale route return"
    );
}

#[test]
fn exit_attempt_generation_overflow_fails_without_mutating_exposure() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-GENERATION-OVERFLOW"),
        Quantity::new(10.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-GENERATION-OVERFLOW"),
        FixturePositionLineage::CurrentProcess,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create exit authority");
    let managed = exit
        .position
        .clone()
        .expect("local exit fixture must retain managed position");
    strategy.exposure.set_managed_for_test(managed.clone());
    strategy
        .exposure
        .set_next_exit_generation_for_test(u64::MAX);

    let failure = strategy
        .exposure
        .begin_exit(exit.pending_exit, exit.authority)
        .expect_err("checked generation overflow must fail closed");

    assert!(failure.to_string().contains("generation overflow"));
    assert_eq!(strategy.exposure.managed_position_context(), Some(&managed));
}

#[test]
fn stale_entry_arm_capability_cannot_abort_a_reused_client_order_identity() {
    let mut strategy = ready_to_trade_strategy();
    let client_order_id = ClientOrderId::from("ENTRY-REUSED-GENERATION");
    let pending = pending_entry_state(&mut strategy, client_order_id);
    let stale = strategy
        .exposure
        .arm_entry(pending.clone())
        .expect("flat exposure should arm the first generation");
    strategy.exposure.set_flat_for_test();
    let current = strategy
        .exposure
        .arm_entry(pending)
        .expect("flat exposure should arm the replacement generation");

    strategy
        .exposure
        .settle_entry_arm(stale, EntryArmSettlement::Abort);

    assert_eq!(
        strategy
            .exposure
            .pending_entry_arm()
            .map(|pending| pending.client_order_id),
        Some(client_order_id),
        "a stale generation must not abort the current arm"
    );
    strategy
        .exposure
        .settle_entry_arm(current, EntryArmSettlement::Abort);
}

#[test]
fn live_entered_and_pending_adopted_positions_retain_interval_end_boundary() {
    let live_pending_entry = || {
        let mut strategy = ready_to_trade_strategy_with_bound_economics();
        register_test_strategy_with_active_instruments(&mut strategy);
        set_active_books_best_prices(&mut strategy, 0.40, 0.41);
        strategy.config.order_notional_target = 25.0;
        strategy.config.maximum_position_notional = 25.0;
        strategy.config.risk_lambda = 0.0001;
        strategy
            .try_submit_entry_order(1_200)
            .expect("live entry should be admissible")
            .expect("live entry should create pending exposure");
        strategy
    };

    let mut fill_strategy = live_pending_entry();
    let (fill_client_order_id, fill_instrument_id, fill_interval_end_ms) = {
        let pending = fill_strategy
            .pending_entry()
            .expect("live submit should retain pending entry context");
        (
            pending.client_order_id,
            pending.instrument_id,
            pending
                .lifecycle
                .interval_end_ms()
                .expect("live pending entry must inherit the selected market interval end"),
        )
    };
    let fill_position_id = PositionId::from("P-LIVE-ENTRY-INTERVAL-PIN");
    seed_nt_open_position(
        &mut fill_strategy,
        fill_instrument_id,
        fill_position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    fill_strategy.on_order_filled(&order_filled_event(
        fill_client_order_id,
        fill_instrument_id,
        fill_position_id,
    ));
    assert_eq!(
        managed_position_snapshot(&fill_strategy)
            .and_then(|position| position.lifecycle.interval_end_ms()),
        Some(fill_interval_end_ms),
        "live-entered position must preserve the pending entry interval end"
    );

    let mut position_strategy = live_pending_entry();
    let (position_instrument_id, position_interval_end_ms) = {
        let pending = position_strategy
            .pending_entry()
            .expect("live submit should retain pending entry context");
        (
            pending.instrument_id,
            pending
                .lifecycle
                .interval_end_ms()
                .expect("live pending entry must inherit the selected market interval end"),
        )
    };
    let adopted_position_id = PositionId::from("P-PENDING-ADOPTED-INTERVAL-PIN");
    seed_nt_open_position(
        &mut position_strategy,
        position_instrument_id,
        adopted_position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    position_strategy.on_position_opened(position_opened_event(
        position_instrument_id,
        adopted_position_id,
        Quantity::new(10.0, 2),
        0.450,
    ));
    assert_eq!(
        managed_position_snapshot(&position_strategy)
            .and_then(|position| position.lifecycle.interval_end_ms()),
        Some(position_interval_end_ms),
        "pending-adopted position must inherit the pending entry interval end"
    );
}
