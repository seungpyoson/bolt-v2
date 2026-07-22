use std::{fs, path::Path};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_current_evidence::{
        DecisionEvidenceRuntime, OrderLifecycleFact, OrderLifecycleOutcome,
        OrderLifecycleTransition, OutcomeSide, RecordFailure, SettlementBookingErrorFact,
        SettlementBookingErrorReason, SettlementFact, SubmitReservationFillFact,
        SubmitReservationMetadataFact, TerminalSettlementFact,
    },
};
use tempfile::TempDir;

fn loaded_in(temp: &TempDir) -> LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(Path::new("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config must load");
    loaded.root.persistence.catalog_directory = temp.path().display().to_string();
    let evidence = &loaded.root.persistence.decision_evidence;
    for relative in [
        evidence.machine_relative_path.as_str(),
        evidence.observation_relative_path.as_str(),
    ] {
        let parent = temp
            .path()
            .join(relative)
            .parent()
            .expect("evidence path must have a parent")
            .to_path_buf();
        fs::create_dir_all(parent).expect("evidence parent must be created");
    }
    loaded
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

fn reservation_metadata_command() -> SubmitReservationMetadataFact {
    SubmitReservationMetadataFact {
        client_order_id: "client-1".to_string(),
        submit_reservation_id: "reservation-1".to_string(),
        venue_id: "POLYMARKET".to_string(),
        account_id: "POLYMARKET-001".to_string(),
        product_kind: "binary".to_string(),
        collateral_currency: "USDC".to_string(),
        capital_pool_id: "pool-1".to_string(),
        collateral_group_id: "group-1".to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        side: "buy".to_string(),
        submitted_quantity: "1".to_string(),
        liability_factor: "1".to_string(),
        additive_liability: "0".to_string(),
        reserved_liability: "1".to_string(),
        observed_at_ns: 1,
        source: "submit_admission".to_string(),
    }
}

fn reservation_fill_command() -> SubmitReservationFillFact {
    SubmitReservationFillFact {
        client_order_id: "client-1".to_string(),
        submit_reservation_id: "reservation-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "YES-USD.POLYMARKET".to_string(),
        side: "buy".to_string(),
        fill_quantity: "1".to_string(),
        observed_at_ns: 2,
        reconciliation: false,
        source: "order_event".to_string(),
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
        entry_order_side: "buy".to_string(),
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
        source: "settlement".to_string(),
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

#[test]
fn open_constructs_fresh_current_streams_atomically() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);

    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");

    assert!(machine_path(&loaded).is_file());
    assert!(observation_path(&loaded).is_file());
    assert!(runtime.startup_recovery().is_empty());
}

#[test]
fn invalid_command_is_rejected_before_the_machine_sink_is_touched() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let machine = machine_path(&loaded);

    let mut command = reservation_metadata_command();
    command.client_order_id.clear();
    let error = runtime
        .recorder()
        .record_submit_reservation_metadata(command)
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

    runtime
        .recorder()
        .record_submit_reservation_metadata(reservation_metadata_command())
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
    recorder
        .record_submit_reservation_metadata(reservation_metadata_command())
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
        format!("{error:#}").contains("does not match submit-reservation metadata"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn settlement_restart_retains_the_complete_semantic_fact() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let expected = settlement_fact();
    runtime
        .recorder()
        .record_settlement(expected.clone())
        .expect("settlement must append");
    drop(runtime);

    let restarted =
        DecisionEvidenceRuntime::open(&loaded).expect("current settlement must recover");
    assert_eq!(
        restarted
            .startup_recovery()
            .settlements()
            .get(&expected.settlement_key),
        Some(&expected)
    );
}

#[test]
fn contradictory_terminal_booking_error_is_rejected_before_io() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let loaded = loaded_in(&temp);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("fresh current streams must open");
    let machine = machine_path(&loaded);
    let fact = TerminalSettlementFact {
        settlement_key: "settlement-1".to_string(),
        booking_error: Some(SettlementBookingErrorFact {
            strategy_id: "strategy-1".to_string(),
            settlement_key: "different-settlement".to_string(),
            market_id: Some("market-1".to_string()),
            position_id: Some("position-1".to_string()),
            instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            resolution_instrument_id: Some("BTC-USD".to_string()),
            reason: SettlementBookingErrorReason::SettlementBlocked,
            detail: "blocked".to_string(),
            observed_at_ns: 4,
        }),
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
    let current = concat!(
        "{\"schema_version\":16,",
        "\"recorded_at_utc_ns\":1,",
        "\"gate_id\":\"bolt_v3.submit_admission\",",
        "\"gate_version\":\"current\",",
        "\"kind\":\"submit_reservation_metadata\",",
        "\"metadata\":{",
        "\"client_order_id\":\"client-1\",",
        "\"submit_reservation_id\":\"reservation-1\",",
        "\"venue_id\":\"POLYMARKET\",",
        "\"account_id\":\"POLYMARKET-001\",",
        "\"product_kind\":\"binary\",",
        "\"collateral_currency\":\"USDC\",",
        "\"capital_pool_id\":\"pool-1\",",
        "\"collateral_group_id\":\"group-1\",",
        "\"instrument_id\":\"YES-USD.POLYMARKET\",",
        "\"side\":\"buy\",",
        "\"submitted_quantity\":\"1\",",
        "\"liability_factor\":\"1\",",
        "\"additive_liability\":\"0\",",
        "\"reserved_liability\":\"1\",",
        "\"observed_at_ns\":1,",
        "\"source\":\"submit_admission\"}}\n",
    );
    fs::write(machine_path(&loaded), current).expect("current recovery line must be written");

    let runtime = DecisionEvidenceRuntime::open(&loaded)
        .expect("current recovery identity and payload must decode");
    assert!(!runtime.startup_recovery().is_empty());

    let malformed = current.replace(
        "\"source\":\"submit_admission\"",
        "\"source\":\"submit_admission\",\"unexpected\":true",
    );
    fs::write(machine_path(&loaded), malformed).expect("malformed relevant line must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("unknown relevant payload field must fail closed");
    assert!(
        format!("{error:#}").contains("malformed relevant payload"),
        "unexpected error: {error:#}"
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
        .recovery_evidence_max_bytes = Some(8);
    fs::write(machine_path(&loaded), b"123456789").expect("oversized stream must be written");
    let error = DecisionEvidenceRuntime::open(&loaded)
        .expect_err("oversized machine stream must block activation");
    assert!(error.to_string().contains("exceeds configured byte cap"));

    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = Some(64);
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
fn open_accepts_the_exact_byte_cap_and_refuses_one_byte_over() {
    let temp = tempfile::tempdir().expect("tempdir must exist");
    let mut loaded = loaded_in(&temp);
    let current = concat!(
        "{\"schema_version\":16,",
        "\"recorded_at_utc_ns\":1,",
        "\"gate_id\":\"bolt_v3.submit_admission\",",
        "\"gate_version\":\"current\",",
        "\"kind\":\"submit_reservation_metadata\",",
        "\"metadata\":{",
        "\"client_order_id\":\"client-1\",",
        "\"submit_reservation_id\":\"reservation-1\",",
        "\"venue_id\":\"POLYMARKET\",",
        "\"account_id\":\"POLYMARKET-001\",",
        "\"product_kind\":\"binary\",",
        "\"collateral_currency\":\"USDC\",",
        "\"capital_pool_id\":\"pool-1\",",
        "\"collateral_group_id\":\"group-1\",",
        "\"instrument_id\":\"YES-USD.POLYMARKET\",",
        "\"side\":\"buy\",",
        "\"submitted_quantity\":\"1\",",
        "\"liability_factor\":\"1\",",
        "\"additive_liability\":\"0\",",
        "\"reserved_liability\":\"1\",",
        "\"observed_at_ns\":1,",
        "\"source\":\"submit_admission\"}}\n",
    );
    fs::write(machine_path(&loaded), current).expect("current line must be written");
    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = Some(current.len() as u64);
    let runtime = DecisionEvidenceRuntime::open(&loaded).expect("exact byte cap must be accepted");
    drop(runtime);

    loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes = Some(current.len() as u64 - 1);
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
            .contains("malformed machine evidence line")
    );

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
    assert!(error.to_string().contains("same file"));
}
