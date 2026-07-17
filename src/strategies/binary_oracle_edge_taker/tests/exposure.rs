#![cfg(test)]

use super::*;
use nautilus_trading::Strategy;
use std::sync::Arc;

#[test]
fn position_events_update_live_position_state() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-001");

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert!(strategy.managed_position().is_some());
    assert_eq!(
        strategy.managed_position().map(|managed| managed.origin),
        Some(ManagedPositionOrigin::RecoveryBootstrap)
    );
    let managed_position =
        managed_position_ref(&strategy).expect("position should be managed after open event");
    assert_eq!(managed_position.lifecycle.market_id(), None);
    assert_eq!(managed_position.instrument_id, instrument_id);
    assert_eq!(managed_position.position_id, position_id);
    assert_eq!(managed_position.lifecycle.outcome_side(), None);
    assert_eq!(managed_position.entry_order_side, OrderSide::Buy);
    assert_eq!(managed_position.side, PositionSide::Long);
    assert_eq!(managed_position.quantity, Quantity::new(10.0, 2));
    assert_eq!(managed_position.avg_px_open, 0.450);
    assert_eq!(managed_position.lifecycle.settlement_strike(), None);
    assert_eq!(managed_position.lifecycle.selection_published_at_ms(), None);
    assert_eq!(
        managed_position.lifecycle.seconds_to_expiry_at_selection(),
        None
    );
    let managed_book = managed_position.book.clone();
    let expected_book = OutcomeBookState::from_instrument_id(instrument_id);
    assert_eq!(managed_book, expected_book);

    let recovered_position = managed_position_ref(&strategy)
        .cloned()
        .expect("position should be managed before exit pending");
    set_exit_pending(
        &mut strategy,
        recovered_position,
        ClientOrderId::from("EXIT-001"),
        false,
        false,
        ManagedPositionOrigin::RecoveryBootstrap,
    );
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(strategy.managed_position().is_none());
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(ClientOrderId::from("EXIT-001"))
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(false)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.close_received),
        Some(true)
    );
    assert!(!strategy.exposure.is_recovering());
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy
        .on_order_filled(&order_filled_event(
            exit_client_order_id,
            instrument_id,
            position_id,
        ))
        .expect("exit fill bookkeeping should succeed");

    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.close_received),
        Some(false)
    );
    assert!(strategy.managed_position().is_some());

    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(strategy.managed_position().is_none());
    assert!(pending_exit_ref(&strategy).is_none());
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
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
        .exit_pending()
        .expect("position change should keep exit pending");
    assert_eq!(
        exit_pending.pending_exit.client_order_id,
        exit_client_order_id
    );
    assert_eq!(exit_pending.pending_exit.position_id, Some(position_id));
    assert!(!exit_pending.pending_exit.fill_received);
    assert!(!exit_pending.pending_exit.close_received);

    let position = exit_pending
        .position
        .as_ref()
        .expect("exit pending should keep managed position");
    assert_eq!(position.origin, ManagedPositionOrigin::StrategyEntry);
    assert_eq!(position.position.quantity, Quantity::new(7.0, 2));
    assert_eq!(position.position.avg_px_open, 0.470);
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(ClientOrderId::from("EXIT-001"))
    );
    assert!(strategy.managed_position().is_some());
}

#[test]
fn unrelated_position_close_does_not_clear_filled_pending_exit() {
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
        true,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(ClientOrderId::from("EXIT-001"))
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true)
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    canceled
        .on_order_canceled(&order_canceled_event(exit_client_order_id, instrument_id))
        .expect("exit cancel bookkeeping should succeed");
    assert!(pending_exit_ref(&canceled).is_none());
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    rejected.on_order_rejected(order_rejected_event(exit_client_order_id, instrument_id));
    assert!(pending_exit_ref(&rejected).is_none());
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    expired.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));
    assert!(pending_exit_ref(&expired).is_none());
    assert!(expired.managed_position().is_some());
}

#[test]
fn filled_exit_pending_ignores_stale_cancel_until_position_close() {
    let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-CANCEL");

    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FILLED-CANCEL"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        true,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy
        .on_order_canceled(&order_canceled_event(exit_client_order_id, instrument_id))
        .expect("stale cancel should not clear filled exit pending");
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true)
    );

    strategy.on_position_closed(position_closed_event(
        instrument_id,
        PositionId::from("P-FILLED-CANCEL"),
    ));
    assert!(pending_exit_ref(&strategy).is_none());
    assert!(strategy.managed_position().is_none());
}

#[test]
fn filled_exit_pending_ignores_stale_reject() {
    let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-REJECT");

    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FILLED-REJECT"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        true,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.on_order_rejected(order_rejected_event(exit_client_order_id, instrument_id));
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true)
    );
}

#[test]
fn filled_exit_pending_ignores_stale_expire() {
    let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-EXPIRE");

    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-FILLED-EXPIRE"),
        Quantity::new(1.0, 2),
        0.45,
    );
    set_exit_pending(
        &mut strategy,
        position,
        exit_client_order_id,
        true,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
        Some(exit_client_order_id)
    );
    assert_eq!(
        pending_exit_ref(&strategy).map(|pending| pending.fill_received),
        Some(true)
    );
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    let mut fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    strategy
        .on_order_filled(&fill)
        .expect("partial exit fill bookkeeping should succeed");
    strategy.materialize_position_from_event(
        PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(6.0, 2),
            avg_px_open: 0.45,
        },
        0,
    );

    strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

    assert!(pending_exit_ref(&strategy).is_none());
    assert_eq!(
        strategy.exposure_occupancy(),
        Some(ExposureOccupancy::ManagedPosition)
    );
    assert_eq!(
        managed_position_ref(&strategy).map(|position| position.quantity),
        Some(Quantity::new(6.0, 2))
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
        false,
        true,
        ManagedPositionOrigin::StrategyEntry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy
        .on_order_filled(&order_filled_event(
            exit_client_order_id,
            foreign_instrument_id,
            position_id,
        ))
        .expect("foreign-venue exit fill should fail closed");

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
        ManagedPositionOrigin::StrategyEntry,
        pending_entry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy
        .on_order_filled(&order_filled_event_with_details(
            entry_client_order_id,
            foreign_instrument_id,
            Some(PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL")),
            OrderSide::Buy,
        ))
        .expect("foreign-venue managed entry fill should fail closed");

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

    strategy
        .on_order_canceled(&order_canceled_event(
            entry_client_order_id,
            foreign_instrument_id,
        ))
        .expect("foreign-venue entry cancel should fail closed");

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
        ManagedPositionOrigin::StrategyEntry,
        pending_entry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy
        .on_order_canceled(&order_canceled_event(
            entry_client_order_id,
            foreign_instrument_id,
        ))
        .expect("foreign-venue managed entry cancel should fail closed");

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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy
        .on_order_canceled(&order_canceled_event(
            exit_client_order_id,
            foreign_instrument_id,
        ))
        .expect("foreign-venue exit cancel should fail closed");

    assert_foreign_venue_blind_recovery(&strategy);
}

#[test]
fn position_event_without_context_does_not_guess_side_from_suffix() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = InstrumentId::from("external-MKT-1-UP.POLYMARKET");

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        PositionId::from("P-SUFFIX-001"),
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert_eq!(
        managed_position_ref(&strategy).and_then(|position| position.lifecycle.outcome_side()),
        None
    );
    let position = managed_position_ref(&strategy).expect("position should be tracked");
    assert_eq!(position.lifecycle.market_id(), None);
    assert_eq!(position.lifecycle.settlement_strike(), None);
    assert_eq!(position.lifecycle.selection_published_at_ms(), None);
    assert_eq!(position.lifecycle.seconds_to_expiry_at_selection(), None);
    assert_eq!(
        strategy.managed_position().map(|managed| managed.origin),
        Some(ManagedPositionOrigin::RecoveryBootstrap)
    );
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

    assert!(strategy.exposure.is_recovering());
}

