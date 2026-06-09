use backtesting_vertical_slice::backfill_source_proof_scope::{
    BACKFILL_SOURCE_PROOF_SCOPE_REPORT_FILE, BackfillSourceProofScopeIssue,
    BackfillSourceProofScopeStatus, evaluate_backfill_source_proof_scope,
    write_backfill_source_proof_scope_report_from_spec_file,
};
use backtesting_vertical_slice::source_proof::SourceProofUsageScope;
use serde::Deserialize;
use serde_json::json;

#[test]
fn source_proof_scope_selects_one_manifest_object_without_accepting_whole_run() {
    let binding = first_trade_binding();
    let selected_uri = raw_uri("selected-object");
    let proof = proof_json(&binding, &selected_uri, "selected-object", 11);
    let manifest = manifest_json(
        &selected_uri,
        concrete_source_url(&binding.source_uri),
        "selected-object",
        11,
        vec![object_json(
            &raw_uri("unselected-object"),
            "unselected-object",
            17,
        )],
    );

    let report = evaluate_backfill_source_proof_scope(
        "synthetic-source-proof-scope",
        &proof.to_string(),
        &manifest.to_string(),
    )
    .expect("report");

    assert_eq!(
        report.status,
        BackfillSourceProofScopeStatus::CandidateFound
    );
    assert_eq!(report.manifest_payload_object_count, 2);
    assert_eq!(report.matching_object_count, 1);
    assert!(report.object_level_tranche_required);
    assert!(report.blocking_issues.is_empty());
    let selected = report.selected_object.expect("selected object");
    assert_eq!(selected.s3_uri, selected_uri);
    assert_eq!(selected.sha256, "selected-object");
    assert_eq!(selected.bytes, 11);
    assert_eq!(report.source_binding, binding.key);
    assert_eq!(report.table_family, "trades");
    assert_eq!(
        report.source_usage_scope,
        SourceProofUsageScope::CanonicalBackfillInput
    );
}

#[test]
fn source_proof_scope_preserves_object_selection_metadata() {
    let binding = first_trade_binding();
    let selected_uri = raw_uri("selected-object");
    let proof = proof_json(&binding, &selected_uri, "selected-object", 11);
    let mut selected_object = object_json(&selected_uri, "selected-object", 11);
    selected_object["source_row_groups"] = json!([3, 5]);
    selected_object["predicate_ref"] = json!("source-proof://synthetic/row-groups");
    selected_object["source_url"] = json!(concrete_source_url(&binding.source_uri));
    let manifest = json!({
        "run_id": "synthetic-run",
        "write_mode": "s3_staging_only",
        "canonical_s3_write": false,
        "object_count_excluding_manifest": 1,
        "bytes_excluding_manifest": 11,
        "errors": [],
        "payload_records": [selected_object]
    });

    let report = evaluate_backfill_source_proof_scope(
        "synthetic-source-proof-scope",
        &proof.to_string(),
        &manifest.to_string(),
    )
    .expect("report");

    assert_eq!(
        report.status,
        BackfillSourceProofScopeStatus::CandidateFound
    );
    let selected = report.selected_object.expect("selected object");
    assert_eq!(selected.source_row_groups, vec![3, 5]);
    assert_eq!(
        selected.predicate_ref.as_deref(),
        Some("source-proof://synthetic/row-groups")
    );
}

