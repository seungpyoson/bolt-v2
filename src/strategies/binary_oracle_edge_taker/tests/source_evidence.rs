#![cfg(test)]

use super::*;

#[test]
fn source_owned_reference_identity_does_not_panic_nt_quote_filter() {
    let mut strategy = test_strategy();
    strategy.config.reference_venue = Some("resolution_oracle_primary".to_string());
    strategy.config.reference_instrument_id = Some("configured-reference-price".to_string());

    strategy
        .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_200))
        .expect("source-owned reference identity should not be parsed as an NT instrument");

    assert_eq!(strategy.pricing.last_reference_fair_value, None);
    assert_eq!(strategy.pricing.fast_spot, None);
}

#[test]
fn interval_open_captures_source_bound_price_to_beat_at_or_after_market_start() {
    let mut strategy = test_strategy();
    let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
    let SelectionState::Active { market } = &mut snapshot.decision.state else {
        panic!("expected active snapshot");
    };
    market.price_to_beat = Some(3_099.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0));
    assert!(strategy.active.interval_open.is_none());

    strategy.observe_reference_snapshot(&reference_tick(1_000, 3_101.0));
    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_reference_price_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
        ],
    });

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn interval_open_uses_source_bound_price_to_beat_over_reference() {
    let mut strategy = test_strategy();
    let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
    let SelectionState::Active { market } = &mut snapshot.decision.state else {
        panic!("expected active snapshot");
    };
    market.price_to_beat = Some(3_099.0);
    strategy.apply_selection_snapshot(snapshot);

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![
            oracle_venue("reference", 1.0, 3_100.0, 1_000),
            orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
        ],
    });

    assert_eq!(strategy.active.interval_open, Some(3_099.0));
}

#[test]
fn interval_open_does_not_use_fused_reference_without_source_bound_price_to_beat() {
    let mut strategy = test_strategy();
    strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

    strategy.observe_reference_snapshot(&ReferenceSnapshot {
        ts_ms: 1_000,
        topic: "platform.reference.test.spot".to_string(),
        fair_value: Some(3_107.0),
        confidence: 1.0,
        venues: vec![],
    });

    assert_eq!(strategy.active.interval_open, None);
    assert_eq!(strategy.active.last_reference_ts_ms, None);
    assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
}

#[test]
fn strike_fetch_reissues_at_interval_open_for_future_next_selection() {
    // B-fetch-at-open regression lock. With `market_selection_rule =
    // "active_or_next"`, the configured target can select a FUTURE "Next"
    // interval whose open boundary is still ahead of wall-clock. The live
    // Chainlink strike report for that interval does not exist yet, so a
    // strike fetch issued before the interval opens cannot bind
    // `price_to_beat`. The strategy must (re)issue the strike fetch once
    // wall-clock reaches the interval open. Today `apply_selection_snapshot`
    // fires `subscribe_resolution_strike` exactly once — when
    // `interval_start_ms` first changes — and never again while the same
    // interval stays selected, so the one-shot fetch fired before open is the
    // only attempt and the strike is permanently stranded. This test drives
    // the production selection path across an interval-open boundary and
    // asserts a SECOND fetch attempt at/after open. It MUST fail until the
    // strategy re-issues the strike fetch at interval open.
    let mut strategy = test_strategy();
    assert_eq!(
        strategy.context.execution_venue(),
        fixture_execution_venue(),
        "harness precondition: production execution venue must be the POLYMARKET fixture",
    );
    // Bind the resolution (strike) instrument to this instance's underlying
    // asset so it clears the fail-closed asset-binding guard inside
    // `subscribe_resolution_strike`; the symbol's leading `-`-segment must
    // equal `underlying_asset`. Without this the live-strike subscribe is a
    // no-op for an unrelated reason (asset mismatch), masking the boundary
    // bug under test.
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    // Only the FUTURE "Next" interval's instruments exist in the cache. With
    // 300s cadence and a period-aligned base, `next_start` is one full cadence
    // period ahead of the period containing `now_before_open`.
    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // 1) Refresh BEFORE the interval opens. `now_before_open` sits in the
    //    period preceding `next_start`, so the configured target selects the
    //    future "Next" interval. The strategy issues its single one-shot
    //    strike fetch here — but the live report for `next_start` does not
    //    exist yet, so this attempt cannot bind the strike.
    let now_before_open_ms = current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    strategy.refresh_selection_from_cache(now_before_open_ms);
    assert_eq!(
        strategy.active.interval_start_ms,
        Some(next_start_ms),
        "precondition: a future Next interval must be selected before its open",
    );
    assert_eq!(
        strategy.active.market_selection_outcome,
        MarketSelectionOutcome::Next,
        "precondition: the selected interval must be the future Next interval",
    );
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: no live strike can exist before the interval opens",
    );
    let fetches_before_open = strategy.resolution_strike_subscribe_count();
    assert_eq!(
        fetches_before_open, 1,
        "the one-shot fetch must fire once when the future interval is first selected",
    );

    // 2) Wall-clock reaches the interval open. The retry-timer path re-runs
    //    selection with `now >= next_start`; the SAME market is now the
    //    Current interval and its live strike report exists. The strategy must
    //    (re)issue the strike fetch for this now-open interval.
    let now_at_open_ms = next_start_ms + 1;
    strategy.refresh_selection_from_cache(now_at_open_ms);
    assert_eq!(
        strategy.active.interval_start_ms,
        Some(next_start_ms),
        "the same interval must still be selected once it opens",
    );

    let fetches_after_open = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_open > fetches_before_open,
        "strike fetch must be re-issued once wall-clock reaches interval open for a future-selected interval that has no strike yet \
         (before-open fetches={fetches_before_open}, after-open fetches={fetches_after_open})",
    );
}

