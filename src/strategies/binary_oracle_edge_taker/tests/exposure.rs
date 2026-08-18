#![cfg(test)]

use super::*;
use crate::bolt_v3_order_execution::BoltV3RouteAttemptCompletion;
use nautilus_model::enums::PositionSideSpecified;
use nautilus_model::identifiers::TradeId;
use nautilus_trading::Strategy;
use std::{collections::BTreeSet, sync::Arc};

fn reduce_position_close_with_projection(
    strategy: &BinaryOracleEdgeTaker,
    episode: PositionEpisodeFingerprint,
    projection: FreshCanonicalPositionProjection,
) -> ExposureTransitionOutcome {
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionClosed(
            PositionClosedEvent::ObservedWithFreshProjection {
                expected_generation: strategy.exposure.generation(),
                episode,
                projection,
            },
        ),
    )
}

#[test]
fn position_events_update_live_position_state() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-001");

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    assert!(strategy.managed_position().is_some());
    assert_eq!(
        strategy.managed_position().map(|managed| managed.origin),
        Some(ManagedPositionOrigin::RecoveryBootstrap)
    );
    let managed_position =
        managed_position_snapshot(&strategy).expect("position should be managed after open event");
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

    let recovered_position = managed_position_snapshot(&strategy)
        .expect("position should be managed before exit pending");
    set_exit_pending(
        &mut strategy,
        recovered_position,
        ClientOrderId::from("EXIT-001"),
        ManagedPositionOrigin::RecoveryBootstrap,
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
    assert!(!strategy.exposure.is_recovering());
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
        OrderSide::Buy,
        PositionSide::Long,
    );

    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    let retained = managed_position_snapshot(&strategy)
        .expect("a stale close callback cannot override the NT open-position cache");
    assert_eq!(retained.position_id, position_id);
    assert_eq!(retained.quantity, Quantity::new(7.0, 2));
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
        OrderSide::Buy,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let mut fill = order_filled_event(
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
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id()),
        Some(exit_client_order_id)
    );
    assert!(strategy.managed_position().is_some());
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(7.0, 2),
        0.470,
        OrderSide::Buy,
    );
    strategy.materialize_position_from_event(
        PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(7.0, 2),
            avg_px_open: 0.470,
            opening_order_id: ClientOrderId::from(format!("ENTRY-{position_id}").as_str()),
            ts_opened_ns: 1,
        },
        0,
    );

    let exit_pending = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("position change should keep exit pending");
    assert_eq!(exit_pending.client_order_id(), exit_client_order_id);
    assert_eq!(exit_pending.position_id(), position_id);

    let context = &exit_pending.position;
    assert_eq!(context.origin, ManagedPositionOrigin::StrategyEntry);
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-001"),
        ManagedPositionOrigin::StrategyEntry,
    );

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id()),
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-001"),
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.on_order_filled(&order_filled_event(
        ClientOrderId::from("EXIT-001"),
        tracked_instrument,
        Some(PositionId::from("P-TRACKED")),
        OrderSide::Buy,
    ));

    strategy.on_position_closed(position_closed_event(
        tracked_instrument,
        PositionId::from("P-OTHER"),
    ));

    assert_eq!(
        pending_exit_snapshot(&strategy).map(|pending| pending.client_order_id()),
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut canceled,
        canceled_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut rejected,
        rejected_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut expired,
        expired_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let mut fill = order_filled_event(
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
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
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

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        0.45,
        OrderSide::Buy,
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
        managed_position_snapshot(&strategy).map(|position| position.quantity),
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let mut fill = order_filled_event(
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
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));

    let canceled_event = order_canceled_event(exit_client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled_event.clone()),
    );
    strategy.on_order_canceled(&canceled_event);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        0.45,
        OrderSide::Buy,
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
        managed_position_snapshot(&strategy).map(|position| position.quantity),
        Some(Quantity::new(6.0, 2))
    );
    assert!(pending_exit_snapshot(&strategy).is_none());
}

#[test]
fn provenance_free_fill_void_quarantines_without_minting_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let client_order_id = ClientOrderId::from("EXIT-FILL-VOID-MISSING-CACHE");
    let position_id = PositionId::from("P-FILL-VOID-MISSING-CACHE");
    let event = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        nautilus_model::identifiers::TradeId::from("TRADE-FILL-VOID-MISSING-CACHE"),
        Quantity::new(1.0, 2),
        1_000,
        OrderSide::Sell,
    );

    strategy.on_order_fill_voided(&event);

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert!(strategy.exposure.quarantined_order(&client_order_id));

    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("provenance-free-quarantine"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("quarantined foreign correction should remain non-routing");
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    assert!(strategy.exposure.quarantined_order(&client_order_id));
}

#[test]
fn provenance_free_exit_fill_quarantines_without_displacing_live_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-UNTRACKED-EXIT-FILL");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let retained_episode = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed")
        .episode;
    let client_order_id = ClientOrderId::from("EXIT-UNTRACKED-FILL");
    let event = order_filled_event(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    strategy.handle_order_filled(&event);

    assert!(strategy.exposure.quarantined_order(&client_order_id));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.episode == retained_episode
    ));
}

#[test]
fn fill_void_without_position_identity_keeps_the_hold_until_exact_attribution_arrives() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FILL-VOID-LATE-ATTRIBUTION");
    let client_order_id = ClientOrderId::from("EXIT-FILL-VOID-LATE-ATTRIBUTION");
    let mut event = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        nautilus_model::identifiers::TradeId::from("TRADE-FILL-VOID-LATE-ATTRIBUTION"),
        Quantity::new(1.0, 2),
        1_000,
        OrderSide::Sell,
    );
    event.position_id = None;

    strategy.on_order_fill_voided(&event);
    assert!(strategy.exposure.quarantined_order(&client_order_id));

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(1.0, 2),
        0.45,
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(1.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    assert!(strategy.exposure.quarantined_order(&client_order_id));
}

#[test]
fn terminal_callback_with_missing_cached_exit_enters_hold_and_resumes_exact_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-TERMINAL-MISSING-CACHE");
    let client_order_id = ClientOrderId::from("EXIT-TERMINAL-MISSING-CACHE");
    let quantity = Quantity::new(10.0, 2);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        quantity,
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    register_test_strategy(&mut strategy).borrow_mut().reset();

    strategy.on_order_canceled(&order_canceled_event(client_order_id, instrument_id));

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitAuthorityRecoveryHold(_)
    ));

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        quantity,
        0.45,
        OrderSide::Buy,
    );
    seed_nt_working_order(
        &mut strategy,
        recovered_exit_order(client_order_id, instrument_id, quantity),
        position_id,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("terminal-missing-cache-recovery"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("fresh exact cached order should restore the retained exit authority");

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));
    let canceled = order_canceled_event(client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );
    strategy.on_order_canceled(&canceled);
    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.quantity),
        Some(quantity)
    );
}

#[test]
fn timer_cache_loss_moves_working_exit_into_the_same_non_routing_hold() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-TIMER-MISSING-CACHE");
    let client_order_id = ClientOrderId::from("EXIT-TIMER-MISSING-CACHE");
    let quantity = Quantity::new(10.0, 2);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        quantity,
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    register_test_strategy(&mut strategy).borrow_mut().reset();

    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("timer-missing-cache-hold"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("cache loss should become a typed non-routing hold");

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitAuthorityRecoveryHold(_)
    ));
    assert_eq!(
        strategy
            .exposure
            .exit_authority_recovery_hold()
            .map(|hold| hold.client_order_id()),
        Some(client_order_id)
    );
}

#[test]
fn timer_quantity_conflict_moves_exit_into_the_same_non_routing_hold() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-TIMER-QUANTITY-CONFLICT");
    let client_order_id = ClientOrderId::from("EXIT-TIMER-QUANTITY-CONFLICT");
    let authorized_quantity = Quantity::new(10.0, 2);
    let open_position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        authorized_quantity,
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    register_test_strategy(&mut strategy).borrow_mut().reset();
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        authorized_quantity,
        0.45,
        OrderSide::Buy,
    );
    seed_nt_working_order(
        &mut strategy,
        recovered_exit_order(client_order_id, instrument_id, Quantity::new(9.0, 2)),
        position_id,
    );

    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("timer-quantity-conflict-hold"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("an order-authority conflict should be held without routing");

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitAuthorityRecoveryHold(_)
    ));

    close_nt_position(&mut strategy, position_id);
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Flat,
        Quantity::zero(2),
        1_200,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("timer-quantity-conflict-flat-report"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_200_u64),
            UnixNanos::from(1_200_u64),
        ),
    )
    .expect("a flat report must not authorize past a still-working conflicting order");
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitAuthorityRecoveryHold(_)
    ));
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let trade_id = nautilus_model::identifiers::TradeId::from("TRADE-MISSED-FILL-VOID");
    let mut fill = order_filled_event(
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
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));

    let fill_voided = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        trade_id,
        Quantity::new(10.0, 2),
        1_100,
        OrderSide::Sell,
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
        matches!(strategy.exposure.state(), ExposureState::ExitPending(_)),
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let trade_id = nautilus_model::identifiers::TradeId::from("TRADE-CACHED-VOIDED");
    let mut fill = order_filled_event(
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
        OrderSide::Sell,
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
        matches!(
            strategy.exposure.state(),
            ExposureState::TerminalExitAwaitingPosition(_)
        ),
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
fn recovery_hold_observation_updates_reconstruction_floor_before_retry() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-HOLD-OBSERVATION");
    let client_order_id = ClientOrderId::from("EXIT-HOLD-OBSERVATION");
    let quantity = Quantity::new(10.0, 2);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        quantity,
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let retained_exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should hold the original sealed authority");
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::RecoveryHold(ExitAuthorityRecoveryHoldState {
            position: retained_exit.position.clone(),
            pending_exit: retained_exit.pending_exit.clone(),
            plan: ExitAuthorityRecoveryPlan::Reconstruct {
                cause: BoltV3RecoveredExitCause::FillVoidReopen,
                client_order_id,
            },
            flat_recovery: ExitAuthorityFlatRecovery::AwaitingLease,
            observations: BTreeMap::new(),
        }),
    ));

    let mut partial_fill = order_filled_event(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    partial_fill.last_qty = Quantity::new(3.0, 2);
    partial_fill.trade_id = TradeId::from("TRADE-HOLD-OBSERVATION");
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(partial_fill),
    );
    strategy.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
        client_order_id,
        instrument_id,
        transition: OrderLifecycleTransition::OrderFilled,
        source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
        raw_reason_text: None,
        ts_event_ns: 2_000,
        authority: ExitOrderAuthorityObservation::Lifecycle,
    });

    let reconstructed = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("the held observation should feed the successful reconstruction retry");
    assert!(
        reconstructed
            .authority
            .observed_fill_ids()
            .contains(&TradeId::from("TRADE-HOLD-OBSERVATION")),
        "reconstruction from the stale pre-observation floor would omit the held fill identity"
    );
    drop(retained_exit);
}

#[test]
fn fill_void_after_terminal_release_reconstructs_recovered_exit_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-FILL-VOID-AFTER-RELEASE");
    let client_order_id = ClientOrderId::from("EXIT-FILL-VOID-AFTER-RELEASE");
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
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );

    let trade_id = nautilus_model::identifiers::TradeId::from("TRADE-FILL-VOID-AFTER-RELEASE");
    let mut fill = order_filled_event(
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
    close_nt_position(&mut strategy, position_id);
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
            ustr::Ustr::from("terminal-release-before-fill-void"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_100_u64),
            UnixNanos::from(1_100_u64),
        ),
    )
    .expect("terminal exit should release before the correction");
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    let fill_voided = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        trade_id,
        Quantity::new(10.0, 2),
        1_200,
        OrderSide::Sell,
    );
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::FillVoided(fill_voided.clone()),
    );
    strategy.on_order_fill_voided(&fill_voided);

    assert!(
        matches!(strategy.exposure.state(), ExposureState::ExitPending(_)),
        "a post-release fill void must reconstruct recovered authority for the reopened order: {:?}",
        strategy.exposure
    );
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
    );
    observe_position_authority_report(
        &strategy,
        instrument_id,
        PositionSideSpecified::Long,
        Quantity::new(10.0, 2),
        1_300,
    );
    DataActor::on_time_event(
        &mut strategy,
        &TimeEvent::new(
            ustr::Ustr::from("fill-void-recovered-baseline"),
            nautilus_core::UUID4::new(),
            UnixNanos::from(1_300_u64),
            UnixNanos::from(1_300_u64),
        ),
    )
    .expect("timer should establish the post-correction recovered baseline");
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));
}

