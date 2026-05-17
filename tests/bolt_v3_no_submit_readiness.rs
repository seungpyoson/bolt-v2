mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
    bolt_v3_no_submit_readiness::{
        BoltV3NoSubmitReadinessError, BoltV3NoSubmitReadinessReportMetadata,
        BoltV3NoSubmitReadinessStatus, reference_readiness_from_cached_instrument_ids,
        run_bolt_v3_no_submit_readiness, run_bolt_v3_no_submit_readiness_from_stage_results,
        run_bolt_v3_no_submit_readiness_on_runtime,
    },
    bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, LIVE_NODE_BUILD_STAGE,
        NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE,
        REPORT_WRITE_STAGE, SECRET_RESOLUTION_STAGE, STAGE_KEY, STAGES_KEY, STATUS_KEY,
        STATUS_SATISFIED,
    },
};
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "current_thread")]
async fn no_submit_readiness_schema_matches_live_canary_gate_contract() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let report = serde_json::json!({
        STAGES_KEY: [
            { STAGE_KEY: OPERATOR_APPROVAL_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: SECRET_RESOLUTION_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: LIVE_NODE_BUILD_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: CONTROLLED_CONNECT_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: REFERENCE_READINESS_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: CONTROLLED_DISCONNECT_STAGE, STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: REPORT_WRITE_STAGE, STATUS_KEY: STATUS_SATISFIED }
        ]
    });
    std::fs::write(
        &report_path,
        serde_json::to_vec(&report).expect("report should serialize"),
    )
    .expect("report should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            operator_evidence: None,
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("producer schema should satisfy live canary gate");
}

#[test]
fn no_submit_readiness_local_runner_writes_satisfied_connect_reference_disconnect_report() {
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Ok(()),
        Ok(()),
        Ok(()),
        &["secret-value".to_string()],
    );

    assert_eq!(
        report.stage_status("controlled_connect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
    assert_eq!(
        report.stage_status("reference_readiness"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
    assert_eq!(
        report.stage_status("controlled_disconnect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_submit_readiness_report_records_authenticated_fields_and_required_stages() {
    let loaded = loaded_with_test_live_canary();
    let head_sha = "a526e1886f1877fcce0e5c7f667c45375c1709a4";
    let metadata = BoltV3NoSubmitReadinessReportMetadata::from_loaded(
        &loaded,
        "operator-approved-canary-001",
        head_sha,
    )
    .await
    .expect("report metadata should be derived from loaded config");

    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        metadata,
        Ok(()),
        Ok(()),
        Ok(()),
        &["secret-value".to_string()],
    );
    let value = serde_json::to_value(&report).expect("report should serialize");
    let approval_id_hash = sha256_hex("operator-approved-canary-001");
    let config_checksum = sha256_hex(
        &std::fs::read_to_string(&loaded.root_path).expect("fixture root TOML should be readable"),
    );
    let expected_report_path = loaded
        .root_path
        .parent()
        .expect("fixture root path should have parent")
        .join("not-written-before-approval-check.json");

    assert_eq!(value["schema_version"], NO_SUBMIT_READINESS_SCHEMA_VERSION);
    assert_eq!(value["approval_id_hash"], approval_id_hash);
    assert_ne!(value["approval_id_hash"], "operator-approved-canary-001");
    assert_eq!(value["head_sha"], head_sha);
    assert_eq!(value["config_checksum"], config_checksum);
    assert_eq!(
        value["report_path"],
        expected_report_path.to_string_lossy().as_ref()
    );
    for required_stage in [
        OPERATOR_APPROVAL_STAGE,
        SECRET_RESOLUTION_STAGE,
        LIVE_NODE_BUILD_STAGE,
        CONTROLLED_CONNECT_STAGE,
        REFERENCE_READINESS_STAGE,
        CONTROLLED_DISCONNECT_STAGE,
        REPORT_WRITE_STAGE,
    ] {
        assert_eq!(
            report.stage_status(required_stage),
            vec![BoltV3NoSubmitReadinessStatus::Satisfied],
            "required readiness stage `{required_stage}` should be satisfied"
        );
    }
    let debug = format!("{report:#?}");
    let json = serde_json::to_string_pretty(&report).expect("report should serialize");
    assert!(!debug.contains("operator-approved-canary-001"));
    assert!(!json.contains("operator-approved-canary-001"));
}

#[test]
fn no_submit_readiness_report_does_not_contain_resolved_secret_values() {
    let secret = "0x4242424242424242424242424242424242424242424242424242424242424242";
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Err(format!("connect rejected key {secret}")),
        Ok(()),
        Err(format!("disconnect rejected key {secret}")),
        &[secret.to_string()],
    );
    let debug = format!("{report:#?}");
    let json = serde_json::to_string_pretty(&report).expect("report should serialize");

    assert!(!debug.contains(secret), "debug report leaked secret value");
    assert!(!json.contains(secret), "json report leaked secret value");
    assert!(
        json.contains("[redacted]"),
        "json should show redaction marker"
    );
}

#[test]
fn no_submit_readiness_redacts_longest_overlapping_secret_values_first() {
    let short_secret = "phase7-secret";
    let long_secret = "phase7-secret-only-long-part";
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Err(format!("connect rejected key {long_secret}")),
        Ok(()),
        Ok(()),
        &[short_secret.to_string(), long_secret.to_string()],
    );
    let json = serde_json::to_string_pretty(&report).expect("report should serialize");

    assert!(
        !json.contains(short_secret),
        "json leaked short secret value"
    );
    assert!(!json.contains(long_secret), "json leaked long secret value");
    assert!(
        !json.contains("only-long-part"),
        "json leaked the long-only suffix of an overlapping secret value"
    );
}

#[test]
fn no_submit_readiness_redaction_marker_survives_secret_values_inside_marker() {
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Err("connect rejected very-secret".to_string()),
        Ok(()),
        Ok(()),
        &["very-secret".to_string(), "redact".to_string()],
    );
    let detail = report
        .stages
        .iter()
        .find(|stage| stage.stage == CONTROLLED_CONNECT_STAGE)
        .and_then(|stage| stage.detail.as_deref())
        .expect("failed connect stage should record redacted detail");

    assert_eq!(detail, "connect rejected [redacted]");
}

#[test]
fn no_submit_readiness_records_failed_connect_reference_skip_and_disconnect_failure() {
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Err("simulated connect failure".to_string()),
        Ok(()),
        Err("simulated disconnect failure".to_string()),
        &[],
    );

    assert_eq!(
        report.stage_status("controlled_connect"),
        vec![BoltV3NoSubmitReadinessStatus::Failed]
    );
    assert_eq!(
        report.stage_status("reference_readiness"),
        vec![BoltV3NoSubmitReadinessStatus::Skipped]
    );
    assert_eq!(
        report.stage_status("controlled_disconnect"),
        vec![BoltV3NoSubmitReadinessStatus::Failed]
    );
}

