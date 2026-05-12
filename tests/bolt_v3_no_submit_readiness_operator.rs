use std::{env, path::PathBuf};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
    bolt_v3_no_submit_readiness::run_bolt_v3_no_submit_readiness,
};

#[test]
#[ignore = "requires explicit operator approval, real SSM, and real venue connectivity"]
fn operator_approved_real_no_submit_readiness_writes_redacted_report() {
    let root_path = PathBuf::from(
        env::var("BOLT_V3_ROOT_TOML").expect("BOLT_V3_ROOT_TOML must point to approved TOML"),
    );
    let approval_id = env::var("BOLT_V3_OPERATOR_APPROVAL_ID")
        .expect("BOLT_V3_OPERATOR_APPROVAL_ID must be set by the operator");
    let loaded = load_bolt_v3_config(&root_path).expect("operator TOML should load");

    let report = run_bolt_v3_no_submit_readiness(&loaded, &approval_id)
        .expect("real no-submit readiness should complete");
    let report_path = report
        .write_redacted_json_for_loaded_config(&loaded)
        .expect("redacted readiness report should be written to configured path");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("operator gate verification runtime should build")
        .block_on(check_bolt_v3_live_canary_gate(&loaded))
        .expect("live canary gate should accept the redacted readiness report");

    println!(
        "bolt-v3 no-submit readiness report written to {}",
        report_path.display()
    );
}