#[test]
fn wrong_side_fill_void_does_not_link_to_a_released_exit() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-WRONG-SIDE-FILL-VOID");
    let client_order_id = ClientOrderId::from("EXIT-WRONG-SIDE-FILL-VOID");
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
        open_position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    let fill_voided = order_fill_voided_event(
        client_order_id,
        instrument_id,
        position_id,
        TradeId::from("TRADE-WRONG-SIDE-FILL-VOID"),
        Quantity::new(10.0, 2),
        1_200,
        OrderSide::Buy,
    );
    strategy.on_order_fill_voided(&fill_voided);

    assert!(
        matches!(strategy.exposure.state(), ExposureState::Flat),
        "a correction with the entry side must not acquire released-exit authority: {:?}",
        strategy.exposure
    );
    assert!(pending_exit_snapshot(&strategy).is_none());
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

    strategy.on_order_filled(&order_filled_event(
        exit_client_order_id,
        foreign_instrument_id,
        Some(position_id),
        OrderSide::Buy,
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
        OrderSide::Buy,
        PositionSide::Long,
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

    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        foreign_instrument_id,
        Some(PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL")),
        OrderSide::Buy,
    ));

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(ref retained)
            if retained.episode.position_id == position_id
                && retained.pending_entry.as_ref().is_some_and(|pending|
                    pending.client_order_id == entry_client_order_id)
    ));
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
        OrderSide::Buy,
        PositionSide::Long,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        ManagedPositionOrigin::StrategyEntry,
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
        OrderSide::Buy,
    );

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    assert_eq!(
        managed_position_snapshot(&strategy).and_then(|position| position.lifecycle.outcome_side()),
        None
    );
    let position = managed_position_snapshot(&strategy).expect("position should be tracked");
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
        OrderSide::Buy,
    );
    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_a,
        Some(position_id),
        OrderSide::Buy,
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
        OrderSide::Buy,
    );
    let mut first_fill = order_filled_event(
        entry_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    );
    first_fill.last_qty = Quantity::new(4.0, 2);
    strategy.on_order_filled(&first_fill);
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(4.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    let mut second_fill = order_filled_event(
        entry_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    );
    second_fill.last_qty = Quantity::new(6.0, 2);
    strategy.on_order_filled(&second_fill);

    assert_eq!(strategy.market_churn_count("MKT-1"), 2);
    assert_eq!(
        managed_position_snapshot(&strategy).map(|position| position.quantity),
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
fn forced_flat_exit_submits_despite_resting_pending_entry() {
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

        let decision = strategy.exit_intent_decision_at(1_200);

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
fn forced_flat_submit_cancels_resting_entry_and_converges_if_entry_fill_callback_races() {
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

        let exit_order = risk_messages
            .iter()
            .find_map(|message| match message {
                TradingCommand::SubmitOrder(command)
                    if command.client_order_id == exit_client_order_id =>
                {
                    Some(
                        nautilus_model::orders::OrderAny::from_events(vec![
                            nautilus_model::events::OrderEventAny::Initialized(
                                command.order_init.clone(),
                            ),
                        ])
                        .expect("submitted exit command should replay into an order"),
                    )
                }
                _ => None,
            })
            .expect("forced-flat submit should expose its final order");
        let exit_order = seed_nt_working_order(&mut strategy, exit_order, position_id);

        strategy.on_order_filled(&order_filled_event(
            entry_client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Buy,
        ));
        let mut exit_fill = order_filled_event(
            exit_client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Sell,
        );
        exit_fill.order_type = exit_order.order_type();
        exit_fill.last_qty = exit_order.quantity();
        exit_fill.trade_id = nautilus_model::identifiers::TradeId::from("TRADE-FORCED-EXIT");
        apply_exit_order_event_to_nt_cache(
            &mut strategy,
            nautilus_model::events::OrderEventAny::Filled(exit_fill.clone()),
        );
        apply_exit_fill_to_nt_position(&mut strategy, position_id, &exit_fill);
        strategy.on_order_filled(&exit_fill);

        let expected_residual = position_quantity - exit_order.quantity();

        assert_eq!(
            expected_residual.as_decimal(),
            Decimal::ZERO,
            "this fixture must exercise a full forced-flat reduction: {instrument_id}",
        );
        assert!(
            strategy.managed_position().is_none(),
            "a causally applied full forced-flat fill must leave no managed exposure: {instrument_id}",
        );
        assert!(
            strategy.exposure.exit_pending_snapshot().is_none(),
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

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
    );
    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    ));

    assert_eq!(
        strategy
            .exposure
            .managed_position_context()
            .and_then(|managed| managed.pending_entry),
        None
    );
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

    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_id,
        None,
        OrderSide::Buy,
    ));

    assert!(strategy.exposure.is_recovering());
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
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        late_position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
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
    assert_eq!(
        strategy.managed_position().map(|managed| managed.origin),
        Some(ManagedPositionOrigin::StrategyEntry)
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
    let entry_client_order_id = ClientOrderId::from("ENTRY-ZERO-FILL-CANCEL");
    let canceled_pending = pending_entry_state(&mut canceled, entry_client_order_id);
    let canceled_instrument_id = canceled_pending.instrument_id;
    set_entry_reconcile_pending(
        &mut canceled,
        canceled_pending,
        EntryReconcileReason::UnresolvedAtSelectionBoundary,
    );
    canceled.on_order_canceled(&order_canceled_event(
        entry_client_order_id,
        canceled_instrument_id,
    ));
    assert!(matches!(canceled.exposure.state(), ExposureState::Flat));
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
    assert!(matches!(rejected.exposure.state(), ExposureState::Flat));

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
    assert!(matches!(denied.exposure.state(), ExposureState::Flat));

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
    assert!(matches!(expired.exposure.state(), ExposureState::Flat));
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
        canceled.exposure.state(),
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::AwaitingPositionMaterialization,
            ..
        }
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
        expired.exposure.state(),
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::AwaitingPositionMaterialization,
            ..
        }
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
    set_pending_entry(&mut strategy, pending);

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
    set_pending_entry(&mut strategy, pending);

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
    set_pending_entry(&mut strategy, pending);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-NEXT", 2_000));

    assert!(matches!(strategy.exposure.state(),
        ExposureState::EntryReconcilePending {
            pending,
            reason: EntryReconcileReason::UnresolvedAtSelectionBoundary,
        } if pending.instrument_id == instrument_id
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
    set_pending_entry(&mut strategy, pending);

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
    set_pending_entry(&mut strategy, pending);
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
    set_pending_entry(&mut strategy, pending);

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
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::EntryReconcilePending { .. }
    ));
    assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
}

#[test]
fn position_closed_releases_entry_reconcile_pending_for_same_instrument() {
    let evidence = recording_decision_evidence();
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

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::EntryReconcilePending { .. }
    ));
    assert!(strategy.pending_entry().is_some());
    assert!(
        evidence
            .recorded_facts()
            .expect("recorded current evidence must decode")
            .into_iter()
            .any(|event| matches!(
                event,
                CurrentFact::OrderLifecycle(record)
                    if record.transition
                        == crate::bolt_v3_current_evidence::OrderLifecycleTransition::ExposureQuarantined
                        && record.position_id.as_deref() == Some("P-CLOSED-BEFORE-OPEN")
                        && record.outcome
                            == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::Quarantined
            )),
        "unattributed position close must write quarantine evidence"
    );
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
    let mut closed = position_closed_event(instrument_id, position_id);
    closed.opening_order_id = entry_client_order_id;
    strategy.on_position_closed(closed);

    let exec_messages = exec_messages.get_messages();
    assert!(
        exec_messages.iter().any(|message| matches!(
            message,
            TradingCommand::CancelOrder(command)
                if command.client_order_id == entry_client_order_id
        )),
        "external position close should cancel the resting entry"
    );
    assert!(matches!(strategy.exposure.state(),
        ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            ..
        }) if client_order_id == entry_client_order_id
    ));
    assert!(strategy.pending_entry().is_some());

    strategy.on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id));
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
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
    let mut closed = position_closed_event(instrument_id, position_id);
    closed.opening_order_id = entry_client_order_id;
    strategy.on_position_closed(closed);

    let exec_messages = exec_messages.get_messages();
    assert!(
        !exec_messages
            .iter()
            .any(|message| matches!(message, TradingCommand::CancelOrder(_))),
        "shadow mode must not emit a venue CancelOrder on external position close"
    );
    // The exposure still transitions to retain the pending-entry context; only
    // the venue cancel is suppressed in shadow mode.
    assert!(matches!(strategy.exposure.state(),
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
        strategy.exposure.state(),
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
        OrderSide::Buy,
        PositionSide::Long,
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        open_position,
        ClientOrderId::from("EXIT-FOREIGN-CLOSE"),
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
            episode: position_episode_for_test(instrument_id, position_id),
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
            episode: position_episode_for_test(instrument_id, position_id),
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

    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn sell_fill_enters_recovery_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        instrument_id,
        Some(PositionId::from("P-SHORT")),
        OrderSide::Sell,
    ));

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
    let mut fill = order_filled_event(entry_client_order_id, instrument_id, None, OrderSide::Buy);
    fill.last_qty = Quantity::new(2.0, 2);

    awaiting.on_order_filled(&fill);

    assert!(matches!(
        awaiting.exposure.state(),
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::AwaitingPositionMaterialization,
            ..
        }
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
    let mut fill = order_filled_event(
        entry_client_order_id,
        instrument_id,
        Some(PositionId::from("P-FILL-UNSUPPORTED-SIDE")),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(3.0, 2);

    unsupported.on_order_filled(&fill);

    assert!(matches!(
        unsupported.exposure.state(),
        ExposureState::EntryReconcilePending {
            reason: EntryReconcileReason::UnsupportedEntryFillSide {
                order_side: OrderSide::Sell,
            },
            ..
        }
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

    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        fill_instrument_id,
        Some(PositionId::from("P-MISMATCHED-FILL")),
        OrderSide::Sell,
    ));

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(ref retained)
            if retained.client_order_id == entry_client_order_id
                && retained.instrument_id == pending_instrument_id
    ));
    assert!(strategy.managed_position().is_none());
}

#[test]
fn pending_entry_short_position_event_stays_fail_closed_without_materializing_position() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);
    let position_id = PositionId::from("P-SHORT");
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Sell,
    );

    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Sell,
        PositionSide::Short,
    ));

    assert!(strategy.exposure.is_recovering());
    assert!(strategy.managed_position().is_none());
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            reason: BlindRecoveryReason::DivergentUnsupportedPosition,
            ..
        })
    ));
    assert!(strategy.tracked_observed_position().is_none());
    assert_eq!(
        strategy.pending_entry().map(|entry| entry.client_order_id),
        Some(entry_client_order_id)
    );
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

    strategy.on_position_opened(position_opened_event(
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
            strategy.exposure.state(),
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition { .. },
                ..
            })
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

    strategy.on_order_filled(&order_filled_event(
        entry_client_order_id,
        foreign_instrument_id,
        Some(PositionId::from("P-FOREIGN-FILL")),
        OrderSide::Buy,
    ));

    // The foreign observation is quarantined in place and never displaces the live entry.
    assert!(
        matches!(
            strategy.exposure.state(),
            ExposureState::PendingEntry(ref retained)
                if retained.client_order_id == entry_client_order_id
                    && retained.instrument_id == execution_instrument_id
        ),
        "foreign-venue entry fill must preserve the retained entry authority, got {:?}",
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

    strategy.on_position_opened(position_opened_event(
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
        OrderSide::Buy,
        PositionSide::Long,
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
        OrderSide::Buy,
        PositionSide::Long,
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
fn recovery_bootstrap_quarantines_foreign_venue_position() {
    // Recovery-path regression lock. The entry path is venue-scoped, but
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
        matches!(managed, ClassifiedOpenPosition::Managed(_)),
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
            ClassifiedOpenPosition::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition { .. },
                ..
            })
        ),
        "foreign-venue position must be quarantined to blind recovery, got {quarantined:?}",
    );
}

#[test]
fn fresh_blind_recovery_classifies_an_unsupported_contract_before_adoption() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RECOVERY-UNSUPPORTED-CONTRACT"),
        Quantity::new(1.0, 2),
        0.45,
        OrderSide::Sell,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::BlindRecovery(BlindRecoveryState::authority_free(
            BlindRecoveryReason::CacheProbeFailed,
        )),
    ));

    strategy.reconcile_blind_recovery_from_fresh_probe(0);

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::UnsupportedObserved(UnsupportedObservedState {
            reason: UnsupportedObservedReason::BootstrappedUnsupportedContract,
            ..
        })
    ));
}

#[test]
fn typed_blind_recovery_rejects_an_invalid_position_side() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let mut invalid = configured_position_probe(&mut strategy, instrument_id);
    invalid.side = PositionSide::Flat;
    let classified =
        strategy.bootstrapped_exposure_for(invalid, strategy.context.execution_venue());
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::BlindRecovery(BlindRecoveryState::authority_free(
            BlindRecoveryReason::CacheProbeFailed,
        )),
    ));
    let grant = strategy
        .exposure
        .request_recovery_operation(strategy.exposure.generation())
        .expect("blind recovery should grant only the typed recovery operation");
    let RecoveryOperationCommit {
        outcome,
        replacement_adoption,
        restart_adoption,
    } = grant.commit(FreshCanonicalPositionProjection::ExactlyOne(Box::new(
        classified,
    )));
    assert!(matches!(
        outcome,
        ExposureTransitionOutcome::Applied {
            to: ExposureStateKind::BlindRecovery,
            ..
        }
    ));
    assert!(replacement_adoption.is_none());
    assert!(!restart_adoption);

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            reason: BlindRecoveryReason::InvalidBootstrappedPosition { .. },
            ..
        })
    ));
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
    let foreign_fill = order_filled_event(
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
        matches!(strategy.exposure.state(), ExposureState::Flat),
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
    let execution_fill = order_filled_event(
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
        episode: position_episode_for_test(
            strategy.active.books.up.instrument_id.unwrap(),
            PositionId::from("P-INVARIANT-1"),
        ),
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
        episode: position_episode_for_test(
            strategy.active.books.up.instrument_id.unwrap(),
            PositionId::from("P-INVARIANT-2"),
        ),
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

    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
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
        episode: position_episode_for_test(
            strategy.active.books.up.instrument_id.unwrap(),
            PositionId::from("P-UP-LOG-001"),
        ),
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
            episode: position_episode_for_test(instrument_id, PositionId::from("P-UNKNOWN-001")),
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

    let decision = strategy.exit_intent_decision_at(2_000);

    assert_eq!(decision.evaluation.exit_decision, None);
    assert_eq!(decision.instrument_id, None);
    assert_eq!(decision.order_side, None);
    assert_eq!(decision.price, None);
    assert_eq!(decision.quantity, None);
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceExitBlockedReason::PositionIntervalUnknown)
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
    };

    assert_eq!(exposure.pending_entry(), Some(&pending));
    assert_eq!(
        exposure.occupancy(),
        Some(ExposureOccupancy::EntryReconcilePending)
    );
    assert!(exposure.blocks_new_entries());
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
    let authority = BoltV3ExitOrderAuthorityHandle::recovered_for_test(
        BoltV3RecoveredExitCause::StartupAdoption,
        client_order_id,
        instrument_id,
        position_id,
        PositionEpisodeFingerprint {
            instrument_id,
            position_id,
            opening_order_id: ClientOrderId::from("ENTRY-EXIT-STATE-001"),
            ts_opened_ns: 1,
        },
        quantity.as_decimal(),
        PositionSideSpecified::Long,
        &recovered_exit_order(client_order_id, instrument_id, quantity),
        lease,
    )
    .expect("fixture exit authority should build");
    let context = managed_position_context(
        OpenPositionState {
            episode: position_episode_for_test(instrument_id, PositionId::from("P-RECOVERY-001")),
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
        },
        ManagedPositionOrigin::StrategyEntry,
        None,
    );
    let exit_pending = ExitPendingState {
        position: context,
        pending_exit: PendingExitState {
            submitted_at_ms: Some(1_000),
        },
        authority,
    };

    assert_eq!(
        exit_pending.client_order_id(),
        ClientOrderId::from("EXIT-STATE-001")
    );
    assert_eq!(
        exit_pending.position.position_id,
        PositionId::from("P-EXIT-STATE-001")
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
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-ATTEMPT-GENERATION"),
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create exit authority");
    let managed = exit.position.clone();
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            managed.clone(),
        )));
    let generation = strategy.exposure.generation();
    let grant = strategy
        .exposure
        .request_exit_operation(generation)
        .expect("managed exposure should grant one exit operation");
    let attempt_generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation: attempt_generation,
            managed,
            pending_exit: exit.pending_exit.clone(),
            authority: exit.authority.clone(),
        })
        .expect("exit grant should bind its attempt");
    participant
        .consume_at_pre_sink()
        .expect("exit attempt should consume at the pre-sink boundary");

    // Models the cache-first synchronous NT callback that advances the attempt
    // while the raw submit leaf is still on the stack.
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::TerminalAwaitingPosition(exit.clone()),
    ));
    drop(participant);

    assert_eq!(
        strategy.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(exit),
        "the callback-owned terminal fence must win over the stale route return"
    );
}

