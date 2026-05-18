# Data Model: Phase 9 Audit Artifacts

## AuditFinding

- `id`: stable identifier, for example `P9-BLOCKER-001`
- `severity`: `blocker`, `high`, `medium`, or `low`
- `category`: audit category from Phase 9 scope
- `claim`: concise finding
- `evidence`: one or more `EvidenceCitation`
- `impact`: readiness or implementation impact
- `recommendation`: exact next action
- `status`: `open`, `accepted`, `disproved`, or `closed`

## EvidenceCitation

- `kind`: `file-line`, `command-output`, `test-output`, `pr-metadata`, or `reviewer-job`
- `reference`: path and line, command string, job id, or PR id
- `summary`: short evidence text
- `captured_at`: date or branch head when captured

## CleanupCandidate

- `id`: stable identifier
- `scope`: files or modules proposed for cleanup
- `risk`: what behavior could change
- `required_test`: public behavior test or source fence required before edit
- `review_gate`: reviewers required before implementation
- `decision`: `blocked`, `approved-for-plan-review`, or `implemented`

## ExternalReviewDisposition

- `provider`: Claude, DeepSeek, GLM, or another available reviewer
- `job_id`: reviewer job id or failure id
- `source_transmission`: approval-token evidence for direct API reviewers
- `status`: `approve`, `approve-with-findings`, `request-changes`, `needs-info`, `failed`, or `blocked`
- `status_mapping`: external prompt verdicts map as follows:
  `Verdict: APPROVE` maps to `approve` or `approve-with-findings`;
  `Verdict: REQUEST_CHANGES` maps to `request-changes`;
  `Verdict: NEEDS_INFO` maps to `needs-info`; `failed` records tool or
  review-quality failure; `blocked` records source-access or review-infrastructure
  blockage before a substantive verdict.
- `findings`: list of findings with accept/disprove/defer decision
- `blocking`: whether implementation may proceed

## Synthetic Example Instances

These examples map the schema to current Phase 9 artifacts; they are not new
findings. Example job ids are synthetic unless they are recorded in a PR
evidence comment.

```text
AuditFinding {
  id: "P9-BLOCKER-001",
  severity: "blocker",
  category: "No-submit live evidence",
  evidence: ["specs/001-thin-live-canary-path/tasks.md T038"],
  status: "open"
}

ExternalReviewDisposition {
  provider: "DeepSeek",
  job_id: "job_55d503cf-104a-40d1-a5e0-37ac9a68966b",
  source_transmission: "sent_after_user_approval",
  status: "approve-with-findings",
  blocking: false
}
```
