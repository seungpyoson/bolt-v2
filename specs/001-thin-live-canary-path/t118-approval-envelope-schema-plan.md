# T118 Approval Envelope Schema Plan

## Scope

T118 defines and enforces a non-circular approval-envelope schema for the T046
tiny-capital canary packet. This is a planning/evidence artifact only until
external review approves the design.

T118 must not execute the ignored canary runner, submit orders, cancel orders,
deploy, transfer funds, or mutate production state.

## Current Evidence

- Root TOML owns `approval_envelope_path` and `approval_envelope_sha256` in
  `[live_canary.operator_evidence]`: `src/bolt_v3_config.rs:198-203`.
- `config_bundle_checksum` hashes the final root TOML text plus strategy TOML
  texts: `src/bolt_v3_config.rs:429-442`. Because root TOML contains
  `approval_envelope_sha256`, any envelope-file content that includes
  `config_bundle_checksum` would create a hash loop.
- The live canary gate computes `root_toml_sha256` from the loaded root TOML
  path at validation time and compares it only against the post-preflight
  approval-consumption proof: `src/bolt_v3_live_canary_gate.rs:962-970` and
  `src/bolt_v3_live_canary_gate.rs:1235-1242`.
- The live canary gate hash-binds `approval_envelope_path` to the TOML-owned
  `approval_envelope_sha256`: `src/bolt_v3_live_canary_gate.rs:1364-1373`.
- Current `Phase8OperatorApprovalEnvelope` carries both `root_toml_sha256` and
  `approval_envelope_sha256`: `src/bolt_v3_tiny_canary_evidence.rs:1255-1259`.
- `Phase8OperatorApprovalEnvelope::from_env` requires
  `BOLT_V3_PHASE8_ROOT_TOML_SHA256` and
  `BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256`:
  `src/bolt_v3_tiny_canary_evidence.rs:1281-1288`.
- The approval-consumption writer emits `root_toml_sha256` and
  `approval_envelope_sha256` after preflight:
  `src/bolt_v3_tiny_canary_evidence.rs:1586-1592` and
  `src/bolt_v3_tiny_canary_evidence.rs:2220-2227`.
- Current schema docs already state that `root_toml_sha256` is not configured in
  TOML because hashing the file into itself would be circular:
  `docs/bolt-v3/2026-04-25-bolt-v3-schema.md:743`.
- Current quickstart still asks the harness operator environment to provide
  `BOLT_V3_PHASE8_ROOT_TOML_SHA256` and
  `BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256`:
  `specs/001-thin-live-canary-path/quickstart.md:95-103`.

## Root Cause

There is no explicit approval-envelope file schema. Current production gate
code hash-checks `approval_envelope_path` but does not parse that file into
`Phase8OperatorApprovalEnvelope`. The current `Phase8OperatorApprovalEnvelope`
is a harness environment envelope, but its name and fields make it look like the
approval-envelope artifact itself. If that struct is treated as the file schema,
it is circular:

- `approval_envelope_sha256` cannot be inside the approval-envelope file whose
  final SHA-256 is stored in root TOML.
- `root_toml_sha256` cannot be inside the approval-envelope file while root TOML
  stores the approval-envelope SHA-256.
- `config_bundle_checksum` also cannot be inside the approval-envelope file
  because it hashes the root TOML that stores the approval-envelope SHA-256.

The safe split is:

- Root TOML owns static paths and static artifact SHA-256s, including
  `approval_envelope_sha256`.
- Loaded config owns `config_bundle_checksum` after final root TOML exists.
- The live canary gate computes current `root_toml_sha256`.
- Approval-consumption proof is generated after preflight and may include
  `root_toml_sha256`, `approval_envelope_sha256`, and `config_bundle_checksum`
  because it is not hashed back into root TOML.
- Approval-envelope file must not include its own SHA-256, root TOML SHA-256, or
  config bundle checksum.

## Target Hash Ownership

| Value | Owner | May appear in approval-envelope file? | Reason |
| --- | --- | --- | --- |
| `approval_envelope_path` | root TOML | No | Path is config-owned; file proves approval content only. |
| `approval_envelope_sha256` | root TOML | No | Self-hash loop if placed inside file. |
| `root_toml_sha256` | gate / approval-consumption proof | No | Root TOML stores envelope hash, so pre-authored file cannot include final root hash. |
| `config_bundle_checksum` | loaded config / reports | No | Bundle checksum includes final root TOML and would loop through envelope hash. |
| `head_sha` | root TOML + build-owned head | Yes | Non-circular exact-head approval. |
| `approval_id_hash` | derived from `[live_canary].approval_id` | Yes | Non-secret approval binding without raw id if desired. |
| static artifact hashes | root TOML | Yes, if validated equal | Non-circular duplicate approval statement over packet contents. |
| approval window | root TOML | Yes, if validated equal | Non-circular operator approval bounds. |
| `canary_evidence_path_hash` | derived from root TOML path string | Yes | Non-circular output binding. |
| optional `strategy_cancel_path_hash` | derived from root TOML path string | Yes | Non-circular optional result binding. |

