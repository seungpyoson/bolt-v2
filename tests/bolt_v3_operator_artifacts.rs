use sha2::{Digest, Sha256};

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LiveCanaryOperatorEvidenceBlock, load_bolt_v3_config},
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3OrderIntentEvidence,
        BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitIntentKind,
    },
    bolt_v3_market_families::updown::updown_market_slug,
    bolt_v3_operator_artifacts::{WrittenOperatorArtifact, build_redacted_ssm_manifest},
    bolt_v3_tiny_canary_evidence::{
        Phase8AbortPlanSourceProofs, Phase8OperatorApprovalEnvelope, Phase8PreRunStateSourceProofs,
        Phase8StrategyInputSafetyAudit,
    },
};
use nautilus_core::Params;
use nautilus_model::{
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::{BinaryOption, InstrumentAny},
    types::{Currency, Price, Quantity},
};

mod support;
use support::repo_path;

// Test-only updown fixture values mirror tests/fixtures/bolt_v3/strategies/binary_oracle.toml.
const TEST_MARKET_SELECTION_UNDERLYING_ASSET: &str = "BTC";
const TEST_MARKET_SELECTION_CADENCE_SLUG: &str = "5m";
const TEST_MARKET_SELECTION_CURRENT_START_SECONDS: i64 = 600;
const TEST_MARKET_SELECTION_NOW_MS: u64 = 600_000;
const TEST_MARKET_SELECTION_START_MS: u64 = 600_000;
const TEST_MARKET_SELECTION_END_MS: u64 = 900_000;
const TEST_MARKET_SELECTION_SOURCE_FILE: &str = "market-selection-source.json";
const TEST_UP_INSTRUMENT_ID: &str = "condition-current-up.POLYMARKET";
const TEST_DOWN_INSTRUMENT_ID: &str = "condition-current-down.POLYMARKET";
const TEST_MARKET_ID: &str = "market-current";
const TEST_CONDITION_ID: &str = "condition-current";
const TEST_QUESTION_ID: &str = "question-current";
const TEST_UP_OUTCOME: &str = "Up";
const TEST_DOWN_OUTCOME: &str = "Down";
const TEST_BINARY_OPTION_PRICE_INCREMENT: &str = "0.001";
const TEST_BINARY_OPTION_SIZE_INCREMENT: &str = "0.01";

#[test]
fn redacted_ssm_manifest_omits_raw_paths_and_dictionary_hashes() {
    let loaded = load_bolt_v3_config(&repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load");

    let manifest =
        build_redacted_ssm_manifest(&loaded).expect("redacted SSM manifest should build");

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.record_kind, "bolt_v3.redacted_ssm_manifest.v1");
    assert_eq!(
        manifest.config_bundle_checksum,
        loaded.config_bundle_checksum
    );
    assert_eq!(manifest.aws_region, loaded.root.aws.region);
    assert_eq!(manifest.entries.len(), 6);

    let manifest_json =
        serde_json::to_string(&manifest).expect("manifest should serialize for redaction check");
    for raw_path in [
        "/bolt/polymarket_main/private_key",
        "/bolt/polymarket_main/api_key",
        "/bolt/polymarket_main/api_secret",
        "/bolt/polymarket_main/passphrase",
        "/bolt/binance_reference/api_key",
        "/bolt/binance_reference/api_secret",
    ] {
        assert!(
            !manifest_json.contains(raw_path),
            "redacted SSM manifest must not contain raw SSM path {raw_path}"
        );
        let dictionary_hash = sha256_text(raw_path);
        assert!(
            !manifest_json.contains(&dictionary_hash),
            "redacted SSM manifest must not contain dictionary-confirmable SSM path hash {dictionary_hash}"
        );
    }
    assert!(
        !manifest_json.contains("ssm_path_sha256"),
        "redacted SSM manifest schema must not expose per-path dictionary hashes"
    );

    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "private_key_ssm_path",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_key_ssm_path",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_secret_ssm_path",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "passphrase_ssm_path",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_key_ssm_path",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_secret_ssm_path",
    );
}

#[test]
fn financial_envelope_from_loaded_config_is_operator_approval_schema_compatible() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();

    let financial_envelope = bolt_v2::bolt_v3_operator_artifacts::build_phase8_financial_envelope(
        &loaded,
        strategy_instance_id,
    )
    .expect("financial envelope should build from loaded config");
    let financial_envelope_bytes = serde_json::to_vec_pretty(&financial_envelope)
        .expect("financial envelope should serialize");

    let temp = tempfile::tempdir().expect("tempdir should create");
    let financial_envelope_path = temp.path().join("phase8-financial-envelope.json");
    std::fs::write(&financial_envelope_path, financial_envelope_bytes)
        .expect("financial envelope should write");
    let financial_envelope_sha256 =
        Phase8OperatorApprovalEnvelope::sha256_file(&financial_envelope_path)
            .expect("financial envelope hash should compute");
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: String::new(),
        root_toml_path: String::new(),
        root_toml_sha256: String::new(),
        approval_envelope_sha256: String::new(),
        ssm_manifest_path: String::new(),
        ssm_manifest_sha256: String::new(),
        strategy_input_evidence_path: String::new(),
        strategy_input_evidence_sha256: String::new(),
        financial_envelope_path: financial_envelope_path.to_string_lossy().to_string(),
        financial_envelope_sha256,
        pre_run_state_path: String::new(),
        pre_run_state_sha256: String::new(),
        abort_plan_path: String::new(),
        abort_plan_sha256: String::new(),
        operator_approval_id: String::new(),
        approval_not_before_unix_secs: 0,
        approval_not_after_unix_secs: 0,
        approval_nonce_path: String::new(),
        approval_nonce_sha256: String::new(),
        approval_consumption_path: String::new(),
        canary_evidence_path: String::new(),
        strategy_cancel_path: None,
    };

    assert_eq!(
        envelope
            .approved_strategy_instance_id_hash()
            .expect("operator envelope should parse helper financial envelope"),
        sha256_text(strategy_instance_id)
    );
}

#[test]
fn approval_nonce_writer_stores_hash_only_and_refuses_overwrite() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nonce_path = temp.path().join("approval-nonce.json");

    let written = bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&nonce_path)
        .expect("approval nonce should write once");
    let nonce_bytes = std::fs::read(&nonce_path).expect("approval nonce should read");
    assert_eq!(written.sha256, hex::encode(Sha256::digest(&nonce_bytes)));

    let nonce_json: serde_json::Value =
        serde_json::from_slice(&nonce_bytes).expect("approval nonce should parse");
    assert_eq!(nonce_json["schema_version"], 1);
    assert_eq!(
        nonce_json["record_kind"],
        "bolt_v3.operator_approval_nonce.v1"
    );
    let nonce_hash = nonce_json["nonce_sha256"]
        .as_str()
        .expect("nonce hash should be a string");
    assert_eq!(nonce_hash.len(), 64);
    assert!(
        nonce_hash.chars().all(|char| char.is_ascii_hexdigit()),
        "nonce hash should be lowercase hex"
    );
    assert_eq!(
        nonce_json
            .as_object()
            .expect("nonce should be object")
            .len(),
        3
    );
    let serialized = String::from_utf8(nonce_bytes).expect("nonce JSON should be UTF-8");
    assert!(!serialized.contains("raw"));
    assert!(!serialized.contains("nonce_bytes"));
    assert!(!serialized.contains("nonce_material"));

    let overwrite_error =
        bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&nonce_path)
            .expect_err("approval nonce writer should refuse overwrite");
    assert!(
        overwrite_error.to_string().contains("already exists"),
        "overwrite error should mention existing file: {overwrite_error}"
    );
}

#[test]
fn abort_plan_writer_fails_closed_when_static_prerequisites_are_unproven() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let abort_plan_path = temp.path().join("abort-plan.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact(
        &loaded,
        strategy_instance_id,
        &abort_plan_path,
    )
    .expect_err("abort plan should fail closed until panic gate and service policy are proven");

    assert!(
        error.to_string().contains("panic gate and service policy"),
        "abort blocker should cite missing panic gate evidence: {error}"
    );
    assert!(
        !abort_plan_path.exists(),
        "failed abort plan generation must not leave a success artifact"
    );
}

#[test]
fn abort_plan_writer_emits_config_bound_artifact_from_source_proofs() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let abort_plan_path = temp.path().join("abort-plan.json");
    let proof_hashes = TestAbortPlanProofHashes::new();

    let written =
        bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact_from_source_proofs(
            &loaded,
            strategy_instance_id,
            proof_hashes.as_source_proofs(),
            &abort_plan_path,
        )
        .expect("source-proven abort paths should write artifact");

    let artifact_bytes = std::fs::read(&abort_plan_path).expect("abort plan artifact should exist");
    assert_eq!(written.sha256, hex::encode(Sha256::digest(&artifact_bytes)));

    let json: serde_json::Value =
        serde_json::from_slice(&artifact_bytes).expect("abort plan should be JSON");
    assert_eq!(
        json["execution_client_id"],
        loaded.strategies[0].config.execution_client_id.to_string()
    );
    assert_eq!(
        json["configured_target_id"],
        loaded.strategies[0]
            .config
            .target
            .get("configured_target_id")
            .and_then(|value| value.as_str())
            .expect("fixture target should have configured_target_id")
    );
    assert_eq!(json["cancel_if_open_defined"], true);
    assert_eq!(json["nt_accepted_venue_pending_abort_defined"], true);
    assert_eq!(json["partial_fill_abort_defined"], true);
    assert_eq!(json["network_partition_during_submit_abort_defined"], true);
    assert_eq!(json["panic_gate_trip_abort_defined"], true);
    assert_eq!(json["cancel_if_open_evidence_hash"], proof_hashes.cancel);
    assert_eq!(
        json["nt_accepted_venue_pending_abort_evidence_hash"],
        proof_hashes.venue_pending
    );
    assert_eq!(
        json["partial_fill_abort_evidence_hash"],
        proof_hashes.partial_fill
    );
    assert_eq!(
        json["network_partition_during_submit_abort_evidence_hash"],
        proof_hashes.network_partition
    );
    assert_eq!(
        json["panic_gate_trip_abort_evidence_hash"],
        proof_hashes.panic_gate
    );
}

#[test]
fn abort_plan_writer_emits_artifact_from_source_bundle_file() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let bundle = abort_plan_source_bundle_json();
    let bundle_path = temp.path().join("abort-source-bundle.json");
    write_json_value_and_hash(&bundle_path, &bundle);
    let output_path = temp.path().join("abort-plan.json");

    bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact_from_source_bundle_file(
        &loaded,
        strategy_instance_id,
        &bundle_path,
        100_000,
        &output_path,
    )
    .expect("source bundle should write abort-plan artifact");

    let json = read_json_value(&output_path);
    assert_eq!(json["cancel_if_open_defined"], true);
    assert_eq!(
        json["cancel_if_open_evidence_hash"],
        sha256_json_value(&bundle["cancel_if_open_evidence"])
    );
    assert_eq!(
        json["panic_gate_trip_abort_evidence_hash"],
        sha256_json_value(&bundle["panic_gate_trip_abort_evidence"])
    );
}

#[test]
fn abort_plan_writer_rejects_source_bundle_false_path_without_artifact() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut bundle = abort_plan_source_bundle_json();
    bundle["cancel_if_open_defined"] = serde_json::json!(false);
    let bundle_path = temp.path().join("abort-source-bundle.json");
    write_json_value_and_hash(&bundle_path, &bundle);
    let output_path = temp.path().join("abort-plan.json");

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact_from_source_bundle_file(
            &loaded,
            strategy_instance_id,
            &bundle_path,
            100_000,
            &output_path,
        )
        .expect_err("false cancel proof must fail closed");

    assert!(
        error.to_string().contains("cancel_if_open_defined"),
        "cancel proof failure should identify field: {error}"
    );
    assert!(
        !output_path.exists(),
        "false cancel proof must not leave abort-plan artifact"
    );
}

#[test]
fn abort_plan_writer_rejects_each_undefined_source_path_without_artifact() {
    assert_rejects_undefined_abort_plan_source_path("cancel_if_open_defined", |proofs| {
        proofs.cancel_if_open_defined = false;
    });
    assert_rejects_undefined_abort_plan_source_path("cancel_if_open_evidence_hash", |proofs| {
        proofs.cancel_if_open_evidence_hash = "invalid";
    });
    assert_rejects_undefined_abort_plan_source_path(
        "nt_accepted_venue_pending_abort_defined",
        |proofs| {
            proofs.nt_accepted_venue_pending_abort_defined = false;
        },
    );
    assert_rejects_undefined_abort_plan_source_path(
        "nt_accepted_venue_pending_abort_evidence_hash",
        |proofs| {
            proofs.nt_accepted_venue_pending_abort_evidence_hash = "invalid";
        },
    );
    assert_rejects_undefined_abort_plan_source_path("partial_fill_abort_defined", |proofs| {
        proofs.partial_fill_abort_defined = false;
    });
    assert_rejects_undefined_abort_plan_source_path("partial_fill_abort_evidence_hash", |proofs| {
        proofs.partial_fill_abort_evidence_hash = "invalid";
    });
    assert_rejects_undefined_abort_plan_source_path(
        "network_partition_during_submit_abort_defined",
        |proofs| {
            proofs.network_partition_during_submit_abort_defined = false;
        },
    );
    assert_rejects_undefined_abort_plan_source_path(
        "network_partition_during_submit_abort_evidence_hash",
        |proofs| {
            proofs.network_partition_during_submit_abort_evidence_hash = "invalid";
        },
    );
    assert_rejects_undefined_abort_plan_source_path("panic_gate_trip_abort_defined", |proofs| {
        proofs.panic_gate_trip_abort_defined = false;
    });
    assert_rejects_undefined_abort_plan_source_path(
        "panic_gate_trip_abort_evidence_hash",
        |proofs| {
            proofs.panic_gate_trip_abort_evidence_hash = "invalid";
        },
    );
}

#[test]
fn pre_run_state_writer_fails_closed_when_source_evidence_is_unproven() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let pre_run_state_path = temp.path().join("pre-run-state.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_pre_run_state_artifact(
        &loaded,
        strategy_instance_id,
        &pre_run_state_path,
    )
    .expect_err("pre-run state should fail closed until source-bound evidence exists");

    assert!(
        error.to_string().contains("pre-run state"),
        "pre-run state blocker should cite missing source-bound evidence: {error}"
    );
    assert!(
        error.to_string().contains("T121 remains blocked"),
        "pre-run state blocker should cite T121: {error}"
    );
    assert!(
        !pre_run_state_path.exists(),
        "failed pre-run state generation must not leave a success artifact"
    );
}

#[test]
fn pre_run_state_writer_emits_hash_bound_artifact_from_source_proofs() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let pre_run_state_path = temp.path().join("pre-run-state.json");
    let proof_hashes = TestPreRunStateProofHashes::new();

    let written =
        bolt_v2::bolt_v3_operator_artifacts::write_pre_run_state_artifact_from_source_proofs(
            &loaded,
            strategy_instance_id,
            proof_hashes.as_source_proofs(),
            &pre_run_state_path,
        )
        .expect("source-bound pre-run state proofs should write artifact");

    let artifact_bytes = std::fs::read(&pre_run_state_path)
        .expect("pre-run state artifact should exist after source proofs");
    assert_eq!(written.sha256, hex::encode(Sha256::digest(&artifact_bytes)));

    let json: serde_json::Value =
        serde_json::from_slice(&artifact_bytes).expect("pre-run state should be JSON");
    assert_eq!(
        json["execution_client_id"],
        loaded.strategies[0].config.execution_client_id.to_string()
    );
    assert_eq!(
        json["configured_target_id"],
        loaded.strategies[0]
            .config
            .target
            .get("configured_target_id")
            .and_then(|value| value.as_str())
            .expect("fixture target should have configured_target_id")
    );
    assert_eq!(json["host_clock_skew_within_bound"], true);
    assert_eq!(json["conflicting_open_orders_absent"], true);
    assert_eq!(json["preexisting_position_absent"], true);
    assert_eq!(json["market_state_approved"], true);
    assert_eq!(json["market_window_approved"], true);
    assert_eq!(
        json["release_manifest_clob_signing_version"],
        "clob-v2-release-test"
    );
}

