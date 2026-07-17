#![cfg(test)]

use super::*;
use nautilus_trading::Strategy;

#[test]
fn switch_resets_only_active_market_state() {
    let mut strategy = test_strategy();
    strategy.market_lifecycle.insert(
        "A".to_string(),
        MarketLifecycleLedger {
            cooldown_expires_at_ms: Some(123),
            churn_count: 2,
        },
    );
    set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);
    strategy
        .pricing
        .set_selected_pricing_spot(Some(fast_spot("bybit", 3_100.5, 1_200)));
    strategy
        .pricing
        .seed_ready_realized_vol(Some("<SOURCE_ID>".to_string()), 1.5, 1_200);
    {
        let active = &mut strategy.active;
        active.interval_open = Some(3_000.0);
        active.warmup_count = 7;
    }

    strategy.apply_selection_snapshot(active_snapshot("B"));

    assert_eq!(
        strategy.market_lifecycle.get("A"),
        Some(&MarketLifecycleLedger {
            cooldown_expires_at_ms: Some(123),
            churn_count: 2,
        })
    );
    assert!(strategy.exposure.is_recovering());
    let active = &strategy.active;
    assert_eq!(active.market_id.as_deref(), Some("B"));
    assert!(active.interval_open.is_none());
    assert_eq!(active.warmup_count, 0);
    assert_eq!(
        strategy.pricing.selected_pricing_spot().cloned(),
        Some(fast_spot("bybit", 3_100.5, 1_200))
    );
    assert_eq!(
        strategy
            .pricing
            .current_realized_vol_at(LocalReceiveMs::new(1_200), None),
        Some(1.5)
    );
    assert_eq!(
        strategy
            .pricing
            .current_realized_vol_source_at(LocalReceiveMs::new(1_200), None),
        (Some("<SOURCE_ID>".to_string()), Some(1_200))
    );
}

#[test]
fn same_market_interval_rollover_preserves_reconstructed_books() {
    let mut strategy = ready_to_trade_strategy();
    let original_up_bid = strategy.active.books.up.best_bid;
    let original_down_ask = strategy.active.books.down.best_ask;

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 301_000));

    assert_eq!(strategy.active.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(strategy.active.interval_start_ms, Some(301_000));
    assert_eq!(strategy.active.books.up.best_bid, original_up_bid);
    assert_eq!(strategy.active.books.down.best_ask, original_down_ask);
    assert!(strategy.active.books.is_priced());
}

#[test]
fn fill_arms_cooldown_for_filled_market_not_current_selection() {
    let mut strategy = ready_to_trade_strategy();
    let entry_client_order_id = ClientOrderId::from("ENTRY-A");
    let position_id = PositionId::from("P-A");
    let pending = pending_entry_state(&mut strategy, entry_client_order_id);
    let instrument_a = pending.instrument_id;
    set_pending_entry(&mut strategy, pending);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy
        .on_order_filled(&order_filled_event(
            entry_client_order_id,
            instrument_a,
            position_id,
        ))
        .expect("fill bookkeeping should succeed");

    assert!(strategy.market_in_cooldown("MKT-1", 1_000));
    assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
    assert_eq!(strategy.market_churn_count("MKT-1"), 1);
    assert_eq!(strategy.market_churn_count("MKT-2"), 0);
}

#[test]
fn exit_fill_arms_cooldown_for_position_market_not_current_selection() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = selected_entry_instrument(&strategy);
    let exit_client_order_id = ClientOrderId::from("EXIT-A");
    let position_id = PositionId::from("P-A");
    let open_position = materialize_configured_position(
        &mut strategy,
        tracked_instrument,
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
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));

    strategy
        .on_order_filled(&order_filled_event(
            exit_client_order_id,
            tracked_instrument,
            position_id,
        ))
        .expect("exit fill bookkeeping should succeed");

    assert!(strategy.market_in_cooldown("MKT-1", 1_000));
    assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
    assert_eq!(strategy.market_churn_count("MKT-1"), 1);
    assert_eq!(strategy.market_churn_count("MKT-2"), 0);
}