#[test]
fn synchronous_partial_fill_working_callback_retires_the_exit_route_arm() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-EXIT-PARTIAL-CALLBACK");
    let client_order_id = ClientOrderId::from("EXIT-PARTIAL-CALLBACK");
    let position = materialize_configured_position(
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
        position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should create exit authority");
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));
    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("managed exposure should grant one exit operation");
    let generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation,
            managed: exit.position,
            pending_exit: exit.pending_exit,
            authority: exit.authority,
        })
        .expect("exit grant should bind its attempt");
    participant.consume_at_pre_sink().unwrap();
    participant.mark_sink_invoked(1_500).unwrap();

    let trade_id = TradeId::from("TRADE-EXIT-PARTIAL-CALLBACK");
    let mut partial_fill = order_filled_event(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    partial_fill.last_qty = Quantity::new(3.0, 2);
    partial_fill.trade_id = trade_id;
    apply_exit_order_event_to_nt_cache(
        &mut strategy,
        nautilus_model::events::OrderEventAny::Filled(partial_fill),
    );
    strategy.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
        client_order_id,
        instrument_id,
        transition: OrderLifecycleTransition::OrderFilled,
        source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
        raw_reason_text: None,
        ts_event_ns: 2_000,
        authority: ExitOrderAuthorityObservation::Lifecycle,
    });

    participant.complete(BoltV3RouteAttemptCompletion::Submitted);
    let retained = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("the working callback should own the submitted exit state");
    assert!(retained.authority.observed_fill_ids().contains(&trade_id));
}

#[test]
fn overlapping_exit_operation_requests_mint_only_one_grant() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-OVERLAPPING-EXIT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let client_order_id = ClientOrderId::from("EXIT-OVERLAPPING");
    set_exit_pending(
        &mut strategy,
        position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should retain one sealed exit authority");
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));
    let generation = strategy.exposure.generation();
    let first = strategy
        .exposure
        .request_exit_operation(generation)
        .expect("first exit operation should arm");
    let second = strategy
        .exposure
        .request_exit_operation(generation)
        .expect_err("overlapping exit operation must be rejected");
    assert_eq!(
        second.reason,
        ExposureOperationBlockedReason::StaleGeneration
    );
    let decision = strategy.exit_intent_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceExitBlockedReason::StaleGeneration)
    );
    strategy
        .record_exit_intent_or_hold_once(
            1_200,
            ExitEvaluationTriggerContext::unknown(1_200),
            &decision,
        )
        .expect("stale-generation decision evidence should record");
    assert!(
        evidence
            .recorded_facts()
            .expect("typed stale-generation evidence should decode")
            .into_iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::ExitHoldDecision(record)
                    if record.blocked_reason == Some(EvidenceExitBlockedReason::StaleGeneration)
                        && record.outcome == ExitHoldOutcome::Blocked
            ))
    );
    let first_generation = first.generation();
    let mut participant = first
        .bind(ExitAttemptingState {
            generation: first_generation,
            managed: exit.position,
            pending_exit: exit.pending_exit,
            authority: exit.authority,
        })
        .expect("the sole minted exit grant should bind");
    participant
        .consume_at_pre_sink()
        .expect("the sole minted exit grant should reach pre-sink");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    participant.complete(BoltV3RouteAttemptCompletion::Submitted);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(current) if current.client_order_id() == client_order_id
    ));
}

#[test]
fn same_episode_refresh_preserves_fingerprint_and_close_floor() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-SAME-EPISODE-REFRESH");
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let episode = position.episode.clone();
    reduce_position_close_with_projection(
        &strategy,
        episode.clone(),
        FreshCanonicalPositionProjection::ExactlyOne(Box::new(ClassifiedOpenPosition::Managed(
            strategy
                .exposure
                .managed_position_context()
                .expect("fixture position should be managed"),
        ))),
    );
    let mut refreshed = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    refreshed.book.best_bid = Some(0.44);
    refreshed.episode_close_seen = false;
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(refreshed)),
        )),
    );

    let preserved = strategy
        .exposure
        .managed_position_context()
        .expect("same episode must remain managed");
    assert_eq!(preserved.episode, episode);
    assert!(preserved.episode_close_seen);
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn delayed_close_for_reused_position_id_cannot_release_new_episode() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-REUSED-EPISODE");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut episode_b = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    episode_b.episode.opening_order_id = ClientOrderId::from("ENTRY-B");
    episode_b.episode.ts_opened_ns = 2_000;
    let episode_a = strategy
        .exposure
        .managed_position_context()
        .expect("fixture should retain episode A")
        .episode;
    strategy.exposure.reduce(ExposureEvent::SettlementEffect(
        SettlementEffectEvent::ReleaseFlat { episode: episode_a },
    ));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(episode_b.clone())),
        )),
    );
    reduce_position_close_with_projection(
        &strategy,
        PositionEpisodeFingerprint {
            instrument_id,
            position_id,
            opening_order_id: ClientOrderId::from(format!("ENTRY-{position_id}").as_str()),
            ts_opened_ns: 1_000,
        },
        FreshCanonicalPositionProjection::ExactlyOne(Box::new(ClassifiedOpenPosition::Managed(
            episode_b.clone(),
        ))),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.episode == episode_b.episode
    ));
}

fn replacement_conflict_with_working_remainder(
    strategy: &mut BinaryOracleEdgeTaker,
) -> (PositionEpisodeFingerprint, PendingEntryState) {
    let instrument_id = selected_entry_instrument(strategy);
    let position_id = PositionId::from("P-REPLACEMENT-WITH-WORKING-REMAINDER");
    let client_order_id = ClientOrderId::from("ENTRY-P-REPLACEMENT-WITH-WORKING-REMAINDER");
    let open_position = materialize_configured_position(
        strategy,
        instrument_id,
        position_id,
        Quantity::new(5.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut pending = pending_entry_state(strategy, client_order_id);
    pending.instrument_id = instrument_id;
    set_managed_position_with_pending_entry(
        strategy,
        open_position,
        ManagedPositionOrigin::StrategyEntry,
        pending.clone(),
    );
    let retained = strategy
        .exposure
        .managed_position_context()
        .expect("working remainder fixture should be managed");
    assert_eq!(retained.pending_entry.as_ref(), Some(&pending));
    let retained_episode = retained.episode.clone();
    let mut candidate = retained;
    candidate.position_id = PositionId::from("P-REPLACEMENT-CANDIDATE-WITH-REMAINDER");
    candidate.episode.position_id = candidate.position_id;
    candidate.episode.opening_order_id =
        ClientOrderId::from("ENTRY-REPLACEMENT-CANDIDATE-WITH-REMAINDER");
    candidate.episode.ts_opened_ns = 2_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate)),
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));
    (retained_episode, pending)
}

fn assert_working_remainder_stays_occupied(
    strategy: &BinaryOracleEdgeTaker,
    pending: &PendingEntryState,
) {
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(current)
            if current.client_order_id == pending.client_order_id
    ));
    assert_eq!(
        strategy
            .exposure
            .request_entry_operation(strategy.exposure.generation())
            .expect_err("the working entry remainder must keep entry occupied")
            .reason,
        ExposureOperationBlockedReason::PendingEntryOccupied
    );
}

#[test]
fn replacement_conflict_close_then_canonical_none_preserves_working_remainder() {
    let mut strategy = ready_to_trade_strategy();
    let (retained_episode, pending) = replacement_conflict_with_working_remainder(&mut strategy);

    reduce_position_close_with_projection(
        &strategy,
        retained_episode,
        FreshCanonicalPositionProjection::ProbeFailed {
            diagnostic: "replacement projection unavailable after close".to_string(),
        },
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );

    assert_working_remainder_stays_occupied(&strategy, &pending);
}

#[test]
fn replacement_conflict_canonical_none_then_close_preserves_working_remainder() {
    let mut strategy = ready_to_trade_strategy();
    let (retained_episode, pending) = replacement_conflict_with_working_remainder(&mut strategy);

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    reduce_position_close_with_projection(
        &strategy,
        retained_episode,
        FreshCanonicalPositionProjection::None,
    );

    assert_working_remainder_stays_occupied(&strategy, &pending);
}

#[test]
fn replacement_conflict_requires_retained_episode_close_and_matching_candidate() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_a = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-A"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate_b = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate_b.position_id = PositionId::from("P-REPLACEMENT-B");
    candidate_b.episode.position_id = candidate_b.position_id;
    candidate_b.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-B");
    candidate_b.episode.ts_opened_ns = 2_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate_b.clone())),
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));
    reduce_position_close_with_projection(
        &strategy,
        position_a.episode,
        FreshCanonicalPositionProjection::None,
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate_b.clone())),
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.episode == candidate_b.episode
    ));
}

#[test]
fn replacement_conflict_never_adopts_a_candidate_that_is_no_longer_canonical() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_a = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-STABLE-A"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate_b = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate_b.position_id = PositionId::from("P-REPLACEMENT-STALE-B");
    candidate_b.episode.position_id = candidate_b.position_id;
    candidate_b.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-STALE-B");
    candidate_b.episode.ts_opened_ns = 2_000;
    let mut candidate_c = candidate_b.clone();
    candidate_c.position_id = PositionId::from("P-REPLACEMENT-CURRENT-C");
    candidate_c.episode.position_id = candidate_c.position_id;
    candidate_c.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-CURRENT-C");
    candidate_c.episode.ts_opened_ns = 3_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate_b.clone())),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate_c.clone())),
        )),
    );
    let retained_episode = position_a.episode.clone();
    reduce_position_close_with_projection(
        &strategy,
        retained_episode.clone(),
        FreshCanonicalPositionProjection::ExactlyOne(Box::new(ClassifiedOpenPosition::Managed(
            candidate_c.clone(),
        ))),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));
    let ExposureAdoptionCommit {
        outcome,
        replacement_adoption,
    } = strategy
        .exposure
        .reduce(AdoptionCapableExposureEvent::PositionClosed(
            PositionClosedEvent::ObservedWithFreshProjection {
                expected_generation: strategy.exposure.generation(),
                episode: retained_episode.clone(),
                projection: FreshCanonicalPositionProjection::ExactlyOne(Box::new(
                    ClassifiedOpenPosition::Managed(candidate_b.clone()),
                )),
            },
        ));
    assert!(matches!(
        outcome,
        ExposureTransitionOutcome::Applied {
            to: ExposureStateKind::Managed,
            ..
        }
    ));
    let adoption = replacement_adoption
        .expect("the exact retained-close conjunction must return its replacement adoption");
    assert_eq!(adoption.retained_episode, retained_episode);
    assert_eq!(adoption.adopted.episode, candidate_b.episode);
    assert_eq!(
        adoption.cause,
        ReplacementAdoptionCause::CanonicalCloseConjunction
    );
    let retained = strategy
        .exposure
        .managed_position_context()
        .expect("the original candidate should be adopted by the atomic conjunction");
    assert_eq!(retained.episode, candidate_b.episode);
    let mut stale_candidate = retained.clone();
    stale_candidate.position_id = PositionId::from("P-REPLACEMENT-DISAPPEARING-D");
    stale_candidate.episode.position_id = stale_candidate.position_id;
    stale_candidate.episode.opening_order_id =
        ClientOrderId::from("ENTRY-REPLACEMENT-DISAPPEARING-D");
    stale_candidate.episode.ts_opened_ns = 4_000;
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(stale_candidate)),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(retained.clone())),
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.episode == retained.episode
    ));
}

#[test]
fn replacement_conflict_keeps_the_original_candidate_across_unrelated_exactly_one() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-STABLE-RETAINED"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate_b = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate_b.position_id = PositionId::from("P-REPLACEMENT-STABLE-CANDIDATE");
    candidate_b.episode.position_id = candidate_b.position_id;
    candidate_b.episode.opening_order_id =
        ClientOrderId::from("ENTRY-REPLACEMENT-STABLE-CANDIDATE");
    candidate_b.episode.ts_opened_ns = 2_000;
    let mut unrelated_c = candidate_b.clone();
    unrelated_c.position_id = PositionId::from("P-REPLACEMENT-UNRELATED");
    unrelated_c.episode.position_id = unrelated_c.position_id;
    unrelated_c.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-UNRELATED");
    unrelated_c.episode.ts_opened_ns = 3_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate_b.clone())),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(unrelated_c)),
        )),
    );

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(conflict)
            if conflict.candidate.episode == candidate_b.episode
    ));
}

#[test]
fn stale_close_projection_cannot_discharge_a_replacement_conflict() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let retained = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-STALE-GENERATION-A"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate.position_id = PositionId::from("P-REPLACEMENT-STALE-GENERATION-B");
    candidate.episode.position_id = candidate.position_id;
    candidate.episode.opening_order_id =
        ClientOrderId::from("ENTRY-REPLACEMENT-STALE-GENERATION-B");
    candidate.episode.ts_opened_ns = 2_000;
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate.clone())),
        )),
    );
    let stale_generation = strategy.exposure.generation();
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionClosed(
            PositionClosedEvent::ObservedWithFreshProjection {
                expected_generation: stale_generation,
                episode: retained.episode,
                projection: FreshCanonicalPositionProjection::ExactlyOne(Box::new(
                    ClassifiedOpenPosition::Managed(candidate),
                )),
            },
        ),
    );

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));
}

#[test]
fn occupied_replacement_conflict_recovers_after_multiple_with_close_and_matching_candidate() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let retained = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-MULTIPLE-RETAINED"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate.position_id = PositionId::from("P-REPLACEMENT-MULTIPLE-CANDIDATE");
    candidate.episode.position_id = candidate.position_id;
    candidate.episode.opening_order_id =
        ClientOrderId::from("ENTRY-REPLACEMENT-MULTIPLE-CANDIDATE");
    candidate.episode.ts_opened_ns = 2_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate.clone())),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::Multiple {
                count: 2,
                recovery: BlindRecoveryState::authority_free(
                    BlindRecoveryReason::MultipleOpenPositions { count: 2 },
                ),
            },
        )),
    );
    reduce_position_close_with_projection(
        &strategy,
        retained.episode,
        FreshCanonicalPositionProjection::Multiple { count: 2 },
    );
    let ExposureAdoptionCommit {
        outcome: _,
        replacement_adoption,
    } = strategy
        .exposure
        .reduce(AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::ExactlyOne(Box::new(
                    ClassifiedOpenPosition::Managed(candidate.clone()),
                )),
            ),
        ));
    assert!(replacement_adoption.is_some());

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(managed) if managed.episode == candidate.episode
    ));
}