#[test]
fn pre_run_state_writer_emits_artifact_from_source_bundle_file() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let bundle = pre_run_state_source_bundle_json();
    let bundle_path = temp.path().join("pre-run-source-bundle.json");
    write_json_value_and_hash(&bundle_path, &bundle);
    let output_path = temp.path().join("pre-run-state.json");

    bolt_v2::bolt_v3_operator_artifacts::write_pre_run_state_artifact_from_source_bundle_file(
        &loaded,
        strategy_instance_id,
        &bundle_path,
        100_000,
        &output_path,
    )
    .expect("source bundle should write pre-run-state artifact");

    let json = read_json_value(&output_path);
    assert_eq!(json["host_clock_skew_within_bound"], true);
    assert_eq!(
        json["host_clock_skew_evidence_hash"],
        sha256_json_value(&bundle["host_clock_evidence"])
    );
    assert_eq!(
        json["venue_account_state_evidence_hash"],
        sha256_json_value(&bundle["venue_account_state_evidence"])
    );
    assert_eq!(
        json["market_state_evidence_hash"],
        bundle["market_state_evidence_hash"]
    );
    assert_eq!(
        json["release_manifest_evidence_hash"],
        bundle["release_manifest_evidence_hash"]
    );
}

#[test]
fn pre_run_state_writer_rejects_source_bundle_false_proof_without_artifact() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut bundle = pre_run_state_source_bundle_json();
    bundle["host_clock_skew_within_bound"] = serde_json::json!(false);
    let bundle_path = temp.path().join("pre-run-source-bundle.json");
    write_json_value_and_hash(&bundle_path, &bundle);
    let output_path = temp.path().join("pre-run-state.json");

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_pre_run_state_artifact_from_source_bundle_file(
            &loaded,
            strategy_instance_id,
            &bundle_path,
            100_000,
            &output_path,
        )
        .expect_err("false host-clock proof must fail closed");

    assert!(
        error.to_string().contains("host_clock_skew_within_bound"),
        "host-clock proof failure should identify field: {error}"
    );
    assert!(
        !output_path.exists(),
        "false host-clock proof must not leave pre-run-state artifact"
    );
}

