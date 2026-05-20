# Bolt-v3 Production Readiness End-to-End Trace

Date: 2026-05-20

Trace state: PR #388 branch after PR #408 was merged into `main`.
Base traced: `origin/main` at `ddace92880c3126c3cb6c161c1c239f217d75a62`

Exact pushed PR heads and verification runs are recorded in PR handoff comments
and GitHub Actions. They are not embedded here because every metadata-only edit
changes the document's containing commit SHA.

Purpose: give reviewers and operators a concrete code-path map for live trade readiness. This is not a readiness claim. It separates source-code path evidence from approval-gated real SSM, venue, canary, and production-operation evidence.

Related tracking:

- Tiny-canary proof chain: issue #360
- Production-grade readiness beyond tiny canary: issue #369
- Real no-order connectivity blocker: issue #385
- PR carrying this trace/control surface: PR #388

## Current Verdict

Not production-ready.

Current source code contains the intended single bolt-v3 production path and local tests cover several fail-closed gates. That is not enough for production-grade live trading. Required operator evidence still includes real no-submit connectivity, accepted gate report, live canary artifact, order lifecycle evidence, restart reconciliation, repeated-operation controls, monitoring, deploy provenance, and post-run hygiene.

## End-to-End Production Run Path

1. CLI entrypoint
   - `src/main.rs:60-68`
   - `Run` loads TOML with `load_bolt_v3_config`, builds with `build_bolt_v3_live_node`, then enters NT only through `run_bolt_v3_live_node`.
   - Trace meaning: production live run should not bypass the bolt-v3 wrapper.

2. Root and strategy TOML load
   - `src/bolt_v3_config.rs:26-39`
   - `src/bolt_v3_config.rs:257-294`
   - `src/bolt_v3_config.rs:359-423`
   - Root and strategy structs use `deny_unknown_fields`; root `strategy_files` are loaded and included in `config_bundle_checksum`.
   - Trace meaning: stale config keys fail before SSM, client registration, or NT runner entry.

3. Config validation
   - `src/bolt_v3_validate.rs:84-111`
   - `src/bolt_v3_validate.rs:378-424`
   - `src/bolt_v3_validate.rs:444-543`
   - Validation checks live runtime mode, clients, SSM path shape, strategy execution client, and reference data clients.
   - Trace meaning: execution and data clients must come from TOML-defined `[clients.<id>]` blocks.

4. SSM secret resolution
   - `src/bolt_v3_secrets.rs:70-111`
   - `src/bolt_v3_secrets.rs:190-243`
   - `src/bolt_v3_secrets.rs:243-282`
   - `src/secrets.rs:76-152`
   - Production path rejects forbidden credential environment variables, resolves every configured secret through `SsmResolverSession`, uses AWS SSM `GetParameter` with decryption, and rejects empty or whitespace-containing secret values.
   - Trace meaning: no environment fallback, AWS CLI subprocess, file secret, or non-SSM secret backend should be part of production readiness.

5. Provider binding and adapter mapping
   - `src/bolt_v3_providers/mod.rs:109-136`
   - `src/bolt_v3_providers/mod.rs:140-164`
   - Provider bindings own validation, secret resolution, credential log filters, forbidden env var lists, and adapter mapping.
   - Trace meaning: core should route through provider bindings, not hardcoded venue-specific production branches.

6. NT client registration
   - `src/bolt_v3_client_registration.rs:93-129`
   - `src/bolt_v3_live_node.rs:647-655`
   - Resolved adapters are registered into NT data and execution clients.
   - Trace meaning: configured clients must appear in the NT builder summary before any live runner claim.

7. Strategy registration
   - `src/bolt_v3_strategy_registration.rs:31-32`
   - `src/bolt_v3_strategy_registration.rs:101-124`
   - `src/bolt_v3_live_node.rs:655-669`
   - Strategy contexts receive mandatory decision evidence and the shared submit-admission state.
   - Trace meaning: strategy construction cannot be treated as live-ready unless decision evidence and admission state are wired into the registered strategy context.

8. Live canary gate before runner entry
   - `src/bolt_v3_live_node.rs:436-446`
   - `src/bolt_v3_live_canary_gate.rs:259-430`
   - `src/bolt_v3_live_canary_gate.rs:474-570`
   - The live runner checks `[live_canary].operator_evidence`, the no-submit readiness report, report freshness, approval hash, executable identity, config bundle checksum, configured caps, and required satisfied stages before arming submit admission.
   - The operator approval window is checked before report validation and again after report validation using the late gate timestamp. The same late timestamp validates `generated_at_unix_seconds` against TOML-owned `readiness_report_max_age_seconds`.
   - Trace meaning: missing operator evidence, expired approval, expired readiness report, or a report with failed, skipped, stale, or mismatched stages is not live-runner evidence.

9. Submit admission before NT submit
   - `src/bolt_v3_submit_admission.rs:20-81`
   - `src/strategies/binary_oracle_edge_taker.rs:3422-3433`
   - `src/strategies/binary_oracle_edge_taker.rs:3552`
   - `src/strategies/binary_oracle_edge_taker.rs:3763`
   - Strategy submit path records order-intent evidence, derives an admission request, obtains an admission permit, then calls NT `submit_order`.
   - Trace meaning: decision evidence and admission must precede every live submit candidate; tests and source fences must catch alternate submit paths.

