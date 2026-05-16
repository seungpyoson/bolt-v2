use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::{
        PHASE8_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID, Phase8CanaryBlockReason, Phase8CanaryEvidence,
        Phase8CanaryEvidenceInput, Phase8EvidenceRef, Phase8LiveCanaryResultRefs,
        Phase8LiveOrderRef, Phase8OperatorApprovalEnvelope, Phase8RuntimeCaptureRef,
        Phase8StrategyInputSafetyAudit, evaluate_phase8_canary_preflight, phase8_required_env,
        phase8_sha256_text,
    },
    nt_runtime_capture::spool_root_for_instance,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::env;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn phase8_operator_harness_is_ignored_and_uses_production_runner_shape() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("Phase8OperatorApprovalEnvelope::from_env"));
    assert!(source.contains("validate_and_consume_against"));
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
fn phase8_operator_harness_does_not_block_before_production_runner() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(!source.contains(&format!("{}{}", "LiveProof", "CaptureUnavailable")));
    assert!(!source.contains(&format!(
        "{}{}",
        "phase8_live_runner_requires_", "post_run_evidence_capture"
    )));
}

#[test]
fn phase8_operator_harness_derives_strategy_audit_from_evidence_file() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("Phase8StrategyInputSafetyAudit::from_evidence_file"));
    assert!(!source.contains(&format!(
        "{}{}",
        "Phase8StrategyInputSafetyAudit::", "approved()"
    )));
}

#[test]
fn phase8_operator_harness_prevalidates_success_evidence_before_runner() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");
    let start = source
        .rfind("async fn phase8_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn phase8_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    let input_index = harness
        .find("let evidence_input = phase8_operator_evidence_input")
        .expect("success evidence input should be prepared before live runner");
    let snapshot_index = harness
        .find("snapshot_before_run")
        .expect("post-run evidence paths should be snapshotted before live runner");
    let runner_index = harness
        .find("run_bolt_v3_live_node")
        .expect("operator harness should use production live runner");

    assert!(input_index < runner_index);
    assert!(snapshot_index < runner_index);
}

#[test]
fn phase8_operator_harness_binds_live_proof_to_runtime_admission_and_spool() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("admitted_order_count()"));
    assert!(source.contains("spool_root_for_instance"));
    assert!(source.contains("assert_belongs_to_runtime_capture"));
    assert!(source.contains("assert_changed_after_run"));
    assert!(source.contains("phase8_read_operator_evidence_proof"));
    assert!(!source.contains(&format!("{}{}{}", "BOLT_V3_PHASE8_", "RUNTIME_RUN", "_ID")));
    assert!(!source.contains(&format!(
        "{}{}{}",
        "strategy_cancel_path: phase8_required_env(\"",
        "BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH",
        "\")?"
    )));
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
    let current_unix_seconds = phase8_current_unix_seconds()?;
    envelope.validate_and_consume_against(
        &current_head,
        &root_hash,
        loaded
            .root
            .live_canary
            .as_ref()
            .map(|block| block.approval_id.as_str())
            .unwrap_or_default(),
        &loaded,
        current_unix_seconds,
    )?;
    let strategy_audit = Phase8StrategyInputSafetyAudit::from_evidence_file(
        &envelope.strategy_input_evidence_path,
        &envelope.strategy_input_evidence_sha256,
    )?;
    let preflight = evaluate_phase8_canary_preflight(&loaded, &current_head, strategy_audit).await;
    if !preflight.can_enter_live_runner() {
        let blocked_runtime_capture_ref = Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&loaded.root.persistence.catalog_directory),
            run_id: PHASE8_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID.to_string(),
        };
        let evidence = Phase8CanaryEvidence::blocked_before_submit(
            phase8_operator_evidence_input(
                &envelope,
                &loaded,
                &root_hash,
                blocked_runtime_capture_ref,
            )?,
            preflight
                .block_reasons
                .first()
                .cloned()
                .unwrap_or(Phase8CanaryBlockReason::BlockedBeforeLiveOrder),
        );
        evidence.write_json_file(&envelope.canary_evidence_path)?;
        anyhow::bail!("phase8 canary preflight blocked before live runner");
    }
    let result_paths = Phase8OperatorLiveResultPaths::from_env()?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut node = build_bolt_v3_live_node(&loaded)?;
            let runtime_capture = phase8_operator_runtime_capture(&loaded, &node.instance_id());
            let evidence_input = phase8_operator_evidence_input(
                &envelope,
                &loaded,
                &root_hash,
                runtime_capture.reference.clone(),
            )?;
            result_paths.assert_belongs_to_runtime_capture(&runtime_capture.spool_root)?;
            let pre_run_snapshot = result_paths.snapshot_before_run()?;
            run_bolt_v3_live_node(&mut node, &loaded)
                .await
                .map_err(anyhow::Error::from)?;
            let admitted_order_count = node.admitted_order_count();
            let (decision_evidence_ref, live_order_ref, result_refs) =
                result_paths.to_refs(&pre_run_snapshot, &runtime_capture.reference.run_id)?;
            let evidence = Phase8CanaryEvidence::live_canary_proof(
                evidence_input,
                decision_evidence_ref,
                live_order_ref,
                result_refs,
                admitted_order_count,
            )?;
            evidence.write_json_file(&envelope.canary_evidence_path)?;
            Ok::<(), anyhow::Error>(())
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