#[test]
fn pre_run_state_writer_rejects_each_unsatisfied_source_proof_without_artifact() {
    assert_rejects_unsatisfied_pre_run_source_proof("host_clock_skew_within_bound", |proofs| {
        proofs.host_clock_skew_within_bound = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("host_clock_skew_evidence_hash", |proofs| {
        proofs.host_clock_skew_evidence_hash = "invalid";
    });
    assert_rejects_unsatisfied_pre_run_source_proof("conflicting_open_orders_absent", |proofs| {
        proofs.conflicting_open_orders_absent = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("preexisting_position_absent", |proofs| {
        proofs.preexisting_position_absent = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof(
        "venue_account_state_evidence_hash",
        |proofs| {
            proofs.venue_account_state_evidence_hash = "invalid";
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof("market_state_approved", |proofs| {
        proofs.market_state_approved = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("market_window_approved", |proofs| {
        proofs.market_window_approved = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("market_state_evidence_hash", |proofs| {
        proofs.market_state_evidence_hash = "invalid";
    });
    assert_rejects_unsatisfied_pre_run_source_proof(
        "funding_margin_covers_max_notional_plus_fees",
        |proofs| {
            proofs.funding_margin_covers_max_notional_plus_fees = false;
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof("funding_margin_evidence_hash", |proofs| {
        proofs.funding_margin_evidence_hash = "invalid";
    });
    assert_rejects_unsatisfied_pre_run_source_proof("single_runner_lock_acquired", |proofs| {
        proofs.single_runner_lock_acquired = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("single_runner_lock_evidence_hash", |proofs| {
        proofs.single_runner_lock_evidence_hash = "invalid";
    });
    assert_rejects_unsatisfied_pre_run_source_proof("egress_identity_approved", |proofs| {
        proofs.egress_identity_approved = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof("egress_identity_evidence_hash", |proofs| {
        proofs.egress_identity_evidence_hash = "invalid";
    });
    assert_rejects_unsatisfied_pre_run_source_proof("clob_v2_adapter_signing_verified", |proofs| {
        proofs.clob_v2_adapter_signing_verified = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof(
        "clob_v2_adapter_signing_evidence_hash",
        |proofs| {
            proofs.clob_v2_adapter_signing_evidence_hash = "invalid";
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof(
        "clob_v2_collateral_accounting_verified",
        |proofs| {
            proofs.clob_v2_collateral_accounting_verified = false;
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof(
        "clob_v2_collateral_accounting_evidence_hash",
        |proofs| {
            proofs.clob_v2_collateral_accounting_evidence_hash = "invalid";
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof("clob_v2_fee_behavior_verified", |proofs| {
        proofs.clob_v2_fee_behavior_verified = false;
    });
    assert_rejects_unsatisfied_pre_run_source_proof(
        "clob_v2_fee_behavior_evidence_hash",
        |proofs| {
            proofs.clob_v2_fee_behavior_evidence_hash = "invalid";
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof(
        "release_manifest_clob_signing_version",
        |proofs| {
            proofs.release_manifest_clob_signing_version = "";
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof(
        "release_manifest_nt_revision_matches_compiled_pin",
        |proofs| {
            proofs.release_manifest_nt_revision_matches_compiled_pin = false;
        },
    );
    assert_rejects_unsatisfied_pre_run_source_proof("release_manifest_evidence_hash", |proofs| {
        proofs.release_manifest_evidence_hash = "invalid";
    });
}

#[test]
fn pre_run_release_manifest_source_proof_derives_source_owned_values() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let cargo_toml_path = repo_path("Cargo.toml");
    let cargo_lock_path = repo_path("Cargo.lock");
    let eip712_path = temp.path().join("eip712.rs");
    std::fs::write(
        &eip712_path,
        r#"
const CLOB_AUTH_DOMAIN_VERSION: &str = "1";
const DOMAIN_VERSION: &str = "2";
"#,
    )
    .expect("eip712 fixture should write");

    let proof = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        1024 * 1024,
    )
    .expect("source-owned release manifest proof should build");

    assert_eq!(proof.clob_signing_version, "2");
    assert!(proof.nt_revision_matches_compiled_pin);
    assert_eq!(proof.cargo_toml_sha256, sha256_file(&cargo_toml_path));
    assert_eq!(proof.cargo_lock_sha256, sha256_file(&cargo_lock_path));
    assert_eq!(proof.clob_signing_source_sha256, sha256_file(&eip712_path));
    assert_eq!(proof.evidence_hash.len(), 64);
    assert!(
        proof
            .evidence_hash
            .chars()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase()),
        "release manifest evidence hash must be lowercase sha256"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_compiled_nt_revision_drift() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let cargo_toml_path = temp.path().join("Cargo.toml");
    let cargo_lock_path = temp.path().join("Cargo.lock");
    let eip712_path = temp.path().join("eip712.rs");
    let agreed_but_uncompiled_revision = "1111111111111111111111111111111111111111";

    std::fs::write(
        &cargo_toml_path,
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{agreed_but_uncompiled_revision}" }}
"#
        ),
    )
    .expect("Cargo.toml fixture should write");
    std::fs::write(
        &cargo_lock_path,
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={agreed_but_uncompiled_revision}#{agreed_but_uncompiled_revision}"
"#
        ),
    )
    .expect("Cargo.lock fixture should write");
    std::fs::write(&eip712_path, r#"const DOMAIN_VERSION: &str = "2";"#)
        .expect("eip712 fixture should write");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        1024 * 1024,
    )
    .expect_err("release manifest proof must reject NT revision drift from compiled build pin");

    assert!(
        error.to_string().contains("release manifest"),
        "release manifest compiled-pin drift error should identify proof surface: {error}"
    );
    assert!(
        error.to_string().contains("nautilus"),
        "release manifest compiled-pin drift error should identify NT revision drift: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_nt_revision_drift() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let cargo_toml_path = temp.path().join("Cargo.toml");
    let cargo_lock_path = temp.path().join("Cargo.lock");
    let eip712_path = temp.path().join("eip712.rs");
    let cargo_toml_revision = "1111111111111111111111111111111111111111";
    let cargo_lock_revision = "2222222222222222222222222222222222222222";

    std::fs::write(
        &cargo_toml_path,
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{cargo_toml_revision}" }}
"#
        ),
    )
    .expect("Cargo.toml fixture should write");
    std::fs::write(
        &cargo_lock_path,
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={cargo_lock_revision}#{cargo_lock_revision}"
"#
        ),
    )
    .expect("Cargo.lock fixture should write");
    std::fs::write(&eip712_path, r#"const DOMAIN_VERSION: &str = "2";"#)
        .expect("eip712 fixture should write");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        1024 * 1024,
    )
    .expect_err("release manifest proof must reject Cargo.toml/Cargo.lock NT drift");

    assert!(
        error.to_string().contains("release manifest"),
        "release manifest drift error should identify proof surface: {error}"
    );
    assert!(
        error.to_string().contains("nautilus"),
        "release manifest drift error should identify NT revision drift: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_malformed_nautilus_toml_dependency() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nt_revision = current_fixture_nt_revision();
    let (cargo_toml_path, cargo_lock_path, eip712_path) = write_release_manifest_source_files(
        temp.path(),
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{nt_revision}" }}
nautilus-model = {{ path = "../local-nautilus-model" }}
"#
        ),
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={nt_revision}#{nt_revision}"
"#
        ),
        r#"const DOMAIN_VERSION: &str = "2";"#,
    );

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        1024 * 1024,
    )
    .expect_err("release manifest proof must reject non-upstream nautilus Cargo.toml dependency");

    assert!(
        error.to_string().contains("nautilus"),
        "malformed nautilus dependency error should identify NT dependency proof: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_malformed_nautilus_lock_package() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nt_revision = current_fixture_nt_revision();
    let (cargo_toml_path, cargo_lock_path, eip712_path) = write_release_manifest_source_files(
        temp.path(),
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{nt_revision}" }}
nautilus-model = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{nt_revision}" }}
"#
        ),
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={nt_revision}#{nt_revision}"

[[package]]
name = "nautilus-model"
version = "0.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#
        ),
        r#"const DOMAIN_VERSION: &str = "2";"#,
    );

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        16 * 1024,
    )
    .expect_err("release manifest proof must reject non-upstream nautilus Cargo.lock package");

    assert!(
        error.to_string().contains("nautilus"),
        "malformed nautilus package error should identify NT lock proof: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_lookalike_nautilus_toml_git_url() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nt_revision = current_fixture_nt_revision();
    let (cargo_toml_path, cargo_lock_path, eip712_path) = write_release_manifest_source_files(
        temp.path(),
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/not-nautechsystems/nautilus_trader.git", rev = "{nt_revision}" }}
"#
        ),
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={nt_revision}#{nt_revision}"
"#
        ),
        r#"const DOMAIN_VERSION: &str = "2";"#,
    );

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        16 * 1024,
    )
    .expect_err("release manifest proof must reject lookalike NT Cargo.toml git URLs");

    assert!(
        error.to_string().contains("nautilus"),
        "lookalike Cargo.toml git URL error should identify NT source proof: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_rejects_lookalike_nautilus_lock_source() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let nt_revision = current_fixture_nt_revision();
    let (cargo_toml_path, cargo_lock_path, eip712_path) = write_release_manifest_source_files(
        temp.path(),
        format!(
            r#"
[dependencies]
nautilus-polymarket = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{nt_revision}" }}
"#
        ),
        format!(
            r#"
[[package]]
name = "nautilus-polymarket"
version = "0.0.0"
source = "git+https://github.com/not-nautechsystems/nautilus_trader.git?rev={nt_revision}#{nt_revision}"
"#
        ),
        r#"const DOMAIN_VERSION: &str = "2";"#,
    );

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        16 * 1024,
    )
    .expect_err("release manifest proof must reject lookalike NT Cargo.lock sources");

    assert!(
        error.to_string().contains("nautilus"),
        "lookalike Cargo.lock source error should identify NT source proof: {error}"
    );
}

#[test]
fn pre_run_release_manifest_source_proof_ignores_prefixed_domain_version_names() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let cargo_toml_path = repo_path("Cargo.toml");
    let cargo_lock_path = repo_path("Cargo.lock");
    let eip712_path = temp.path().join("eip712.rs");
    std::fs::write(
        &eip712_path,
        r#"
const DOMAIN_VERSION_FAKE: &str = "not-the-domain";
const DOMAIN_VERSION: &str = "2";
"#,
    )
    .expect("eip712 fixture should write");

    let proof = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_release_manifest_source_proof(
        &cargo_toml_path,
        &cargo_lock_path,
        &eip712_path,
        1024 * 1024,
    )
    .expect("release manifest proof should parse exact DOMAIN_VERSION identifier");

    assert_eq!(proof.clob_signing_version, "2");
}

#[test]
fn static_operator_artifacts_report_market_selection_blocker_until_runtime_proof_exists() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");

    let outcome = bolt_v2::bolt_v3_operator_artifacts::write_static_operator_artifacts(
        &loaded,
        strategy_instance_id,
        temp.path(),
    )
    .expect("static artifacts should write redacted partial packet");

    assert!(
        outcome
            .blockers
            .iter()
            .any(|blocker| blocker.contains("market-selection")),
        "static packet should report market-selection blocker: {:?}",
        outcome.blockers
    );
    assert!(
        !temp.path().join(TEST_MARKET_SELECTION_SOURCE_FILE).exists(),
        "static packet must not write market-selection source without runtime proof"
    );
}

#[test]
fn static_operator_artifacts_validate_financial_envelope_before_first_write() {
    let loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_static_operator_artifacts(
        &loaded,
        "missing-strategy-instance",
        temp.path(),
    )
    .expect_err("invalid strategy must fail before static artifact writes");

    assert!(
        error.to_string().contains("strategy_instance_id"),
        "financial envelope validation error should name strategy instance: {error}"
    );
    assert!(
        !temp.path().join("ssm-manifest.json").exists(),
        "failed static artifact build must not leave ssm manifest"
    );
    assert!(
        !temp.path().join("financial-envelope.json").exists(),
        "failed static artifact build must not leave financial envelope"
    );
    assert!(
        !temp.path().join("static-artifacts-manifest.json").exists(),
        "failed static artifact build must not leave static manifest"
    );
}

#[test]
fn static_operator_artifacts_remove_prior_outputs_when_later_write_fails() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_nonce_path = temp.path().join("approval-nonce.json");
    std::fs::create_dir(&approval_nonce_path).expect("blocking directory should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_static_operator_artifacts(
        &loaded,
        strategy_instance_id,
        temp.path(),
    )
    .expect_err("approval nonce write failure should fail static artifact generation");

    assert!(
        error.to_string().contains("write"),
        "static artifact write failure should surface as write error: {error}"
    );
    assert!(
        !temp.path().join("ssm-manifest.json").exists(),
        "later write failure must remove prior SSM manifest"
    );
    assert!(
        !temp.path().join("financial-envelope.json").exists(),
        "later write failure must remove prior financial envelope"
    );
    assert!(
        approval_nonce_path.is_dir(),
        "cleanup must not remove pre-existing output-directory entry"
    );
}

#[test]
fn static_operator_artifacts_track_future_success_outputs_for_cleanup() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");
    let function_start = source
        .find("pub fn write_static_operator_artifacts")
        .expect("static artifact writer should exist");
    let function_end = source[function_start..]
        .find("let outcome_blockers = blockers.clone();")
        .map(|offset| function_start + offset)
        .expect("static artifact writer should build outcome after source artifact attempts");
    let writer_source = &source[function_start..function_end];

    for artifact_name in [
        "STRATEGY_INPUT_ARTIFACT_NAME",
        "PRE_RUN_STATE_ARTIFACT_NAME",
        "ABORT_PLAN_ARTIFACT_NAME",
    ] {
        let artifact_ref = format!("static_artifact_ref({artifact_name}, written)");
        let ref_index = writer_source
            .find(&artifact_ref)
            .unwrap_or_else(|| panic!("writer should reference {artifact_name}"));
        let preceding_branch = &writer_source[..ref_index];
        assert!(
            preceding_branch.ends_with(
                "written_artifacts.push(written.clone());\n            generated_artifacts.push("
            ),
            "{artifact_name} successful write must enter cleanup ledger before manifest reference"
        );
    }
}

#[test]
#[cfg(unix)]
fn approval_nonce_writer_creates_private_mode_artifact() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("approval-nonce.json");

    bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&path)
        .expect("approval nonce should write");

    let mode = std::fs::metadata(&path)
        .expect("approval nonce metadata should read")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "operator artifact files should be private by default"
    );
}

#[test]
#[cfg(unix)]
fn approval_nonce_writer_rejects_broken_symlink_output_path() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target = temp.path().join("unexpected-target.json");
    let link = temp.path().join("approval-nonce-link.json");
    std::os::unix::fs::symlink(&target, &link).expect("broken symlink should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_approval_nonce_artifact(&link)
        .expect_err("artifact writer should reject symlink output path");

    assert!(
        error.to_string().contains("write"),
        "symlink rejection should surface as write failure: {error}"
    );
    assert!(
        !target.exists(),
        "artifact writer must not follow broken symlink and create target"
    );
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "failed symlink write must leave the original symlink untouched"
    );
}

#[test]
fn static_manifest_writer_emits_blocker_free_manifest_from_operator_evidence() {
    let fixture = assembled_final_packet_fixture();
    std::fs::remove_file(&fixture.static_manifest_path)
        .expect("fixture static manifest should be removable");

    let written =
        bolt_v2::bolt_v3_operator_artifacts::write_static_artifacts_manifest_from_operator_evidence(
            &fixture.loaded,
            &fixture.static_manifest_path,
        )
        .expect("operator-evidence-bound manifest should write");

    let manifest = read_json_value(&fixture.static_manifest_path);
    assert_eq!(written.sha256, sha256_file(&fixture.static_manifest_path));
    assert_eq!(manifest["blockers"], serde_json::json!([]));
    let generated = manifest["generated_artifacts"]
        .as_array()
        .expect("generated artifacts should be an array");
    for expected_name in [
        "ssm-manifest",
        "strategy-input",
        "financial-envelope",
        "pre-run-state",
        "abort-plan",
        "approval-nonce",
    ] {
        assert!(
            generated
                .iter()
                .any(|artifact| artifact["name"] == expected_name),
            "manifest should include {expected_name}: {manifest}"
        );
    }
}

#[test]
fn approval_packet_assembly_refuses_static_manifest_with_blockers() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        Vec::new(),
        vec!["strategy-input remains blocked at /bolt/not-a-real-secret-path"],
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("static manifest blockers should fail closed");

    assert!(
        error.to_string().contains("static manifest blockers"),
        "error should name static manifest blockers: {error}"
    );
    assert!(
        !error.to_string().contains("/bolt/not-a-real-secret-path"),
        "blocker diagnostics must not echo supplied unsafe text: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "blocked assembly must not write approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "blocked assembly must not write operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_unbound_approval_envelope_hash_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    operator_evidence.head_sha = option_env!("BOLT_V3_BUILD_HEAD_SHA")
        .unwrap_or_else(|| panic!("build head sha should be compiled for packet assembly tests"))
        .to_string();
    operator_evidence.approval_envelope_sha256 = "0".repeat(64);
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("assembly must reject a configured approval_envelope_sha256 that cannot verify");

    assert!(
        error.to_string().contains("approval_envelope_sha256"),
        "unbound approval-envelope hash error should name the configured hash field: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "rejected assembly must not write approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "rejected assembly must not write operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_static_manifest_integrity_gaps() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let approval_envelope_path =
        std::path::PathBuf::from(&operator_evidence.approval_envelope_path);
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence.clone());

    for (case_name, mutate) in [
        (
            "missing strategy-input",
            Box::new(
                |refs: &mut Vec<serde_json::Value>,
                 _loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                 _operator_evidence: &LiveCanaryOperatorEvidenceBlock| {
                    refs.retain(|artifact| artifact["name"] != "strategy-input");
                },
            )
                as Box<
                    dyn FnOnce(
                        &mut Vec<serde_json::Value>,
                        &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                        &LiveCanaryOperatorEvidenceBlock,
                    ),
                >,
        ),
        (
            "path mismatch",
            Box::new(
                |refs: &mut Vec<serde_json::Value>,
                 _loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                 _operator_evidence: &LiveCanaryOperatorEvidenceBlock| {
                    refs.iter_mut()
                        .find(|artifact| artifact["name"] == "pre-run-state")
                        .expect("pre-run-state artifact should exist")["path"] =
                        serde_json::json!("/bolt/not-a-real-secret-path");
                },
            ),
        ),
        (
            "sha mismatch",
            Box::new(
                |refs: &mut Vec<serde_json::Value>,
                 _loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                 _operator_evidence: &LiveCanaryOperatorEvidenceBlock| {
                    refs.iter_mut()
                        .find(|artifact| artifact["name"] == "abort-plan")
                        .expect("abort artifact should exist")["sha256"] =
                        serde_json::json!("0".repeat(64));
                },
            ),
        ),
        (
            "file hash mismatch",
            Box::new(
                |_refs: &mut Vec<serde_json::Value>,
                 _loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                 operator_evidence: &LiveCanaryOperatorEvidenceBlock| {
                    std::fs::write(
                        &operator_evidence.financial_envelope_path,
                        b"different-artifact-bytes",
                    )
                    .expect("test artifact should mutate");
                },
            ),
        ),
        (
            "config drift",
            Box::new(
                |_refs: &mut Vec<serde_json::Value>,
                 loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
                 _operator_evidence: &LiveCanaryOperatorEvidenceBlock| {
                    loaded.config_bundle_checksum = sha256_text("different-config-bundle");
                },
            ),
        ),
    ] {
        let case_dir = temp.path().join(case_name.replace(' ', "-"));
        std::fs::create_dir_all(&case_dir).expect("case dir should create");
        let manifest_path = case_dir.join("static-artifacts-manifest.json");
        let mut case_refs = refs.clone();
        let mut case_loaded = loaded.clone();
        mutate(&mut case_refs, &mut case_loaded, &operator_evidence);
        write_static_artifacts_manifest_for_test(
            &manifest_path,
            &loaded.config_bundle_checksum,
            case_refs,
            Vec::new(),
        );
        let operator_packet_path = case_dir.join("operator-evidence-packet.json");

        let error =
            bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
                &case_loaded,
                &manifest_path,
                &operator_packet_path,
            )
            .expect_err("manifest integrity gap should fail closed");
        let message = error.to_string();

        assert!(
            message.contains("static manifest"),
            "{case_name} error should cite static manifest: {message}"
        );
        assert!(
            !message.contains("/bolt/not-a-real-secret-path"),
            "{case_name} error must not echo supplied unsafe path: {message}"
        );
        if case_name == "file hash mismatch" {
            assert!(
                !message.contains(&operator_evidence.financial_envelope_path),
                "{case_name} error must not echo configured artifact path: {message}"
            );
        }
        assert!(
            !approval_envelope_path.exists(),
            "{case_name} must not leave approval envelope"
        );
        assert!(
            !operator_packet_path.exists(),
            "{case_name} must not leave operator packet"
        );
    }
}

#[test]
fn approval_packet_assembly_writes_non_circular_envelope_from_existing_refs() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    operator_evidence.strategy_cancel_path = Some(
        temp.path()
            .join("strategy-cancel.json")
            .to_string_lossy()
            .to_string(),
    );
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence.clone());
    bind_expected_approval_envelope_hash(&mut loaded, &mut operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let outcome =
        bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
            &loaded,
            &manifest_path,
            &operator_packet_path,
        )
        .expect("blocker-free manifest should assemble packet");

    let envelope_text = std::fs::read_to_string(&operator_evidence.approval_envelope_path)
        .expect("approval envelope should read");
    let envelope: serde_json::Value =
        serde_json::from_str(&envelope_text).expect("approval envelope should parse");
    assert_eq!(envelope["record_kind"], "phase8_operator_approval_envelope");
    assert_eq!(envelope["head_sha"], operator_evidence.head_sha);
    assert_eq!(
        envelope["ssm_manifest_sha256"],
        operator_evidence.ssm_manifest_sha256
    );
    assert_eq!(
        envelope["strategy_input_evidence_sha256"],
        operator_evidence.strategy_input_evidence_sha256
    );
    assert_eq!(
        envelope["financial_envelope_sha256"],
        operator_evidence.financial_envelope_sha256
    );
    assert_eq!(
        envelope["pre_run_state_sha256"],
        operator_evidence.pre_run_state_sha256
    );
    assert_eq!(
        envelope["abort_plan_sha256"],
        operator_evidence.abort_plan_sha256
    );
    assert_eq!(
        envelope["approval_nonce_sha256"],
        operator_evidence.approval_nonce_sha256
    );
    assert_eq!(
        envelope["approval_id_hash"],
        sha256_text(
            &loaded
                .root
                .live_canary
                .as_ref()
                .expect("live canary should exist")
                .approval_id
        )
    );
    assert_eq!(
        envelope["canary_evidence_path_hash"],
        sha256_text(&operator_evidence.canary_evidence_path)
    );
    assert_eq!(
        envelope["strategy_cancel_path_hash"],
        sha256_text(
            operator_evidence
                .strategy_cancel_path
                .as_deref()
                .expect("strategy cancel path should exist")
        )
    );

    for forbidden in [
        "approval_envelope_sha256",
        "root_toml_sha256",
        "config_bundle_checksum",
        "operator_approval_id",
        "raw_nonce",
        "nonce_material",
    ] {
        assert!(
            envelope.get(forbidden).is_none(),
            "approval envelope must not contain circular or raw field {forbidden}"
        );
        assert!(
            !envelope_text.contains(forbidden),
            "approval envelope text must not contain forbidden field {forbidden}"
        );
    }
    assert!(
        envelope.get("approval_id").is_none(),
        "approval envelope must not contain raw approval_id field"
    );
    assert!(
        !envelope_text.contains(
            &loaded
                .root
                .live_canary
                .as_ref()
                .expect("live canary should exist")
                .approval_id
        ),
        "approval envelope must hash, not print, operator approval id"
    );
    assert!(!envelope_text.contains("/bolt/"));

    let operator_packet_text =
        std::fs::read_to_string(&operator_packet_path).expect("operator packet should read");
    let operator_packet: serde_json::Value =
        serde_json::from_str(&operator_packet_text).expect("operator packet should parse");
    assert_eq!(
        operator_packet["record_kind"],
        "bolt_v3.operator_evidence_packet.v1"
    );
    assert_eq!(
        operator_packet["config_bundle_checksum"],
        loaded.config_bundle_checksum
    );
    assert_eq!(
        operator_packet["static_manifest_sha256"],
        outcome.static_manifest.sha256
    );
    assert_eq!(
        operator_packet["live_canary_operator_evidence"]["approval_envelope_sha256"],
        outcome.approval_envelope.sha256
    );
    assert_eq!(
        operator_packet["live_canary_operator_evidence"]["ssm_manifest_sha256"],
        operator_evidence.ssm_manifest_sha256
    );
    for forbidden in [
        "max_operator_evidence_file_bytes",
        "approval_consumption_max_age_seconds",
        "approval_not_before_unix_seconds",
        "approval_not_after_unix_seconds",
    ] {
        assert!(
            operator_packet["live_canary_operator_evidence"]
                .get(forbidden)
                .is_none(),
            "operator packet must not carry runtime policy/window field {forbidden}"
        );
        assert!(
            !operator_packet_text.contains(forbidden),
            "operator packet text must not carry runtime policy/window field {forbidden}"
        );
    }
    assert!(
        !operator_packet_text.contains("/bolt/"),
        "operator packet must not print raw SSM parameter paths"
    );
    assert!(
        !operator_packet_text.contains("secret_sentinel"),
        "operator packet must not copy artifact contents"
    );
    assert!(
        !operator_packet_text.contains(
            &loaded
                .root
                .live_canary
                .as_ref()
                .expect("live canary should exist")
                .approval_id
        ),
        "operator packet must not print raw approval id"
    );
}

#[test]
fn approval_packet_assembly_binds_relative_static_manifest_to_config_root() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    loaded.root_path = temp.path().join("root.toml");
    std::fs::write(&loaded.root_path, "fixture root").expect("root fixture should write");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    operator_evidence.head_sha = option_env!("BOLT_V3_BUILD_HEAD_SHA")
        .unwrap_or_else(|| {
            panic!("build head sha should be compiled for relative manifest verifier test")
        })
        .to_string();
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence.clone());
    bind_expected_approval_envelope_hash(&mut loaded, &mut operator_evidence);
    write_final_live_evidence_artifacts_for_test(&loaded, &operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let relative_manifest_path = std::path::Path::new("static-artifacts-manifest.json");
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let outcome =
        bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
            &loaded,
            relative_manifest_path,
            &operator_packet_path,
        )
        .expect("relative static manifest should resolve from config root");
    let packet = read_json_value(&operator_packet_path);

    assert_eq!(outcome.static_manifest.path, manifest_path);
    assert_eq!(
        packet["static_manifest_path"],
        manifest_path.to_string_lossy().to_string()
    );
    bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &loaded,
        &operator_packet_path,
    )
    .expect("verifier should resolve the same manifest path stored by assembly");
}

#[test]
fn approval_packet_assembly_redacts_invalid_hash_values_from_errors() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let secret_like_value = "/bolt/not-a-real-secret-path";
    loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist")
        .approval_nonce_sha256 = secret_like_value.to_string();
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("invalid hash shape should fail closed");
    let message = error.to_string();

    assert!(
        message.contains("approval_nonce_sha256"),
        "hash shape error should name field: {message}"
    );
    assert!(
        !message.contains(secret_like_value),
        "hash shape error must not echo invalid value: {message}"
    );
    assert!(
        !operator_packet_path.exists(),
        "invalid hash shape must not leave operator packet"
    );
}

#[test]
fn approval_packet_assembly_redacts_invalid_static_manifest_hash_values_from_errors() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let mut refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let secret_like_value = "/bolt/not-a-real-secret-path";
    refs.iter_mut()
        .find(|artifact| artifact["name"] == "approval-nonce")
        .expect("approval nonce ref should exist")["sha256"] = serde_json::json!(secret_like_value);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("invalid static manifest hash shape should fail closed");
    let message = error.to_string();

    assert!(
        message.contains("static manifest"),
        "static manifest hash error should name manifest scope: {message}"
    );
    assert!(
        !message.contains("[live_canary.operator_evidence]"),
        "static manifest hash error must not use operator-evidence prefix: {message}"
    );
    assert!(
        !message.contains(secret_like_value),
        "static manifest hash error must not echo invalid value: {message}"
    );
    assert!(
        !operator_packet_path.exists(),
        "invalid static manifest hash must not leave operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_oversized_static_manifest_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist")
        .max_operator_evidence_file_bytes = 8;
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    std::fs::write(&manifest_path, vec![b'{'; 9]).expect("oversized manifest should write");
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("oversized static manifest should fail closed");

    assert!(
        error
            .to_string()
            .contains("max_operator_evidence_file_bytes"),
        "oversized manifest error should cite configured cap: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "oversized manifest must not leave approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "oversized manifest must not leave operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_oversized_static_artifact_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let mut refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs.clone(),
        Vec::new(),
    );
    let manifest_len = std::fs::metadata(&manifest_path)
        .expect("manifest metadata should read")
        .len();
    let max_bytes = manifest_len + 1;
    let oversized_bytes = vec![b'x'; max_bytes as usize + 1];
    std::fs::write(&operator_evidence.financial_envelope_path, &oversized_bytes)
        .expect("oversized artifact should write");
    let oversized_sha = sha256_bytes(&oversized_bytes);
    operator_evidence.financial_envelope_sha256 = oversized_sha.clone();
    refs.iter_mut()
        .find(|artifact| artifact["name"] == "financial-envelope")
        .expect("financial envelope ref should exist")["sha256"] = serde_json::json!(oversized_sha);
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    operator_evidence.max_operator_evidence_file_bytes = max_bytes;
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("oversized static artifact should fail closed");

    assert!(
        error
            .to_string()
            .contains("max_operator_evidence_file_bytes"),
        "oversized artifact error should cite configured cap: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "oversized artifact must not leave approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "oversized artifact must not leave operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_invalid_output_paths_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    operator_evidence.approval_envelope_path = "../approval-envelope.json".to_string();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("parent-dir output path should fail closed");

    assert!(
        error.to_string().contains("approval_envelope_path"),
        "path-shape error should name approval envelope path: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "invalid output path must not leave original approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "invalid output path must not leave operator packet"
    );
}

#[test]
fn approval_packet_assembly_rejects_output_path_collision_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = std::path::PathBuf::from(&approval_envelope_path);

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("colliding output paths should fail closed");

    assert!(
        error.to_string().contains("output path"),
        "path-collision error should name output path issue: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "colliding outputs must not leave approval envelope"
    );
}

#[test]
fn approval_packet_assembly_rejects_equivalent_output_path_collision_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let current_dir = std::env::current_dir().expect("current dir should resolve");
    let temp = tempfile::Builder::new()
        .prefix("t128-output-collision-")
        .tempdir_in(&current_dir)
        .expect("repo-local tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let approval_envelope_path =
        std::path::PathBuf::from(&operator_evidence.approval_envelope_path);
    loaded.root_path = current_dir.join("root.toml");
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp
        .path()
        .strip_prefix(&current_dir)
        .expect("temp path should be cwd-relative")
        .join("approval-envelope.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("equivalent output paths should fail closed before writes");

    assert!(
        error.to_string().contains("output path"),
        "equivalent path-collision error should name output path issue: {error}"
    );
    assert!(
        !approval_envelope_path.exists(),
        "equivalent colliding outputs must not leave approval envelope"
    );
}

#[test]
#[cfg(unix)]
fn approval_packet_assembly_rejects_symlinked_output_parent_collision_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let real_dir = temp.path().join("real-output");
    let alias_dir = temp.path().join("alias-output");
    std::fs::create_dir_all(&real_dir).expect("real output dir should create");
    std::os::unix::fs::symlink(&real_dir, &alias_dir).expect("output symlink should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    operator_evidence.approval_envelope_path = real_dir
        .join("approval-envelope.json")
        .to_string_lossy()
        .to_string();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence.clone());
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = alias_dir.join("approval-envelope.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("symlinked parent collision should fail closed before writes");

    assert!(
        error.to_string().contains("output path"),
        "symlinked parent collision should name output path issue: {error}"
    );
    assert!(
        !std::path::Path::new(&operator_evidence.approval_envelope_path).exists(),
        "symlinked parent collision must not leave approval envelope"
    );
}

#[test]
fn approval_packet_assembly_rejects_invalid_output_parent_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let invalid_parent = temp.path().join("not-a-directory");
    std::fs::write(&invalid_parent, b"not a directory").expect("invalid parent file should write");
    let operator_packet_path = temp
        .path()
        .join("not-a-directory")
        .join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("invalid output parent should fail before writes");

    assert!(
        error.to_string().contains("output path"),
        "invalid output parent should name output path issue: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "invalid output parent must not leave approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "invalid output parent must not leave operator packet"
    );
}

#[test]
#[cfg(unix)]
fn approval_packet_assembly_rejects_symlinked_static_artifact_before_writes() {
    let mut loaded = load_fixture_with_live_canary();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    let mut refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let real_financial_path = std::path::PathBuf::from(&operator_evidence.financial_envelope_path);
    let symlink_path = temp.path().join("financial-envelope-link.json");
    std::fs::remove_file(&real_financial_path).expect("real financial artifact should remove");
    std::fs::write(
        &real_financial_path,
        br#"{"record_kind":"financial-envelope"}"#,
    )
    .expect("real financial artifact should rewrite");
    std::os::unix::fs::symlink(&real_financial_path, &symlink_path)
        .expect("financial artifact symlink should create");
    let bytes = std::fs::read(&real_financial_path).expect("real financial artifact should read");
    operator_evidence.financial_envelope_path = symlink_path.to_string_lossy().to_string();
    operator_evidence.financial_envelope_sha256 = sha256_bytes(&bytes);
    refs.iter_mut()
        .find(|artifact| artifact["name"] == "financial-envelope")
        .expect("financial envelope ref should exist")["path"] =
        serde_json::json!(operator_evidence.financial_envelope_path);
    refs.iter_mut()
        .find(|artifact| artifact["name"] == "financial-envelope")
        .expect("financial envelope ref should exist")["sha256"] =
        serde_json::json!(operator_evidence.financial_envelope_sha256);
    let approval_envelope_path = operator_evidence.approval_envelope_path.clone();
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("fixture should have live canary")
        .operator_evidence = Some(operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");

    let error = bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
        &loaded,
        &manifest_path,
        &operator_packet_path,
    )
    .expect_err("symlinked static artifact should fail closed");

    assert!(
        error.to_string().contains("regular file"),
        "symlinked artifact error should cite regular-file policy: {error}"
    );
    assert!(
        !std::path::Path::new(&approval_envelope_path).exists(),
        "symlinked artifact must not leave approval envelope"
    );
    assert!(
        !operator_packet_path.exists(),
        "symlinked artifact must not leave operator packet"
    );
}

#[test]
fn final_packet_verifier_accepts_t128_packet_bound_to_current_config() {
    let fixture = assembled_final_packet_fixture();

    let outcome = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect("final packet verifier should accept current config-bound packet");

    assert_eq!(outcome.operator_packet.path, fixture.operator_packet_path);
    assert_eq!(outcome.static_manifest.path, fixture.static_manifest_path);
    assert_eq!(
        outcome.approval_envelope.path,
        std::path::PathBuf::from(
            &fixture
                .loaded
                .root
                .live_canary
                .as_ref()
                .and_then(|live_canary| live_canary.operator_evidence.as_ref())
                .expect("operator evidence should remain configured")
                .approval_envelope_path
        )
    );
}

#[test]
fn final_packet_verifier_redacted_summary_omits_artifact_paths() {
    let fixture = assembled_final_packet_fixture();
    let outcome = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect("final packet verifier should accept current config-bound packet");

    let summary_json =
        serde_json::to_value(outcome.redacted_summary()).expect("summary should serialize");
    let summary_text = serde_json::to_string(&summary_json).expect("summary should stringify");

    assert!(
        !summary_text.contains(&fixture.temp.path().to_string_lossy().to_string()),
        "redacted summary must not print artifact paths: {summary_text}"
    );
    let artifacts = summary_json["verified_artifacts"]
        .as_array()
        .expect("summary should expose verified artifact list");
    assert_eq!(artifacts.len(), 3);
    for artifact in artifacts {
        let artifact = artifact.as_object().expect("artifact summary is object");
        assert_eq!(
            artifact.keys().map(String::as_str).collect::<Vec<_>>(),
            ["name", "sha256"]
        );
        assert_eq!(
            artifact["sha256"]
                .as_str()
                .expect("summary sha should be string")
                .len(),
            64
        );
    }
}

#[test]
fn final_packet_verifier_debug_omits_artifact_paths() {
    let fixture = assembled_final_packet_fixture();
    let outcome = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect("final packet verifier should accept current config-bound packet");

    let debug = format!("{outcome:?}");

    for forbidden_path in [
        fixture.operator_packet_path.to_string_lossy().to_string(),
        fixture.static_manifest_path.to_string_lossy().to_string(),
        fixture.operator_evidence().approval_envelope_path.clone(),
    ] {
        assert!(
            !debug.contains(&forbidden_path),
            "final packet verification Debug output must not expose artifact path {forbidden_path}: {debug}"
        );
    }
    assert!(
        debug.contains("verified_artifacts"),
        "debug output should still report redacted verification content: {debug}"
    );

    let nested_debug = format!("{:?}", outcome.operator_packet);
    assert!(
        !nested_debug.contains(&fixture.operator_packet_path.to_string_lossy().to_string()),
        "nested operator artifact Debug output must not expose artifact path: {nested_debug}"
    );
}

#[test]
fn final_packet_verifier_rejects_missing_operator_evidence() {
    let mut fixture = assembled_final_packet_fixture();
    fixture
        .loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .operator_evidence = None;

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("missing operator evidence should fail closed");

    assert!(
        error
            .to_string()
            .contains("[live_canary.operator_evidence]"),
        "missing operator evidence error should cite config block: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_packet_config_bundle_drift() {
    let mut fixture = assembled_final_packet_fixture();
    fixture.loaded.config_bundle_checksum = sha256_text("different-config-bundle");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("packet config-bundle drift should fail closed");

    assert!(
        error.to_string().contains("config_bundle_checksum"),
        "config drift error should name checksum field: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_static_manifest_sha_mismatch() {
    let fixture = assembled_final_packet_fixture();
    let mut manifest = read_json_value(&fixture.static_manifest_path);
    manifest["generated_artifacts"]
        .as_array_mut()
        .expect("generated artifacts should be array")
        .push(serde_json::json!({
            "name": "duplicate-extra",
            "path": fixture.temp.path().join("extra.json").to_string_lossy(),
            "sha256": "0".repeat(64),
        }));
    write_json_value_and_hash(&fixture.static_manifest_path, &manifest);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("static manifest SHA drift should fail closed");

    assert!(
        error.to_string().contains("static_manifest_sha256"),
        "static manifest SHA drift should name linkage hash: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_static_manifest_config_bundle_drift() {
    let fixture = assembled_final_packet_fixture();
    let mut manifest = read_json_value(&fixture.static_manifest_path);
    manifest["config_bundle_checksum"] = serde_json::json!(sha256_text("different-static-config"));
    let manifest_sha = write_json_value_and_hash(&fixture.static_manifest_path, &manifest);
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["static_manifest_sha256"] = serde_json::json!(manifest_sha);
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("static manifest config-bundle drift should fail closed");

    assert!(
        error.to_string().contains("config_bundle_checksum"),
        "static manifest config drift should name checksum: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_static_manifest_blockers_without_echoing_text() {
    let fixture = assembled_final_packet_fixture();
    let secret_like_blocker = "/bolt/not-a-real-secret-path";
    let mut manifest = read_json_value(&fixture.static_manifest_path);
    manifest["blockers"] = serde_json::json!([secret_like_blocker]);
    let manifest_sha = write_json_value_and_hash(&fixture.static_manifest_path, &manifest);
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["static_manifest_sha256"] = serde_json::json!(manifest_sha);
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("static manifest blockers should fail closed");
    let message = error.to_string();

    assert!(
        message.contains("static manifest blockers"),
        "blocker error should name static manifest blockers: {message}"
    );
    assert!(
        !message.contains(secret_like_blocker),
        "blocker error must not echo supplied blocker text: {message}"
    );
}

#[test]
fn final_packet_verifier_rejects_missing_operator_packet_file() {
    let fixture = assembled_final_packet_fixture();
    std::fs::remove_file(&fixture.operator_packet_path).expect("operator packet should remove");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("missing operator packet should fail closed");

    assert!(
        error.to_string().contains("operator packet"),
        "missing packet error should name operator packet: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains(&fixture.operator_packet_path.to_string_lossy().to_string()),
        "missing packet error must not echo raw artifact path: {error}"
    );
    assert!(
        !format!("{error:?}").contains(&fixture.operator_packet_path.to_string_lossy().to_string()),
        "missing packet Debug output must not echo raw artifact path: {error:?}"
    );
}

#[test]
fn final_packet_verifier_rejects_missing_static_manifest_file() {
    let fixture = assembled_final_packet_fixture();
    std::fs::remove_file(&fixture.static_manifest_path).expect("static manifest should remove");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("missing static manifest should fail closed");

    assert!(
        error.to_string().contains("static manifest"),
        "missing manifest error should name static manifest: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains(&fixture.static_manifest_path.to_string_lossy().to_string()),
        "missing manifest error must not echo raw artifact path: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_missing_approval_envelope_file() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    std::fs::remove_file(&evidence.approval_envelope_path)
        .expect("approval envelope should remove");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("missing approval envelope should fail closed");

    assert!(
        error.to_string().contains("approval envelope"),
        "missing approval envelope error should name approval envelope: {error}"
    );
    assert!(
        !error.to_string().contains(&evidence.approval_envelope_path),
        "missing approval envelope error must not echo raw artifact path: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_missing_live_canary_evidence_file() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    let canary_evidence_path = std::path::PathBuf::from(&evidence.canary_evidence_path);
    let _ = std::fs::remove_file(&canary_evidence_path);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("final packet verifier must read and verify live canary evidence");

    assert!(
        error.to_string().contains("canary_evidence_path"),
        "missing live-canary evidence error should name the configured final evidence field: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains(&canary_evidence_path.to_string_lossy().to_string()),
        "missing live-canary evidence error must not echo raw artifact path: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_canary_static_evidence_ref_drift() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    let canary_evidence_path = std::path::PathBuf::from(&evidence.canary_evidence_path);
    let mut canary = read_json_value(&canary_evidence_path);
    canary["strategy_input_evidence_ref"]["record_hash"] = serde_json::json!("0".repeat(64));
    write_json_value_and_hash(&canary_evidence_path, &canary);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("canary static evidence ref drift should fail closed");

    assert!(
        error.to_string().contains("strategy_input_evidence_ref"),
        "static canary evidence ref drift should name the drifted ref: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_approval_consumption_root_toml_drift() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    let approval_consumption_path = std::path::PathBuf::from(&evidence.approval_consumption_path);
    let mut approval_consumption = read_json_value(&approval_consumption_path);
    approval_consumption["root_toml_sha256"] =
        serde_json::json!(sha256_text("different-root-toml"));
    write_json_value_and_hash(&approval_consumption_path, &approval_consumption);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("approval consumption root TOML drift should fail closed");

    assert!(
        error
            .to_string()
            .contains("approval_consumption_path.root_toml_sha256"),
        "root TOML drift error should name root_toml_sha256: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_stale_approval_envelope_file_hash() {
    let fixture = assembled_final_packet_fixture();
    let approval_envelope_path =
        std::path::PathBuf::from(&fixture.operator_evidence().approval_envelope_path);
    let mut envelope = read_json_value(&approval_envelope_path);
    envelope["approval_nonce_sha256"] = serde_json::json!("1".repeat(64));
    write_json_value_and_hash(&approval_envelope_path, &envelope);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("stale approval envelope hash should fail closed");

    assert!(
        error.to_string().contains("approval envelope file hash"),
        "stale approval envelope hash should name file-hash mismatch: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_oversized_operator_packet_before_parsing() {
    let mut fixture = assembled_final_packet_fixture();
    let packet_len = std::fs::metadata(&fixture.operator_packet_path)
        .expect("operator packet metadata should read")
        .len();
    fixture
        .loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist")
        .max_operator_evidence_file_bytes = packet_len.saturating_sub(1);

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("oversized operator packet should fail closed");

    assert!(
        error
            .to_string()
            .contains("max_operator_evidence_file_bytes"),
        "oversized packet should cite configured cap: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_deleted_required_artifact() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    std::fs::remove_file(&evidence.strategy_input_evidence_path)
        .expect("strategy input artifact should remove");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("deleted required artifact should fail closed");

    assert!(
        error.to_string().contains("strategy-input"),
        "deleted artifact error should name artifact: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_mutated_required_artifact_hash() {
    let fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence();
    std::fs::write(
        &evidence.financial_envelope_path,
        b"different-financial-envelope-bytes",
    )
    .expect("financial envelope artifact should mutate");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("mutated required artifact should fail closed");

    assert!(
        error.to_string().contains("financial-envelope"),
        "mutated artifact error should name artifact: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_packet_operator_evidence_mismatch() {
    let fixture = assembled_final_packet_fixture();
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["live_canary_operator_evidence"]["ssm_manifest_path"] =
            serde_json::json!("/bolt/not-a-real-secret-path");
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("packet and TOML operator evidence mismatch should fail closed");
    let message = error.to_string();

    assert!(
        message.contains("ssm_manifest_path"),
        "packet mismatch should name field: {message}"
    );
    assert!(
        !message.contains("/bolt/not-a-real-secret-path"),
        "packet mismatch must not echo supplied unsafe path: {message}"
    );
}

#[test]
fn final_packet_verifier_rejects_approval_envelope_body_mismatch() {
    let mut fixture = assembled_final_packet_fixture();
    let evidence = fixture.operator_evidence().clone();
    let approval_envelope_path = std::path::PathBuf::from(&evidence.approval_envelope_path);
    let mut envelope = read_json_value(&approval_envelope_path);
    envelope["head_sha"] = serde_json::json!("abcdefabcdefabcdefabcdefabcdefabcdefabcd");
    let envelope_sha = write_json_value_and_hash(&approval_envelope_path, &envelope);
    fixture
        .loaded
        .root
        .live_canary
        .as_mut()
        .and_then(|live_canary| live_canary.operator_evidence.as_mut())
        .expect("operator evidence should exist")
        .approval_envelope_sha256 = envelope_sha.clone();
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["live_canary_operator_evidence"]["approval_envelope_sha256"] =
            serde_json::json!(envelope_sha);
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("approval envelope body mismatch should fail closed");

    assert!(
        error.to_string().contains("head_sha"),
        "approval envelope body mismatch should name field: {error}"
    );
}

#[test]
fn final_packet_verifier_rejects_unknown_packet_runtime_policy_fields() {
    let fixture = assembled_final_packet_fixture();
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["live_canary_operator_evidence"]["max_operator_evidence_file_bytes"] =
            serde_json::json!(4096);
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("unknown runtime policy field in packet should fail closed");

    assert!(
        error
            .to_string()
            .contains("max_operator_evidence_file_bytes"),
        "unknown field parse error should name unexpected field: {error}"
    );
}

#[test]
#[cfg(unix)]
fn final_packet_verifier_rejects_symlinked_operator_packet_before_parsing() {
    let fixture = assembled_final_packet_fixture();
    let symlink_path = fixture.temp.path().join("operator-packet-link.json");
    std::os::unix::fs::symlink(&fixture.operator_packet_path, &symlink_path)
        .expect("operator packet symlink should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &symlink_path,
    )
    .expect_err("symlinked operator packet should fail closed");

    assert!(
        error.to_string().contains("regular file"),
        "symlinked packet should cite regular-file policy: {error}"
    );
}

#[test]
#[cfg(unix)]
fn final_packet_verifier_rejects_symlinked_static_manifest_before_parsing() {
    let fixture = assembled_final_packet_fixture();
    let symlink_path = fixture
        .temp
        .path()
        .join("static-artifacts-manifest-link.json");
    std::os::unix::fs::symlink(&fixture.static_manifest_path, &symlink_path)
        .expect("static manifest symlink should create");
    mutate_packet_json(&fixture.operator_packet_path, |packet| {
        packet["static_manifest_path"] = serde_json::json!(symlink_path.to_string_lossy());
    });

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("symlinked static manifest should fail closed");

    assert!(
        error.to_string().contains("regular file"),
        "symlinked static manifest should cite regular-file policy: {error}"
    );
}

#[test]
#[cfg(unix)]
fn final_packet_verifier_rejects_symlinked_approval_envelope_before_parsing() {
    let fixture = assembled_final_packet_fixture();
    let approval_envelope_path =
        std::path::PathBuf::from(&fixture.operator_evidence().approval_envelope_path);
    let real_path = fixture.temp.path().join("approval-envelope-real.json");
    std::fs::rename(&approval_envelope_path, &real_path)
        .expect("approval envelope should move behind symlink");
    std::os::unix::fs::symlink(&real_path, &approval_envelope_path)
        .expect("approval envelope symlink should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("symlinked approval envelope should fail closed");

    assert!(
        error.to_string().contains("regular file"),
        "symlinked approval envelope should cite regular-file policy: {error}"
    );
}

#[test]
#[cfg(unix)]
fn final_packet_verifier_rejects_symlinked_required_artifact_before_hashing() {
    let fixture = assembled_final_packet_fixture();
    let strategy_input_path =
        std::path::PathBuf::from(&fixture.operator_evidence().strategy_input_evidence_path);
    let real_path = fixture.temp.path().join("strategy-input-real.json");
    std::fs::rename(&strategy_input_path, &real_path)
        .expect("strategy input artifact should move behind symlink");
    std::os::unix::fs::symlink(&real_path, &strategy_input_path)
        .expect("strategy input symlink should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::verify_final_operator_packet(
        &fixture.loaded,
        &fixture.operator_packet_path,
    )
    .expect_err("symlinked required artifact should fail closed");

    assert!(
        error.to_string().contains("regular file"),
        "symlinked required artifact should cite regular-file policy: {error}"
    );
}

#[test]
fn market_selection_source_builder_binds_configured_target_to_nt_instruments() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let now_ms = TEST_MARKET_SELECTION_NOW_MS;
    let market_start_ms = TEST_MARKET_SELECTION_START_MS;
    let market_end_ms = TEST_MARKET_SELECTION_END_MS;
    let market_slug = updown_market_slug(
        TEST_MARKET_SELECTION_UNDERLYING_ASSET,
        TEST_MARKET_SELECTION_CADENCE_SLUG,
        TEST_MARKET_SELECTION_CURRENT_START_SECONDS,
    );
    let up_instrument_id = TEST_UP_INSTRUMENT_ID;
    let down_instrument_id = TEST_DOWN_INSTRUMENT_ID;
    let instruments = vec![
        updown_binary_option(
            up_instrument_id,
            &market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_UP_OUTCOME,
            market_start_ms,
            market_end_ms,
        ),
        updown_binary_option(
            down_instrument_id,
            &market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_DOWN_OUTCOME,
            market_start_ms,
            market_end_ms,
        ),
    ];
    let artifact = bolt_v2::bolt_v3_operator_artifacts::build_market_selection_source_artifact(
        &loaded,
        strategy_instance_id,
        &instruments,
        now_ms,
    )
    .expect("market selection source should build from config and NT instruments");
    let json = serde_json::to_value(&artifact).expect("market selection source should serialize");
    assert_eq!(json["record_kind"], "market_selection_result");
    assert_eq!(json["source"], "nt_runtime_selection_snapshot");
    assert_eq!(json["market_selection_timestamp_ms"], now_ms);
    assert_eq!(json["market_selection_outcome"], "current");
    assert_eq!(json["polymarket_condition_id"], TEST_CONDITION_ID);
    assert_eq!(json["polymarket_market_slug"], market_slug);
    assert_eq!(json["polymarket_question_id"], TEST_QUESTION_ID);
    assert_eq!(json["up_instrument_id"], up_instrument_id);
    assert_eq!(json["down_instrument_id"], down_instrument_id);
    assert_eq!(json["selected_market_observed_timestamp_ms"], now_ms);
    assert_eq!(
        json["polymarket_market_start_timestamp_ms"],
        market_start_ms
    );
    assert_eq!(json["polymarket_market_end_timestamp_ms"], market_end_ms);
}

#[test]
fn market_selection_source_builder_uses_nt_expiration_not_rounded_seconds_to_end() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let now_ms = TEST_MARKET_SELECTION_NOW_MS + 999;
    let market_slug = updown_market_slug(
        TEST_MARKET_SELECTION_UNDERLYING_ASSET,
        TEST_MARKET_SELECTION_CADENCE_SLUG,
        TEST_MARKET_SELECTION_CURRENT_START_SECONDS,
    );
    let instruments = vec![
        updown_binary_option(
            TEST_UP_INSTRUMENT_ID,
            &market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_UP_OUTCOME,
            TEST_MARKET_SELECTION_START_MS,
            TEST_MARKET_SELECTION_END_MS,
        ),
        updown_binary_option(
            TEST_DOWN_INSTRUMENT_ID,
            &market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_DOWN_OUTCOME,
            TEST_MARKET_SELECTION_START_MS,
            TEST_MARKET_SELECTION_END_MS,
        ),
    ];

    let artifact = bolt_v2::bolt_v3_operator_artifacts::build_market_selection_source_artifact(
        &loaded,
        strategy_instance_id,
        &instruments,
        now_ms,
    )
    .expect("market selection source should build from config and NT instruments");
    let json = serde_json::to_value(&artifact).expect("market selection source should serialize");

    assert_eq!(
        json["polymarket_market_end_timestamp_ms"], TEST_MARKET_SELECTION_END_MS,
        "market-selection source must bind the selected NT instrument expiration"
    );
}

#[test]
fn market_selection_source_writer_fails_closed_until_strategy_decision_inputs_exist() {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let source_path = temp.path().join(TEST_MARKET_SELECTION_SOURCE_FILE);

    let error = bolt_v2::bolt_v3_operator_artifacts::write_market_selection_source_artifact(
        &loaded,
        strategy_instance_id,
        &[],
        TEST_MARKET_SELECTION_NOW_MS,
        &source_path,
    )
    .expect_err("market selection source writer should fail closed until decision inputs exist");

    assert!(
        error.to_string().contains("price-to-beat"),
        "market selection blocker should cite price-to-beat gap: {error}"
    );
    assert!(
        error.to_string().contains("strategy decision input"),
        "market selection blocker should cite strategy decision gap: {error}"
    );
    assert!(
        !source_path.exists(),
        "failed market selection generation must not leave a success artifact"
    );
}

#[test]
fn market_selection_source_writer_promotes_source_bound_decision_evidence() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture
        .temp
        .path()
        .join("decision-bound-market-selection.json");
    let decision_evidence_path =
        write_entry_decision_evidence_chain(&fixture.temp, &fixture.snapshot);
    let market_slug = fixture
        .snapshot
        .polymarket_market_slug
        .as_deref()
        .expect("fixture snapshot should bind market slug");
    let instruments = market_selection_instruments_for_slug(market_slug);

    let written = bolt_v2::bolt_v3_operator_artifacts::write_market_selection_source_artifact_from_decision_evidence_file(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &decision_evidence_path,
        100_000,
        &instruments,
        &output_path,
    )
    .expect("source-bound decision evidence should write market-selection source");

    let artifact_bytes =
        std::fs::read(&output_path).expect("market-selection source artifact should read");
    assert_eq!(written.sha256, hex::encode(Sha256::digest(&artifact_bytes)));
    let json: serde_json::Value =
        serde_json::from_slice(&artifact_bytes).expect("market-selection source should parse");
    assert_eq!(
        json["market_selection_timestamp_ms"],
        TEST_MARKET_SELECTION_NOW_MS
    );
    assert_eq!(json["polymarket_condition_id"], TEST_CONDITION_ID);
    assert_eq!(json["polymarket_question_id"], TEST_QUESTION_ID);
    assert_eq!(json["up_instrument_id"], TEST_UP_INSTRUMENT_ID);
    assert_eq!(json["down_instrument_id"], TEST_DOWN_INSTRUMENT_ID);
}

#[test]
fn market_selection_source_writer_promotes_decision_evidence_from_instrument_source_file() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture
        .temp
        .path()
        .join("decision-bound-market-selection-from-instrument-file.json");
    let decision_evidence_path =
        write_entry_decision_evidence_chain(&fixture.temp, &fixture.snapshot);
    let market_slug = fixture
        .snapshot
        .polymarket_market_slug
        .as_deref()
        .expect("fixture snapshot should bind market slug");
    let instruments = market_selection_instruments_for_slug(market_slug);
    let instrument_source_path = fixture.temp.path().join("market-instruments.json");
    std::fs::write(
        &instrument_source_path,
        serde_json::to_vec_pretty(&instruments).expect("instrument source should serialize"),
    )
    .expect("instrument source should write");

    let written = bolt_v2::bolt_v3_operator_artifacts::write_market_selection_source_artifact_from_decision_evidence_and_instrument_source_file(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &decision_evidence_path,
        100_000,
        &instrument_source_path,
        100_000,
        &output_path,
    )
    .expect("source-bound decision evidence plus instrument source should write market-selection source");

    let json = read_json_value(&output_path);
    assert_eq!(written.sha256, sha256_file(&output_path));
    assert_eq!(
        json["market_selection_timestamp_ms"],
        TEST_MARKET_SELECTION_NOW_MS
    );
    assert_eq!(json["polymarket_condition_id"], TEST_CONDITION_ID);
}

#[test]
fn market_selection_source_writer_rejects_unusable_price_to_beat_values() {
    for (case_index, price_to_beat_value) in ["not-a-price", "0"].into_iter().enumerate() {
        let fixture = strategy_input_runtime_fixture();
        let output_path = fixture
            .temp
            .path()
            .join(format!("decision-bound-market-selection-{case_index}.json"));
        let mut snapshot = fixture.snapshot.clone();
        snapshot.price_to_beat_value = price_to_beat_value.to_string();
        let decision_evidence_path = write_entry_decision_evidence_chain(&fixture.temp, &snapshot);
        let market_slug = snapshot
            .polymarket_market_slug
            .as_deref()
            .expect("fixture snapshot should bind market slug");
        let instruments = market_selection_instruments_for_slug(market_slug);

        let error = bolt_v2::bolt_v3_operator_artifacts::write_market_selection_source_artifact_from_decision_evidence_file(
            &fixture.loaded,
            &fixture.strategy_instance_id,
            &decision_evidence_path,
            100_000,
            &instruments,
            &output_path,
        )
        .expect_err("unusable price-to-beat values must fail closed before market-selection source write");

        assert!(
            error.to_string().contains("price-to-beat value"),
            "price-to-beat rejection should stay diagnostic and redacted: {error}"
        );
        assert!(
            !output_path.exists(),
            "failed price-to-beat validation must not leave a market-selection artifact"
        );
    }
}

#[test]
fn market_selection_source_writer_uses_family_dispatch_not_updown_directly() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");

    assert!(
        !source.contains("updown::"),
        "operator artifacts must use market-family dispatch instead of direct updown calls"
    );
}

#[test]
fn strategy_input_writer_emits_phase8_artifact_from_runtime_snapshot_and_market_source() {
    let fixture = strategy_input_runtime_fixture();
    let loaded = &fixture.loaded;
    let strategy_instance_id = fixture.strategy_instance_id.as_str();
    let market_selection_source_ref = &fixture.market_selection_source_ref;
    let temp = &fixture.temp;
    let strategy_input_path = temp.path().join("strategy-input.json");
    let snapshot = fixture.snapshot.clone();

    let intent = BoltV3OrderIntentEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        intent_kind: BoltV3OrderIntentKind::Entry,
        instrument_id: snapshot.submission_instrument_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        order_side: snapshot.submission_order_side.clone(),
        price: snapshot.submission_price.clone(),
        quantity: snapshot.submission_quantity.clone(),
        order_fields: BoltV3OrderIntentOrderFields {
            order_type: "Limit".to_string(),
            time_in_force: "Gtc".to_string(),
            price: Some(snapshot.submission_price.clone()),
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            expire_time_unix_nanos: None,
            is_post_only: false,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    };
    let admission = BoltV3AdmissionDecisionEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        instrument_id: snapshot.submission_instrument_id.clone(),
        notional: "0.50".to_string(),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::RejectedNotArmed,
        loss_halt_reasons: Vec::new(),
    };
    let decision_evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut decision_evidence = String::new();
    for line in [
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": "bolt_v3.strategy_input_snapshot",
            "gate_version": "0.1.0",
            "kind": "strategy_input_snapshot",
            "snapshot": snapshot.clone(),
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 2_i64,
            "gate_id": "bolt_v3.order_intent",
            "gate_version": "0.1.0",
            "kind": "order_intent",
            "intent": intent.clone(),
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 3_i64,
            "gate_id": "bolt_v3.submit_admission",
            "gate_version": "0.1.0",
            "kind": "admission_decision",
            "decision": admission.clone(),
        }),
    ] {
        decision_evidence.push_str(
            &serde_json::to_string(&line).expect("decision evidence line should serialize"),
        );
        decision_evidence.push('\n');
    }
    std::fs::write(&decision_evidence_path, decision_evidence)
        .expect("decision evidence should write");

    let written = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_decision_evidence_file(
        loaded,
        strategy_instance_id,
        &decision_evidence_path,
        100_000,
        market_selection_source_ref,
        &[TEST_MARKET_SELECTION_START_MS],
        &strategy_input_path,
    )
    .expect("source-bound runtime decision evidence should write strategy input evidence");

    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&strategy_input_path).expect("strategy input evidence should read"),
    )
    .expect("strategy input evidence should parse");
    assert_eq!(
        json["market_selection_source_path"],
        market_selection_source_ref.path.to_string_lossy().as_ref()
    );
    assert_eq!(
        json["market_selection_source_sha256"],
        market_selection_source_ref.sha256
    );
    assert_eq!(json["strategy_instance_id"], strategy_instance_id);
    assert_eq!(json["polymarket_condition_id"], TEST_CONDITION_ID);
    assert_eq!(json["polymarket_question_id"], TEST_QUESTION_ID);

    let audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &strategy_input_path,
        &written.sha256,
        "chainlink_data_streams.report_at_boundary",
    )
    .expect("strategy input evidence should parse");
    assert!(
        audit.is_approved(),
        "runtime snapshot artifact should approve"
    );

    let wrong_strategy_input_path = temp.path().join("wrong-strategy-input.json");
    let wrong_decision_evidence_path = temp.path().join("wrong-strategy-decision-evidence.jsonl");
    let mut wrong_snapshot = snapshot.clone();
    wrong_snapshot.strategy_id = "other-strategy-instance".to_string();
    let mut wrong_intent = intent.clone();
    wrong_intent.strategy_id = wrong_snapshot.strategy_id.clone();
    let mut wrong_admission = admission.clone();
    wrong_admission.strategy_id = wrong_snapshot.strategy_id.clone();
    let mut wrong_decision_evidence = String::new();
    for line in [
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": "bolt_v3.strategy_input_snapshot",
            "gate_version": "0.1.0",
            "kind": "strategy_input_snapshot",
            "snapshot": wrong_snapshot,
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 2_i64,
            "gate_id": "bolt_v3.order_intent",
            "gate_version": "0.1.0",
            "kind": "order_intent",
            "intent": wrong_intent,
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 3_i64,
            "gate_id": "bolt_v3.submit_admission",
            "gate_version": "0.1.0",
            "kind": "admission_decision",
            "decision": wrong_admission,
        }),
    ] {
        wrong_decision_evidence.push_str(
            &serde_json::to_string(&line).expect("decision evidence line should serialize"),
        );
        wrong_decision_evidence.push('\n');
    }
    std::fs::write(&wrong_decision_evidence_path, wrong_decision_evidence)
        .expect("wrong strategy decision evidence should write");

    let error = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_decision_evidence_file(
        loaded,
        strategy_instance_id,
        &wrong_decision_evidence_path,
        100_000,
        market_selection_source_ref,
        &[TEST_MARKET_SELECTION_START_MS],
        &wrong_strategy_input_path,
    )
    .expect_err("strategy input evidence must reject decision chains from another strategy");
    assert!(
        error.to_string().contains("strategy"),
        "strategy mismatch should be diagnostic: {error}"
    );
}

#[test]
#[allow(clippy::type_complexity)]
fn strategy_input_writer_rejects_runtime_snapshot_target_source_and_hash_mismatches() {
    let cases: [(
        &str,
        fn(&mut BoltV3StrategyInputEvidenceSnapshot, &mut WrittenOperatorArtifact),
        &str,
    ); 3] = [
        (
            "configured target",
            |snapshot, _source_ref| {
                snapshot.configured_target_id = "other-target".to_string();
            },
            "target",
        ),
        (
            "price-to-beat source",
            |snapshot, _source_ref| {
                snapshot.price_to_beat_source = "other-source".to_string();
            },
            "price-to-beat source",
        ),
        (
            "market-selection source hash",
            |_snapshot, source_ref| {
                source_ref.sha256 = "0".repeat(64);
            },
            "market-selection source hash",
        ),
    ];

    for (case_name, mutate, diagnostic) in cases {
        let fixture = strategy_input_runtime_fixture();
        let output_path = fixture.temp.path().join(format!("{case_name}.json"));
        let mut snapshot = fixture.snapshot.clone();
        let mut source_ref = fixture.market_selection_source_ref.clone();
        mutate(&mut snapshot, &mut source_ref);

        let error =
            bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
                &fixture.loaded,
                &fixture.strategy_instance_id,
                &snapshot,
                &source_ref,
                100_000,
                &fixture.candidate_market_start_timestamps_ms,
                &output_path,
            )
            .expect_err(case_name);

        assert!(
            error.to_string().contains(diagnostic),
            "{case_name} should include {diagnostic} diagnostic: {error}"
        );
        assert!(
            !output_path.exists(),
            "{case_name} failure must not leave strategy-input artifact"
        );
    }
}

#[test]
fn strategy_input_writer_rejects_oversized_market_selection_source_before_artifact() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture.temp.path().join("strategy-input.json");
    let source_len = std::fs::metadata(&fixture.market_selection_source_ref.path)
        .expect("market source metadata should read")
        .len();

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
            &fixture.loaded,
            &fixture.strategy_instance_id,
            &fixture.snapshot,
            &fixture.market_selection_source_ref,
            source_len.saturating_sub(1),
            &fixture.candidate_market_start_timestamps_ms,
            &output_path,
        )
        .expect_err("oversized market-selection source must fail before strategy-input write");

    let message = error.to_string();
    assert!(
        message.contains("failed to read market-selection source evidence"),
        "oversized market-selection source should be a read diagnostic: {message}"
    );
    assert!(
        !output_path.exists(),
        "oversized market-selection source failure must not write strategy-input artifact"
    );
}

#[test]
fn pre_run_market_window_source_proof_derives_source_owned_values() {
    let fixture = strategy_input_runtime_fixture();
    let loaded = &fixture.loaded;
    let strategy_instance_id = fixture.strategy_instance_id.as_str();
    let temp = &fixture.temp;
    let strategy_input_path = temp.path().join("strategy-input.json");
    let pre_run_state_path = temp.path().join("pre-run-state.json");

    let strategy_input = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
        loaded,
        strategy_instance_id,
        &fixture.snapshot,
        &fixture.market_selection_source_ref,
        100_000,
        &fixture.candidate_market_start_timestamps_ms,
        &strategy_input_path,
    )
    .expect("source-bound strategy input evidence should write");

    let proof = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_market_window_source_proof(
        &strategy_input_path,
        &strategy_input.sha256,
        fixture.snapshot.price_to_beat_source.as_str(),
        100_000,
    )
    .expect("source-bound market/window proof should collect");

    assert!(proof.market_state_approved);
    assert!(proof.market_window_approved);
    assert_eq!(proof.market_state_evidence_hash.len(), 64);
    assert!(
        proof
            .market_state_evidence_hash
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "market proof evidence hash should be lowercase hex"
    );
    assert!(
        !pre_run_state_path.exists(),
        "market/window proof collection must not write final pre-run-state.json"
    );
}

