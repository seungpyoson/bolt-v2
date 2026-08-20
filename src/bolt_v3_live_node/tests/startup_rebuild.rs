#![cfg(test)]

use super::*;

#[test]
fn nt_projection_requests_coalesce_and_run_only_on_the_runtime_thread() {
    let calls = Rc::new(Cell::new(0_u32));
    let calls_for_trigger = Rc::clone(&calls);
    let trigger: Rc<dyn Fn()> = Rc::new(move || calls_for_trigger.set(calls_for_trigger.get() + 1));
    let requested = Arc::new(AtomicBool::new(false));

    assert!(!dispatch_requested_submit_admission_nt_projection(
        NodeState::Starting,
        Some(&trigger),
        Some(&requested),
    ));
    requested.store(true, Ordering::Release);
    requested.store(true, Ordering::Release);
    assert!(!dispatch_requested_submit_admission_nt_projection(
        NodeState::Starting,
        Some(&trigger),
        Some(&requested),
    ));
    assert_eq!(calls.get(), 0);
    assert!(dispatch_requested_submit_admission_nt_projection(
        NodeState::Running,
        Some(&trigger),
        Some(&requested),
    ));
    assert_eq!(calls.get(), 1);
    assert!(!dispatch_requested_submit_admission_nt_projection(
        NodeState::Running,
        Some(&trigger),
        Some(&requested),
    ));
    assert_eq!(calls.get(), 1);
}

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

    let health = runtime
        .operator_health_surface(None)
        .expect("operator health must surface the poisoned observation stream");
    assert_eq!(
        health.decision_evidence_observation.status,
        BoltV3OperatorHealthStatus::Degraded
    );
    assert!(
        health
            .decision_evidence_observation
            .poison_cause
            .as_deref()
            .is_some_and(|cause| !cause.is_empty())
    );
}

