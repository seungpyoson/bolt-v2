use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_tiny_canary_evidence::{
        TINY_CANARY_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID, TinyCanaryBlockReason, TinyCanaryEvidence,
        TinyCanaryEvidenceInput, TinyCanaryEvidenceRef, TinyCanaryLiveCanaryResultRefs,
        TinyCanaryLiveOrderRef, TinyCanaryOperatorApprovalEnvelope, TinyCanaryRuntimeCaptureRef,
        TinyCanaryStrategyInputSafetyAudit, evaluate_tiny_canary_preflight,
        tiny_canary_sha256_text,
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
fn tiny_canary_operator_contract_is_config_owned_not_env_owned() {
    let evidence_source = std::fs::read_to_string("src/bolt_v3_tiny_canary_evidence.rs")
        .expect("tiny canary evidence source should be readable");
    let operator_source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");
    for source in [&evidence_source, &operator_source] {
        for env_token in [
            concat!("BOLT_V3_", "TINY_CANARY"),
            concat!("TinyCanaryOperatorApprovalEnvelope", "::", "from_", "env"),
            concat!("TinyCanaryOperatorLiveResultPaths", "::", "from_", "env"),
            concat!("tiny_canary_", "required_", "env"),
        ] {
            assert!(
                !source.contains(env_token),
                "tiny canary operator contract must not depend on env token `{env_token}`"
            );
        }
    }
}

#[test]
fn tiny_canary_operator_harness_is_ignored_and_uses_production_runner_shape() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");
    let start = source
        .find(
            "#[ignore]\nfn tiny_canary_operator_harness_requires_exact_approval_before_live_runner",
        )
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn tiny_canary_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    assert!(source.contains("#[ignore]"));
    assert!(source.contains("TinyCanaryOperatorApprovalEnvelope::from_config"));
    assert!(source.contains("validate_and_consume_against"));
    assert!(source.contains("evaluate_tiny_canary_preflight"));
    assert!(source.contains("write_json_file"));
    assert!(harness.contains("build_bolt_v3_live_node"));
    assert!(harness.contains("run_bolt_v3_live_node"));
    assert!(harness.contains("tokio::task::LocalSet"));
    assert!(harness.contains("runtime.block_on(local.run_until"));
    assert!(!source.contains(concat!("#[", "tokio::test")));
    assert!(!source.contains(&format!("{}{}", "LiveNode", "::run")));
    assert!(!source.contains(&format!("{}{}", ".submit", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".cancel", "_order(")));
    assert!(!source.contains(&format!("{}{}", ".replace", "_order(")));

    let build_live_node = harness
        .find("let mut node = build_bolt_v3_live_node(&loaded)?;")
        .expect("operator harness must build the LiveNode");
    let build_runtime = harness
        .find("let runtime = tokio::runtime::Builder::new_current_thread()")
        .expect("operator harness must build the Tokio runtime");
    let enter_runtime = harness
        .find("runtime.block_on(local.run_until")
        .expect("operator harness must enter the runner future through LocalSet");
    assert!(build_live_node < build_runtime);
    assert!(build_runtime < enter_runtime);
}

#[test]
fn tiny_canary_operator_harness_does_not_block_before_production_runner() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(!source.contains(&format!("{}{}", "LiveProof", "CaptureUnavailable")));
    assert!(!source.contains(&format!(
        "{}{}",
        "tiny_canary_live_runner_requires_", "post_run_evidence_capture"
    )));
}

#[test]
fn tiny_canary_operator_harness_derives_strategy_audit_from_evidence_file() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("TinyCanaryStrategyInputSafetyAudit::from_evidence_file"));
    assert!(!source.contains(&format!(
        "{}{}",
        "TinyCanaryStrategyInputSafetyAudit::", "approved()"
    )));
}