#[cfg(unix)]
#[test]
fn pre_run_market_window_source_proof_rejects_symlinked_market_source() {
    let fixture = strategy_input_runtime_fixture();
    let strategy_input_path = fixture.temp.path().join("strategy-input.json");
    let strategy_input = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &fixture.snapshot,
        &fixture.market_selection_source_ref,
        100_000,
        &fixture.candidate_market_start_timestamps_ms,
        &strategy_input_path,
    )
    .expect("source-bound strategy input evidence should write");
    let real_path = fixture
        .temp
        .path()
        .join("real-market-selection-source.json");
    std::fs::rename(&fixture.market_selection_source_ref.path, &real_path)
        .expect("market source should move behind symlink");
    std::os::unix::fs::symlink(&real_path, &fixture.market_selection_source_ref.path)
        .expect("market source symlink should create");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_market_window_source_proof(
        &strategy_input_path,
        &strategy_input.sha256,
        fixture.snapshot.price_to_beat_source.as_str(),
        100_000,
    )
    .expect_err("symlinked market source must not approve market/window proof");

    assert!(
        error.to_string().contains("market_selection_source_path"),
        "symlinked market source should identify source path validation: {error}"
    );
}

#[test]
fn pre_run_market_window_source_proof_rejects_stale_market_source_hash() {
    let fixture = strategy_input_runtime_fixture();
    let strategy_input_path = fixture.temp.path().join("strategy-input.json");
    let strategy_input = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &fixture.snapshot,
        &fixture.market_selection_source_ref,
        100_000,
        &fixture.candidate_market_start_timestamps_ms,
        &strategy_input_path,
    )
    .expect("source-bound strategy input evidence should write");
    std::fs::write(&fixture.market_selection_source_ref.path, b"{}")
        .expect("market source should mutate");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_market_window_source_proof(
        &strategy_input_path,
        &strategy_input.sha256,
        fixture.snapshot.price_to_beat_source.as_str(),
        100_000,
    )
    .expect_err("stale market source hash must not approve market/window proof");

    assert!(
        error.to_string().contains("market_selection_source_sha256"),
        "stale market source should identify source hash validation: {error}"
    );
}

