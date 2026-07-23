#![cfg(test)]

use super::*;

#[test]
fn live_node_surfaces_poisoned_observation_stream_without_gating_startup() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let observation_path = temp.path().join(
        &loaded
            .root
            .persistence
            .decision_evidence
            .observation_relative_path,
    );
    std::fs::write(&observation_path, b"\n")
        .expect("invalid observation history must be installed");

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("observation corruption must not gate the live node");

    assert!(matches!(
        runtime.decision_evidence_observation_status(),
        crate::bolt_v3_current_evidence::ObservationStreamStatus::Poisoned { .. }
    ));
}

#[test]
fn startup_rebuild_does_not_recover_known_submit_reservation_from_nt_cache_without_venue_truth() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let metadata = fixture_submit_reservation_metadata(
        "startup-known-client-order",
        "condition-fixture-yes.POLYMARKET",
        "buy",
        "10",
        "0.4",
        "0.3",
        "4.3",
    );
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
    write_submit_reservation_metadata(&loaded, &metadata);
    seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
    seed_accepted_open_limit_order(
        &runtime,
        generic_limit_order(
            "startup-known-client-order",
            "condition-fixture-yes.POLYMARKET",
            OrderSide::Buy,
            Quantity::from(6),
            Price::from("0.40"),
        ),
        "POLYMARKET-001",
    );

    let rebuild = runtime.rebuild_capital_admission_from_nt_cache(2_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        }
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
    assert_eq!(
        runtime
            .submit_admission
            .capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert!(
        !runtime
            .submit_admission
            .capital_admission_has_live_reservation("startup-known-client-order"),
        "NT cache alone must not install recovered reservations without accepted venue truth"
    );
}

#[test]
fn startup_rebuild_stays_closed_for_unknown_nt_cache_order() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
    seed_accepted_open_limit_order(
        &runtime,
        generic_limit_order(
            "startup-unknown-client-order",
            "condition-fixture-yes.POLYMARKET",
            OrderSide::Buy,
            Quantity::from(6),
            Price::from("0.40"),
        ),
        "POLYMARKET-001",
    );

    let rebuild = runtime.rebuild_capital_admission_from_nt_cache(2_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        }
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
    assert!(
        !runtime
            .submit_admission
            .capital_admission_has_live_reservation("startup-unknown-client-order")
    );
}

#[test]
fn startup_rebuild_guard_aborts_before_live_node_run_for_unattributed_open_orders() {
    let rebuild = BoltV3SubmitCapitalAdmissionRebuildDecision {
        accepted: false,
        reason: Some(ReservationRejectionReason::MissingEvidence),
        attempted_reservation_count: 1,
        rebuilt_reservation_count: 0,
        live_reserved_liability: Decimal::ZERO,
        missing_nt_account_cache_balance: None,
    };

    let error = fail_closed_on_unreconciled_startup_rebuild(rebuild.clone())
        .expect_err("unattributed startup open orders must abort before NT runner entry");

    match error {
        BoltV3LiveNodeError::StartupCapitalAdmissionRebuild(decision) => {
            assert_eq!(decision, rebuild);
        }
        other => panic!("unexpected startup rebuild guard error: {other:?}"),
    }
}

#[test]
fn startup_rebuild_reports_missing_nt_account_cache_balance() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    let rebuild = runtime.rebuild_capital_admission_from_nt_cache(2_000);

    assert_eq!(
        rebuild.missing_nt_account_cache_balance,
        Some(BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance {
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
        })
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));

    let feed = runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission runtime feed");
    let account_state = account_state_event("POLYMARKET-001", "PUSD", 100.0, 100.0, 2_100);
    assert!(
        feed.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_account_state(&account_state)
            .is_none(),
        "NT account state is advisory-only and cannot publish money readiness without venue truth"
    );
    assert_eq!(
        runtime.capital_admission_reconciled(),
        Some(false),
        "NT account state cannot convert missing startup venue truth into readiness"
    );
    assert!(
        runtime
            .submit_admission
            .capital_admission_state_snapshot()
            .is_none(),
        "NT account state must not publish capital admission state before accepted venue truth"
    );
}

