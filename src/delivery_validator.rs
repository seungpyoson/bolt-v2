use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use toml::Value as TomlValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Intake,
    SeamLocked,
    ProofLocked,
    Review,
    MergeCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Block,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Block => "BLOCK",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub status: Status,
    pub kind: String,
    pub where_: String,
    pub why: String,
    pub next: String,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub messages: Vec<Message>,
}

impl Report {
    pub fn has_block(&self) -> bool {
        self.messages.iter().any(|m| m.status == Status::Block)
    }

    pub fn push(
        &mut self,
        status: Status,
        kind: impl Into<String>,
        where_: impl Into<String>,
        why: impl Into<String>,
        next: impl Into<String>,
    ) {
        self.messages.push(Message {
            status,
            kind: kind.into(),
            where_: where_.into(),
            why: why.into(),
            next: next.into(),
        });
    }
}

#[derive(Debug, Deserialize, Default)]
struct IssueContract {
    #[serde(default)]
    required_outcomes: Vec<String>,
    #[serde(default)]
    non_goals: Vec<String>,
    #[serde(default)]
    allowed_surfaces: Vec<String>,
    #[serde(default)]
    forbidden_surfaces: Vec<String>,
    #[serde(default)]
    problem_statement: String,
}

#[derive(Debug, Deserialize, Default)]
struct SeamContract {
    #[serde(default)]
    status: String,
    #[serde(default)]
    seams: Vec<SeamRow>,
}

