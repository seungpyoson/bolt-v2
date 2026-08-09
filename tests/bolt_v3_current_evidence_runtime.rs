use std::{fs, path::Path, process::Command};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_current_evidence::{
        AdmissionDecisionOutcome, AdmissionDetails, AdmittedEntryAdmissionFact,
        BasketAdmissionDetails, BasketAdmissionGrantedFact, BasketAdmissionIntentKind,
        BasketAdmittedLeg, CurrentEvidenceStream, DecisionEvidenceRuntime, EvidenceOrderSide,
        EvidenceRequoteLeg, ForcedReductionAdmissionFact, NonBlockingRecordOutcome,
        ObservationRecordOutcome, ObservationStreamStatus, OrderLifecycleFact,
        OrderLifecycleOutcome, OrderLifecycleSource, OrderLifecycleTransition, OutcomeSide,
        PositiveFiniteEvidenceReadCap, RecordFailure, RecoveredSettlementOutcome,
        RequoteActionCostClass, RequoteThrottleBlockReason, RequoteThrottleBound,
        RequoteThrottleObservationFact, ReservationAttribution, ReservationProductKind,
        RiskReducingExitAdmissionFact, SettlementBookingErrorFact, SettlementBookingErrorReason,
        SettlementFact, ShadowPnlEvent, SubmitReservationFillFact, SubmitReservationFillSource,
        TerminalSettlementFact, read_backtest_run_guard_events, read_shadow_pnl_events,
    },
};
use tempfile::TempDir;

fn finite_cap(value: u64) -> PositiveFiniteEvidenceReadCap {
    PositiveFiniteEvidenceReadCap::new(value).expect("test cap must be positive and finite")
}

fn loaded_in(temp: &TempDir) -> LoadedBoltV3Config {
    loaded_at(&std::fs::canonicalize(temp.path()).expect("temporary catalog must canonicalize"))
}

fn loaded_at(catalog: &Path) -> LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config must load");
    loaded.root.persistence.catalog_directory = catalog.display().to_string();
    let evidence = &loaded.root.persistence.decision_evidence;
    for relative in [
        evidence.machine_relative_path.as_str(),
        evidence.observation_relative_path.as_str(),
    ] {
        let parent = catalog
            .join(relative)
            .parent()
            .expect("evidence path must have a parent")
            .to_path_buf();
        fs::create_dir_all(parent).expect("evidence parent must be created");
    }
    loaded
}

#[cfg(unix)]
const CATALOG_LOCK_CHILD_PATH: &str = "BOLT_TEST_CURRENT_EVIDENCE_CATALOG";
#[cfg(unix)]
const CATALOG_LOCK_CHILD_EXPECT_BLOCKED: &str = "BOLT_TEST_CURRENT_EVIDENCE_EXPECT_BLOCKED";

#[cfg(unix)]
#[test]
fn catalog_lock_child_process() {
    let Some(catalog) = std::env::var_os(CATALOG_LOCK_CHILD_PATH) else {
        return;
    };
    let loaded = loaded_at(Path::new(&catalog));
    let result = DecisionEvidenceRuntime::open(&loaded);
    if std::env::var_os(CATALOG_LOCK_CHILD_EXPECT_BLOCKED).is_some() {
        let error = result.expect_err("parent-held catalog lock must exclude this process");
        assert!(error.to_string().contains("WriterAlreadyActive"));
    } else {
        result.expect("catalog lock must become available after the owning process drops it");
    }
}

#[cfg(unix)]
fn run_catalog_lock_child(catalog: &Path, expect_blocked: bool) {
    let mut command = Command::new(std::env::current_exe().expect("test executable must resolve"));
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("--exact")
        .arg("catalog_lock_child_process")
        .arg("--nocapture")
        .env(CATALOG_LOCK_CHILD_PATH, catalog);
    if expect_blocked {
        command.env(CATALOG_LOCK_CHILD_EXPECT_BLOCKED, "1");
    }
    let status = command.status().expect("catalog-lock child must run");
    assert!(status.success(), "catalog-lock child must pass");
}

fn machine_path(loaded: &LoadedBoltV3Config) -> std::path::PathBuf {
    Path::new(&loaded.root.persistence.catalog_directory).join(
        loaded
            .root
            .persistence
            .decision_evidence
            .machine_relative_path
            .trim(),
    )
}

fn observation_path(loaded: &LoadedBoltV3Config) -> std::path::PathBuf {
    Path::new(&loaded.root.persistence.catalog_directory).join(
        loaded
            .root
            .persistence
            .decision_evidence
            .observation_relative_path
            .trim(),
    )
}

#[test]
fn runtime_holds_exclusive_catalog_ownership_for_recorder_lifetime() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let first = DecisionEvidenceRuntime::open(&loaded).expect("first runtime must open");
    let machine = machine_path(&loaded);
    let observation = observation_path(&loaded);
    let lengths_before = (
        fs::metadata(&machine)
            .expect("machine metadata must read")
            .len(),
        fs::metadata(&observation)
            .expect("observation metadata must read")
            .len(),
    );

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("second runtime must not acquire the same catalog");

    assert!(error.to_string().contains("WriterAlreadyActive"));
    assert_eq!(
        (
            fs::metadata(&machine)
                .expect("machine metadata must read")
                .len(),
            fs::metadata(&observation)
                .expect("observation metadata must read")
                .len(),
        ),
        lengths_before,
        "ownership conflict must not touch either stream"
    );
    drop(first);
    DecisionEvidenceRuntime::open(&loaded)
        .expect("catalog ownership must release when the recorder runtime drops");
}

