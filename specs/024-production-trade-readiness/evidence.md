# Production Trade Readiness Evidence Baseline

Date: 2026-05-25
Worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/024-production-trade-readiness`
Branch: `goal/024-production-trade-readiness`

This file is a source-inspection baseline, not a self-validating exact-head proof. A commit that refreshes this file necessarily changes `HEAD`, so external-review audit manifests and `gh pr view 480` output are authoritative for the current review head. Do not treat any historical commit hash in this file as proof that the current head is approved.

- PR base: `main` at `3a444a57cfdcdc31d58cbfe8d22857eb86f8bad9`
- PR head: read from `gh pr view 480 --json headRefOid` or the external-review audit manifest for the exact review run.
- Active branch: `goal/024-production-trade-readiness`

## Git And PR State

- `git status --short --branch` in main: `## main...origin/main`.
- `git status --short --branch` in active readiness worktree after rename: `## goal/024-production-trade-readiness...origin/goal/024-production-trade-readiness`.
- Historical note: the previous local worktree directory and branch name contained `466-command-tokenization-characterization`; both are no longer active readiness identifiers.
- The old PR #478 branch was renamed in GitHub and locally to `goal/024-production-trade-readiness`.
- GitHub closed PR #478 after the branch rename and would not reopen it because the old head branch no longer exists.
- PR #480 was opened from `goal/024-production-trade-readiness` as the active single production-readiness PR.
- PR #480 title/body identify `specs/024-production-trade-readiness/` as the active task packet, exclude order-intent and #466 work, and record the six-reviewer task-list gate.
- `gh pr view 480 --json number,title,state,isDraft,headRefName,headRefOid,baseRefName,baseRefOid,url` showed:
  - #480 `Production trade-readiness consolidation`, draft, active readiness PR.
  - base `main` at `3a444a57cfdcdc31d58cbfe8d22857eb86f8bad9`.
  - head `goal/024-production-trade-readiness`; exact head is intentionally read from `gh pr view 480 --json headRefOid` at review time.
- `gh pr list --state open` showed exactly two open PRs:
  - #480 `Production trade-readiness consolidation`, draft, active readiness PR, base `main`.
  - #478 is historical/closed after the branch rename.
  - #479 `Finalize #466 verifier decomposition ledger`, draft, head `8efef5863a6bd4a0f1a9276852fd63a37305bd2f`, base `main`.
- Historical #478 status check rollup showed successful build/test/gate/check jobs, with deploy and same-sha-main-evidence skipped. #480 requires fresh exact-head CI before final review.

## Issue State

- #369 is open and defines production-grade live trading readiness beyond a tiny canary.
- #385 is open and tracks real no-order live connectivity. Its text is older than later T038 EC2/EIP success evidence.
- #409 is open and requests PortfolioSnapshot runtime capture.
- #360 is closed and explicitly says tiny-canary readiness is not production live trading readiness.

## Speckit And Readiness Ledger Evidence

- `.specify/feature.json` and the AGENTS Speckit block point to `specs/023-nt-order-intent-layer`; source-fence requires that pointer. PR #480 therefore keeps `specs/024-production-trade-readiness/` as an explicit readiness task packet, not the active `.specify` feature.
- `specs/001-thin-live-canary-path/tasks.md` marks:
  - T038 checked only for historical EC2/EIP no-submit.
  - T046, T116, T122, T124, T125, T126, T127, T128, T130, and T131 unchecked.
  - T129 checked only for final-packet verifier coverage.
- `docs/bolt-v3/2026-05-23-pr388-t124-t128-root-problem-memos.md` says T124-T128 are not readiness completion; they still require real source-owned artifacts.

## Current Code Evidence

`rg` over active readiness branch `src/bolt_v3_operator_artifacts.rs` found these current collector functions:

- `collect_abort_plan_cancel_if_open_source_proof`
- `collect_pre_run_release_manifest_source_proof`
- `collect_pre_run_host_clock_source_proof`
- `collect_pre_run_venue_account_state_source_proof`
- `collect_pre_run_funding_margin_source_proof`
- `collect_pre_run_clob_v2_adapter_signing_source_proof`
- `collect_pre_run_clob_v2_collateral_accounting_source_proof`
- `collect_pre_run_clob_v2_fee_behavior_source_proof`
- `collect_pre_run_market_window_source_proof`
- `collect_pre_run_single_runner_lock_source_proof`
- `collect_pre_run_egress_identity_source_proof`
- `collect_abort_plan_nt_accepted_venue_pending_source_proof`
- `collect_abort_plan_partial_fill_source_proof`
- `collect_abort_plan_network_partition_source_proof`
- `collect_abort_plan_panic_gate_service_policy_source_proof`

Implication: the active readiness branch now has source-owned collector functions for the planned T126 pre-run proof fields and T127 abort-plan proof fields. T023/T024 still need T126 binding and focused verification; T033/T034 still need T127 binding and focused verification.

## T038 Branch Evidence

- `git ls-remote --heads origin t038-operator-config-snapshot` returned `53c43608e74d7d8293c8830f57ed180d94bb7c5a`.
- `git fetch origin t038-operator-config-snapshot` fetched that head.
- `git log --oneline -12 FETCH_HEAD` showed the old branch commits:
  - `bced44fe config: add bolt-v3 t038 operator snapshot`
  - `48201c32 fix: build no-submit live node before async readiness`
  - `36a50aa1 fix: tighten no-submit readiness connect path`
  - `b7c4d419 fix: align bolt v3 live config with NT clients`
  - `f6e3dcc8 fix: pin NT for Binance SBE v4`
  - `33f6c738 fix: use NT start for no-submit readiness`
  - `2849fb73 fix: pump NT events during no-submit connect`
  - `53c43608 fix: bind no-submit events before client registration`
- `git log --oneline --all --grep='no-submit|Binance SBE|SBE v4|controlled connect|reference readiness'` showed later current-main no-submit/SBE work, including `ddace928 Unblock no-submit transport and Binance SBE (#408)`, `973cb4f3 fix: run no-submit readiness from sync boundary`, `d69b43c2 fix: harden no-submit reference readiness evidence`, and `85ec589d fix: harden no-submit readiness waits`.
- Current readiness branch tests assert no-submit uses `node.run()` in the strategy-free helper and must not use `LiveNode::start` because `start` does not drain execution account events.
- `specs/001-thin-live-canary-path/tasks.md` records a later EC2/EIP T038 pass at head `1245264f294ae096155bffc3236fb692cc46b46f`, with all seven no-submit stages satisfied.

Implication: do not port `t038-operator-config-snapshot` wholesale. The remaining task is a targeted port audit for any still-missing behavior or operator snapshot evidence after current no-submit code and docs are considered.

T006 completed in `specs/024-production-trade-readiness/t038-port-audit.md`: no exact old `origin/t038-operator-config-snapshot` behavior needs to be ported into current `main` / PR #480 no-submit readiness code.

T008 completed in `specs/024-production-trade-readiness/issue-385-no-submit.md`: historical T038 no-submit is satisfied only for the May 22 EC2/EIP no-submit run, while final-packet T131/T122 no-submit remains unproven until the verified final packet exists and the exact-head EC2/EIP no-submit rerun is executed.

## PortfolioSnapshot Evidence For #409

Current source already contains:

- `src/nt_runtime_capture.rs` imports `subscribe_portfolio_snapshot` and `unsubscribe_portfolio_snapshot`.
- `src/nt_runtime_capture.rs` defines `portfolio_snapshot/snapshots.jsonl` capture paths.
- `src/nt_runtime_capture.rs` writes `CaptureMessage::PortfolioSnapshot` to `jsonl_paths.portfolio_snapshots`.
- `tests/nt_runtime_capture.rs` publishes a `PortfolioSnapshot` and asserts one JSONL row is written.
- `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml` marks the PortfolioSnapshot stream captured.
- `scripts/verify_runtime_capture_yaml.py` includes PortfolioSnapshot capture checks.

Implication: #409 may be closable or may need PR/issue evidence updates, but it does not appear to be a blocker for T126/T127 collector implementation.

T007 completed in `specs/024-production-trade-readiness/issue-409-portfolio-snapshot.md`: PortfolioSnapshot-specific source, tests, docs, and runtime-capture verifier acceptance are satisfied, but #409 should not be closed until PR #480 source-fence/CI are green or the unrelated source-fence blocker is explicitly waived for the #409 slice.

## Local Verification After Source-Fence Fix

- `git diff --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_schema_current.py`: `OK: Bolt-v3 schema/status docs match current order-intent source scope.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_schema_current.py`: `OK: Bolt-v3 schema-current verifier self-tests passed.`
- `just source-fence`: passed after rerun with approved access to `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`.

## Task-List Gate Disposition

T004/T005 are complete by recorded reviewer skip, not by unanimous six-reviewer approval. Gemini, DeepSeek, GLM, and Grok approved the corrected task-list direction. Claude is skipped because repeated OAuth subscription attempts failed before source transmission with `oauth_inference_rejected` / HTTP 401. Kimi is skipped because repeated attempts produced no verdict; the latest sent source, session `1351b5ee-2d46-4f7f-8c79-5bb6ae919dc0`, then failed with `step_limit_exceeded` at max step budget `50`. See `specs/024-production-trade-readiness/external-tasklist-review.md`.

## T019/T020 Egress Identity Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_egress_identity_source_proof_derives_source_owned_values -- --nocapture` failed with `E0425` because `collect_pre_run_egress_identity_source_proof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_egress_identity_source_proof -- --nocapture` passed: 4 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collector is non-live and source-owned: it reads a bounded local JSON source, validates schema/record kind, validates lowercase observed/approved egress identity hashes, fails closed on mismatch, returns only approval plus hashes, and does not write `pre-run-state.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T019/T020.

## T015/T016 Venue Account State Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof_derives_source_owned_absence -- --nocapture` failed with `E0425` because `collect_pre_run_venue_account_state_source_proof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof -- --nocapture` passed: 4 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collector is non-live and source-owned: it reads a bounded local JSON source, validates schema/record kind, validates a lowercase account-state snapshot hash, requires zero open orders and zero open positions, fails closed on present orders/positions or invalid shape, returns only absence booleans plus hashes, and does not write `pre-run-state.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T015/T016.

## T017/T018 Funding Margin Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_funding_margin_source_proof_derives_source_owned_coverage -- --nocapture` failed with `E0425` because `collect_pre_run_funding_margin_source_proof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_funding_margin_source_proof -- --nocapture` passed: 4 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collector is non-live and source-owned: it reads a bounded local JSON source, validates schema/record kind, validates a lowercase margin snapshot hash, parses collateral and required max-notional-plus-fees as decimals, requires positive required coverage, fails closed when available collateral is insufficient, returns only a coverage boolean plus hashes, and does not write `pre-run-state.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T017/T018.