#[test]
fn pre_run_market_window_source_proof_rejects_parent_dir_market_source_before_read() {
    let fixture = strategy_input_runtime_fixture();
    let strategy_input_path = fixture.temp.path().join("strategy-input.json");
    let strategy_input = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &fixture.snapshot,
        &fixture.market_selection_source_ref,
        100_000,
        &fixture.candidate_market_start_timestamps_ms,
        &strategy_input_path,
    )
    .expect("source-bound strategy input evidence should write");
    let intermediate_dir = fixture.temp.path().join("parent-dir-hop");
    std::fs::create_dir(&intermediate_dir).expect("intermediate directory should create");
    let parent_dir_source_path = intermediate_dir
        .join("..")
        .join(TEST_MARKET_SELECTION_SOURCE_FILE);
    let market_source_bytes = std::fs::read(&fixture.market_selection_source_ref.path)
        .expect("market source should read");
    let mut strategy_input_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&strategy_input_path).expect("strategy input should read"),
    )
    .expect("strategy input should parse");
    strategy_input_json["market_selection_source_path"] =
        serde_json::Value::String(parent_dir_source_path.to_string_lossy().into_owned());
    strategy_input_json["market_selection_source_sha256"] =
        serde_json::Value::String(sha256_bytes(&market_source_bytes));
    let strategy_input_bytes =
        serde_json::to_vec_pretty(&strategy_input_json).expect("strategy input should serialize");
    std::fs::write(&strategy_input_path, &strategy_input_bytes)
        .expect("strategy input should rewrite");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_market_window_source_proof(
        &strategy_input_path,
        &sha256_bytes(&strategy_input_bytes),
        fixture.snapshot.price_to_beat_source.as_str(),
        100_000,
    )
    .expect_err("parent-dir market source must not approve market/window proof");
    let message = error.to_string();

    assert!(
        message.contains("market_selection_source_path"),
        "parent-dir market source should fail source path policy before audit: {message}"
    );
    assert!(
        !message.contains("strategy_input_evidence"),
        "parent-dir market source must not be discovered only by later audit: {message}"
    );
    assert_ne!(
        strategy_input.sha256,
        sha256_bytes(&strategy_input_bytes),
        "test must rewrite strategy-input evidence so new hash is required"
    );
}