#[cfg(unix)]
#[test]
fn catalog_ownership_excludes_an_independent_process_until_runtime_drop() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let catalog = std::fs::canonicalize(temp.path()).expect("temporary catalog must canonicalize");
    let loaded = loaded_at(&catalog);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("parent runtime must open");

    run_catalog_lock_child(&catalog, true);
    drop(runtime);
    run_catalog_lock_child(&catalog, false);
}

fn reservation_attribution() -> ReservationAttribution {
    ReservationAttribution {
        client_order_id: "client-1".to_string(),
        submit_reservation_id: "reservation-1".to_string(),
        venue_id: "POLYMARKET".to_string(),
        account_id: "POLYMARKET-001".to_string(),
        product_kind: ReservationProductKind::PredictionMarketBinary,
        collateral_currency: "USDC".to_string(),
        capital_pool_id: "pool-1".to_string(),
        collateral_group_id: "group-1".to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        side: EvidenceOrderSide::Buy,
        submitted_quantity: "1".to_string(),
        liability_factor: "1".to_string(),
        additive_liability: "0".to_string(),
        reserved_liability: "1".to_string(),
        observed_at_ns: 1,
    }
}

fn admitted_entry_with_reservation() -> AdmittedEntryAdmissionFact {
    AdmittedEntryAdmissionFact {
        details: admission_details("client-1"),
        reservation: Some(reservation_attribution()),
    }
}

fn admission_details(client_order_id: &str) -> AdmissionDetails {
    AdmissionDetails {
        strategy_id: "strategy-1".to_string(),
        execution_client_id: "execution-1".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        notional: "1".to_string(),
        loss_halt_reasons: Vec::new(),
        snapshot_present: false,
        snapshot_observed_at_ns: None,
        admission_now_ns: 1,
        snapshot_age_ns: None,
        max_snapshot_age_ns: None,
        snapshot_source: None,
        per_trade_pnl_present: false,
        daily_pnl_present: false,
        rolling_pnl_present: false,
        current_equity_present: false,
        peak_equity_present: false,
        last_account_state_observed_at_ns: None,
        last_portfolio_snapshot_observed_at_ns: None,
        last_position_event_observed_at_ns: None,
        stale_reason: None,
        loss_snapshot_observed_at_ns: None,
        loss_eval_now_ns: None,
        economics: None,
    }
}

fn reservation_fill_command() -> SubmitReservationFillFact {
    SubmitReservationFillFact {
        client_order_id: "client-1".to_string(),
        submit_reservation_id: "reservation-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        side: EvidenceOrderSide::Buy,
        fill_quantity: "1".to_string(),
        observed_at_ns: 2,
        reconciliation: false,
        source: SubmitReservationFillSource::NtOrderFill,
    }
}

fn settlement_fact() -> SettlementFact {
    SettlementFact {
        strategy_id: "strategy-1".to_string(),
        settlement_key: "settlement-1".to_string(),
        market_id: "market-1".to_string(),
        position_id: "position-1".to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        product_id: "product-1".to_string(),
        outcome_side: OutcomeSide::Up,
        entry_order_side: EvidenceOrderSide::Buy,
        quantity: "1".to_string(),
        entry_price: "0.4".to_string(),
        family_key: "family-1".to_string(),
        strike_price: "100".to_string(),
        resolution_instrument_id: "BTC-USD".to_string(),
        resolution_ts_event_ns: 3,
        reference_close_price: "101".to_string(),
        payout_per_share: "1".to_string(),
        terminal_value: "1".to_string(),
        realized_pnl: "0.6".to_string(),
        settlement_currency: "USDC".to_string(),
    }
}

fn lifecycle_fact() -> OrderLifecycleFact {
    OrderLifecycleFact {
        strategy_id: "strategy-1".to_string(),
        transition: OrderLifecycleTransition::SettlementBookingTerminal,
        outcome: OrderLifecycleOutcome::Flat,
        source: OrderLifecycleSource::SettlementBookingTerminal,
        market_id: Some("market-1".to_string()),
        instrument_id: Some("YES-USD.POLYMARKET".to_string()),
        position_id: Some("position-1".to_string()),
        client_order_id: None,
        prior_client_order_id: None,
        raw_reason_text: None,
        order_side: None,
        filled_quantity: None,
        residual_quantity: None,
        ts_event_ns: Some(4),
    }
}