#[test]
fn tiny_canary_operator_harness_prevalidates_success_evidence_before_runner() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");
    let start = source
        .rfind("fn tiny_canary_operator_harness_requires_exact_approval_before_live_runner")
        .expect("operator harness start should exist");
    let end = source[start..]
        .find("\nfn tiny_canary_current_checkout_head_sha")
        .map(|offset| start + offset)
        .expect("operator harness end should exist");
    let harness = &source[start..end];

    let input_index = harness
        .find("let evidence_input = tiny_canary_operator_evidence_input")
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
fn tiny_canary_operator_harness_binds_live_proof_to_runtime_admission_and_spool() {
    let source = std::fs::read_to_string("tests/bolt_v3_tiny_canary_operator.rs")
        .expect("operator harness source should be readable");

    assert!(source.contains("admitted_order_count()"));
    assert!(source.contains("spool_root_for_instance"));
    assert!(source.contains("assert_belongs_to_runtime_capture"));
    assert!(source.contains("assert_changed_after_run"));
    assert!(source.contains("tiny_canary_read_operator_evidence_proof"));
    assert!(!source.contains(concat!("runtime_", "run_id")));
    assert!(!source.contains(concat!("strategy_cancel_path: ", "required_operator_field")));
}

#[test]
fn tiny_canary_operator_head_is_resolved_from_checkout() -> anyhow::Result<()> {
    let head = tiny_canary_current_checkout_head_sha()?;

    assert_eq!(head.len(), 40);
    assert!(head.chars().all(|byte| byte.is_ascii_hexdigit()));
    Ok(())
}

#[test]
#[ignore]
fn tiny_canary_operator_harness_requires_exact_approval_before_live_runner() -> anyhow::Result<()> {
    let root_toml_path = env::var("BOLT_V3_ROOT_TOML")
        .map_err(|_| anyhow::anyhow!("BOLT_V3_ROOT_TOML must point to root TOML"))?;
    let loaded = load_bolt_v3_config(std::path::Path::new(&root_toml_path))?;
    let root_hash = TinyCanaryOperatorApprovalEnvelope::sha256_file(&loaded.root_path)?;
    let current_head = tiny_canary_current_checkout_head_sha()?;
    let envelope = TinyCanaryOperatorApprovalEnvelope::from_config(&loaded)?;
    let current_unix_seconds = tiny_canary_current_unix_seconds()?;
    envelope.validate_and_consume_against(
        &current_head,
        &root_hash,
        loaded
            .root
            .live_canary
            .as_ref()
            .map(|block| block.approval_id.as_str())
            .unwrap_or_default(),
        current_unix_seconds,
    )?;
    let strategy_audit = TinyCanaryStrategyInputSafetyAudit::from_evidence_file(
        &envelope.strategy_input_evidence_path,
        &envelope.strategy_input_evidence_sha256,
    )?;
    let preflight_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let preflight = preflight_runtime.block_on(evaluate_tiny_canary_preflight(
        &loaded,
        &current_head,
        strategy_audit,
    ));
    drop(preflight_runtime);
    if !preflight.can_enter_live_runner() {
        let blocked_runtime_capture_ref = TinyCanaryRuntimeCaptureRef {
            spool_root_hash: tiny_canary_sha256_text(&loaded.root.persistence.catalog_directory),
            run_id: TINY_CANARY_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID.to_string(),
        };
        let evidence = TinyCanaryEvidence::blocked_before_submit(
            tiny_canary_operator_evidence_input(
                &envelope,
                &loaded,
                &root_hash,
                blocked_runtime_capture_ref,
            )?,
            preflight
                .block_reasons
                .first()
                .cloned()
                .unwrap_or(TinyCanaryBlockReason::BlockedBeforeLiveOrder),
        );
        evidence.write_json_file(&envelope.canary_evidence_path)?;
        anyhow::bail!("tiny canary preflight blocked before live runner");
    }
    let result_paths = TinyCanaryOperatorLiveResultPaths::from_config(&loaded)?;

    let mut node = build_bolt_v3_live_node(&loaded)?;
    let runtime_capture = tiny_canary_operator_runtime_capture(&loaded, &node.instance_id());
    let evidence_input = tiny_canary_operator_evidence_input(
        &envelope,
        &loaded,
        &root_hash,
        runtime_capture.reference.clone(),
    )?;
    result_paths.assert_belongs_to_runtime_capture(&runtime_capture.spool_root)?;
    let pre_run_snapshot = result_paths.snapshot_before_run()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async {
        run_bolt_v3_live_node(&mut node, &loaded)
            .await
            .map_err(anyhow::Error::from)
    }))?;
    let admitted_order_count = node.admitted_order_count();
    let (decision_evidence_ref, live_order_ref, result_refs) =
        result_paths.to_refs(&pre_run_snapshot, &runtime_capture.reference.run_id)?;
    let evidence = TinyCanaryEvidence::live_canary_proof(
        evidence_input,
        decision_evidence_ref,
        live_order_ref,
        result_refs,
        admitted_order_count,
    )?;
    evidence.write_json_file(&envelope.canary_evidence_path)?;
    Ok(())
}