#[test]
fn pre_run_market_window_source_proof_rejects_oversized_market_source_before_audit() {
    let fixture = strategy_input_runtime_fixture();
    let strategy_input_path = fixture.temp.path().join("strategy-input.json");
    let strategy_input = bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
        &fixture.loaded,
        &fixture.strategy_instance_id,
        &fixture.snapshot,
        &fixture.market_selection_source_ref,
        100_000,
        &fixture.candidate_market_start_timestamps_ms,
        &strategy_input_path,
    )
    .expect("source-bound strategy input evidence should write");
    let strategy_input_len = std::fs::metadata(&strategy_input_path)
        .expect("strategy input should stat")
        .len();
    let max_bytes = strategy_input_len + 1;
    let mut market_source = std::fs::read(&fixture.market_selection_source_ref.path)
        .expect("market source should read");
    market_source.extend(std::iter::repeat_n(b' ', max_bytes as usize));
    std::fs::write(&fixture.market_selection_source_ref.path, market_source)
        .expect("oversized market source should write");

    let error = bolt_v2::bolt_v3_operator_artifacts::collect_pre_run_market_window_source_proof(
        &strategy_input_path,
        &strategy_input.sha256,
        fixture.snapshot.price_to_beat_source.as_str(),
        max_bytes,
    )
    .expect_err("oversized market source must not approve market/window proof");

    assert!(
        error.to_string().contains("market_selection_source_path"),
        "oversized market source should fail before audit: {error}"
    );
}

#[test]
fn pre_run_market_window_source_proof_audits_already_bounded_bytes() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");
    let collector_start = source
        .find("pub fn collect_pre_run_market_window_source_proof")
        .expect("market/window collector should exist");
    let collector_end = source[collector_start..]
        .find("fn read_strategy_input_market_selection_source_bytes")
        .map(|offset| collector_start + offset)
        .expect("market/window source validator should follow collector");
    let collector_source = &source[collector_start..collector_end];

    assert!(
        collector_source.contains("from_evidence_bytes_with_market_selection_source"),
        "collector should audit the same bounded bytes it already validated"
    );
    assert!(
        !collector_source.contains("from_evidence_file"),
        "collector must not reopen strategy-input or market-selection evidence after bounded validation"
    );
}

#[test]
fn strategy_input_writer_reports_market_selection_source_read_as_read_error() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture.temp.path().join("strategy-input.json");
    let missing_ref = WrittenOperatorArtifact {
        path: fixture
            .temp
            .path()
            .join("missing-market-selection-source.json"),
        sha256: fixture.market_selection_source_ref.sha256.clone(),
    };

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
            &fixture.loaded,
            &fixture.strategy_instance_id,
            &fixture.snapshot,
            &missing_ref,
            100_000,
            &fixture.candidate_market_start_timestamps_ms,
            &output_path,
        )
        .expect_err("missing market-selection source should fail before write");

    let message = error.to_string();
    assert!(
        message.contains("failed to read market-selection source evidence"),
        "missing market-selection source should be a read diagnostic: {message}"
    );
    assert!(
        !message.contains("failed to write operator artifact"),
        "missing market-selection source must not be reported as output write failure: {message}"
    );
    assert!(
        !message.contains(missing_ref.path.to_string_lossy().as_ref()),
        "missing market-selection source diagnostic must not print source path: {message}"
    );
    let source_message = std::error::Error::source(&error)
        .expect("read diagnostic should preserve source error")
        .to_string();
    assert!(
        !source_message.contains(missing_ref.path.to_string_lossy().as_ref()),
        "missing market-selection source error chain must not print source path: {source_message}"
    );
    assert!(
        !output_path.exists(),
        "read failure must not leave strategy-input artifact"
    );
}

#[test]
fn strategy_input_writer_reports_market_selection_source_json_as_parse_error() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture.temp.path().join("strategy-input.json");
    let invalid_json = b"{not-json";
    std::fs::write(&fixture.market_selection_source_ref.path, invalid_json)
        .expect("invalid market-selection source should write");
    let invalid_ref = WrittenOperatorArtifact {
        path: fixture.market_selection_source_ref.path.clone(),
        sha256: sha256_bytes(invalid_json),
    };

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
            &fixture.loaded,
            &fixture.strategy_instance_id,
            &fixture.snapshot,
            &invalid_ref,
            100_000,
            &fixture.candidate_market_start_timestamps_ms,
            &output_path,
        )
        .expect_err("invalid market-selection source JSON should fail before write");

    let message = error.to_string();
    assert!(
        message.contains("failed to parse market-selection source evidence"),
        "invalid market-selection source JSON should be a parse diagnostic: {message}"
    );
    assert!(
        !message.contains("failed to serialize operator artifact"),
        "invalid market-selection source JSON must not be reported as serialization failure: {message}"
    );
    assert!(
        !message.contains(invalid_ref.path.to_string_lossy().as_ref()),
        "invalid market-selection source diagnostic must not print source path: {message}"
    );
    let source_message = std::error::Error::source(&error)
        .expect("parse diagnostic should preserve source error")
        .to_string();
    assert!(
        !source_message.contains(invalid_ref.path.to_string_lossy().as_ref()),
        "invalid market-selection source error chain must not print source path: {source_message}"
    );
    assert!(
        !output_path.exists(),
        "parse failure must not leave strategy-input artifact"
    );
}

#[cfg(unix)]
#[test]
fn strategy_input_writer_rejects_symlinked_market_selection_source_before_artifact() {
    let fixture = strategy_input_runtime_fixture();
    let output_path = fixture.temp.path().join("strategy-input.json");
    let real_path = fixture
        .temp
        .path()
        .join("real-market-selection-source.json");
    std::fs::rename(&fixture.market_selection_source_ref.path, &real_path)
        .expect("market-selection source should move behind symlink");
    std::os::unix::fs::symlink(&real_path, &fixture.market_selection_source_ref.path)
        .expect("market-selection source symlink should create");

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_strategy_input_evidence_artifact_from_runtime_snapshot(
            &fixture.loaded,
            &fixture.strategy_instance_id,
            &fixture.snapshot,
            &fixture.market_selection_source_ref,
            100_000,
            &fixture.candidate_market_start_timestamps_ms,
            &output_path,
        )
        .expect_err("symlinked market-selection source must fail before artifact write");

    let message = error.to_string();
    assert!(
        message.contains("failed to read market-selection source evidence"),
        "symlinked market-selection source should be a read diagnostic: {message}"
    );
    assert!(
        !message.contains(
            fixture
                .market_selection_source_ref
                .path
                .to_string_lossy()
                .as_ref()
        ),
        "symlinked market-selection source diagnostic must not print source path: {message}"
    );
    let source_message = std::error::Error::source(&error)
        .expect("symlink diagnostic should preserve source error")
        .to_string();
    assert!(
        !source_message.contains(
            fixture
                .market_selection_source_ref
                .path
                .to_string_lossy()
                .as_ref()
        ),
        "symlinked market-selection source error chain must not print source path: {source_message}"
    );
    assert!(
        !output_path.exists(),
        "symlinked market-selection source failure must not leave strategy-input artifact"
    );
    assert!(
        std::fs::symlink_metadata(&fixture.market_selection_source_ref.path)
            .expect("source symlink metadata should read")
            .file_type()
            .is_symlink(),
        "failed symlink read must leave original symlink untouched"
    );
}

#[test]
fn static_artifact_reader_uses_no_follow_identity_verified_open() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");

    assert!(
        !source.contains("fs::File::open(path)"),
        "operator artifact reads must not use symlink-following File::open"
    );
    assert!(
        source.contains("O_NOFOLLOW"),
        "operator artifact reads must use no-follow open on Unix"
    );
    assert!(
        source.contains("MetadataExt"),
        "operator artifact reads must compare opened file identity on Unix"
    );
}

#[test]
fn approval_nonce_builder_zeroizes_raw_nonce_after_hashing() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");

    assert!(
        source.contains("nonce.zeroize()"),
        "approval nonce builder must clear raw nonce bytes after hashing"
    );
}

struct StrategyInputRuntimeFixture {
    temp: tempfile::TempDir,
    loaded: bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    strategy_instance_id: String,
    market_selection_source_ref: WrittenOperatorArtifact,
    candidate_market_start_timestamps_ms: Vec<u64>,
    snapshot: BoltV3StrategyInputEvidenceSnapshot,
}

