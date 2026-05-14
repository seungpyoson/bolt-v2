use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::Phase8OperatorApprovalEnvelope,
};

#[test]
fn phase8_operator_harness_is_ignored_and_uses_production_runner_shape() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("Phase8OperatorApprovalEnvelope::from_env"));
    assert!(source.contains("build_bolt_v3_live_node"));
    assert!(source.contains("run_bolt_v3_live_node"));
    assert!(source.contains("tokio::task::LocalSet"));
    assert!(!source.contains(&format!("{}{}", "LiveNode", "::run")));
    assert!(!source.contains(&format!("{}{}", ".submit", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".cancel", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".replace", "_order(")));
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn phase8_operator_harness_requires_exact_approval_before_live_runner() -> anyhow::Result<()>
{
    let envelope = Phase8OperatorApprovalEnvelope::from_env()?;
    let loaded = load_bolt_v3_config(std::path::Path::new(&envelope.root_toml_path))?;
    let root_hash = Phase8OperatorApprovalEnvelope::sha256_file(&envelope.root_toml_path)?;
    let current_head = std::env::var("BOLT_V3_PHASE8_CURRENT_HEAD_SHA")?;
    envelope.validate_against(
        &current_head,
        &root_hash,
        loaded
            .root
            .live_canary
            .as_ref()
            .map(|block| block.approval_id.as_str())
            .unwrap_or_default(),
    )?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut node = build_bolt_v3_live_node(&loaded)?;
            run_bolt_v3_live_node(&mut node, &loaded).await
        })
        .await?;
    Ok(())
}
