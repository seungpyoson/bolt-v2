# T119 Static Operator Artifacts Plan

> **Historical implementation record — not an active plan.** Do not execute
> commands or tasks from this file. Current `main`, `AGENTS.md`, and tracked
> issues are authoritative.

Date: 2026-05-22
Branch: `codex/production-readiness-evidence-audit`
PR: #388
Head inspected: `07647275057accb4632dc28d1507cd4820027762`
Base inspected: `origin/main` `c1a4edbdd5202ea507750f069c64d8edca044381`

## Scope

T119 only covers production helper coverage for static operator artifacts that do not require a live submit:

- redacted SSM manifest
- financial envelope from loaded TOML
- approval nonce
- abort plan

T119 does not cover strategy-input evidence (T120), pre-run state evidence (T121), final operator-evidence config packet and no-submit rerun (T122), final review/CI packet (T123), or tiny-capital canary execution (T116).

No helper may print secret values, read `.env`, use a CLI secret backend, or require non-SSM secret sources.

## Current Evidence

Repo and PR state:

- `git fetch origin` completed with no output.
- `git status --short --branch` reported `## codex/production-readiness-evidence-audit...origin/codex/production-readiness-evidence-audit`.
- `git rev-parse HEAD` reported `07647275057accb4632dc28d1507cd4820027762`.
- `git rev-parse origin/main` reported `c1a4edbdd5202ea507750f069c64d8edca044381`.
- `gh pr view 388 --repo seungpyoson/bolt-v2 --json state,headRefOid,baseRefOid,mergeStateStatus,url` reported `state=OPEN`, `headRefOid=07647275057accb4632dc28d1507cd4820027762`, `baseRefOid=c1a4edbdd5202ea507750f069c64d8edca044381`, and `mergeStateStatus=CLEAN`.

Speckit state:

- T115 still lists missing SSM manifest, strategy-input evidence, financial envelope, pre-run state, abort plan, approval nonce, approval envelope, final config-owned operator-evidence block, and rerun proof in `specs/001-thin-live-canary-path/tasks.md:241`.
- T119 is unchecked and asks for production helper coverage for redacted SSM manifest, financial envelope from loaded TOML, approval nonce, and abort plan in `specs/001-thin-live-canary-path/tasks.md:270`.
- T120-T123 remain unchecked in `specs/001-thin-live-canary-path/tasks.md:271`.

CLI gap:

- Current production CLI exposes `run`, `no-submit-readiness`, and `secrets` only in `src/main.rs:20`.
- `no-submit-readiness` writes only the no-submit report in `src/main.rs:53`.
- `secrets check` prints configured secret field names only in `src/main.rs:78`.
- `secrets resolve` uses `SsmResolverSession` and prints only per-client success lines in `src/main.rs:98`.
- No production command exists for static operator artifact generation. This is corroborated by read-only subagent review and `rg` searches for `operator-artifacts`, `static operator`, `SSM manifest`, `approval nonce`, `financial envelope`, and `abort plan`.

Gate contract:

- `[live_canary.operator_evidence]` owns artifact paths and hashes, including `ssm_manifest_path`, `ssm_manifest_sha256`, `financial_envelope_path`, `financial_envelope_sha256`, `abort_plan_path`, `abort_plan_sha256`, `approval_nonce_path`, and `approval_nonce_sha256` in `src/bolt_v3_config.rs:198`.
- Production gate requires operator evidence before runner entry in `src/bolt_v3_live_canary_gate.rs:807`.
- Gate validates non-empty fields, head SHA, configured path shape, lowercase SHA-256 shape, positive size/freshness limits, active approval window, file hashes, approval envelope, and approval consumption in `src/bolt_v3_live_canary_gate.rs:820`.
- Gate binds the seven static input file hashes by path and expected SHA-256 in `src/bolt_v3_live_canary_gate.rs:1580`.
- Quickstart requires redacted SSM manifest, strategy input evidence, financial envelope, pre-run state, abort plan, and approval nonce bindings in `specs/001-thin-live-canary-path/quickstart.md:79`.
- Schema doc describes the same operator-evidence fields and strict JSON artifact expectations in `docs/bolt-v3/2026-04-25-bolt-v3-schema.md:735` and `docs/bolt-v3/2026-04-25-bolt-v3-schema.md:780`.

Existing validation but no production writer:

