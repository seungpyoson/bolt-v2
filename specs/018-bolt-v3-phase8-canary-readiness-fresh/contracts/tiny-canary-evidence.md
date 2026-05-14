# Tiny Canary Evidence Contract

## Purpose

Define the redacted evidence shape Phase 8 must write before and during any approved tiny canary attempt. This contract does not permit live order execution by itself.

## JSON Shape

```json
{
  "schema_version": 1,
  "head_sha": "40-hex-char git sha",
  "root_config_sha256": "64-hex-char sha256",
  "ssm_manifest_sha256": "64-hex-char sha256",
  "approval_id_hash": "64-hex-char sha256",
  "max_live_order_count": 1,
  "max_notional_per_order": "1.00",
  "decision_evidence_ref": {
    "path_hash": "64-hex-char sha256",
    "record_hash": "64-hex-char sha256"
  },
  "submit_admission_ref": {
    "status": "accepted_or_rejected",
    "admitted_order_count": 0,
    "reason": "redacted diagnostic"
  },
  "runtime_capture_ref": {
    "spool_root_hash": "64-hex-char sha256",
    "run_id": "redacted stable id"
  },
  "nt_lifecycle_refs": [],
  "outcome": "blocked_before_submit",
  "block_reasons": [
    "decision_evidence_unavailable"
  ]
}
```

## Required Invariants

- `max_live_order_count` must equal the configured live canary cap.
- `max_notional_per_order` must equal the configured live canary cap string.
- `approval_id_hash` must be a hash, not the raw approval id, unless the approval id is explicitly approved for display in the PR.
- `decision_evidence_ref` must exist before any `nt_order_*` outcome.
- `submit_admission_ref` must exist before any `nt_order_*` outcome.
- `nt_lifecycle_refs` must cite NT event/report/capture evidence for every live outcome.
- Blocked outcomes must not require NT lifecycle references.
- `block_reasons` must be non-empty for blocked outcomes.
- `blocked_before_live_order`, `decision_evidence_unavailable`, and
  `strategy_input_safety_audit_blocked` are `block_reasons`, not `outcome`
  values.
- No raw API key, private key, secret value, passphrase, or raw SSM value may appear.

## Forbidden Evidence Sources

- Mock venue worlds as live proof.
- Bolt-synthesized order status reports.
- Bolt-owned reconciliation reports.
- Direct exec-engine cancel commands from the operator harness.
- Env-var credential values.
- Unbounded or unredacted log dumps.