#[test]
fn fill_after_rotation_preserves_exitable_position_book_and_subscription() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
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
    strategy
        .on_order_filled(&order_filled_event(
            entry_client_order_id,
            instrument_a,
            position_id,
        ))
        .expect("fill bookkeeping should succeed");

    assert_eq!(
        managed_position_ref(&strategy).and_then(|p| p.book.best_bid),
        original_book.best_bid
    );
    assert_eq!(
        managed_position_ref(&strategy).and_then(|p| p.lifecycle.settlement_strike()),
        Some(3_100.0)
    );
    assert_eq!(
        managed_position_ref(&strategy).and_then(|p| p.lifecycle.selection_published_at_ms()),
        Some(1_000)
    );
    assert_eq!(
        managed_position_ref(&strategy).and_then(|p| p.lifecycle.seconds_to_expiry_at_selection()),
        Some(300)
    );
    assert_eq!(
        strategy.book_subscriptions.tracked_position_instrument_id,
        Some(instrument_a)
    );
    let decision = strategy.exit_submission_decision_at(2_000);
    assert_eq!(decision.instrument_id, Some(instrument_a));
    assert_eq!(decision.order_side, Some(OrderSide::Sell));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::EntryFillMaterialized
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Managed
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

    let mut first_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    first_fill.last_qty = Quantity::new(4.0, 2);
    strategy
        .on_order_filled(&first_fill)
        .expect("first maker partial fill should be recorded");
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(4.0, 2),
        0.450,
    ));

    let mut second_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
    second_fill.last_qty = Quantity::new(6.0, 2);
    strategy
        .on_order_filled(&second_fill)
        .expect("later maker partial fill for same order should be recorded");

    assert_eq!(strategy.market_churn_count("MKT-1"), 2);
    assert_eq!(
        managed_position_ref(&strategy).map(|position| position.quantity),
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

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(
            decision.blocked_reason,
            Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING),
            "{instrument_id}"
        );
        assert_eq!(
            decision.evaluation.blocked_reason,
            Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING),
            "{instrument_id}"
        );
        assert_eq!(decision.instrument_id, None, "{instrument_id}");
        assert_eq!(decision.order_side, None, "{instrument_id}");
        assert_eq!(decision.quantity, None, "{instrument_id}");
        assert!(decision.forced_flat_reasons.is_empty(), "{instrument_id}");
    }
}

#[test]
fn forced_flat_exit_submits_despite_resting_pending_entry() {
    let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy();
        configure_limit_base_entry_order(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        strategy.active.phase = SelectionPhase::Freeze;
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let expected_exit_time_in_force = strategy.config.forced_exit_order.time_in_force;
        let expected_exit_reduce_only = strategy.config.forced_exit_order.is_reduce_only;
        materialize_managed_position_with_resting_pending_entry(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("POSITION-FORCED-WORKING-{instrument_id}").as_str()),
            position_quantity,
        );
        let expected_exit_price = strategy
            .managed_position()
            .and_then(|managed| managed.position.book.best_bid);
        let expected_quantity = strategy
            .managed_position()
            .expect("fixture should materialize managed position")
            .position
            .quantity;

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.blocked_reason, None, "{instrument_id}");
        assert_eq!(decision.evaluation.blocked_reason, None, "{instrument_id}");
        assert_eq!(
            decision.forced_flat_reasons,
            vec![ForcedFlatReason::Freeze],
            "{instrument_id}"
        );
        assert_eq!(
            decision.order_type,
            Some(OrderType::Market),
            "{instrument_id}"
        );
        assert_eq!(
            decision.time_in_force,
            Some(expected_exit_time_in_force),
            "{instrument_id}"
        );
        assert_eq!(
            decision.order_side,
            Some(OrderSide::Sell),
            "{instrument_id}"
        );
        assert_eq!(
            decision.quantity,
            Some(expected_quantity),
            "{instrument_id}"
        );
        assert_eq!(decision.price, expected_exit_price, "{instrument_id}");
        assert_eq!(decision.is_post_only, Some(false), "{instrument_id}");
        assert_eq!(
            decision.is_reduce_only,
            Some(expected_exit_reduce_only),
            "{instrument_id}"
        );
    }
}

#[test]
fn forced_flat_submit_cancels_resting_entry_and_recovers_if_entry_fill_races() {
    let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
    for instrument_id in configured_instruments {
        let submit_admission = submit_admission_with_provider_cap(
            Decimal::new(10_000, 0),
            Arc::new(RecordingDecisionEvidenceWriter),
        );
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission,
        );
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
        cache
            .borrow_mut()
            .add_order(
                entry_order,
                None,
                Some(ClientId::from(strategy.config.client_id.as_str())),
                true,
            )
            .expect("test cache should accept resting entry order");

        let exit_client_order_id = strategy
            .try_submit_exit_order_for_trigger(
                1_200,
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    1_200,
                )),
            )
            .expect("forced-flat exit submit should not fail")
            .expect("forced-flat exit should submit");

        let exec_messages = exec_messages.get_messages();
        assert!(
            exec_messages.iter().any(|message| matches!(
                message,
                TradingCommand::CancelOrder(command)
                    if command.client_order_id == entry_client_order_id
            )),
            "forced-flat submit should cancel the resting entry before relying on exit: {instrument_id}"
        );
        let risk_messages = risk_messages.get_messages();
        assert!(
            risk_messages.iter().any(|message| matches!(
                message,
                TradingCommand::SubmitOrder(command)
                    if command.client_order_id == exit_client_order_id
            )),
            "forced-flat exit should still submit after the entry cancel request: {instrument_id}"
        );

        strategy
            .on_order_filled(&order_filled_event(
                entry_client_order_id,
                instrument_id,
                position_id,
            ))
            .expect("racing entry fill should be handled while exit is pending");
        strategy
            .on_order_filled(&order_filled_event_with_details(
                exit_client_order_id,
                instrument_id,
                Some(position_id),
                OrderSide::Sell,
            ))
            .expect("exit fill should be handled");
        strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

        assert!(
            strategy.managed_position().is_some(),
            "entry remainder fill racing the first forced-flat exit should recover to managed residual exposure: {instrument_id}"
        );
        assert!(
            strategy.exposure.exit_pending().is_none(),
            "terminal forced-flat exit with residual exposure must not stay exit-pending forever: {instrument_id}"
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

    strategy
        .on_order_filled(&order_filled_event(
            entry_client_order_id,
            instrument_id,
            position_id,
        ))
        .expect("IOC entry fill should materialize a managed position");

    assert_eq!(
        strategy
            .managed_position()
            .and_then(|managed| managed.pending_entry.as_ref()),
        None
    );
    assert_eq!(
        strategy.exit_submission_decision_at(1_200).blocked_reason,
        Some(EXIT_BLOCK_REASON_EXIT_HOLD)
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

    strategy
        .on_order_filled(&order_filled_event_with_details(
            entry_client_order_id,
            instrument_id,
            None,
            OrderSide::Buy,
        ))
        .expect("fill without position id should not wedge");

    assert!(strategy.exposure.is_recovering());
    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(strategy.market_in_cooldown("MKT-1", 1_000));

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        PositionId::from("P-LATE"),
        Quantity::new(10.0, 2),
        0.450,
    ));

    assert!(strategy.managed_position().is_some());
    assert_eq!(
        managed_position_ref(&strategy).map(|position| position.position_id),
        Some(PositionId::from("P-LATE"))
    );
    assert_eq!(
        managed_position_ref(&strategy).and_then(|position| position.lifecycle.market_id()),
        Some("MKT-1")
    );
    assert_eq!(
        managed_position_ref(&strategy).map(|position| position.book.clone()),
        Some(original_book)
    );
    assert_eq!(
        strategy.managed_position().map(|managed| managed.origin),
        Some(ManagedPositionOrigin::StrategyEntry)
    );
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn late_zero_fill_entry_terminal_events_resolve_entry_reconcile_to_flat() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut canceled = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut canceled);
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-CANCEL");
    let canceled_pending = pending_entry_state(&mut canceled, entry_client_order_id);
    let canceled_instrument_id = canceled_pending.instrument_id;
    set_entry_reconcile_pending(
        &mut canceled,
        canceled_pending,
        EntryReconcileReason::UnresolvedAtSelectionBoundary,
    );
    canceled
        .on_order_canceled(&order_canceled_event(
            entry_client_order_id,
            canceled_instrument_id,
        ))
        .expect("zero-fill cancel should resolve reconcile state");
    assert!(matches!(canceled.exposure, ExposureState::Flat));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderCanceled
                    && record.client_order_id.as_deref() == Some("ENTRY-ZERO-FILL-CANCEL")
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
        )),
        "zero-fill cancel must record a Flat terminal lifecycle outcome"
    );

    let mut rejected = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-REJECT");
    let rejected_pending = pending_entry_state(&mut rejected, entry_client_order_id);
    let rejected_instrument_id = rejected_pending.instrument_id;
    set_entry_reconcile_pending_with_observed_fill(
        &mut rejected,
        rejected_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
        Quantity::new(1.0, 2),
    );
    rejected.on_order_rejected(order_rejected_event(
        entry_client_order_id,
        rejected_instrument_id,
    ));
    assert!(matches!(rejected.exposure, ExposureState::Flat));

    let mut denied = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-DENIED");
    let denied_pending = pending_entry_state(&mut denied, entry_client_order_id);
    let denied_instrument_id = denied_pending.instrument_id;
    set_entry_reconcile_pending_with_observed_fill(
        &mut denied,
        denied_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
        Quantity::new(1.0, 2),
    );
    denied.on_order_denied(order_denied_event_with_reason(
        entry_client_order_id,
        denied_instrument_id,
        "DENIED",
    ));
    assert!(matches!(denied.exposure, ExposureState::Flat));

    let mut expired = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-EXPIRE");
    let expired_pending = pending_entry_state(&mut expired, entry_client_order_id);
    let expired_instrument_id = expired_pending.instrument_id;
    set_entry_reconcile_pending(
        &mut expired,
        expired_pending,
        EntryReconcileReason::UnresolvedAtSelectionBoundary,
    );
    expired.on_order_expired(order_expired_event(
        entry_client_order_id,
        expired_instrument_id,
    ));
    assert!(matches!(expired.exposure, ExposureState::Flat));
}

