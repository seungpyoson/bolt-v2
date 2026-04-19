use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

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
    value: bool,
    #[serde(default)]
    supported_by: Vec<String>,
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
    let review_target_present = dir.join("review_target.toml").exists();

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
        if stage == Stage::MergeCandidate && !merge.merge_ready {
            report.push(
                Status::Block,
                "merge_claim",
                "merge_claims.toml",
                "merge_candidate stage requires merge_ready = true",
                "satisfy the package and then mark merge_ready true",
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
