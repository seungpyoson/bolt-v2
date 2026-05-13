use std::{env, path::PathBuf};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
};

#[test]
#[ignore = "requires explicit operator approval, real SSM, real venue connectivity, and NT cache reference proof"]
fn operator_approved_real_no_submit_readiness_writes_redacted_report() {
    let root_path = PathBuf::from(
        env::var("BOLT_V3_ROOT_TOML").expect("BOLT_V3_ROOT_TOML must be set by operator"),
    );
    let approval_id = env::var("BOLT_V3_OPERATOR_APPROVAL_ID")
        .expect("BOLT_V3_OPERATOR_APPROVAL_ID must be set by operator");
    let loaded = load_bolt_v3_config(&root_path).expect("operator root TOML should load");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("operator readiness runtime should build");
    let report = runtime
        .block_on(run_bolt_v3_no_submit_readiness(&loaded, &approval_id))
        .expect("operator-approved readiness should complete");
    report
        .write_configured_redacted_json(&loaded)
        .expect("redacted readiness report should be written");
    runtime
        .block_on(check_bolt_v3_live_canary_gate(&loaded))
        .expect("live canary gate should accept redacted readiness report");
}