#[test]
fn startup_rebuild_does_not_recover_known_submit_reservation_from_nt_cache_without_provider_collateral_allowance()
 {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let metadata = fixture_reservation_attribution(
        "startup-known-client-order",
        "condition-fixture-yes.POLYMARKET",
        "buy",
        "10",
        "0.4",
        "0.3",
        "4.3",
    );
    write_admitted_entry_reservation(&loaded, &metadata);
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert_eq!(runtime.capital_admission_reconciled(), Some(false));
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

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(2_000)
        .expect("startup rebuild should preserve internal invariants");

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
        "NT cache alone must not install reservations without committed Bolt attribution"
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

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(2_000)
        .expect("startup rebuild should preserve internal invariants");

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
fn current_process_admission_remains_attributed_on_nt_reprojection() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    loaded.root.risk.loss_governor = None;
    let mut runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
    runtime
        .provider_collateral_allowance_runtime_guard
        .take()
        .expect("fixture should start provider collateral allowance runtime")
        .stop_and_join();
    seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
    runtime
        .capital_admission_runtime_feed
        .as_ref()
        .expect("fixture should configure capital-admission runtime feed")
        .lock()
        .expect("capital-admission feed should lock")
        .on_provider_collateral_allowance_snapshot(ProviderCollateralAllowanceSnapshot {
            source: crate::bolt_v3_capital_admission_state::POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE
                .to_string(),
            observed_at_ns: 1_000,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
            collateral_allowance: Decimal::new(100, 0),
        });
    assert!(
        runtime
            .rebuild_capital_admission_from_nt_cache(1_050)
            .expect("startup rebuild should preserve internal invariants")
            .accepted
    );

    let request = crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest {
        strategy_id: "current-process-strategy".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: "current-process-client-order".to_string(),
        instrument_id: "condition-fixture-yes.POLYMARKET".to_string(),
        notional: Decimal::new(4, 0),
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(10, 0),
        intent_kind: crate::bolt_v3_submit_admission::BoltV3SubmitIntentKind::Entry,
        risk_reducing_exit_proof: None,
        admission_evidence: Some(
            crate::bolt_v3_submit_admission::BoltV3CompiledOrderAdmissionEvidence {
                venue_id: "POLYMARKET".to_string(),
                product_kind:
                    crate::bolt_v3_submit_admission::BoltV3CompiledProductKind::PredictionMarketBinary,
                side: BoltV3CompiledOrderSide::Buy,
                quantity: Decimal::new(10, 0),
                effective_price: Decimal::new(40, 2),
                order_kind:
                    crate::bolt_v3_submit_admission::BoltV3CompiledOrderKind::Limit,
                liquidity:
                    crate::bolt_v3_submit_admission::BoltV3CompiledOrderLiquidity::Taker,
                quote_set_id: None,
                prediction_market_outcome: Some(
                    crate::bolt_v3_submit_admission::PredictionMarketOutcomeSide::Yes,
                ),
            },
        ),
    };
    runtime
        .submit_admission
        .admit_at(&request, 1_100)
        .expect("current-process admission should commit")
        .commit_submitted();
    seed_accepted_open_limit_order(
        &runtime,
        generic_limit_order(
            "current-process-client-order",
            "condition-fixture-yes.POLYMARKET",
            OrderSide::Buy,
            Quantity::from(10),
            Price::from("0.40"),
        ),
        "POLYMARKET-001",
    );

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(1_150)
        .expect("startup rebuild should preserve internal invariants");

    assert!(rebuild.accepted, "{rebuild:?}");
    assert_eq!(rebuild.rebuilt_reservation_count, 1);
    assert_eq!(runtime.capital_admission_reconciled(), Some(true));
    assert!(
        runtime
            .submit_admission
            .capital_admission_has_live_reservation("current-process-client-order")
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

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(2_000)
        .expect("startup rebuild should preserve internal invariants");

    assert_eq!(
        rebuild.missing_nt_account_cache_balance,
        Some(BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance {
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
        })
    );
    assert_eq!(runtime.capital_admission_reconciled(), Some(false));

    assert_eq!(
        runtime.capital_admission_reconciled(),
        Some(false),
        "missing canonical NT account state must keep admission closed"
    );
    assert!(
        runtime
            .submit_admission
            .capital_admission_state_snapshot()
            .is_none(),
        "NT account state must not publish admission without provider collateral allowance"
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

    let _ = runtime.rebuild_capital_admission_from_nt_cache(2_000);
}

#[test]
#[should_panic(expected = "capital admission canonical NT projection feed lock poisoned")]
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

    let _ = feed
        .lock()
        .expect("capital admission canonical NT projection feed lock poisoned")
        .canonical_nt_components(CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns: None,
            account_balances: Some((Decimal::ONE, Decimal::ONE)),
            open_client_order_ids: Vec::new(),
            yes_position: Decimal::ZERO,
            no_position: Decimal::ZERO,
            observed_at_ns: 2_000,
        });
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
    loaded.root.persistence.catalog_directory = std::fs::canonicalize(temp.path())
        .expect("test catalog should canonicalize")
        .to_string_lossy()
        .to_string();
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
        source: Some(crate::bolt_v3_loss_governor::LossSnapshotSource::NtLossRuntimeFeed),
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
fn startup_rebuild_nt_cached_balance_is_advisory_without_provider_collateral_allowance() {
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

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(2_000)
        .expect("startup rebuild should preserve internal invariants");

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
        "NT balance alone cannot seed admission without provider collateral allowance"
    );
}

#[test]
fn startup_rebuild_rejects_attribution_when_open_quantity_exceeds_submitted() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
    let metadata = fixture_reservation_attribution(
        "startup-overopen-client-order",
        "condition-fixture-yes.POLYMARKET",
        "buy",
        "10",
        "0.4",
        "0.3",
        "4.3",
    );
    write_admitted_entry_reservation(&loaded, &metadata);
    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

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

    let rebuild = runtime
        .rebuild_capital_admission_from_nt_cache(2_000)
        .expect("startup rebuild should preserve internal invariants");

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