fn tiny_canary_current_checkout_head_sha() -> anyhow::Result<String> {
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

fn tiny_canary_current_unix_seconds() -> anyhow::Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?;
    i64::try_from(duration.as_secs())
        .map_err(|source| anyhow::anyhow!("current unix seconds exceeds i64: {source}"))
}

fn tiny_canary_operator_evidence_input(
    envelope: &TinyCanaryOperatorApprovalEnvelope,
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    root_hash: &str,
    runtime_capture_ref: TinyCanaryRuntimeCaptureRef,
) -> anyhow::Result<TinyCanaryEvidenceInput> {
    let block =
        loaded.root.live_canary.as_ref().ok_or_else(|| {
            anyhow::anyhow!("tiny canary operator evidence requires `[live_canary]`")
        })?;
    Ok(TinyCanaryEvidenceInput {
        head_sha: envelope.head_sha.clone(),
        root_config_sha256: root_hash.to_string(),
        ssm_manifest_sha256: envelope.ssm_manifest_sha256.clone(),
        ssm_manifest_ref: TinyCanaryEvidenceRef {
            path_hash: tiny_canary_sha256_text(&envelope.ssm_manifest_path),
            record_hash: envelope.ssm_manifest_sha256.clone(),
        },
        strategy_input_evidence_ref: TinyCanaryEvidenceRef {
            path_hash: tiny_canary_sha256_text(&envelope.strategy_input_evidence_path),
            record_hash: envelope.strategy_input_evidence_sha256.clone(),
        },
        approval_id: envelope.operator_approval_id.clone(),
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order: Decimal::from_str_exact(&block.max_notional_per_order)?,
        runtime_capture_ref,
    })
}

struct TinyCanaryOperatorRuntimeCapture {
    reference: TinyCanaryRuntimeCaptureRef,
    spool_root: String,
}

fn tiny_canary_operator_runtime_capture(
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    instance_id: &str,
) -> TinyCanaryOperatorRuntimeCapture {
    let spool_root =
        spool_root_for_instance(&loaded.root.persistence.catalog_directory, instance_id);
    TinyCanaryOperatorRuntimeCapture {
        reference: TinyCanaryRuntimeCaptureRef {
            spool_root_hash: tiny_canary_sha256_text(&spool_root),
            run_id: instance_id.to_string(),
        },
        spool_root,
    }
}

struct TinyCanaryOperatorLiveResultPaths {
    decision_evidence_path: String,
    client_order_id_hash: String,
    venue_order_id_hash: String,
    nt_submit_event_path: String,
    venue_order_state_path: String,
    strategy_cancel_path: Option<String>,
    restart_reconciliation_path: String,
}