#[test]
fn occupied_replacement_conflict_recovers_after_probe_failure_with_close_and_none() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let retained = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REPLACEMENT-PROBE-RETAINED"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut candidate = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    candidate.position_id = PositionId::from("P-REPLACEMENT-PROBE-CANDIDATE");
    candidate.episode.position_id = candidate.position_id;
    candidate.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-PROBE-CANDIDATE");
    candidate.episode.ts_opened_ns = 2_000;

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate)),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "replacement probe failed".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    reduce_position_close_with_projection(
        &strategy,
        retained.episode,
        FreshCanonicalPositionProjection::ProbeFailed {
            diagnostic: "replacement probe failed after close".to_string(),
        },
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ),
    );

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn unsupported_position_cannot_displace_a_working_entry_before_terminal_proof() {
    let mut strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-UNSUPPORTED-RETAINED-A"),
    );
    let instrument_id = pending.instrument_id;
    set_pending_entry(&mut strategy, pending.clone());
    let unsupported_episode = PositionEpisodeFingerprint {
        instrument_id,
        position_id: PositionId::from("P-UNSUPPORTED-DIVERGENT-B"),
        opening_order_id: ClientOrderId::from("ENTRY-UNSUPPORTED-DIVERGENT-B"),
        ts_opened_ns: 2_000,
    };
    let unsupported = UnsupportedObservedState {
        context: managed_position_context(
            OpenPositionState {
                episode: unsupported_episode.clone(),
                lifecycle: BoltV3PositionMarketLifecycle::missing(),
                instrument_id,
                position_id: unsupported_episode.position_id,
                entry_order_side: OrderSide::Sell,
                side: PositionSide::Long,
                quantity: Quantity::new(1.0, 2),
                avg_px_open: 0.55,
                book: pending.book.clone(),
            },
            ManagedPositionOrigin::RecoveryBootstrap,
            None,
        ),
        reason: UnsupportedObservedReason::LiveUnsupportedContract,
    };

    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::Unsupported(unsupported),
    ));
    reduce_position_close_with_projection(
        &strategy,
        unsupported_episode,
        FreshCanonicalPositionProjection::None,
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );

    assert!(matches!(
        strategy.exposure.pending_entry(),
        Some(retained) if retained.client_order_id == pending.client_order_id
    ));
    assert_eq!(
        strategy
            .exposure
            .request_entry_operation(strategy.exposure.generation())
            .expect_err("retained entry A must block entry C")
            .reason,
        ExposureOperationBlockedReason::BlindRecoveryOccupied
    );

    let managed_a = managed_position_context(
        OpenPositionState {
            episode: PositionEpisodeFingerprint {
                instrument_id,
                position_id: PositionId::from("P-UNSUPPORTED-LATE-FILL-A"),
                opening_order_id: pending.client_order_id,
                ts_opened_ns: 3_000,
            },
            lifecycle: pending.lifecycle.clone(),
            instrument_id,
            position_id: PositionId::from("P-UNSUPPORTED-LATE-FILL-A"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            book: pending.book,
        },
        ManagedPositionOrigin::StrategyEntry,
        None,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::EntryTerminalMaterialization {
            client_order_id: pending.client_order_id,
            managed: managed_a.clone(),
        },
    ));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::ExactlyOne(Box::new(
                    ClassifiedOpenPosition::Managed(managed_a.clone()),
                )),
            ),
        ),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(managed) if managed.episode == managed_a.episode
    ));
}

#[test]
fn replacement_conflict_and_adoption_emit_typed_identity_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let instrument_id = selected_entry_instrument(&strategy);
    let position_a = PositionId::from("P-EVIDENCE-REPLACEMENT-A");
    let position_b = PositionId::from("P-EVIDENCE-REPLACEMENT-B");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_a,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    close_nt_position(&mut strategy, position_a);
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_b,
        Quantity::new(7.0, 2),
        0.47,
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_b,
        Quantity::new(7.0, 2),
        0.47,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));

    strategy.on_position_closed(position_closed_event(instrument_id, position_a));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.position_id == position_b
    ));
    let facts = evidence
        .recorded_facts()
        .expect("typed lifecycle evidence should decode");
    let entry_a = format!("ENTRY-{position_a}");
    let entry_b = format!("ENTRY-{position_b}");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::PositionIdentityConflict
                && record.outcome == OrderLifecycleOutcome::ReplacementConflict
                && record.position_id.as_deref() == Some(position_b.as_str())
                && record.prior_client_order_id.as_deref() == Some(entry_a.as_str())
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ReplacementAdopted
                && record.outcome == OrderLifecycleOutcome::Managed
                && record.position_id.as_deref() == Some(position_b.as_str())
                && record.client_order_id.as_deref() == Some(entry_b.as_str())
    )));
}

#[test]
fn close_first_replacement_adoption_emits_exact_prior_and_adopted_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let instrument_id = selected_entry_instrument(&strategy);
    let position_a = PositionId::from("P-EVIDENCE-CLOSE-FIRST-A");
    let position_b = PositionId::from("P-EVIDENCE-CLOSE-FIRST-B");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_a,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    close_nt_position(&mut strategy, position_a);
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_b,
        Quantity::new(7.0, 2),
        0.47,
        OrderSide::Buy,
    );

    strategy.on_position_closed(position_closed_event(instrument_id, position_a));

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.position_id == position_b
    ));
    let facts = evidence
        .recorded_facts()
        .expect("close-first replacement evidence should decode");
    let entry_a = format!("ENTRY-{position_a}");
    let entry_b = format!("ENTRY-{position_b}");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::PositionIdentityConflict
                && record.outcome == OrderLifecycleOutcome::Managed
                && record.position_id.as_deref() == Some(position_b.as_str())
                && record.prior_client_order_id.as_deref() == Some(entry_a.as_str())
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ReplacementAdopted
                && record.outcome == OrderLifecycleOutcome::Managed
                && record.position_id.as_deref() == Some(position_b.as_str())
                && record.client_order_id.as_deref() == Some(entry_b.as_str())
                && record.prior_client_order_id.as_deref() == Some(entry_a.as_str())
    )));
}

#[test]
fn timer_recovery_replacement_adoption_emits_exact_prior_and_adopted_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    let retained_instrument = selected_entry_instrument(&strategy);
    let occluding_instrument = configured_instrument_except(&strategy, retained_instrument);
    let retained_position = PositionId::from("P-EVIDENCE-TIMER-RETAINED-A");
    let adopted_position = PositionId::from("P-EVIDENCE-TIMER-ADOPTED-B");
    let occluding_position = PositionId::from("P-EVIDENCE-TIMER-OCCLUDING-C");
    materialize_configured_position(
        &mut strategy,
        retained_instrument,
        retained_position,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    close_nt_position(&mut strategy, retained_position);
    seed_nt_open_position(
        &mut strategy,
        retained_instrument,
        adopted_position,
        Quantity::new(7.0, 2),
        0.47,
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        retained_instrument,
        adopted_position,
        Quantity::new(7.0, 2),
        0.47,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ReplacementConflict(_)
    ));

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "replacement recovery probe failed".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    seed_nt_open_position(
        &mut strategy,
        occluding_instrument,
        occluding_position,
        Quantity::new(3.0, 2),
        0.52,
        OrderSide::Buy,
    );
    strategy.on_position_closed(position_closed_event(
        retained_instrument,
        retained_position,
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(_)
    ));

    close_nt_position(&mut strategy, occluding_position);
    strategy.reconcile_blind_recovery_from_fresh_probe(3_000);

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.position_id == adopted_position
    ));
    let facts = evidence
        .recorded_facts()
        .expect("timer replacement evidence should decode");
    let retained_entry = format!("ENTRY-{retained_position}");
    let adopted_entry = format!("ENTRY-{adopted_position}");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ReplacementAdopted
                && record.outcome == OrderLifecycleOutcome::Managed
                && record.source == OrderLifecycleSource::ReconcilePass
                && record.position_id.as_deref() == Some(adopted_position.as_str())
                && record.client_order_id.as_deref() == Some(adopted_entry.as_str())
                && record.prior_client_order_id.as_deref() == Some(retained_entry.as_str())
    )));
}

#[test]
fn pending_entry_identity_conflict_retains_entry_until_its_terminal_fill() {
    let mut strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(&mut strategy, ClientOrderId::from("ENTRY-AUTHORITY-A"));
    set_pending_entry(&mut strategy, pending.clone());
    let instrument_id = pending.instrument_id;
    let candidate = managed_position_context(
        OpenPositionState {
            episode: position_episode_for_test(instrument_id, PositionId::from("P-CONFLICT-B")),
            lifecycle: pending.lifecycle.clone(),
            instrument_id,
            position_id: PositionId::from("P-CONFLICT-B"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(4.0, 2),
            avg_px_open: 0.45,
            book: pending.book.clone(),
        },
        ManagedPositionOrigin::RecoveryBootstrap,
        None,
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(candidate)),
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(current) if current.client_order_id == pending.client_order_id
    ));
    assert!(strategy.exposure.identity_conflict().is_some());

    let entry_a = managed_position_context(
        OpenPositionState {
            episode: position_episode_for_test(instrument_id, PositionId::from("P-ENTRY-A")),
            lifecycle: pending.lifecycle,
            instrument_id,
            position_id: PositionId::from("P-ENTRY-A"),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(4.0, 2),
            avg_px_open: 0.44,
            book: pending.book,
        },
        ManagedPositionOrigin::StrategyEntry,
        None,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::EntryTerminalMaterialization {
            client_order_id: pending.client_order_id,
            managed: entry_a.clone(),
        },
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(context) if context.episode == entry_a.episode
    ));
}

#[test]
fn blind_recovery_raw_truth_never_clears_quarantine_but_fresh_probe_can() {
    let mut strategy = ready_to_trade_strategy();
    set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(_)
    ));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ),
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn occupied_source_blind_recovery_rejects_transient_fresh_none() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RETAINED-BLIND"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "transient probe failure".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            provenance: BlindRecoveryProvenance::ProbeClass {
                retained_authority: Some(_),
            },
            ..
        })
    ));
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "repeated transient probe failure".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            provenance: BlindRecoveryProvenance::ProbeClass {
                retained_authority: Some(ref retained),
            },
            ..
        }) if matches!(**retained, ExposureState::Managed(_))
    ));
}

#[test]
fn every_blind_recovery_reason_rejects_raw_truth_and_uses_its_authorized_class() {
    let template = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&template);
    let episode = position_episode_for_test(instrument_id, PositionId::from("P-BLIND-CENSUS"));
    let managed = managed_position_context(
        OpenPositionState {
            episode: episode.clone(),
            lifecycle: BoltV3PositionMarketLifecycle::missing(),
            instrument_id,
            position_id: episode.position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            book: OutcomeBookState::from_instrument_id(instrument_id),
        },
        ManagedPositionOrigin::RecoveryBootstrap,
        None,
    );
    let recoveries = vec![
        BlindRecoveryState::with_recorded_episode(
            BlindRecoveryReason::InvalidBootstrappedPosition {
                entry_order_side: OrderSide::Buy,
                side: PositionSide::Flat,
            },
            episode.clone(),
        ),
        BlindRecoveryState::with_recorded_episode(
            BlindRecoveryReason::InvalidLivePosition {
                entry_order_side: OrderSide::Buy,
                side: Some(PositionSide::Flat),
            },
            episode.clone(),
        ),
        BlindRecoveryState::with_recorded_episode(
            BlindRecoveryReason::DivergentUnsupportedPosition,
            episode.clone(),
        ),
        BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
        BlindRecoveryState::authority_free(BlindRecoveryReason::MultipleOpenPositions { count: 2 }),
        BlindRecoveryState::authority_free(BlindRecoveryReason::SettlementEvidenceRecoveryFailed),
        BlindRecoveryState::restart_adoption(
            BlindRecoveryReason::AmbiguousRestartOpenExitOrders {
                instrument_id,
                count: 2,
            },
            instrument_id,
            vec![
                ClientOrderId::from("EXIT-BLIND-AMBIGUOUS-A"),
                ClientOrderId::from("EXIT-BLIND-AMBIGUOUS-B"),
            ],
        ),
        BlindRecoveryState::restart_adoption(
            BlindRecoveryReason::UnattributedRestartOpenExitOrder { instrument_id },
            instrument_id,
            vec![ClientOrderId::from("EXIT-BLIND-UNATTRIBUTED")],
        ),
        BlindRecoveryState::authority_free(BlindRecoveryReason::ForeignVenuePosition {
            instrument_id,
            instrument_venue: instrument_id.venue,
            execution_venue: Venue::from("OTHER"),
        }),
    ];

    for recovery in recoveries {
        let strategy = ready_to_trade_strategy();
        let authorized_projection = match recovery.provenance {
            BlindRecoveryProvenance::IdentityBearing { .. }
            | BlindRecoveryProvenance::RestartAdoption { .. } => {
                FreshCanonicalPositionProjection::ExactlyOne(Box::new(
                    ClassifiedOpenPosition::Managed(managed.clone()),
                ))
            }
            BlindRecoveryProvenance::ProbeClass { .. }
            | BlindRecoveryProvenance::ForeignVenue { .. } => {
                FreshCanonicalPositionProjection::None
            }
        };
        strategy.exposure.reduce(ExposureEvent::PositionTruth(
            PositionTruthEvent::BlindRecovery(recovery),
        ));
        strategy.exposure.reduce_without_replacement_adoption(
            AdoptionCapableExposureEvent::PositionTruth(
                AdoptionCapablePositionTruthEvent::Canonical(
                    CanonicalPositionProjection::ExactlyOne(Box::new(managed.clone())),
                ),
            ),
        );
        strategy.exposure.reduce_without_replacement_adoption(
            AdoptionCapableExposureEvent::PositionTruth(
                AdoptionCapablePositionTruthEvent::Canonical(CanonicalPositionProjection::None),
            ),
        );
        assert!(matches!(
            strategy.exposure.state(),
            ExposureState::BlindRecovery(_)
        ));

        strategy.exposure.reduce_without_replacement_adoption(
            AdoptionCapableExposureEvent::PositionTruth(
                AdoptionCapablePositionTruthEvent::AuthorizedRecovery(authorized_projection),
            ),
        );
        assert!(matches!(
            strategy.exposure.state(),
            ExposureState::Flat | ExposureState::Managed(_)
        ));
    }
}