## T021/T022 CLOB V2 Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_clob_v2 -- --nocapture` failed with `E0425` because `collect_pre_run_clob_v2_adapter_signing_source_proof`, `collect_pre_run_clob_v2_collateral_accounting_source_proof`, and `collect_pre_run_clob_v2_fee_behavior_source_proof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_clob_v2 -- --nocapture` passed: 7 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collectors are non-live and source-owned: they read bounded local JSON source proofs, validate schema/record kinds, validate lowercase source/evidence hashes, bind adapter signing to the caller-provided release-manifest CLOB signing version, require recovered-signer match proof, derive pUSD collateral coverage from balance/allowance/required decimals, require the CLOB V2 fee-behavior policy booleans and valid price/fee-rate bounds, return only booleans plus hashes, and do not write `pre-run-state.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T021/T022.

## T023/T024 Source-Owned T126 Pre-Run-State Binding Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_state_writer_emits_artifact_from_source_owned_collectors -- --nocapture` failed with `E0425`/`E0422` because `write_pre_run_state_artifact_from_source_collectors` and `PreRunStateSourceCollectorInputs` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_state_writer_emits_artifact_from_source_owned_collectors -- --nocapture` passed: 1 passed, 0 failed.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_pre_run_state_source_collector_command -- --nocapture` failed because `generate-pre-run-state-from-source-collectors` was not exposed.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_pre_run_state_source_collector_command -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_ -- --nocapture`: 52 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

T126 no longer depends on a caller-supplied pre-run source bundle for local artifact generation. `write_pre_run_state_artifact_from_source_collectors` and the `operator-artifacts generate-pre-run-state-from-source-collectors` CLI path collect release-manifest, host-clock, venue-account-state, market/window, funding/margin, single-runner-lock, egress-identity, CLOB V2 adapter-signing, CLOB V2 collateral-accounting, and CLOB V2 fee-behavior proofs from bounded local source inputs, then write the final `pre-run-state.json`. The schema-level operator evidence config still binds the final artifact by `pre_run_state_path` and `pre_run_state_sha256`; the approved root TOML update remains T037 with the full final packet.

## T024A Venue-Account Identity Binding Repair Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof_rejects_mismatched_config_identity -- --nocapture` failed with `E0061` because `collect_pre_run_venue_account_state_source_proof` accepted only the source path and byte cap, with no expected execution-client or target identity.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof_rejects_mismatched_config_identity -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof -- --nocapture`: 5 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_state_writer_emits_artifact_from_source_owned_collectors -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_ -- --nocapture`: 53 passed, 0 failed.
- `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_pre_run_state_source_collector_command -- --nocapture`: 1 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `just fmt-check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.
- `just clippy`: passed.

The venue-account source proof now requires `execution_client_id` and `configured_target_id` fields and includes them in the source-proof hash input. `write_pre_run_state_artifact_from_source_collectors` derives the expected values from the loaded strategy's financial envelope before collecting venue-account proof. A zero open-order/open-position snapshot from another account or target now fails closed before it can satisfy T126. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T024A.

## T024B Pre-Run Price-Source Override Repair Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts pre_run_state_writer_rejects_caller_supplied_price_source_override -- --nocapture` failed because `write_pre_run_state_artifact_from_source_collectors` accepted a caller-supplied `manual_source_override` price-to-beat source and wrote `pre-run-state.json`.
- GREEN: `cargo test --test bolt_v3_operator_artifacts pre_run_state_writer_rejects_caller_supplied_price_source_override -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_state -- --nocapture`: 7 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts pre_run_market_window_source_proof -- --nocapture`: 6 passed, 0 failed.
- `cargo test --test bolt_v3_cli source_collector -- --nocapture`: 2 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `git diff --check`: passed.

`PreRunStateSourceCollectorInputs` no longer carries `expected_price_to_beat_source`, and the CLI no longer exposes `--expected-price-to-beat-source` for `generate-pre-run-state-from-source-collectors`. The pre-run writer derives the expected price-to-beat source from the loaded TOML financial envelope before validating strategy-input evidence, so a tampered strategy-input artifact cannot be made valid by passing a matching caller override. This is non-live local artifact validation only. No AWS, SSM, external network, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run.

## T025/T026 NT-Accepted Venue-Pending Abort Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts abort_plan_nt_accepted_venue_pending -- --nocapture` failed with `E0425`/`E0422` because `collect_abort_plan_nt_accepted_venue_pending_source_proof` and `Phase8AbortPlanNtAcceptedVenuePendingSourceProof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts abort_plan_nt_accepted_venue_pending -- --nocapture` passed: 2 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 21 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collector is non-live and source-owned: it reads bounded local Rust source, validates that `try_submit_exit_order` creates `ExitPending`/`PendingExitState` before `submit_order_with_decision_evidence`, validates submit-error restoration to `Managed`, validates cancel/reject/expire terminal handlers call `mark_exit_order_terminal`, returns only a source-proof hash, and does not write `abort-plan.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T025/T026.

## T027/T028 Partial-Fill Abort Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts abort_plan_partial_fill -- --nocapture` failed with `E0425`/`E0422` because `collect_abort_plan_partial_fill_source_proof` and `Phase8AbortPlanPartialFillSourceProof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts abort_plan_partial_fill -- --nocapture` passed: 2 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 23 passed, 0 failed.
- `cargo fmt --check`: passed.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.

The collector is non-live and source-owned: it reads bounded local Rust source, validates that exit fills set `fill_received` and wait for `close_received` before flat, validates position-close completion sets `close_received` and clears the stored position, validates residual-after-fill state preservation, validates terminal-without-flat preserves managed exposure instead of falsely flattening, returns only a source-proof hash, and does not write `abort-plan.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T027/T028.

## T029/T030 Network-Partition Abort Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts abort_plan_network_partition -- --nocapture` failed with `E0425`/`E0422` because `collect_abort_plan_network_partition_source_proof` and `Phase8AbortPlanNetworkPartitionSourceProof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts abort_plan_network_partition -- --nocapture` passed: 2 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 25 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

The collector is non-live and source-owned: it reads bounded local Rust source, validates that `try_submit_exit_order` calls `submit_order_with_decision_evidence`, validates submit-error restoration to `Managed`, validates the submit error is returned instead of swallowed, returns only a source-proof hash, and does not write `abort-plan.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T029/T030.

## T031/T032 Panic-Gate And Service-Policy Abort Collector Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts abort_plan_panic_gate_service_policy -- --nocapture` failed with `E0425`/`E0422` because `collect_abort_plan_panic_gate_service_policy_source_proof` and `Phase8AbortPlanPanicGateServicePolicySourceProof` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts abort_plan_panic_gate_service_policy -- --nocapture` passed: 2 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 27 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

The collector is non-live and source-owned: it reads bounded local Rust source for the strategy and submit-admission modules, validates cache-probe panic containment into `BlindRecovery`, validates debug-only invariant panic with release-mode report/error return, validates service-submit lifecycle policy derives from the strategy config, validates submit admission rejects unarmed and lifecycle-disallowed submits before admission, validates replace-submit is gated by policy while entry/risk-reducing exits remain allowed, returns only a source-proof hash, and does not write `abort-plan.json`. No AWS, SSM, external network, no-submit, live trading, or secret-source commands were run for T031/T032.

## T033/T034 Source-Owned T127 Abort-Plan Binding Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts abort_plan_writer_emits_artifact_from_source_owned_collectors -- --nocapture` failed with `E0425` because `write_abort_plan_artifact_from_source_collectors` did not exist.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_abort_plan_source_collector_command -- --nocapture` failed because `generate-abort-plan-from-source-collectors` was not exposed.
- GREEN: `cargo test --test bolt_v3_operator_artifacts abort_plan_writer_emits_artifact_from_source_owned_collectors -- --nocapture` passed: 1 passed, 0 failed.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_abort_plan_source_collector_command -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 28 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

T127 no longer depends on a caller-supplied abort source bundle for local artifact generation. `write_abort_plan_artifact_from_source_collectors` and the `operator-artifacts generate-abort-plan-from-source-collectors` CLI path collect cancel-if-open, NT-accepted/venue-pending, partial-fill, network-partition, and panic/service-policy proofs from bounded local source inputs, then write the final `abort-plan.json`. The schema-level operator evidence config still binds the final artifact by `abort_plan_path` and `abort_plan_sha256`; the approved root TOML update remains T037 with the full final packet.

## T034A Abort-Plan Collector Provenance Repair Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_abort_plan_built_from_synthetic_source_proofs -- --nocapture` failed because `verify_final_operator_packet` accepted an abort-plan artifact built from synthetic caller-supplied proof hashes.
- GREEN: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_abort_plan_built_from_synthetic_source_proofs -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts abort_plan_ -- --nocapture`: 29 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet_verifier -- --nocapture`: 32 passed, 0 failed.
- `cargo test --test bolt_v3_tiny_canary_preconditions operator_approval_envelope -- --nocapture`: 8 passed, 0 failed.
- `cargo test --test bolt_v3_tiny_canary_operator phase8_operator_envelope -- --nocapture`: 8 passed, 0 failed.
- `cargo test --test bolt_v3_cli abort_plan -- --nocapture`: 1 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `git diff --check`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

Abort-plan artifacts now distinguish collector-derived evidence from caller-supplied proof hashes. `write_abort_plan_artifact_from_source_collectors` writes `source_collector_derived = true` plus the bounded strategy and submit-admission source hashes, and final-packet verification requires those hashes to match the source compiled into the current binary. Source-proof and source-bundle writers remain usable for local tests and diagnostics, but their artifacts cannot satisfy final-packet readiness. This is non-live local artifact validation only. No AWS, SSM, external network, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run.

## T011/T012 Runtime Strategy-Input JSONL Binding Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_non_runtime_decision_evidence_jsonl_for_strategy_input -- --nocapture` failed because the final-packet verifier accepted a non-runtime `decision-evidence.jsonl` path and returned a successful verification summary instead of rejecting the packet.
- GREEN: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_non_runtime_decision_evidence_jsonl_for_strategy_input -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_ -- --nocapture`: 29 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts strategy_input -- --nocapture`: 10 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just clippy`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

T125 strategy-input replay is now bound to the configured runtime decision-evidence JSONL path. The verifier resolves `[live_canary.operator_evidence].decision_evidence_path`, compares it to the canonical `[persistence]` `decision_evidence_path(&loaded)`, and fails closed with `strategy_input_replay.decision_evidence_path` before replaying if the paths differ. Existing strategy code remains the runtime producer: `binary_oracle_edge_taker` records the strategy snapshot, order intent, and admission decision through the decision-evidence writer, and the final packet now accepts replay only from that configured runtime JSONL location. Final root TOML operator-evidence values remain T037.

## T009/T010 Runtime Market-Selection Source Binding Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_fixture_market_selection_source_for_t124 -- --nocapture` failed because the final-packet verifier accepted a copied fixture/static `market-selection-source.json` and returned a successful verification summary instead of rejecting the packet.
- GREEN: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_fixture_market_selection_source_for_t124 -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_ -- --nocapture`: 30 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts market_selection -- --nocapture`: 13 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts strategy_input -- --nocapture`: 10 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just clippy`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