fn strategy_input_runtime_fixture() -> StrategyInputRuntimeFixture {
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .clone();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let market_slug = updown_market_slug(
        TEST_MARKET_SELECTION_UNDERLYING_ASSET,
        TEST_MARKET_SELECTION_CADENCE_SLUG,
        TEST_MARKET_SELECTION_CURRENT_START_SECONDS,
    );
    let market_selection_source =
        bolt_v2::bolt_v3_operator_artifacts::build_market_selection_source_artifact(
            &loaded,
            strategy_instance_id.as_str(),
            &[
                updown_binary_option(
                    TEST_UP_INSTRUMENT_ID,
                    &market_slug,
                    TEST_MARKET_ID,
                    TEST_CONDITION_ID,
                    TEST_QUESTION_ID,
                    TEST_UP_OUTCOME,
                    TEST_MARKET_SELECTION_START_MS,
                    TEST_MARKET_SELECTION_END_MS,
                ),
                updown_binary_option(
                    TEST_DOWN_INSTRUMENT_ID,
                    &market_slug,
                    TEST_MARKET_ID,
                    TEST_CONDITION_ID,
                    TEST_QUESTION_ID,
                    TEST_DOWN_OUTCOME,
                    TEST_MARKET_SELECTION_START_MS,
                    TEST_MARKET_SELECTION_END_MS,
                ),
            ],
            TEST_MARKET_SELECTION_NOW_MS,
        )
        .expect("market selection source should build");
    let market_selection_source_path = temp.path().join(TEST_MARKET_SELECTION_SOURCE_FILE);
    std::fs::write(
        &market_selection_source_path,
        serde_json::to_vec_pretty(&market_selection_source)
            .expect("market selection source should serialize"),
    )
    .expect("market selection source should write");
    let market_selection_source_ref = WrittenOperatorArtifact {
        path: market_selection_source_path.clone(),
        sha256: Phase8OperatorApprovalEnvelope::sha256_file(&market_selection_source_path)
            .expect("market selection source sha256 should compute"),
    };
    let snapshot = BoltV3StrategyInputEvidenceSnapshot {
        strategy_id: strategy_instance_id.clone(),
        configured_target_id: "btc_updown_5m".to_string(),
        market_selection_ruleset_id: "btc_updown_5m".to_string(),
        market_selection_outcome: "current".to_string(),
        market_id: Some(TEST_MARKET_ID.to_string()),
        polymarket_condition_id: Some(TEST_CONDITION_ID.to_string()),
        polymarket_market_slug: Some(market_slug),
        polymarket_question_id: Some(TEST_QUESTION_ID.to_string()),
        up_instrument_id: Some(TEST_UP_INSTRUMENT_ID.to_string()),
        down_instrument_id: Some(TEST_DOWN_INSTRUMENT_ID.to_string()),
        market_selection_timestamp_ms: Some(TEST_MARKET_SELECTION_NOW_MS),
        selected_market_observed_timestamp_ms: Some(TEST_MARKET_SELECTION_NOW_MS),
        polymarket_market_start_timestamp_ms: Some(TEST_MARKET_SELECTION_START_MS),
        polymarket_market_end_timestamp_ms: Some(TEST_MARKET_SELECTION_END_MS),
        price_to_beat_source: "chainlink_data_streams.report_at_boundary".to_string(),
        price_to_beat_value: "3100".to_string(),
        reference_quote_ts_event: TEST_MARKET_SELECTION_NOW_MS,
        spot_price: "3100.5".to_string(),
        reference_fair_value: Some("3100.5".to_string()),
        realized_volatility: "1.5".to_string(),
        seconds_to_market_end: 300,
        pricing_kurtosis: "3".to_string(),
        theta_decay_factor: "1".to_string(),
        theta_scaled_min_edge_bps: "12.5".to_string(),
        fair_probability_up: "0.6".to_string(),
        uncertainty_band_probability: "0.01".to_string(),
        expected_edge_basis_points: "12.5".to_string(),
        worst_case_edge_basis_points: "12.5".to_string(),
        fee_rate_basis_points: "0".to_string(),
        selected_side: Some("up".to_string()),
        submission_instrument_id: TEST_UP_INSTRUMENT_ID.to_string(),
        submission_order_side: "Buy".to_string(),
        submission_price: "0.50".to_string(),
        submission_quantity: "1".to_string(),
        client_order_id: "client-order-one".to_string(),
    };

    StrategyInputRuntimeFixture {
        temp,
        loaded,
        strategy_instance_id,
        market_selection_source_ref,
        candidate_market_start_timestamps_ms: vec![TEST_MARKET_SELECTION_START_MS],
        snapshot,
    }
}

fn write_entry_decision_evidence_chain(
    temp: &tempfile::TempDir,
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> std::path::PathBuf {
    let intent = BoltV3OrderIntentEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        intent_kind: BoltV3OrderIntentKind::Entry,
        instrument_id: snapshot.submission_instrument_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        order_side: snapshot.submission_order_side.clone(),
        price: snapshot.submission_price.clone(),
        quantity: snapshot.submission_quantity.clone(),
        order_fields: BoltV3OrderIntentOrderFields {
            order_type: "Limit".to_string(),
            time_in_force: "Gtc".to_string(),
            price: Some(snapshot.submission_price.clone()),
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            expire_time_unix_nanos: None,
            is_post_only: false,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    };
    let admission = BoltV3AdmissionDecisionEvidence {
        strategy_id: snapshot.strategy_id.clone(),
        client_order_id: snapshot.client_order_id.clone(),
        instrument_id: snapshot.submission_instrument_id.clone(),
        notional: "0.50".to_string(),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        outcome: BoltV3AdmissionOutcome::RejectedNotArmed,
        loss_halt_reasons: Vec::new(),
    };
    let decision_evidence_path = temp.path().join("decision-evidence.jsonl");
    let mut decision_evidence = String::new();
    for line in [
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 1_i64,
            "gate_id": "bolt_v3.strategy_input_snapshot",
            "gate_version": "0.1.0",
            "kind": "strategy_input_snapshot",
            "snapshot": snapshot,
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 2_i64,
            "gate_id": "bolt_v3.order_intent",
            "gate_version": "0.1.0",
            "kind": "order_intent",
            "intent": intent,
        }),
        serde_json::json!({
            "schema_version": 6,
            "recorded_at_utc_ns": 3_i64,
            "gate_id": "bolt_v3.submit_admission",
            "gate_version": "0.1.0",
            "kind": "admission_decision",
            "decision": admission,
        }),
    ] {
        decision_evidence.push_str(
            &serde_json::to_string(&line).expect("decision evidence line should serialize"),
        );
        decision_evidence.push('\n');
    }
    std::fs::write(&decision_evidence_path, decision_evidence)
        .expect("decision evidence should write");
    decision_evidence_path
}

fn market_selection_instruments_for_slug(market_slug: &str) -> [InstrumentAny; 2] {
    [
        updown_binary_option(
            TEST_UP_INSTRUMENT_ID,
            market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_UP_OUTCOME,
            TEST_MARKET_SELECTION_START_MS,
            TEST_MARKET_SELECTION_END_MS,
        ),
        updown_binary_option(
            TEST_DOWN_INSTRUMENT_ID,
            market_slug,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_DOWN_OUTCOME,
            TEST_MARKET_SELECTION_START_MS,
            TEST_MARKET_SELECTION_END_MS,
        ),
    ]
}

fn load_fixture_with_live_canary() -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(&repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load");
    loaded.root.live_canary = Some(LiveCanaryBlock {
        approval_id: "test-operator-approval".to_string(),
        no_submit_readiness_report_path: "reports/no-submit-readiness.json".to_string(),
        max_no_submit_readiness_report_bytes: 1_000_000,
        readiness_report_max_age_seconds: 300,
        reference_quote_max_age_seconds: 30,
        reference_quote_wait_timeout_seconds: 5,
        reference_quote_probe_actor_id: "test-reference-probe".to_string(),
        reference_quote_probe_log_events: false,
        reference_quote_probe_log_commands: false,
        max_live_order_count: 1,
        max_notional_per_order: loaded.root.risk.default_max_notional_per_order.clone(),
        operator_evidence: None,
    });
    loaded
}

struct FinalPacketFixture {
    temp: tempfile::TempDir,
    loaded: bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    static_manifest_path: std::path::PathBuf,
    operator_packet_path: std::path::PathBuf,
}

impl FinalPacketFixture {
    fn operator_evidence(&self) -> &LiveCanaryOperatorEvidenceBlock {
        self.loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|live_canary| live_canary.operator_evidence.as_ref())
            .expect("final packet fixture should configure operator evidence")
    }
}

fn assembled_final_packet_fixture() -> FinalPacketFixture {
    let mut loaded = load_fixture_with_live_canary();
    loaded.config_bundle_checksum = sha256_text("final-packet-config-bundle");
    let temp = tempfile::tempdir().expect("tempdir should create");
    let mut operator_evidence = test_operator_evidence_packet_bindings(temp.path());
    operator_evidence.head_sha = option_env!("BOLT_V3_BUILD_HEAD_SHA")
        .unwrap_or_else(|| {
            panic!("build head sha should be compiled for final-packet verifier tests")
        })
        .to_string();
    let refs = write_required_static_artifacts_for_test(temp.path(), &mut operator_evidence);
    let manifest_path = temp.path().join("static-artifacts-manifest.json");
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs.clone(),
        Vec::new(),
    );

    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .operator_evidence = Some(operator_evidence.clone());
    bind_expected_approval_envelope_hash(&mut loaded, &mut operator_evidence);
    write_final_live_evidence_artifacts_for_test(&loaded, &operator_evidence);
    write_static_artifacts_manifest_for_test(
        &manifest_path,
        &loaded.config_bundle_checksum,
        refs,
        Vec::new(),
    );
    let operator_packet_path = temp.path().join("operator-evidence-packet.json");
    let final_packet =
        bolt_v2::bolt_v3_operator_artifacts::assemble_operator_packet_from_static_manifest(
            &loaded,
            &manifest_path,
            &operator_packet_path,
        )
        .expect("final packet should assemble");
    assert_eq!(
        final_packet.approval_envelope.sha256,
        operator_evidence.approval_envelope_sha256
    );

    FinalPacketFixture {
        temp,
        loaded,
        static_manifest_path: manifest_path,
        operator_packet_path,
    }
}

fn bind_expected_approval_envelope_hash(
    loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    operator_evidence: &mut LiveCanaryOperatorEvidenceBlock,
) {
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .operator_evidence = Some(operator_evidence.clone());
    operator_evidence.approval_envelope_sha256 =
        bolt_v2::bolt_v3_operator_artifacts::compute_operator_approval_envelope_sha256(loaded)
            .expect("approval envelope hash should compute");
    loaded
        .root
        .live_canary
        .as_mut()
        .expect("live canary should exist")
        .operator_evidence = Some(operator_evidence.clone());
}

fn write_final_live_evidence_artifacts_for_test(
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    operator_evidence: &LiveCanaryOperatorEvidenceBlock,
) {
    let decision_hash = write_final_bytes_for_test(
        &operator_evidence.decision_evidence_path,
        b"{\"kind\":\"admission_decision\",\"outcome\":\"admitted\"}\n",
    );
    let nt_submit_hash = write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.nt_submit_event_path),
        &serde_json::json!({"record_kind": "phase8.nt_submit_event.v1"}),
    );
    let venue_order_hash = write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.venue_order_state_path),
        &serde_json::json!({"record_kind": "phase8.venue_order_state.v1"}),
    );
    let restart_hash = write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.restart_reconciliation_path),
        &serde_json::json!({"record_kind": "phase8.restart_reconciliation.v1"}),
    );
    let hygiene_hash = write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.post_run_hygiene_path),
        &serde_json::json!({"record_kind": "phase8.post_run_hygiene.v1"}),
    );
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .expect("live canary should exist");
    let root_toml_sha256 = sha256_file(&loaded.root_path);
    let canary = serde_json::json!({
        "schema_version": 1,
        "head_sha": operator_evidence.head_sha,
        "root_config_sha256": root_toml_sha256.clone(),
        "ssm_manifest_sha256": operator_evidence.ssm_manifest_sha256,
        "ssm_manifest_ref": final_evidence_ref_for_test(
            &operator_evidence.ssm_manifest_path,
            &operator_evidence.ssm_manifest_sha256,
        ),
        "strategy_input_evidence_ref": final_evidence_ref_for_test(
            &operator_evidence.strategy_input_evidence_path,
            &operator_evidence.strategy_input_evidence_sha256,
        ),
        "approval_id_hash": sha256_text(&live_canary.approval_id),
        "max_live_order_count": live_canary.max_live_order_count,
        "max_notional_per_order": live_canary.max_notional_per_order.to_string(),
        "decision_evidence_ref": final_evidence_ref_for_test(
            &operator_evidence.decision_evidence_path,
            &decision_hash,
        ),
        "submit_admission_ref": {
            "status": "accepted",
            "admitted_order_count": live_canary.max_live_order_count,
            "reason": "nt_adapter_submit_proven"
        },
        "live_order_ref": {
            "strategy_instance_id_hash": sha256_text("test-strategy"),
            "client_order_id_hash": sha256_text("test-client-order"),
            "venue_order_id_hash": sha256_text("test-venue-order")
        },
        "nt_submit_event_ref": final_evidence_ref_for_test(
            &operator_evidence.nt_submit_event_path,
            &nt_submit_hash,
        ),
        "venue_order_state_ref": final_evidence_ref_for_test(
            &operator_evidence.venue_order_state_path,
            &venue_order_hash,
        ),
        "strategy_cancel_ref": serde_json::Value::Null,
        "restart_reconciliation_ref": final_evidence_ref_for_test(
            &operator_evidence.restart_reconciliation_path,
            &restart_hash,
        ),
        "post_run_hygiene_ref": final_evidence_ref_for_test(
            &operator_evidence.post_run_hygiene_path,
            &hygiene_hash,
        ),
        "runtime_capture_ref": {
            "spool_root_hash": sha256_text("test-spool-root"),
            "run_id": "test-run"
        },
        "nt_lifecycle_refs": [],
        "outcome": "live_canary_proof",
        "block_reasons": []
    });
    write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.canary_evidence_path),
        &canary,
    );
    let approval_consumption = serde_json::json!({
        "schema_version": 1,
        "record_kind": "phase8_operator_approval_consumption",
        "head_sha": operator_evidence.head_sha,
        "root_toml_sha256": root_toml_sha256,
        "approval_envelope_sha256": operator_evidence.approval_envelope_sha256,
        "ssm_manifest_sha256": operator_evidence.ssm_manifest_sha256,
        "strategy_input_evidence_sha256": operator_evidence.strategy_input_evidence_sha256,
        "financial_envelope_sha256": operator_evidence.financial_envelope_sha256,
        "pre_run_state_sha256": operator_evidence.pre_run_state_sha256,
        "abort_plan_sha256": operator_evidence.abort_plan_sha256,
        "approval_id_hash": sha256_text(&live_canary.approval_id),
        "approval_nonce_sha256": operator_evidence.approval_nonce_sha256,
        "approval_not_before_unix_secs": operator_evidence.approval_not_before_unix_seconds,
        "approval_not_after_unix_secs": operator_evidence.approval_not_after_unix_seconds,
        "canary_evidence_path_hash": sha256_text(&operator_evidence.canary_evidence_path),
        "consumed_unix_secs": operator_evidence.approval_not_before_unix_seconds,
    });
    write_json_value_and_hash(
        std::path::Path::new(&operator_evidence.approval_consumption_path),
        &approval_consumption,
    );
}

fn write_final_bytes_for_test(path: &str, bytes: &[u8]) -> String {
    std::fs::write(path, bytes).expect("final evidence fixture should write");
    sha256_bytes(bytes)
}

fn final_evidence_ref_for_test(path: &str, record_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "path_hash": sha256_text(path),
        "record_hash": record_hash,
    })
}

fn read_json_value(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("JSON artifact should read"))
        .expect("JSON artifact should parse")
}

fn write_json_value_and_hash(path: &std::path::Path, value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec_pretty(value).expect("JSON artifact should serialize");
    std::fs::write(path, &bytes).expect("JSON artifact should write");
    sha256_bytes(&bytes)
}

fn sha256_json_value(value: &serde_json::Value) -> String {
    sha256_bytes(&serde_json::to_vec_pretty(value).expect("JSON value should serialize"))
}