#[derive(Debug, Deserialize, Default)]
struct SeamRow {
    #[serde(default)]
    semantic_term: String,
    #[serde(default)]
    storage_field: String,
    #[serde(default)]
    authoritative_source: String,
    #[serde(default)]
    forbidden_sources: Vec<String>,
    #[serde(default)]
    fallback_order: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ProofPlan {
    #[serde(default)]
    claims: Vec<Claim>,
}

#[derive(Debug, Deserialize, Default)]
struct Claim {
    #[serde(default)]
    claim_id: String,
    #[serde(default)]
    falsified_by: Vec<String>,
    #[serde(default)]
    required_before: String,
}

#[derive(Debug, Deserialize, Default)]
struct FindingLedger {
    #[serde(default)]
    findings: Vec<Finding>,
}

#[derive(Debug, Deserialize, Default)]
struct Finding {
    #[serde(default)]
    finding_id: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    locus: String,
    #[serde(default)]
    source_refs: Vec<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    resolution_kind: String,
}

#[derive(Debug, Deserialize, Default)]
struct EvidenceBundle {
    #[serde(default)]
    evidence: Vec<Evidence>,
}

#[derive(Debug, Deserialize, Default)]
struct Evidence {
    #[serde(default)]
    evidence_id: String,
    #[serde(default, rename = "type")]
    type_name: String,
    #[serde(default)]
    producer: String,
}

#[derive(Debug, Deserialize, Default)]
struct MergeClaims {
    #[serde(default)]
    merge_ready: bool,
    #[serde(default)]
    open_blockers: Vec<String>,
    #[serde(default)]
    required_evidence: Vec<String>,
    #[serde(default)]
    claims: Vec<MergeClaim>,
}

#[derive(Debug, Deserialize, Default)]
struct MergeClaim {
    #[serde(default)]
    claim_id: String,
    #[serde(default)]
    value: bool,
    #[serde(default)]
    supported_by: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ReviewTarget {
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    round_id: String,
}

#[derive(Debug, Deserialize, Default)]
struct ExecutionTarget {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    base_ref: String,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    diff_identity: String,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct CiSurface {
    #[serde(default)]
    workflow: String,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    run_selection_rule: String,
    #[serde(default)]
    required_jobs_by_stage: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    ignored_jobs: Vec<String>,
    #[serde(default)]
    partial_ci_allowed_stages: Vec<String>,
    #[serde(default)]
    terminal_ci_required_stages: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ClaimEnforcement {
    #[serde(default)]
    rows: Vec<ClaimEnforcementRow>,
}

#[derive(Debug, Deserialize, Default)]
struct ClaimEnforcementRow {
    #[serde(default)]
    claim_id: String,
    #[serde(default)]
    enforcement_kind: String,
    #[serde(default)]
    enforced_at: String,
    #[serde(default)]
    test_ref: String,
    #[serde(default)]
    ci_ref: String,
    #[serde(default)]
    evidence_required: Vec<String>,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ClaimEnforcementCoverageSummary {
    status: String,
    summary_kind: String,
    summary_verdict: String,
    subject: String,
    source_refs: Vec<String>,
    rule_version: String,
    covered_true_claim_count: i64,
    uncovered_true_claim_count: i64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct OrchestrationReachabilitySummary {
    stage: String,
    summary_kind: String,
    summary_verdict: String,
    source_refs: Vec<String>,
    rule_version: String,
    out_of_surface_required_job_count: i64,
    incomplete_case_count: i64,
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct AssumptionRegister {
    #[serde(default)]
    assumptions: Vec<AssumptionRow>,
}

#[derive(Debug, Deserialize, Default)]
struct AssumptionRow {
    #[serde(default)]
    assumption_id: String,
    #[serde(default)]
    impact_class: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    trust_root: String,
    #[serde(default)]
    monitor: String,
    #[serde(default)]
    expiry_trigger: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct ReviewRound {
    #[serde(default)]
    round_id: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    review_target_ref: String,
    #[serde(default)]
    raw_comment_refs: Vec<String>,
    #[serde(default)]
    ingested_findings: Vec<String>,
    #[serde(default)]
    stale_findings: Vec<String>,
    #[serde(default)]
    wrong_target_findings: Vec<String>,
    #[serde(default)]
    absorbed_by_head: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct StagePromotion {
    #[serde(default)]
    promotions: Vec<StagePromotionRow>,
}

#[derive(Debug, Deserialize, Default)]
struct StagePromotionRow {
    #[serde(default)]
    from_stage: String,
    #[serde(default)]
    to_stage: String,
    #[serde(default)]
    promotion_gate_artifact: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct PromotionGate {
    #[serde(default)]
    gates: Vec<PromotionGateRow>,
}

#[derive(Debug, Deserialize, Default)]
struct PromotionGateRow {
    #[serde(default)]
    gate_id: String,
    #[serde(default)]
    from_stage: String,
    #[serde(default)]
    to_stage: String,
    #[serde(default)]
    comparator_kind: String,
    #[serde(default)]
    left_ref: String,
    #[serde(default)]
    right_ref: String,
    #[serde(default)]
    right_literal: String,
    #[serde(default)]
    clauses: Vec<PromotionGateClause>,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct PromotionGateClause {
    #[serde(default)]
    comparator_kind: String,
    #[serde(default)]
    left_ref: String,
    #[serde(default)]
    right_ref: String,
    #[serde(default)]
    right_literal: String,
}

#[derive(Debug, Deserialize, Default)]
struct OrchestrationReachability {
    #[serde(default)]
    cases: Vec<ReachabilityCase>,
}

#[derive(Debug, Deserialize, Default)]
struct ReachabilityCase {
    #[serde(default)]
    case_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    trigger_job: String,
    #[serde(default)]
    trigger_result: String,
    #[serde(default)]
    required_reachable_jobs: Vec<String>,
    #[serde(default)]
    forbidden_job_results: Vec<String>,
    #[serde(default)]
    proof_ref: String,
    #[serde(default)]
    status: String,
}

fn load_optional<T: for<'de> Deserialize<'de>>(dir: &Path, file_name: &str) -> Result<Option<T>> {
    let path = dir.join(file_name);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value = toml::from_slice(&bytes)
        .with_context(|| format!("failed to parse TOML {}", path.display()))?;
    Ok(Some(value))
}

fn load_from_path<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_slice(&bytes).with_context(|| format!("failed to parse TOML {}", path.display()))
}

fn scalar_summary_rel_path(ref_spec: &str) -> Option<&str> {
    let (rel_path, _) = ref_spec.split_once('#')?;
    (rel_path.ends_with("_summary.toml") || rel_path.ends_with("_coverage.toml"))
        .then_some(rel_path)
}

fn validate_scalar_summary_artifact(
    dir: &Path,
    rel_path: &str,
    report: &mut Report,
    seen: &mut BTreeSet<String>,
) {
    if !seen.insert(rel_path.to_string()) {
        return;
    }
    let path = dir.join(rel_path);
    if !path.exists() {
        report.push(
            Status::Block,
            "schema",
            rel_path,
            "scalar summary artifact is missing",
            "add the scalar summary artifact or remove the broken gate reference",
        );
        return;
    }

    let value = match load_from_path::<TomlValue>(&path) {
        Ok(value) => value,
        Err(_) => {
            report.push(
                Status::Block,
                "schema",
                rel_path,
                "scalar summary artifact is unreadable",
                "make the scalar summary artifact parse as TOML",
            );
            return;
        }
    };

    let require_nonempty_scalar = |field: &str, report: &mut Report| {
        let Some(field_value) = value.get(field) else {
            report.push(
                Status::Block,
                "schema",
                rel_path,
                format!("scalar summary artifact is missing `{field}`"),
                "fill all required scalar summary fields",
            );
            return;
        };
        let ok = match field_value {
            TomlValue::String(v) => !v.is_empty(),
            TomlValue::Boolean(_) => true,
            TomlValue::Integer(_) => true,
            TomlValue::Float(_) => true,
            TomlValue::Datetime(_) => true,
            _ => false,
        };
        if !ok {
            report.push(
                Status::Block,
                "schema",
                rel_path,
                format!("scalar summary field `{field}` is empty or non-scalar"),
                "keep scalar summary fields scalar and nonempty",
            );
        }
    };

    require_nonempty_scalar("status", report);
    require_nonempty_scalar("summary_kind", report);
    require_nonempty_scalar("summary_verdict", report);
    require_nonempty_scalar("rule_version", report);

    match value.get("source_refs") {
        Some(TomlValue::Array(items)) if !items.is_empty() => {}
        _ => report.push(
            Status::Block,
            "schema",
            rel_path,
            "scalar summary artifact must declare nonempty `source_refs`",
            "bind the scalar summary artifact to at least one source artifact ref",
        ),
    }
}

fn compute_claim_enforcement_coverage_summary(
    merge: &MergeClaims,
    enforcement: &ClaimEnforcement,
) -> ClaimEnforcementCoverageSummary {
    let rows_by_claim: BTreeMap<&str, &ClaimEnforcementRow> = enforcement
        .rows
        .iter()
        .filter(|row| !row.claim_id.is_empty())
        .map(|row| (row.claim_id.as_str(), row))
        .collect();

    let mut covered_true_claim_count = 0_i64;
    let mut uncovered_true_claim_count = 0_i64;
    for claim in &merge.claims {
        if !claim.value {
            continue;
        }
        match rows_by_claim.get(claim.claim_id.as_str()) {
            Some(row)
                if !row.enforcement_kind.is_empty()
                    && !row.enforced_at.is_empty()
                    && !row.status.is_empty() =>
            {
                covered_true_claim_count += 1;
            }
            _ => uncovered_true_claim_count += 1,
        }
    }

    let summary_verdict = if uncovered_true_claim_count == 0 {
        "pass"
    } else {
        "block"
    };

    ClaimEnforcementCoverageSummary {
        status: "frozen".to_string(),
        summary_kind: "claim_enforcement_coverage".to_string(),
        summary_verdict: summary_verdict.to_string(),
        subject: "true_merge_claims_have_bound_enforcement_rows".to_string(),
        source_refs: vec![
            "merge_claims.toml".to_string(),
            "claim_enforcement.toml".to_string(),
        ],
        rule_version: "v1".to_string(),
        covered_true_claim_count,
        uncovered_true_claim_count,
    }
}

fn compute_orchestration_reachability_summary(
    stage: Stage,
    ci_surface: &CiSurface,
    reachability: &OrchestrationReachability,
) -> OrchestrationReachabilitySummary {
    let stage_name = stage_key(stage).to_string();
    let stage_jobs: BTreeSet<&str> = ci_surface
        .required_jobs_by_stage
        .get(stage_key(stage))
        .map(|jobs| jobs.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let mut incomplete_case_count = 0_i64;
    let mut out_of_surface_required_job_count = 0_i64;

    for case in &reachability.cases {
        let incomplete = case.case_id.is_empty()
            || case.subject.is_empty()
            || case.trigger_job.is_empty()
            || case.trigger_result.is_empty()
            || case.required_reachable_jobs.is_empty()
            || case.forbidden_job_results.is_empty()
            || case.proof_ref.is_empty()
            || case.status.is_empty();
        if incomplete {
            incomplete_case_count += 1;
        }

        out_of_surface_required_job_count += case
            .required_reachable_jobs
            .iter()
            .filter(|job| !stage_jobs.contains(job.as_str()))
            .count() as i64;
    }

    let summary_verdict = if incomplete_case_count == 0 && out_of_surface_required_job_count == 0 {
        "pass"
    } else {
        "block"
    };

    OrchestrationReachabilitySummary {
        stage: stage_name,
        summary_kind: "orchestration_reachability".to_string(),
        summary_verdict: summary_verdict.to_string(),
        source_refs: vec![
            "orchestration_reachability.toml".to_string(),
            "ci_surface.toml".to_string(),
        ],
        rule_version: "v1".to_string(),
        out_of_surface_required_job_count,
        incomplete_case_count,
        status: "frozen".to_string(),
    }
}

fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::Review => "review",
        Stage::MergeCandidate => "merge_candidate",
        Stage::Intake => "intake",
        Stage::SeamLocked => "seam_locked",
        Stage::ProofLocked => "proof_locked",
    }
}

fn resolve_toml_value_ref(dir: &Path, ref_spec: &str) -> Result<TomlValue> {
    let (rel_path, field) = ref_spec
        .split_once('#')
        .with_context(|| format!("ref `{ref_spec}` must be in `<path>#<field>` form"))?;
    let path = dir.join(rel_path);
    let value = load_from_path::<TomlValue>(&path)?;
    let mut current = &value;
    for segment in field.split('.') {
        current = current
            .get(segment)
            .with_context(|| format!("ref `{ref_spec}` could not resolve segment `{segment}`"))?;
    }
    Ok(current.clone())
}

fn resolve_toml_scalar_ref(dir: &Path, ref_spec: &str) -> Result<String> {
    let value = resolve_toml_value_ref(dir, ref_spec)?;
    match value {
        TomlValue::String(v) => Ok(v),
        TomlValue::Boolean(v) => Ok(v.to_string()),
        TomlValue::Integer(v) => Ok(v.to_string()),
        TomlValue::Float(v) => Ok(v.to_string()),
        TomlValue::Datetime(v) => Ok(v.to_string()),
        _ => Err(anyhow::anyhow!(
            "ref `{ref_spec}` must resolve to a scalar field"
        )),
    }
}

fn load_review_rounds(dir: &Path) -> Result<Vec<(PathBuf, ReviewRound)>> {
    let review_rounds_dir = dir.join("review_rounds");
    if !review_rounds_dir.exists() {
        return Ok(vec![]);
    }
    let mut rounds = Vec::new();
    for entry in fs::read_dir(&review_rounds_dir)
        .with_context(|| format!("failed to read {}", review_rounds_dir.display()))?
    {
        let entry = entry.with_context(|| "failed to read review_rounds entry")?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let round = toml::from_slice(&bytes)
            .with_context(|| format!("failed to parse TOML {}", path.display()))?;
        rounds.push((path, round));
    }
    Ok(rounds)
}

fn validate_stage_promotion(
    dir: &Path,
    stage: Stage,
    stage_promotion: Option<&StagePromotion>,
    report: &mut Report,
) {
    let stage_key = stage_key(stage);
    match stage_promotion {
        Some(promotion) => {
            let matching_rows: Vec<_> = promotion
                .promotions
                .iter()
                .filter(|row| row.to_stage == stage_key)
                .collect();
            match matching_rows.as_slice() {
                [] => report.push(
                    Status::Block,
                    "scope",
                    "stage_promotion.toml",
                    format!(
                        "stage_promotion.toml does not define a promotion row for `{stage_key}`"
                    ),
                    "add a promotion row for the current deliverable stage",
                ),
                [row] => {
                    if row.from_stage.is_empty()
                        || row.promotion_gate_artifact.is_empty()
                        || row.status.is_empty()
                    {
                        report.push(
                            Status::Block,
                            "scope",
                            "stage_promotion.toml",
                            format!("stage promotion row for `{stage_key}` is incomplete"),
                            "fill from_stage, promotion_gate_artifact, and status for the active promotion row",
                        );
                        return;
                    }

                    let gate_artifact = dir.join(&row.promotion_gate_artifact);
                    if !gate_artifact.exists() {
                        report.push(
                            Status::Block,
                            "scope",
                            "stage_promotion.toml",
                            format!(
                                "promotion gate artifact `{}` does not exist for stage `{stage_key}`",
                                row.promotion_gate_artifact
                            ),
                            "bind the stage promotion to one real promotion_gate artifact",
                        );
                        return;
                    }

                    match load_from_path::<PromotionGate>(&gate_artifact) {
                        Ok(gates) => match gates.gates.as_slice() {
                            [] => report.push(
                                Status::Block,
                                "scope",
                                row.promotion_gate_artifact.clone(),
                                "promotion gate artifact contains no gates",
                                "declare exactly one gate in the promotion gate artifact",
                            ),
                            [gate] => {
                                let gate_base_invalid = gate.gate_id.is_empty()
                                    || gate.from_stage.is_empty()
                                    || gate.to_stage.is_empty()
                                    || gate.comparator_kind.is_empty()
                                    || gate.verdict.is_empty()
                                    || gate.status.is_empty();
                                if gate_base_invalid {
                                    report.push(
                                        Status::Block,
                                        "scope",
                                        row.promotion_gate_artifact.clone(),
                                        "promotion gate row is incomplete",
                                        "fill gate_id, from_stage, to_stage, comparator_kind, left_ref, one right-side expectation, verdict, and status",
                                    );
                                    return;
                                }
                                match gate.comparator_kind.as_str() {
                                    "all_of" => {
                                        if gate.clauses.is_empty() {
                                            report.push(
                                                Status::Block,
                                                "scope",
                                                row.promotion_gate_artifact.clone(),
                                                "promotion gate row is incomplete",
                                                "all_of gates must declare at least one clause",
                                            );
                                            return;
                                        }
                                    }
                                    "string_eq" | "scalar_eq" => {
                                        if gate.left_ref.is_empty()
                                            || (gate.right_ref.is_empty()
                                                && gate.right_literal.is_empty())
                                        {
                                            report.push(
                                                Status::Block,
                                                "scope",
                                                row.promotion_gate_artifact.clone(),
                                                "promotion gate row is incomplete",
                                                "string_eq and scalar_eq gates must declare left_ref and one right-side expectation",
                                            );
                                            return;
                                        }
                                    }
                                    "nonempty" => {
                                        if gate.left_ref.is_empty() {
                                            report.push(
                                                Status::Block,
                                                "scope",
                                                row.promotion_gate_artifact.clone(),
                                                "promotion gate row is incomplete",
                                                "nonempty gates must declare left_ref",
                                            );
                                            return;
                                        }
                                    }
                                    other => {
                                        report.push(
                                            Status::Block,
                                            "scope",
                                            row.promotion_gate_artifact.clone(),
                                            format!(
                                                "promotion gate `{}` uses unsupported comparator_kind `{}`",
                                                gate.gate_id, other
                                            ),
                                            "use a supported generic comparator kind",
                                        );
                                        return;
                                    }
                                }
                                if gate.from_stage != row.from_stage
                                    || gate.to_stage != row.to_stage
                                {
                                    report.push(
                                        Status::Block,
                                        "scope",
                                        row.promotion_gate_artifact.clone(),
                                        format!(
                                            "promotion gate `{}` does not match stage transition `{}` -> `{}`",
                                            gate.gate_id, row.from_stage, row.to_stage
                                        ),
                                        "bind the gate to the same stage transition as stage_promotion.toml",
                                    );
                                }
                                if gate.verdict != "pass" {
                                    report.push(
                                        Status::Block,
                                        "scope",
                                        row.promotion_gate_artifact.clone(),
                                        format!(
                                            "promotion gate `{}` has verdict `{}`",
                                            gate.gate_id, gate.verdict
                                        ),
                                        "only a promotion gate with verdict `pass` may advance a stage",
                                    );
                                }
                                let evaluate_eq = |left_ref: &str,
                                                   right_ref: &str,
                                                   right_literal: &str|
                                 -> Result<(String, String)> {
                                    let left_value = resolve_toml_scalar_ref(dir, left_ref)?;
                                    let right_value = if !right_ref.is_empty() {
                                        resolve_toml_scalar_ref(dir, right_ref)?
                                    } else {
                                        right_literal.to_string()
                                    };
                                    Ok((left_value, right_value))
                                };
                                let evaluate_nonempty = |value_ref: &str| -> Result<bool> {
                                    let value = resolve_toml_value_ref(dir, value_ref)?;
                                    let is_nonempty = match value {
                                        TomlValue::String(v) => !v.is_empty(),
                                        TomlValue::Array(v) => !v.is_empty(),
                                        TomlValue::Table(v) => !v.is_empty(),
                                        TomlValue::Boolean(_) => true,
                                        TomlValue::Integer(_) => true,
                                        TomlValue::Float(_) => true,
                                        TomlValue::Datetime(_) => true,
                                    };
                                    Ok(is_nonempty)
                                };
                                let mut seen_summary_artifacts = BTreeSet::new();
                                for summary_rel_path in scalar_summary_rel_path(&gate.left_ref)
                                    .into_iter()
                                    .chain(scalar_summary_rel_path(&gate.right_ref))
                                {
                                    validate_scalar_summary_artifact(
                                        dir,
                                        summary_rel_path,
                                        report,
                                        &mut seen_summary_artifacts,
                                    );
                                }
                                for clause in &gate.clauses {
                                    for summary_rel_path in
                                        scalar_summary_rel_path(&clause.left_ref)
                                            .into_iter()
                                            .chain(scalar_summary_rel_path(&clause.right_ref))
                                    {
                                        validate_scalar_summary_artifact(
                                            dir,
                                            summary_rel_path,
                                            report,
                                            &mut seen_summary_artifacts,
                                        );
                                    }
                                }

                                match gate.comparator_kind.as_str() {
                                    "string_eq" | "scalar_eq" => {
                                        match evaluate_eq(
                                            &gate.left_ref,
                                            &gate.right_ref,
                                            &gate.right_literal,
                                        ) {
                                            Ok((left_value, right_value)) => {
                                                if left_value != right_value {
                                                    report.push(
                                                        Status::Block,
                                                        "scope",
                                                        row.promotion_gate_artifact.clone(),
                                                        format!(
                                                            "promotion gate `{}` comparator failed: `{}` != `{}`",
                                                            gate.gate_id, left_value, right_value
                                                        ),
                                                        "fix the subject artifact, expected artifact, or gate binding before advancing the stage",
                                                    );
                                                }
                                            }
                                            Err(_) => report.push(
                                                Status::Block,
                                                "scope",
                                                row.promotion_gate_artifact.clone(),
                                                format!(
                                                    "promotion gate `{}` has an invalid scalar reference",
                                                    gate.gate_id
                                                ),
                                                "bind the gate to valid scalar refs or a literal",
                                            ),
                                        }
                                    }
                                    "nonempty" => match evaluate_nonempty(&gate.left_ref) {
                                        Ok(true) => {}
                                        Ok(false) => report.push(
                                            Status::Block,
                                            "scope",
                                            row.promotion_gate_artifact.clone(),
                                            format!(
                                                "promotion gate `{}` nonempty check failed for `{}`",
                                                gate.gate_id, gate.left_ref
                                            ),
                                            "populate the referenced field before advancing the stage",
                                        ),
                                        Err(_) => report.push(
                                            Status::Block,
                                            "scope",
                                            row.promotion_gate_artifact.clone(),
                                            format!(
                                                "promotion gate `{}` has an invalid nonempty ref",
                                                gate.gate_id
                                            ),
                                            "bind the gate to a valid ref for nonempty checks",
                                        ),
                                    },
                                    "all_of" => {
                                        for (idx, clause) in gate.clauses.iter().enumerate() {
                                            if clause.comparator_kind != "string_eq"
                                                && clause.comparator_kind != "scalar_eq"
                                                && clause.comparator_kind != "nonempty"
                                            {
                                                report.push(
                                                    Status::Block,
                                                    "scope",
                                                    row.promotion_gate_artifact.clone(),
                                                    format!(
                                                        "promotion gate `{}` clause {} uses unsupported comparator_kind `{}`",
                                                        gate.gate_id, idx, clause.comparator_kind
                                                    ),
                                                    "use only supported generic comparator kinds inside all_of",
                                                );
                                                continue;
                                            }
                                            if clause.left_ref.is_empty()
                                                || (clause.right_ref.is_empty()
                                                    && clause.right_literal.is_empty()
                                                        && clause.comparator_kind != "nonempty")
                                            {
                                                report.push(
                                                    Status::Block,
                                                    "scope",
                                                    row.promotion_gate_artifact.clone(),
                                                    format!(
                                                        "promotion gate `{}` clause {} is incomplete",
                                                        gate.gate_id, idx
                                                    ),
                                                    "fill left_ref and one right-side expectation for every all_of clause",
                                                );
                                                continue;
                                            }
                                            match clause.comparator_kind.as_str() {
                                                "string_eq" | "scalar_eq" => match evaluate_eq(
                                                    &clause.left_ref,
                                                    &clause.right_ref,
                                                    &clause.right_literal,
                                                ) {
                                                    Ok((left_value, right_value)) => {
                                                        if left_value != right_value {
                                                            report.push(
                                                                Status::Block,
                                                                "scope",
                                                                row.promotion_gate_artifact.clone(),
                                                                format!(
                                                                    "promotion gate `{}` clause {} failed: `{}` != `{}`",
                                                                    gate.gate_id, idx, left_value, right_value
                                                                ),
                                                                "fix the subject artifact, expected artifact, or gate binding before advancing the stage",
                                                            );
                                                        }
                                                    }
                                                    Err(_) => report.push(
                                                        Status::Block,
                                                        "scope",
                                                        row.promotion_gate_artifact.clone(),
                                                        format!(
                                                            "promotion gate `{}` clause {} has an invalid scalar reference",
                                                            gate.gate_id, idx
                                                        ),
                                                        "bind every all_of clause to valid scalar refs or a literal",
                                                    ),
                                                },
                                                "nonempty" => match evaluate_nonempty(&clause.left_ref) {
                                                    Ok(true) => {}
                                                    Ok(false) => report.push(
                                                        Status::Block,
                                                        "scope",
                                                        row.promotion_gate_artifact.clone(),
                                                        format!(
                                                            "promotion gate `{}` clause {} nonempty check failed for `{}`",
                                                            gate.gate_id, idx, clause.left_ref
                                                        ),
                                                        "populate the referenced field before advancing the stage",
                                                    ),
                                                    Err(_) => report.push(
                                                        Status::Block,
                                                        "scope",
                                                        row.promotion_gate_artifact.clone(),
                                                        format!(
                                                            "promotion gate `{}` clause {} has an invalid nonempty ref",
                                                            gate.gate_id, idx
                                                        ),
                                                        "bind every all_of clause to a valid ref for nonempty checks",
                                                    ),
                                                },
                                                _ => {}
                                            }
                                        }
                                    }
                                    other => report.push(
                                        Status::Block,
                                        "scope",
                                        row.promotion_gate_artifact.clone(),
                                        format!(
                                            "promotion gate `{}` uses unsupported comparator_kind `{}`",
                                            gate.gate_id, other
                                        ),
                                        "use a supported generic comparator kind",
                                    ),
                                }
                            }
                            _ => report.push(
                                Status::Block,
                                "scope",
                                row.promotion_gate_artifact.clone(),
                                "promotion gate artifact defines multiple gates",
                                "declare exactly one gate in the promotion gate artifact",
                            ),
                        },
                        Err(_) => report.push(
                            Status::Block,
                            "schema",
                            row.promotion_gate_artifact.clone(),
                            "stage promotion points to an unreadable promotion gate artifact",
                            "make the declared promotion gate artifact exist and parse as TOML",
                        ),
                    }
                }
                _ => report.push(
                    Status::Block,
                    "scope",
                    "stage_promotion.toml",
                    format!(
                        "stage_promotion.toml defines multiple promotion rows for `{stage_key}`"
                    ),
                    "declare exactly one promotion row for the active stage",
                ),
            }
        }
        None => report.push(
            Status::Block,
            "schema",
            "stage_promotion.toml",
            format!("selected stage `{stage_key}` is missing stage_promotion.toml"),
            "add stage_promotion.toml and bind it to one promotion_gate artifact",
        ),
    }
}

pub fn validate_dir(dir: &Path, stage: Stage) -> Result<Report> {
    if !dir.exists() {
        bail!("delivery directory does not exist: {}", dir.display());
    }

    let issue_contract = load_optional::<IssueContract>(dir, "issue_contract.toml")?;
    let seam_contract = load_optional::<SeamContract>(dir, "seam_contract.toml")?;
    let proof_plan = load_optional::<ProofPlan>(dir, "proof_plan.toml")?;
    let finding_ledger = load_optional::<FindingLedger>(dir, "finding_ledger.toml")?;
    let evidence_bundle = load_optional::<EvidenceBundle>(dir, "evidence_bundle.toml")?;
    let merge_claims = load_optional::<MergeClaims>(dir, "merge_claims.toml")?;
    let claim_enforcement_coverage =
        load_optional::<ClaimEnforcementCoverageSummary>(dir, "claim_enforcement_coverage.toml")?;
    let orchestration_reachability_summary = load_optional::<OrchestrationReachabilitySummary>(
        dir,
        "orchestration_reachability_summary.toml",
    )?;
    let review_target = load_optional::<ReviewTarget>(dir, "review_target.toml")?;
    let execution_target = load_optional::<ExecutionTarget>(dir, "execution_target.toml")?;
    let ci_surface = load_optional::<CiSurface>(dir, "ci_surface.toml")?;
    let claim_enforcement = load_optional::<ClaimEnforcement>(dir, "claim_enforcement.toml")?;
    let assumption_register = load_optional::<AssumptionRegister>(dir, "assumption_register.toml")?;
    let review_rounds = load_review_rounds(dir)?;
    let stage_promotion = load_optional::<StagePromotion>(dir, "stage_promotion.toml")?;
    let orchestration_reachability =
        load_optional::<OrchestrationReachability>(dir, "orchestration_reachability.toml")?;
    let review_target_present = review_target.is_some();

    if issue_contract.is_none()
        && seam_contract.is_none()
        && proof_plan.is_none()
        && finding_ledger.is_none()
        && evidence_bundle.is_none()
        && merge_claims.is_none()
    {
        bail!("no known artifact files found under {}", dir.display());
    }

    let mut report = Report::default();

    if let Some(issue) = &issue_contract {
        if issue.required_outcomes.is_empty() {
            report.push(
                Status::Block,
                "scope",
                "issue_contract.toml",
                "required_outcomes is empty",
                "declare at least one required outcome",
            );
        }
        if issue.non_goals.is_empty() {
            report.push(
                Status::Block,
                "scope",
                "issue_contract.toml",
                "non_goals is empty",
                "declare at least one non-goal",
            );
        }
        if issue.allowed_surfaces.is_empty() {
            report.push(
                Status::Block,
                "scope",
                "issue_contract.toml",
                "allowed_surfaces is empty",
                "declare allowed surfaces",
            );
        }
        let allowed: BTreeSet<_> = issue.allowed_surfaces.iter().collect();
        let forbidden: BTreeSet<_> = issue.forbidden_surfaces.iter().collect();
        if !allowed.is_disjoint(&forbidden) {
            report.push(
                Status::Block,
                "scope",
                "issue_contract.toml",
                "allowed_surfaces and forbidden_surfaces overlap",
                "remove the overlap so the scope is unambiguous",
            );
        }
        let lowered = issue.problem_statement.to_lowercase();
        if lowered.contains("fix by") || lowered.contains("implement by") {
            report.push(
                Status::Block,
                "scope",
                "issue_contract.toml",
                "problem_statement contains fix-shaped language",
                "keep the issue contract problem-only",
            );
        }
    }

    validate_stage_promotion(dir, stage, stage_promotion.as_ref(), &mut report);

    if review_target_present
        && issue_contract.is_some()
        && seam_contract.is_some()
        && proof_plan.is_some()
        && matches!(stage, Stage::Review | Stage::MergeCandidate)
    {
        match &execution_target {
            Some(_) => {}
            None => report.push(
                Status::Block,
                "schema",
                "execution_target.toml",
                "review-stage package is missing execution_target.toml",
                "add execution_target.toml to bind the package to the exact implementation head",
            ),
        }

        match &ci_surface {
            Some(_) => {}
            None => report.push(
                Status::Block,
                "schema",
                "ci_surface.toml",
                "review-stage package is missing ci_surface.toml",
                "add ci_surface.toml to define the exact CI proof surface for this deliverable",
            ),
        }

        match &claim_enforcement {
            Some(enforcement) => {
                if enforcement.rows.is_empty() {
                    report.push(
                        Status::Block,
                        "schema",
                        "claim_enforcement.toml",
                        "claim_enforcement.toml is present but empty",
                        "add enforcement rows for the claims this package asserts as true",
                    );
                }
            }
            None => report.push(
                Status::Block,
                "schema",
                "claim_enforcement.toml",
                "review-stage package is missing claim_enforcement.toml",
                "add claim_enforcement.toml to bind true claims to real enforcement loci",
            ),
        }
    }

    if let Some(contract) = &seam_contract {
        for (idx, seam) in contract.seams.iter().enumerate() {
            let forbidden: BTreeSet<_> = seam.forbidden_sources.iter().collect();
            let fallback: BTreeSet<_> = seam.fallback_order.iter().collect();
            if !forbidden.is_disjoint(&fallback) {
                report.push(
                    Status::Block,
                    "semantic",
                    format!("seam_contract.toml / seams[{idx}]"),
                    format!(
                        "forbidden_sources and fallback_order overlap for semantic term `{}`",
                        seam.semantic_term
                    ),
                    "remove forbidden sources from fallback_order",
                );
            }
            if contract.status == "locked" && seam.authoritative_source == "UNFROZEN" {
                report.push(
                    Status::Block,
                    "semantic",
                    format!("seam_contract.toml / seams[{idx}]"),
                    format!(
                        "locked seam `{}` still has authoritative_source `UNFROZEN`",
                        seam.semantic_term
                    ),
                    "freeze one authoritative source before proceeding",
                );
            }
        }

        if execution_target.is_some()
            && ci_surface.is_some()
            && orchestration_reachability.is_none()
        {
            report.push(
                Status::Block,
                "proof",
                "orchestration_reachability.toml",
                "review-stage package is missing orchestration_reachability.toml",
                "add orchestration_reachability.toml to declare the critical fallback and fast-path reachability cases",
            );
        }
    }

    let open_findings = finding_ledger
        .as_ref()
        .map(|ledger| {
            ledger
                .findings
                .iter()
                .filter(|finding| finding.status == "open")
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for finding in &open_findings {
        match finding.kind.as_str() {
            "semantic_ambiguity" => {
                let seam_where = seam_contract
                    .as_ref()
                    .and_then(|contract| {
                        contract.seams.iter().enumerate().find_map(|(idx, seam)| {
                            (seam.semantic_term == finding.subject)
                                .then_some((idx, seam))
                        })
                    })
                    .map(|(idx, seam)| {
                        let why = if seam.authoritative_source == "UNFROZEN" {
                            format!(
                                "storage_field `{}` maps to unresolved authoritative_source `UNFROZEN`",
                                if seam.storage_field.is_empty() {
                                    finding.subject.as_str()
                                } else {
                                    seam.storage_field.as_str()
                                }
                            )
                        } else {
                            format!("semantic term `{}` remains ambiguous", finding.subject)
                        };
                        (
                            format!("seam_contract.toml / seams[{idx}]"),
                            why,
                            "freeze one authoritative source and list forbidden fallbacks explicitly"
                                .to_string(),
                        )
                    })
                    .unwrap_or((
                        "finding_ledger.toml".to_string(),
                        format!("semantic ambiguity remains open for `{}`", finding.subject),
                        "freeze the seam before proceeding".to_string(),
                    ));
                report.push(
                    Status::Block,
                    "semantic",
                    seam_where.0,
                    seam_where.1,
                    seam_where.2,
                );
            }
            "artifact_mismatch" => {
                let refs = if finding.source_refs.is_empty() {
                    "unknown evidence refs".to_string()
                } else {
                    finding.source_refs.join(" + ")
                };
                report.push(
                    Status::Block,
                    "evidence",
                    format!("evidence_bundle.toml / {refs}"),
                    "artifact mismatch remains unresolved",
                    "freeze which evidence source is authoritative or keep the deliverable blocked",
                );
            }
            _ => {
                report.push(
                    Status::Block,
                    "finding",
                    if finding.locus.is_empty() {
                        "finding_ledger.toml".to_string()
                    } else {
                        finding.locus.clone()
                    },
                    format!("open finding remains unresolved: {}", finding.finding_id),
                    "close, defer, or invalidate the finding explicitly",
                );
            }
        }
    }

    if let Some(ledger) = &finding_ledger {
        let has_environment_assumption = ledger
            .findings
            .iter()
            .any(|finding| finding.kind == "environment_assumption");

        if has_environment_assumption {
            match &assumption_register {
                Some(register) => {
                    if register.assumptions.is_empty() {
                        report.push(
                            Status::Block,
                            "schema",
                            "assumption_register.toml",
                            "assumption_register.toml is present but empty",
                            "add assumption rows for environment or trust-boundary findings",
                        );
                    } else {
                        let subjects: BTreeSet<_> = register
                            .assumptions
                            .iter()
                            .map(|row| row.subject.as_str())
                            .collect();
                        for finding in &ledger.findings {
                            if finding.kind == "environment_assumption" {
                                if !subjects.contains(finding.subject.as_str()) {
                                    report.push(
                                        Status::Block,
                                        "evidence",
                                        "assumption_register.toml",
                                        format!(
                                            "environment assumption `{}` has no matching register row",
                                            finding.subject
                                        ),
                                        "add an assumption row with the same subject and trust metadata",
                                    );
                                }
                            }
                        }
                        for row in &register.assumptions {
                            if row.assumption_id.is_empty()
                                || row.impact_class.is_empty()
                                || row.subject.is_empty()
                                || row.description.is_empty()
                                || row.trust_root.is_empty()
                                || row.monitor.is_empty()
                                || row.expiry_trigger.is_empty()
                                || row.status.is_empty()
                            {
                                report.push(
                                    Status::Block,
                                    "schema",
                                    "assumption_register.toml",
                                    format!(
                                        "assumption row for `{}` is incomplete",
                                        row.subject
                                    ),
                                    "fill all required assumption fields before proceeding",
                                );
                            }
                        }
                    }
                }
                None => report.push(
                    Status::Block,
                    "schema",
                    "assumption_register.toml",
                    "finding ledger includes environment assumptions but assumption_register.toml is missing",
                    "add assumption_register.toml to hold trust-boundary assumptions in structured state",
                ),
            }
        }

        for finding in &ledger.findings {
            if finding.status == "resolved" && finding.resolution_kind.is_empty() {
                report.push(
                    Status::Block,
                    "finding",
                    "finding_ledger.toml",
                    format!(
                        "resolved finding has no resolution_kind: {}",
                        finding.finding_id
                    ),
                    "assign exactly one terminal disposition",
                );
            }
            if finding.kind == "review_target_mismatch" || finding.kind == "stale_review" {
                report.push(
                    Status::Warn,
                    "review_target",
                    "process decomposition",
                    "stale review-target artifacts are not owned by proof_plan and must be filtered by the review_target gate instead",
                    if review_target_present {
                        "none".to_string()
                    } else {
                        "enforce review_target.toml whenever review-derived findings are present"
                            .to_string()
                    },
                );
            }
        }
    }

    if let (Some(merge), Some(enforcement), Some(summary)) = (
        &merge_claims,
        &claim_enforcement,
        &claim_enforcement_coverage,
    ) {
        let recomputed = compute_claim_enforcement_coverage_summary(merge, enforcement);
        if *summary != recomputed {
            report.push(
                Status::Block,
                "evidence",
                "claim_enforcement_coverage.toml",
                "claim_enforcement coverage summary does not match recomputed producer output",
                "recompute claim_enforcement_coverage.toml from merge_claims.toml and claim_enforcement.toml",
            );
        }
    }

    if let (Some(surface), Some(reachability), Some(summary)) = (
        &ci_surface,
        &orchestration_reachability,
        &orchestration_reachability_summary,
    ) {
        let recomputed = compute_orchestration_reachability_summary(stage, surface, reachability);
        if *summary != recomputed {
            report.push(
                Status::Block,
                "evidence",
                "orchestration_reachability_summary.toml",
                "orchestration reachability summary does not match recomputed producer output",
                "recompute orchestration_reachability_summary.toml from orchestration_reachability.toml and ci_surface.toml",
            );
        }
    }

    if review_target_present
        && issue_contract.is_some()
        && seam_contract.is_some()
        && proof_plan.is_some()
        && matches!(stage, Stage::Review | Stage::MergeCandidate)
    {
        let has_external_review_evidence = evidence_bundle.as_ref().is_some_and(|bundle| {
            bundle
                .evidence
                .iter()
                .any(|e| e.type_name == "external_artifact" && e.producer == "github_review")
        });

        if has_external_review_evidence {
            if review_rounds.is_empty() {
                report.push(
                    Status::Block,
                    "review_target",
                    "review_rounds/",
                    "review-stage package has external review evidence but no review_rounds artifacts",
                    "add review_rounds/<round_id>.toml to record ingestion of the exact review corpus",
                );
            }
        }
    }

    if let Some(proof) = &proof_plan {
        for claim in &proof.claims {
            if claim.required_before.is_empty() {
                report.push(
                    Status::Block,
                    "proof",
                    "proof_plan.toml",
                    format!("claim {} is missing required_before", claim.claim_id),
                    "declare when the claim must be discharged",
                );
            }
            if claim.falsified_by.is_empty() {
                report.push(
                    Status::Block,
                    "proof",
                    "proof_plan.toml",
                    format!("claim {} has no falsifier", claim.claim_id),
                    "add at least one falsifier",
                );
            }
        }
    }

    if let (Some(merge), Some(evidence)) = (&merge_claims, &evidence_bundle) {
        let evidence_ids: BTreeSet<_> = evidence
            .evidence
            .iter()
            .map(|e| e.evidence_id.as_str())
            .collect();
        for required in &merge.required_evidence {
            if !evidence_ids.contains(required.as_str()) {
                report.push(
                    Status::Block,
                    "evidence",
                    "merge_claims.toml",
                    format!("required evidence ref is missing: {required}"),
                    "add the missing evidence row or remove the bad reference",
                );
            }
        }
        for claim in &merge.claims {
            if stage == Stage::MergeCandidate && claim.value && claim.supported_by.is_empty() {
                report.push(
                    Status::Block,
                    "merge_claim",
                    "merge_claims.toml",
                    "true merge claim has no supporting evidence",
                    "attach evidence before asserting the claim as true",
                );
            }
            for evidence_ref in &claim.supported_by {
                if !evidence_ids.contains(evidence_ref.as_str()) {
                    report.push(
                        Status::Block,
                        "merge_claim",
                        "merge_claims.toml",
                        format!("merge claim references missing evidence: {evidence_ref}"),
                        "add the evidence row or remove the bad reference",
                    );
                }
            }
        }
        if merge.merge_ready && !merge.open_blockers.is_empty() {
            report.push(
                Status::Block,
                "merge_claim",
                "merge_claims.toml",
                "merge_ready is true while open_blockers is non-empty",
                "clear the blockers or keep merge_ready false",
            );
        }
    }

    if !report.has_block() {
        if proof_plan.is_some() {
            report.push(
                Status::Pass,
                "proof",
                "proof_plan.toml",
                "schema-boundary, legacy-compatibility, fail-closed legacy behavior, and bounded slug-fetch behavior are all represented as explicit claims with falsifiers",
                "none",
            );
        } else if finding_ledger.is_some() {
            report.push(
                Status::Pass,
                "finding",
                "finding_ledger.toml",
                "repeated wording collapsed into canonical findings and remaining states are structurally consistent",
                "none",
            );
        } else {
            report.push(
                Status::Pass,
                "schema",
                "artifact package",
                "artifact package is structurally valid for the selected stage",
                "none",
            );
        }
    }

    Ok(report)
}

pub fn render_report(report: &Report) -> String {
    let mut out = String::new();
    for (idx, message) in report.messages.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "STATUS: {}\nKIND: {}\nWHERE: {}\nWHY: {}\nNEXT: {}\n",
            message.status.as_str(),
            message.kind,
            message.where_,
            message.why,
            message.next
        ));
    }
    out
}