T124 market-selection replay is now bound to runtime provenance, not fixture consistency alone. `market-selection-source.json` produced through the decision-evidence plus instrument-source path records decision-evidence path/hash and instrument-source path/hash. Final-packet replay now requires that provenance, verifies the decision-evidence path matches the configured runtime JSONL path, verifies both source hashes, parses the instrument source, recomputes the market-selection source from the current config, runtime decision evidence, and instrument facts, and rejects copied fixture/static market-selection sources. Final root TOML operator-evidence values remain T037.

## T013/T014 Operator-Evidence Binding And Focused T124/T125 Verification

- `git ls-files config/live.local.toml config '*.toml' | sort` showed no tracked `config/live.local.toml`; the repo-owned config surface is `config/root.example.toml` plus test fixtures.
- `ls -la config` showed no local `config/live.local.toml` in this worktree. No live-local TOML was read, printed, or edited.
- `rg` over `src/bolt_v3_config.rs` and `config/root.example.toml` shows the schema-level T124/T125 binding fields are `strategy_input_evidence_path`, `strategy_input_evidence_sha256`, and `decision_evidence_path`; there is no standalone top-level market-selection operator-evidence field.
- `rg` over `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` shows existing audit rows for `strategy_input_evidence_path`, `strategy_input_evidence_sha256`, and `decision_evidence_path`.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`

Conclusion: T124 binds through the `strategy_input_evidence_path`/`strategy_input_evidence_sha256` artifact because `strategy-input.json` carries `market_selection_source_path`/`market_selection_source_sha256`, and final replay now requires nested market-selection runtime provenance. T125 binds through `[live_canary.operator_evidence].decision_evidence_path`, which final replay now requires to equal the configured `[persistence]` runtime decision-evidence JSONL path. The actual approved root TOML values and final artifact hashes are not available until T035/T036 assemble the final packet, so `config/live.local.toml` remains T037 and was not touched in this slice.

## T035 T128 Final-Packet Source-Artifact Guard Evidence

- RED: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_hash_only_t126_t127_static_artifacts -- --nocapture` failed because `verify_final_operator_packet` accepted hash-matched marker files for T126/T127 static artifacts and returned a successful final-packet verification summary.
- GREEN: `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_rejects_hash_only_t126_t127_static_artifacts -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts approval_packet_assembly_binds_relative_static_manifest_to_config_root -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts approval_packet_assembly -- --nocapture`: 15 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet_verifier_ -- --nocapture`: 31 passed, 0 failed.
- `cargo fmt --check`: passed.
- `python3 -B scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 -B scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `git diff --check`: passed.
- `just fmt-check`: passed.
- `just clippy`: passed.
- `just source-fence`: passed, including 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

The final-packet verifier now parses and validates the financial envelope, pre-run-state artifact, and abort-plan artifact against the loaded root TOML instead of accepting only static-manifest path/hash agreement. T036/T037 remain open because real approved artifact files and `config/live.local.toml` operator-evidence bindings have not been produced in this worktree.

## T035A T128 Approval-Envelope Hash CLI Evidence

- Read-only subagent audit `019e5bd8-94ae-7f02-8275-537100c99b9c` found that `operator-artifacts assemble-final` requires `[live_canary.operator_evidence].approval_envelope_sha256` to already equal the canonical approval-envelope hash before it writes `approval-envelope.json` or `operator-evidence-packet.json`, while `compute_operator_approval_envelope_sha256` existed only as a Rust library function.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_computes_approval_envelope_sha256_without_printing_operator_paths -- --nocapture` failed because `operator-artifacts compute-approval-envelope-sha256` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_computes_approval_envelope_sha256_without_printing_operator_paths -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_final_operator_packet_assembly_command -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_static_manifest_from_operator_evidence_command -- --nocapture`: 1 passed, 0 failed.

`operator-artifacts compute-approval-envelope-sha256 --config <root.toml>` now loads the approved root TOML, computes the canonical hash through the same non-circular approval-envelope construction used by final packet assembly, and prints only `{ "sha256": "..." }`. It does not read AWS/SSM, run no-submit, submit/cancel orders, mutate live config, or print operator evidence paths, raw approval IDs, nonce material, or secrets. T036/T037 remain open until real artifacts are present and the approved root TOML is updated with the final operator-evidence paths/hashes.

## T036 Prerequisite Entry-Decision Evidence From Source

- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_entry_decision_evidence_source_command -- --nocapture` failed because `operator-artifacts generate-entry-decision-evidence-from-source` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_entry_decision_evidence_source_command -- --nocapture` passed: 1 passed, 0 failed.
- GREEN: `cargo test --test bolt_v3_operator_artifacts entry_decision_evidence_source_collector -- --nocapture` passed: 3 passed, 0 failed. This covers configured JSONL generation, invalid source `price_precision` failing closed without panic, and symlinked configured JSONL rejection before append.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `just clippy`: passed.
- `just source-fence` after reviewer fix: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts generate-entry-decision-evidence-from-source --config <root.toml> ...` now creates the configured `[persistence]` decision-evidence JSONL from bounded source files and the real `BinaryOracleEdgeTaker` entry path. It consumes a source-owned decision input plus the same instrument-source JSON shape used by market-selection replay, registers the strategy core with the root TOML trader id and an explicit NT cache, injects the source quote/volatility/fee/book facts, and lets unarmed submit admission reject the entry order after strategy-input, order-intent, and admission evidence are written. This is a non-live path: it does not read AWS/SSM, run no-submit, connect to a venue, submit/cancel orders, mutate `config/live.local.toml`, or print secrets.

## T035C T128 Pre-Run Final-Packet Verification Scope

- Read-only subagent audits `019e5c26-c203-78b1-806d-1fcb254d83b9` and `019e5c26-d505-7881-9472-a2cf17e8fea0` found two current T036/T038 gaps: real source input files are still absent, and the existing `operator-artifacts verify-final` path required live/no-submit result files such as `canary_evidence_path`, `approval_consumption_path`, `nt_submit_event_path`, `venue_order_state_path`, `restart_reconciliation_path`, and `post_run_hygiene_path`. Those result files cannot honestly exist before the T043/T044 operations.
- RED: `cargo test --test bolt_v3_operator_artifacts final_packet_pre_run_verifier_accepts_packet_before_live_result_evidence_exists -- --nocapture` failed with missing `FinalOperatorPacketVerificationScope` and missing `verify_final_operator_packet_with_scope`.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_final_operator_packet_verifier_command -- --nocapture` failed because help output did not expose `--verification-stage`.
- GREEN: `cargo test --test bolt_v3_operator_artifacts final_packet_pre_run_verifier_accepts_packet_before_live_result_evidence_exists -- --nocapture`: 1 passed, 0 failed.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_final_operator_packet_verifier_command -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet -- --nocapture`: 32 passed, 0 failed.
- `cargo test --test bolt_v3_cli operator_artifacts -- --nocapture`: 2 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.
- `just clippy`: passed.

`operator-artifacts verify-final` now has an explicit `--verification-stage pre-run|post-run` stage. The default remains post-run and still requires the final live/no-submit evidence files. The pre-run stage verifies the operator packet, static manifest, approval envelope, source-owned static readiness artifacts, and strategy-input replay binding before T043/T044 produce result evidence. This is non-live verification only; no AWS, SSM, no-submit, venue connection, order submit/cancel, or `config/live.local.toml` mutation was run.

T036 remains open. The final packet is not assembled until real source inputs exist, the configured market-selection/strategy-input/pre-run/abort/approval artifacts are written together, `config/live.local.toml` is updated in T037, and `operator-artifacts verify-final` passes in T038.

## T035D T037 Operator-Evidence TOML Patch Command

- RED: `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher_updates_only_operator_evidence_block_from_json -- --nocapture` failed with `E0425` because `update_live_canary_operator_evidence_toml_from_json_file` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher_updates_only_operator_evidence_block_from_json -- --nocapture` passed: 1 passed, 0 failed.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_updates_operator_evidence_toml_without_printing_evidence_values -- --nocapture` failed because `update-operator-evidence-toml` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_updates_operator_evidence_toml_without_printing_evidence_values -- --nocapture` passed: 1 passed, 0 failed.
- `cargo test --test bolt_v3_cli operator_artifacts -- --nocapture`: 2 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts final_packet -- --nocapture`: 32 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `just clippy`: passed.
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts update-operator-evidence-toml --config <root.toml> --operator-evidence-json <json> --max-operator-evidence-json-bytes <bytes>` now reads a bounded JSON `LiveCanaryOperatorEvidenceBlock`, validates the current build head, validates configured hashes and path shape, patches only `[live_canary.operator_evidence]`, re-parses the full root TOML, writes the root TOML, and prints only `{ "root_toml_sha256": "..." }`. It does not read AWS/SSM, run no-submit, connect to a venue, submit/cancel orders, mutate live systems, or print approval IDs, artifact paths, nonce material, or secrets.

T037 remains open. This command enables the approved root TOML patch after T036 produces the real final artifact paths/hashes; it did not patch `config/live.local.toml` in this slice.

## T035E T037 Static Artifact Patch Gate