#[test]
fn exit_fill_without_known_position_market_does_not_cool_down_active_selection() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = selected_entry_instrument(&strategy);
    let exit_client_order_id = ClientOrderId::from("EXIT-UNKNOWN");
    let position_id = PositionId::from("P-UNKNOWN");
    let mut open_position = materialize_configured_position(
        &mut strategy,
        tracked_instrument,
        position_id,
        Quantity::new(10.0, 2),
        0.450,
    );
    open_position.lifecycle = BoltV3PositionMarketLifecycle::missing();
    set_exit_pending(
        &mut strategy,
        open_position,
        exit_client_order_id,
        false,
        false,
        ManagedPositionOrigin::StrategyEntry,
    );
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));

    strategy
        .on_order_filled(&order_filled_event(
            exit_client_order_id,
            tracked_instrument,
            position_id,
        ))
        .expect("exit fill bookkeeping should succeed");

    assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
}

#[test]
fn delayed_exit_fill_after_position_closed_does_not_cool_down_active_selection() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = selected_entry_instrument(&strategy);
    let exit_client_order_id = ClientOrderId::from("EXIT-DELAYED");
    let position_id = PositionId::from("P-DELAYED");
    let open_position = materialize_configured_position(
        &mut strategy,
        tracked_instrument,
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
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
    strategy.on_position_closed(position_closed_event(tracked_instrument, position_id));

    strategy
        .on_order_filled(&order_filled_event(
            exit_client_order_id,
            tracked_instrument,
            position_id,
        ))
        .expect("delayed exit fill should not arm the wrong market cooldown");

    assert!(strategy.market_in_cooldown("MKT-1", 1_000));
    assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
    assert!(pending_exit_ref(&strategy).is_none());
}

#[test]
fn same_market_active_to_freeze_updates_forced_flat_without_resetting_shell_state() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
    {
        let active = &mut strategy.active;
        active.interval_open = Some(3_100.0);
        active.warmup_count = 2;
        active.forced_flat = false;
    }

    strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 1_000));

    let active = &strategy.active;
    assert_eq!(active.market_id.as_deref(), Some("MKT-1"));
    assert!(active.forced_flat);
    assert_eq!(active.interval_open, Some(3_100.0));
    assert_eq!(active.warmup_count, 2);
    assert_eq!(
        active
            .books
            .up
            .instrument_id
            .map(|instrument_id| instrument_id.to_string())
            .as_deref(),
        Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
    );
    assert_eq!(
        active
            .books
            .down
            .instrument_id
            .map(|instrument_id| instrument_id.to_string())
            .as_deref(),
        Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
    );
}

#[test]
fn freeze_continues_reference_preparation_without_opening_entries() {
    let mut strategy = test_strategy();
    strategy.config.warmup_tick_count = 2;
    let mut snapshot = freeze_snapshot_with_start("MKT-1", 1_000);
    let SelectionState::Freeze { market, .. } = &mut snapshot.decision.state else {
        panic!("expected freeze snapshot");
    };
    market.price_to_beat = Some(3_100.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy.observe_reference_snapshot(&reference_tick(900, 3_099.0), LocalReceiveMs::new(900));
    assert!(strategy.active.interval_open.is_none());
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, 0);

    strategy
        .observe_reference_snapshot(&reference_tick(1_000, 3_100.0), LocalReceiveMs::new(1_000));
    assert_eq!(strategy.active.interval_open, Some(3_100.0));
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_000));
    assert_eq!(strategy.active.warmup_count, 1);
    assert!(!strategy.active.warmup_complete());
    assert!(strategy.active.forced_flat);

    strategy
        .observe_reference_snapshot(&reference_tick(1_100, 3_101.0), LocalReceiveMs::new(1_100));
    assert_eq!(strategy.active.last_reference_ts_ms, Some(1_100));
    assert_eq!(strategy.active.warmup_count, 2);
    assert!(strategy.active.warmup_complete());
    assert!(strategy.active.forced_flat);
    let gate = strategy.entry_gate_decision_at(1_100);
    assert!(
        gate.blocked_by
            .contains(&EntryBlockReason::ForcedFlat(ForcedFlatReason::Freeze))
    );
}