#[test]
#[should_panic(expected = "capital admission rebuild configuration feed lock poisoned")]
fn startup_rebuild_panics_on_poisoned_capital_admission_config_feed_lock() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    let feed = runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission runtime feed");
    poison_mutex(feed);

    runtime.rebuild_capital_admission_from_nt_cache(2_000);
}

#[test]
#[should_panic(expected = "capital admission rebuild cache seed feed lock poisoned")]
fn startup_rebuild_seed_panics_on_poisoned_capital_admission_feed_lock() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    let feed = runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission runtime feed");
    poison_mutex(feed);

    seed_capital_admission_runtime_feed_from_nt_cache(
        feed,
        Some((Decimal::ONE, Decimal::ONE)),
        true,
        &[],
        Decimal::ZERO,
        Decimal::ZERO,
        2_000,
    );
}

#[test]
fn live_node_build_does_not_apply_loss_halt_before_first_trusted_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
}

#[test]
fn with_resolved_health_and_start_builds_reuse_one_secret_resolution() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    // Keep the real node builders hermetic: redirect the decision-evidence
    // catalog directory under a tempdir so registration does not touch the
    // production `/var/lib/bolt` path, which is unwritable in CI. The
    // one-resolution property below is unaffected by this storage path.
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    crate::bolt_v3_current_evidence::prepare_test_generation(&loaded);
    let secret_bearing_clients = loaded
        .root
        .clients
        .values()
        .filter(|client| client.secrets.is_some())
        .count();
    let resolved_clients = Rc::new(RefCell::new(BTreeSet::<String>::new()));
    let resolved_clients_for_resolver = Rc::clone(&resolved_clients);
    // Count EVERY resolver invocation, not just unique client names: a
    // re-resolution of an already-seen client would not grow the name set
    // above, so the per-invocation counter is the load-bearing "resolved
    // exactly once" guard.
    let resolver_calls = Rc::new(Cell::new(0u32));
    let resolver_calls_for_resolver = Rc::clone(&resolver_calls);

    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
        resolver_calls_for_resolver.set(resolver_calls_for_resolver.get() + 1);
        if path.starts_with("/bolt/polymarket/") {
            resolved_clients_for_resolver
                .borrow_mut()
                .insert("polymarket_main".to_string());
        } else if path.starts_with("/bolt/testnet/chainlink/") {
            let mut clients = resolved_clients_for_resolver.borrow_mut();
            if !clients.contains("chainlink_reference") {
                clients.insert("chainlink_reference".to_string());
            } else {
                clients.insert("chainlink_strike".to_string());
            }
        } else if path.starts_with("/bolt/polyresearch/") {
            resolved_clients_for_resolver
                .borrow_mut()
                .insert("polyresearch_reference".to_string());
        }
        fixture_secret_value(path)
    })
    .expect("fixture secrets should resolve once");
    assert_eq!(resolved.clients.len(), secret_bearing_clients);
    assert_eq!(resolved_clients.borrow().len(), secret_bearing_clients);
    // Snapshot the total resolver invocations performed by the single
    // upstream resolve; the builders below must not add to this.
    let resolver_calls_after_resolve = resolver_calls.get();

    build_bolt_v3_strategy_free_live_node_with_resolved(&loaded, &resolved)
        .expect("strategy-free health builder must consume pre-resolved secrets");
    build_bolt_v3_live_node_with_resolved(&loaded, &resolved)
        .expect("start builder must consume pre-resolved secrets");

    assert_eq!(
        resolver_calls.get(),
        resolver_calls_after_resolve,
        "with-resolved builders must not invoke the secret resolver again, \
         including re-resolving an already-resolved client"
    );
}