#[test]
fn runtime_reconcile_queries_pending_entry_order_from_nt_cache() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let entry_client_order_id = ClientOrderId::from("ENTRY-RECONCILE-QUERY");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let order =
        configured_entry_order_for_reconcile(&mut strategy, instrument_id, entry_client_order_id);
    cache
        .borrow_mut()
        .add_order(
            order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept open entry order");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);

    assert!(
        strategy.runtime_reconcile_query_events.iter().any(|event| {
            event.client_order_id == entry_client_order_id && event.instrument_id == instrument_id
        }),
        "selection retry timer must dispatch an NT order query for unresolved pending entry"
    );
    assert!(matches!(strategy.exposure, ExposureState::PendingEntry(_)));
}

#[test]
fn runtime_reconcile_waits_until_pending_entry_order_is_stale() {
    let mut strategy = ready_to_trade_strategy();
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let entry_client_order_id = ClientOrderId::from("ENTRY-RECONCILE-YOUNG");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let order =
        configured_entry_order_for_reconcile(&mut strategy, instrument_id, entry_client_order_id);
    cache
        .borrow_mut()
        .add_order(
            order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept open entry order");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms.saturating_sub(1));

    assert!(
        strategy.runtime_reconcile_query_events.is_empty(),
        "young pending entry must wait for the configured retry interval before venue reconcile"
    );
    assert!(matches!(strategy.exposure, ExposureState::PendingEntry(_)));
}

#[test]
fn runtime_reconcile_canceled_pending_entry_flattens_with_reconcile_source() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let entry_client_order_id = ClientOrderId::from("ENTRY-RECONCILE-CANCELED");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let mut order =
        configured_entry_order_for_reconcile(&mut strategy, instrument_id, entry_client_order_id);
    close_order_with_canceled_event(&mut order);
    cache
        .borrow_mut()
        .add_order(
            order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept closed entry order");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);

    assert!(matches!(strategy.exposure, ExposureState::Flat));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderCanceled
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
                    && record.source == ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS
                    && record.client_order_id.as_deref() == Some("ENTRY-RECONCILE-CANCELED")
        )),
        "terminal cache truth must flatten pending entry with reconcile_pass source"
    );
}

#[test]
fn runtime_reconcile_cached_position_materializes_managed_with_reconcile_source() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let entry_client_order_id = ClientOrderId::from("ENTRY-RECONCILE-FILLED");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );
    let position_id = PositionId::from("P-RECONCILE-FILLED");
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("active instrument should be cached for reconcile test");
    let position = Position::new(
        &instrument,
        order_filled_event(entry_client_order_id, instrument_id, position_id),
    );
    cache
        .borrow_mut()
        .add_position(&position, NtOmsType::Netting)
        .expect("test cache should accept filled position");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);

    assert!(matches!(
        strategy.exposure,
        ExposureState::Managed(ManagedPositionState {
            position: OpenPositionState { position_id: managed_position_id, .. },
            ..
        }) if managed_position_id == position_id
    ));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::EntryFillMaterialized
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Managed
                    && record.source == ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS
                    && record.client_order_id.as_deref() == Some("ENTRY-RECONCILE-FILLED")
                    && record.position_id.as_deref() == Some("P-RECONCILE-FILLED")
        )),
        "cached position truth must materialize managed exposure with reconcile_pass evidence"
    );
}

#[test]
fn runtime_reconcile_filled_exit_terminal_frees_slot_with_reconcile_source() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-RECONCILE-FILLED"),
        Quantity::new(10.0, 2),
        0.450,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-RECONCILE-FILLED");
    set_exit_pending(
        &mut strategy,
        position.clone(),
        exit_client_order_id,
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    let order_config = strategy
        .normal_exit_order_execution_config()
        .expect("test config should build exit order config");
    let exit_order_side = strategy
        .configured_position_contract()
        .expect("test config should carry position contract")
        .exit_order_side;
    let mut order = strategy
        .build_exit_order_with_execution_config(
            order_config,
            instrument_id,
            exit_order_side,
            position.quantity,
            Price::new(0.45, 2),
            exit_client_order_id,
        )
        .expect("configured exit order should build for reconcile test");
    close_order_with_filled_event(&mut order, position.position_id);
    cache
        .borrow_mut()
        .add_order(
            order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept filled exit order");
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("active instrument should be cached for reconcile test");
    let mut cached_position = Position::new(
        &instrument,
        order_filled_event(
            ClientOrderId::from("ENTRY-EXIT-RECONCILE-FILLED"),
            instrument_id,
            position.position_id,
        ),
    );
    let mut close_fill = order_filled_event_with_details(
        exit_client_order_id,
        instrument_id,
        Some(position.position_id),
        exit_order_side,
    );
    close_fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-EXIT-RECONCILE-FILLED");
    close_fill.last_qty = position.quantity;
    cached_position.apply(&close_fill);
    assert!(cached_position.is_closed());
    cache
        .borrow_mut()
        .add_position(&cached_position, NtOmsType::Netting)
        .expect("test cache should accept closed reconcile position");
    cache
        .borrow_mut()
        .update_position(&cached_position)
        .expect("test cache should index closed reconcile position");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);

    assert!(matches!(strategy.exposure, ExposureState::Flat));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderFilled
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
                    && record.source == ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS
                    && record.client_order_id.as_deref() == Some("EXIT-RECONCILE-FILLED")
        )),
        "filled exit terminal cache truth must free the slot with reconcile_pass evidence"
    );
}

#[test]
fn runtime_reconcile_filled_exit_terminal_waits_without_closed_position_cache() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-RECONCILE-NO-CLOSE"),
        Quantity::new(10.0, 2),
        0.450,
    );
    let exit_client_order_id = ClientOrderId::from("EXIT-RECONCILE-NO-CLOSE");
    set_exit_pending(
        &mut strategy,
        position.clone(),
        exit_client_order_id,
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    let order_config = strategy
        .normal_exit_order_execution_config()
        .expect("test config should build exit order config");
    let exit_order_side = strategy
        .configured_position_contract()
        .expect("test config should carry position contract")
        .exit_order_side;
    let mut order = strategy
        .build_exit_order_with_execution_config(
            order_config,
            instrument_id,
            exit_order_side,
            position.quantity,
            Price::new(0.45, 2),
            exit_client_order_id,
        )
        .expect("configured exit order should build for reconcile test");
    close_order_with_filled_event(&mut order, position.position_id);
    cache
        .borrow_mut()
        .add_order(
            order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept filled exit order");

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);

    assert!(matches!(
        strategy.exposure,
        ExposureState::ExitPending(ExitPendingState {
            pending_exit: PendingExitState {
                fill_received: true,
                close_received: false,
                ..
            },
            ..
        })
    ));
}

