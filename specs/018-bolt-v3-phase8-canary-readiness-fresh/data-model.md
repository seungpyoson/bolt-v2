# Phase 8 Data Model

## Phase8CanaryPreflight

Redacted local status object written before any dry or live canary action.

Fields:

- `head_sha`: exact git commit under evaluation.
- `main_sha`: source-of-truth main SHA used for planning.
- `phase7_dependency_status`: `missing_on_main`, `present`, or `explicit_stacked_base_approved`.
- `no_submit_report_status`: `missing`, `present_unverified`, `accepted_by_gate`, or `rejected_by_gate`.
- `strategy_input_audit_status`: `blocked`, `approved`, or `not_run`.
- `live_canary_gate_status`: `not_run`, `accepted`, or `rejected`.
- `block_reasons`: non-empty list when live action is blocked.

Validation:

- `block_reasons` must be non-empty unless every status is approved or accepted.
- No raw secret values or raw SSM values are allowed.
- Paths may be present only when operator-approved and non-secret; hashes are preferred.

## Phase8CanaryEvidence

Redacted evidence object for dry/no-submit proof or live order result.

Fields:

- `head_sha`
- `root_config_sha256`
- `ssm_manifest_sha256`
- `approval_id_hash`
- `max_live_order_count`
- `max_notional_per_order`
- `decision_evidence_ref`
- `submit_admission_ref`
- `runtime_capture_ref`
- `nt_lifecycle_refs`
- `outcome`
- `block_reasons`

Allowed `outcome` values:

- `blocked_before_build`
- `blocked_before_runner`
- `blocked_before_submit`
- `dry_no_submit_proof`
- `nt_order_submitted`
- `nt_order_rejected`
- `nt_order_accepted`
- `nt_order_filled`
- `nt_strategy_cancel_observed`
- `nt_restart_reconciliation_observed`

Validation:

- `outcome` values beginning with `nt_` require references to NT event/report/capture artifacts.
- `dry_no_submit_proof` must have no live submit reference.
- Live outcomes must include a matching decision evidence reference and submit admission reference.
- Evidence must be serializable without raw secret fields.

## StrategyInputSafetyAudit

Redacted audit object for the mandatory strategy-input safety gate.

Fields:

- `strategy_kind`
- `recommendation`: `approve` or `block`
- `chainlink_feed_status`
- `reference_venue_status`
- `volatility_model_status`
- `pricing_kurtosis_status`
- `theta_decay_status`
- `fee_rebate_status`
- `market_selection_status`
- `edge_economics_status`
- `liquidity_spread_status`
- `evidence_refs`
- `blockers`

Validation:

- Any `*_status = blocked` forces `recommendation = block`.
- `approve` requires every required review item to include source-backed evidence.

## OperatorApprovalEnvelope

Non-secret operator values for ignored harness only.

Fields:

- `head_sha`
- `root_toml_path`
- `root_toml_sha256`
- `ssm_manifest_sha256`
- `operator_approval_id`
- `canary_evidence_path`

Validation:

- Missing field blocks before build.
- `head_sha` must match current git head.
- `root_toml_sha256` must match the file at `root_toml_path`.
- `operator_approval_id` must match `[live_canary].approval_id`.
