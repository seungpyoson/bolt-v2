# Bolt-v3 Production Readiness End-to-End Trace

Date: 2026-05-20

Trace state: PR #388 branch after PR #408 was merged into `main`.
Base traced: `origin/main` at `ddace92880c3126c3cb6c161c1c239f217d75a62`

Exact pushed PR heads and verification runs are recorded in PR handoff comments
and GitHub Actions. They are not embedded here because every metadata-only edit
changes the document's containing commit SHA.

Purpose: give reviewers and operators a concrete code-path map for live trade readiness. This is not a readiness claim. It separates source-code path evidence from approval-gated real SSM, venue, canary, and production-operation evidence.

Related tracking:

- Tiny-canary historical gap tracker: issue #360 (closed). Closure is not proof
  that T046 completed; only a redacted tiny-capital canary artifact on the exact
  reviewed head can satisfy T046.
- Production-grade readiness beyond tiny canary: issue #369
- Real no-order connectivity blocker: issue #385
- PortfolioSnapshot observability tracker: issue #409 (open on 2026-05-20).
  This PR adds source-level capture, but the issue remains an issue-ledger item
  until exact-head verification and approved issue mutation record the new state.
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
   - `src/bolt_v3_live_node.rs:454-464`
   - `src/bolt_v3_live_canary_gate.rs:308-430`
   - `src/bolt_v3_live_canary_gate.rs:474-585`
   - `src/bolt_v3_live_canary_gate.rs:588-690`
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
    - `src/bolt_v3_live_node.rs:454-470`
    - After gate/admission setup, `run_bolt_v3_live_node` starts runtime capture and calls `node.run()`.
    - Trace meaning: entering NT runner is only allowed after the gate and admission state are ready.

## No-Submit Readiness Path

1. CLI entrypoint
   - `src/main.rs:53-57`
   - `NoSubmitReadiness` loads config, runs no-submit readiness, then writes the configured report.

2. No-submit runner
   - `src/bolt_v3_no_submit_readiness.rs:493-534`
   - Builds the live node, computes metadata, then runs readiness inside a dedicated Tokio runtime and `LocalSet`.

3. Controlled connect/reference/disconnect stages
   - `src/bolt_v3_no_submit_readiness.rs:275-311`
   - `src/bolt_v3_no_submit_readiness.rs:413-508`
   - `src/bolt_v3_live_node.rs:721-843`
   - Stage builder records operator approval, secret resolution, live-node build, controlled connect, reference readiness, controlled disconnect, report write, and top-level `generated_at_unix_seconds`.
   - Reference readiness now requires configured quote evidence from the no-submit reference quote probe. Cache-only instrument-ID membership remains fail-closed and is not treated as live reference-data freshness.
   - `[live_canary].reference_quote_max_age_seconds` bounds accepted quote age. `[live_canary].reference_quote_wait_timeout_seconds` bounds the probe wait before the runner stops. `[live_canary].reference_quote_probe_*` owns the NT `DataActorConfig` values.

4. Gate consumption
   - `src/bolt_v3_live_canary_gate.rs:493-598`
   - `src/bolt_v3_live_canary_gate.rs:1381`
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