#[test]
fn no_submit_readiness_fails_when_required_reference_instrument_missing_from_cache() {
    let loaded = loaded_with_test_live_canary();
    let reference_readiness =
        reference_readiness_from_cached_instrument_ids(&loaded, std::iter::empty::<&str>());
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Ok(()),
        reference_readiness,
        Ok(()),
        &[],
    );

    assert_eq!(
        report.stage_status("controlled_connect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
    assert_eq!(
        report.stage_status("reference_readiness"),
        vec![BoltV3NoSubmitReadinessStatus::Failed]
    );
    assert_eq!(
        report.stage_status("controlled_disconnect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
}

#[test]
fn no_submit_readiness_satisfies_reference_when_required_instruments_are_cached() {
    let loaded = loaded_with_test_live_canary();
    let cached_instrument_ids = loaded
        .strategies
        .iter()
        .flat_map(|strategy| strategy.config.reference_data.values())
        .map(|reference| reference.instrument_id.as_str())
        .collect::<Vec<_>>();
    assert!(
        !cached_instrument_ids.is_empty(),
        "fixture must carry required reference instruments for the success case"
    );
    let reference_readiness =
        reference_readiness_from_cached_instrument_ids(&loaded, cached_instrument_ids);
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Ok(()),
        reference_readiness,
        Ok(()),
        &[],
    );

    assert_eq!(
        report.stage_status("controlled_connect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
    assert_eq!(
        report.stage_status("reference_readiness"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
    assert_eq!(
        report.stage_status("controlled_disconnect"),
        vec![BoltV3NoSubmitReadinessStatus::Satisfied]
    );
}

#[test]
fn no_submit_readiness_writer_enforces_configured_byte_cap() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("readiness").join("report.json");
    let report = run_bolt_v3_no_submit_readiness_from_stage_results(
        test_report_metadata(),
        Ok(()),
        Ok(()),
        Ok(()),
        &[],
    );

    let error = report
        .write_redacted_json_with_max_bytes(&report_path, 1_u64)
        .expect_err("oversized report must fail closed");

    let BoltV3NoSubmitReadinessError::ReportTooLarge {
        path,
        length,
        max_length,
    } = error
    else {
        panic!("expected report byte-cap error, got {error:?}");
    };
    assert_eq!(path, report_path);
    assert!(length > 1_u64, "oversized report length must be recorded");
    assert_eq!(max_length, 1_u64);
    assert!(
        !report_path.exists(),
        "oversized report must not be written to disk"
    );
}

#[test]
fn no_submit_readiness_rejects_empty_configured_operator_approval_before_build() {
    let loaded = loaded_with_live_canary(
        loaded_with_test_live_canary(),
        LiveCanaryBlock {
            approval_id: "   ".to_string(),
            no_submit_readiness_report_path: "not-written-before-approval-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            operator_evidence: None,
        },
    );

    let error = run_bolt_v3_no_submit_readiness(
        &loaded,
        "operator-approved-canary-001",
        "a526e1886f1877fcce0e5c7f667c45375c1709a4",
    )
    .expect_err("missing configured approval must fail before runtime build");

    assert!(
        matches!(
            error,
            BoltV3NoSubmitReadinessError::MissingOperatorApprovalId
        ),
        "expected missing approval error, got {error:?}"
    );
}

#[test]
fn no_submit_readiness_rejects_empty_operator_approval_before_build() {
    let loaded = loaded_with_test_live_canary();

    let error =
        run_bolt_v3_no_submit_readiness(&loaded, "   ", "a526e1886f1877fcce0e5c7f667c45375c1709a4")
            .expect_err("missing approval must fail before runtime build");

    assert!(
        matches!(
            error,
            BoltV3NoSubmitReadinessError::MissingOperatorApprovalId
        ),
        "expected missing approval error, got {error:?}"
    );
}

#[test]
fn no_submit_readiness_rejects_operator_approval_mismatch_before_build() {
    let loaded = loaded_with_test_live_canary();

    let error = run_bolt_v3_no_submit_readiness(
        &loaded,
        "different-approval",
        "a526e1886f1877fcce0e5c7f667c45375c1709a4",
    )
    .expect_err("approval mismatch must fail before runtime build");

    assert!(
        matches!(
            error,
            BoltV3NoSubmitReadinessError::OperatorApprovalIdMismatch
        ),
        "expected approval mismatch error, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_submit_readiness_rejects_sync_runner_inside_active_tokio_runtime() {
    let loaded = loaded_with_test_live_canary();

    let error = run_bolt_v3_no_submit_readiness(
        &loaded,
        "operator-approved-canary-001",
        "a526e1886f1877fcce0e5c7f667c45375c1709a4",
    )
    .expect_err("sync no-submit runner must reject active Tokio runtime before SSM build");

    assert!(
        matches!(error, BoltV3NoSubmitReadinessError::ActiveTokioRuntime),
        "expected active runtime boundary error, got {error:?}"
    );
}

#[test]
fn no_submit_readiness_exposes_current_runtime_wrapper_without_node_mut() {
    let _wrapper = run_bolt_v3_no_submit_readiness_on_runtime;
    let live_node_source =
        std::fs::read_to_string("src/bolt_v3_live_node.rs").expect("live node source should exist");

    assert!(
        live_node_source.contains("controlled_no_submit_readiness"),
        "live node should expose a narrow no-submit readiness boundary"
    );
    assert!(
        !live_node_source.contains("pub fn node_mut"),
        "Phase 7 must not expose a broad mutable LiveNode escape hatch"
    );
}

#[test]
fn no_submit_readiness_runtime_source_does_not_treat_connect_as_reference_readiness() {
    let source = std::fs::read_to_string("src/bolt_v3_no_submit_readiness.rs")
        .expect("no-submit readiness source should exist");

    assert!(
        source.contains("reference_readiness_from_cached_instrument_ids"),
        "runtime path must use required reference instruments from NT cache"
    );
    assert!(
        !source.contains("current_main_reference_readiness"),
        "runtime path must not keep the current-main fail-closed placeholder"
    );
    assert!(
        !source.contains("let reference = if connect.is_ok() {\n        Ok(())"),
        "connect success alone must not satisfy reference readiness"
    );
}

#[test]
fn no_submit_readiness_runtime_uses_resolved_secret_redaction_values() {
    let source = std::fs::read_to_string("src/bolt_v3_no_submit_readiness.rs")
        .expect("no-submit readiness source should exist");

    assert!(
        source.contains("runtime.redaction_values()"),
        "runtime path must redact controlled readiness details using resolved secret values"
    );
    assert!(
        !source.contains("run_bolt_v3_no_submit_readiness_on_runtime(&mut runtime, loaded, &[])"),
        "runtime path must not disable redaction with an empty redaction list"
    );
}

#[test]
fn no_submit_readiness_metadata_checksum_uses_async_file_io() {
    let source = std::fs::read_to_string("src/bolt_v3_no_submit_readiness.rs")
        .expect("no-submit readiness source should exist");

    assert!(
        source.contains("tokio::fs::read(&loaded.root_path).await"),
        "metadata checksum must not block the current-thread async readiness path"
    );
    assert!(
        !source.contains("std::fs::read(&loaded.root_path)"),
        "metadata checksum must not use blocking file I/O inside the async readiness path"
    );
}

#[test]
fn no_submit_readiness_sync_runner_uses_localset_after_build() {
    let source = std::fs::read_to_string("src/bolt_v3_no_submit_readiness.rs")
        .expect("no-submit readiness source should exist");

    assert!(
        source.contains("tokio::task::LocalSet::new()"),
        "sync no-submit runner must create a LocalSet for NT local tasks"
    );
    assert!(
        source.contains(".run_until("),
        "sync no-submit runner must enter the readiness future through LocalSet::run_until"
    );
    let build_pos = source
        .find("build_bolt_v3_live_node(loaded)")
        .expect("sync runner must build the live node");
    let localset_pos = source
        .find("tokio::task::LocalSet::new()")
        .expect("sync runner must create a LocalSet");
    assert!(
        build_pos < localset_pos,
        "SSM-backed live-node build must happen before entering the readiness Tokio runtime"
    );
}

#[test]
fn no_submit_readiness_operator_approval_is_config_owned_not_env_owned() {
    let source = std::fs::read_to_string("tests/bolt_v3_no_submit_readiness_operator.rs")
        .expect("operator harness source should exist");

    for forbidden in [
        concat!("BOLT_V3_", "OPERATOR_APPROVAL_ID"),
        concat!("BOLT_V3_", "HEAD_SHA"),
    ] {
        assert!(
            !source.contains(forbidden),
            "operator no-submit approval/head evidence must not be supplied through env var `{forbidden}`"
        );
    }
    assert!(
        source.contains("live_canary.approval_id"),
        "operator no-submit approval must be read from loaded TOML"
    );
    assert!(
        source.contains("run_bolt_v3_no_submit_readiness(&loaded, approval_id, &head_sha)"),
        "operator no-submit harness must pass an explicit approval id through the readiness boundary"
    );
    assert!(
        source.contains("no_submit_readiness_current_checkout_head_sha"),
        "operator no-submit head evidence must be derived from current checkout"
    );
}

#[test]
fn no_submit_readiness_docs_keep_phase8_live_action_blocked() {
    let quickstart = include_str!("../specs/002-phase7-no-submit-readiness/quickstart.md");

    assert!(quickstart.contains("Phase 8 live action remains blocked"));
    assert!(quickstart.contains("Real no-submit report exists"));
    assert!(quickstart.contains("strategy-input safety audit approves"));
    assert!(quickstart.contains("User explicitly approves exact head and live command"));
    assert!(
        !quickstart.contains("--ignored --nocapture --live"),
        "Phase 7 docs must not publish a live-capital command"
    );
}

fn loaded_with_test_live_canary() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-written-before-approval-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            operator_evidence: None,
        },
    )
}

fn test_report_metadata() -> BoltV3NoSubmitReadinessReportMetadata {
    BoltV3NoSubmitReadinessReportMetadata {
        approval_id_hash: sha256_hex("operator-approved-canary-001"),
        head_sha: "a526e1886f1877fcce0e5c7f667c45375c1709a4".to_string(),
        config_checksum: "test-config-checksum".to_string(),
        report_path: "not-written-before-approval-check.json".to_string(),
    }
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[test]
fn no_submit_readiness_source_has_no_trade_or_runner_tokens() {
    let source = std::fs::read_to_string("src/bolt_v3_no_submit_readiness.rs")
        .expect("no-submit readiness source should exist");
    let operator_source = std::fs::read_to_string("tests/bolt_v3_no_submit_readiness_operator.rs")
        .expect("operator harness source should exist");
    for (path, text) in [
        ("src/bolt_v3_no_submit_readiness.rs", source.as_str()),
        (
            "tests/bolt_v3_no_submit_readiness_operator.rs",
            operator_source.as_str(),
        ),
    ] {
        for forbidden in [
            "submit_order",
            "submit_order_list",
            "cancel_order",
            "cancel_all_orders",
            "replace_order",
            "amend_order",
            "subscribe",
            "run_bolt_v3_live_node",
            ".run(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{path} must not contain trade or runner token `{forbidden}`"
            );
        }
    }
}

fn loaded_with_live_canary(
    loaded: LoadedBoltV3Config,
    live_canary: LiveCanaryBlock,
) -> LoadedBoltV3Config {
    let mut root = loaded.root;
    root.live_canary = Some(live_canary);
    LoadedBoltV3Config { root, ..loaded }
}