#[test]
fn strike_fetch_retries_each_open_tick_until_price_to_beat_binds() {
    // F4 regression lock. When the at-open strike fetch fails to bind
    // `price_to_beat` (transient REST error, rate-limit, or the report for the
    // exact window-open second has not propagated yet), the strategy must keep
    // re-issuing the fetch on subsequent selection-retry ticks while the
    // interval stays open and `price_to_beat` is still None. The previous
    // implementation marked the interval "subscribed at open" the instant it
    // FIRED the fetch (not when it BOUND), so a single transient failure at the
    // open boundary stranded `price_to_beat = None` for the whole interval and
    // blocked every entry. This drives three retry ticks across the open
    // boundary with no strike ever binding and asserts the fetch is re-issued on
    // each open tick. It MUST fail until the at-open guard tracks binding
    // success rather than fetch-issued.
    let mut strategy = test_strategy();
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // Pre-open: the future Next interval is selected; one one-shot fetch fires.
    let now_before_open_ms = current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1;
    strategy.refresh_selection_from_cache(now_before_open_ms);
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: no live strike can exist before the interval opens",
    );
    let fetches_pre_open = strategy.resolution_strike_subscribe_count();

    // First open tick: the at-open fetch fires but never binds (the test
    // subscribe is a no-op that does not deliver an IndexPriceUpdate).
    strategy.refresh_selection_from_cache(next_start_ms + 1);
    let fetches_after_first_open_tick = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_first_open_tick > fetches_pre_open,
        "the strike fetch must be re-issued when the interval first opens",
    );
    assert!(
        strategy.active.price_to_beat.is_none(),
        "precondition: the first at-open fetch did not bind price_to_beat",
    );

    // Second open tick (next retry-timer fire): same interval still open, strike
    // still unbound -> the fetch MUST be re-issued again rather than stranded.
    strategy.refresh_selection_from_cache(next_start_ms + 2);
    let fetches_after_second_open_tick = strategy.resolution_strike_subscribe_count();
    assert!(
        fetches_after_second_open_tick > fetches_after_first_open_tick,
        "strike fetch must keep retrying on each open retry tick while price_to_beat is \
         unbound (after first open tick={fetches_after_first_open_tick}, after \
         second={fetches_after_second_open_tick})",
    );
}