struct TinyCanaryOperatorLiveResultSnapshot {
    decision_evidence_sha256: Option<String>,
    nt_submit_event_sha256: Option<String>,
    venue_order_state_sha256: Option<String>,
    strategy_cancel_sha256: Option<String>,
}

#[derive(Deserialize)]
struct TinyCanaryOperatorEvidenceProof {
    record_kind: String,
    run_id: Option<String>,
    source_run_id: Option<String>,
    client_order_id_hash: Option<String>,
    venue_order_id_hash: Option<String>,
}

impl TinyCanaryOperatorLiveResultPaths {
    fn from_config(loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config) -> anyhow::Result<Self> {
        let operator_evidence = loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|block| block.operator_evidence.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "tiny canary live result paths require `[live_canary.operator_evidence]`"
                )
            })?;
        Ok(Self {
            decision_evidence_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.decision_evidence_path,
                "[live_canary.operator_evidence].decision_evidence_path",
            )?,
            client_order_id_hash: required_operator_sha256(
                &operator_evidence.client_order_id_hash,
                "[live_canary.operator_evidence].client_order_id_hash",
            )?,
            venue_order_id_hash: required_operator_sha256(
                &operator_evidence.venue_order_id_hash,
                "[live_canary.operator_evidence].venue_order_id_hash",
            )?,
            nt_submit_event_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.nt_submit_event_path,
                "[live_canary.operator_evidence].nt_submit_event_path",
            )?,
            venue_order_state_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.venue_order_state_path,
                "[live_canary.operator_evidence].venue_order_state_path",
            )?,
            strategy_cancel_path: optional_operator_path(
                &loaded.root_path,
                operator_evidence.strategy_cancel_path.as_deref(),
            ),
            restart_reconciliation_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.restart_reconciliation_path,
                "[live_canary.operator_evidence].restart_reconciliation_path",
            )?,
        })
    }

    fn assert_belongs_to_runtime_capture(&self, spool_root: &str) -> anyhow::Result<()> {
        tiny_canary_assert_path_starts_with(
            &self.nt_submit_event_path,
            spool_root,
            "nt submit event evidence",
        )?;
        tiny_canary_assert_path_starts_with(
            &self.venue_order_state_path,
            spool_root,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            tiny_canary_assert_path_starts_with(
                strategy_cancel_path,
                spool_root,
                "strategy cancel evidence",
            )?;
        }
        Ok(())
    }

    fn snapshot_before_run(&self) -> anyhow::Result<TinyCanaryOperatorLiveResultSnapshot> {
        Ok(TinyCanaryOperatorLiveResultSnapshot {
            decision_evidence_sha256: tiny_canary_optional_sha256_file(
                &self.decision_evidence_path,
            )?,
            nt_submit_event_sha256: tiny_canary_optional_sha256_file(&self.nt_submit_event_path)?,
            venue_order_state_sha256: tiny_canary_optional_sha256_file(
                &self.venue_order_state_path,
            )?,
            strategy_cancel_sha256: match &self.strategy_cancel_path {
                Some(strategy_cancel_path) => {
                    tiny_canary_optional_sha256_file(strategy_cancel_path)?
                }
                None => None,
            },
        })
    }

    fn assert_changed_after_run(
        &self,
        snapshot: &TinyCanaryOperatorLiveResultSnapshot,
    ) -> anyhow::Result<()> {
        tiny_canary_assert_changed_after_run(
            &self.decision_evidence_path,
            &snapshot.decision_evidence_sha256,
            "decision evidence",
        )?;
        tiny_canary_assert_changed_after_run(
            &self.nt_submit_event_path,
            &snapshot.nt_submit_event_sha256,
            "nt submit event evidence",
        )?;
        tiny_canary_assert_changed_after_run(
            &self.venue_order_state_path,
            &snapshot.venue_order_state_sha256,
            "venue order state evidence",
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            tiny_canary_assert_changed_after_run(
                strategy_cancel_path,
                &snapshot.strategy_cancel_sha256,
                "strategy cancel evidence",
            )?;
        }
        Ok(())
    }

    fn to_refs(
        &self,
        snapshot: &TinyCanaryOperatorLiveResultSnapshot,
        run_id: &str,
    ) -> anyhow::Result<(
        TinyCanaryEvidenceRef,
        TinyCanaryLiveOrderRef,
        TinyCanaryLiveCanaryResultRefs,
    )> {
        self.assert_changed_after_run(snapshot)?;
        self.assert_proof_content(run_id)?;
        Ok((
            tiny_canary_operator_evidence_ref(&self.decision_evidence_path)?,
            TinyCanaryLiveOrderRef {
                client_order_id_hash: self.client_order_id_hash.clone(),
                venue_order_id_hash: self.venue_order_id_hash.clone(),
            },
            TinyCanaryLiveCanaryResultRefs {
                nt_submit_event_ref: tiny_canary_operator_evidence_ref(&self.nt_submit_event_path)?,
                venue_order_state_ref: tiny_canary_operator_evidence_ref(
                    &self.venue_order_state_path,
                )?,
                strategy_cancel_ref: self
                    .strategy_cancel_path
                    .as_deref()
                    .map(tiny_canary_operator_evidence_ref)
                    .transpose()?,
                restart_reconciliation_ref: tiny_canary_operator_evidence_ref(
                    &self.restart_reconciliation_path,
                )?,
            },
        ))
    }

    fn assert_proof_content(&self, run_id: &str) -> anyhow::Result<()> {
        tiny_canary_assert_operator_evidence_proof(
            &self.decision_evidence_path,
            "decision_evidence",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            None,
        )?;
        tiny_canary_assert_operator_evidence_proof(
            &self.nt_submit_event_path,
            "nt_submit_event",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            None,
        )?;
        tiny_canary_assert_operator_evidence_proof(
            &self.venue_order_state_path,
            "venue_order_state",
            Some(run_id),
            None,
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )?;
        if let Some(strategy_cancel_path) = &self.strategy_cancel_path {
            tiny_canary_assert_operator_evidence_proof(
                strategy_cancel_path,
                "strategy_cancel",
                Some(run_id),
                None,
                Some(&self.client_order_id_hash),
                Some(&self.venue_order_id_hash),
            )?;
        }
        tiny_canary_assert_operator_evidence_proof(
            &self.restart_reconciliation_path,
            "restart_reconciliation",
            None,
            Some(run_id),
            Some(&self.client_order_id_hash),
            Some(&self.venue_order_id_hash),
        )
    }
}