fn phase8_current_unix_seconds() -> anyhow::Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?;
    i64::try_from(duration.as_secs())
        .map_err(|source| anyhow::anyhow!("current unix seconds exceeds i64: {source}"))
}

fn phase8_operator_evidence_input(
    envelope: &Phase8OperatorApprovalEnvelope,
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    root_hash: &str,
    runtime_capture_ref: Phase8RuntimeCaptureRef,
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
        strategy_input_evidence_ref: Phase8EvidenceRef {
            path_hash: phase8_sha256_text(&envelope.strategy_input_evidence_path),
            record_hash: envelope.strategy_input_evidence_sha256.clone(),
        },
        approval_id: envelope.operator_approval_id.clone(),
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order: Decimal::from_str_exact(&block.max_notional_per_order)?,
        runtime_capture_ref,
    })
}

struct Phase8OperatorRuntimeCapture {
    reference: Phase8RuntimeCaptureRef,
    spool_root: String,
}

fn phase8_operator_runtime_capture(
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    instance_id: &str,
) -> Phase8OperatorRuntimeCapture {
    let spool_root =
        spool_root_for_instance(&loaded.root.persistence.catalog_directory, instance_id);
    Phase8OperatorRuntimeCapture {
        reference: Phase8RuntimeCaptureRef {
            spool_root_hash: phase8_sha256_text(&spool_root),
            run_id: instance_id.to_string(),
        },
        spool_root,
    }
}

fn phase8_optional_env(name: &str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(anyhow::anyhow!(
            "failed to read phase8 env `{name}`: {error}"
        )),
    }
}

struct Phase8OperatorLiveResultPaths {
    decision_evidence_path: String,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    nt_submit_event_path: String,
    venue_order_state_path: String,
    strategy_cancel_path: Option<String>,
    restart_reconciliation_path: String,
}

struct Phase8OperatorLiveResultSnapshot {
    decision_evidence_sha256: Option<String>,
    nt_submit_event_sha256: Option<String>,
    venue_order_state_sha256: Option<String>,
    strategy_cancel_sha256: Option<String>,
}

#[derive(Deserialize)]
struct Phase8OperatorEvidenceProof {
    record_kind: String,
    run_id: Option<String>,
    source_run_id: Option<String>,
    client_order_id_hash: Option<String>,
    venue_order_id_hash: Option<String>,
}