#[test]
fn strategy_selects_configured_updown_target_from_nt_binary_option_metadata() {
    let strategy = test_strategy();
    let current_start = 1_746_000_000_i64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        current_start,
    );
    let instruments = vec![
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-1",
            "Up",
            current_start as u64 * MILLIS_PER_SECOND_U64,
            current_start as u64 * MILLIS_PER_SECOND_U64
                + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-1",
            "Down",
            current_start as u64 * MILLIS_PER_SECOND_U64,
            current_start as u64 * MILLIS_PER_SECOND_U64
                + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
        ),
    ];

    let snapshot = selection_snapshot_from_instruments(
        &strategy.config,
        &instruments,
        current_start as u64 * MILLIS_PER_SECOND_U64 + 1,
    );

    let SelectionState::Active { market } = snapshot.decision.state else {
        panic!("configured target should select active market: {snapshot:?}");
    };
    assert_eq!(market.market_id, "market-1");
    assert_eq!(market.up.instrument_id, "token-up.POLYMARKET");
    assert_eq!(market.down.instrument_id, "token-down.POLYMARKET");
}

#[test]
fn strategy_selects_configured_static_binary_event_from_nt_binary_option_metadata() {
    let mut strategy = test_strategy();
    strategy.config.target_kind = "static_market".to_string();
    strategy.config.rotating_market_family =
        crate::bolt_v3_market_families::static_binary_event::KEY.to_string();
    strategy.config.underlying_asset = "sample_event_2026".to_string();
    strategy.config.cadence_seconds = 1;
    strategy.config.cadence_slug_token = "will-sample-event-resolve-yes".to_string();
    strategy.config.market_selection_rule = "configured_static".to_string();
    strategy.config.static_condition_id = Some("condition-sample-event-yes-no".to_string());
    strategy.config.static_yes_outcome = Some("Yes".to_string());
    strategy.config.static_no_outcome = Some("No".to_string());
    let instruments = vec![
        updown_binary_option(
            "sample-event-no.POLYMARKET",
            &strategy.config.cadence_slug_token,
            "sample-event-yes-no",
            "No",
            1_000,
            30_000,
        ),
        updown_binary_option(
            "sample-event-yes.POLYMARKET",
            &strategy.config.cadence_slug_token,
            "sample-event-yes-no",
            "Yes",
            1_000,
            30_000,
        ),
    ];

    let snapshot = selection_snapshot_from_instruments(&strategy.config, &instruments, 10_000);

    let SelectionState::Active { market } = snapshot.decision.state else {
        panic!("configured static event should select active market: {snapshot:?}");
    };
    assert_eq!(market.market_id, "sample-event-yes-no");
    assert_eq!(market.up.instrument_id, "sample-event-yes.POLYMARKET");
    assert_eq!(market.down.instrument_id, "sample-event-no.POLYMARKET");
    assert_eq!(
        market.source_identity.condition_id,
        "condition-sample-event-yes-no"
    );
}

#[test]
fn strategy_refuses_foreign_venue_market_even_when_slug_matches_the_target() {
    // P5-5 / Codex P5: the shared NT cache can hold instruments from venues OTHER than the
    // execution venue. A foreign-venue binary option that happens to carry the SAME updown slug
    // as the configured target must never be tradeable — a real order only ever routes to the
    // execution client's venue. The market matcher is venue-agnostic (it matches on slug +
    // outcome), so without venue scoping a colliding foreign instrument WOULD be selectable;
    // the execution-venue read filter excludes it and the explicit guard fails closed.
    let strategy = test_strategy();
    let current_start = 1_746_000_000_i64;
    let now_ms = current_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        current_start,
    );
    let start_ms = current_start as u64 * MILLIS_PER_SECOND_U64;
    let end_ms = start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let execution_venue = fixture_execution_venue();

    // The SAME slug + market id exists on a NON-execution venue (a HIP-4 / Hyperliquid market
    // here) as well as on the Polymarket execution venue.
    let foreign = vec![
        updown_binary_option(
            "token-up.HYPERLIQUID",
            &market_slug,
            "market-1",
            "Up",
            start_ms,
            end_ms,
        ),
        updown_binary_option(
            "token-down.HYPERLIQUID",
            &market_slug,
            "market-1",
            "Down",
            start_ms,
            end_ms,
        ),
    ];
    let polymarket = vec![
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-1",
            "Up",
            start_ms,
            end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-1",
            "Down",
            start_ms,
            end_ms,
        ),
    ];

    // The slug genuinely matches on the foreign venue too: in isolation the matcher selects it,
    // which is exactly the latent hazard.
    let foreign_snapshot = selection_snapshot_from_instruments(&strategy.config, &foreign, now_ms);
    let SelectionState::Active { market } = &foreign_snapshot.decision.state else {
        panic!(
            "foreign-venue instruments share the target slug and must select in isolation: {foreign_snapshot:?}"
        );
    };
    assert_eq!(market.up.instrument_id, "token-up.HYPERLIQUID");
    // ...but the execution-venue guard refuses that foreign selection (fail closed).
    assert!(
        !selected_market_on_execution_venue(&foreign_snapshot, execution_venue),
        "a selected market whose outcomes are on a non-execution venue must be refused",
    );

    // The execution-venue market selects and is accepted by the guard.
    let polymarket_snapshot =
        selection_snapshot_from_instruments(&strategy.config, &polymarket, now_ms);
    assert!(
        selected_market_on_execution_venue(&polymarket_snapshot, execution_venue),
        "the execution-venue market must pass the guard",
    );

    // The production cache read scopes by the execution venue, so from a MIXED cache only the
    // execution-venue market is ever considered for selection.
    let mixed = [foreign.clone(), polymarket].concat();
    let scoped = mixed
        .iter()
        .filter(|instrument| instrument.id().venue == execution_venue)
        .cloned()
        .collect::<Vec<_>>();
    let scoped_snapshot = selection_snapshot_from_instruments(&strategy.config, &scoped, now_ms);
    let SelectionState::Active { market } = scoped_snapshot.decision.state else {
        panic!(
            "execution-venue-scoped selection should still find the market: {scoped_snapshot:?}"
        );
    };
    assert_eq!(market.up.instrument_id, "token-up.POLYMARKET");
    assert_eq!(market.down.instrument_id, "token-down.POLYMARKET");
}