- Read-only subagent review `019e5cd7-e42d-7330-aff3-0cb4c43e3174` found that `update-operator-evidence-toml` could patch `[live_canary.operator_evidence]` after validating hash shape and path shape, but before proving the configured static artifact files were materialized.
- RED: `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher_rejects_unmaterialized_static_artifact_bindings_before_patch -- --nocapture` failed because the patcher accepted absent static artifact bindings and wrote the TOML.
- GREEN: `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher_rejects_unmaterialized_static_artifact_bindings_before_patch -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher -- --nocapture`: 2 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts update-operator-evidence-toml` now refuses to mutate the root TOML until `ssm-manifest`, `strategy-input`, `financial-envelope`, `pre-run-state`, `abort-plan`, and `approval-nonce` configured paths exist as bounded regular files and hash to their configured sha256 values. It intentionally does not require the approval-envelope file or post-run live/no-submit files at patch time because `assemble-final` writes the approval envelope and `verify-final --verification-stage pre-run` runs before later live result evidence exists.

This is non-live local artifact validation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run. T036/T037 remain open until real source-owned artifacts exist, the approved root TOML is patched, and the final packet verifies.

## T035F T037 Operator-Evidence JSON Generation

- Read-only subagent review `019e5cd7-cb9c-7df0-b787-133cddd2fc1d` confirmed the current T036/T037 chain still required a manually supplied full `LiveCanaryOperatorEvidenceBlock` JSON before the TOML patch step.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_generates_operator_evidence_json_without_printing_values -- --nocapture` failed because `operator-artifacts generate-operator-evidence-json` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_generates_operator_evidence_json_without_printing_values -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_cli operator_evidence -- --nocapture`: 3 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts operator_evidence_toml_patcher -- --nocapture`: 2 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts generate-operator-evidence-json --config <root.toml> --output <json> ...` now reads bounded materialized static artifacts, computes their sha256 values, fills the current build `head_sha`, computes canonical `approval_envelope_sha256` from the same approval-envelope construction used by final assembly, and writes a full `LiveCanaryOperatorEvidenceBlock` JSON for the T037 patch step. It prints only `operator_evidence_json_sha256`; it does not print artifact paths, approval IDs, nonce material, or secrets, and it does not write `approval-envelope.json`.

This is non-live local artifact generation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run. T036/T037 remain open until real source-owned artifacts exist, the generated operator-evidence JSON is applied to the approved root TOML, and the final packet verifies.

## T036A Entry-Decision Source Input Collector

- RED: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input_collector_writes_replayable_real_source_files -- --nocapture` failed with unresolved collector API/types because `write_entry_decision_source_inputs_from_source_files`, `EntryDecisionSourceInputRequest`, `EntryDecisionSourceMarketInputs`, and `EntryDecisionSourceBookSideInput` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input_collector -- --nocapture` passed: 4 passed, 0 failed. This covers replayable source/instrument file writing, missing source-bound `price_to_beat`, incomplete selected-market instruments, and empty or one-sided books.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_entry_decision_source_inputs -- --nocapture` failed because `operator-artifacts collect-entry-decision-source-inputs` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_entry_decision_source_inputs -- --nocapture` passed: 1 passed, 0 failed.
- Read-only subagent review `019e5c78-2147-77a3-954f-2bfea8825228` found four T036A hardening gaps: provider collection could fetch Gamma/CLOB before validating all local source proofs, crossed books could be written before replay rejected them, book `price_precision` came from the first instrument instead of the selected up/down pair, and provider retry policy still carried non-TOML fixed fields after removing `RetryConfig::default()`.
- RED: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input -- --nocapture` failed on the four review regressions: provider returned `failed to load instruments by configured slugs` before local reference-quote validation, crossed books wrote artifacts, first-instrument precision `6` was written instead of selected precision `3`, and mismatched selected up/down precision wrote artifacts.
- GREEN: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input -- --nocapture`: 8 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts entry_decision -- --nocapture`: 11 passed, 0 failed.
- `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_entry_decision_source_inputs -- --nocapture`: 1 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `just clippy`: passed.
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-entry-decision-source-inputs --config <root.toml> --strategy-instance-id <id> ...` now writes replayable `entry-decision-source.json` and `instrument-source.json` from bounded source proofs before T036 final-packet assembly. Core artifact code validates source-owned price, quote, volatility, selected instruments, fee proof, and two-sided non-crossed books; book precision is derived from the selected up/down instrument pair and rejected if the pair disagrees. The Polymarket provider binding owns the public Gamma/CLOB collection path, validates local source proofs before the first provider fetch, and sources retry count, delay, jitter, operation timeout, and elapsed bound from the configured TOML execution/data client blocks instead of production defaults. This is non-live public/source capture only: it does not read AWS/SSM, run no-submit, connect to private execution, submit/cancel orders, mutate `config/live.local.toml`, or print secrets.

T036 remains open. The real source proofs and live operator artifact files still need to be captured/assembled into the final packet and bound into the approved root TOML before T038 can verify the packet.

## Current-Head CI Compile Fix

- PR #480 exact head `603eae9033b770c7eeef090d7ac4e905e0c8625f` had failing CI checks: `nextest archive`, `test`, and `gate`.
- `gh run view 26377089210 --repo seungpyoson/bolt-v2 --log-failed` showed the root compile error: `missing field collect_entry_decision_source_inputs in initializer of ProviderBinding` in three test-only fake provider bindings in `src/bolt_v3_adapters.rs`.
- The downstream `test` and `gate` failures were secondary because the test archive did not build.
- Fix: set `collect_entry_decision_source_inputs: None` on the three fake `ProviderBinding` test fixtures in `src/bolt_v3_adapters.rs`.
- Verification after fix:
  - `cargo test --lib bolt_v3_adapters -- --nocapture`: 10 passed, 0 failed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.

This is a branch compile repair only. It does not close T036/T037/T038 and does not claim production trade readiness.

## T036 Static Assembly Rerun Evidence

- Command: `cargo run --bin bolt-v2 -- operator-artifacts generate-static --config /Users/spson/Projects/Claude/bolt-v2/config/live.local.toml --output-dir /private/tmp/bolt-t036-static-603eae-rerun --strategy-instance-id bitcoin_updown_main`
- Result: generated `ssm-manifest.json`, `financial-envelope.json`, `approval-nonce.json`, and `static-artifacts-manifest.json`, then failed closed with the expected blockers:
  - `market-selection remains blocked: T046 missing source-bound price-to-beat strategy decision input`
  - `T046 remains blocked: missing source-bound price-to-beat strategy decision input`
  - `T121 remains blocked: T046 source-bound pre-run state evidence is unproven`
  - `panic gate and service policy`

This confirms T036 final-packet assembly is still blocked by real source-owned decision/pre-run evidence, not by static-manifest generation itself.

## T036A Price-To-Beat Report Provenance Hardening

- RED: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input_collector_refuses_price_to_beat_without_report_provenance -- --nocapture` failed because a weak `source-bound-price.json` carrying only source name, value, and timestamps was accepted and wrote `entry-decision-source.json`/`instrument-source.json`.
- GREEN: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input_collector_refuses_price_to_beat_without_report_provenance -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input -- --nocapture`: 9 passed, 0 failed.
- `cargo test --test bolt_v3_operator_artifacts entry_decision -- --nocapture`: 12 passed, 0 failed.
- `cargo test --test bolt_v3_cli collect_entry_decision_source_inputs -- --nocapture`: 1 passed, 0 failed.
- `cargo test --test bolt_v3_strategy_registration binary_oracle -- --nocapture`: 17 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`

`source-bound-price.json` now must include Chainlink report provenance bound to the strategy TOML: `source_report_schema_version`, `source_report_feed_id`, `source_report_decimal_scale`, `source_report_full_sha256`, `source_report_valid_from_timestamp_ms`, `source_report_observations_timestamp_ms`, and `source_report_benchmark_price`. The collector rejects missing/mismatched report schema, feed id, decimal scale, report hash shape, timestamp ordering, or benchmark price before writing replayable decision/instrument source inputs. The live strategy builder raw config surface remains unchanged; these fields are consumed by operator-evidence validation, not by the NT strategy runtime.

This is non-live source-proof validation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run. T036 remains open until the real operator-approved report source, public market source inputs, source-owned pre-run/abort proofs, final packet assembly, and T037 root TOML patch exist.

## T036B Entry-Decision Proof Source Materializer

- Read-only subagent `019e5cf1-3f34-7a70-9f30-06c877fcedcf` confirmed current code could produce `entry-decision-source.json` and `instrument-source.json` only after the operator supplied four proof files: `source-bound-price.json`, `reference-quote.json`, `realized-volatility.json`, and `entry-decision-fees.json`. It also confirmed abort-plan can be produced from current repo source files, while real pre-run source files remain unmaterialized.
- RED: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture` failed with unresolved imports/functions because `EntryDecisionProofSourceMaterializationRequest`, `EntryDecisionReferenceQuoteProofInput`, `EntryDecisionRealizedVolatilityProofInput`, and `write_entry_decision_proof_source_files` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 2 passed, 0 failed.
- RED: `cargo test --test bolt_v3_cli collect_entry_decision_proof_sources -- --nocapture` failed because `operator-artifacts collect-entry-decision-proof-sources` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli entry_decision_proof_sources -- --nocapture`: 2 passed, 0 failed.
- Code-quality sidecar `019e5d19-445f-71d3-a3fd-9fb6f812717e` found two T036B production-grade gaps after initial commit: Chainlink report bytes were only hash-bound rather than parsed, and quote/volatility timestamps were not bounded by market-selection and decision timestamps.
- RED: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture` failed because the materializer request still accepted caller-supplied `source_report_*` fields after tests were changed to require report-derived fields.
- GREEN after reviewer fix: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 4 passed, 0 failed.
- GREEN after reviewer fix: `cargo test --test bolt_v3_cli entry_decision_proof_sources -- --nocapture`: 2 passed, 0 failed.
- Regression check: `cargo test --test bolt_v3_operator_artifacts entry_decision_source_input_provider -- --nocapture`: 1 passed, 0 failed.
- Broader entry-decision check after reviewer fix: `cargo test --test bolt_v3_operator_artifacts entry_decision -- --nocapture`: 16 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-entry-decision-proof-sources --config <root.toml> --strategy-instance-id <id> ...` now writes the four proof files required by the existing T036A source-input collector. It reads a bounded regular-file operator-approved Chainlink report source JSON, verifies its sha256 against the operator-supplied approved report hash, parses the Chainlink `fullReport` ABI payload, derives feed id, valid-from timestamp, observations timestamp, and benchmark price from the report payload, cross-checks those report fields against TOML-bound feed/schema/decimal-scale config, derives the approved price source from the financial-envelope config, bounds quote/volatility timestamps to the market-selection and decision window, validates fee proof inputs, and writes `source-bound-price.json`, `reference-quote.json`, `realized-volatility.json`, and `entry-decision-fees.json` with create-new semantics. The provider path now consumes the same shared fee-proof artifact type/validator as the materializer instead of maintaining a second fee-proof schema path.

This is non-live local source-proof materialization only. No AWS, SSM, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run. T036 remains open until the real operator-approved source payloads and pre-run source files are captured, all static artifacts are produced, the root TOML is patched, and `operator-artifacts verify-final --verification-stage pre-run` passes.

## T036C Base Static Artifact Generator