- `Phase8OperatorApprovalEnvelope::from_env()` reads artifact paths and hashes from harness env and loaded TOML in `src/bolt_v3_tiny_canary_evidence.rs:1280`; it validates existing files, but does not generate them.
- Financial envelope validation derives expected values from loaded TOML via `Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy` in `src/bolt_v3_tiny_canary_evidence.rs:1425` and `src/bolt_v3_tiny_canary_evidence.rs:1750`.
- Approval nonce validation currently re-hashes the nonce file and compares to the expected SHA-256 in `src/bolt_v3_tiny_canary_evidence.rs:1569`; no parsed nonce JSON schema exists in production code.
- Abort plan validation parses a strict JSON body and requires all five abort booleans to be true in `src/bolt_v3_tiny_canary_evidence.rs:2138`.
- Test fixture writers generate hardcoded financial and abort artifacts in `tests/bolt_v3_tiny_canary_preconditions.rs:3279` and `tests/bolt_v3_tiny_canary_preconditions.rs:3360`. Those are test-only and must not become production runtime literals.

SSM evidence:

- Provider schemas own SSM path fields. Polymarket fields are in `src/bolt_v3_providers/polymarket.rs:155`; Binance fields are in `src/bolt_v3_providers/binance.rs:105`.
- Provider bindings expose provider-owned `secret_field_names` in `src/bolt_v3_providers/mod.rs:97`, but do not expose configured SSM path entries.
- SSM path validation is provider-neutral and rejects invalid path shape in `src/bolt_v3_validate.rs:425`.
- Secret resolution uses the Rust SDK SSM session boundary in `src/secrets.rs:119`.
- Forbidden credential env vars are rejected before startup in `src/bolt_v3_secrets.rs:72`.
- The T119 SSM manifest helper must enumerate configured SSM references without calling `SsmResolverSession`, without resolving values, and without printing raw secret material.

Abort-plan evidence risk:

- Existing abort plan validation only checks `execution_client_id`, `configured_target_id`, and five boolean fields in `src/bolt_v3_tiny_canary_evidence.rs:2151`.
- Submit lifecycle tests prove plain cancel is not a submit candidate in `tests/bolt_v3_submit_admission.rs:584`.
- Live-result proof tests require strategy cancel evidence when a venue order remains open in `tests/bolt_v3_tiny_canary_operator.rs:845`.
- Source-grounded status map still marks panic gate and service policy missing in `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:111`.
- Therefore a T119 helper must not blindly emit `panic_gate_trip_abort_defined = true` as a readiness claim. It must either verify source-owned abort-policy prerequisites with explicit tests, or fail closed and record the missing prerequisite.

Targeted test evidence already run during investigation:

- `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_no_submit_readiness_operator_command -- --nocapture`: passed.
- `cargo test --test bolt_v3_tiny_canary_preconditions operator_approval_envelope_verifies_financial_envelope_hash_and_loaded_config -- --nocapture`: passed.
- `cargo test --test bolt_v3_tiny_canary_preconditions operator_approval_envelope_verifies_abort_plan_hash_and_required_paths -- --nocapture`: passed.
- `cargo test --test bolt_v3_tiny_canary_preconditions operator_approval_envelope_verifies_pre_run_state_hash_and_required_clearances -- --nocapture`: passed.
- `cargo test --test bolt_v3_tiny_canary_preconditions operator_approval_envelope_verifies_ssm_manifest_hash -- --nocapture`: passed.

## Design Target

Add a narrow production helper surface:

- Library module: `bolt_v3_operator_artifacts`.
- CLI command: `bolt-v2 operator-artifacts generate-static --config <root.toml> --output-dir <dir> --strategy-instance-id <id>`.
- Command output: one redacted summary containing output file paths and SHA-256 values only. No secret values. No `.env`. No SSM value resolution.
- File writes: create parent dir if needed, use create-new semantics, refuse overwrite, write deterministic pretty JSON, compute SHA-256 after write.

Generated files:

- `ssm-manifest.json`: deterministic redacted inventory of configured SSM references. It includes non-secret config identity, client key, provider key, field name, and AWS region. Exact SSM path strings remain bound by the root config bundle checksum and by the generated manifest file SHA, but the shareable manifest must not include raw SSM paths, raw secret values, or dictionary-confirmable per-path hashes.
- `financial-envelope.json`: exact `Phase8FinancialEnvelopeEvidenceFile` values derived from loaded TOML for the requested `strategy_instance_id`.
- `approval-nonce.json`: one-shot nonce evidence with schema/version/record kind and a generated nonce hash. The nonce source is 32 bytes from an OS CSPRNG through a direct production dependency added for this helper; the artifact stores only lowercase SHA-256 of those bytes. Raw nonce material must not be written, printed, logged, or returned.
- `abort-plan.json`: strict abort plan JSON. It must not report all booleans true unless the helper can prove each static abort prerequisite from current source-owned contracts. If proof is incomplete, the command must fail closed before writing a successful abort plan.
- `static-artifacts-manifest.json`: helper-owned convenience index of generated paths and SHA-256s so the operator can copy values into `[live_canary.operator_evidence]` without manual hashing. It is not a live canary gate input; the gate remains bound to the individual TOML-owned file paths and SHA-256 values.

