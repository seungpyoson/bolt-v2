mod support;

use bolt_v2::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
    bolt_v3_no_submit_readiness_schema::{
        STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
    },
};

#[tokio::test(flavor = "current_thread")]
async fn no_submit_readiness_schema_matches_live_canary_gate_contract() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let report_path = tempdir.path().join("no-submit-readiness.json");
    let report = serde_json::json!({
        STAGES_KEY: [
            { STAGE_KEY: "controlled_connect", STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: "reference_readiness", STATUS_KEY: STATUS_SATISFIED },
            { STAGE_KEY: "controlled_disconnect", STATUS_KEY: STATUS_SATISFIED }
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
        },
    );

    check_bolt_v3_live_canary_gate(&loaded)
        .await
        .expect("producer schema should satisfy live canary gate");
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