#[test]
fn resolution_strike_reissue_unsubscribes_before_each_subscribe_to_defeat_nt_dedup() {
    // NT's `DataClientAdapter` keys index-price subscriptions by `instrument_id`
    // and ignores the params map (`nautilus_data` client.rs:494-498, rev 6e059dc),
    // so a bare re-subscribe with the constant resolution instrument is silently
    // swallowed and the point-in-time strike source never re-fetches for later
    // windows or retries. `subscribe_resolution_strike` must therefore unsubscribe
    // the resolution instrument immediately before every re-subscribe. This drives
    // the pre-open + at-open reissue path and asserts that pairing; it MUST fail
    // until the unsubscribe-before-subscribe is in place.
    let mut strategy = test_strategy();
    strategy.config.resolution_instrument_id = Some(format!(
        "{}-USD.CHAINLINK",
        strategy.config.underlying_asset
    ));
    let cache = register_test_strategy(&mut strategy);

    let cadence_seconds = strategy.config.cadence_seconds as i64;
    let current_period_start = 1_746_000_000_i64;
    let next_period_start = current_period_start + cadence_seconds;
    let next_start_ms = next_period_start as u64 * MILLIS_PER_SECOND_U64;
    let next_end_ms = next_start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
    let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
        &strategy.config.underlying_asset,
        &strategy.config.cadence_slug_token,
        next_period_start,
    );
    let instruments = [
        updown_binary_option(
            "token-up.POLYMARKET",
            &market_slug,
            "market-next",
            "Up",
            next_start_ms,
            next_end_ms,
        ),
        updown_binary_option(
            "token-down.POLYMARKET",
            &market_slug,
            "market-next",
            "Down",
            next_start_ms,
            next_end_ms,
        ),
    ];
    {
        let mut cache_mut = cache.borrow_mut();
        for instrument in &instruments {
            cache_mut
                .add_instrument(instrument.clone())
                .expect("test cache should accept the seeded instrument");
        }
    }

    // Pre-open selection then an at-open tick: each reissue must clear the
    // subscription before re-subscribing.
    strategy.refresh_selection_from_cache(current_period_start as u64 * MILLIS_PER_SECOND_U64 + 1);
    strategy.refresh_selection_from_cache(next_start_ms + 1);

    let events = &strategy.resolution_strike_subscribe_events;
    assert!(
        !events.is_empty(),
        "the strike (re)subscribe path must record at least one event",
    );
    let subscribes = events
        .iter()
        .filter(|event| event.action == RESOLUTION_STRIKE_SUBSCRIBE_ACTION)
        .count();
    let unsubscribes = events
        .iter()
        .filter(|event| event.action == RESOLUTION_STRIKE_UNSUBSCRIBE_ACTION)
        .count();
    assert_eq!(
        unsubscribes, subscribes,
        "every strike subscribe must be paired with a preceding unsubscribe to clear NT's \
         per-instrument index-price dedup (subscribes={subscribes}, unsubscribes={unsubscribes})",
    );
    for pair in events.chunks(2) {
        assert_eq!(
            pair[0].action, RESOLUTION_STRIKE_UNSUBSCRIBE_ACTION,
            "each reissue must unsubscribe before subscribing",
        );
        assert_eq!(pair[1].action, RESOLUTION_STRIKE_SUBSCRIBE_ACTION);
        assert_eq!(
            pair[0].instrument_id, pair[1].instrument_id,
            "the unsubscribe and subscribe must target the same resolution instrument",
        );
    }
}