impl Phase8OperatorLiveResultPaths {
    fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            decision_evidence_path: phase8_required_env("BOLT_V3_PHASE8_DECISION_EVIDENCE_PATH")?,
            client_order_id_hash: phase8_required_sha256_env(
                "BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH",
            )?,
            venue_order_id_hash: phase8_required_sha256_env("BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH")?,
            nt_submit_event_path: phase8_required_env("BOLT_V3_PHASE8_NT_SUBMIT_EVENT_PATH")?,
            venue_order_state_path: phase8_required_env("BOLT_V3_PHASE8_VENUE_ORDER_STATE_PATH")?,
            strategy_cancel_path: phase8_optional_env("BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH")?,
            restart_reconciliation_path: phase8_required_env(
                "BOLT_V3_PHASE8_RESTART_RECONCILIATION_PATH",
            )?,
        })
    }

    fn assert_belongs_to_runtime_capture(&self, spool_root: &str) -> anyhow::Result<()> {
        phase8_assert_path_starts_with(
            &self.nt_submit_event_path,
            spool_root,
            "nt submit event evidence",
        )?;
        phase8_assert_path_starts_with(
            &self.venue_order_state_path,
            spool_root,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_path_starts_with(
                strategy_cancel_path,
                spool_root,
                "strategy cancel evidence",
            )?;
        }
        Ok(())
    }

    fn snapshot_before_run(&self) -> anyhow::Result<Phase8OperatorLiveResultSnapshot> {
        Ok(Phase8OperatorLiveResultSnapshot {
            decision_evidence_sha256: phase8_optional_sha256_file(&self.decision_evidence_path)?,
            nt_submit_event_sha256: phase8_optional_sha256_file(&self.nt_submit_event_path)?,
            venue_order_state_sha256: phase8_optional_sha256_file(&self.venue_order_state_path)?,
            strategy_cancel_sha256: match &self.strategy_cancel_path {
                Some(strategy_cancel_path) => phase8_optional_sha256_file(strategy_cancel_path)?,
                None => None,
            },
        })
    }

    fn assert_changed_after_run(
        &self,
        snapshot: &Phase8OperatorLiveResultSnapshot,
    ) -> anyhow::Result<()> {
        phase8_assert_changed_after_run(
            &self.decision_evidence_path,
            &snapshot.decision_evidence_sha256,
            "decision evidence",
        )?;
        phase8_assert_changed_after_run(
            &self.nt_submit_event_path,
            &snapshot.nt_submit_event_sha256,
            "nt submit event evidence",
        )?;
        phase8_assert_changed_after_run(
            &self.venue_order_state_path,
            &snapshot.venue_order_state_sha256,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_changed_after_run(
                strategy_cancel_path,
                &snapshot.strategy_cancel_sha256,
                "strategy cancel evidence",
            )?;
        }
        Ok(())
    }

    fn to_refs(
        &self,
        snapshot: &Phase8OperatorLiveResultSnapshot,
        run_id: &str,
    ) -> anyhow::Result<(
        Phase8EvidenceRef,
        Phase8LiveOrderRef,
        Phase8LiveCanaryResultRefs,
    )> {
        self.assert_changed_after_run(snapshot)?;
        self.assert_proof_content(run_id)?;
        Ok((
            phase8_operator_evidence_ref(&self.decision_evidence_path)?,
            Phase8LiveOrderRef {
                client_order_id_hash: self.client_order_id_hash.clone(),
                venue_order_id_hash: self.venue_order_id_hash.clone(),
            },
            Phase8LiveCanaryResultRefs {
                nt_submit_event_ref: phase8_operator_evidence_ref(&self.nt_submit_event_path)?,
                venue_order_state_ref: phase8_operator_evidence_ref(&self.venue_order_state_path)?,
                strategy_cancel_ref: self
                    .strategy_cancel_path
                    .as_deref()
                    .map(phase8_operator_evidence_ref)
                    .transpose()?,
                restart_reconciliation_ref: phase8_operator_evidence_ref(
                    &self.restart_reconciliation_path,
                )?,
            },
        ))
    }

    fn assert_proof_content(&self, run_id: &str) -> anyhow::Result<()> {
        phase8_assert_operator_evidence_proof(
            &self.decision_evidence_path,
            "decision_evidence",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            None,
        )?;
        phase8_assert_operator_evidence_proof(
            &self.nt_submit_event_path,
            "nt_submit_event",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            None,
        )?;
        phase8_assert_operator_evidence_proof(
            &self.venue_order_state_path,
            "venue_order_state",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            phase8_assert_operator_evidence_proof(
                strategy_cancel_path,
                "strategy_cancel",
                Some(run_id),
                None,
                Some(&self.client_order_id_hash),
                Some(&self.venue_order_id_hash),
            )?;
        }
        phase8_assert_operator_evidence_proof(
            &self.restart_reconciliation_path,
            "restart_reconciliation",
            None,
            Some(run_id),
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )
    }
}