#[test]
fn runtime_reconcile_query_failure_writes_evidence_and_retries_without_state_change() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let entry_client_order_id = ClientOrderId::from("ENTRY-RECONCILE-MISSING-ORDER");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    let reconcile_due_time_ms = reconcile_due_time_ms(&strategy, 1_000);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms);
    strategy.apply_runtime_venue_reconcile(reconcile_due_time_ms.saturating_add(1));

    assert!(matches!(
        strategy.exposure,
        ExposureState::PendingEntry(PendingEntryState { client_order_id, .. })
            if client_order_id == entry_client_order_id
    ));
    let instrument_id_text = instrument_id.to_string();
    let failures = evidence
        .events()
        .into_iter()
        .filter(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::ReconcileQueryFailed
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::PendingEntry
                    && record.source == ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS
                    && record.instrument_id.as_deref() == Some(instrument_id_text.as_str())
                    && record.client_order_id.as_deref() == Some("ENTRY-RECONCILE-MISSING-ORDER")
        ))
        .count();
    assert_eq!(
        failures, 2,
        "failed reconcile query should write loud evidence and retry without mutating exposure"
    );
}

#[test]
fn runtime_reconcile_closed_unsupported_observed_position_flattens_with_reconcile_source() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let instrument_id = selected_entry_instrument(&strategy);
    let observed = configured_position_probe(&mut strategy, instrument_id);
    let observed_position_id = observed.position_id.to_string();
    set_unsupported_observed(
        &mut strategy,
        observed.clone(),
        UnsupportedObservedReason::LiveUnsupportedContract,
    );
    let instrument = strategy
        .current_instrument(instrument_id)
        .expect("active instrument should be cached for reconcile test");
    let mut entry_fill = order_filled_event_with_details(
        ClientOrderId::from("ENTRY-UNSUPPORTED-RECONCILE-CLOSED"),
        instrument_id,
        Some(observed.position_id),
        observed.entry_order_side,
    );
    entry_fill.last_qty = observed.quantity;
    let mut cached_position = Position::new(&instrument, entry_fill);
    let exit_order_side = match observed.side {
        PositionSide::Long => OrderSide::Sell,
        PositionSide::Short => OrderSide::Buy,
        PositionSide::Flat | PositionSide::NoPositionSide => observed.entry_order_side,
    };
    let mut close_fill = order_filled_event_with_details(
        ClientOrderId::from("EXIT-UNSUPPORTED-RECONCILE-CLOSED"),
        instrument_id,
        Some(observed.position_id),
        exit_order_side,
    );
    close_fill.trade_id =
        nautilus_model::identifiers::TradeId::from("TRADE-UNSUPPORTED-RECONCILE-CLOSED");
    close_fill.last_qty = observed.quantity;
    cached_position.apply(&close_fill);
    assert!(cached_position.is_closed());
    cache
        .borrow_mut()
        .add_position(&cached_position, NtOmsType::Netting)
        .expect("test cache should accept closed unsupported position");
    cache
        .borrow_mut()
        .update_position(&cached_position)
        .expect("test cache should index closed unsupported position");

    emit_selection_retry_time_event(&mut strategy, 1_205);

    assert!(matches!(strategy.exposure, ExposureState::Flat));
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::PositionClosed
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
                    && record.source == ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS
                    && record.position_id.as_deref() == Some(observed_position_id.as_str())
        )),
        "closed unsupported observed position must flatten with reconcile_pass source"
    );
}

#[test]
fn runtime_reconcile_unsupported_observed_position_waits_without_closed_position_cache() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let cache = register_test_strategy(&mut strategy);
    add_active_instruments_to_cache(&strategy, &cache);
    let instrument_id = selected_entry_instrument(&strategy);
    let observed = configured_position_probe(&mut strategy, instrument_id);
    let observed_position_id = observed.position_id;
    set_unsupported_observed(
        &mut strategy,
        observed,
        UnsupportedObservedReason::LiveUnsupportedContract,
    );

    emit_selection_retry_time_event(&mut strategy, 1_205);

    assert!(matches!(
        strategy.exposure,
        ExposureState::UnsupportedObserved(UnsupportedObservedState {
            observed: OpenPositionState { position_id, .. },
            ..
        }) if position_id == observed_position_id
    ));
}

#[test]
fn late_fill_observed_entry_cancel_or_expire_preserves_entry_reconcile_fail_closed_state() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());

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
    set_entry_reconcile_pending_with_observed_fill(
        &mut canceled,
        canceled_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
        Quantity::new(2.0, 2),
    );
    canceled
        .on_order_canceled(&order_canceled_event(
            entry_client_order_id,
            canceled_instrument_id,
        ))
        .expect("fill-observed cancel should preserve fail-closed reconcile state");
    assert!(matches!(
        canceled.exposure,
        ExposureState::EntryReconcilePending {
            observed_fill_quantity: Some(quantity),
            ..
        } if quantity == Quantity::new(2.0, 2)
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
    set_entry_reconcile_pending_with_observed_fill(
        &mut expired,
        expired_pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
        Quantity::new(3.0, 2),
    );
    expired.on_order_expired(order_expired_event(
        entry_client_order_id,
        expired_instrument_id,
    ));
    assert!(matches!(
        expired.exposure,
        ExposureState::EntryReconcilePending {
            observed_fill_quantity: Some(quantity),
            ..
        } if quantity == Quantity::new(3.0, 2)
    ));

    let events = evidence.events();
    assert!(
        events.iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderCanceled
                    && record.raw_reason_text.as_deref()
                        == Some(ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON)
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
        )),
        "fill-observed cancel must record preserved fail-closed lifecycle evidence"
    );
    assert!(
        events.into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderExpired
                    && record.raw_reason_text.as_deref()
                        == Some(ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON)
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
        )),
        "fill-observed expiry must record preserved fail-closed lifecycle evidence"
    );
}

#[test]
fn malformed_entry_reject_stops_same_instrument_entry_decisions() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-MALFORMED-AMOUNTS");
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure = ExposureState::PendingEntry(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "invalid order amounts: maker amount exceeds allowed decimal precision",
    ));

    let decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(decision.blocked_reason, Some("entry_malformed_rejected"));
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn order_denied_clears_matching_pending_entry_and_records_lifecycle_evidence() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
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
    strategy.exposure = ExposureState::PendingEntry(pending);

    strategy.on_order_denied(order_denied_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "RATE_LIMIT_EXCEEDED",
    ));

    assert!(strategy.pending_entry().is_none());
    assert_eq!(
        strategy.entry_submission_decision_at(1_200).blocked_reason,
        Some("entry_unfillable_rejected_unchanged_book"),
        "a local denial must not fall through to immediate resubmit"
    );
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::OrderDenied
                    && record.client_order_id.as_deref() == Some("ENTRY-DENIED")
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
        )),
        "denial handling must write distinguishable lifecycle evidence"
    );
}

#[test]
fn selection_rotation_reclassifies_unresolved_pending_entry_and_records_lifecycle_evidence() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
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
    strategy.exposure = ExposureState::PendingEntry(pending);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-NEXT", 2_000));

    assert!(matches!(
        strategy.exposure,
        ExposureState::EntryReconcilePending {
            pending,
            reason: EntryReconcileReason::UnresolvedAtSelectionBoundary,
            observed_fill_quantity: None,
        } if pending.instrument_id == instrument_id
    ));
    let instrument_id_text = instrument_id.to_string();
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::BoundaryReclassification
                    && record.client_order_id.as_deref() == Some("ENTRY-BOUNDARY-NO-TERMINAL")
                    && record.instrument_id.as_deref() == Some(instrument_id_text.as_str())
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
        )),
        "selection-boundary recovery must write distinguishable lifecycle evidence"
    );
}