fn terminal_fact() -> TerminalSettlementFact {
    TerminalSettlementFact {
        settlement_key: "settlement-1".to_string(),
        booking_error: SettlementBookingErrorFact {
            strategy_id: "strategy-1".to_string(),
            settlement_key: "settlement-1".to_string(),
            market_id: Some("market-1".to_string()),
            position_id: Some("position-1".to_string()),
            instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            resolution_instrument_id: Some("BTC-USD".to_string()),
            reason: SettlementBookingErrorReason::SettlementBlocked,
            detail: "blocked".to_string(),
            observed_at_ns: 4,
        },
        lifecycle: lifecycle_fact(),
    }
}

fn requote_observation() -> RequoteThrottleObservationFact {
    RequoteThrottleObservationFact {
        strategy_id: "strategy-1".to_string(),
        family_key: "family-1".to_string(),
        market_id: Some("market-1".to_string()),
        leg: EvidenceRequoteLeg::Yes,
        now_ms: 6,
        observed_at_ns: 7,
        action_cost_class: RequoteActionCostClass::CancelResubmit,
        block_reason: RequoteThrottleBlockReason::RequoteBudgetExhausted,
        bound_by: RequoteThrottleBound::RestCallWindow,
        submit_commands_in_window: 2,
        submit_command_cap: 3,
        submit_window_ms: 1_000,
        rest_cost_in_window: 4,
        rest_cap_per_minute: 5,
        rest_window_ms: 60_000,
        min_interval_ms: 100,
    }
}

#[test]
fn open_constructs_fresh_current_streams_atomically() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);

    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");

    assert!(machine_path(&loaded).is_file());
    assert!(observation_path(&loaded).is_file());
    assert!(runtime.reservation_recovery().is_empty());
    assert!(runtime.settlement_recovery().is_empty());
    assert!(runtime.booking_recovery().is_empty());
}

#[test]
fn invalid_command_is_rejected_before_the_machine_sink_is_touched() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let machine = machine_path(&loaded);

    let mut command = admitted_entry_with_reservation();
    command
        .reservation
        .as_mut()
        .expect("fixture must carry reservation attribution")
        .client_order_id
        .clear();
    let error = runtime
        .recorder()
        .record_admitted_entry_admission(command)
        .expect_err("invalid command must be rejected");

    assert!(matches!(error, RecordFailure::Rejected(_)));
    assert_eq!(
        fs::metadata(machine)
            .expect("machine stream must exist")
            .len(),
        0
    );
}

#[test]
fn append_uses_the_validated_descriptor_after_the_path_is_replaced() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let active = machine_path(&loaded);
    let retained = active.with_file_name("retained-machine.jsonl");
    fs::rename(&active, &retained).expect("validated stream must be renamed");
    fs::write(&active, b"").expect("replacement stream must be created");

    let _committed = runtime
        .recorder()
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("record must append through the retained descriptor");

    assert!(
        fs::metadata(retained)
            .expect("retained stream must exist")
            .len()
            > 0
    );
    assert_eq!(
        fs::metadata(active)
            .expect("replacement stream must exist")
            .len(),
        0
    );
}

#[test]
fn restart_refuses_a_fill_linked_to_the_wrong_reservation() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let recorder = runtime.recorder();
    let _committed = recorder
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("metadata must append");
    let mut fill = reservation_fill_command();
    fill.submit_reservation_id = "wrong-reservation".to_string();
    recorder
        .record_submit_reservation_fill(fill)
        .expect("well-formed but contradictory fill must append");
    drop(recorder);
    drop(runtime);

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("restart must reject contradictory reservation linkage");
    assert!(
        format!("{error:#}").contains("does not match submit-reservation attribution"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn settlement_restart_retains_the_complete_semantic_fact() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let expected = settlement_fact();
    let _committed = runtime
        .recorder()
        .record_settlement(expected.clone())
        .expect("settlement must append");
    drop(runtime);

    let restarted =
        DecisionEvidenceRuntime::open(&loaded).expect("current settlement must recover");
    assert_eq!(
        restarted
            .settlement_recovery()
            .outcomes()
            .get(&expected.settlement_key),
        Some(&RecoveredSettlementOutcome::Successful(expected))
    );
}

#[test]
fn restart_rejects_duplicate_or_conflicting_settlement_outcomes() {
    for (name, write_facts) in [
        (
            "duplicate-success",
            vec![
                (Some(settlement_fact()), None),
                (Some(settlement_fact()), None),
            ],
        ),
        (
            "success-then-terminal",
            vec![
                (Some(settlement_fact()), None),
                (None, Some(terminal_fact())),
            ],
        ),
        (
            "duplicate-terminal",
            vec![(None, Some(terminal_fact())), (None, Some(terminal_fact()))],
        ),
    ] {
        let temp = tempfile::tempdir().expect("tempdir must exist");
        let loaded = loaded_in(&temp);
        let runtime =
            DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
        for (settlement, terminal) in write_facts {
            if let Some(settlement) = settlement {
                let _committed = runtime
                    .recorder()
                    .record_settlement(settlement)
                    .unwrap_or_else(|error| panic!("{name}: settlement must append: {error}"));
            }
            if let Some(terminal) = terminal {
                runtime
                    .recorder()
                    .record_terminal_settlement(terminal)
                    .unwrap_or_else(|error| panic!("{name}: terminal must append: {error}"));
            }
        }
        drop(runtime);
        let error = DecisionEvidenceRuntime::open(&loaded)
            .expect_err("duplicate or conflicting terminal outcomes must fail startup");
        assert!(
            format!("{error:#}").contains("duplicate or conflicting terminal settlement outcome"),
            "{name}: unexpected error: {error:#}"
        );
    }
}

#[test]
fn contradictory_terminal_booking_error_is_rejected_before_io() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let machine = machine_path(&loaded);
    let fact = TerminalSettlementFact {
        settlement_key: "settlement-1".to_string(),
        booking_error: SettlementBookingErrorFact {
            strategy_id: "strategy-1".to_string(),
            settlement_key: "different-settlement".to_string(),
            market_id: Some("market-1".to_string()),
            position_id: Some("position-1".to_string()),
            instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            resolution_instrument_id: Some("BTC-USD".to_string()),
            reason: SettlementBookingErrorReason::SettlementBlocked,
            detail: "blocked".to_string(),
            observed_at_ns: 4,
        },
        lifecycle: lifecycle_fact(),
    };

    let error = runtime
        .recorder()
        .record_terminal_settlement(fact)
        .expect_err("contradictory terminal fact must be rejected");
    assert!(matches!(error, RecordFailure::Rejected(_)));
    assert_eq!(
        fs::metadata(machine)
            .expect("machine stream must exist")
            .len(),
        0
    );
}