#[test]
fn occupied_blind_recovery_accumulates_matching_entry_and_exit_terminal_proofs() {
    let mut entry_strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(
        &mut entry_strategy,
        ClientOrderId::from("ENTRY-BLIND-RETAINED"),
    );
    set_pending_entry(&mut entry_strategy, pending);
    entry_strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "entry probe failed".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    entry_strategy
        .exposure
        .reduce(ExposureEvent::EntryLifecycle(
            EntryLifecycleEvent::ReleaseFlat,
        ));
    assert!(matches!(
        entry_strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            provenance: BlindRecoveryProvenance::ProbeClass {
                retained_authority: Some(ref retained),
            },
            ..
        }) if matches!(**retained, ExposureState::Flat)
    ));
    entry_strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ),
    );
    assert!(matches!(
        entry_strategy.exposure.state(),
        ExposureState::Flat
    ));

    let mut exit_strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&exit_strategy);
    let position = materialize_configured_position(
        &mut exit_strategy,
        instrument_id,
        PositionId::from("P-BLIND-RETAINED-EXIT"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut exit_strategy,
        position,
        ClientOrderId::from("EXIT-BLIND-RETAINED"),
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = exit_strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should retain exit authority");
    exit_strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ProbeFailed {
                diagnostic: "exit probe failed".to_string(),
                recovery: BlindRecoveryState::authority_free(BlindRecoveryReason::CacheProbeFailed),
            },
        )),
    );
    exit_strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::TerminalAwaitingPosition(exit),
    ));
    assert!(matches!(
        exit_strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            provenance: BlindRecoveryProvenance::ProbeClass {
                retained_authority: Some(ref retained),
            },
            ..
        }) if matches!(**retained, ExposureState::TerminalExitAwaitingPosition(_))
    ));
    exit_strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    assert!(matches!(
        exit_strategy.exposure.state(),
        ExposureState::BlindRecovery(_)
    ));
    exit_strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ),
    );
    assert!(matches!(
        exit_strategy.exposure.state(),
        ExposureState::Flat
    ));
}

#[test]
fn entry_route_grant_unwinds_each_phase_and_sink_unknown_discharge_is_typed() {
    let mut strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(&mut strategy, ClientOrderId::from("ENTRY-GRANT-PHASES"));

    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    assert_eq!(grant.generation(), strategy.exposure.generation());
    drop(grant);
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("rolled-back entry slot should grant again");
    let mut participant = grant
        .bind(pending.clone())
        .expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume at the final pre-sink boundary");
    drop(participant);
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("post-consumption unwind should restore the entry slot");
    let mut participant = grant
        .bind(pending.clone())
        .expect("entry payload should bind for successful consumption");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume successfully");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    participant.complete(BoltV3RouteAttemptCompletion::Submitted);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(current) if current.client_order_id == pending.client_order_id
    ));
    strategy.exposure.reduce(ExposureEvent::EntryLifecycle(
        EntryLifecycleEvent::ReleaseFlat,
    ));

    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("successful entry completion should leave the entry slot governed");
    let mut participant = grant
        .bind(pending.clone())
        .expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(OperationSinkUnknownState {
            operation: ExposureOperationKind::EntryRoute,
            ..
        })
    ));
    assert!(strategy.exposure.blocks_new_entries());
    assert!(matches!(
        strategy.exposure.last_outcome(),
        ExposureTransitionOutcome::Applied {
            to: ExposureStateKind::OperationSinkUnknown,
            ..
        }
    ));
    strategy.exposure.reduce(ExposureEvent::TimerReconciliation(
        TimerReconciliationEvent::SinkUnknown(SinkUnknownResolution::Submitted),
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(current) if current.client_order_id == pending.client_order_id
    ));
}

#[test]
fn sink_unknown_requires_proof_and_discharges_terminal_and_filled_outcomes() {
    let mut terminal_strategy = ready_to_trade_strategy();
    let terminal_pending = pending_entry_state(
        &mut terminal_strategy,
        ClientOrderId::from("ENTRY-SINK-UNKNOWN-TERMINAL"),
    );
    let grant = terminal_strategy
        .exposure
        .request_entry_operation(terminal_strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    let mut participant = grant
        .bind(terminal_pending)
        .expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    assert!(matches!(
        terminal_strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));
    terminal_strategy
        .exposure
        .reduce(ExposureEvent::TimerReconciliation(
            TimerReconciliationEvent::SinkUnknown(SinkUnknownResolution::Terminal {
                residual: None,
            }),
        ));
    assert!(matches!(
        terminal_strategy.exposure.state(),
        ExposureState::Flat
    ));

    let mut filled_strategy = ready_to_trade_strategy();
    let filled_pending = pending_entry_state(
        &mut filled_strategy,
        ClientOrderId::from("ENTRY-SINK-UNKNOWN-FILLED"),
    );
    let instrument_id = filled_pending.instrument_id;
    let book = filled_pending.book.clone();
    let lifecycle = filled_pending.lifecycle.clone();
    let grant = filled_strategy
        .exposure
        .request_entry_operation(filled_strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    let mut participant = grant
        .bind(filled_pending)
        .expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    let position_id = PositionId::from("P-SINK-UNKNOWN-FILLED");
    let filled = managed_position_context(
        OpenPositionState {
            episode: position_episode_for_test(instrument_id, position_id),
            lifecycle,
            instrument_id,
            position_id,
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(4.0, 2),
            avg_px_open: 0.45,
            book,
        },
        ManagedPositionOrigin::StrategyEntry,
        None,
    );
    filled_strategy
        .exposure
        .reduce(ExposureEvent::TimerReconciliation(
            TimerReconciliationEvent::SinkUnknown(SinkUnknownResolution::Filled {
                managed: filled.clone(),
            }),
        ));
    assert!(matches!(
        filled_strategy.exposure.state(),
        ExposureState::Managed(current) if current.episode == filled.episode
    ));

    let mut quarantined_strategy = ready_to_trade_strategy();
    let quarantined_pending = pending_entry_state(
        &mut quarantined_strategy,
        ClientOrderId::from("ENTRY-SINK-UNKNOWN-QUARANTINED"),
    );
    let grant = quarantined_strategy
        .exposure
        .request_entry_operation(quarantined_strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    let mut participant = grant
        .bind(quarantined_pending)
        .expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    quarantined_strategy
        .exposure
        .reduce_without_replacement_adoption(AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::Canonical(
                CanonicalPositionProjection::ProbeFailed {
                    diagnostic: "sink-unknown probe failed".to_string(),
                    recovery: BlindRecoveryState::authority_free(
                        BlindRecoveryReason::CacheProbeFailed,
                    ),
                },
            ),
        ));
    quarantined_strategy
        .exposure
        .reduce(ExposureEvent::TimerReconciliation(
            TimerReconciliationEvent::SinkUnknown(SinkUnknownResolution::ProvenAbsent),
        ));
    assert!(matches!(
        quarantined_strategy.exposure.state(),
        ExposureState::BlindRecovery(BlindRecoveryState {
            provenance: BlindRecoveryProvenance::ProbeClass {
                retained_authority: Some(ref retained),
            },
            ..
        }) if matches!(**retained, ExposureState::Flat)
    ));
    quarantined_strategy
        .exposure
        .reduce_without_replacement_adoption(AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthorizedRecovery(
                FreshCanonicalPositionProjection::None,
            ),
        ));
    assert!(matches!(
        quarantined_strategy.exposure.state(),
        ExposureState::Flat
    ));
}

#[test]
fn sink_unknown_denial_proof_discharges_through_production_reconciliation_with_typed_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let client_order_id = ClientOrderId::from("ENTRY-SINK-UNKNOWN-DENIAL-PROOF");
    let pending = pending_entry_state(&mut strategy, client_order_id);
    let instrument_id = pending.instrument_id;
    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    let mut participant = grant.bind(pending).expect("entry payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("entry participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);

    strategy.reconcile_operation_sink_unknown_on_timer();
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));
    strategy.on_order_denied(order_denied_event_with_reason(
        ClientOrderId::from("ENTRY-SINK-UNKNOWN-FOREIGN-DENIAL"),
        instrument_id,
        "foreign denial",
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(_)
    ));

    strategy.on_order_denied(order_denied_event_with_reason(
        client_order_id,
        instrument_id,
        "risk-engine denial proves no venue submission",
    ));
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    let facts = evidence
        .recorded_facts()
        .expect("sink-unknown lifecycle evidence should decode");
    let instrument_id_text = instrument_id.to_string();
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownEntered
                && record.outcome == OrderLifecycleOutcome::OperationSinkUnknown
                && record.instrument_id.as_deref() == Some(instrument_id_text.as_str())
                && record.client_order_id.as_deref() == Some(client_order_id.as_str())
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownResolved
                && record.outcome == OrderLifecycleOutcome::Flat
                && record.instrument_id.as_deref() == Some(instrument_id_text.as_str())
                && record.client_order_id.as_deref() == Some(client_order_id.as_str())
    )));
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::OperationSinkUnknownResolved
                && record.client_order_id.as_deref()
                    == Some("ENTRY-SINK-UNKNOWN-FOREIGN-DENIAL")
    )));
}

#[test]
fn canonical_none_close_conjunction_releases_without_awaiting_evidence() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-CANONICAL-NONE-CLOSE-CONJUNCTION");
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(6.0, 2),
        0.47,
        OrderSide::Buy,
        PositionSide::Long,
    );
    close_nt_position(&mut strategy, position_id);

    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
    let facts = evidence
        .recorded_facts()
        .expect("close-conjunction evidence should decode");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::PositionClosed
                && record.outcome == OrderLifecycleOutcome::Flat
                && record.position_id.as_deref() == Some(position_id.as_str())
    )));
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::CanonicalPositionAwaiting
                && record.position_id.as_deref() == Some(position_id.as_str())
    )));
}

#[test]
fn canonical_multiple_and_transient_none_emit_typed_health_without_adopting_event_payloads() {
    let multiple_evidence = recording_decision_evidence();
    let mut multiple = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        multiple_evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(
                multiple_evidence.clone(),
            ),
        ),
    );
    let instrument_id = selected_entry_instrument(&multiple);
    let position_a = PositionId::from("P-CANONICAL-MULTIPLE-A");
    let position_b = PositionId::from("P-CANONICAL-MULTIPLE-B");
    seed_nt_open_position(
        &mut multiple,
        instrument_id,
        position_a,
        Quantity::new(4.0, 2),
        0.45,
        OrderSide::Buy,
    );
    seed_nt_open_position(
        &mut multiple,
        instrument_id,
        position_b,
        Quantity::new(5.0, 2),
        0.46,
        OrderSide::Buy,
    );
    multiple.on_position_opened(position_opened_event(
        instrument_id,
        position_a,
        Quantity::new(4.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert!(matches!(
        multiple.exposure.state(),
        ExposureState::BlindRecovery(_)
    ));
    assert!(multiple.exposure.managed_position_context().is_none());
    assert!(
        multiple_evidence
            .recorded_facts()
            .expect("canonical multiplicity evidence should decode")
            .iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::OrderLifecycle(record)
                    if record.transition == OrderLifecycleTransition::CanonicalPositionMultiplicity
                        && record.outcome == OrderLifecycleOutcome::BlindRecovery
            ))
    );

    let awaiting_evidence = recording_decision_evidence();
    let mut awaiting = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        awaiting_evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(
                awaiting_evidence.clone(),
            ),
        ),
    );
    let awaiting_position = PositionId::from("P-CANONICAL-AWAITING");
    materialize_configured_position(
        &mut awaiting,
        instrument_id,
        awaiting_position,
        Quantity::new(6.0, 2),
        0.47,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let retained_episode = awaiting
        .exposure
        .managed_position_context()
        .expect("fixture should govern the managed episode")
        .episode;
    close_nt_position(&mut awaiting, awaiting_position);
    awaiting.on_position_opened(position_opened_event(
        instrument_id,
        awaiting_position,
        Quantity::new(6.0, 2),
        0.47,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert!(matches!(
        awaiting.exposure.state(),
        ExposureState::Managed(context) if context.episode == retained_episode
    ));
    assert!(
        awaiting_evidence
            .recorded_facts()
            .expect("canonical awaiting evidence should decode")
            .iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::OrderLifecycle(record)
                    if record.transition == OrderLifecycleTransition::CanonicalPositionAwaiting
                        && record.outcome == OrderLifecycleOutcome::Managed
                        && record.position_id.as_deref() == Some(awaiting_position.as_str())
            ))
    );

    let flat_evidence = recording_decision_evidence();
    let mut flat = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        flat_evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(flat_evidence.clone()),
        ),
    );
    flat.on_position_opened(position_opened_event(
        instrument_id,
        PositionId::from("P-CANONICAL-NONE-FLAT-CONTROL"),
        Quantity::new(1.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert!(matches!(flat.exposure.state(), ExposureState::Flat));
    assert!(
        !flat_evidence
            .recorded_facts()
            .expect("flat control evidence should decode")
            .iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::OrderLifecycle(record)
                    if record.transition == OrderLifecycleTransition::CanonicalPositionAwaiting
            ))
    );
}

#[test]
fn position_close_with_canonical_none_keeps_active_exit_and_records_awaiting_health() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-CLOSE-NONE-ACTIVE-EXIT");
    let client_order_id = ClientOrderId::from("EXIT-CLOSE-NONE-ACTIVE");
    let position = materialize_configured_position(
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
        position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    close_nt_position(&mut strategy, position_id);
    strategy.on_position_closed(position_closed_event(instrument_id, position_id));

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(exit) if exit.client_order_id() == client_order_id
    ));
    assert!(
        evidence
            .recorded_facts()
            .expect("close-event awaiting evidence should decode")
            .iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::OrderLifecycle(record)
                    if record.transition == OrderLifecycleTransition::CanonicalPositionAwaiting
                        && record.outcome == OrderLifecycleOutcome::ExitPending
                        && record.position_id.as_deref() == Some(position_id.as_str())
                        && record.client_order_id.as_deref() == Some(client_order_id.as_str())
            ))
    );
}

#[test]
fn exit_route_grant_unwinds_pre_sink_and_enters_exit_tagged_sink_unknown() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-EXIT-GRANT-PHASES"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-GRANT-PHASES"),
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should build sealed exit authority");
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));

    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("managed exposure should grant a provisional exit");
    drop(grant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));

    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("managed exposure should grant exit");
    let generation = grant.generation();
    let attempt = ExitAttemptingState {
        generation,
        managed: exit.position.clone(),
        pending_exit: exit.pending_exit.clone(),
        authority: exit.authority.clone(),
    };
    let mut participant = grant
        .bind(attempt.clone())
        .expect("exit payload should bind");
    participant
        .consume_at_pre_sink()
        .expect("exit participant should consume");
    drop(participant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));

    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("post-consumption unwind should restore the exit slot");
    let generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation,
            ..attempt.clone()
        })
        .expect("exit payload should bind for successful consumption");
    participant
        .consume_at_pre_sink()
        .expect("exit participant should consume successfully");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    participant.complete(BoltV3RouteAttemptCompletion::Submitted);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ExitPending(_)
    ));
    strategy
        .exposure
        .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
            exit.position.clone(),
        )));

    let grant = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect("pre-sink unwind should restore exit grantability");
    let generation = grant.generation();
    let mut participant = grant
        .bind(ExitAttemptingState {
            generation,
            ..attempt
        })
        .expect("exit payload should bind again");
    participant
        .consume_at_pre_sink()
        .expect("exit participant should consume");
    participant
        .mark_sink_invoked(0)
        .expect("test participant should reach the sink");
    drop(participant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::OperationSinkUnknown(OperationSinkUnknownState {
            operation: ExposureOperationKind::ExitRoute,
            ..
        })
    ));
    strategy.exposure.reduce(ExposureEvent::TimerReconciliation(
        TimerReconciliationEvent::SinkUnknown(SinkUnknownResolution::ProvenAbsent),
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));
}