fn tiny_canary_operator_evidence_ref(path: &str) -> anyhow::Result<TinyCanaryEvidenceRef> {
    Ok(TinyCanaryEvidenceRef {
        path_hash: tiny_canary_sha256_text(path),
        record_hash: TinyCanaryOperatorApprovalEnvelope::sha256_file(path)?,
    })
}

fn required_operator_field(value: &str, field: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow::anyhow!(
            "required tiny canary config field `{field}` is empty"
        ));
    }
    Ok(trimmed.to_string())
}

fn required_operator_path(root_path: &Path, value: &str, field: &str) -> anyhow::Result<String> {
    let value = required_operator_field(value, field)?;
    Ok(resolve_operator_path(root_path, &value))
}

fn optional_operator_path(root_path: &Path, value: Option<&str>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(resolve_operator_path(root_path, trimmed))
        }
    })
}

fn resolve_operator_path(root_path: &Path, configured_path: &str) -> String {
    let configured_path = Path::new(configured_path);
    let resolved = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        root_path
            .parent()
            .map(|parent| parent.join(configured_path))
            .unwrap_or_else(|| configured_path.to_path_buf())
    };
    resolved.to_string_lossy().to_string()
}

fn required_operator_sha256(value: &str, field: &str) -> anyhow::Result<String> {
    let value = required_operator_field(value, field)?;
    if value.len() != 64 || !value.chars().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!(
            "required tiny canary config field `{field}` must be a sha256 hex digest"
        ));
    }
    Ok(value)
}

