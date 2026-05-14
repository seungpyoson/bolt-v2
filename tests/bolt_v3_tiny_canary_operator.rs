use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::{
        Phase8CanaryBlockReason, Phase8CanaryEvidence, Phase8CanaryEvidenceInput,
        Phase8EvidenceRef, Phase8OperatorApprovalEnvelope, Phase8RuntimeCaptureRef,
        Phase8StrategyInputSafetyAudit, evaluate_phase8_canary_preflight,
    },
};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::process::Command;

#[test]
fn phase8_operator_harness_is_ignored_and_uses_production_runner_shape() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("Phase8OperatorApprovalEnvelope::from_env"));
    assert!(source.contains("evaluate_phase8_canary_preflight"));
    assert!(source.contains("write_json_file"));
    assert!(source.contains("build_bolt_v3_live_node"));
    assert!(source.contains("run_bolt_v3_live_node"));
    assert!(source.contains("tokio::task::LocalSet"));
    assert!(!source.contains(&format!(
        "{}{}{}",
        "BOLT_V3_PHASE8_", "CURRENT_HEAD", "_SHA"
    )));
    assert!(!source.contains(&format!("{}{}", "LiveNode", "::run")));
    assert!(!source.contains(&format!("{}{}", ".submit", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".cancel", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".replace", "_order(")));
}

#[test]
fn phase8_operator_head_is_resolved_from_checkout() -> anyhow::Result<()> {
    let head = phase8_current_checkout_head_sha()?;

    assert_eq!(head.len(), 40);
    assert!(head.chars().all(|byte| byte.is_ascii_hexdigit()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn phase8_operator_harness_requires_exact_approval_before_live_runner() -> anyhow::Result<()>
{
    let envelope = Phase8OperatorApprovalEnvelope::from_env()?;
    let loaded = load_bolt_v3_config(std::path::Path::new(&envelope.root_toml_path))?;
    let root_hash = Phase8OperatorApprovalEnvelope::sha256_file(&envelope.root_toml_path)?;
    let current_head = phase8_current_checkout_head_sha()?;
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
    let preflight = evaluate_phase8_canary_preflight(
        &loaded,
        &current_head,
        Phase8StrategyInputSafetyAudit::approved(),
    )
    .await;
    if !preflight.can_enter_live_runner() {
        let evidence = Phase8CanaryEvidence::blocked_before_submit(
            phase8_operator_evidence_input(&envelope, &loaded, &root_hash)?,
            preflight
                .block_reasons
                .first()
                .cloned()
                .unwrap_or(Phase8CanaryBlockReason::BlockedBeforeLiveOrder),
        );
        evidence.write_json_file(&envelope.canary_evidence_path)?;
        anyhow::bail!("phase8 canary preflight blocked before live runner");
    }
    let blocked_evidence = Phase8CanaryEvidence::blocked_before_submit(
        phase8_operator_evidence_input(&envelope, &loaded, &root_hash)?,
        Phase8CanaryBlockReason::LiveProofCaptureUnavailable,
    );
    blocked_evidence.write_json_file(&envelope.canary_evidence_path)?;
    phase8_live_runner_requires_post_run_evidence_capture()?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut node = build_bolt_v3_live_node(&loaded)?;
            run_bolt_v3_live_node(&mut node, &loaded).await
        })
        .await?;
    Ok(())
}

fn phase8_current_checkout_head_sha() -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|source| anyhow::anyhow!("failed to run git rev-parse HEAD: {source}"))?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let head = String::from_utf8(output.stdout)?;
    let head = head.trim();
    if head.is_empty() {
        return Err(anyhow::anyhow!("git rev-parse HEAD returned an empty head"));
    }
    Ok(head.to_string())
}

fn phase8_operator_evidence_input(
    envelope: &Phase8OperatorApprovalEnvelope,
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    root_hash: &str,
) -> anyhow::Result<Phase8CanaryEvidenceInput> {
    let block = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("phase8 operator evidence requires `[live_canary]`"))?;
    Ok(Phase8CanaryEvidenceInput {
        head_sha: envelope.head_sha.clone(),
        root_config_sha256: root_hash.to_string(),
        ssm_manifest_sha256: envelope.ssm_manifest_sha256.clone(),
        ssm_manifest_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.ssm_manifest_path),
            record_hash: envelope.ssm_manifest_sha256.clone(),
        },
        approval_id: envelope.operator_approval_id.clone(),
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order: Decimal::from_str_exact(&block.max_notional_per_order)?,
        runtime_capture_ref: Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&loaded.root.persistence.catalog_directory),
            run_id: "phase8-blocked-before-live-runner".to_string(),
        },
    })
}

fn phase8_sha256_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}")
}

fn phase8_live_runner_requires_post_run_evidence_capture() -> anyhow::Result<()> {
    anyhow::bail!(
        "phase8 live runner remains blocked until NT submit, venue state, cancel, and restart reconciliation refs are written to BOLT_V3_PHASE8_EVIDENCE_PATH"
    )
}