#[test]
fn exit_sink_unknown_terminal_callbacks_use_sealed_fill_authority_before_remanaging() {
    fn enter_unknown_exit(
        suffix: &str,
    ) -> (
        BinaryOracleEdgeTaker,
        InstrumentId,
        PositionId,
        ClientOrderId,
    ) {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from(format!("P-SINK-UNKNOWN-EXIT-{suffix}").as_str());
        let client_order_id = ClientOrderId::from(format!("EXIT-SINK-UNKNOWN-{suffix}").as_str());
        let position = materialize_configured_position(
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
            position,
            client_order_id,
            ManagedPositionOrigin::StrategyEntry,
        );
        let exit = strategy
            .exposure
            .exit_pending_snapshot()
            .expect("fixture should create a sealed exit authority");
        strategy
            .exposure
            .reduce(ExposureEvent::ExitLifecycle(ExitLifecycleEvent::Residual(
                exit.position.clone(),
            )));
        let grant = strategy
            .exposure
            .request_exit_operation(strategy.exposure.generation())
            .expect("managed exposure should grant exit");
        let generation = grant.generation();
        let mut participant = grant
            .bind(ExitAttemptingState {
                generation,
                managed: exit.position,
                pending_exit: exit.pending_exit,
                authority: exit.authority,
            })
            .expect("exit attempt should bind");
        participant
            .consume_at_pre_sink()
            .expect("exit attempt should consume");
        participant
            .mark_sink_invoked(0)
            .expect("test participant should reach the sink");
        drop(participant);
        assert!(matches!(
            strategy.exposure.state(),
            ExposureState::OperationSinkUnknown(OperationSinkUnknownState {
                operation: ExposureOperationKind::ExitRoute,
                ..
            })
        ));
        (strategy, instrument_id, position_id, client_order_id)
    }

    let (mut partial, instrument_id, position_id, client_order_id) = enter_unknown_exit("PARTIAL");
    let mut fill = order_filled_event(
        client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    fill.last_qty = Quantity::new(4.0, 2);
    fill.trade_id = TradeId::from("TRADE-SINK-UNKNOWN-EXIT-PARTIAL");
    apply_exit_order_event_to_nt_cache(
        &mut partial,
        nautilus_model::events::OrderEventAny::Filled(fill.clone()),
    );
    partial.on_order_filled(&fill);
    let expired = order_expired_event(client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut partial,
        nautilus_model::events::OrderEventAny::Expired(expired.clone()),
    );
    partial.on_order_expired(expired);
    assert!(matches!(
        partial.exposure.state(),
        ExposureState::TerminalExitAwaitingPosition(_)
    ));
    assert!(partial.managed_position().is_some());

    let (mut zero_fill, instrument_id, position_id, client_order_id) =
        enter_unknown_exit("ZERO-FILL");
    let canceled = order_canceled_event(client_order_id, instrument_id);
    apply_exit_order_event_to_nt_cache(
        &mut zero_fill,
        nautilus_model::events::OrderEventAny::Canceled(canceled.clone()),
    );
    zero_fill.on_order_canceled(&canceled);
    assert!(matches!(
        zero_fill.exposure.state(),
        ExposureState::Managed(context) if context.position_id == position_id
    ));
    assert_eq!(
        managed_position_snapshot(&zero_fill).map(|position| position.quantity),
        Some(Quantity::new(10.0, 2))
    );
}

#[test]
fn bootstrap_and_correction_grants_commit_atomically_and_unwind_exactly() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-GOVERNED-COMMITS"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let managed = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be managed");
    strategy.exposure.reduce(ExposureEvent::SettlementEffect(
        SettlementEffectEvent::ReleaseFlat {
            episode: managed.episode.clone(),
        },
    ));
    let generation = strategy.exposure.generation();
    let grant = strategy
        .exposure
        .request_bootstrap_operation(generation)
        .expect("flat exposure should grant bootstrap");
    drop(grant);
    assert_eq!(strategy.exposure.generation(), generation);
    let grant = strategy
        .exposure
        .request_bootstrap_operation(generation)
        .expect("bootstrap unwind should restore exact generation");
    grant.commit(BootstrapAdoptionEvent::Managed(managed));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));
    let generation = strategy.exposure.generation();
    let grant = strategy
        .exposure
        .request_bootstrap_operation(generation)
        .expect("managed exposure should provisionally grant bootstrap");
    let preserved = grant.commit(BootstrapAdoptionEvent::Managed(
        strategy
            .exposure
            .managed_position_context()
            .expect("managed context should remain available"),
    ));
    assert!(matches!(
        preserved,
        ExposureTransitionOutcome::Preserved {
            state: ExposureStateKind::Managed
        }
    ));
    assert_eq!(strategy.exposure.generation(), generation);
    drop(
        strategy
            .exposure
            .request_bootstrap_operation(generation)
            .expect("a preserved bootstrap transition must unwind its provisional arm"),
    );

    let generation = strategy.exposure.generation();
    let correction = strategy
        .exposure
        .request_correction_operation(generation)
        .expect("managed exposure should grant correction");
    drop(correction);
    assert_eq!(strategy.exposure.generation(), generation);
    let correction = strategy
        .exposure
        .request_correction_operation(generation)
        .expect("correction unwind should restore exact generation");
    let preserved = correction.commit(ExitLifecycleEvent::ReleaseFlat);
    assert!(matches!(
        preserved,
        ExposureTransitionOutcome::Preserved {
            state: ExposureStateKind::Managed
        }
    ));
    let generation = strategy.exposure.generation();
    let correction = strategy
        .exposure
        .request_correction_operation(generation)
        .expect("preserved correction should unwind its provisional arm");
    let preserved = correction.commit(ExitLifecycleEvent::ReleaseFlat);
    assert!(matches!(
        preserved,
        ExposureTransitionOutcome::Preserved {
            state: ExposureStateKind::Managed
        }
    ));
    assert_eq!(strategy.exposure.generation(), generation);
    drop(
        strategy
            .exposure
            .request_correction_operation(generation)
            .expect("a preserved correction transition must unwind its provisional arm"),
    );

    let correction = strategy
        .exposure
        .request_correction_operation(strategy.exposure.generation())
        .expect("managed exposure should grant a callback-race correction");
    let refreshed = strategy
        .exposure
        .managed_position_context()
        .expect("managed context should remain available");
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(refreshed)),
        )),
    );
    let stale = correction.commit(ExitLifecycleEvent::ReleaseFlat);
    assert!(matches!(
        stale,
        ExposureTransitionOutcome::Preserved {
            state: ExposureStateKind::Managed
        }
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));
}

#[test]
fn synchronous_callback_wins_over_late_grant_drop() {
    let mut strategy = ready_to_trade_strategy();
    let pending = pending_entry_state(&mut strategy, ClientOrderId::from("ENTRY-CALLBACK-WINS"));
    let grant = strategy
        .exposure
        .request_entry_operation(strategy.exposure.generation())
        .expect("flat exposure should grant entry");
    strategy.exposure.reduce(ExposureEvent::EntryLifecycle(
        EntryLifecycleEvent::RestorePending(pending.clone()),
    ));
    drop(grant);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::PendingEntry(current) if current.client_order_id == pending.client_order_id
    ));
}

#[test]
fn recovery_hold_rejects_exit_with_exact_typed_cause_while_managed_control_grants() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-HOLD-BLOCK"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let managed_generation = strategy.exposure.generation();
    let control = strategy
        .exposure
        .request_exit_operation(managed_generation)
        .expect("managed control must grant");
    drop(control);
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-HOLD-BLOCK"),
        ManagedPositionOrigin::StrategyEntry,
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should retain exit authority");
    register_test_strategy(&mut strategy).borrow_mut().reset();
    strategy.enter_exit_authority_recovery_hold(
        exit.position,
        exit.pending_exit,
        ExitAuthorityRecoveryPlan::Resume(exit.authority),
        1_000,
    );
    let rejection = strategy
        .exposure
        .request_exit_operation(strategy.exposure.generation())
        .expect_err("recovery hold must reject a second exit");
    assert_eq!(
        rejection.reason,
        ExposureOperationBlockedReason::RecoveryHoldOccupied
    );
    let decision = strategy.exit_intent_decision_at(1_200);
    assert_eq!(
        decision.blocked_reason,
        Some(EvidenceExitBlockedReason::RecoveryHoldOccupied)
    );
    strategy
        .record_exit_intent_or_hold_once(
            1_200,
            ExitEvaluationTriggerContext::unknown(1_200),
            &decision,
        )
        .expect("hold-occupied decision evidence should record");
    assert!(
        evidence
            .recorded_facts()
            .expect("typed hold evidence should decode")
            .into_iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::ExitHoldDecision(record)
                    if record.blocked_reason
                        == Some(EvidenceExitBlockedReason::RecoveryHoldOccupied)
                        && record.outcome == ExitHoldOutcome::Blocked
            ))
    );

    let mut startup_strategy = ready_to_trade_strategy();
    let startup_instrument_id = selected_entry_instrument(&startup_strategy);
    materialize_configured_position(
        &mut startup_strategy,
        startup_instrument_id,
        PositionId::from("P-STARTUP-HOLD-BLOCK"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let startup_position = startup_strategy
        .exposure
        .managed_position_context()
        .expect("startup fixture position should be managed");
    let startup_exit_id = ClientOrderId::from("EXIT-STARTUP-HOLD-BLOCK");
    register_test_strategy(&mut startup_strategy)
        .borrow_mut()
        .reset();
    startup_strategy.enter_exit_authority_recovery_hold(
        startup_position,
        PendingExitState {
            submitted_at_ms: None,
        },
        ExitAuthorityRecoveryPlan::Reconstruct {
            cause: BoltV3RecoveredExitCause::StartupAdoption,
            client_order_id: startup_exit_id,
        },
        1_000,
    );
    assert_eq!(
        startup_strategy
            .exposure
            .request_exit_operation(startup_strategy.exposure.generation())
            .expect_err("startup-created recovery hold must reject exit")
            .reason,
        ExposureOperationBlockedReason::RecoveryHoldOccupied
    );
}

#[test]
fn book_delta_trigger_records_runtime_and_startup_recovery_holds_without_nt_projection() {
    fn assert_trigger(cause: BoltV3RecoveredExitCause, suffix: &str) {
        let evidence = recording_decision_evidence();
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            evidence.clone(),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
            ),
        );
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("P-TRIGGER-HOLD-{suffix}").as_str()),
            Quantity::new(10.0, 2),
            0.45,
            OrderSide::Buy,
            PositionSide::Long,
        );
        let client_order_id = ClientOrderId::from(format!("EXIT-TRIGGER-HOLD-{suffix}").as_str());
        set_exit_pending(
            &mut strategy,
            position,
            client_order_id,
            ManagedPositionOrigin::StrategyEntry,
        );
        let exit = strategy
            .exposure
            .exit_pending_snapshot()
            .expect("fixture should retain exit authority");
        register_test_strategy(&mut strategy).borrow_mut().reset();
        strategy.enter_exit_authority_recovery_hold(
            exit.position,
            exit.pending_exit,
            ExitAuthorityRecoveryPlan::Reconstruct {
                cause,
                client_order_id,
            },
            1_000,
        );
        assert!(strategy.managed_position().is_none());

        strategy
            .on_book_deltas(&book_deltas(
                instrument_id,
                &[(BookAction::Update, OrderSide::Sell, 0.44, 500.0)],
            ))
            .expect("book-delta trigger should contain the typed exit hold");

        assert!(
            evidence
                .recorded_facts()
                .expect("book-delta hold evidence should decode")
                .into_iter()
                .any(|fact| matches!(
                    fact,
                    CurrentFact::ExitHoldDecision(record)
                        if record.blocked_reason
                            == Some(EvidenceExitBlockedReason::RecoveryHoldOccupied)
                            && record.outcome == ExitHoldOutcome::Blocked
                            && record.details.exit_trigger_source
                                == EvidenceExitTriggerSource::BookDelta
                ))
        );
    }

    assert_trigger(BoltV3RecoveredExitCause::FillVoidReopen, "RUNTIME");
    assert_trigger(BoltV3RecoveredExitCause::StartupAdoption, "STARTUP");
}

#[test]
fn historical_exit_obligations_compact_duplicates_and_saturate_without_eviction() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-OBLIGATION-BOUND"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let client_order_id = ClientOrderId::from("EXIT-OBLIGATION-BOUND");
    set_exit_pending(
        &mut strategy,
        position,
        client_order_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    let correction = |index: u64| HistoricalExitCorrection {
        client_order_id,
        instrument_id,
        trade_id: nautilus_model::identifiers::TradeId::from(
            format!("TRADE-OBLIGATION-{index}").as_str(),
        ),
        voided_quantity: Quantity::new(1.0, 2),
        ts_event_ns: 1_000 + index,
    };
    strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
        UntrackedOrderEvent::HistoricalExitCorrection(correction(0)),
    ));
    strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
        UntrackedOrderEvent::HistoricalExitCorrection(correction(0)),
    ));
    assert_eq!(
        strategy
            .exposure
            .deferred_obligation(&client_order_id)
            .expect("historical attribution should create an obligation")
            .history
            .len(),
        1
    );
    for index in 1..256 {
        strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
            UntrackedOrderEvent::HistoricalExitCorrection(correction(index)),
        ));
    }
    assert_eq!(
        strategy
            .exposure
            .deferred_obligation(&client_order_id)
            .expect("bounded obligation must not evict history")
            .history
            .len(),
        256
    );
    strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
        UntrackedOrderEvent::HistoricalExitCorrection(correction(256)),
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            client_order_id: saturated,
            ..
        }) if saturated == client_order_id
    ));
    for index in 257..512 {
        strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
            UntrackedOrderEvent::HistoricalExitCorrection(correction(index)),
        ));
    }
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            retained,
            client_order_id: saturated,
            obligation_count: 1,
        }) if saturated == client_order_id
            && !matches!(*retained, ExposureState::ObligationSaturated(_))
    ));
}

