use std::fs;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceCommand, BoltV3DecisionEvidenceWriter,
        BoltV3DecisionEvidenceWriterExt, BoltV3RequoteActionCostClass,
        BoltV3RequoteThrottleBlockReason, BoltV3RequoteThrottleBound,
        BoltV3RequoteThrottleEvidence, BoltV3SubmitReservationFillEvidence,
        BoltV3SubmitReservationMetadataEvidence, JsonlBoltV3DecisionEvidenceWriter,
        machine_decision_evidence_path, observation_decision_evidence_path,
        read_settlement_booking_error_evidence, read_settlement_evidence,
        read_submit_reservation_recovery_evidence, read_terminal_settlement_evidence,
    },
};

#[derive(Debug)]
struct AppendOnlyDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for AppendOnlyDecisionEvidenceWriter {
    fn try_record_command(&self, _command: BoltV3DecisionEvidenceCommand) -> anyhow::Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingCommandDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for FailingCommandDecisionEvidenceWriter {
    fn try_record_command(&self, _command: BoltV3DecisionEvidenceCommand) -> anyhow::Result<()> {
        anyhow::bail!("injected decision-evidence write failure")
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn writer_implementations_supply_one_append_operation() {
    fn assert_writer<T: BoltV3DecisionEvidenceWriter>() {}
    assert_writer::<AppendOnlyDecisionEvidenceWriter>();
}

#[test]
fn generated_effect_policy_controls_write_failure_reaction() {
    let writer = FailingCommandDecisionEvidenceWriter;
    let must_precede =
        writer.record_submit_reservation_metadata(&BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-policy".to_string(),
            submit_reservation_id: "reservation-policy".to_string(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "account-policy".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "USDC".to_string(),
            capital_pool_id: "pool-policy".to_string(),
            collateral_group_id: "group-policy".to_string(),
            instrument_id: "instrument-policy".to_string(),
            side: "buy".to_string(),
            submitted_quantity: "1".to_string(),
            liability_factor: "1".to_string(),
            additive_liability: "0".to_string(),
            reserved_liability: "1".to_string(),
            observed_at_ns: 1,
            source: "policy-test".to_string(),
        });
    assert!(must_precede.is_err());

    let observation = writer.record_requote_throttle(&BoltV3RequoteThrottleEvidence {
        strategy_id: "strategy-policy".to_string(),
        family_key: "family-policy".to_string(),
        market_id: None,
        leg: "up".to_string(),
        now_ms: 1,
        observed_at_ns: 1,
        action_cost_class: BoltV3RequoteActionCostClass::FreshSubmit,
        block_reason: BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted,
        bound_by: BoltV3RequoteThrottleBound::SubmitCommandWindow,
        submit_commands_in_window: 1,
        submit_command_cap: 1,
        submit_window_ms: 1_000,
        rest_cost_in_window: 0,
        rest_cap_per_minute: 10,
        rest_window_ms: 60_000,
        min_interval_ms: 100,
    });
    assert!(observation.is_ok());
}

fn loaded_config(temp: &tempfile::TempDir) -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(std::path::Path::new("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load");
    loaded.root.persistence.catalog_directory = temp.path().display().to_string();
    loaded
}

#[test]
fn current_reservation_records_round_trip_through_machine_recovery() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config(&temp);
    let writer = JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(&loaded)
        .expect("current decision-evidence writer should open");

    writer
        .record_submit_reservation_metadata(&BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "reservation-1".to_string(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "account-1".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "USDC".to_string(),
            capital_pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            instrument_id: "instrument-1".to_string(),
            side: "buy".to_string(),
            submitted_quantity: "2".to_string(),
            liability_factor: "1".to_string(),
            additive_liability: "0".to_string(),
            reserved_liability: "2".to_string(),
            observed_at_ns: 10,
            source: "submit".to_string(),
        })
        .expect("metadata should append");
    writer
        .record_submit_reservation_fill(&BoltV3SubmitReservationFillEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "reservation-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-1".to_string(),
            side: "buy".to_string(),
            fill_quantity: "1".to_string(),
            observed_at_ns: 11,
            reconciliation: false,
            source: "fill".to_string(),
        })
        .expect("fill should append");

    let machine_path =
        machine_decision_evidence_path(&loaded).expect("machine path should resolve");
    let recovery = read_submit_reservation_recovery_evidence(&machine_path, 100_000)
        .expect("current machine stream should recover");
    let recovered = recovery
        .metadata_by_client_order_id
        .get("client-order-1")
        .expect("reservation should recover");
    assert_eq!(recovered.metadata.submit_reservation_id, "reservation-1");
    assert_eq!(recovered.fill_trade_ids, ["trade-1".to_string()].into());
}

#[test]
fn observation_records_never_append_to_the_machine_stream() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let loaded = loaded_config(&temp);
    let writer = JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(&loaded)
        .expect("current decision-evidence writer should open");

    writer
        .record_requote_throttle(&BoltV3RequoteThrottleEvidence {
            strategy_id: "strategy-1".to_string(),
            family_key: "family-1".to_string(),
            market_id: Some("market-1".to_string()),
            leg: "up".to_string(),
            now_ms: 1,
            observed_at_ns: 1_000_000,
            action_cost_class: BoltV3RequoteActionCostClass::FreshSubmit,
            block_reason: BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted,
            bound_by: BoltV3RequoteThrottleBound::SubmitCommandWindow,
            submit_commands_in_window: 1,
            submit_command_cap: 1,
            submit_window_ms: 1_000,
            rest_cost_in_window: 0,
            rest_cap_per_minute: 10,
            rest_window_ms: 60_000,
            min_interval_ms: 100,
        })
        .expect("observation should append");

    let machine_path =
        machine_decision_evidence_path(&loaded).expect("machine path should resolve");
    let observation_path =
        observation_decision_evidence_path(&loaded).expect("observation path should resolve");
    assert_eq!(
        fs::metadata(machine_path)
            .expect("machine file should exist")
            .len(),
        0
    );
    assert!(
        fs::metadata(observation_path)
            .expect("observation file should exist")
            .len()
            > 0
    );
}

#[test]
fn settlement_recovery_routes_each_identity_in_every_stream_order() {
    let fixtures = [
        include_bytes!("fixtures/bolt_v3/decision_evidence_contract/positive/settlement_v1.jsonl")
            as &[u8],
        include_bytes!(
            "fixtures/bolt_v3/decision_evidence_contract/positive/settlement_booking_error_v1.jsonl"
        ) as &[u8],
        include_bytes!(
            "fixtures/bolt_v3/decision_evidence_contract/positive/terminal_settlement_v1.jsonl"
        ) as &[u8],
    ];
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    for (index, order) in permutations.into_iter().enumerate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("machine-{index}.jsonl"));
        let mut stream = Vec::new();
        for fixture_index in order {
            stream.extend_from_slice(fixtures[fixture_index]);
        }
        fs::write(&path, &stream).unwrap();

        assert_eq!(
            read_settlement_evidence(&path, stream.len() as u64)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            read_settlement_booking_error_evidence(&path, stream.len() as u64)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            read_terminal_settlement_evidence(&path, stream.len() as u64)
                .unwrap()
                .len(),
            1
        );
    }
}
