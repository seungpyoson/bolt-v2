use std::{env, path::PathBuf, process::Command};

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
    let loaded = load_bolt_v3_config(&root_path).expect("operator root TOML should load");
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .expect("operator root TOML must define [live_canary]");
    assert!(
        !live_canary.approval_id.trim().is_empty(),
        "operator root TOML must define live_canary.approval_id"
    );
    let approval_id = live_canary.approval_id.trim();
    let head_sha = no_submit_readiness_current_checkout_head_sha();

    let report = run_bolt_v3_no_submit_readiness(&loaded, approval_id, &head_sha)
        .expect("operator-approved readiness should complete");
    report
        .write_configured_redacted_json(&loaded)
        .expect("redacted readiness report should be written");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("operator live-canary gate runtime should build");
    runtime
        .block_on(async { check_bolt_v3_live_canary_gate(&loaded).await })
        .expect("live canary gate should accept redacted readiness report");
}

fn no_submit_readiness_current_checkout_head_sha() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run git rev-parse HEAD");
    assert!(
        output.status.success(),
        "git rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let head = String::from_utf8(output.stdout).expect("git HEAD output should be UTF-8");
    let head = head.trim();
    assert!(
        !head.is_empty(),
        "git rev-parse HEAD returned an empty head"
    );
    head.to_string()
}