10. NT runner
    - `src/bolt_v3_live_node.rs:436-471`
    - After gate/admission setup, `run_bolt_v3_live_node` starts runtime capture and calls `node.run()`.
    - Trace meaning: entering NT runner is only allowed after the gate and admission state are ready.

## No-Submit Readiness Path

1. CLI entrypoint
   - `src/main.rs:53-57`
   - `NoSubmitReadiness` loads config, runs no-submit readiness, then writes the configured report.

2. No-submit runner
   - `src/bolt_v3_no_submit_readiness.rs:354-378`
   - Builds the live node, computes metadata, then runs readiness inside a dedicated Tokio runtime and `LocalSet`.

3. Controlled connect/reference/disconnect stages
   - `src/bolt_v3_no_submit_readiness.rs:244-281`
   - `src/bolt_v3_no_submit_readiness.rs:291-331`
   - `src/bolt_v3_no_submit_readiness.rs:334-352`
   - Stage builder records operator approval, secret resolution, live-node build, controlled connect, reference readiness, controlled disconnect, report write, and top-level `generated_at_unix_seconds`.
   - Current reference readiness is fail-closed when NT cache evidence only proves configured instrument IDs. Instrument-ID cache membership is not treated as live reference-data freshness.

4. Gate consumption
   - `src/bolt_v3_live_canary_gate.rs:474-570`
   - Gate requires all readiness stages to be present and satisfied, the generated timestamp to be fresh under `[live_canary].readiness_report_max_age_seconds`, and the report linkage fields to match the current approval, executable identity, and config bundle checksum.

Current hard-evidence requirements:

- Before this trace is used as current PR evidence, rerun
  `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`,
  `cargo test --test bolt_v3_live_canary_gate -- --nocapture`,
  `cargo fmt --check`, and `git diff --check origin/main...HEAD` on the exact
  pushed PR head.
- A passing command on an older branch head is not production-readiness evidence
  for a later rebased head.

Current live-operator evidence:

- A local no-submit command was run with explicit approval against local operator config and real SSM/venue surfaces.
- It reached SSM resolution, NT LiveNode build, client registration, strategy registration, and NT startup.
- It did not prove readiness: live connectivity/reference readiness still failed. Do not treat command exit status as readiness proof.
- Detailed secret-management mutation metadata is intentionally not committed here.

## Tiny-Canary Path

1. Operator harness and preflight
   - `tests/bolt_v3_tiny_canary_operator.rs:1070-1086`
   - `src/bolt_v3_tiny_canary_evidence.rs:483`
   - Preflight blocks before live runner if the approval/evidence envelope is incomplete.
   - Production `Run` also requires `[live_canary].operator_evidence` at the live canary gate. The gate validates required evidence fields and the active approval window before submit admission can arm.

2. Live runner entry
   - `tests/bolt_v3_tiny_canary_operator.rs:1109`
   - Harness uses `run_bolt_v3_live_node`, not a separate live architecture.

3. Required artifact paths
   - `tests/bolt_v3_tiny_canary_operator.rs:1236-1292`
   - Harness requires venue order state, optional strategy cancel, restart reconciliation, and other evidence paths.

4. Evidence validation
   - `tests/bolt_v3_tiny_canary_operator.rs:1339-1418`
   - Evidence hashes and references are bound before proof is accepted.
   - Operator-envelope regressions cover approval-window rejection, nonce hash mismatch, SSM manifest hash mismatch, strategy-input hash mismatch, financial-envelope hash mismatch, and pre-run evidence hash mismatch.

Current hard evidence:

- T046 remains unchecked in `specs/001-thin-live-canary-path/tasks.md:110`.
- No tiny-capital canary artifact was produced in this trace.
- Therefore no production-grade readiness claim is supported.

## Production-Grade Gap Surface

Issue #369 remains the production-grade control issue. The new Speckit checklist at `specs/001-thin-live-canary-path/checklists/production-readiness.md` defines 38 requirements-quality checks covering:

- production-grade readiness versus tiny-canary readiness;
- end-to-end traceability;
- no-hardcode and registry-driven core requirements;
- SSM-only credential hygiene and non-disclosure requirements;
- real no-submit and live gate evidence;
- adapter and venue protocol drift;
- order lifecycle and restart reconciliation;
- repeated-live operation;
- monitoring, deploy provenance, and rollback;
- TDD and exact-head verification discipline.

## Next TDD Slices

Do not implement from guesses. Each fix needs one behavior test first, then the smallest code change.

Candidate slices, in order:

1. No-submit stage correctness on current main
   - Behavior: failed NT client connect must produce failed `controlled_connect` and skipped `reference_readiness`.
   - Tracker: issue #385.

2. Build-feature/config compatibility
   - Behavior: unavailable transport backend must fail before operator live-connect attempts with a clear config/build error.
   - Tracker: issue #385 or a linked child if the existing scope gets too broad.

3. Venue protocol drift
   - Behavior: adapter protocol version mismatch must be pinned, detected, or routed to an accepted upstream NT revision without core hardcoding.
   - Tracker: issue #385.

4. Production-grade readiness definition
   - Behavior: no artifact, issue, PR, or status map can claim production-ready until issue #369 checklist requirements are satisfied or explicitly waived.
   - Tracker: issue #369.