- Read-only subagent `019e5d1c-4db3-7641-aefe-3ee37af33d30` confirmed that T036 cannot honestly close before the T037 TOML patch because final assembly consumes `[live_canary.operator_evidence]`, and it identified one remaining code gap on the critical path: there was no successful non-live CLI path for only the base static artifacts `ssm-manifest.json`, `financial-envelope.json`, and `approval-nonce.json`.
- RED: `cargo test --test bolt_v3_operator_artifacts base_static_operator_artifacts -- --nocapture` failed because `write_base_static_operator_artifacts` did not exist.
- GREEN: `cargo test --test bolt_v3_operator_artifacts base_static_operator_artifacts -- --nocapture`: 1 passed, 0 failed.
- RED/GREEN CLI: `cargo test --test bolt_v3_cli base_static_operator_artifacts -- --nocapture`: 2 passed, 0 failed after adding `operator-artifacts generate-base-static`.
- Regression after sidecar Chainlink `fullReport` compatibility finding: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 4 passed, 0 failed.
- Regression: `cargo test --test bolt_v3_cli entry_decision_proof_sources -- --nocapture`: 2 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- Historical non-live operator-config command-path proof: copied the ignored root TOML into this PR worktree without printing it, confirmed `config/live.local.toml` is ignored by `.gitignore`, then ran `cargo run --bin bolt-v2 -- operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-trade-readiness-4e583417/base-static-pr-worktree --strategy-instance-id bitcoin_updown_main` at head `4e583417460c602f57c1c3e6f271992145cae580`. The command passed and wrote `ssm-manifest.json` (`8b12f2a636961b3bb35dce203c66b26f482537e102a6bbf96aae592ddcb4da4a`), `financial-envelope.json` (`0fe8aef150af7156ece1db2c2b8b0c738a51352dd4837ba4eb7d13e0469cd253`), and `approval-nonce.json` (`2fa765155835265251783b7aeb73b7b2342d1a624958c73d5f717fef5c274ec5`). A prior attempt using `/Users/spson/Projects/Claude/bolt-v2/config/live.local.toml` failed because that main-checkout root TOML resolved `strategies/binary_oracle.example.toml` from the stale main checkout, which lacks the PR's tracked `price_to_beat_*` strategy fields. This run is command-path evidence only, not production readiness evidence; the later fail-closed validation below intentionally keeps the shipped operator/example strategy placeholder invalid until a real operator-approved feed id is supplied.

`operator-artifacts generate-base-static --config <root.toml> --strategy-instance-id <id> --output-dir <dir>` now writes only the unblocked base static artifacts and prints redacted path/sha256 refs. It does not write `static-artifacts-manifest.json`, `strategy-input.json`, `pre-run-state.json`, or `abort-plan.json`, and it does not expose raw secret paths or nonce material in stdout. The existing fail-closed `generate-static` blocker-manifest path remains intact for audit diagnostics.

This is non-live local static artifact generation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, `config/live.local.toml` mutation, or live trading side effect was run. T036 still requires real source-input collection, pre-run/abort artifact generation, T037 root TOML patch, final manifest/packet assembly, and T038 pre-run verification.

## T036D Price-To-Beat Feed ID Fail-Closed Validation

- Sidecar review finding: the initial placeholder repair only rejected repeated-character feed ids and replaced the shipped strategy example with a better-looking synthetic `0x0123456789abcdef...` feed id. That would let local artifact generation look production-valid without a real operator-approved Chainlink feed id.
- RED: `cargo test --test config_parsing binary_oracle_strategy_rejects_placeholder_price_to_beat_feed_id -- --nocapture` failed before validation because the repeated-character placeholder produced no validation error.
- GREEN: validation now rejects malformed, repeated-character, and repeated-segment Chainlink feed ids in `parameters.runtime.price_to_beat_feed_id`, while keeping schema version and decimal scale positive.
- The shipped operator/example strategy keeps an explicit placeholder feed id and has a regression test proving it remains fail-closed until the operator supplies a real feed id. Test-only fixtures use a separate non-pattern feed id and must not be treated as production evidence.
- Focused validation: `cargo test --test config_parsing price_to_beat -- --nocapture`: 3 passed, 0 failed.
- Broader config validation: `cargo test --test config_parsing -- --nocapture`: 112 passed, 0 failed.
- Affected artifact/CLI regressions: `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 4 passed, 0 failed; `cargo test --test bolt_v3_operator_artifacts base_static_operator_artifacts -- --nocapture`: 1 passed, 0 failed; `cargo test --test bolt_v3_cli entry_decision_proof_sources -- --nocapture`: 2 passed, 0 failed; `cargo test --test bolt_v3_strategy_registration binary_oracle -- --nocapture`: 17 passed, 0 failed.
- Hygiene: `cargo fmt --check`: passed; `git diff --check`: passed; `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- Current-root fail-closed proof after the repair: `cargo run --bin bolt-v2 -- operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-trade-readiness-f5ce72bf/base-static-after-feed-validation --strategy-instance-id bitcoin_updown_main` failed with `parameters.runtime.price_to_beat_feed_id must not be a placeholder feed id`, proving the ignored operational root no longer produces base-static artifacts while it still resolves the shipped placeholder strategy.
- Current-head rerun after Kimi-waiver checklist update: head `087c9a74b6ae4b769c2e3b6ba4ce6ce6c1d725d2`; `config/live.local.toml` exists and is git-ignored; `cargo run --bin bolt-v2 -- operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-t036-audit-087c9a74 --strategy-instance-id bitcoin_updown_main` failed with `parameters.runtime.price_to_beat_feed_id must not be a placeholder feed id`. This confirms T036 is currently blocked by missing operator-approved runtime feed configuration, not by missing base-static code.

This is non-live local validation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, root TOML mutation, or live trading side effect was run. The earlier base-static artifacts remain command-path evidence only and cannot close T036 production readiness until a real operator-approved feed id and the missing source/pre-run inputs are supplied.

## T036E Current T036 Rerun And Egress Sequencing Repair

- Kimi final-review slot is waived by operator instruction; remaining final exact-head review slots are Claude, Gemini, DeepSeek, GLM, and Grok.
- Official feed-id source check used Chainlink's Crypto Streams docs at `https://docs.chain.link/data-streams/crypto-streams`; the BTC/USD Data Streams feed id copied into the ignored local strategy config is `0x00039d9e45394f473ab1f050a1b963e6b05351e52d71e507509ada0c95ed75b8`.
- Local ignored config hygiene: `.gitignore` now ignores `config/strategies/*.local.toml`; `config/live.local.toml` points at an ignored `config/strategies/binary_oracle.local.toml` for this worktree rerun. The tracked example strategy remains fail-closed with the placeholder feed id.
- Base-static rerun from current PR worktree head `1ed268f603c1bf73683c1c89d60aa80bdc68d821` plus ignored local strategy config: `cargo run --bin bolt-v2 -- operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-t036-audit-feed-1ed268f6 --strategy-instance-id bitcoin_updown_main` passed and wrote `ssm-manifest.json` (`f6d757a72d876a2a32e5fd047c5903e26d61a7f4abcf834c4b5b137e284a66bc`), `financial-envelope.json` (`0fe8aef150af7156ece1db2c2b8b0c738a51352dd4837ba4eb7d13e0469cd253`), and `approval-nonce.json` (`28e8fdd0c7e440ba45a89042755bc77357c0611b58a0f51883ee661298f4afcf`).
- Non-private source materializer reruns passed: `collect-pre-run-host-clock-source` wrote `host-clock-source.json` (`b16142939caacea8de24c72b63834414118fc73bbdf0ea538906d62f2fe7a0ee`); `collect-pre-run-clob-v2-adapter-signing-source` wrote `clob-v2-adapter-signing-source.json` (`7c7f3fa955ced05a0289ec6ffc6280e67cea3f4b5c607a9ddc0980ee33c9bef1`); `collect-pre-run-clob-v2-fee-behavior-source` wrote `clob-v2-fee-behavior-source.json` (`d290cb10b60a13b9dcead481689658c78fd2996f13c166498d23678c00fc1da5`).
- Real sequencing bug found: `operator-artifacts collect-pre-run-egress-identity-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-audit-feed-1ed268f6/egress-identity-source.json` failed with `refusing to assemble operator packet because [live_canary.operator_evidence] is missing`, proving the egress source collector incorrectly depended on the final T037 operator-evidence patch.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_egress_identity_source_before_operator_evidence_patch -- --nocapture` failed before the repair because `[live_canary]` rejected `approved_egress_identity_sha256` as an unknown field.
- GREEN: `cargo test --test bolt_v3_cli egress_identity_source -- --nocapture`: 2 passed, 0 failed.
- Focused egress proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_egress_identity_source -- --nocapture`: 4 passed, 0 failed.
- RED/GREEN dual-source cleanup: `cargo test --test config_parsing bolt_v3_operator_evidence_rejects_pre_run_egress_probe_inputs -- --nocapture` first failed because `[live_canary.operator_evidence]` still accepted `egress_identity_observed_path` and `approved_egress_identity_sha256`; after the schema cleanup it passed, proving pre-run egress probe inputs have only one config owner.
- Operator-evidence config regression: `cargo test --test config_parsing bolt_v3_operator_evidence -- --nocapture`: 2 passed, 0 failed.
- Fixture compile checks: `cargo test --test bolt_v3_live_canary_gate --no-run`; `cargo test --test bolt_v3_no_submit_readiness --test bolt_v3_tiny_canary_preconditions --test bolt_v3_tiny_canary_operator --no-run`.
- Runtime/default/source fence: `cargo fmt --check`, `git diff --check`, `python3 scripts/test_verify_bolt_v3_runtime_literals.py`, `python3 scripts/verify_bolt_v3_runtime_literals.py`, and `just source-fence` passed after removing the rejected production `serde(default)` fallback from the new egress fields.
- Current materializer rerun after repair and default-fence cleanup: `cargo run --bin bolt-v2 -- operator-artifacts collect-pre-run-egress-identity-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-audit-feed-1ed268f6/egress-identity-source.json` now fails with `egress identity source field \`egress_identity_observed_path\` is invalid or unproven`, confirming the cycle is removed and T036F now needs real operator-owned egress evidence instead of final operator-evidence preexistence.
- Read-only sidecar `019e5e40-c0fe-7ed0-9137-790267bbb26d` confirmed there is no repo-supported public-IP/EC2 metadata probe command in this path: the canonical path is an operator-provided observed identity file plus approved sha256 in ignored `config/live.local.toml`. It also flagged that substituting a local laptop public IP would bind the packet to the wrong runner identity and would not honestly unblock EC2/EIP readiness.

