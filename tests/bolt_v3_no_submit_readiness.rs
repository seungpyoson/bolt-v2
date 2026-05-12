use serde_json::json;

use bolt_v2::bolt_v3_no_submit_readiness_schema::{
    SATISFIED_STATUS, STAGE_KEY, STAGES_KEY, STATUS_KEY,
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