## TDD Plan

### RED 1: reject circular approval-envelope fields

Add a public behavior test in `tests/bolt_v3_tiny_canary_preconditions.rs`:

`approval_envelope_file_schema_rejects_self_referential_hash_fields`

The test writes an approval-envelope JSON containing any of these fields:

- `approval_envelope_sha256`
- `root_toml_sha256`
- `config_bundle_checksum`

Expected failure: strict schema parsing rejects unknown/circular fields with an
error naming the offending field.

### RED 2: accept non-circular approval-envelope file

Add:

`approval_envelope_file_schema_accepts_external_hash_ownership`

The test writes a valid approval-envelope JSON with:

- `schema_version`
- `record_kind = "phase8_operator_approval_envelope"`
- `head_sha`
- `approval_id_hash`
- `approval_not_before_unix_secs`
- `approval_not_after_unix_secs`
- static artifact SHA-256s
- `canary_evidence_path_hash`
- optional `strategy_cancel_path_hash` when configured

Expected result: validation passes only when all values match the loaded TOML
operator evidence and current head. The approval-envelope file SHA-256 remains
computed externally and bound through
`[live_canary.operator_evidence].approval_envelope_sha256`; approval-consumption
still carries `root_toml_sha256` and `approval_envelope_sha256`.

### RED 3: fail on approval-envelope drift

Add:

`operator_approval_envelope_file_rejects_toml_drift`

Mutate one TOML-owned value in the JSON, such as `ssm_manifest_sha256` or
`approval_id_hash`.

Expected result: validation fails closed before approval consumption.

### RED 4: harness env and file schema stay separate

Add in `tests/bolt_v3_tiny_canary_operator.rs`:

`phase8_operator_harness_does_not_use_approval_env_struct_as_file_schema`

Expected result: the ignored operator harness reads env/preflight inputs
separately from the approval-envelope file schema. The test must fail if
`Phase8OperatorApprovalEnvelope` is reused as the approval-envelope file schema.

### RED 5: harness env no longer requires circular inputs

Add:

`operator_approval_envelope_from_env_does_not_require_root_or_self_hash`

Clear `BOLT_V3_PHASE8_ROOT_TOML_SHA256` and
`BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256` from the test environment while
providing `BOLT_V3_PHASE8_ROOT_TOML_PATH`.

Expected result: the harness computes current root TOML hash internally and
gets `approval_envelope_path` / `approval_envelope_sha256` from loaded TOML,
not from environment.

## Implementation Plan After External Review

1. Add a `Phase8OperatorApprovalEnvelopeFile` JSON schema with
   `serde(deny_unknown_fields)` and an explicit record kind constant.
2. Add a validator that reads
   `[live_canary.operator_evidence].approval_envelope_path`, verifies bytes
   against TOML-owned `approval_envelope_sha256`, parses the file, rejects
   circular fields by schema, and validates all non-circular bindings against
   loaded TOML plus current head.
3. Rename or separate the current harness environment carrier if needed so it is
   not confused with the approval-envelope file schema.
4. Change `Phase8OperatorApprovalEnvelope::from_env` or its successor to stop
   requiring `BOLT_V3_PHASE8_ROOT_TOML_SHA256` and
   `BOLT_V3_PHASE8_APPROVAL_ENVELOPE_SHA256`.
5. Keep approval-consumption proof unchanged for `root_toml_sha256` and
   `approval_envelope_sha256`, because it is generated after final root TOML and
   is validated by the gate.
6. Update `specs/001-thin-live-canary-path/quickstart.md` and
   `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` to state exact hash ownership.
7. Update tests that manually construct the current envelope struct.

## Verification Plan

- RED/GREEN each test above one at a time.
- Run targeted:
  - `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`
  - `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`
  - `cargo test --test bolt_v3_live_canary_gate -- --nocapture`
  - `cargo test --test config_parsing -- --nocapture`
- Run quality:
  - `cargo fmt --check`
  - `git diff --check`
  - added-line slop scan
  - runtime literal audit if production/runtime literal allowlist changes
- Push only after local checks pass.
- Wait for exact-head PR CI.
- Get Gemini, Claude, GLM, DeepSeek, and Kimi consensus before checking T118.

## Not In Scope

- T119 helper generation.
- T120 strategy-input production generation.
- T121 pre-run state production generation.
- T122 final packet/no-submit rerun.
- T116 tiny-capital canary execution.