fn tiny_canary_assert_path_starts_with(path: &str, base: &str, label: &str) -> anyhow::Result<()> {
    tiny_canary_reject_parent_dir(path, label)?;
    tiny_canary_reject_parent_dir(base, "runtime capture spool root")?;
    if !Path::new(path).starts_with(Path::new(base)) {
        return Err(anyhow::anyhow!(
            "tiny canary {label} path must be under runtime capture spool root"
        ));
    }
    Ok(())
}

fn tiny_canary_reject_parent_dir(path: &str, label: &str) -> anyhow::Result<()> {
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!(
            "tiny canary {label} path must not contain parent directory traversal"
        ));
    }
    Ok(())
}

fn tiny_canary_optional_sha256_file(path: &str) -> anyhow::Result<Option<String>> {
    if Path::new(path).exists() {
        Ok(Some(TinyCanaryOperatorApprovalEnvelope::sha256_file(path)?))
    } else {
        Ok(None)
    }
}

fn tiny_canary_assert_changed_after_run(
    path: &str,
    before_sha256: &Option<String>,
    label: &str,
) -> anyhow::Result<()> {
    let after_sha256 = TinyCanaryOperatorApprovalEnvelope::sha256_file(path)?;
    if before_sha256.as_ref() == Some(&after_sha256) {
        return Err(anyhow::anyhow!(
            "tiny canary {label} did not change during live canary run"
        ));
    }
    Ok(())
}

fn tiny_canary_read_operator_evidence_proof(
    path: &str,
    label: &str,
) -> anyhow::Result<TinyCanaryOperatorEvidenceProof> {
    let file = std::fs::File::open(path)
        .map_err(|source| anyhow::anyhow!("failed to open tiny canary {label} proof: {source}"))?;
    serde_json::from_reader(file)
        .map_err(|source| anyhow::anyhow!("failed to parse tiny canary {label} proof: {source}"))
}

fn tiny_canary_assert_operator_evidence_proof(
    path: &str,
    expected_kind: &str,
    expected_run_id: Option<&str>,
    expected_source_run_id: Option<&str>,
    expected_client_order_id_hash: Option<&str>,
    expected_venue_order_id_hash: Option<&str>,
) -> anyhow::Result<()> {
    let proof = tiny_canary_read_operator_evidence_proof(path, expected_kind)?;
    if proof.record_kind != expected_kind {
        return Err(anyhow::anyhow!(
            "tiny canary {expected_kind} proof has unexpected record_kind"
        ));
    }
    if let Some(expected_run_id) = expected_run_id
        && proof.run_id.as_deref() != Some(expected_run_id)
    {
        return Err(anyhow::anyhow!(
            "tiny canary {expected_kind} proof run_id does not match live canary run"
        ));
    }
    if let Some(expected_source_run_id) = expected_source_run_id
        && proof.source_run_id.as_deref() != Some(expected_source_run_id)
    {
        return Err(anyhow::anyhow!(
            "tiny canary {expected_kind} proof source_run_id does not match live canary run"
        ));
    }
    if let Some(expected_client_order_id_hash) = expected_client_order_id_hash
        && proof.client_order_id_hash.as_deref() != Some(expected_client_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "tiny canary {expected_kind} proof client_order_id_hash does not match"
        ));
    }
    if let Some(expected_venue_order_id_hash) = expected_venue_order_id_hash
        && proof.venue_order_id_hash.as_deref() != Some(expected_venue_order_id_hash)
    {
        return Err(anyhow::anyhow!(
            "tiny canary {expected_kind} proof venue_order_id_hash does not match"
        ));
    }
    Ok(())
}