#[test]
fn source_proof_scope_exposes_and_blocks_one_off_usage_scope() {
    let binding = first_trade_binding();
    let selected_uri = raw_uri("selected-object");
    let mut proof = proof_json(&binding, &selected_uri, "selected-object", 11);
    proof["usage_scope"] = json!("one_off_backfill_data");
    let manifest = manifest_json(
        &selected_uri,
        concrete_source_url(&binding.source_uri),
        "selected-object",
        11,
        Vec::new(),
    );

    let report = evaluate_backfill_source_proof_scope(
        "synthetic-source-proof-scope",
        &proof.to_string(),
        &manifest.to_string(),
    )
    .expect("report");

    assert_eq!(report.status, BackfillSourceProofScopeStatus::Blocked);
    assert_eq!(
        report.source_usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    assert!(
        report
            .blocking_issues
            .contains(&BackfillSourceProofScopeIssue::SourceProofAcceptanceFailed)
    );
}

#[test]
fn source_proof_scope_blocks_when_manifest_does_not_contain_raw_sample() {
    let binding = first_trade_binding();
    let proof = proof_json(&binding, &raw_uri("selected-object"), "selected-object", 11);
    let manifest = manifest_json(
        &raw_uri("different-object"),
        concrete_source_url(&binding.source_uri),
        "different-object",
        11,
        Vec::new(),
    );

    let report = evaluate_backfill_source_proof_scope(
        "synthetic-source-proof-scope",
        &proof.to_string(),
        &manifest.to_string(),
    )
    .expect("report");

    assert_eq!(report.status, BackfillSourceProofScopeStatus::Blocked);
    assert!(report.selected_object.is_none());
    assert!(
        report
            .blocking_issues
            .contains(&BackfillSourceProofScopeIssue::NoMatchingManifestObject)
    );
}

#[test]
fn source_proof_scope_reads_toml_spec_and_writes_report_idempotently() {
    let binding = first_trade_binding();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let proof_path = dir.path().join("source-proof.json");
    let manifest_path = dir.path().join("manifest.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("source-proof-scope.toml");
    let selected_uri = raw_uri("selected-object");

    std::fs::write(
        &proof_path,
        proof_json(&binding, &selected_uri, "selected-object", 11).to_string(),
    )
    .expect("write proof");
    std::fs::write(
        &manifest_path,
        manifest_json(
            &selected_uri,
            concrete_source_url(&binding.source_uri),
            "selected-object",
            11,
            vec![object_json(
                &raw_uri("unselected-object"),
                "unselected-object",
                17,
            )],
        )
        .to_string(),
    )
    .expect("write manifest");
    std::fs::write(
        &spec_path,
        format!(
            r#"report_id = "synthetic-source-proof-scope"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
source_proof_path = "{}"
manifest_path = "{}"
output_dir = "{}"
"#,
            proof_path.display(),
            manifest_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first = write_backfill_source_proof_scope_report_from_spec_file(&spec_path).expect("first");
    let second =
        write_backfill_source_proof_scope_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(BACKFILL_SOURCE_PROOF_SCOPE_REPORT_FILE)
    );
}

#[derive(Debug, Deserialize)]
struct Registry {
    #[serde(rename = "source_binding")]
    source_bindings: Vec<RegistryBinding>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryBinding {
    key: String,
    venue: String,
    product_family: String,
    evidence_state: String,
    source_uri: String,
    table_families: Vec<String>,
}

fn first_trade_binding() -> RegistryBinding {
    let registry: Registry = toml::from_str(include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
    ))
    .expect("registry");
    registry
        .source_bindings
        .into_iter()
        .find(|binding| {
            binding
                .table_families
                .iter()
                .any(|family| family == "trades")
        })
        .expect("trade binding")
}

fn proof_json(
    binding: &RegistryBinding,
    raw_sample_uri: &str,
    raw_sample_hash: &str,
    accepted_bytes: u64,
) -> serde_json::Value {
    json!({
        "source_proof_id": "source-proof-synthetic-scope",
        "source_proof_version": 1,
        "contract_version": "backfill-table-contract.v1",
        "schema_version": "backfill-source-proof.v1",
        "status": "accepted",
        "source_binding": binding.key,
        "venue": binding.venue,
        "product_family": binding.product_family,
        "product_category": binding.product_family,
        "table_family": "trades",
        "evidence_state": binding.evidence_state,
        "source_candidate_class": "official_free",
        "source_selection_status": "ACCEPTED_LOWER_FIDELITY",
        "fixture_type": "perps-spot",
        "requested_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "instrument_universe_id": "synthetic-instrument-universe",
        "raw_sample_uri": raw_sample_uri,
        "raw_sample_hash": raw_sample_hash,
        "schema_sample_uri": raw_sample_uri,
        "schema_sample_hash": raw_sample_hash,
        "license_ref": "license://synthetic",
        "license_scope": "public",
        "retention_ref": "retention://synthetic",
        "cost_ref": "cost://synthetic",
        "nt_mapping_status": "accepted",
        "fidelity_class": "TRADE_REPLAY",
        "l2_replay_evidence": {},
        "forbidden_claims": ["No execution-quality claims."],
        "claim_limits": [{
            "id": "source-proof-claim-limit-synthetic",
            "severity": "blocking",
            "claim": "No execution-quality claims.",
            "reason": "Synthetic trade replay source.",
            "evidence_ref": "source-proof://synthetic/fidelity"
        }],
        "acceptance_scope": {
            "planned_objects": 1,
            "completed_objects": 1,
            "failed_objects": 0,
            "skipped_objects": 0,
            "accepted_bytes": accepted_bytes,
            "selector_scope_violations": 0
        },
        "gap_policy_id": "",
        "required_checks": passed_checks(),
        "acceptance_mode": "manual",
        "accepted_by": "synthetic-operator",
        "accepted_at": "2026-06-02T00:00:00Z"
    })
}

fn passed_checks() -> serde_json::Value {
    let passed = json!({
        "outcome": "passed",
        "evidence_ref": "evidence://synthetic"
    });
    json!({
        "source_access": passed,
        "license": passed,
        "schema": passed,
        "time_semantics": passed,
        "instrument_universe": passed,
        "coverage": passed,
        "retention_freshness": passed,
        "granularity": passed,
        "completeness": passed,
        "nt_mapping": passed,
        "cost": passed,
        "storage": passed
    })
}

fn manifest_json(
    selected_uri: &str,
    selected_source_url: String,
    selected_hash: &str,
    selected_bytes: u64,
    mut extra_objects: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut payload_records = vec![object_json(selected_uri, selected_hash, selected_bytes)];
    payload_records[0]["source_url"] = json!(selected_source_url);
    payload_records.append(&mut extra_objects);
    json!({
        "run_id": "synthetic-run",
        "write_mode": "s3_staging_only",
        "canonical_s3_write": false,
        "object_count_excluding_manifest": payload_records.len(),
        "bytes_excluding_manifest": payload_records.iter().map(|value| value["bytes"].as_u64().unwrap()).sum::<u64>(),
        "errors": [],
        "payload_records": payload_records
    })
}

fn object_json(s3_uri: &str, sha256: &str, bytes: u64) -> serde_json::Value {
    json!({
        "archive_date": "2026-03-01",
        "attrs": {
            "dt": "2026-03-01"
        },
        "bytes": bytes,
        "family": "tick_trades",
        "s3_uri": s3_uri,
        "schema_sample": {
            "header_columns": ["id", "timestamp", "price", "volume", "side"]
        },
        "sha256": sha256,
        "source_url": "https://example.invalid/synthetic.csv.gz"
    })
}

fn raw_uri(object_hash: &str) -> String {
    format!("s3://synthetic-artifacts/raw/v1/object={object_hash}.csv.gz")
}

fn concrete_source_url(template: &str) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
            }
            output.push_str("synthetic");
        } else {
            output.push(ch);
        }
    }
    output
}