#[test]
fn open_strictly_decodes_current_recovery_facts() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let initial = DecisionEvidenceRuntime::open(&loaded).expect("fresh stream must open");
    let _committed = initial
        .recorder()
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("atomic admitted fact must append");
    drop(initial);
    let current =
        fs::read_to_string(machine_path(&loaded)).expect("current recovery line must read");

    let runtime = DecisionEvidenceRuntime::open(&loaded)
        .expect("current recovery identity and payload must decode");
    assert!(!runtime.reservation_recovery().is_empty());
    drop(runtime);

    let malformed = current.replace(
        "\"reserved_liability\":\"1\"",
        "\"reserved_liability\":\"1\",\"unexpected\":true",
    );
    fs::write(machine_path(&loaded), malformed).expect("malformed relevant line must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("unknown relevant payload field must fail closed");
    assert!(
        format!("{error:#}").contains("malformed current payload"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn recovery_projects_every_committed_non_reservation_authorization() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let initial = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let recorder = initial.recorder();

    let _entry = recorder
        .record_admitted_entry_admission(AdmittedEntryAdmissionFact {
            details: admission_details("entry-unreserved"),
            reservation: None,
        })
        .expect("unreserved entry admission must append");
    assert!(matches!(
        recorder.record_risk_reducing_exit_admission(RiskReducingExitAdmissionFact {
            details: admission_details("risk-reducing"),
            outcome: AdmissionDecisionOutcome::Admitted,
        }),
        NonBlockingRecordOutcome::Appended(_)
    ));
    assert!(matches!(
        recorder.record_forced_reduction_admission(ForcedReductionAdmissionFact {
            details: admission_details("forced-reduction"),
            outcome: AdmissionDecisionOutcome::Admitted,
        }),
        NonBlockingRecordOutcome::Appended(_)
    ));
    let _basket = recorder
        .record_basket_admission_granted(BasketAdmissionGrantedFact {
            details: BasketAdmissionDetails {
                strategy_id: "strategy-1".to_string(),
                execution_client_id: "execution-1".to_string(),
                basket_id: "basket-1".to_string(),
                group_id: "group-1".to_string(),
                leg_instrument_ids: vec![
                    "YES-USD.POLYMARKET".to_string(),
                    "NO-USD.POLYMARKET".to_string(),
                ],
                total_notional: "2".to_string(),
                leg_order_count: 2,
            },
            admitted_legs: vec![
                BasketAdmittedLeg {
                    client_order_id: "basket-entry".to_string(),
                    instrument_id: "YES-USD.POLYMARKET".to_string(),
                    intent_kind: BasketAdmissionIntentKind::Entry,
                    reservation: None,
                },
                BasketAdmittedLeg {
                    client_order_id: "basket-risk-reducing".to_string(),
                    instrument_id: "NO-USD.POLYMARKET".to_string(),
                    intent_kind: BasketAdmissionIntentKind::RiskReducingExit,
                    reservation: None,
                },
            ],
        })
        .expect("basket admission must append");
    drop(recorder);
    drop(initial);

    let runtime =
        DecisionEvidenceRuntime::open(&loaded).expect("committed authorizations must recover");
    let recovery = runtime.reservation_recovery();
    for client_order_id in [
        "entry-unreserved",
        "risk-reducing",
        "forced-reduction",
        "basket-entry",
        "basket-risk-reducing",
    ] {
        assert!(
            recovery.authorizes_non_reservation_order(client_order_id),
            "{client_order_id} must remain authorized after restart"
        );
    }
    assert!(
        recovery.authorizes_forced_reduction_order("forced-reduction"),
        "forced-reduction liveness must remain distinguishable"
    );
}

#[test]
fn open_refuses_any_retired_path_presence() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let retired = temp.path().join(
        &loaded
            .root
            .persistence
            .decision_evidence
            .retired_relative_paths[0],
    );
    fs::create_dir_all(retired.parent().expect("retired path must have a parent"))
        .expect("retired parent must exist");
    fs::write(&retired, b"").expect("retired path must be created");

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("retired path presence must block activation");
    assert!(error.to_string().contains("retired decision-evidence path"));
}

#[test]
fn open_refuses_foreign_or_observation_identity_in_machine_stream() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    fs::write(
        machine_path(&loaded),
        b"{\"kind\":\"strategy_input_snapshot\",\"schema_version\":15,\"gate_id\":\"bolt_v3.strategy_input_snapshot\",\"gate_version\":\"pre-cutover\",\"recorded_at_utc_ns\":1}\n",
    )
    .expect("old line must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("pre-cutover identity must block activation");
    assert!(error.to_string().contains("unsupported exact identity"));

    fs::write(
        machine_path(&loaded),
        b"{\"kind\":\"blocked_strategy_input_observation\",\"schema_version\":1,\"gate_id\":\"bolt_v3.strategy_input_snapshot\",\"gate_version\":\"current\",\"recorded_at_utc_ns\":1}\n",
    )
    .expect("observation line must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("observation identity in machine stream must block activation");
    assert!(
        error
            .to_string()
            .contains("observation identity in machine stream"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn open_refuses_oversized_or_malformed_machine_stream() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = 8;
    fs::write(machine_path(&loaded), b"123456789").expect("oversized stream must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("oversized machine stream must block activation");
    assert!(error.to_string().contains("exceeds configured byte cap"));

    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = 64;
    fs::write(machine_path(&loaded), b"{not-json}\n").expect("malformed stream must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("malformed machine stream must block activation");
    assert!(
        error
            .to_string()
            .contains("malformed machine evidence line 1")
    );
}

#[test]
fn open_refuses_malformed_startup_irrelevant_machine_payload() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let machine = machine_path(&loaded);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    assert!(matches!(
        runtime.recorder().record_order_lifecycle(lifecycle_fact()),
        bolt_v2::bolt_v3_current_evidence::NonBlockingRecordOutcome::Appended(_)
    ));
    drop(runtime);

    let mut line: serde_json::Value =
        serde_json::from_slice(&fs::read(&machine).expect("machine stream must read"))
            .expect("recorded lifecycle line must decode as JSON");
    line.as_object_mut()
        .expect("lifecycle line must be an object")
        .remove("lifecycle");
    let mut malformed = serde_json::to_vec(&line).expect("malformed line must serialize");
    malformed.push(b'\n');
    fs::write(&machine, malformed).expect("malformed lifecycle line must be written");

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("malformed startup-irrelevant machine payload must block activation");
    assert!(
        format!("{error:#}").contains("malformed current payload"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn invalid_observation_history_poison_is_typed_bounded_and_byte_preserving() {
    let cases: &[(&str, &[u8])] = &[
        ("blank", b"\n"),
        ("torn", b"{\"kind\":"),
        (
            "legacy identity",
            b"{\"kind\":\"requote_throttle\",\"schema_version\":15,\"gate_id\":\"bolt_v3.requote_throttle\",\"gate_version\":\"pre-cutover\",\"recorded_at_utc_ns\":1}\n",
        ),
        (
            "unknown identity",
            b"{\"kind\":\"not_registered\",\"schema_version\":1,\"gate_id\":\"not.registered\",\"gate_version\":\"current\",\"recorded_at_utc_ns\":1}\n",
        ),
        (
            "malformed observation payload",
            b"{\"kind\":\"requote_throttle\",\"schema_version\":16,\"gate_id\":\"bolt_v3.requote_throttle\",\"gate_version\":\"current\",\"recorded_at_utc_ns\":1}\n",
        ),
    ];

    for (name, invalid) in cases {
        let temp = tempfile::tempdir().expect("tempdir must exist");
        let loaded = loaded_in(&temp);
        let initial = DecisionEvidenceRuntime::open(&loaded)
            .unwrap_or_else(|error| panic!("{name}: fresh streams must open: {error:#}"));
        let _committed = initial
            .recorder()
            .record_admitted_entry_admission(admitted_entry_with_reservation())
            .unwrap_or_else(|error| panic!("{name}: machine fact must append: {error}"));
        drop(initial);
        fs::write(observation_path(&loaded), invalid).unwrap_or_else(|error| {
            panic!("{name}: invalid observation must be installed: {error}")
        });

        let runtime = DecisionEvidenceRuntime::open(&loaded).unwrap_or_else(|error| {
            panic!("{name}: observation corruption must not gate: {error:#}")
        });
        assert!(!runtime.reservation_recovery().is_empty(), "{name}");
        assert!(
            matches!(
                runtime.observation_stream_status(),
                ObservationStreamStatus::Poisoned { .. }
            ),
            "{name}"
        );
        assert!(matches!(
            runtime
                .recorder()
                .record_requote_throttle_observation(requote_observation()),
            ObservationRecordOutcome::FailureReported(RecordFailure::SinkPoisoned { .. })
        ));
        assert!(matches!(
            runtime
                .recorder()
                .record_requote_throttle_observation(requote_observation()),
            ObservationRecordOutcome::FailureSuppressed
        ));
        assert_eq!(
            fs::read(observation_path(&loaded)).expect("observation bytes must remain readable"),
            *invalid,
            "{name}"
        );
    }
}

#[test]
fn crlf_observation_history_is_poisoned_and_preserved_without_gating_machine() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let fixture = include_bytes!(
        "fixtures/bolt_v3/current_evidence/positive/requote_throttle_observation.jsonl"
    );
    let crlf = fixture
        .iter()
        .flat_map(|byte| {
            if *byte == b'\n' {
                vec![b'\r', b'\n']
            } else {
                vec![*byte]
            }
        })
        .collect::<Vec<_>>();
    fs::write(observation_path(&loaded), &crlf).expect("CRLF fixture must be written");

    let runtime = DecisionEvidenceRuntime::open(&loaded)
        .expect("observation framing must never become readiness authority");
    let ObservationStreamStatus::Poisoned { cause } = runtime.observation_stream_status() else {
        panic!("CRLF observation framing must poison its sink");
    };
    assert!(cause.contains("carriage return"), "{cause}");
    assert_eq!(
        fs::read(observation_path(&loaded)).expect("observation bytes must read"),
        crlf
    );
}

#[test]
fn oversized_observation_history_poison_does_not_gate_machine_recovery() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = 8;
    let invalid = b"123456789\n";
    fs::write(observation_path(&loaded), invalid).expect("oversized observation must be installed");

    let runtime = DecisionEvidenceRuntime::open(&loaded)
        .expect("oversized observation must not become readiness authority");
    let ObservationStreamStatus::Poisoned { cause } = runtime.observation_stream_status() else {
        panic!("oversized observation must poison its sink");
    };
    assert!(cause.contains("exceeds configured byte cap"));
    assert_eq!(
        fs::read(observation_path(&loaded)).expect("observation bytes must read"),
        invalid
    );
}

#[test]
fn observation_stream_enforces_sink_membership_without_gating_machine_recovery() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let initial = DecisionEvidenceRuntime::open(&loaded).expect("fresh streams must open");
    let _committed = initial
        .recorder()
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("machine fact must append");
    drop(initial);
    let machine_bytes = fs::read(machine_path(&loaded)).expect("machine bytes must read");
    fs::write(observation_path(&loaded), &machine_bytes)
        .expect("machine identity must be installed in observation stream");

    let runtime = DecisionEvidenceRuntime::open(&loaded)
        .expect("wrong-sink observation content must not gate machine recovery");
    assert!(!runtime.reservation_recovery().is_empty());
    assert!(matches!(
        runtime.observation_stream_status(),
        ObservationStreamStatus::Poisoned { .. }
    ));
    assert_eq!(
        fs::read(observation_path(&loaded)).expect("observation bytes must read"),
        machine_bytes
    );
}

#[test]
fn valid_observation_history_opens_available_and_remains_appendable() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let fixture = include_bytes!(
        "fixtures/bolt_v3/current_evidence/positive/requote_throttle_observation.jsonl"
    );
    fs::write(observation_path(&loaded), fixture).expect("valid observation must be installed");

    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("valid streams must open");
    assert_eq!(
        runtime.observation_stream_status(),
        ObservationStreamStatus::Available
    );
    assert!(matches!(
        runtime
            .recorder()
            .record_requote_throttle_observation(requote_observation()),
        ObservationRecordOutcome::Appended(_)
    ));
    assert!(
        fs::metadata(observation_path(&loaded))
            .expect("observation stream must exist")
            .len()
            > fixture.len() as u64
    );
}

#[test]
fn shadow_pnl_skips_irrelevant_identity_before_payload_decode() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let fixture = include_str!("fixtures/bolt_v3/current_evidence/positive/order_lifecycle.jsonl");
    let mut line: serde_json::Value = serde_json::from_str(
        fixture
            .lines()
            .next()
            .expect("order lifecycle fixture must contain a line"),
    )
    .expect("order lifecycle fixture must decode as JSON");
    line.as_object_mut()
        .expect("order lifecycle line must be an object")
        .remove("lifecycle");
    let path = temp.path().join("irrelevant-malformed-payload.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string(&line).expect("malformed line must serialize")
        ),
    )
    .expect("malformed irrelevant line must be written");

    let events = read_shadow_pnl_events(&path, finite_cap(u64::MAX - 1))
        .expect("irrelevant identity must be skipped before payload decoding");
    assert!(events.is_empty());
}

#[test]
fn shadow_pnl_refuses_stream_over_configured_read_cap() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let path = temp.path().join("oversized-shadow-pnl.jsonl");
    let fixture =
        include_bytes!("fixtures/bolt_v3/current_evidence/positive/entry_order_intent.jsonl");
    fs::write(&path, fixture).expect("shadow-PnL fixture must be written");

    let error = read_shadow_pnl_events(&path, finite_cap(fixture.len() as u64 - 1))
        .expect_err("shadow-PnL must enforce the configured evidence cap");

    assert!(error.to_string().contains("exceeds configured byte cap"));
}

#[test]
fn backtest_run_guard_skips_irrelevant_identity_before_payload_decode() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let fixture = include_str!("fixtures/bolt_v3/current_evidence/positive/order_lifecycle.jsonl");
    let mut line: serde_json::Value = serde_json::from_str(
        fixture
            .lines()
            .next()
            .expect("order lifecycle fixture must contain a line"),
    )
    .expect("order lifecycle fixture must decode as JSON");
    line.as_object_mut()
        .expect("order lifecycle line must be an object")
        .remove("lifecycle");
    let path = temp.path().join("irrelevant-malformed-payload.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string(&line).expect("malformed line must serialize")
        ),
    )
    .expect("malformed irrelevant line must be written");

    let events = read_backtest_run_guard_events(
        &path,
        finite_cap(u64::MAX - 1),
        bolt_v2::bolt_v3_current_evidence::CurrentEvidenceStream::Machine,
    )
    .expect("irrelevant identity must be skipped before payload decoding");
    assert!(events.is_empty());
}