#[test]
fn unfillable_fok_entry_reject_waits_for_book_change_before_redeciding() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-FOK-NO-MATCH");
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let rejected_book = pending.book.clone();
    strategy.exposure = ExposureState::PendingEntry(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "FOK order could not be matched against the current book",
    ));

    let unchanged_book_decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        unchanged_book_decision.blocked_reason,
        Some("entry_unfillable_rejected_unchanged_book")
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
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    strategy.exposure = ExposureState::PendingEntry(pending);
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
    assert_eq!(decision.blocked_reason, Some("entry_balance_rejected"));
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    let changed_book_decision = strategy.entry_submission_decision_at(1_201);
    assert_eq!(
        changed_book_decision.blocked_reason,
        Some("entry_balance_rejected")
    );
}

#[test]
fn unknown_entry_reject_waits_for_book_change_before_redeciding() {
    let entry_client_order_id = ClientOrderId::from("ENTRY-UNKNOWN-REJECTED");
    let mut strategy = ready_to_trade_strategy();
    strategy.config.entry_order.order_type = OrderType::Market;
    strategy.config.entry_order.time_in_force = TimeInForce::Fok;
    strategy.config.entry_order.is_quote_quantity = true;
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    let rejected_book = pending.book.clone();
    strategy.exposure = ExposureState::PendingEntry(pending);

    strategy.on_order_rejected(order_rejected_event_with_reason(
        entry_client_order_id,
        instrument_id,
        "venue rejected entry for an unmodeled reason",
    ));

    let unchanged_book_decision = strategy.entry_submission_decision_at(1_200);
    assert_eq!(
        unchanged_book_decision.blocked_reason,
        Some("entry_unfillable_rejected_unchanged_book")
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
    let mut strategy = ready_to_trade_strategy();
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
    assert!(matches!(
        strategy.exposure,
        ExposureState::EntryReconcilePending { .. }
    ));
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
}

#[test]
fn position_closed_releases_entry_reconcile_pending_for_same_instrument() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSED-BEFORE-OPEN");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_entry_reconcile_pending(
        &mut strategy,
        pending,
        EntryReconcileReason::AwaitingPositionMaterialization,
    );

    strategy.on_position_closed(position_closed_event(
        instrument_id,
        PositionId::from("P-CLOSED-BEFORE-OPEN"),
    ));

    assert!(matches!(strategy.exposure, ExposureState::Flat));
    assert!(strategy.pending_entry().is_none());
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::PositionClosed
                    && record.client_order_id.as_deref() == Some("ENTRY-CLOSED-BEFORE-OPEN")
                    && record.position_id.as_deref() == Some("P-CLOSED-BEFORE-OPEN")
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Flat
        )),
        "position-closed release must write lifecycle evidence"
    );
}

#[test]
fn position_closed_cancels_managed_resting_pending_entry_and_keeps_context() {
    let mut strategy = ready_to_trade_strategy();
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
    cache
        .borrow_mut()
        .add_order(
            entry_order,
            None,
            Some(ClientId::from(strategy.config.client_id.as_str())),
            true,
        )
        .expect("test cache should accept resting entry order");

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
    assert!(matches!(
        strategy.exposure,
        ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            ..
        }) if client_order_id == entry_client_order_id
    ));
    assert!(strategy.pending_entry().is_some());

    strategy
        .on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id))
        .expect("entry cancel should clear retained pending-entry context");
    assert!(matches!(strategy.exposure, ExposureState::Flat));
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn forced_flat_exit_in_shadow_mode_suppresses_resting_entry_cancel() {
    let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
    for instrument_id in configured_instruments {
        let submit_admission = submit_admission_with_provider_cap(
            Decimal::new(10_000, 0),
            Arc::new(RecordingDecisionEvidenceWriter),
        );
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission,
        );
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
    let mut strategy = ready_to_trade_strategy();
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
    assert!(matches!(
        strategy.exposure,
        ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            ..
        }) if client_order_id == entry_client_order_id
    ));
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

    assert!(matches!(
        strategy.exposure,
        ExposureState::EntryReconcilePending { .. }
    ));
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
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
        UnsupportedObservedReason::BootstrappedUnsupportedContract,
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
        UnsupportedObservedReason::BootstrappedUnsupportedContract,
    );

    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(matches!(strategy.exposure, ExposureState::Flat));
}

#[test]
fn sell_fill_enters_recovery_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy
        .on_order_filled(&order_filled_event_with_details(
            entry_client_order_id,
            instrument_id,
            Some(PositionId::from("P-SHORT")),
            OrderSide::Sell,
        ))
        .expect("sell fill should fail closed into recovery");

    assert!(strategy.exposure.is_recovering());
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
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());

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

    awaiting
        .on_order_filled(&fill)
        .expect("entry fill without position id should enter reconcile state");

    assert!(matches!(
        awaiting.exposure,
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::AwaitingPositionMaterialization,
            observed_fill_quantity: Some(quantity),
            ..
        } if quantity == Quantity::new(2.0, 2)
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

    unsupported
        .on_order_filled(&fill)
        .expect("entry fill with unsupported side should enter reconcile state");

    assert!(matches!(
        unsupported.exposure,
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::UnsupportedEntryFillSide {
                order_side: OrderSide::Sell,
            },
            observed_fill_quantity: Some(quantity),
            ..
        } if quantity == Quantity::new(3.0, 2)
    ));

    let events = evidence.events();
    assert!(
        events.iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::EntryReconcilePending
                    && record.client_order_id.as_deref() == Some("ENTRY-FILL-AWAITING-POSITION")
                    && record.position_id.is_none()
                    && record.filled_quantity.is_some()
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
        )),
        "awaiting-position entry fill must write lifecycle evidence"
    );
    assert!(
        events.into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::EntryReconcilePending
                    && record.client_order_id.as_deref() == Some("ENTRY-FILL-UNSUPPORTED-SIDE")
                    && record.position_id.as_deref() == Some("P-FILL-UNSUPPORTED-SIDE")
                    && record.order_side.as_deref() == Some("Sell")
                    && record.filled_quantity.is_some()
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
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

    strategy
        .on_order_filled(&order_filled_event_with_details(
            entry_client_order_id,
            fill_instrument_id,
            Some(PositionId::from("P-MISMATCHED-FILL")),
            OrderSide::Sell,
        ))
        .expect("unsupported mismatched fill should fail closed");

    let ExposureState::BlindRecovery(recovery) = &strategy.exposure else {
        panic!("expected blind recovery, got {:?}", strategy.exposure);
    };
    assert_eq!(
        recovery.reason,
        BlindRecoveryReason::InvalidLivePosition {
            entry_order_side: OrderSide::Sell,
            side: None,
        }
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn pending_entry_short_position_event_stays_fail_closed_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy.on_position_opened(position_opened_event_with_details(
        instrument_id,
        PositionId::from("P-SHORT"),
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Sell,
        PositionSide::Short,
    ));

    assert!(strategy.exposure.is_recovering());
    assert!(strategy.managed_position().is_none());
    let quarantined = match &strategy.exposure {
        ExposureState::UnsupportedObserved(state) => state,
        other => panic!("expected unsupported observed exposure, got {other:?}"),
    };
    assert_eq!(quarantined.observed.instrument_id, instrument_id);
    assert_eq!(
        quarantined.observed.position_id,
        PositionId::from("P-SHORT")
    );
    assert_eq!(quarantined.observed.entry_order_side, OrderSide::Sell);
    assert_eq!(quarantined.observed.side, PositionSide::Short);
    assert!(strategy.pending_entry().is_none());
}

#[test]
fn live_position_event_quarantines_foreign_venue_position() {
    // P5-5 / Codex P5 — LIVE-PATH regression lock (mirror of
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
            strategy.exposure,
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition { .. }
            })
        ),
        "foreign-venue live position event must be quarantined to blind recovery, got {:?}",
        strategy.exposure,
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn order_fill_entry_quarantines_foreign_venue_position() {
    // P5-5 / Codex P5 — LIVE ORDER-FILL regression lock (sibling of
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

    strategy
        .on_order_filled(&order_filled_event_with_details(
            entry_client_order_id,
            foreign_instrument_id,
            Some(PositionId::from("P-FOREIGN-FILL")),
            OrderSide::Buy,
        ))
        .expect("foreign-venue entry fill must not wedge the strategy");

    // Observable exposure: quarantined to blind recovery, never adopted into Managed.
    assert!(
        matches!(
            strategy.exposure,
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition { .. }
            })
        ),
        "foreign-venue entry fill must be quarantined to blind recovery, got {:?}",
        strategy.exposure,
    );
    assert!(strategy.managed_position().is_none());
}

