mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_canary_gate::{BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_gate},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with, run_bolt_v3_live_node},
};
use tokio::task::LocalSet;

#[test]
fn run_bolt_v3_live_node_rejects_missing_live_canary_before_nt_run() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_without_live_canary(loaded);
    let mut node = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let local = LocalSet::new();

    let error = runtime.block_on(local.run_until(async {
        run_bolt_v3_live_node(&mut node, &loaded)
            .await
            .expect_err("missing live_canary block must fail before NT run")
    }));

    assert!(
        matches!(
            error,
            BoltV3LiveNodeError::LiveCanaryGate(BoltV3LiveCanaryGateError::MissingConfig)
        ),
        "expected missing live canary gate error, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_approval_id() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "  ".to_string(),
            no_submit_readiness_report_path: "not-read-before-approval-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty approval_id must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::MissingApprovalId),
        "expected missing approval rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_readiness_report_path() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "  ".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty no-submit readiness report path must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::MissingReadinessReportPath),
        "expected missing readiness report path rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_order_count() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-order-count-check.json".to_string(),
            max_live_order_count: 0,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero max_live_order_count must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidMaxLiveOrderCount { value: 0 }
        ),
        "expected order-count rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_zero_report_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-size-limit-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 0,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("zero readiness report byte cap must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::InvalidReadinessReportSizeLimit { value: 0 }
        ),
        "expected readiness report byte-cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_invalid_canary_notional_values() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");

    for candidate in ["abc", "0.00", "-1.00"] {
        let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: "not-read-before-notional-check.json".to_string(),
                max_live_order_count: 1,
                max_notional_per_order: candidate.to_string(),
                max_no_submit_readiness_report_bytes: 4096,
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("invalid canary notional must fail closed");

        match error {
            BoltV3LiveCanaryGateError::InvalidMaxNotional { field, value, .. } => {
                assert_eq!(field, "max_notional_per_order");
                assert_eq!(value, candidate);
            }
            other => panic!("expected invalid canary notional rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_invalid_root_notional_values() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");

    for candidate in ["abc", "0.00", "-1.00"] {
        let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
        loaded.root.risk.default_max_notional_per_order = candidate.to_string();
        let loaded = loaded_with_live_canary(
            loaded,
            LiveCanaryBlock {
                approval_id: "operator-approved-canary-001".to_string(),
                no_submit_readiness_report_path: "not-read-before-root-notional-check.json"
                    .to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
            },
        );

        let error = check_bolt_v3_live_canary_gate(&loaded)
            .await
            .expect_err("invalid root notional must fail closed");

        match error {
            BoltV3LiveCanaryGateError::InvalidMaxNotional { field, value, .. } => {
                assert_eq!(field, "risk.default_max_notional_per_order");
                assert_eq!(value, candidate);
            }
            other => panic!("expected invalid root notional rejection, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_satisfied_no_submit_report_with_trimmed_capped_notional() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.risk.default_max_notional_per_order = " 10.00 ".to_string();
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(
        &report_path,
        &[("connect", "satisfied"), ("disconnect", "satisfied")],
    );

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: " 1.00 ".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("satisfied no-submit report and capped notional should pass");

    assert_eq!(report.approval_id, "operator-approved-canary-001");
    assert_eq!(report.max_live_order_count, 1);
    assert_eq!(
        report.no_submit_readiness_report_path, report_path,
        "absolute report path should be preserved"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_notional_equal_to_root_risk_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.risk.default_max_notional_per_order = "10.00".to_string();
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[("connect", "satisfied")]);

    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "10.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("notional equal to root risk cap should pass");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_notional_above_root_risk_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "not-read-before-cap-check.json".to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "11.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("canary notional above root risk cap must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::MaxNotionalExceedsRootRisk { .. }
        ),
        "expected root risk cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_empty_stage_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"{"stages":[]}"#).expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("empty no-submit stage report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons.iter().any(|reason| reason.contains("empty")),
                "error should name the empty stages array, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_report_missing_stages_key() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"{"other":true}"#).expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing stages key must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("stages array is missing")),
                "error should name the missing stages array, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_unsatisfied_no_submit_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(
        &report_path,
        &[("connect", "satisfied"), ("disconnect", "blocked")],
    );
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("unsatisfied no-submit report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons.iter().any(|reason| reason.contains("disconnect")),
                "error should name the blocked stage, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_missing_no_submit_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("missing-no-submit-readiness.json");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("missing no-submit readiness report must fail closed");

    assert!(
        matches!(error, BoltV3LiveCanaryGateError::ReadinessReportRead { .. }),
        "expected read rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_malformed_no_submit_report_json() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"{"stages":["#).expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("malformed no-submit readiness report must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::ReadinessReportParse { .. }
        ),
        "expected parse rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_report_exactly_at_configured_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report(&report_path, &[("connect", "satisfied")]);
    let report_len = std::fs::metadata(&report_path)
        .expect("report metadata should be readable")
        .len();
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: report_len,
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("report exactly at configured byte cap should pass");
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_rejects_no_submit_report_above_configured_byte_cap() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"{"stages":[]}"#).expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 1,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("oversized no-submit readiness report must fail closed");

    assert!(
        matches!(
            error,
            BoltV3LiveCanaryGateError::ReadinessReportTooLarge { .. }
        ),
        "expected size-cap rejection, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_distinguishes_non_object_report_from_missing_stages() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"["satisfied"]"#).expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("non-object no-submit readiness report must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("expected JSON object")),
                "error should name the malformed report object, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_distinguishes_non_array_stages_from_missing_stages() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    std::fs::write(&report_path, r#"{"stages":"satisfied"}"#)
        .expect("report fixture should be written");
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("non-array stages field must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("stages must be an array")),
                "error should name the malformed stages field, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_reports_unsatisfied_stage_name_fallback() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_with_stage_field(&report_path, "name", &[("disconnect", "failed")]);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect_err("unsatisfied report with name fallback must fail closed");

    match error {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { reasons, .. } => {
            assert!(
                reasons.iter().any(|reason| reason.contains("disconnect")),
                "error should name the blocked stage from name fallback, got {reasons:?}"
            );
        }
        other => panic!("expected unsatisfied report rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_canary_gate_accepts_case_insensitive_satisfied_status() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    write_no_submit_report_with_stage_field(&report_path, "stage", &[("connect", "SATISFIED")]);
    let loaded = loaded_with_live_canary(
        loaded,
        LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
        },
    );

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("uppercase satisfied status should pass");

    assert_eq!(report.approval_id, "operator-approved-canary-001");
}

fn loaded_with_live_canary(
    loaded: LoadedBoltV3Config,
    live_canary: LiveCanaryBlock,
) -> LoadedBoltV3Config {
    let mut root = loaded.root;
    root.live_canary = Some(live_canary);
    LoadedBoltV3Config { root, ..loaded }
}

fn loaded_without_live_canary(loaded: LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut root = loaded.root;
    root.live_canary = None;
    LoadedBoltV3Config { root, ..loaded }
}

fn write_no_submit_report(path: &std::path::Path, stages: &[(&str, &str)]) {
    write_no_submit_report_with_stage_field(path, "stage", stages);
}

fn write_no_submit_report_with_stage_field(
    path: &std::path::Path,
    stage_field: &str,
    stages: &[(&str, &str)],
) {
    let stages: Vec<_> = stages
        .iter()
        .map(|(stage, status)| serde_json::json!({ stage_field: stage, "status": status }))
        .collect();
    let report = serde_json::json!({ "stages": stages });
    std::fs::write(
        path,
        serde_json::to_string_pretty(&report).expect("report should serialize"),
    )
    .expect("report fixture should be written");
}