#[test]
fn refresh_selection_from_cache_filters_foreign_venue_in_production_path() {
    // P5-5 / Codex P5 — PRODUCTION-PATH regression lock. The sibling test
    // `strategy_refuses_foreign_venue_market_even_when_slug_matches_the_target` proves the
    // selection HELPERS refuse a foreign-venue market, but it REPLICATES the venue filter
    // inside the test, so deleting the production filter would not fail it. This test drives
    // the real `refresh_selection_from_cache` against a shared NT cache holding BOTH a
    // foreign-venue (HYPERLIQUID) and the execution-venue (POLYMARKET) updown market that
    // share the configured target slug, and proves the production path selects ONLY the
    // execution-venue market. Removing the venue-scoped cache filter (the
    // `instrument.id().venue == execution_venue` read) or the
    // `selected_market_on_execution_venue` guard makes this fail. A real order can only ever
    // route to the execution client's venue.
    let mut strategy = test_strategy();
    assert_eq!(
        strategy.context.execution_venue(),
        fixture_execution_venue(),
        "harness precondition: production execution venue must be the POLYMARKET fixture",
    );
    let cache = register_test_strategy(&mut strategy);

    let current_start = 1_746_000_000_i64;
    let now_ms = current_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        current_start,
    );
    let start_ms = current_start as u64 * MILLIS_PER_SECOND_U64;
    let end_ms = start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;

    // Same slug + market id on a NON-execution venue (HYPERLIQUID) AND the execution venue
    // (POLYMARKET). The matcher is venue-agnostic, so without the production venue filter the
    // foreign instruments would be selectable.
    let mixed = [
        updown_binary_option(
            "token-up.HYPERLIQUID",
            &market_slug,
            "market-1",
            "Up",
            start_ms,
            end_ms,
        ),
        updown_binary_option(
            "token-down.HYPERLIQUID",
            &market_slug,
            "market-1",
            "Down",
            start_ms,
            end_ms,
        ),
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-1",
            "Up",
            start_ms,
            end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-1",
            "Down",
            start_ms,
            end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &mixed {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    strategy.refresh_selection_from_cache(now_ms);

    // The production venue-scoped read + guard select ONLY the execution-venue market, even
    // though the foreign-venue market carrying the identical slug is present in the cache.
    assert_eq!(
        strategy
            .active
            .books
            .up
            .instrument_id
            .map(|id| id.to_string())
            .as_deref(),
        Some("token-up.POLYMARKET"),
        "production refresh must select the execution-venue Up outcome from a mixed-venue cache",
    );
    assert_eq!(
        strategy
            .active
            .books
            .down
            .instrument_id
            .map(|id| id.to_string())
            .as_deref(),
        Some("token-down.POLYMARKET"),
        "production refresh must select the execution-venue Down outcome from a mixed-venue cache",
    );
}

#[test]
fn strategy_selects_next_updown_target_outcome_from_nt_binary_option_metadata() {
    let strategy = test_strategy();
    let current_start = 1_746_000_000_i64;
    let next_start = current_start + strategy.config.cadence_seconds as i64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_start,
    );
    let instruments = vec![
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start as u64 * MILLIS_PER_SECOND_U64,
            next_start as u64 * MILLIS_PER_SECOND_U64
                + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start as u64 * MILLIS_PER_SECOND_U64,
            next_start as u64 * MILLIS_PER_SECOND_U64
                + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
        ),
    ];

    let snapshot = selection_snapshot_from_instruments(
        &strategy.config,
        &instruments,
        current_start as u64 * MILLIS_PER_SECOND_U64 + 1,
    );

    let SelectionState::Active { market } = snapshot.decision.state else {
        panic!("configured target should select next market: {snapshot:?}");
    };
    assert_eq!(market.market_id, "market-next");
    assert_eq!(market.selection_outcome, MarketSelectionOutcome::Next);
    assert_eq!(
        market.expiration_ts_ms,
        next_start as u64 * MILLIS_PER_SECOND_U64
            + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64
    );
}