#[test]
fn entry_strategy_input_evidence_records_empty_gate_identity_without_readiness_evidence() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy =
        ready_to_trade_strategy_with_decision_evidence_and_submit_admission_without_readiness_evidence(
            evidence.clone(),
            submit_admission,
        );
    register_test_strategy_with_active_instruments(&mut strategy);

    let error = strategy
        .try_submit_entry_order(1_200)
        .expect_err("submit admission should reject after evidence capture");
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "regular live path must reach submit_admission without operator readiness evidence: {error:#}"
    );

    let events = evidence.events();
    let [
        RecordedDecisionEvidenceEvent::StrategyInput(snapshot),
        RecordedDecisionEvidenceEvent::OrderIntent(_),
        RecordedDecisionEvidenceEvent::AdmissionDecision(_),
    ] = events.as_slice()
    else {
        panic!("expected strategy input, order intent, admission sequence; got {events:#?}");
    };

    assert!(
        snapshot.gate_session_hash.is_empty(),
        "regular path carries no operator gate session hash: {:?}",
        snapshot.gate_session_hash
    );
    assert!(
        snapshot.selected_market_key.is_empty(),
        "regular path carries no operator selected-market key: {:?}",
        snapshot.selected_market_key
    );
    assert!(
        snapshot.gate_evidence.is_empty(),
        "regular path carries no operator gate evidence: {:?}",
        snapshot.gate_evidence
    );
    assert_eq!(
        snapshot.price_to_beat_value, "3100",
        "live source-bound entry snapshot is still captured"
    );
}

#[test]
fn strategy_input_evidence_records_source_bound_entry_snapshot_before_order_intent() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);

    let error = strategy
        .try_submit_entry_order(1_200)
        .expect_err("submit admission should reject after evidence capture");
    assert!(
        error.to_string().contains("notional cap is exceeded"),
        "{error:#}"
    );

    let events = evidence.events();
    let [
        RecordedDecisionEvidenceEvent::StrategyInput(snapshot),
        RecordedDecisionEvidenceEvent::OrderIntent(intent),
        RecordedDecisionEvidenceEvent::AdmissionDecision(admission),
    ] = events.as_slice()
    else {
        panic!("expected strategy input, order intent, admission sequence; got {events:#?}");
    };

    assert_eq!(snapshot.strategy_id, strategy.config.strategy_id);
    assert_eq!(snapshot.gate_session_hash, "gate-session-hash-one");
    assert_eq!(snapshot.selected_market_key, "selected-market-key-one");
    assert_eq!(
        snapshot
            .gate_evidence
            .get("resolution_price")
            .and_then(|identity| identity.normalized_value_sha256.as_deref()),
        Some("normalized-value-sha-one")
    );
    assert_eq!(snapshot.price_to_beat_value, "3100");
    assert_eq!(snapshot.reference_quote_ts_event, 1_200);
    assert_eq!(snapshot.spot_price, "3100.5");
    assert_eq!(snapshot.realized_volatility, "1.5");
    assert_eq!(snapshot.seconds_to_market_end, 300);
    assert_eq!(snapshot.market_id.as_deref(), Some("MKT-1"));
    assert_eq!(
        snapshot.polymarket_condition_id.as_deref(),
        Some("condition-MKT-1")
    );
    assert_eq!(
        snapshot.polymarket_market_slug.as_deref(),
        Some("slug-MKT-1")
    );
    assert_eq!(
        snapshot.polymarket_question_id.as_deref(),
        Some("question-MKT-1")
    );
    assert_eq!(
        snapshot.up_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
    );
    assert_eq!(
        snapshot.down_instrument_id.as_deref(),
        Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
    );
    assert_eq!(snapshot.selected_side.as_deref(), Some("up"));
    assert_eq!(snapshot.submission_instrument_id, intent.instrument_id);
    assert_eq!(snapshot.submission_order_side, intent.order_side);
    assert_eq!(snapshot.submission_price, intent.price);
    assert_eq!(snapshot.submission_quantity, intent.quantity);
    assert_eq!(snapshot.client_order_id, intent.client_order_id);
    assert_eq!(admission.client_order_id, intent.client_order_id);
    assert_eq!(
        admission.outcome,
        crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::RejectedNotionalCapExceeded
    );
}

#[test]
fn strategy_input_evidence_market_end_uses_selection_expiry_not_remaining_seconds() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.interval_end_ms = Some(301_999);

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(snapshot.seconds_to_market_end, 299);
    assert_eq!(snapshot.market_selection_timestamp_ms, Some(1_000));
    assert_eq!(snapshot.polymarket_market_start_timestamp_ms, Some(1_000));
    assert_eq!(
        snapshot.polymarket_market_end_timestamp_ms,
        Some(301_999),
        "market end must bind to selected expiration without seconds rounding"
    );
}