This was non-live local artifact/source verification except for public Chainlink documentation fetch and host-clock public HTTP time collection. It did not read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, patch `[live_canary.operator_evidence]`, or execute a trade. T036 remains open until the real EC2/EIP observed egress identity file/hash, private source-owned account/funding/collateral materializers, final pre-run-state/abort/static artifacts, T037 root TOML patch, and T038 pre-run verification all exist from the approved operational root.

## T036F EC2/EIP Egress Identity Capture

- AWS current-runner check: `aws ec2 describe-instances --region eu-west-1 --instance-ids i-0b68843392a62e359` reported instance `i-0b68843392a62e359` running with public IP `34.248.143.2`; `aws ssm describe-instance-information` reported SSM `Online`; `aws ec2 describe-addresses --public-ips 34.248.143.2` reported the EIP associated to the same instance.
- SSM egress capture command `105fe4c4-df89-4633-8c7b-500abcd82ec9` ran only shell, `curl https://checkip.amazonaws.com`, local file write, `sha256sum`, and `stat` on the approved EC2 runner. It wrote `/tmp/bolt-v2-t036-audit-feed-b15b6152/egress-identity-observed.txt`, observed identity `34.248.143.2`, trimmed-content sha256 `a64009eb4c2a651f6880e552dc0197adc00584bd92cd1022761224bce6da5751`, and mode/size `600 13`.
- Local operator-source mirror: `/private/tmp/bolt-v2-t036-audit-feed-b15b6152/egress-identity-observed.txt` contains the EC2-observed identity; `perl -0pe 's/[\r\n ]//g' ... | shasum -a 256` produced the same trimmed hash `a64009eb4c2a651f6880e552dc0197adc00584bd92cd1022761224bce6da5751`.
- Ignored config hygiene: `git check-ignore -v config/live.local.toml config/strategies/binary_oracle.local.toml` confirmed both files are ignored. Targeted `rg` confirmed ignored `config/live.local.toml` points at the local EC2-observed identity source path, byte cap `1024`, approved hash `a64009eb4c2a651f6880e552dc0197adc00584bd92cd1022761224bce6da5751`, and ignored strategy file `strategies/binary_oracle.local.toml`.
- GREEN materializer rerun: `cargo run --bin bolt-v2 -- operator-artifacts collect-pre-run-egress-identity-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-audit-feed-b15b6152/egress-identity-source.json` passed and wrote `egress-identity-source.json` with sha256 `5c31068483ec5cfd0cd39c451001c8a7f16ce52a4669905823f39e3b118df572`.

This was an approved AWS/SSM operational side effect limited to public egress identity capture and a temp file write on the approved EC2/EIP runner. It did not read real AWS SSM secret values, connect to a private venue account, run no-submit, submit/cancel orders, patch `[live_canary.operator_evidence]`, or execute a trade. T036F is closed; T036 remains open until the remaining real source-owned inputs, final pre-run-state/abort/static artifacts, T037 root TOML patch, and T038 pre-run verification exist.

## T036G Venue-Account Flatness Blocker

- Current-head rerun after T036F at head `5113ef74e52dca0f2297a73cc0b5c9fbf71cd072`: `cargo run --bin bolt-v2 -- operator-artifacts collect-pre-run-venue-account-state-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-audit-feed-b15b6152/venue-account-state-source.json`.
- Sandbox rerun first failed only on the known Rust verification cache lock at `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`; the escalated rerun reached the production materializer and failed closed with `venue account state source field preexisting_position_absent is invalid or unproven`.
- Code evidence: `src/bolt_v3_operator_artifacts.rs:3978` rejects nonzero materialized `open_position_count` as `preexisting_position_absent`, and `src/bolt_v3_operator_artifacts.rs:4569` rejects any source file whose `open_position_count` is nonzero.
- Provider-source evidence: `src/bolt_v3_providers/polymarket/venue_account_state_source.rs:109` derives the count from the configured account/funder Data API positions response, then hashes account/request details and returns only counts plus snapshot hash material; the command output did not print positions, credentials, keys, or SSM values.
- T036G1 evidence: pinned NT `PolymarketDataApiHttpClient::get_positions` fetches all rows with `sizeThreshold=0`, while NT reconciliation emits position reports only for `p.size >= nautilus_polymarket::common::consts::DUST_POSITION_THRESHOLD`. The Bolt materializer now uses that exported NT threshold for `open_position_count` instead of raw Data API row count, so zero/dust rows no longer create a false `preexisting_position_absent` blocker.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_venue_account_state_source_ignores_zero_and_dust_positions -- --nocapture` failed with `venue account state source field preexisting_position_absent is invalid or unproven` before the threshold fix.
- GREEN focused CLI: `cargo test --test bolt_v3_cli venue_account_state_source -- --nocapture`: 4 passed, 0 failed. This covers configured-account collection, help exposure, zero/dust acceptance, and threshold-sized active-position rejection.
- GREEN proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source -- --nocapture`: 5 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.
- Live no-submit rerun after T036G1 with the rebuilt managed debug binary: `/Users/spson/.cache/rust-verification/bolt-v2/target/debug/bolt-v2 operator-artifacts collect-pre-run-venue-account-state-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036g-after-position-threshold/venue-account-state-source.json` still failed closed with `venue account state source field preexisting_position_absent is invalid or unproven`.
- The post-fix output directory remained empty, so no partial source artifact was written. This was a fail-closed materializer result only; the later current-head rerun below supersedes it for current account flatness. The failed command did not print positions, credentials, keys, account addresses, or SSM values.
- Current-head rerun at `bce49f32089b5ae8d7ed1f119ff97734656554d1`: `/Users/spson/.cache/rust-verification/bolt-v2/target/debug/bolt-v2 operator-artifacts collect-pre-run-venue-account-state-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036g-current-bce49f32/venue-account-state-source-rerun.json` passed and wrote `venue-account-state-source-rerun.json` with sha256 `d99d615d7f33b31a3142b1798a90318c1321330aafe21b618c5b70fcc8bf4bf5`; the sanitized artifact reports `open_order_count=0`, `open_position_count=0`, `execution_client_id=polymarket_main`, and `configured_target_id=btc_updown_5m`.

T036G is closed for the current configured account at `bce49f32089b5ae8d7ed1f119ff97734656554d1`. No submit/cancel/transfer/no-submit action was run by this check.

## Current-Head Non-Blocked Artifact Refresh And Early T039 Sweep

At head `2a89f1e7c3a2ca0b6e52b39b1a70a03dcf3b6080`, before any final-packet assembly, the non-blocked T036 source/base artifacts were refreshed into `/private/tmp/bolt-v2-t036-current-ZgN0lc`:

- `operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-t036-current-ZgN0lc/base-static --strategy-instance-id bitcoin_updown_main` passed and wrote:
  - `base-static/ssm-manifest.json`: `cd4aaf74afab10e168898ecb7718b638a11296906f89825b1b4f35df44861f1c`
  - `base-static/financial-envelope.json`: `0fe8aef150af7156ece1db2c2b8b0c738a51352dd4837ba4eb7d13e0469cd253`
  - `base-static/approval-nonce.json`: `29dfff73adbdf3d3f9a5447d4667d49e4d660346407c4dea63b3fbcbec4b415d`
- `operator-artifacts collect-pre-run-host-clock-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-current-ZgN0lc/host-clock-source.json` passed and wrote `host-clock-source.json`: `cb72b2a2874f284fa044ab4008c89ebc61cb66ae29178467ff41e57ca6fdceb6`.
- `operator-artifacts collect-pre-run-egress-identity-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-current-ZgN0lc/egress-identity-source.json` passed and wrote `egress-identity-source.json`: `5c31068483ec5cfd0cd39c451001c8a7f16ce52a4669905823f39e3b118df572`.
- `operator-artifacts collect-pre-run-clob-v2-adapter-signing-source --cargo-toml Cargo.toml --cargo-lock Cargo.lock --clob-signing-source /Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb/crates/adapters/polymarket/src/signing/eip712.rs --max-source-bytes 200000 --output /private/tmp/bolt-v2-t036-current-ZgN0lc/clob-v2-adapter-signing-source.json` passed and wrote `clob-v2-adapter-signing-source.json`: `fe4feef3cd747d7c186ae6986a12cc9764cbf15bd0924496aac64e6b6458e548`.
- `operator-artifacts collect-pre-run-clob-v2-fee-behavior-source --nt-execution-parse-source /Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb/crates/adapters/polymarket/src/execution/parse.rs --nt-http-parse-source /Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb/crates/adapters/polymarket/src/http/parse.rs --max-source-bytes 200000 --output /private/tmp/bolt-v2-t036-current-ZgN0lc/clob-v2-fee-behavior-source.json` passed and wrote `clob-v2-fee-behavior-source.json`: `d290cb10b60a13b9dcead481689658c78fd2996f13c166498d23678c00fc1da5`.

These artifacts are current-head prerequisite evidence only. They do not close T036 because T036G venue-account flatness remains blocked, the operator-approved entry-decision proof-source inputs needed by `collect-entry-decision-proof-sources` have not yet been supplied, funding/collateral source materializers still require the real fee-rate source artifact, and T037/T038 are not complete.

Early T039 focused readiness sweep at the same head:

`cargo test --test bolt_v3_operator_artifacts --test bolt_v3_tiny_canary_preconditions --test bolt_v3_tiny_canary_operator --test bolt_v3_live_canary_gate --test bolt_v3_cli -- --nocapture`

- `tests/bolt_v3_cli.rs`: 37 passed, 0 failed.
- `tests/bolt_v3_live_canary_gate.rs`: 68 passed, 0 failed.
- `tests/bolt_v3_operator_artifacts.rs`: 174 passed, 0 failed.
- `tests/bolt_v3_tiny_canary_operator.rs`: 31 passed, 0 failed, 1 ignored.
- `tests/bolt_v3_tiny_canary_preconditions.rs`: 63 passed, 0 failed.

This sweep is current-head regression evidence only. Formal T039 remains open until T038 final-packet verification exists, per the task dependency graph in `tasks.md`.

## T041 Exact-Head CI Compile Repair

- PR #480 CI on head `41cf8be09b01afa56887df072f49d7af95f9a2ef` failed in `nextest archive` while running `cargo test --no-run --message-format json-render-diagnostics --locked`.
- Failed job log root cause: `src/bolt_v3_live_canary_gate.rs:2004`, `src/bolt_v3_live_canary_gate.rs:2005`, `src/bolt_v3_live_canary_gate.rs:2211`, and `src/bolt_v3_live_canary_gate.rs:2212` still set removed `LiveCanaryOperatorEvidenceBlock` fields `egress_identity_observed_path` and `approved_egress_identity_sha256` in in-crate test helpers.
- Repair: removed those stale test-helper field initializers only. Runtime ownership remains unchanged: pre-run egress probe inputs belong to top-level `[live_canary]`, not `[live_canary.operator_evidence]`.
- Local CI-equivalent reproduction after repair: `cargo test --no-run --locked` passed and built all test binaries.
- `cargo fmt --check` passed after the repair; `git diff --check` passed after the code and evidence update.

