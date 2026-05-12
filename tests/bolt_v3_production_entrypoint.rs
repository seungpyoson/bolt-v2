//! Source fence for the production binary entrypoint.
//!
//! Phase 2 requires `src/main.rs` to enter NT through the bolt-v3
//! `run_bolt_v3_live_node` wrapper, not through a direct production
//! `LiveNode::run` call. This is a best-effort textual guard; it is not
//! a compile-time proof.

#[test]
fn main_uses_bolt_v3_runner_wrapper_only() {
    let source = include_str!("../src/main.rs");

    assert!(
        source.contains("run_bolt_v3_live_node"),
        "production entrypoint must call the bolt-v3 gated runner wrapper"
    );

    for forbidden in ["node.run()", "LiveNode::run("] {
        assert!(
            !source.contains(forbidden),
            "production entrypoint must not call NT runner directly via `{forbidden}`"
        );
    }
}
