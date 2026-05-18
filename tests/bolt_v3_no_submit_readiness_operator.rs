use std::{env, path::PathBuf, process::Command};

use bolt_v2::bolt_v3_config::load_bolt_v3_config;

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

    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "no-submit-readiness",
            "--config",
            root_path
                .to_str()
                .expect("operator root TOML path must be UTF-8"),
        ])
        .output()
        .expect("failed to run bolt-v2 no-submit-readiness");
    assert!(
        output.status.success(),
        "bolt-v2 no-submit-readiness failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