#[test]
fn historical_exit_deferral_and_capacity_are_loud_through_fill_void_handler() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-HISTORICAL-EVIDENCE");
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let released_exit_id = ClientOrderId::from("EXIT-HISTORICAL-EVIDENCE");
    set_exit_pending(
        &mut strategy,
        position,
        released_exit_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-HISTORICAL-EVIDENCE-BLOCKER"),
    );
    set_pending_entry(&mut strategy, pending.clone());

    let configured_history_limit = strategy
        .exposure
        .limits()
        .max_history_events_per_obligation
        .get() as usize;
    for index in 0..=configured_history_limit {
        strategy.on_order_fill_voided(&order_fill_voided_event(
            released_exit_id,
            instrument_id,
            position_id,
            TradeId::from(format!("TRADE-HISTORICAL-EVIDENCE-{index}").as_str()),
            Quantity::new(1.0, 2),
            10_000 + index as u64,
            OrderSide::Sell,
        ));
    }
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            retained,
            client_order_id,
            ..
        }) if client_order_id == released_exit_id
            && matches!(*retained, ExposureState::PendingEntry(ref current)
                if current.client_order_id == pending.client_order_id)
    ));
    let facts = evidence
        .recorded_facts()
        .expect("historical obligation evidence should decode");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::HistoricalExitCorrectionDeferred
                && record.outcome == OrderLifecycleOutcome::PendingEntry
                && record.client_order_id.as_deref() == Some(released_exit_id.as_str())
                && record.position_id.as_deref() == Some(position_id.as_str())
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ExposureObligationSaturated
                && record.outcome == OrderLifecycleOutcome::ObligationSaturated
                && record.client_order_id.as_deref() == Some(released_exit_id.as_str())
                && record.position_id.as_deref() == Some(position_id.as_str())
    )));
}

#[test]
fn historical_fill_observation_capacity_is_loud_through_production_handler() {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-HISTORICAL-FILL-CAPACITY");
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let released_exit_id = ClientOrderId::from("EXIT-HIST-FILL-CAP");
    set_exit_pending(
        &mut strategy,
        position,
        released_exit_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-HIST-FILL-CAP-BLOCK"),
    );
    set_pending_entry(&mut strategy, pending);
    strategy.on_order_fill_voided(&order_fill_voided_event(
        released_exit_id,
        instrument_id,
        position_id,
        TradeId::from("T-HFC-CORRECTION"),
        Quantity::new(1.0, 2),
        10_000,
        OrderSide::Sell,
    ));
    let configured_history_limit = strategy
        .exposure
        .limits()
        .max_history_events_per_obligation
        .get() as usize;
    for index in 0..configured_history_limit {
        let mut fill = order_filled_event(
            released_exit_id,
            instrument_id,
            Some(position_id),
            OrderSide::Sell,
        );
        fill.trade_id = TradeId::from(format!("T-HFC-{index}").as_str());
        fill.ts_event = UnixNanos::from(20_000 + index as u64);
        strategy.handle_order_filled(&fill);
    }
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            client_order_id,
            ..
        }) if client_order_id == released_exit_id
    ));
    assert!(
        evidence
            .recorded_facts()
            .expect("historical fill saturation evidence should decode")
            .iter()
            .any(|fact| matches!(
                fact,
                CurrentFact::OrderLifecycle(record)
                    if record.transition == OrderLifecycleTransition::ExposureObligationSaturated
                        && record.outcome == OrderLifecycleOutcome::ObligationSaturated
                        && record.client_order_id.as_deref() == Some(released_exit_id.as_str())
                        && record.position_id.as_deref() == Some(position_id.as_str())
            ))
    );
}

#[test]
fn historical_exit_obligations_retain_lifecycle_history_behind_each_occupied_authority() {
    fn released_exit_fixture(
        suffix: &str,
    ) -> (
        BinaryOracleEdgeTaker,
        InstrumentId,
        OpenPositionState,
        ClientOrderId,
    ) {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from(format!("P-HISTORICAL-{suffix}").as_str()),
            Quantity::new(10.0, 2),
            0.45,
            OrderSide::Buy,
            PositionSide::Long,
        );
        let client_order_id = ClientOrderId::from(format!("EXIT-HISTORICAL-{suffix}").as_str());
        set_exit_pending(
            &mut strategy,
            position.clone(),
            client_order_id,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
            ExitLifecycleEvent::ReleaseFlat,
        ));
        assert!(strategy.exposure.released_exit(&client_order_id).is_some());
        (strategy, instrument_id, position, client_order_id)
    }

    fn defer_correction(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        suffix: &str,
    ) {
        strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
            UntrackedOrderEvent::HistoricalExitCorrection(HistoricalExitCorrection {
                client_order_id,
                instrument_id,
                trade_id: TradeId::from(format!("TRADE-HISTORICAL-{suffix}").as_str()),
                voided_quantity: Quantity::new(1.0, 2),
                ts_event_ns: 1_000,
            }),
        ));
    }

    fn observe_terminal(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
    ) {
        strategy.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id,
            instrument_id,
            transition: OrderLifecycleTransition::OrderCanceled,
            source: OrderLifecycleSource::OrderCanceled,
            raw_reason_text: None,
            ts_event_ns: 2_000,
            authority: ExitOrderAuthorityObservation::Lifecycle,
        });
    }

    fn assert_complete_history(strategy: &BinaryOracleEdgeTaker, client_order_id: ClientOrderId) {
        let obligation = strategy
            .exposure
            .deferred_obligation(&client_order_id)
            .expect("occupied authority must retain the released exit obligation");
        assert_eq!(obligation.history.len(), 1);
        assert!(!obligation.observations.is_empty());
        assert!(
            obligation
                .observations
                .values()
                .any(|observation| observation.terminal)
        );
    }

    let (mut pending_strategy, instrument_id, _, released_id) = released_exit_fixture("PENDING");
    let pending = pending_entry_state(
        &mut pending_strategy,
        ClientOrderId::from("ENTRY-HISTORICAL-BLOCK"),
    );
    set_pending_entry(&mut pending_strategy, pending.clone());
    defer_correction(&mut pending_strategy, instrument_id, released_id, "PENDING");
    let same_timestamp_key = HistoricalExitObservationKey::Lifecycle {
        ts_event_ns: 1_500,
        terminal: false,
    };
    for (quantity, trade_id) in [
        (Quantity::new(1.0, 2), TradeId::from("TRADE-SAME-TS-A")),
        (Quantity::new(2.0, 2), TradeId::from("TRADE-SAME-TS-B")),
    ] {
        pending_strategy
            .exposure
            .reduce(ExposureEvent::UntrackedOrder(
                UntrackedOrderEvent::HistoricalExitObservation(HistoricalExitObservation {
                    client_order_id: released_id,
                    instrument_id,
                    key: same_timestamp_key.clone(),
                    observation: ExitRecoveryObservation {
                        ts_event_ns: 1_500,
                        trade_ids: BTreeSet::from([trade_id]),
                        effective_filled_quantity: quantity,
                        terminal: false,
                        correction: BoltV3ExitOrderCorrection::Unchanged,
                    },
                }),
            ));
    }
    let same_timestamp = pending_strategy
        .exposure
        .deferred_obligation(&released_id)
        .expect("same-timestamp observation should remain attributed");
    assert_eq!(same_timestamp.observations.len(), 1);
    assert_eq!(
        same_timestamp
            .observations
            .get(&same_timestamp_key)
            .expect("same-key update should replace the stale snapshot")
            .effective_filled_quantity,
        Quantity::new(2.0, 2)
    );
    observe_terminal(&mut pending_strategy, instrument_id, released_id);
    assert!(matches!(
        pending_strategy.exposure.state(),
        ExposureState::PendingEntry(current) if current.client_order_id == pending.client_order_id
    ));
    assert_complete_history(&pending_strategy, released_id);

    let (mut exit_strategy, instrument_id, mut position, released_id) =
        released_exit_fixture("ACTIVE");
    position.position_id = PositionId::from("P-HISTORICAL-ACTIVE-B");
    position.episode.position_id = position.position_id;
    position.episode.opening_order_id = ClientOrderId::from("ENTRY-HISTORICAL-ACTIVE-B");
    position.episode.ts_opened_ns = 3_000;
    let active_id = ClientOrderId::from("EXIT-HISTORICAL-ACTIVE-B");
    set_exit_pending(
        &mut exit_strategy,
        position,
        active_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    defer_correction(&mut exit_strategy, instrument_id, released_id, "ACTIVE");
    observe_terminal(&mut exit_strategy, instrument_id, released_id);
    assert!(matches!(
        exit_strategy.exposure.state(),
        ExposureState::ExitPending(current) if current.client_order_id() == active_id
    ));
    assert_complete_history(&exit_strategy, released_id);

    let (mut hold_strategy, instrument_id, mut position, released_id) =
        released_exit_fixture("HOLD");
    position.position_id = PositionId::from("P-HISTORICAL-HOLD-B");
    position.episode.position_id = position.position_id;
    position.episode.opening_order_id = ClientOrderId::from("ENTRY-HISTORICAL-HOLD-B");
    position.episode.ts_opened_ns = 4_000;
    let hold_id = ClientOrderId::from("EXIT-HISTORICAL-HOLD-B");
    set_exit_pending(
        &mut hold_strategy,
        position,
        hold_id,
        ManagedPositionOrigin::StrategyEntry,
    );
    let held_exit = hold_strategy
        .exposure
        .exit_pending_snapshot()
        .expect("fixture should retain the different active exit");
    register_test_strategy(&mut hold_strategy)
        .borrow_mut()
        .reset();
    hold_strategy.enter_exit_authority_recovery_hold(
        held_exit.position,
        held_exit.pending_exit,
        ExitAuthorityRecoveryPlan::Resume(held_exit.authority),
        1_500,
    );
    defer_correction(&mut hold_strategy, instrument_id, released_id, "HOLD");
    observe_terminal(&mut hold_strategy, instrument_id, released_id);
    assert!(
        hold_strategy
            .exposure
            .exit_authority_recovery_hold()
            .is_some_and(|hold| hold.client_order_id() == hold_id)
    );
    assert_complete_history(&hold_strategy, released_id);
}

#[test]
fn historical_exit_obligation_count_cap_is_loud_bounded_and_preserves_retained_authority() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let template = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-OBLIGATION-COUNT-TEMPLATE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let configured_limit = strategy.exposure.limits().max_count.get() as usize;
    let mut final_released_id = None;
    for index in 0..=configured_limit {
        let mut position = template.clone();
        position.position_id = PositionId::from(format!("P-OBLIGATION-COUNT-{index}").as_str());
        position.episode.position_id = position.position_id;
        position.episode.opening_order_id =
            ClientOrderId::from(format!("ENTRY-OBLIGATION-COUNT-{index}").as_str());
        position.episode.ts_opened_ns = 10_000 + index as u64;
        let client_order_id =
            ClientOrderId::from(format!("EXIT-OBLIGATION-COUNT-{index}").as_str());
        set_exit_pending(
            &mut strategy,
            position,
            client_order_id,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
            ExitLifecycleEvent::ReleaseFlat,
        ));
        if index == configured_limit {
            final_released_id = Some(client_order_id);
            continue;
        }
        strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
            UntrackedOrderEvent::HistoricalExitCorrection(HistoricalExitCorrection {
                client_order_id,
                instrument_id,
                trade_id: TradeId::from(format!("TRADE-OBLIGATION-COUNT-{index}").as_str()),
                voided_quantity: Quantity::new(1.0, 2),
                ts_event_ns: 20_000 + index as u64,
            }),
        ));
    }
    let pending = pending_entry_state(
        &mut strategy,
        ClientOrderId::from("ENTRY-OBLIGATION-COUNT-RETAINED"),
    );
    set_pending_entry(&mut strategy, pending.clone());
    let final_released_id = final_released_id.expect("fixture should reserve the cap event");
    strategy.exposure.reduce(ExposureEvent::UntrackedOrder(
        UntrackedOrderEvent::HistoricalExitCorrection(HistoricalExitCorrection {
            client_order_id: final_released_id,
            instrument_id,
            trade_id: TradeId::from("TRADE-OBLIGATION-COUNT-CAP"),
            voided_quantity: Quantity::new(1.0, 2),
            ts_event_ns: 30_000,
        }),
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            retained,
            client_order_id,
            obligation_count,
        }) if client_order_id == final_released_id
            && obligation_count == configured_limit
            && matches!(*retained, ExposureState::PendingEntry(ref current)
                if current.client_order_id == pending.client_order_id)
    ));
    assert_eq!(
        strategy
            .exposure
            .pending_entry()
            .map(|current| current.client_order_id),
        Some(pending.client_order_id)
    );
    strategy.exposure.reduce(ExposureEvent::EntryLifecycle(
        EntryLifecycleEvent::ReleaseFlat,
    ));
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState { retained, .. })
            if matches!(*retained, ExposureState::Flat)
    ));
    assert_eq!(
        strategy
            .exposure
            .request_entry_operation(strategy.exposure.generation())
            .expect_err("saturation must remain non-routing")
            .reason,
        ExposureOperationBlockedReason::ObligationSaturated
    );
}

#[test]
fn released_exit_provenance_cap_is_loud_bounded_and_preserves_the_live_exit() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let template = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-RELEASE-PROVENANCE-TEMPLATE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let configured_limit = strategy
        .exposure
        .limits()
        .max_released_exit_provenance_count
        .get() as usize;
    let mut saturated_order_id = None;
    for index in 0..=configured_limit {
        let mut position = template.clone();
        position.position_id = PositionId::from(format!("P-RELEASE-PROVENANCE-{index}").as_str());
        position.episode.position_id = position.position_id;
        position.episode.opening_order_id =
            ClientOrderId::from(format!("ENTRY-RELEASE-PROVENANCE-{index}").as_str());
        position.episode.ts_opened_ns = 40_000 + index as u64;
        let client_order_id =
            ClientOrderId::from(format!("EXIT-RELEASE-PROVENANCE-{index}").as_str());
        set_exit_pending(
            &mut strategy,
            position,
            client_order_id,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
            ExitLifecycleEvent::ReleaseFlat,
        ));
        if index == configured_limit {
            saturated_order_id = Some(client_order_id);
        }
    }

    let saturated_order_id = saturated_order_id.expect("cap iteration should run");
    assert_eq!(strategy.exposure.released_exit_count(), configured_limit);
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::ObligationSaturated(ObligationSaturatedState {
            retained,
            client_order_id,
            obligation_count,
        }) if client_order_id == saturated_order_id
            && obligation_count == configured_limit
            && matches!(*retained, ExposureState::ExitPending(_))
    ));
}