#[test]
fn warmup_requires_consecutive_fresh_ticks() {
    let mut strategy = test_strategy();
    strategy.config.warmup_tick_count = 3;
    let mut snapshot = active_snapshot("MKT-1");
    let SelectionState::Active { market } = &mut snapshot.decision.state else {
        panic!("expected active snapshot");
    };
    market.price_to_beat = Some(3_100.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy
        .observe_reference_snapshot(&reference_tick(1_000, 3_100.0), LocalReceiveMs::new(1_000));
    strategy
        .observe_reference_snapshot(&reference_tick(1_100, 3_101.0), LocalReceiveMs::new(1_100));
    assert!(!strategy.active.warmup_complete());

    strategy
        .observe_reference_snapshot(&reference_tick(1_200, 3_102.0), LocalReceiveMs::new(1_200));
    assert!(strategy.active.warmup_complete());
}

#[test]
fn inactive_expired_market_lifecycle_is_pruned_after_selection_update() {
    let mut strategy = ready_to_trade_strategy();
    strategy.record_market_fill("STALE", 0);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 31_001));

    assert!(!strategy.market_lifecycle.contains_key("STALE"));
    assert_eq!(strategy.market_churn_count("STALE"), 0);
}

#[test]
fn selection_rotation_prunes_entry_reject_state_to_active_instruments() {
    let mut strategy = ready_to_trade_strategy();
    let stale_instrument = selected_entry_instrument(&strategy);
    let next_active_instrument = InstrumentId::from("condition-MKT-2-MKT-2-UP.POLYMARKET");
    strategy
        .entry_reject_state
        .insert(stale_instrument, EntryRejectState::Malformed);
    strategy
        .entry_reject_state
        .insert(next_active_instrument, EntryRejectState::Balance);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 31_001));

    assert!(!strategy.entry_reject_state.contains_key(&stale_instrument));
    assert!(
        strategy
            .entry_reject_state
            .contains_key(&next_active_instrument)
    );
}

#[test]
fn tracked_market_lifecycle_is_retained_after_cooldown_expiry() {
    let mut strategy = ready_to_trade_strategy();
    let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
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
        instrument_id: tracked_instrument,
        position_id: PositionId::from("P-LIFECYCLE-001"),
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
    strategy.record_market_fill("MKT-1", 0);

    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 31_001));

    assert!(strategy.market_lifecycle.contains_key("MKT-1"));
    assert_eq!(strategy.market_churn_count("MKT-1"), 1);
}

#[test]
fn same_market_transition_replaces_changed_selection_metadata() {
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(
        active.market_selection_outcome,
        MarketSelectionOutcome::Current
    );
    assert_eq!(active.interval_end_ms, Some(301_000));

    let mut next_market = candidate_market("MKT-1", 1_000);
    next_market.selection_outcome = MarketSelectionOutcome::Next;
    next_market.expiration_ts_ms = 301_999;
    let next = selection_snapshot(
        1_000,
        SelectionState::Active {
            market: next_market,
        },
    );

    apply_selection_snapshot_to_active(&mut active, &next, 0);

    assert_eq!(
        active.market_selection_outcome,
        MarketSelectionOutcome::Next
    );
    assert_eq!(active.interval_end_ms, Some(301_999));
}
