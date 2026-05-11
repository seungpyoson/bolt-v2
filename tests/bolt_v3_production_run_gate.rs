mod support;

#[test]
fn production_run_wrapper_no_longer_checks_start_readiness_before_nt_run() {
    let source = std::fs::read_to_string(support::repo_path("src/bolt_v3_live_node.rs"))
        .expect("bolt_v3_live_node.rs should read");
    let wrapper = source
        .split("pub async fn run_bolt_v3_live_node")
        .nth(1)
        .expect("production run wrapper should exist")
        .split("#[cfg(test)]")
        .next()
        .expect("production run wrapper body should be before module tests");

    assert!(
        !wrapper.contains("require_bolt_v3_start_readiness_gate"),
        "production run wrapper must not require selected-market instruments before NT run; \
         first boot lets NT data clients populate instruments during startup"
    );
    assert!(
        wrapper.contains(".run().await"),
        "production run wrapper must remain the only explicit v3 NT run boundary"
    );
}

#[test]
fn production_run_error_type_no_longer_exposes_start_readiness_failure() {
    let source = std::fs::read_to_string(support::repo_path("src/bolt_v3_live_node.rs"))
        .expect("bolt_v3_live_node.rs should read");
    let error_type = source
        .split("pub enum BoltV3LiveNodeError")
        .nth(1)
        .expect("live-node error type should exist")
        .split("impl std::fmt::Display for BoltV3LiveNodeError")
        .next()
        .expect("error type body should precede Display impl");

    assert!(
        !error_type.contains("StartReadiness"),
        "run-wrapper error type must not expose stale pre-run instrument-readiness failure"
    );
}