- 2026-05-21 18:34:44 KST: a local no-submit command was run with explicit approval against local operator config and real SSM/venue surfaces at head `3190803c5cb51ffeaebbd80a029c4a65bf3291c4`.
- Command: `cargo run --bin bolt-v2 -- no-submit-readiness --config config/live.local.toml`.
- Report path: `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`; mode observed as `-rw-------`, size `1283` bytes.
- Report fields: schema `bolt-v3.no-submit-readiness.v2`, generated timestamp `1779356084`, config bundle checksum `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`, executable identity `ec913e9f98ab11d60b8a2dd921e92d99163cc0e959f124e0bd9c3199fb31c601`.
- Satisfied stages: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_disconnect`, and `report_write`.
- Failed stage: `controlled_connect`, with report detail `bolt-v3 no-submit controlled-run reached NT Running but live reference quote evidence was not observed; engine connectivity cannot be treated as proven: reference quote probe did not observe all configured reference_data quotes within [live_canary].reference_quote_wait_timeout_seconds=20`.
- Skipped stage: `reference_readiness`, with report detail `controlled connect failed`.
- Runtime log evidence showed `polymarket_main` data and execution connected, `binance_reference` data did not connect, `DataEngine.check_connected() == false`, `ExecEngine.check_connected() == true`, and NT refused to start the trader.
- The observed Binance reference failure was a WebSocket handshake rejection from `stream-sbe.binance.com/ws` with HTTP 400 and `Invalid X-MBX-APIKEY header`; no credential value was printed.
- Approved local SSM probe confirmed the configured Binance SecureString parameters resolve as non-empty; credential values and account/parameter metadata remain in untracked operator evidence. This does not prove the Binance API key is active, paired to the private key, or allowed from this host's IP.
- 2026-05-21 19:34:17 KST follow-up root-cause probe at head `d69b43c22ce22d018bc1c39006bbd2e7d642c372` pinned `binance_reference` and printed no secret values. It fetched both configured SSM parameters successfully, observed the configured Binance API key was nonempty, validated the private key as Ed25519 PKCS#8 key material, reached Binance `/api/v3/time` with HTTP `200`, then signed read-only `/api/v3/account` and received HTTP `401` with Binance code `-2015` (`Invalid API-key, IP, or permissions for action.`). Binance official Spot docs classify `-2015` as invalid API key, IP, or permissions; Binance official SBE docs require an Ed25519 API key in `X-MBX-APIKEY` and state IP whitelists still restrict SBE market data access. This rules out empty configured SSM values and malformed Ed25519 private-key shape in this probe, but it does not identify whether the remaining blocker is a wrong configured SSM parameter target, key pairing/state, IP whitelist, permission, account, or environment configuration.
- This does not prove no-submit live connectivity readiness and does not complete T038. Do not treat command exit status or partial Polymarket connectivity as readiness proof.
- Detailed secret-management mutation metadata is intentionally not committed here.

## Tiny-Canary Path

1. Operator harness and preflight
   - `tests/bolt_v3_tiny_canary_operator.rs:1366-1391`
   - `src/bolt_v3_tiny_canary_evidence.rs:483`
   - Preflight blocks before live runner if the approval/evidence envelope is incomplete.
   - Production `Run` also requires `[live_canary].operator_evidence` at the live canary gate. The gate validates required evidence fields and the active approval window before submit admission can arm.

2. Live runner entry
   - `tests/bolt_v3_tiny_canary_operator.rs:1426-1430`
   - Harness uses `run_bolt_v3_live_node`, not a separate live architecture.

3. Required artifact paths
   - `tests/bolt_v3_tiny_canary_operator.rs:1552-1617`
   - `tests/bolt_v3_tiny_canary_operator.rs:1620-1650`
   - Harness requires venue order state, optional strategy cancel, restart reconciliation, and other evidence paths.

4. Evidence validation
   - `tests/bolt_v3_tiny_canary_operator.rs:2036-2065`
   - `tests/bolt_v3_tiny_canary_operator.rs:207-260`
   - Evidence hashes and references are bound before proof is accepted.
   - Operator-envelope regressions cover approval-window rejection, nonce hash mismatch, SSM manifest hash mismatch, strategy-input hash mismatch, financial-envelope hash mismatch, and pre-run evidence hash mismatch.

Current hard evidence:

- T046 remains unchecked in `specs/001-thin-live-canary-path/tasks.md`.
- Issue #360 is closed, but that closure is only historical tracking state and
  is not accepted as T046 evidence.
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

## Issue Ledger Mutation Status

No GitHub issue mutation was performed by this trace update. Required issue
ledger updates remain blocked operator actions until explicitly approved:

- Update issue #409 with the exact PR/head verification that proves
  PortfolioSnapshot capture is represented in source, docs, and verifier gates.
- Add or update successor/context links explaining that issue #360 closure does
  not complete T046 and does not prove a tiny-capital canary artifact exists.

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