#[test]
fn backtest_run_guard_rejects_identity_from_the_wrong_stream() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let path = temp.path().join("wrong-stream.jsonl");
    fs::write(
        &path,
        include_bytes!(
            "fixtures/bolt_v3/current_evidence/positive/requote_throttle_observation.jsonl"
        ),
    )
    .expect("observation fixture must be written");

    let error = read_backtest_run_guard_events(
        &path,
        finite_cap(u64::MAX - 1),
        CurrentEvidenceStream::Machine,
    )
    .expect_err("an observation identity must not be accepted as machine evidence");

    assert!(
        error
            .to_string()
            .contains("observation identity in machine stream")
    );
}

#[test]
fn shadow_pnl_dispositions_have_typed_reducers_for_the_complete_current_corpus() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/bolt_v3/current_evidence/positive");
    let mut entries = fs::read_dir(&corpus_dir)
        .expect("positive corpus directory must exist")
        .map(|entry| entry.expect("positive corpus entry must be readable"))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    let mut corpus = String::new();
    let mut expected_snapshots = 0;
    let mut expected_intents = 0;
    let mut expected_admissions = 0;
    for entry in entries {
        let contents =
            fs::read_to_string(entry.path()).expect("positive corpus file must be readable");
        let is_machine_fixture = match read_backtest_run_guard_events(
            &entry.path(),
            finite_cap(u64::MAX - 1),
            CurrentEvidenceStream::Machine,
        ) {
            Ok(_) => true,
            Err(machine_error) => {
                read_backtest_run_guard_events(
                    &entry.path(),
                    finite_cap(u64::MAX - 1),
                    CurrentEvidenceStream::Observation,
                )
                .unwrap_or_else(|observation_error| {
                    panic!(
                        "positive fixture must belong to exactly one registered sink: path={} machine_error={machine_error:#} observation_error={observation_error:#}",
                        entry.path().display()
                    )
                });
                false
            }
        };
        if !is_machine_fixture {
            continue;
        }
        let line_count = contents.lines().count();
        match entry
            .file_name()
            .to_str()
            .expect("fixture name must be UTF-8")
        {
            "submit_linked_strategy_input_snapshot.jsonl" => expected_snapshots += line_count,
            "entry_order_intent.jsonl" => expected_intents += line_count,
            "admitted_entry_admission.jsonl" => expected_admissions += line_count,
            _ => {}
        }
        corpus.push_str(&contents);
    }

    let path = temp.path().join("complete-current-corpus.jsonl");
    fs::write(&path, corpus).expect("combined corpus must be written");
    let events = read_shadow_pnl_events(&path, finite_cap(u64::MAX - 1))
        .expect("every relevant disposition must have a typed Shadow PnL reducer");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ShadowPnlEvent::SubmitLinkedStrategyInputSnapshot(_)))
            .count(),
        expected_snapshots
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ShadowPnlEvent::EntryOrderIntent(_)))
            .count(),
        expected_intents
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ShadowPnlEvent::AdmittedEntryAdmission(_)))
            .count(),
        expected_admissions
    );
    assert_eq!(
        events.len(),
        expected_snapshots + expected_intents + expected_admissions
    );
}

