use sha2::{Digest, Sha256};

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, load_bolt_v3_config},
    bolt_v3_market_families::updown::updown_market_slug,
    bolt_v3_operator_artifacts::build_redacted_ssm_manifest,
    bolt_v3_tiny_canary_evidence::{Phase8OperatorApprovalEnvelope, Phase8PreRunStateSourceProofs},
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
fn redacted_ssm_manifest_hashes_configured_ssm_paths_without_values() {
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
    }

    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "private_key_ssm_path",
        "/bolt/polymarket_main/private_key",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_key_ssm_path",
        "/bolt/polymarket_main/api_key",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "api_secret_ssm_path",
        "/bolt/polymarket_main/api_secret",
    );
    assert_manifest_entry(
        &manifest,
        "polymarket_main",
        "POLYMARKET",
        "passphrase_ssm_path",
        "/bolt/polymarket_main/passphrase",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_key_ssm_path",
        "/bolt/binance_reference/api_key",
    );
    assert_manifest_entry(
        &manifest,
        "binance_reference",
        "BINANCE",
        "api_secret_ssm_path",
        "/bolt/binance_reference/api_secret",
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
fn market_selection_source_writer_uses_family_dispatch_not_updown_directly() {
    let source = std::fs::read_to_string(repo_path("src/bolt_v3_operator_artifacts.rs"))
        .expect("operator artifacts source should read");

    assert!(
        !source.contains("updown::"),
        "operator artifacts must use market-family dispatch instead of direct updown calls"
    );
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
    ssm_path: &str,
) {
    let entry = manifest
        .entries
        .iter()
        .find(|entry| {
            entry.client_key == client_key
                && entry.provider_key == provider_key
                && entry.field_name == field_name
        })
        .expect("expected redacted SSM manifest entry");
    assert_eq!(entry.ssm_path_sha256, sha256_text(ssm_path));
}

fn sha256_text(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