T041 is still open until the repaired head is pushed and GitHub CI is green on the exact pushed head.

## T036 Abort-Plan Artifact Materialization

- Command: `/Users/spson/.cache/rust-verification/bolt-v2/target/debug/bolt-v2 operator-artifacts generate-abort-plan-from-source-collectors --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --strategy-source src/strategies/binary_oracle_edge_taker.rs --submit-admission-source src/bolt_v3_submit_admission.rs --max-source-bytes 599315 --output /private/tmp/bolt-v2-trade-readiness-4e583417/final-artifacts/abort-plan.json`
- Bound source-size evidence: `wc -c src/strategies/binary_oracle_edge_taker.rs src/bolt_v3_submit_admission.rs` reported `599315` bytes for the strategy source and `9613` bytes for submit admission, so `--max-source-bytes 599315` bounded both required inputs.
- Result: passed and wrote `/private/tmp/bolt-v2-trade-readiness-4e583417/final-artifacts/abort-plan.json` with sha256 `eabbde2c170fa42ae91598c6744161ecf5f43e4248475e6381e53eccf6571c84`.
- Base static artifacts were copied into the same final-artifacts directory without changing hashes: `ssm-manifest.json` (`8b12f2a636961b3bb35dce203c66b26f482537e102a6bbf96aae592ddcb4da4a`), `financial-envelope.json` (`0fe8aef150af7156ece1db2c2b8b0c738a51352dd4837ba4eb7d13e0469cd253`), and `approval-nonce.json` (`2fa765155835265251783b7aeb73b7b2342d1a624958c73d5f717fef5c274ec5`).

This is non-live local source-code artifact generation only. No AWS, SSM, no-submit, venue connection, order submit/cancel, root TOML mutation, or live trading side effect was run. T036 still requires real entry-decision source artifacts, `strategy-input.json`, `pre-run-state.json`, the T037 root TOML patch, final manifest/packet assembly, and T038 pre-run verification.

## T024C/T024D Host-Clock Source Materializer

- Read-only sidecar `019e5d7e-f010-7172-b9c4-3eac518793c6` confirmed the current pre-run host-clock path validated a caller-supplied `host-clock-source.json`: `collect_pre_run_host_clock_source_proof` reads bounded bytes and validates schema/record/skew, while `generate-pre-run-state-from-source-collectors` consumes `--host-clock-source`. It also confirmed the Chainlink `CHAINLINK_REPORT_*` values are audited as `chainlink_report_protocol_decoder` parser invariants, not TOML runtime/operator values.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_host_clock_source_from_configured_provider_time -- --nocapture` first hit the known sandbox cache-lock permission issue at `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`; rerun outside the sandbox failed as expected because `operator-artifacts collect-pre-run-host-clock-source` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_host_clock_source_from_configured_provider_time -- --nocapture`: 1 passed, 0 failed.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_host_clock_source_collector_does_not_accept_caller_timestamps -- --nocapture`: 1 passed, 0 failed.
- Regression: `cargo test --test bolt_v3_cli host_clock -- --nocapture`: 2 passed, 0 failed.
- Regression: `cargo test --test bolt_v3_operator_artifacts host_clock -- --nocapture`: 6 passed, 0 failed.
- Broader CLI regression: `cargo test --test bolt_v3_cli -- --nocapture`: 25 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`

`operator-artifacts collect-pre-run-host-clock-source --config <root.toml> --strategy-instance-id <id> --output <host-clock-source.json>` now derives the execution client from the selected strategy, reads `base_url_http` and `http_timeout_secs` from the TOML execution block, fetches the configured provider HTTP `Date` header, records host runtime milliseconds, and writes the existing `bolt_v3.pre_run_host_clock_source.v1` artifact with create-new semantics. The CLI does not accept `--host-unix-millis` or `--reference-unix-millis`, and stdout uses the existing redacted artifact summary instead of raw timestamps.

This is non-live public HTTP provider-time collection only. No AWS, SSM, no-submit, private venue account access, order submit/cancel, root TOML mutation, or live trading side effect was run. At T024D completion time, T036 still required real source-owned materializers for venue account/open orders/positions, funding/margin, egress identity, and CLOB V2 signing/collateral/fee behavior before a blocker-free `pre-run-state.json`, T037 root TOML patch, final packet, and T038 verification could be honestly produced. T024F below narrowed that list by closing the CLOB V2 adapter-signing source materializer only.

## T024F CLOB V2 Adapter-Signing Source Materializer

- Read-only sidecar `019e5d8f-9c30-77d1-93f2-6fb4013fe92c` recommended CLOB V2 adapter signing as the smallest honest T024E slice: venue/funding/collateral require private account or chain state, egress lacks an approved identity config field, and fee behavior is larger public CLOB work.
- Read-only sidecar `019e5d92-05c0-7d63-8006-7fc8201b36f2` identified the pinned NT source path and reusable signing functions: release-manifest proof already reads `--clob-signing-source`, NT exposes `OrderSigner::new`, `OrderSigner::sign_order`, `order_hash`, and existing NT tests use `recover_address_from_prehash`.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_clob_v2_adapter_signing_source_from_nt_signing_source -- --nocapture` failed because `operator-artifacts collect-pre-run-clob-v2-adapter-signing-source` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_clob_v2_adapter_signing_source_from_nt_signing_source -- --nocapture`: 1 passed, 0 failed.
- Focused CLOB proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_clob_v2 -- --nocapture`: 7 passed, 0 failed.
- Broader CLI regression: `cargo test --test bolt_v3_cli -- --nocapture`: 26 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- Exact-head CI repair after PR #480 head `1708d3c4d7514837a82e60e743b710bc886cb721` failed `source-fence`: provider-specific NT CLOB V2 order/signature probe code moved from `src/bolt_v3_operator_artifacts.rs` into `src/bolt_v3_providers/polymarket/adapter_signing_source.rs` behind the provider-root materialization interface.
- Boundary/fence rerun after repair:
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including `tests/bolt_v3_controlled_connect.rs` 11 passed and `tests/bolt_v3_production_entrypoint.rs` 5 passed.

`operator-artifacts collect-pre-run-clob-v2-adapter-signing-source --cargo-toml <Cargo.toml> --cargo-lock <Cargo.lock> --clob-signing-source <eip712.rs> --max-source-bytes <n> --output <clob-v2-adapter-signing-source.json>` now derives the CLOB signing version and source hash through the existing release-manifest proof, checks the pinned NT signing source contains the expected domain/order-signing markers, signs a local deterministic probe order with an ephemeral key through NT's CLOB V2 `OrderSigner`, recovers the signer from the EIP-712 order hash, and writes only the existing bounded source-proof JSON fields.

This is non-live local source/signature verification only. It does not read AWS/SSM, use configured private keys, connect to a venue, submit/cancel orders, mutate root TOML, or print signatures/private key material. At T024F completion time, T024E still needed real source-owned materializers for venue account/open orders/positions, funding/margin, egress identity, and CLOB V2 collateral/fee behavior before a blocker-free `pre-run-state.json`, T037 root TOML patch, final packet, and T038 verification could be honestly produced.

## T024G CLOB V2 Fee-Behavior Source Materializer

- Read-only sidecar `019e5db2-5424-7aa1-aa26-9f8469c5bfe0` recommended CLOB V2 fee behavior as the next smallest honest T024E slice after adapter signing: it is public/source-owned, can be bounded by pinned NT fee parser source files, and does not require private venue/account state.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_clob_v2_fee_behavior_source_from_nt_fee_sources -- --nocapture` failed because `operator-artifacts collect-pre-run-clob-v2-fee-behavior-source` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_clob_v2_fee_behavior_source_from_nt_fee_sources -- --nocapture`: 1 passed, 0 failed.
- Focused CLOB proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_clob_v2 -- --nocapture`: 7 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
- `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-pre-run-clob-v2-fee-behavior-source --nt-execution-parse-source <parse.rs> --nt-http-parse-source <http/parse.rs> --max-source-bytes <n> --output <clob-v2-fee-behavior-source.json>` now reads bounded pinned NT fee parser source files, verifies required maker-fee, taker-fee, fee-schedule, commission, taker-side, binary fee-curve, market-buy adjustment, and price-bound source markers, runs a local deterministic NT `compute_commission`/`adjust_market_buy_amount` self-test, and writes the existing bounded CLOB V2 fee-behavior source artifact fields with source and assumptions hashes. The non-runtime literals introduced by the self-test and source markers are explicitly audited as record-kind, validation-field, source-marker, or deterministic self-test fixtures; production fee rates, prices, quantities, balances, and venue state remain source/config owned.

This is non-live local source/fee-behavior verification only. It does not read AWS/SSM, use configured private keys, connect to a venue, submit/cancel orders, mutate root TOML, or print raw NT source. At T024G completion time, T024E still needed real source-owned materializers for venue account/open orders/positions, funding/margin, egress identity, and CLOB V2 collateral accounting before a blocker-free `pre-run-state.json`, T037 root TOML patch, final packet, and T038 verification could be honestly produced.

## T024H Egress-Identity Source Materializer

- Read-only sidecar `019e5dc8-7319-7a41-92e4-6315b9bf6c3c` identified the missing egress materializer gap: existing code only validated a caller-supplied egress source artifact, while production readiness needs TOML-owned approved identity hash plus a TOML-owned observed probe source.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_egress_identity_source_from_configured_probe -- --nocapture` failed because `operator-artifacts collect-pre-run-egress-identity-source` was not a recognized subcommand.
- GREEN: `cargo test --test bolt_v3_cli bolt_v3_cli_collects_egress_identity_source_from_configured_probe -- --nocapture`: 1 passed, 0 failed.
- Focused egress proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_egress_identity_source -- --nocapture`: 4 passed, 0 failed.
- Live-canary operator-evidence regression: `cargo test --test bolt_v3_live_canary_gate operator_evidence -- --nocapture`: 14 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
- `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
- `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
- `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-pre-run-egress-identity-source --config <root.toml> --strategy-instance-id <id> --output <egress-identity-source.json>` now loads the selected root TOML, proves the strategy exists in that config, reads `[live_canary].egress_identity_observed_path` with `[live_canary].egress_identity_observed_max_bytes`, hashes the trimmed observed identity, compares it to `[live_canary].approved_egress_identity_sha256`, and writes the existing bounded egress identity source artifact with only hashes and record metadata. The CLI does not accept caller-supplied observed identity or approved hash arguments, and stdout uses the existing redacted artifact summary instead of raw egress identity.