fn pre_run_state_source_bundle_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.pre_run_state_source_proof_bundle.v1",
        "host_clock_skew_within_bound": true,
        "host_clock_evidence": {
            "source": "clock-skew-probe",
            "observed_skew_ms": 1,
            "max_skew_ms": 100
        },
        "conflicting_open_orders_absent": true,
        "preexisting_position_absent": true,
        "venue_account_state_evidence": {
            "source": "venue-account-state-probe",
            "open_orders": [],
            "positions": []
        },
        "market_state_approved": true,
        "market_window_approved": true,
        "market_state_evidence_hash": sha256_text("market-window-proof"),
        "funding_margin_covers_max_notional_plus_fees": true,
        "funding_margin_evidence": {
            "source": "funding-margin-probe",
            "available_collateral": "10.00",
            "required_collateral": "1.00"
        },
        "single_runner_lock_acquired": true,
        "single_runner_lock_evidence": {
            "source": "single-runner-lock",
            "lock_held": true
        },
        "egress_identity_approved": true,
        "egress_identity_evidence": {
            "source": "egress-identity-probe",
            "approved": true
        },
        "clob_v2_adapter_signing_verified": true,
        "clob_v2_adapter_signing_evidence": {
            "source": "clob-signing-audit",
            "verified": true
        },
        "clob_v2_collateral_accounting_verified": true,
        "clob_v2_collateral_accounting_evidence": {
            "source": "clob-collateral-audit",
            "verified": true
        },
        "clob_v2_fee_behavior_verified": true,
        "clob_v2_fee_behavior_evidence": {
            "source": "clob-fee-audit",
            "verified": true
        },
        "release_manifest_clob_signing_version": "clob-v2-release-test",
        "release_manifest_nt_revision_matches_compiled_pin": true,
        "release_manifest_evidence_hash": sha256_text("release-manifest-proof")
    })
}

fn abort_plan_source_bundle_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.abort_plan_source_proof_bundle.v1",
        "cancel_if_open_defined": true,
        "cancel_if_open_evidence": {
            "source": "cancel-if-open-policy",
            "defined": true
        },
        "nt_accepted_venue_pending_abort_defined": true,
        "nt_accepted_venue_pending_abort_evidence": {
            "source": "nt-accepted-venue-pending-policy",
            "defined": true
        },
        "partial_fill_abort_defined": true,
        "partial_fill_abort_evidence": {
            "source": "partial-fill-policy",
            "defined": true
        },
        "network_partition_during_submit_abort_defined": true,
        "network_partition_during_submit_abort_evidence": {
            "source": "network-partition-policy",
            "defined": true
        },
        "panic_gate_trip_abort_defined": true,
        "panic_gate_trip_abort_evidence": {
            "source": "panic-gate-service-policy",
            "defined": true
        }
    })
}

fn mutate_packet_json<F>(path: &std::path::Path, mutate: F)
where
    F: FnOnce(&mut serde_json::Value),
{
    let mut value = read_json_value(path);
    mutate(&mut value);
    write_json_value_and_hash(path, &value);
}

#[allow(clippy::too_many_arguments)]
fn updown_binary_option(
    instrument_id: &str,
    market_slug: &str,
    market_id: &str,
    condition_id: &str,
    question_id: &str,
    outcome: &str,
    activation_ms: u64,
    expiration_ms: u64,
) -> InstrumentAny {
    let mut info = Params::new();
    info.insert(
        "market_slug".to_string(),
        serde_json::Value::String(market_slug.to_string()),
    );
    info.insert(
        "market_id".to_string(),
        serde_json::Value::String(market_id.to_string()),
    );
    info.insert(
        "condition_id".to_string(),
        serde_json::Value::String(condition_id.to_string()),
    );
    info.insert(
        "question_id".to_string(),
        serde_json::Value::String(question_id.to_string()),
    );
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(instrument_id),
        Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
        AssetClass::Alternative,
        Currency::USDC(),
        (activation_ms.saturating_mul(1_000_000)).into(),
        (expiration_ms.saturating_mul(1_000_000)).into(),
        3,
        2,
        Price::from(TEST_BINARY_OPTION_PRICE_INCREMENT),
        Quantity::from(TEST_BINARY_OPTION_SIZE_INCREMENT),
        Some(ustr::Ustr::from(outcome)),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(info),
        1.into(),
        1.into(),
    ))
}

struct TestPreRunStateProofHashes {
    host_clock: String,
    venue_account: String,
    market: String,
    funding: String,
    single_runner: String,
    egress: String,
    clob_signing: String,
    clob_collateral: String,
    clob_fee: String,
    release_manifest: String,
}

impl TestPreRunStateProofHashes {
    fn new() -> Self {
        Self {
            host_clock: sha256_text("host-clock-proof"),
            venue_account: sha256_text("venue-account-state-proof"),
            market: sha256_text("market-window-proof"),
            funding: sha256_text("funding-margin-proof"),
            single_runner: sha256_text("single-runner-lock-proof"),
            egress: sha256_text("egress-identity-proof"),
            clob_signing: sha256_text("clob-v2-signing-proof"),
            clob_collateral: sha256_text("clob-v2-collateral-proof"),
            clob_fee: sha256_text("clob-v2-fee-proof"),
            release_manifest: sha256_text("release-manifest-proof"),
        }
    }

    fn as_source_proofs(&self) -> Phase8PreRunStateSourceProofs<'_> {
        Phase8PreRunStateSourceProofs {
            host_clock_skew_within_bound: true,
            host_clock_skew_evidence_hash: &self.host_clock,
            conflicting_open_orders_absent: true,
            preexisting_position_absent: true,
            venue_account_state_evidence_hash: &self.venue_account,
            market_state_approved: true,
            market_window_approved: true,
            market_state_evidence_hash: &self.market,
            funding_margin_covers_max_notional_plus_fees: true,
            funding_margin_evidence_hash: &self.funding,
            single_runner_lock_acquired: true,
            single_runner_lock_evidence_hash: &self.single_runner,
            egress_identity_approved: true,
            egress_identity_evidence_hash: &self.egress,
            clob_v2_adapter_signing_verified: true,
            clob_v2_adapter_signing_evidence_hash: &self.clob_signing,
            clob_v2_collateral_accounting_verified: true,
            clob_v2_collateral_accounting_evidence_hash: &self.clob_collateral,
            clob_v2_fee_behavior_verified: true,
            clob_v2_fee_behavior_evidence_hash: &self.clob_fee,
            release_manifest_clob_signing_version: "clob-v2-release-test",
            release_manifest_nt_revision_matches_compiled_pin: true,
            release_manifest_evidence_hash: &self.release_manifest,
        }
    }
}

struct TestAbortPlanProofHashes {
    cancel: String,
    venue_pending: String,
    partial_fill: String,
    network_partition: String,
    panic_gate: String,
}

impl TestAbortPlanProofHashes {
    fn new() -> Self {
        Self {
            cancel: sha256_text("cancel-if-open-proof"),
            venue_pending: sha256_text("nt-accepted-venue-pending-proof"),
            partial_fill: sha256_text("partial-fill-proof"),
            network_partition: sha256_text("network-partition-proof"),
            panic_gate: sha256_text("panic-gate-service-policy-proof"),
        }
    }

    fn as_source_proofs(&self) -> Phase8AbortPlanSourceProofs<'_> {
        Phase8AbortPlanSourceProofs {
            cancel_if_open_defined: true,
            cancel_if_open_evidence_hash: &self.cancel,
            nt_accepted_venue_pending_abort_defined: true,
            nt_accepted_venue_pending_abort_evidence_hash: &self.venue_pending,
            partial_fill_abort_defined: true,
            partial_fill_abort_evidence_hash: &self.partial_fill,
            network_partition_during_submit_abort_defined: true,
            network_partition_during_submit_abort_evidence_hash: &self.network_partition,
            panic_gate_trip_abort_defined: true,
            panic_gate_trip_abort_evidence_hash: &self.panic_gate,
        }
    }
}

fn assert_rejects_undefined_abort_plan_source_path<F>(expected_field: &str, mutate: F)
where
    F: FnOnce(&mut Phase8AbortPlanSourceProofs),
{
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let abort_plan_path = temp.path().join(format!("{expected_field}.json"));
    let proof_hashes = TestAbortPlanProofHashes::new();
    let mut source_proofs = proof_hashes.as_source_proofs();
    mutate(&mut source_proofs);

    let error = bolt_v2::bolt_v3_operator_artifacts::write_abort_plan_artifact_from_source_proofs(
        &loaded,
        strategy_instance_id,
        source_proofs,
        &abort_plan_path,
    )
    .expect_err("undefined abort source path should fail closed");

    assert!(
        error.to_string().contains(expected_field),
        "abort plan blocker should cite {expected_field}: {error}"
    );
    assert!(
        !abort_plan_path.exists(),
        "failed abort-plan source generation must not leave artifact for {expected_field}"
    );
}

fn assert_rejects_unsatisfied_pre_run_source_proof<F>(expected_field: &str, mutate: F)
where
    F: for<'a> FnOnce(&mut Phase8PreRunStateSourceProofs<'a>),
{
    let loaded = load_fixture_with_live_canary();
    let strategy_instance_id = loaded
        .strategies
        .first()
        .expect("fixture should load a strategy")
        .config
        .strategy_instance_id
        .as_str();
    let temp = tempfile::tempdir().expect("tempdir should create");
    let pre_run_state_path = temp.path().join(format!("{expected_field}.json"));
    let proof_hashes = TestPreRunStateProofHashes::new();
    let mut source_proofs = proof_hashes.as_source_proofs();
    mutate(&mut source_proofs);

    let error =
        bolt_v2::bolt_v3_operator_artifacts::write_pre_run_state_artifact_from_source_proofs(
            &loaded,
            strategy_instance_id,
            source_proofs,
            &pre_run_state_path,
        )
        .expect_err("unsatisfied source proof should fail closed");

    assert!(
        error.to_string().contains(expected_field),
        "pre-run state blocker should cite {expected_field}: {error}"
    );
    assert!(
        !pre_run_state_path.exists(),
        "failed source-proof pre-run state generation must not leave artifact for {expected_field}"
    );
}

fn assert_manifest_entry(
    manifest: &bolt_v2::bolt_v3_operator_artifacts::BoltV3RedactedSsmManifest,
    client_key: &str,
    provider_key: &str,
    field_name: &str,
) {
    manifest
        .entries
        .iter()
        .find(|entry| {
            entry.client_key == client_key
                && entry.provider_key == provider_key
                && entry.field_name == field_name
        })
        .expect("expected redacted SSM manifest entry");
}

fn test_operator_evidence_packet_bindings(
    dir: &std::path::Path,
) -> LiveCanaryOperatorEvidenceBlock {
    LiveCanaryOperatorEvidenceBlock {
        head_sha: "1234567890abcdef1234567890abcdef12345678".to_string(),
        max_operator_evidence_file_bytes: 4096,
        approval_consumption_max_age_seconds: 60,
        approval_envelope_path: dir
            .join("approval-envelope.json")
            .to_string_lossy()
            .to_string(),
        approval_envelope_sha256: "0".repeat(64),
        ssm_manifest_path: dir.join("ssm-manifest.json").to_string_lossy().to_string(),
        ssm_manifest_sha256: String::new(),
        strategy_input_evidence_path: dir
            .join("strategy-input.json")
            .to_string_lossy()
            .to_string(),
        strategy_input_evidence_sha256: String::new(),
        financial_envelope_path: dir
            .join("financial-envelope.json")
            .to_string_lossy()
            .to_string(),
        financial_envelope_sha256: String::new(),
        pre_run_state_path: dir.join("pre-run-state.json").to_string_lossy().to_string(),
        pre_run_state_sha256: String::new(),
        abort_plan_path: dir.join("abort-plan.json").to_string_lossy().to_string(),
        abort_plan_sha256: String::new(),
        canary_evidence_path: dir
            .join("canary-evidence.json")
            .to_string_lossy()
            .to_string(),
        approval_not_before_unix_seconds: 1_900_000_000,
        approval_not_after_unix_seconds: 1_900_000_300,
        approval_nonce_path: dir
            .join("approval-nonce.json")
            .to_string_lossy()
            .to_string(),
        approval_nonce_sha256: String::new(),
        approval_consumption_path: dir
            .join("approval-consumed.json")
            .to_string_lossy()
            .to_string(),
        decision_evidence_path: dir
            .join("decision-evidence.jsonl")
            .to_string_lossy()
            .to_string(),
        nt_submit_event_path: dir
            .join("nt-submit-event.json")
            .to_string_lossy()
            .to_string(),
        venue_order_state_path: dir
            .join("venue-order-state.json")
            .to_string_lossy()
            .to_string(),
        strategy_cancel_path: None,
        restart_reconciliation_path: dir
            .join("restart-reconciliation.json")
            .to_string_lossy()
            .to_string(),
        post_run_hygiene_path: dir
            .join("post-run-hygiene.json")
            .to_string_lossy()
            .to_string(),
    }
}

fn write_required_static_artifacts_for_test(
    dir: &std::path::Path,
    operator_evidence: &mut LiveCanaryOperatorEvidenceBlock,
) -> Vec<serde_json::Value> {
    let bindings = [
        (
            "ssm-manifest",
            operator_evidence.ssm_manifest_path.as_str(),
            "redacted-ssm-manifest",
        ),
        (
            "strategy-input",
            operator_evidence.strategy_input_evidence_path.as_str(),
            "strategy-input-evidence",
        ),
        (
            "financial-envelope",
            operator_evidence.financial_envelope_path.as_str(),
            "financial-envelope",
        ),
        (
            "pre-run-state",
            operator_evidence.pre_run_state_path.as_str(),
            "pre-run-state",
        ),
        (
            "abort-plan",
            operator_evidence.abort_plan_path.as_str(),
            "abort-plan",
        ),
        (
            "approval-nonce",
            operator_evidence.approval_nonce_path.as_str(),
            "approval-nonce",
        ),
    ];
    let refs: Vec<_> = bindings
        .iter()
        .map(|(name, path, marker)| {
            let path = std::path::Path::new(path);
            assert!(
                path.starts_with(dir),
                "test artifact path should stay under temp dir"
            );
            let bytes = serde_json::to_vec(&serde_json::json!({
                "record_kind": marker,
                "secret_sentinel": "not-present",
            }))
            .expect("artifact should serialize");
            std::fs::write(path, &bytes).expect("artifact should write");
            serde_json::json!({
                "name": name,
                "path": path.to_string_lossy(),
                "sha256": sha256_bytes(&bytes),
            })
        })
        .collect();
    for artifact in &refs {
        let sha256 = artifact["sha256"]
            .as_str()
            .expect("test artifact sha should be string")
            .to_string();
        match artifact["name"]
            .as_str()
            .expect("test artifact name should be string")
        {
            "ssm-manifest" => operator_evidence.ssm_manifest_sha256 = sha256,
            "strategy-input" => operator_evidence.strategy_input_evidence_sha256 = sha256,
            "financial-envelope" => operator_evidence.financial_envelope_sha256 = sha256,
            "pre-run-state" => operator_evidence.pre_run_state_sha256 = sha256,
            "abort-plan" => operator_evidence.abort_plan_sha256 = sha256,
            "approval-nonce" => operator_evidence.approval_nonce_sha256 = sha256,
            other => panic!("unexpected artifact ref {other}"),
        }
    }
    refs
}

fn write_static_artifacts_manifest_for_test(
    manifest_path: &std::path::Path,
    config_bundle_checksum: &str,
    generated_artifacts: Vec<serde_json::Value>,
    blockers: Vec<&str>,
) {
    let manifest = serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.static_operator_artifacts_manifest.v1",
        "config_bundle_checksum": config_bundle_checksum,
        "generated_artifacts": generated_artifacts,
        "blockers": blockers,
    });
    std::fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest should serialize"),
    )
    .expect("manifest should write");
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_file(path: &std::path::Path) -> String {
    sha256_bytes(&std::fs::read(path).expect("sha256 fixture input should read"))
}

fn current_fixture_nt_revision() -> String {
    let cargo_toml =
        std::fs::read_to_string(repo_path("Cargo.toml")).expect("repo Cargo.toml should read");
    let marker = "rev = \"";
    let revision = cargo_toml
        .split(marker)
        .nth(1)
        .and_then(|value| value.split('"').next())
        .expect("repo Cargo.toml should pin NT git revision");
    revision.to_string()
}

fn write_release_manifest_source_files(
    dir: &std::path::Path,
    cargo_toml: impl AsRef<str>,
    cargo_lock: impl AsRef<str>,
    eip712: impl AsRef<str>,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let cargo_toml_path = dir.join("Cargo.toml");
    let cargo_lock_path = dir.join("Cargo.lock");
    let eip712_path = dir.join("eip712.rs");
    std::fs::write(&cargo_toml_path, cargo_toml.as_ref()).expect("Cargo.toml fixture should write");
    std::fs::write(&cargo_lock_path, cargo_lock.as_ref()).expect("Cargo.lock fixture should write");
    std::fs::write(&eip712_path, eip712.as_ref()).expect("eip712 fixture should write");
    (cargo_toml_path, cargo_lock_path, eip712_path)
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
