# Data Model: Production Live Readiness

## ReadinessLevel

- **Fields**:
  - `name`: `tiny-canary ready`, `staged live ready`, or `production live ready`
  - `allowed_claim`: exact language permitted by current evidence
  - `blocked_claims`: broader language forbidden until promotion evidence exists
  - `required_evidence`: ordered list of evidence package entries
- **Validation**:
  - Every PR, issue, runbook, or status update that claims readiness must name one level.
  - A broader level is invalid if any required evidence is missing, stale, or unwaived.

## PromotionEvidencePackage

- **Fields**:
  - `reviewed_commit_sha`
  - `root_toml_path_hash`
  - `root_toml_record_hash`
  - `binary_build_provenance`
  - `host_identity_proof`
  - `ssm_manifest_ref`
  - `operator_approval_ref`
  - `no_submit_readiness_ref`
  - `strategy_input_ref`
  - `financial_envelope_ref`
  - `pre_run_state_ref`
  - `live_order_refs`
  - `monitoring_alerting_ref`
  - `residual_blockers`
  - `operator_waivers`
- **Validation**:
  - Raw secrets, private keys, raw approval ids, and account balances invalidate the package.
  - Staged-live and production-live packages must include live-order, monitoring, and deploy provenance evidence.

## OperatorRunbook

- **Fields**:
  - `runbook_kind`: repeated-live operation, abort, restart recovery, or post-run hygiene
  - `preconditions`
  - `operator_actions`
  - `evidence_outputs`
  - `failure_blocks`
- **Validation**:
  - Staged-live readiness is blocked unless all four runbook kinds are linked.
  - Runbooks must not replace the live-canary gate or approval gate.

## ProductionClaimBlocker

- **Fields**:
  - `blocker_kind`: missing evidence, stale evidence, unresolved status-map row, reviewer finding, CI failure, or explicit scope mismatch
  - `source_reference`
  - `waiver_reference`
- **Validation**:
  - Production-live readiness is blocked while any blocker lacks an explicit operator waiver.
