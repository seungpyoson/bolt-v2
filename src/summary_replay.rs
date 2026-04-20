use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ClaimEnforcementCoverageSummary {
    pub status: String,
    pub summary_kind: String,
    pub summary_verdict: String,
    pub subject: String,
    pub source_refs: Vec<String>,
    pub rule_version: String,
    pub covered_true_claim_count: i64,
    pub uncovered_true_claim_count: i64,
}

#[derive(Debug, Clone)]
pub struct ClaimEnforcementRowInput {
    pub claim_id: String,
    pub enforcement_kind: String,
    pub enforced_at: String,
    pub status: String,
}

pub fn compute_claim_enforcement_coverage_summary(
    claim_values: &[(String, bool)],
    enforcement_rows: &[ClaimEnforcementRowInput],
) -> ClaimEnforcementCoverageSummary {
    let rows_by_claim: BTreeMap<&str, &ClaimEnforcementRowInput> = enforcement_rows
        .iter()
        .filter(|row| !row.claim_id.is_empty())
        .map(|row| (row.claim_id.as_str(), row))
        .collect();

    let mut covered_true_claim_count = 0_i64;
    let mut uncovered_true_claim_count = 0_i64;
    for (claim_id, value) in claim_values {
        if !value {
            continue;
        }
        match rows_by_claim.get(claim_id.as_str()) {
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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OrchestrationReachabilitySummary {
    pub stage: String,
    pub summary_kind: String,
    pub summary_verdict: String,
    pub source_refs: Vec<String>,
    pub rule_version: String,
    pub out_of_surface_required_job_count: i64,
    pub incomplete_case_count: i64,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ReachabilityCaseInput {
    pub case_id: String,
    pub subject: String,
    pub trigger_job: String,
    pub trigger_result: String,
    pub required_reachable_jobs: Vec<String>,
    pub forbidden_job_results: Vec<String>,
    pub proof_ref: String,
    pub status: String,
}

pub fn compute_orchestration_reachability_summary(
    stage_name: &str,
    stage_jobs: &BTreeSet<String>,
    cases: &[ReachabilityCaseInput],
) -> OrchestrationReachabilitySummary {
    let mut incomplete_case_count = 0_i64;
    let mut out_of_surface_required_job_count = 0_i64;

    for case in cases {
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
            .filter(|job| !stage_jobs.contains(*job))
            .count() as i64;
    }

    let summary_verdict = if incomplete_case_count == 0 && out_of_surface_required_job_count == 0 {
        "pass"
    } else {
        "block"
    };

    OrchestrationReachabilitySummary {
        stage: stage_name.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_enforcement_summary_counts_true_claim_coverage() {
        let claims = vec![
            ("C1".to_string(), true),
            ("C2".to_string(), false),
            ("C3".to_string(), true),
        ];
        let rows = vec![ClaimEnforcementRowInput {
            claim_id: "C1".to_string(),
            enforcement_kind: "workflow".to_string(),
            enforced_at: "path".to_string(),
            status: "bound".to_string(),
        }];

        let summary = compute_claim_enforcement_coverage_summary(&claims, &rows);
        assert_eq!(summary.covered_true_claim_count, 1);
        assert_eq!(summary.uncovered_true_claim_count, 1);
        assert_eq!(summary.summary_verdict, "block");
    }

    #[test]
    fn reachability_summary_counts_out_of_surface_jobs() {
        let mut stage_jobs = BTreeSet::new();
        stage_jobs.insert("fmt-check".to_string());
        let cases = vec![ReachabilityCaseInput {
            case_id: "R1".to_string(),
            subject: "subject".to_string(),
            trigger_job: "same_sha_proof".to_string(),
            trigger_result: "failure".to_string(),
            required_reachable_jobs: vec!["fmt-check".to_string(), "build".to_string()],
            forbidden_job_results: vec!["skipped".to_string()],
            proof_ref: "proof".to_string(),
            status: "declared".to_string(),
        }];

        let summary = compute_orchestration_reachability_summary("review", &stage_jobs, &cases);
        assert_eq!(summary.out_of_surface_required_job_count, 1);
        assert_eq!(summary.incomplete_case_count, 0);
        assert_eq!(summary.summary_verdict, "block");
    }

    #[test]
    fn reachability_summary_blocks_incomplete_case() {
        let stage_jobs = BTreeSet::new();
        let cases = vec![ReachabilityCaseInput {
            case_id: "".to_string(),
            subject: "subject".to_string(),
            trigger_job: "same_sha_proof".to_string(),
            trigger_result: "failure".to_string(),
            required_reachable_jobs: vec!["fmt-check".to_string()],
            forbidden_job_results: vec!["skipped".to_string()],
            proof_ref: "proof".to_string(),
            status: "declared".to_string(),
        }];

        let summary = compute_orchestration_reachability_summary("review", &stage_jobs, &cases);
        assert_eq!(summary.incomplete_case_count, 1);
        assert_eq!(summary.summary_verdict, "block");
    }
}
