# Phase 7 Data Model

## NoSubmitReadinessReport

Purpose: Redacted JSON artifact consumed by the live-canary gate.

Fields:

- `schema_version`: Operator-safe schema version string.
- `approval_id_hash`: Non-secret identity for approval matching when recorded.
- `head_sha`: Exact git head for approved real run evidence.
- `config_checksum`: Non-secret checksum of approved root TOML for approved real run evidence.
- `report_path`: Config-selected output path.
- `stages`: Ordered list of `NoSubmitReadinessStage`.

Validation:

- Report must be a JSON object.
- `stages` must be non-empty.
- Every required stage must be present and satisfied for live-canary acceptance.
- Report size must remain within `[live_canary].max_no_submit_readiness_report_bytes`.
- Resolved credential values must never appear in serialized or debug output.

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

Purpose: Explicit approval input for side-effect-bearing real no-submit readiness.

Fields:

- `approval_id`: Non-secret operator-provided id.
- `configured_approval_id`: Value from `[live_canary].approval_id`.

Validation:

- Missing or whitespace approval id fails before secret resolution.
- Mismatch fails before secret resolution.
- Approval id is not a credential and does not allow secret fallback from environment.

## ReadinessRunEvidence

Purpose: Non-secret audit record for approved real no-submit readiness.

Fields:

- `head_sha`
- `root_toml_checksum`
- `report_path`
- `command_name`
- `exit_status`
- `result`

Validation:

- Must not include raw TOML contents, SSM values, API keys, private keys, passphrases, or bearer-like tokens.
- Must be recorded in PR/handoff text or operator-approved artifact after an approved real run.