This is non-live local probe-file materialization only. It does not read AWS/SSM, use configured private keys, connect to a venue, submit/cancel orders, mutate root TOML, or run live trading side effects. T036 remains open: T024E still needs real source-owned materializers for venue account/open orders/positions, funding/margin, and CLOB V2 collateral accounting before a blocker-free `pre-run-state.json`, T037 root TOML patch, final packet, and T038 verification can be honestly produced.

## T024I CLOB V2 Collateral-Accounting Source Materializer

- Read-only sidecar `019e5de4-116b-76c0-8bbd-c5b61478dc4f` found no existing source-owned derivation for `required_max_notional_plus_fees`; the old proof only validated a supplied value. It recommended deriving from TOML-owned `max_notional_per_order` plus an approved fee source instead of treating the deterministic fee-behavior self-test as production fee policy.
- Read-only sidecar `019e5de4-2e84-7123-af27-d4124a4cd242` recommended a CLI integration test using fake SSM plus fake CLOB `/balance-allowance`, proving the production command path resolves configured SSM secrets and calls the authenticated NT balance-allowance endpoint without exposing secrets.
- RED: `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_clob_v2_collateral_accounting_source_from_configured_balance_allowance -- --nocapture` failed because `operator-artifacts collect-pre-run-clob-v2-collateral-accounting-source` was not a recognized subcommand.
- GREEN focused CLI regression: `cargo test --test bolt_v3_cli clob_v2_collateral_accounting -- --nocapture`: 2 passed, 0 failed.
- Full CLI regression: `cargo test --test bolt_v3_cli -- --nocapture`: 30 passed, 0 failed.
- Full operator-artifact regression: `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 174 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-pre-run-clob-v2-collateral-accounting-source --config <root.toml> --strategy-instance-id <id> --fee-rate-source <fee-rate-source.json> --fee-rate-source-sha256 <sha256> --max-fee-rate-source-bytes <bytes> --output <clob-v2-collateral-accounting-source.json>` now loads the selected root TOML, resolves configured Polymarket credentials through the production SSM resolver, calls NT's authenticated `GET /balance-allowance` for collateral, converts micro-pUSD balance/allowance into pUSD using the fixed Polymarket API unit scale, validates the approved fee-rate source hash, derives `required_max_notional_plus_fees = max_notional_per_order * (1 + max_fee_bps / 10000)`, and writes the existing CLOB V2 collateral accounting source artifact with only proof hashes and bounded decimal strings.

The verification used local fake SSM and fake CLOB HTTP servers only. No real AWS/SSM, private venue account access, no-submit, order submit/cancel, root TOML mutation, or live trading side effect was run. T036 remains open: T024E still needs real source-owned materializers for venue account/open orders/positions and funding/margin before a blocker-free `pre-run-state.json`, T037 root TOML patch, final packet, and T038 verification can be honestly produced.

## T024E Venue-Account And Funding/Margin Source Materializers

- Funding/margin sidecar `019e5e09-fce5-7b81-88dc-f2639495dc27` identified the remaining proof shape: current source proofs validate `venue_account_state_source` and `funding_margin_source` files, but T024E still needed production CLI materializers so those files are derived from configured account queries instead of caller-supplied counts, balances, or hashes.
- RED venue-account CLI: `cargo test --test bolt_v3_cli venue_account_state_source_from_configured_account_queries -- --nocapture` failed with unrecognized subcommand `collect-pre-run-venue-account-state-source`.
- GREEN venue-account CLI: `cargo test --test bolt_v3_cli venue_account_state_source_from_configured_account_queries -- --nocapture`: 2 passed, 0 failed.
- RED funding/margin CLI: `cargo test --test bolt_v3_cli funding_margin_source_from -- --nocapture` failed with unrecognized subcommand `collect-pre-run-funding-margin-source`.
- GREEN funding/margin CLI: `cargo test --test bolt_v3_cli funding_margin_source_from -- --nocapture`: 2 passed, 0 failed.
- Focused source-owned CLI regression: `cargo test --test bolt_v3_cli source_from -- --nocapture`: 10 passed, 0 failed.
- Existing venue-account proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_venue_account_state_source_proof -- --nocapture`: 5 passed, 0 failed.
- Existing funding/margin proof regression: `cargo test --test bolt_v3_operator_artifacts pre_run_funding_margin_source_proof -- --nocapture`: 4 passed, 0 failed.
- Full CLI regression: `cargo test --test bolt_v3_cli -- --nocapture`: 34 passed, 0 failed.
- Full operator-artifact regression: `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 174 passed, 0 failed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

`operator-artifacts collect-pre-run-venue-account-state-source --config <root.toml> --strategy-instance-id <id> --output <venue-account-state-source.json>` now loads the selected root TOML, resolves configured Polymarket credentials through the production SSM resolver, calls NT's authenticated CLOB open-order query, calls NT's Polymarket Data API positions query for the configured account/funder, requires zero open orders and zero NT-reconciled active positions, and writes the existing `bolt_v3.pre_run_venue_account_state_source.v1` artifact with configured execution-client/target identity and a source-owned snapshot hash. The CLI does not accept caller-supplied `open_order_count`, `open_position_count`, or `account_state_snapshot_sha256`.

`operator-artifacts collect-pre-run-funding-margin-source --config <root.toml> --strategy-instance-id <id> --fee-rate-source <fee-rate-source.json> --fee-rate-source-sha256 <sha256> --max-fee-rate-source-bytes <bytes> --output <funding-margin-source.json>` now loads the selected root TOML, resolves configured Polymarket credentials through the production SSM resolver, calls NT's authenticated `GET /balance-allowance`, reuses the same TOML-owned `max_notional_per_order` plus approved fee-rate source derivation as CLOB collateral accounting, sets available collateral from the spendable pUSD balance/allowance, requires coverage for `required_max_notional_plus_fees`, and writes the existing `bolt_v3.pre_run_funding_margin_source.v1` artifact.

The verification used local fake SSM, fake CLOB, and fake Data API HTTP servers only. No real AWS/SSM, private venue account access, no-submit, order submit/cancel, root TOML mutation, or live trading side effect was run. T024E is now closed for source-owned materializer commands; T036 remains open for assembling the blocker-free static artifacts and final packet from the materialized source files.

## T036G2/T036G3 Provider Snapshot Hard-Stop Confirmation

- Current Speckit helper status: `.specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks` still returns `specs/023-nt-order-intent-layer`. Per this readiness packet, PR #480 continues using explicit `specs/024-production-trade-readiness/` task/evidence files and does not change the source-fence-owned 023 pointer.
- Inventory artifact: `specs/024-production-trade-readiness/provider-snapshot-hard-stop-inventory.md` classifies the market/venue/account-agnostic external-provider-snapshot hard-stop class and separates immediate readiness fixes from T038/T043/T044 final-packet/no-submit/tiny-canary gates.
- Immediate fixed gates:
  - Venue account open orders: `conflicting_open_orders_absent`.
  - Venue account active positions: `preexisting_position_absent`.
  - CLOB collateral balance/allowance: `collateral_accounting_verified`.
  - CLOB funding margin balance/allowance: `funding_margin_covers_max_notional_plus_fees`.
- Claude adversarial review: job `2b5835b2-84a9-4171-9b3e-138ab623a762` reviewed the assessment plus current diff and returned `APPROVE` with no blocking findings. Non-blocking findings were persistent-block coverage, confirmation-fetch-failure coverage, nested retry call count, retry-delay naming, and diff-only inventory scope.
- Implementation response:
  - Shared provider helper confirms blocking external snapshots with the configured retry policy before hard-stop.
  - Confirmation stays fail-closed: persistent blocking state, parse failure, and confirmation fetch failure still block.
  - CLOB collateral/funding confirmation now uses one non-nested balance/allowance fetch per configured confirmation retry; the initial fetch still uses configured retry behavior.
- Focused fail-closed coverage: `cargo test --test bolt_v3_cli keeps_ -- --nocapture`: 4 passed, 0 failed.
- Focused transient-clear coverage: `cargo test --test bolt_v3_cli confirms_transient -- --nocapture`: 4 passed, 0 failed.
- Focused CLI regressions:
  - `cargo test --test bolt_v3_cli venue_account_state_source -- --nocapture`: 7 passed, 0 failed.
  - `cargo test --test bolt_v3_cli clob_v2_collateral_accounting -- --nocapture`: 5 passed, 0 failed.
  - `cargo test --test bolt_v3_cli funding_margin_source -- --nocapture`: 4 passed, 0 failed.
- Focused proof regressions:
  - `cargo test --test bolt_v3_operator_artifacts pre_run_clob_v2 -- --nocapture`: 7 passed, 0 failed.
  - `cargo test --test bolt_v3_operator_artifacts pre_run_funding_margin_source -- --nocapture`: 4 passed, 0 failed.
- Hygiene:
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-server verification only. It did not read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036 remains open until final packet assembly, T037 root TOML patching, and T038 verification are completed.

## T036H0E Mainline Sync And Static Reference Cleanup

- Current mainline input: `origin/main` at `53fd50d2ccd05a81e9ca65575594514315511fdc` includes PR #487 and the NT 0.58/HIP-4 bump.
- Initial replay rebase attempt was aborted after it tried to replay already-merged command-tokenization/#466 commits and conflicted in out-of-scope files. Those commits are not part of the PR #480 readiness surface.
- PR #480 was synced by final-tree merge instead: merge commit `df1d079b` merges `origin/main` into `goal/024-production-trade-readiness`.
- The only merge conflict was `Cargo.toml`; it was resolved to keep current main's `nautilus-portfolio` dev-dependency at NT rev `6e059dcbb59ac1e582132fc431a581936c216c3c`.
- The local Binance/BTCUSDT cleanup attempt was reapplied and then narrowed: the misleading replacement with `polymarket_main` plus `condition-1-UP.POLYMARKET` was removed, and shipped config/example/fixture files were left unchanged until T036H3/T036H14 can migrate them under RED/GREEN gate-schema coverage.
- The official task contract now carries the required cleanup instead: no Binance/BTCUSDT canonical reference, no Polymarket-only selected-market identity, no UP-only selected market, no closed provider-kind list, and no price-only role schema.
- Verification: `git diff --check` passed after the merge conflict resolution and before the task-packet cleanup continued.

No production code, live config, secret source, no-submit path, or trading operation was executed for this sync. T036H0F remains open for exact-delta review of the cleaned provider-agnostic contract before T036H1 RED tests begin.