#[test]
fn authenticated_opening_fill_void_rebases_a_continuous_episode_and_refloors_close_proofs() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REBASE-CONTINUOUS"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut before = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be governed");
    let opening_fill_id = before
        .episode_fill_ids
        .iter()
        .next()
        .copied()
        .expect("cache-derived episode must record its opening fill");
    let surviving_fill_id = TradeId::from("TRADE-REBASE-SURVIVOR");
    before.episode_fill_ids.insert(surviving_fill_id);
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::RefreshContext(before.clone()),
    ));
    reduce_position_close_with_projection(
        &strategy,
        before.episode.clone(),
        FreshCanonicalPositionProjection::ExactlyOne(Box::new(ClassifiedOpenPosition::Managed(
            before.clone(),
        ))),
    );

    let mut rebased = before.clone();
    rebased.episode.opening_order_id = ClientOrderId::from("ENTRY-REBASE-SURVIVOR");
    rebased.episode.ts_opened_ns = 2_000;
    rebased.episode_fill_ids = BTreeSet::from([surviving_fill_id]);
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before: before.episode.clone(),
                authenticated_order_id: before.episode.opening_order_id,
                authenticated_fill_id: opening_fill_id,
                rebased: Some(Box::new(rebased.clone())),
            },
        ),
    );

    let current = strategy
        .exposure
        .managed_position_context()
        .expect("surviving replay segment must remain governed");
    assert_eq!(current.episode, rebased.episode);
    assert!(!current.episode_close_seen);
    assert!(!current.canonical_none_seen);
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::None,
        )),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));
}

fn split_flip_fill_void_fixture(
    voided_qty: Quantity,
) -> (
    BinaryOracleEdgeTaker,
    PositionEpisodeFingerprint,
    PositionEpisodeFingerprint,
    Arc<crate::bolt_v3_current_evidence::DecisionEvidenceRecorder>,
) {
    let evidence = recording_decision_evidence();
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new(evidence.clone()),
        ),
    );
    let instrument_id = selected_entry_instrument(&strategy);
    let position_id = PositionId::from("P-SPLIT-FLIP-VOID");
    register_test_strategy_with_instrument(&mut strategy, &instrument_id);
    let cache = register_test_strategy(&mut strategy);
    let instrument = cache
        .borrow()
        .instrument(&instrument_id)
        .cloned()
        .expect("selected instrument should be cached");

    let mut opening = order_filled_event(
        ClientOrderId::from("O-SPLIT-FLIP-OPEN"),
        instrument_id,
        Some(position_id),
        OrderSide::Sell,
    );
    opening.trade_id = TradeId::from("T-SPLIT-FLIP-OPEN");
    opening.last_qty = Quantity::new(10.0, 2);
    opening.ts_event = nautilus_core::UnixNanos::from(1_000_u64);
    let mut position = nautilus_model::position::Position::new(&instrument, opening);

    let mut closing = order_filled_event(
        ClientOrderId::from("O-SPLIT-FLIP"),
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    );
    closing.trade_id = TradeId::from("T-SPLIT-FLIP");
    closing.last_qty = Quantity::new(10.0, 2);
    closing.ts_event = nautilus_core::UnixNanos::from(2_000_u64);
    let mut reopening = closing.clone();
    reopening.last_qty = Quantity::new(5.0, 2);
    reopening.event_id = nautilus_core::UUID4::new();
    reopening.causation_id = Some(closing.event_id);
    position.apply(&closing);
    position.apply(&reopening);
    assert_eq!(position.side, PositionSide::Long);
    assert_eq!(position.opening_order_id, reopening.client_order_id);
    cache
        .borrow_mut()
        .add_position(&position, nautilus_model::enums::OmsType::Netting)
        .expect("split-flip position should enter the authoritative cache");

    assert!(strategy.materialize_position_from_event(
        PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side: position.entry,
            side: position.side,
            quantity: position.quantity,
            avg_px_open: position.avg_px_open,
            opening_order_id: position.opening_order_id,
            ts_opened_ns: position.ts_opened.as_u64(),
        },
        position.ts_last.as_u64(),
    ));
    strategy.sync_exposure_context_from_active();
    let context_b = strategy
        .exposure
        .managed_position_context()
        .expect("reopened split fragment should materialize episode B");
    let episode_b = context_b.episode.clone();
    let governed = managed_position_snapshot(&strategy).expect("episode B should be governed");
    set_exit_pending(
        &mut strategy,
        governed,
        ClientOrderId::from("EXIT-SPLIT-FLIP-B"),
        ManagedPositionOrigin::RecoveryBootstrap,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::RefreshContext(context_b),
    ));
    cache
        .borrow_mut()
        .update_position(&position)
        .expect("split replay truth should replace the generic exit fixture position");

    let correction = order_fill_voided_event(
        closing.client_order_id,
        instrument_id,
        position_id,
        closing.trade_id,
        voided_qty,
        3_000,
        closing.order_side,
    );
    position
        .apply_fill_void(correction.clone(), voided_qty, correction.commission_voided)
        .expect("pinned NT replay should accept the split-flip correction");
    cache
        .borrow_mut()
        .update_position(&position)
        .expect("corrected split-flip position should replace the cache snapshot");
    assert!(strategy.apply_authenticated_episode_rebase_for_fill_void(&correction));
    let corrected_episode = PositionEpisodeFingerprint {
        instrument_id,
        position_id,
        opening_order_id: position.opening_order_id,
        ts_opened_ns: position.ts_opened.as_u64(),
    };
    (strategy, episode_b, corrected_episode, evidence)
}

#[test]
fn pinned_nt_split_flip_void_cannot_transfer_exit_authority_across_flat_segment() {
    let (strategy, episode_b, corrected_episode_a, evidence) =
        split_flip_fill_void_fixture(Quantity::new(12.0, 2));

    assert_ne!(episode_b, corrected_episode_a);
    assert!(strategy.exposure.exit_pending_snapshot().is_none());
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(current) if current.episode == corrected_episode_a
    ));
    let corrected_instrument_id = corrected_episode_a.instrument_id.to_string();
    let facts = evidence
        .recorded_facts()
        .expect("flat-crossing replay adoption evidence should decode");
    assert!(facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ReplacementAdopted
                && record.outcome == OrderLifecycleOutcome::Managed
                && record.source == OrderLifecycleSource::OrderFillVoided
                && record.instrument_id.as_deref()
                    == Some(corrected_instrument_id.as_str())
                && record.position_id.as_deref()
                    == Some(corrected_episode_a.position_id.as_str())
                && record.client_order_id.as_deref()
                    == Some(corrected_episode_a.opening_order_id.as_str())
                && record.prior_client_order_id.as_deref()
                    == Some(episode_b.opening_order_id.as_str())
                && record.raw_reason_text.as_deref()
                    == Some("authenticated_fill_void_correction")
    )));
}

#[test]
fn pinned_nt_partial_split_fragment_void_preserves_same_segment_exit_authority() {
    let (strategy, episode_b, corrected_episode, evidence) =
        split_flip_fill_void_fixture(Quantity::new(2.0, 2));

    assert_eq!(episode_b, corrected_episode);
    assert!(matches!(
        strategy.exposure.exit_pending_snapshot(),
        Some(exit) if exit.episode() == episode_b
    ));
    let facts = evidence
        .recorded_facts()
        .expect("same-segment replay evidence should decode");
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        CurrentFact::OrderLifecycle(record)
            if record.transition == OrderLifecycleTransition::ReplacementAdopted
    )));
}

#[test]
fn authenticated_sole_opening_fill_void_uses_correction_specific_release_proof() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REBASE-SOLE"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let before = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be governed");
    let opening_fill_id = *before
        .episode_fill_ids
        .iter()
        .next()
        .expect("cache-derived episode must record its opening fill");

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before: before.episode.clone(),
                authenticated_order_id: before.episode.opening_order_id,
                authenticated_fill_id: TradeId::from("TRADE-NOT-IN-EPISODE"),
                rebased: None,
            },
        ),
    );
    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(_)
    ));

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before: before.episode.clone(),
                authenticated_order_id: before.episode.opening_order_id,
                authenticated_fill_id: opening_fill_id,
                rebased: None,
            },
        ),
    );
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));
}

#[test]
fn authenticated_rebase_updates_the_sealed_exit_authority_and_position_atomically() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REBASE-EXIT-AUTHORITY"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let mut before = strategy
        .exposure
        .managed_position_context()
        .expect("fixture position should be governed");
    let opening_fill_id = *before
        .episode_fill_ids
        .iter()
        .next()
        .expect("cache-derived episode must record its opening fill");
    let surviving_fill_id = TradeId::from("TRADE-REBASE-EXIT-SURVIVOR");
    before.episode_fill_ids.insert(surviving_fill_id);
    set_exit_pending(
        &mut strategy,
        position,
        ClientOrderId::from("EXIT-REBASE-AUTHORITY"),
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::RefreshContext(before.clone()),
    ));
    let mut rebased = before.clone();
    rebased.episode.opening_order_id = ClientOrderId::from("ENTRY-REBASE-EXIT-SURVIVOR");
    rebased.episode.ts_opened_ns = 3_000;
    rebased.episode_fill_ids = BTreeSet::from([surviving_fill_id]);

    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before: before.episode.clone(),
                authenticated_order_id: before.episode.opening_order_id,
                authenticated_fill_id: opening_fill_id,
                rebased: Some(Box::new(rebased.clone())),
            },
        ),
    );
    let exit = strategy
        .exposure
        .exit_pending_snapshot()
        .expect("active exit must survive continuous replay");
    assert_eq!(exit.position.episode, rebased.episode);
    assert_eq!(exit.episode(), rebased.episode);
}

#[test]
fn correction_closing_episode_a_never_rebases_reopened_episode_b_with_the_same_position_id() {
    let mut strategy = ready_to_trade_strategy();
    let instrument_id = selected_entry_instrument(&strategy);
    let position_a = materialize_configured_position(
        &mut strategy,
        instrument_id,
        PositionId::from("P-REBASE-REUSED"),
        Quantity::new(10.0, 2),
        0.45,
        OrderSide::Buy,
        PositionSide::Long,
    );
    let context_a = strategy
        .exposure
        .managed_position_context()
        .expect("episode A should be governed");
    let opening_fill_a = *context_a
        .episode_fill_ids
        .iter()
        .next()
        .expect("episode A should retain its opening fill identity");
    let exit_a = ClientOrderId::from("EXIT-REBASE-EPISODE-A");
    set_exit_pending(
        &mut strategy,
        position_a,
        exit_a,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.exposure.reduce(ExposureEvent::PositionTruth(
        PositionTruthEvent::RefreshContext(context_a.clone()),
    ));
    strategy.exposure.reduce(ExposureEvent::ExitLifecycle(
        ExitLifecycleEvent::ReleaseFlat,
    ));
    assert!(strategy.exposure.released_exit(&exit_a).is_some());

    let mut context_b = context_a.clone();
    context_b.episode.opening_order_id = ClientOrderId::from("ENTRY-REBASE-EPISODE-B");
    context_b.episode.ts_opened_ns = 4_000;
    context_b.episode_fill_ids = BTreeSet::from([TradeId::from("TRADE-REBASE-B")]);
    context_b.replay_segment.clear();
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(AdoptionCapablePositionTruthEvent::Canonical(
            CanonicalPositionProjection::ExactlyOne(Box::new(context_b.clone())),
        )),
    );
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::AuthenticatedEpisodeRebase {
                before: context_a.episode.clone(),
                authenticated_order_id: context_a.episode.opening_order_id,
                authenticated_fill_id: opening_fill_a,
                rebased: Some(Box::new(context_b.clone())),
            },
        ),
    );

    assert!(matches!(
        strategy.exposure.state(),
        ExposureState::Managed(current) if current.episode == context_b.episode
    ));
    assert!(strategy.exposure.released_exit(&exit_a).is_none());
}

#[test]
fn exposure_managed_recovery_origin_is_explicit_without_recovery_boolean() {
    let strategy = ready_to_trade_strategy();
    let instrument_id = strategy.active.books.up.instrument_id.unwrap();
    let managed = ExposureState::Managed(managed_position_context(
        OpenPositionState {
            episode: position_episode_for_test(instrument_id, PositionId::from("P-RECOVERY-001")),
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
        ManagedPositionOrigin::RecoveryBootstrap,
        None,
    ));

    let managed = managed
        .managed_position_context()
        .expect("managed exposure should return managed context");
    assert_eq!(managed.origin, ManagedPositionOrigin::RecoveryBootstrap);
    assert_eq!(managed.position_id, PositionId::from("P-RECOVERY-001"));
}

#[test]
fn position_truth_recovery_after_terminal_flat_records_rematerialization_evidence() {
    let evidence = recording_decision_evidence();
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

    strategy.on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id));
    assert!(matches!(strategy.exposure.state(), ExposureState::Flat));

    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(5.0, 2),
        0.450,
        OrderSide::Buy,
    );
    strategy.on_position_opened(position_opened_event(
        instrument_id,
        position_id,
        Quantity::new(5.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));

    assert!(
        evidence.recorded_facts().expect("recorded current evidence must decode").into_iter().any(|event| matches!(
            event,
            CurrentFact::OrderLifecycle(record)
                if record.transition
                    == crate::bolt_v3_current_evidence::OrderLifecycleTransition::PositionTruthRematerialized
                    && record.outcome
                        == crate::bolt_v3_current_evidence::OrderLifecycleOutcome::Managed
                    && record.source == OrderLifecycleSource::PositionEvent
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
        OrderSide::Buy,
        PositionSide::Long,
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

    strategy.exposure.reduce(ExposureEvent::SettlementEffect(
        SettlementEffectEvent::ReleaseFlat {
            episode: strategy
                .exposure
                .managed_position_context()
                .expect("fixture should remain managed before settlement")
                .episode,
        },
    ));
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
    let mut strategy = ready_to_trade_strategy_with_bound_economics();
    register_test_strategy_with_active_instruments(&mut strategy);
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
        OrderSide::Buy,
    );
    fill_strategy.on_order_filled(&order_filled_event(
        fill_client_order_id,
        fill_instrument_id,
        Some(fill_position_id),
        OrderSide::Buy,
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
        OrderSide::Buy,
    );
    position_strategy.on_position_opened(position_opened_event(
        position_instrument_id,
        adopted_position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
        PositionSide::Long,
    ));
    assert_eq!(
        managed_position_snapshot(&position_strategy)
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

    let position_id = PositionId::from("P-DIRECT-CLEAR-001");
    seed_nt_open_position(
        &mut strategy,
        instrument_id,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
        OrderSide::Buy,
    );
    strategy.on_order_filled(&order_filled_event(
        fill_pending.client_order_id,
        instrument_id,
        Some(position_id),
        OrderSide::Buy,
    ));

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