#[test]
fn open_accepts_the_exact_byte_cap_and_refuses_one_byte_over() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    let initial = DecisionEvidenceRuntime::open(&loaded).expect("fresh stream must open");
    let _committed = initial
        .recorder()
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("atomic admitted fact must append");
    drop(initial);
    let current = fs::read(machine_path(&loaded)).expect("current line must read");
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = current.len() as u64;
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("exact byte cap must be accepted");
    drop(runtime);

    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = current.len() as u64 - 1;
    let error = DecisionEvidenceRuntime::open(&loaded).expect_err("one byte over must fail closed");
    assert!(error.to_string().contains("exceeds configured byte cap"));
}

#[test]
fn open_refuses_blank_torn_unknown_and_non_regular_machine_streams() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let machine = machine_path(&loaded);

    fs::write(&machine, b"\n").expect("blank stream must be written");
    let error = DecisionEvidenceRuntime::open(&loaded).expect_err("blank line must fail closed");
    assert!(error.to_string().contains("blank machine evidence line"));

    fs::write(&machine, b"{\"kind\":").expect("torn stream must be written");
    let error = DecisionEvidenceRuntime::open(&loaded).expect_err("torn line must fail closed");
    assert!(
        error
            .to_string()
            .contains("non-newline-terminated final record")
    );

    fs::write(&machine, b"").expect("machine stream must be reset");
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("current stream must open");
    let _committed = runtime
        .recorder()
        .record_admitted_entry_admission(admitted_entry_with_reservation())
        .expect("current record must append");
    drop(runtime);
    let mut complete_without_newline = fs::read(&machine).expect("current stream must read");
    assert_eq!(complete_without_newline.pop(), Some(b'\n'));
    fs::write(&machine, complete_without_newline)
        .expect("complete non-newline record must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("complete non-newline record must fail closed");
    assert!(
        error
            .to_string()
            .contains("non-newline-terminated final record")
    );

    let fixture =
        include_bytes!("fixtures/bolt_v3/current_evidence/positive/admitted_entry_admission.jsonl");
    let crlf = fixture
        .iter()
        .flat_map(|byte| {
            if *byte == b'\n' {
                vec![b'\r', b'\n']
            } else {
                vec![*byte]
            }
        })
        .collect::<Vec<_>>();
    fs::write(&machine, crlf).expect("CRLF machine stream must be written");
    let error =
        DecisionEvidenceRuntime::open(&loaded).expect_err("CRLF machine framing must fail closed");
    assert!(error.to_string().contains("carriage return"));

    fs::write(
        &machine,
        b"{\"kind\":\"future_kind\",\"schema_version\":1,\"gate_id\":\"future\",\"gate_version\":\"current\",\"recorded_at_utc_ns\":1}\n",
    )
    .expect("unknown stream must be written");
    let error =
        DecisionEvidenceRuntime::open(&loaded).expect_err("unknown identity must fail closed");
    assert!(error.to_string().contains("unsupported exact identity"));

    fs::remove_file(&machine).expect("machine stream must be removed");
    fs::create_dir(&machine).expect("machine directory must be created");
    let error = DecisionEvidenceRuntime::open(&loaded).expect_err("directory must fail closed");
    assert!(
        format!("{error:#}").contains("regular file"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn open_refuses_symlinks_and_machine_observation_inode_aliases() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let target = temp.path().join("target.jsonl");
    fs::write(&target, b"").expect("target must be written");
    std::os::unix::fs::symlink(&target, machine_path(&loaded)).expect("symlink must be created");
    let error =
        DecisionEvidenceRuntime::open(&loaded).expect_err("machine symlink must block activation");
    assert!(
        format!("{error:#}").contains("regular file"),
        "unexpected error: {error:#}"
    );

    fs::remove_file(machine_path(&loaded)).expect("symlink must be removed");
    fs::hard_link(&target, machine_path(&loaded)).expect("machine hard link must be created");
    fs::hard_link(&target, observation_path(&loaded))
        .expect("observation hard link must be created");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("machine-observation inode alias must block activation");
    assert!(
        format!("{error:#}").contains("hard-link aliases"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn open_refuses_out_of_catalog_hard_link_without_changing_foreign_bytes() {
    let catalog = tempfile::tempdir().expect("catalog tempdir must exist");
    let outside = tempfile::tempdir().expect("outside tempdir must exist");
    let loaded = loaded_in(&catalog);
    let foreign = outside.path().join("foreign.jsonl");
    fs::write(&foreign, b"foreign\n").expect("foreign file must be written");
    fs::hard_link(&foreign, machine_path(&loaded)).expect("hard link must be created");

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("out-of-catalog hard link must block activation");

    assert!(
        format!("{error:#}").contains("hard-link aliases"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        fs::read(&foreign).expect("foreign bytes must remain readable"),
        b"foreign\n"
    );
}

#[cfg(unix)]
#[test]
fn open_refuses_intermediate_symlink_in_active_path() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    let real_parent = temp.path().join("real-active-parent");
    fs::create_dir(&real_parent).expect("real active parent must exist");
    std::os::unix::fs::symlink(&real_parent, temp.path().join("linked-active-parent"))
        .expect("intermediate symlink must exist");
    loaded
        .root
        .persistence
        .decision_evidence
        .machine_relative_path = "linked-active-parent/machine.jsonl".to_string();

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("an intermediate active symlink must block activation");
    assert!(
        format!("{error:#}").contains("symlink"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn open_refuses_intermediate_symlink_in_retired_path() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    let real_parent = temp.path().join("real-retired-parent");
    fs::create_dir(&real_parent).expect("real retired parent must exist");
    std::os::unix::fs::symlink(&real_parent, temp.path().join("linked-retired-parent"))
        .expect("intermediate symlink must exist");
    loaded
        .root
        .persistence
        .decision_evidence
        .retired_relative_paths = vec!["linked-retired-parent/old.jsonl".to_string()];

    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("an intermediate retired symlink must block activation");
    assert!(
        format!("{error:#}").contains("symlink"),
        "unexpected error: {error:#}"
    );
}