Provider boundary:

- Add provider-owned SSM manifest extraction next to provider-owned secret schema parsing. Core code must call a provider binding function and must not hardcode Polymarket or Binance secret field names.
- Existing `secret_field_names` can remain for CLI field-name inventory, but T119 needs configured path inventory. Add a provider-owned extractor or typed helper per provider.
- Provider extractor interface returns `(field_name, configured_ssm_path)` pairs after provider-owned config parsing. Core code owns only redaction, sorting, and manifest serialization.

Hardcode rule:

- Production helper may contain schema field names, record kinds, and config-key names.
- Production helper must not contain BTC, BINANCE, BTCUSDT.BINANCE, venue IDs, quantities, timeouts, paths, or IDs except values read from loaded TOML or schema-owned field names.
- Test fixtures may continue to use fixture literals, classified as test-only.

## TDD Plan

Proceed one vertical slice at a time. Do not write broad implementation before each RED failure is observed.

1. RED: CLI help exposes `operator-artifacts generate-static` with `--config`, `--output-dir`, and `--strategy-instance-id`.
   GREEN: add minimal clap command wired to the first safe artifact writer; do not commit a permanent dead command.

2. RED: helper generates redacted SSM manifest from `tests/fixtures/bolt_v3/root.toml` without invoking an SSM resolver and without printing raw secret values.
   GREEN: add provider-owned manifest extraction for redacted configured SSM reference inventory without raw paths or per-path dictionary hashes.

3. RED: helper generates financial envelope from loaded TOML for one strategy, and a changed TOML value changes or rejects the artifact through existing validator.
   GREEN: reuse the existing `from_loaded_for_strategy` logic by moving or exposing it without duplicating test fixture literals.

4. RED: helper generates approval nonce with create-new semantics and refuses overwrite; stdout omits nonce material and artifact contains only `nonce_sha256`, not raw nonce bytes.
   GREEN: add nonce file schema and writer backed by a direct OS-CSPRNG dependency; hash the 32 generated bytes and discard the raw bytes.

5. RED: helper refuses to write a successful abort plan when static abort prerequisites cannot be proven, including missing panic-gate proof.
   GREEN: add fail-closed abort-plan generation. If current evidence can prove every abort prerequisite, emit true booleans; otherwise return a blocker error with no successful abort artifact.

6. RED: complete command writes all accepted static artifacts plus `static-artifacts-manifest.json`, reports hashes, and never resolves SSM or starts the live runner.
   GREEN: compose slices behind the CLI.

7. Refactor only after green: remove duplication, keep provider ownership, run slop scan, runtime literal scan, targeted tests, `cargo fmt --check`, `git diff --check`, source-fence if production runtime literals changed, and external model review.

## External Review Questions

External reviewers must answer before implementation:

- Does the plan satisfy T119 without weakening the existing live canary gate?
- Does the no-raw-path/no-per-path-hash SSM manifest policy preserve enough auditability through config checksum, client/provider/field inventory, and artifact SHA without leaking account or parameter naming structure?
- Is fail-closed abort-plan generation acceptable for T119 if panic-gate evidence remains missing, or must T119 remain unchecked until panic-gate proof exists?
- Does exposing or moving `Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy` preserve schema and validation behavior without creating a second source of truth?
- Does the planned provider-owned SSM extractor avoid production hardcodes and provider leakage into core?
- Are the planned tests public-interface TDD tests rather than implementation-shape tests?

## Completion Criteria

T119 can be checked only when current exact-head evidence proves:

- production helper exists and is reachable through public CLI or public library API
- redacted SSM manifest helper is deterministic, provider-owned, no-resolve, and secret-safe
- financial envelope helper derives all values from loaded TOML
- approval nonce helper refuses overwrite and does not print raw nonce material
- abort plan helper either proves all static abort prerequisites and emits safe JSON, or fails closed with a documented blocker
- generated hashes bind to files that existing gate/harness validation accepts
- no production runtime hardcodes were introduced
- targeted tests pass
- slop/static scans pass
- exact-head CI state is known
- Gemini, Claude, GLM, DeepSeek, and Kimi review the meaningful implementation slice, or a failed reviewer slot is documented with source-send/timeout evidence and user-relayed review if needed