fn phase8_operator_evidence_ref(path: &str) -> anyhow::Result<Phase8EvidenceRef> {
    Ok(Phase8EvidenceRef {
        path_hash: phase8_sha256_text(path),
        record_hash: Phase8OperatorApprovalEnvelope::sha256_file(path)?,
    })
}

fn phase8_required_sha256_env(name: &str) -> anyhow::Result<String> {
    let value = phase8_required_env(name)?;
    if value.len() != 64 || !value.chars().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!(
            "required phase8 env `{name}` must be a sha256 hex digest"
        ));
    }
    Ok(value)
}

fn phase8_assert_path_starts_with(path: &str, base: &str, label: &str) -> anyhow::Result<()> {
    phase8_reject_parent_dir(path, label)?;
    phase8_reject_parent_dir(base, "runtime capture spool root")?;
    if !Path::new(path).starts_with(Path::new(base)) {
        return Err(anyhow::anyhow!(
            "phase8 {label} path must be under runtime capture spool root"
        ));
    }
    Ok(())
}

fn phase8_reject_parent_dir(path: &str, label: &str) -> anyhow::Result<()> {
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!(
            "phase8 {label} path must not contain parent directory traversal"
        ));
    }
    Ok(())
}

fn phase8_optional_sha256_file(path: &str) -> anyhow::Result<Option<String>> {
    if Path::new(path).exists() {
        Ok(Some(Phase8OperatorApprovalEnvelope::sha256_file(path)?))
    } else {
        Ok(None)
    }
}

fn phase8_assert_changed_after_run(
    path: &str,
    before_sha256: &Option<String>,
    label: &str,
) -> anyhow::Result<()> {
    let after_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(path)?;
    if before_sha256.as_ref() == Some(&after_sha256) {
        return Err(anyhow::anyhow!(
            "phase8 {label} did not change during live canary run"
        ));
    }
    Ok(())
}

fn phase8_read_operator_evidence_proof(
    path: &str,
    label: &str,
) -> anyhow::Result<Phase8OperatorEvidenceProof> {
    let file = std::fs::File::open(path)
        .map_err(|source| anyhow::anyhow!("failed to open phase8 {label} proof: {source}"))?;
    serde_json::from_reader(file)
        .map_err(|source| anyhow::anyhow!("failed to parse phase8 {label} proof: {source}"))
}

fn phase8_assert_operator_evidence_proof(
    path: &str,
    expected_kind: &str,
    expected_run_id: Option<&str>,
    expected_source_run_id: Option<&str>,
    expected_client_order_id_hash: Option<&str>,
    expected_venue_order_id_hash: Option<&str>,
) -> anyhow::Result<()> {
    let proof = phase8_read_operator_evidence_proof(path, expected_kind)?;
    if proof.record_kind != expected_kind {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof has unexpected record_kind"
        ));
    }
    if let Some(expected_run_id) = expected_run_id
        && proof.run_id.as_deref() != Some(expected_run_id)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof run_id does not match live canary run"
        ));
    }
    if let Some(expected_source_run_id) = expected_source_run_id
        && proof.source_run_id.as_deref() != Some(expected_source_run_id)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof source_run_id does not match live canary run"
        ));
    }
    if let Some(expected_client_order_id_hash) = expected_client_order_id_hash
        && proof.client_order_id_hash.as_deref() != Some(expected_client_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof client_order_id_hash does not match"
        ));
    }
    if let Some(expected_venue_order_id_hash) = expected_venue_order_id_hash
        && proof.venue_order_id_hash.as_deref() != Some(expected_venue_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "phase8 {expected_kind} proof venue_order_id_hash does not match"
        ));
    }
    Ok(())
}