#[test]
fn strategy_input_evidence_records_next_market_selection_outcome() {
    let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
    let submit_admission = submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
    let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        evidence.clone(),
        submit_admission,
    );
    register_test_strategy_with_active_instruments(&mut strategy);
    strategy.active.market_selection_outcome = MarketSelectionOutcome::Next;

    strategy
        .try_submit_entry_order(2_000)
        .expect_err("submit admission should reject after evidence capture");

    let events = evidence.events();
    let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
        panic!("expected first evidence event to be strategy input; got {events:#?}");
    };

    assert_eq!(snapshot.market_selection_outcome, "next");
}

#[test]
fn observe_resolution_strike_binds_strike_at_interval_open() {
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.phase, SelectionPhase::Active);
    assert_eq!(active.interval_start_ms, Some(1_000));
    assert_eq!(active.price_to_beat, None);

    active.observe_resolution_strike(3_100.5, 1_000, 1_250);

    assert_eq!(
        active.price_to_beat,
        Some(3_100.5),
        "a positive strike bound to the interval-open must set price_to_beat"
    );
    assert_eq!(active.last_resolution_ts_ms, Some(1_250));
}

#[test]
fn observe_resolution_strike_rejects_mismatched_window_fail_closed() {
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.interval_start_ms, Some(1_000));

    // Window-open boundary does not match the market's interval-open.
    active.observe_resolution_strike(3_100.5, 2_000, 2_250);

    assert_eq!(
        active.price_to_beat, None,
        "a strike whose window-open does not match the interval-open must be ignored (fail-closed)"
    );
    assert_eq!(active.last_resolution_ts_ms, None);
}

/// H-observe-log: a window-mismatch rejection in a *configured* (non-Idle,
/// interval-bound) market is an actionable fail-closed anomaly — the strike
/// feed disagrees with the selected interval. It must be observable, and it
/// must be observably distinct from an Idle drop (where nothing is running,
/// so a mismatched update is simply not relevant).
///
/// Behavioral contract under both cases: `price_to_beat` stays `None`. The
/// distinguishing observable: a non-Idle mismatch records an observable
/// rejection, while an Idle drop does not.
#[test]
fn observe_resolution_strike_window_mismatch_is_observable_and_distinct_from_idle() {
    // Configured (non-Idle) market whose interval-open is 1_000.
    let mut active =
        ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
    assert_eq!(active.phase, SelectionPhase::Active);
    assert_eq!(active.interval_start_ms, Some(1_000));
    assert_eq!(active.price_to_beat, None);
    assert_eq!(active.resolution_strike_window_mismatch_count, 0);

    // A strike whose window-open (2_000) does not match the interval-open.
    active.observe_resolution_strike(3_100.5, 2_000, 2_250);

    // Behavioral contract: fail-closed, price_to_beat untouched.
    assert_eq!(
        active.price_to_beat, None,
        "configured window mismatch must leave price_to_beat None (fail-closed)"
    );
    assert_eq!(active.last_resolution_ts_ms, None);

    // Observable rejection: a configured mismatch must be recorded so the
    // anomaly is visible, not silently dropped.
    assert_eq!(
        active.resolution_strike_window_mismatch_count, 1,
        "a configured (non-Idle) window mismatch must record an observable rejection"
    );

    // An Idle drop is NOT the same event: nothing is configured, so a
    // mismatched update is irrelevant and must not be recorded as an anomaly.
    let mut idle = ActiveMarketState::idle();
    assert_eq!(idle.phase, SelectionPhase::Idle);
    assert_eq!(idle.resolution_strike_window_mismatch_count, 0);

    idle.observe_resolution_strike(3_100.5, 2_000, 2_250);

    assert_eq!(
        idle.price_to_beat, None,
        "Idle drop must leave price_to_beat None"
    );
    assert_eq!(
        idle.resolution_strike_window_mismatch_count, 0,
        "an Idle drop must be handled distinctly from a configured mismatch \
         (no observable rejection recorded)"
    );
}
