mod support;

use std::sync::Arc;

use serde_json::json;

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3BuiltLiveNode, make_bolt_v3_live_node_builder},
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness_on_built_node,
    bolt_v3_no_submit_readiness_schema::{
        SATISFIED_STATUS, STAGE_CONTROLLED_CONNECT, STAGE_CONTROLLED_DISCONNECT, STAGE_KEY,
        STAGES_KEY, STATUS_KEY,
    },
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_data_subscriptions, clear_mock_exec_submissions, recorded_mock_data_subscriptions,
    recorded_mock_exec_submissions,
};

#[test]
fn no_submit_readiness_schema_matches_live_canary_gate_contract() {
    let report = json!({
        STAGES_KEY: [
            {
                STAGE_KEY: "connect",
                STATUS_KEY: SATISFIED_STATUS,
            },
            {
                STAGE_KEY: "disconnect",
                STATUS_KEY: SATISFIED_STATUS,
            },
        ],
    });

    assert_eq!(report[STAGES_KEY][0][STAGE_KEY], "connect");
    assert_eq!(report[STAGES_KEY][1][STAGE_KEY], "disconnect");
    assert_eq!(report[STAGES_KEY][0][STATUS_KEY], "satisfied");
}

#[test]
fn no_submit_readiness_local_runner_writes_satisfied_connect_disconnect_report() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.nautilus.timeout_connection_seconds = 30;
    loaded.root.nautilus.timeout_disconnection_seconds = 10;
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();
    let mut built = mock_built_live_node(&loaded);

    let report = run_bolt_v3_no_submit_readiness_on_built_node(&mut built, &loaded)
        .expect("local no-submit readiness should complete against mock NT clients");

    assert!(
        report
            .stage_status(STAGE_CONTROLLED_CONNECT)
            .iter()
            .any(|status| status.as_str() == SATISFIED_STATUS)
    );
    assert!(
        report
            .stage_status(STAGE_CONTROLLED_DISCONNECT)
            .iter()
            .any(|status| status.as_str() == SATISFIED_STATUS)
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

fn mock_built_live_node(loaded: &LoadedBoltV3Config) -> BoltV3BuiltLiveNode {
    let builder =
        make_bolt_v3_live_node_builder(loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("MOCK_DATA", "MOCKVENUE")),
        )
        .expect("mock data client should register on bolt-v3 builder");
    let builder = builder
        .add_exec_client(
            Some("MOCK_EXEC".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(MockExecClientConfig::new(
                "MOCK_EXEC",
                "MOCK-ACCOUNT",
                "MOCKVENUE",
            )),
        )
        .expect("mock exec client should register on bolt-v3 builder");
    BoltV3BuiltLiveNode::new(
        builder.build().expect("LiveNode should build with mocks"),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed()),
    )
}