#[test]
fn pending_entry_unknown_position_side_stays_fail_closed_without_materializing_position() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
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

    assert!(strategy.exposure.is_recovering());
    assert!(strategy.managed_position().is_none());
    assert_eq!(
        strategy
            .pending_entry()
            .map(|pending| pending.client_order_id),
        Some(entry_client_order_id)
    );
    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::EntryReconcilePending
                    && record.source == ORDER_LIFECYCLE_SOURCE_POSITION_EVENT
                    && record.client_order_id.as_deref() == Some("ENTRY-BAD-SIDE")
                    && record.position_id.as_deref() == Some("P-BAD-SIDE")
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::EntryReconcilePending
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
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.active.interval_open = Some(3_200.0);
    strategy.on_position_opened(position_opened_event(
        instrument_a,
        PositionId::from("P-A"),
        Quantity::new(10.0, 2),
        0.450,
    ));

    let open_position = managed_position_ref(&strategy)
        .cloned()
        .expect("position should remain tracked");
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
fn recovery_bootstrap_quarantines_foreign_venue_position() {
    // P5-5 / Codex P5 — RECOVERY-PATH regression lock. The entry path is venue-scoped, but
    // recovery bootstrap previously adopted any-venue cached positions, and the exit path would
    // then build/submit a real order on the foreign-venue instrument. `bootstrapped_exposure_for`
    // is the single fail-closed adoption decision and must quarantine a foreign-venue position
    // BEFORE the contract check. This test holds the venue as the ONLY difference between a managed
    // and a quarantined position, proving the venue guard is what diverts it.
    let mut strategy = ready_to_trade_strategy();
    let execution_venue = fixture_execution_venue();
    let instrument_id = configured_outcome_instruments(&strategy)
        .into_iter()
        .next()
        .expect("ready-to-trade fixture should expose a configured instrument");
    let supported = configured_position_probe(&mut strategy, instrument_id);
    assert_eq!(
        supported.instrument_id.venue, execution_venue,
        "probe should produce an execution-venue position",
    );

    // Control: an execution-venue, supported-side position is adopted into Managed.
    let managed = strategy.bootstrapped_exposure_for(supported.clone(), execution_venue);
    assert!(
        matches!(managed, ExposureState::Managed(_)),
        "execution-venue supported position must be managed, got {managed:?}",
    );

    // Same position on a foreign venue (only the venue differs) must be quarantined, never managed.
    let foreign_instrument =
        InstrumentId::new(supported.instrument_id.symbol, Venue::from("HYPERLIQUID"));
    let foreign = OpenPositionState {
        instrument_id: foreign_instrument,
        book: OutcomeBookState::from_instrument_id(foreign_instrument),
        ..supported.clone()
    };
    let quarantined = strategy.bootstrapped_exposure_for(foreign, execution_venue);
    assert!(
        matches!(
            quarantined,
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition { .. }
            })
        ),
        "foreign-venue position must be quarantined to blind recovery, got {quarantined:?}",
    );
}

#[test]
fn bootstrap_recovery_from_cache_ignores_foreign_venue_position() {
    // P5-5 / Codex P5 — RECOVERY-PATH regression lock. The entry path scopes selection to the
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
        matches!(strategy.exposure, ExposureState::Flat),
        "a foreign-venue cached position must NOT be recovered into Managed state: got {:?}",
        strategy.exposure,
    );
}

#[test]
fn bootstrap_recovery_from_cache_loads_execution_venue_position() {
    // Baseline: an execution-venue position matching the strategy contract IS recovered into
    // Managed state. This ensures the venue filter does not over-reject.
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

    let managed = strategy
        .managed_position()
        .expect("execution-venue position must be recovered into Managed state");
    assert_eq!(
        managed.position.instrument_id.to_string(),
        "token-up.POLYMARKET",
        "recovered position must be the execution-venue instrument",
    );
    assert_eq!(
        managed.position.position_id.to_string(),
        "POS-EXEC-001",
        "recovered position must carry the correct position id",
    );
    assert!(
        matches!(managed.origin, ManagedPositionOrigin::RecoveryBootstrap),
        "recovered position must carry RecoveryBootstrap origin",
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
fn task5_one_position_invariant_panics_in_debug_or_rejects_in_release() {
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        strategy.enforce_one_position_invariant()
    }));

    if cfg!(debug_assertions) {
        assert!(result.is_err());
    } else {
        assert!(result.expect("release builds should not panic").is_err());
    }
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
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
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

    strategy.exposure = ExposureState::Flat;
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
    let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
    for instrument_id in configured_instruments {
        let mut strategy = ready_to_trade_strategy();
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
        let decision = strategy.exit_submission_decision_at(1_200);
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
    let mut strategy =
        test_strategy_with_economics_source(RecordingEconomicsAdmissionSource::cold());
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
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.active.interval_open = Some(3_200.0);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_101.0, 2_000)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 2.5, 2_000);

    let decision = strategy.exit_submission_decision_at(2_000);
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
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
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
        ManagedPositionOrigin::RecoveryBootstrap,
    );

    let decision = strategy.exit_submission_decision_at(2_000);

    assert_eq!(decision.evaluation.exit_decision, None);
    assert_eq!(decision.instrument_id, None);
    assert_eq!(decision.order_side, None);
    assert_eq!(decision.price, None);
    assert_eq!(decision.quantity, None);
    assert_eq!(
        decision.blocked_reason,
        Some(EXIT_BLOCK_REASON_POSITION_INTERVAL_UNKNOWN)
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
    let exposure = ExposureState::EntryReconcilePending {
        pending: pending.clone(),
        reason: EntryReconcileReason::AwaitingPositionMaterialization,
        observed_fill_quantity: None,
    };

    assert_eq!(exposure.pending_entry(), Some(&pending));
    assert_eq!(
        exposure.occupancy(),
        Some(ExposureOccupancy::EntryReconcilePending)
    );
    assert!(exposure.blocks_new_entries());
}

#[test]
fn exposure_exit_pending_requires_both_fill_and_close_to_become_flat() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let managed = ManagedPositionState {
        position: OpenPositionState {
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
            position_id: PositionId::from("P-EXIT-STATE-001"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            book: strategy.active.books.up.clone(),
        },
        origin: ManagedPositionOrigin::StrategyEntry,
        pending_entry: None,
    };
    let mut exit_pending = ExitPendingState {
        position: Some(managed.clone()),
        pending_exit: PendingExitState {
            client_order_id: ClientOrderId::from("EXIT-STATE-001"),
            submitted_at_ms: Some(1_000),
            market_id: Some("MKT-1".to_string()),
            position_id: Some(PositionId::from("P-EXIT-STATE-001")),
            fill_received: false,
            filled_quantity: None,
            close_received: false,
            terminal_received: false,
            residual_position_observed_after_fill: false,
        },
    };

    assert!(!exit_pending.is_terminal());
    exit_pending.pending_exit.fill_received = true;
    assert!(!exit_pending.is_terminal());
    exit_pending.pending_exit.close_received = true;
    assert!(exit_pending.is_terminal());
    assert_eq!(
        exit_pending
            .position
            .as_ref()
            .map(|state| state.position.position_id),
        Some(PositionId::from("P-EXIT-STATE-001"))
    );
}

