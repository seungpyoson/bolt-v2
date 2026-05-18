# Phase 7 Data Model

## NoSubmitReadinessReport

Purpose: Redacted JSON artifact consumed by the live-canary gate.

Fields:

- `schema_version`: Operator-safe schema version string.
- `approval_id_hash`: Full lowercase SHA-256 hex digest of `[live_canary].approval_id`.
- `executable_identity`: Full lowercase SHA-256 hex digest of the current executable bytes.
- `config_bundle_checksum`: Non-secret checksum of the exact loaded root and strategy TOML bytes.
- `stages`: Ordered list of `NoSubmitReadinessStage`.

Validation:

- Report must be a JSON object.
- `stages` must be non-empty.
- Every required stage must be present and satisfied for live-canary acceptance.
- Report size must remain within `[live_canary].max_no_submit_readiness_report_bytes`.
- Resolved credential values must never appear in serialized or debug output.
- Raw approval id must never appear in serialized or debug output when `approval_id_hash` is present.
- Live-canary gate must compare all linkage fields against current runtime state before accepting the report.

## NoSubmitReadinessStage

Purpose: One readiness observation.

Fields:

- `stage`: Stable stage key.
- `status`: `satisfied`, `failed`, or `skipped`.
- `detail`: Redacted operator-safe detail.

Required stage set:

- `operator_approval`
- `secret_resolution`
- `live_node_build`
- `controlled_connect`
- `reference_readiness`
- `controlled_disconnect`
- `report_write`

Validation:

- Missing required stage fails closed.
- Any `failed` or `skipped` required stage fails closed.
- Detail must be redacted before serialization.

## OperatorApproval

Purpose: Config-owned approval input for side-effect-bearing real no-submit readiness.

Fields:

- `approval_id`: Non-secret value from `[live_canary].approval_id`.

Validation:

- Missing or whitespace approval id fails before secret resolution.
- Approval id is not a credential and does not allow secret fallback from environment.
- Recorded approval identity uses full SHA-256 hex digest, not the raw approval id.

## ReadinessRunEvidence

Purpose: Non-secret audit record for approved real no-submit readiness.

Fields:

- `executable_identity`
- `config_bundle_checksum`
- `command_name`
- `exit_status`
- `result`

Validation:

- Must not include raw TOML contents, SSM values, API keys, private keys, passphrases, or bearer-like tokens.
- Must be recorded in PR/handoff text or operator-approved artifact after an approved real run.