#[test]
fn manual_recovery_evidence_clears_live_reducing_state_after_fresh_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    runtime
        .node
        .kernel()
        .risk_engine()
        .borrow_mut()
        .set_trading_state(TradingState::Reducing);
    runtime.submit_admission.update_loss_snapshot(LossSnapshot {
        source: "nt_loss_runtime_feed".to_string(),
        observed_at_ns: 2_000,
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(Decimal::ZERO),
        rolling_pnl: Some(Decimal::ZERO),
        current_equity: Some(Decimal::new(100, 0)),
        peak_equity: Some(Decimal::new(100, 0)),
        source_observations: LossSourceObservationTimestamps::unobserved(),
    });
    let evidence = LossGovernorManualRecoveryEvidence::new(
        "operator-primary",
        "loss-governor/manual-recovery.json",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        2_050,
        256,
    )
    .expect("bounded manual recovery evidence should validate");

    let target = runtime.apply_loss_governor_manual_recovery(&evidence, 2_100);

    assert_eq!(target, Some(TradingState::Active));
    assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
}

#[test]
fn startup_rebuild_nt_cached_balance_is_advisory_without_venue_truth() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    // Helper writes NT AccountBalance as (total, locked, free): total=100, locked=40, free=60.
    seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 60.0);
    {
        let account_id = AccountId::from("POLYMARKET-001");
        let cache = runtime.node.kernel().cache();
        let cache = cache.borrow();
        let account = cache
            .account_owned(&account_id)
            .expect("seeded account should be present in NT cache");
        let balances = account.balances();
        let balance = balances
            .values()
            .find(|balance| balance.currency.code.as_str() == "PUSD")
            .expect("seeded collateral balance should be present in NT cache");
        assert_eq!(balance.total.as_decimal(), Decimal::new(100, 0));
        assert_eq!(balance.locked.as_decimal(), Decimal::new(40, 0));
        assert_eq!(balance.free.as_decimal(), Decimal::new(60, 0));
    }

    let rebuild = runtime.rebuild_capital_admission_from_nt_cache(2_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        }
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
    assert!(
        runtime
            .submit_admission
            .capital_admission_state_snapshot()
            .is_none(),
        "cached NT balance is advisory and cannot seed capital admission without accepted venue truth"
    );
}

#[test]
fn startup_rebuild_rejects_known_metadata_when_open_quantity_exceeds_submitted() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let metadata = fixture_submit_reservation_metadata(
        "startup-overopen-client-order",
        "condition-fixture-yes.POLYMARKET",
        "buy",
        "10",
        "0.4",
        "0.3",
        "4.3",
    );
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    write_submit_reservation_metadata(&loaded, &metadata);
    seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
    seed_accepted_open_limit_order(
        &runtime,
        generic_limit_order(
            "startup-overopen-client-order",
            "condition-fixture-yes.POLYMARKET",
            OrderSide::Buy,
            Quantity::from(11),
            Price::from("0.40"),
        ),
        "POLYMARKET-001",
    );

    let rebuild = runtime.rebuild_capital_admission_from_nt_cache(2_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        }
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
    assert!(
        !runtime
            .submit_admission
            .capital_admission_has_live_reservation("startup-overopen-client-order")
    );
}

#[test]
fn nt_limit_order_snapshot_maps_to_generic_open_order_evidence() {
    let order = generic_limit_order(
        "client-order-1",
        "instrument-yes.VENUE-A",
        OrderSide::Buy,
        Quantity::from(10),
        Price::from("0.40"),
    );

    let evidence = nt_open_order_evidence_from_order(&order, 1_000)
        .expect("bounded NT limit order should produce generic open-order evidence");

    assert_eq!(evidence.client_order_id, "client-order-1");
    assert_eq!(evidence.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(evidence.side, BoltV3CompiledOrderSide::Buy);
    assert_eq!(evidence.open_quantity, Decimal::new(10, 0));
    assert_eq!(evidence.limit_price, Decimal::new(4, 1));
    assert_eq!(evidence.observed_at_ns, 1_000);
    assert_eq!(evidence.evidence_label, "nt_open_order_cache");
}

#[test]
fn nt_non_limit_order_snapshot_is_not_sizing_evidence() {
    let order = generic_market_order(
        "client-order-1",
        "instrument-yes.VENUE-A",
        OrderSide::Buy,
        Quantity::from(10),
    );

    assert!(nt_open_order_evidence_from_order(&order, 1_000).is_none());
}