#[test]
fn residual_position_after_terminal_preserves_fill_precision() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let managed = ManagedPositionState {
        position: OpenPositionState {
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
            position_id: PositionId::from("P-EXIT-PRECISION-001"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            book: strategy.active.books.up.clone(),
        },
        origin: ManagedPositionOrigin::StrategyEntry,
        pending_entry: None,
    };
    let exit_pending = ExitPendingState {
        position: Some(managed),
        pending_exit: PendingExitState {
            client_order_id: ClientOrderId::from("EXIT-PRECISION-001"),
            submitted_at_ms: Some(1_000),
            market_id: Some("MKT-1".to_string()),
            position_id: Some(PositionId::from("P-EXIT-PRECISION-001")),
            fill_received: true,
            filled_quantity: Some(Quantity::new(4.1234, 4)),
            close_received: false,
            terminal_received: true,
            residual_position_observed_after_fill: false,
        },
    };

    let residual = exit_pending
        .residual_position_after_terminal()
        .expect("positive residual quantity should be returned");

    assert_eq!(residual.quantity, Quantity::new(5.8766, 4));
    assert_eq!(residual.quantity.precision, 4);
}

#[test]
fn residual_position_after_terminal_uses_observed_position_after_fill() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-OBSERVED-RESIDUAL-001"),
        Quantity::new(7.0, 2),
        0.450,
    );
    let exit_pending = ExitPendingState {
        position: Some(ManagedPositionState {
            position: open_position.clone(),
            origin: ManagedPositionOrigin::StrategyEntry,
            pending_entry: None,
        }),
        pending_exit: PendingExitState {
            client_order_id: ClientOrderId::from("EXIT-OBSERVED-RESIDUAL-001"),
            submitted_at_ms: Some(1_000),
            market_id: open_position.lifecycle.market_id_owned(),
            position_id: Some(open_position.position_id),
            fill_received: true,
            filled_quantity: Some(Quantity::new(4.0, 2)),
            close_received: false,
            terminal_received: true,
            residual_position_observed_after_fill: true,
        },
    };

    let residual = exit_pending
        .residual_position_after_terminal()
        .expect("observed residual position should be authoritative");

    assert_eq!(residual.position_id, open_position.position_id);
    assert_eq!(residual.quantity, Quantity::new(7.0, 2));
}

#[test]
fn exposure_exit_pending_terminal_with_residual_position_restores_managed_state() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-RESIDUAL-001"),
        Quantity::new(10.0, 2),
        0.450,
    );
    let exit_pending = ExitPendingState {
        position: Some(ManagedPositionState {
            position: open_position.clone(),
            origin: ManagedPositionOrigin::StrategyEntry,
            pending_entry: None,
        }),
        pending_exit: PendingExitState {
            client_order_id: ClientOrderId::from("EXIT-RESIDUAL-001"),
            submitted_at_ms: Some(1_000),
            market_id: open_position.lifecycle.market_id_owned(),
            position_id: Some(open_position.position_id),
            fill_received: true,
            filled_quantity: Some(Quantity::new(4.0, 2)),
            close_received: false,
            terminal_received: true,
            residual_position_observed_after_fill: true,
        },
    };

    let state = exit_pending.into_state_after_exit_update();

    let ExposureState::Managed(restored) = state else {
        panic!("terminal residual position must restore managed exposure");
    };
    assert_eq!(restored.position.position_id, open_position.position_id);
    assert_eq!(restored.origin, ManagedPositionOrigin::StrategyEntry);
}

#[test]
fn exposure_managed_recovery_origin_is_explicit_without_recovery_boolean() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let managed = ExposureState::Managed(ManagedPositionState {
        position: OpenPositionState {
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
            position_id: PositionId::from("P-RECOVERY-001"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.440,
            book: strategy.active.books.up.clone(),
        },
        origin: ManagedPositionOrigin::RecoveryBootstrap,
        pending_entry: None,
    });

    let managed = managed
        .managed_position()
        .expect("managed exposure should return managed position");
    assert_eq!(managed.origin, ManagedPositionOrigin::RecoveryBootstrap);
    assert_eq!(
        managed.position.position_id,
        PositionId::from("P-RECOVERY-001")
    );
}

#[test]
fn position_truth_recovery_after_terminal_flat_records_rematerialization_evidence() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-REMATERIALIZED-001");
    let entry_client_order_id = ClientOrderId::from("ENTRY-REMATERIALIZED-001");
    let pending = PendingEntryState {
        client_order_id: entry_client_order_id,
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
        book: configured_book_for_instrument(&mut strategy, instrument_id),
    };
    set_pending_entry(&mut strategy, pending);

    strategy
        .on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id))
        .expect("entry cancel should clear pending entry before rematerialization");
    assert!(matches!(strategy.exposure, ExposureState::Flat));

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(5.0, 2),
        0.450,
    ));

    assert!(
        evidence.events().into_iter().any(|event| matches!(
            event,
            RecordedDecisionEvidenceEvent::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleTransition::PositionTruthRematerialized
                    && record.outcome
                        == crate::bolt_v3_decision_evidence::BoltV3OrderLifecycleOutcome::Managed
                    && record.source == "position_event"
                    && record.client_order_id.as_deref() == Some("ENTRY-REMATERIALIZED-001")
                    && record.position_id.as_deref() == Some("P-REMATERIALIZED-001")
                    && record.residual_quantity.as_deref() == Some("5.00")
        )),
        "position truth rematerialization after a terminal Flat override must write linking lifecycle evidence"
    );
}

#[test]
fn flat_terminal_override_clears_without_linking_on_instrument_mismatch() {
    let mut strategy = ready_to_trade_strategy();
    let configured_instruments = configured_outcome_instruments(&strategy);
    assert!(
        configured_instruments.len() >= 2,
        "fixture must expose two outcome instruments"
    );
    let stored_instrument_id = configured_instruments[0];
    let mismatch_instrument_id = configured_instruments[1];
    let pending = pending_entry_for_terminal_override(
        &mut strategy,
        stored_instrument_id,
        ClientOrderId::from("ENTRY-MISMATCH-001"),
    );
    strategy.remember_flat_terminal_entry_override(&pending);

    assert!(
        strategy
            .take_position_truth_rematerialization_override(
                mismatch_instrument_id,
                ManagedPositionOrigin::RecoveryBootstrap,
            )
            .is_none(),
        "a recovery event for another instrument must not link to the stored terminal entry"
    );

    assert!(
        strategy
            .take_position_truth_rematerialization_override(
                stored_instrument_id,
                ManagedPositionOrigin::RecoveryBootstrap,
            )
            .is_none(),
        "instrument mismatch clears the stored override without linking it"
    );
}

#[test]
fn flat_terminal_override_clears_for_non_recovery_bootstrap_origin() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let pending = pending_entry_for_terminal_override(
        &mut strategy,
        instrument_id,
        ClientOrderId::from("ENTRY-NON-RECOVERY-001"),
    );
    strategy.remember_flat_terminal_entry_override(&pending);

    assert!(
        strategy
            .take_position_truth_rematerialization_override(
                instrument_id,
                ManagedPositionOrigin::StrategyEntry,
            )
            .is_none()
    );
    assert!(
        strategy
            .take_position_truth_rematerialization_override(
                instrument_id,
                ManagedPositionOrigin::RecoveryBootstrap,
            )
            .is_none(),
        "non-RecoveryBootstrap materialization clears the stale override"
    );
}

#[test]
fn flat_terminal_override_is_not_consumed_when_exposure_is_not_flat() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let pending = pending_entry_for_terminal_override(
        &mut strategy,
        instrument_id,
        ClientOrderId::from("ENTRY-NON-FLAT-001"),
    );
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-NON-FLAT-001"),
        Quantity::new(5.0, 2),
        0.450,
    );
    set_managed_position(
        &mut strategy,
        position,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.remember_flat_terminal_entry_override(&pending);

    assert!(
        strategy
            .take_position_truth_rematerialization_override(
                instrument_id,
                ManagedPositionOrigin::RecoveryBootstrap,
            )
            .is_none(),
        "non-Flat exposure must not consume the stored override"
    );

    strategy.exposure = ExposureState::Flat;
    assert_eq!(
        strategy
            .take_position_truth_rematerialization_override(
                instrument_id,
                ManagedPositionOrigin::RecoveryBootstrap,
            )
            .map(|terminal_override| terminal_override.client_order_id),
        Some(ClientOrderId::from("ENTRY-NON-FLAT-001"))
    );
}

#[test]
fn new_entry_submit_clears_stale_flat_terminal_override() {
    let mut strategy = ready_to_trade_strategy();
    set_active_books_best_prices(&mut strategy, 0.40, 0.41);
    strategy.config.order_notional_target = 25.0;
    strategy.config.maximum_position_notional = 25.0;
    strategy.config.risk_lambda = 0.0001;
    let instrument_id = selected_entry_instrument(&strategy);
    let pending = pending_entry_for_terminal_override(
        &mut strategy,
        instrument_id,
        ClientOrderId::from("ENTRY-SUBMIT-CLEAR-001"),
    );
    strategy.remember_flat_terminal_entry_override(&pending);
    let decision = strategy.entry_submission_decision_at(1_200);
    assert!(
        decision.instrument_id.is_some()
            && decision.order_side.is_some()
            && decision.price.is_some()
            && decision.quantity_value.is_some()
            && decision.blocked_reason.is_none(),
        "entry submit setup must reach the submit path; got {decision:#?}"
    );

    let submitted_client_order_id = strategy
        .try_submit_entry_order(1_200)
        .expect("entry submit setup should be admissible");

    assert!(
        submitted_client_order_id.is_some(),
        "entry submit setup must create a fresh pending entry"
    );
    assert!(
        strategy.last_flat_terminal_entry_override.is_none(),
        "new entry submit must clear stale terminal-entry override state"
    );
}

#[test]
fn live_entered_and_pending_adopted_positions_retain_interval_end_boundary() {
    let live_pending_entry = || {
        let mut strategy = ready_to_trade_strategy();
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
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
    fill_strategy
        .on_order_filled(&order_filled_event(
            fill_client_order_id,
            fill_instrument_id,
            PositionId::from("P-LIVE-ENTRY-INTERVAL-PIN"),
        ))
        .expect("live entry fill should materialize managed exposure");
    assert_eq!(
        managed_position_ref(&fill_strategy)
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
    position_strategy.on_position_opened(position_opened_event(
        position_instrument_id,
        PositionId::from("P-PENDING-ADOPTED-INTERVAL-PIN"),
        Quantity::new(10.0, 2),
        0.450,
    ));
    assert_eq!(
        managed_position_ref(&position_strategy)
            .and_then(|position| position.lifecycle.interval_end_ms()),
        Some(position_interval_end_ms),
        "pending-adopted position must inherit the pending entry interval end"
    );
}

#[test]
fn direct_entry_fill_materialization_clears_stale_flat_terminal_override() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let stale_pending = pending_entry_for_terminal_override(
        &mut strategy,
        instrument_id,
        ClientOrderId::from("ENTRY-DIRECT-STALE-001"),
    );
    strategy.remember_flat_terminal_entry_override(&stale_pending);
    let fill_pending = pending_entry_for_terminal_override(
        &mut strategy,
        instrument_id,
        ClientOrderId::from("ENTRY-DIRECT-FILL-001"),
    );
    set_pending_entry(&mut strategy, fill_pending.clone());

    strategy
        .on_order_filled(&order_filled_event(
            fill_pending.client_order_id,
            instrument_id,
            PositionId::from("P-DIRECT-CLEAR-001"),
        ))
        .expect("direct entry fill materialization should succeed");

    assert!(
        strategy.last_flat_terminal_entry_override.is_none(),
        "direct entry fill materialization must clear stale terminal-entry override state"
    );
}

fn pending_entry_for_terminal_override(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
) -> PendingEntryState {
    let book = configured_book_for_instrument(strategy, instrument_id);
    PendingEntryState {
        client_order_id,
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
        book,
    }
}

fn configured_entry_order_for_reconcile(
    strategy: &mut BinaryOracleEdgeTaker,
    instrument_id: InstrumentId,
    client_order_id: ClientOrderId,
) -> OrderAny {
    strategy
        .build_configured_entry_order(
            instrument_id,
            strategy
                .configured_entry_order_side()
                .expect("test config should carry entry order side"),
            Quantity::new(10.0, 2),
            Price::new(0.45, 2),
            client_order_id,
        )
        .expect("configured entry order should build for reconcile test")
}

fn close_order_with_canceled_event(order: &mut OrderAny) {
    let (submitted, accepted) = submitted_and_accepted_events_for_reconcile(order);
    order
        .apply(submitted)
        .expect("submitted event should apply to test order");
    order
        .apply(accepted)
        .expect("accepted event should apply to test order");
    order
        .apply(nautilus_model::events::OrderEventAny::Canceled(
            reconcile_order_canceled_event(order),
        ))
        .expect("canceled event should apply to accepted test order");
}

fn close_order_with_filled_event(order: &mut OrderAny, position_id: PositionId) {
    let (submitted, accepted) = submitted_and_accepted_events_for_reconcile(order);
    order
        .apply(submitted)
        .expect("submitted event should apply to test order");
    order
        .apply(accepted)
        .expect("accepted event should apply to test order");
    let mut fill = order_filled_event_with_details(
        order.client_order_id(),
        order.instrument_id(),
        Some(position_id),
        order.order_side(),
    );
    fill.last_qty = order.quantity();
    order
        .apply(nautilus_model::events::OrderEventAny::Filled(fill))
        .expect("filled event should apply to accepted test order");
}

fn submitted_and_accepted_events_for_reconcile(
    order: &OrderAny,
) -> (
    nautilus_model::events::OrderEventAny,
    nautilus_model::events::OrderEventAny,
) {
    let trader_id = nautilus_model::identifiers::TraderId::from("TRADER-001");
    let strategy_id = StrategyId::from("BINARYORACLEEDGETAKER-001");
    let instrument_id = order.instrument_id();
    let client_order_id = order.client_order_id();
    let account_id = nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT");
    (
        nautilus_model::events::OrderEventAny::Submitted(
            nautilus_model::events::OrderSubmitted::new(
                trader_id,
                strategy_id,
                instrument_id,
                client_order_id,
                account_id,
                nautilus_core::UUID4::new(),
                UnixNanos::from(1_000_u64),
                UnixNanos::from(1_000_u64),
            ),
        ),
        nautilus_model::events::OrderEventAny::Accepted(
            nautilus_model::events::OrderAccepted::new(
                trader_id,
                strategy_id,
                instrument_id,
                client_order_id,
                nautilus_model::identifiers::VenueOrderId::from("V-RECONCILE-001"),
                account_id,
                nautilus_core::UUID4::new(),
                UnixNanos::from(1_001_u64),
                UnixNanos::from(1_001_u64),
                false,
            ),
        ),
    )
}

fn reconcile_order_canceled_event(order: &OrderAny) -> nautilus_model::events::OrderCanceled {
    nautilus_model::events::OrderCanceled::new(
        nautilus_model::identifiers::TraderId::from("TRADER-001"),
        StrategyId::from("BINARYORACLEEDGETAKER-001"),
        order.instrument_id(),
        order.client_order_id(),
        nautilus_core::UUID4::new(),
        UnixNanos::from(1_002_u64),
        UnixNanos::from(1_002_u64),
        false,
        Some(nautilus_model::identifiers::VenueOrderId::from(
            "V-RECONCILE-001",
        )),
        Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
    )
}

fn emit_selection_retry_time_event(strategy: &mut BinaryOracleEdgeTaker, event_ts_ms: u64) {
    let event = TimeEvent::new(
        ustr::Ustr::from(strategy.selection_retry_timer_name().as_str()),
        nautilus_core::UUID4::new(),
        UnixNanos::from(event_ts_ms * NANOS_PER_MILLI_U64),
        UnixNanos::from(event_ts_ms * NANOS_PER_MILLI_U64),
    );
    DataActor::on_time_event(strategy, &event)
        .expect("selection retry time event should route through strategy handler");
}

fn reconcile_due_time_ms(strategy: &BinaryOracleEdgeTaker, submitted_at_ms: u64) -> u64 {
    submitted_at_ms.saturating_add(strategy.runtime_reconcile_min_age_ms())
}
