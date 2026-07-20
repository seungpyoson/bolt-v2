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
- Historical #478 status check rollup showed successful build/test/gate/check jobs, with deploy and same-sha-main-evidence skipped. #480 requires fresh exact-head CI before completion.

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

- `git ls-files config/live.local.toml config '*.toml' | sort` showed no tracked `config/live.local.toml`; the repo-owned config surface is `config/root.toml` plus test fixtures.
- `ls -la config` showed no local `config/live.local.toml` in this worktree. No live-local TOML was read, printed, or edited.
- `rg` over `src/bolt_v3_config.rs` and `config/root.toml` shows the schema-level T124/T125 binding fields are `strategy_input_evidence_path`, `strategy_input_evidence_sha256`, and `decision_evidence_path`; there is no standalone top-level market-selection operator-evidence field.
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
- The only merge conflict was `Cargo.toml`; it initially exposed two NT 0.58 dependency alignment issues:
  - `nautilus-portfolio` existed as an old normal dependency on rev `7c2aafb30fb143069c915a3f2057bb12174405f6` and as a new dev-dependency on rev `6e059dcbb59ac1e582132fc431a581936c216c3c`. Because branch source uses `nautilus_portfolio` in production strategy code, the fix is one normal dependency at rev `6e059dcbb59ac1e582132fc431a581936c216c3c`, not a split normal/dev source.
  - `alloy-primitives = "=1.5.7"` conflicted with NT 0.58's `^1.6.0` requirement from `nautilus-polymarket`. The direct dependency was updated to the lockfile-resolved `=1.6.0`.
- The local Binance/BTCUSDT cleanup attempt was reapplied and then narrowed: the misleading replacement with `polymarket_main` plus `condition-1-UP.POLYMARKET` was removed, and shipped config/example/fixture files were left unchanged until T036H3/T036H14 can migrate them under RED/GREEN gate-schema coverage.
- The official task contract now carries the required cleanup instead: no Binance/BTCUSDT canonical reference, no Polymarket-only selected-market identity, no UP-only selected market, no closed provider-kind list, and no price-only role schema.
- Verification:
  - `git diff --check`: passed after the merge conflict resolution and before the task-packet cleanup continued.
  - `cargo test --test config_parsing`: passed after dependency alignment; 117 passed, 0 failed.

No production code, live config, secret source, no-submit path, or trading operation was executed for this sync. T036H0F remains open for exact-delta review of the cleaned provider-agnostic contract before T036H1 RED tests begin.

## T036H16 Provider-Neutral Gate Evidence And Entry-Readiness Session

- Implemented provider-neutral `GateEvidence`, boxed `GateSatisfaction`, canonical `EntryReadinessGateSession`, and `build_entry_readiness_gate_session` in `src/bolt_v3_operator_artifacts.rs`.
- Added provider binding lookup in `src/bolt_v3_providers/mod.rs` and Polymarket entry-decision provenance payload support in `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs`.
- Added `SelectedMarketRequirement` deserialization support and operator-artifact tests for complete-only evidence normalization, selected-market binding, freshness/clock-skew rejection, deterministic provider preference, and explicit no-resolution satisfaction.
- Focused entry-readiness regression: `cargo test --test bolt_v3_operator_artifacts entry_readiness_gate_session -- --nocapture`: 4 passed, 0 failed.
- Focused gate-evidence regression: `cargo test --test bolt_v3_operator_artifacts gate_evidence_normalization -- --nocapture`: 1 passed, 0 failed.
- Full operator-artifact regression after final code shape: `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 179 passed, 0 failed.
- Production library lint: `cargo clippy --locked --lib -- -D warnings`: passed.
- Formatting and hygiene:
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

The join now fails closed unless normalized evidence matches the required role, value kind, selected-market key, configured provider binding, provider capability, subscription provider filters, market mapping, evidence checksums, freshness max-age, and clock-skew policy. Multiple matching evidence items require deterministic TOML `provider_preference`; no-resolution satisfaction is only accepted when archetype, target subscription, and selected-market metadata all allow it. This was local fake-fixture verification only: no live AWS/SSM, no venue connection, no no-submit run, no order submit/cancel, no root TOML mutation, and no trade side effect. T036H17 remains open for consumer-boundary rewiring to consume the readiness session.

## T036H17 Partial Decision Evidence And Entry-Replay Consumer Boundary

- Decision evidence now records and validates readiness-session identity: `gate_session_hash`, `selected_market_key`, and per-role normalized evidence identity with normalized-value and provider-provenance hashes.
- Strategy input evidence creation now requires `StrategyBuildContext` readiness evidence instead of accepting provider-specific `price_to_beat_source` equality as readiness proof.
- Entry-decision source replay now serializes and consumes readiness evidence derived from the normalized `EntryReadinessGateSession`; stale source JSON without readiness evidence fails closed.
- Operator artifact replay and direct runtime snapshot writes validate readiness gate identity before accepting a strategy-input evidence chain.
- The runtime literal audit was updated for the new non-runtime gate evidence labels and validation fields.
- Local verification:
  - `cargo test --test bolt_v3_decision_evidence -- --nocapture`: 11 passed, 0 failed.
  - `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 179 passed, 0 failed.
  - `cargo test --lib -- --nocapture`: 356 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 remains open for the remaining tiny-canary, live-canary, CLI, registration, live-node runtime, replay, and final-packet readiness-session consumer paths.

## T036H17 Partial Tiny And Live Canary Consumer Boundary

- Tiny-canary strategy-input audit now consumes normalized readiness identity (`gate_session_hash`, `selected_market_key`, and per-role `gate_evidence`) before accepting a strategy-input evidence file. Legacy `price_to_beat_source` string equality alone is no longer treated as approval proof.
- Live-canary operator evidence now requires `gate_session_path` and `expected_gate_session_sha256`, hash-verifies the bounded gate-session file, parses the `EntryReadinessGateSession`, validates the normalized selected-market evidence identity, and checks the session strategy/configured target against the loaded root TOML before accepting the operator report path.
- Operator artifact pre-run replay now rejects strategy-input artifacts with missing readiness identity. The replay boundary is the normalized gate-session identity, not caller-controlled price-source text.
- The runtime literal audit was updated for the new operator gate-session fields and configured-target validation label.
- Local verification:
  - `cargo test --test bolt_v3_tiny_canary_preconditions strategy_audit_uses_normalized_readiness_identity_not_price_source_string -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`: 63 passed, 0 failed.
  - `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`: 31 passed, 0 failed, 1 ignored.
  - `cargo test --test bolt_v3_live_canary_gate gate_session -- --nocapture`: 2 passed, 0 failed.
  - `cargo test --test bolt_v3_live_canary_gate -- --nocapture`: 70 passed, 0 failed.
  - `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`: 33 passed, 0 failed.
  - `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 179 passed, 0 failed.
  - `cargo test --test bolt_v3_decision_evidence -- --nocapture`: 11 passed, 0 failed.
  - `cargo test --lib -- --nocapture`: 356 passed, 0 failed.
  - Final combined focused rerun after the source-fence fix: `cargo test --locked --test bolt_v3_decision_evidence --test bolt_v3_tiny_canary_preconditions --test bolt_v3_tiny_canary_operator --test bolt_v3_live_canary_gate --test bolt_v3_no_submit_readiness --test bolt_v3_operator_artifacts -- --nocapture`: 387 passed, 0 failed, 1 ignored.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 remains open for the remaining CLI command contract, registration/live-node runtime path, final-packet path/hash materialization, and complete readiness-session consumer rewiring.

## T036H17 Partial Final-Packet Gate-Session Binding

- `operator-artifacts generate-operator-evidence-json` now accepts provider-neutral `--gate-session` and `--expected-gate-session-sha256`, verifies the bounded gate-session file hash before writing operator evidence JSON, and writes those fields into `[live_canary.operator_evidence]`.
- Operator-evidence TOML patch validation now requires a materialized gate-session path/hash pair and verifies the configured file hash before patching.
- `operator-evidence-packet.json` now carries `gate_session_path` and `expected_gate_session_sha256`, and final-packet verification compares those fields back to the root TOML operator-evidence block before accepting the packet.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_cli bolt_v3_cli_generates_operator_evidence_json_without_printing_values -- --nocapture` failed because `generate-operator-evidence-json` rejected `--gate-session` as an unexpected argument.
  - `cargo test --locked --test bolt_v3_operator_artifacts approval_packet_assembly_writes_non_circular_envelope_from_existing_refs -- --nocapture` failed because the assembled operator packet omitted `gate_session_path`.
- Local verification:
  - `cargo test --locked --test bolt_v3_cli --test bolt_v3_operator_artifacts -- --nocapture`: 224 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_cli --test bolt_v3_operator_artifacts --test bolt_v3_live_canary_gate --test bolt_v3_no_submit_readiness -- --nocapture`: 327 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 remains open for the remaining generic CLI command cleanup, registration/live-node runtime path, and complete readiness-session consumer rewiring.

## T036H17 Partial CLI Provider-Specific Entry Collection

- Generic `operator-artifacts collect-entry-decision-source-inputs` and `operator-artifacts collect-entry-decision-proof-sources` were removed from the public CLI surface so generic entry-decision commands no longer expose Chainlink-shaped `--price-report`, `--expected-price-report-sha256`, or `--price-to-beat-source` flags.
- The existing Chainlink Data Streams proof/source-input materialization path now lives behind provider-specific CLI commands:
  - `operator-artifacts collect-chainlink-entry-decision-proof-sources`
  - `operator-artifacts collect-chainlink-entry-decision-source-inputs`
- Existing source validation remains intact: the Chainlink proof-source materializer still binds the report to TOML-derived provider/feed/schema/decimal-scale config, bounded source files, selected decision windows, and fee proof inputs before writing local artifacts.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_cli entry_decision -- --nocapture` failed because the old generic collectors still exposed legacy provider-shaped flags and the new Chainlink-specific collector commands were unrecognized.
- Local verification:
  - `cargo test --locked --test bolt_v3_cli entry_decision -- --nocapture`: 5 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_cli -- --nocapture`: 46 passed, 0 failed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 remains open for the remaining registration/live-node runtime path and complete readiness-session consumer rewiring.

## T036H17 Partial Registration And Live-Node Runtime Readiness Binding

- Strategy registration now loads the bounded `[live_canary.operator_evidence].gate_session_path`, verifies it against `expected_gate_session_sha256`, parses the `EntryReadinessGateSession`, validates the normalized readiness identity, checks the loaded strategy/configured target binding, and passes a `BoltV3ReadinessGateEvidenceSnapshot` into matching registration contexts.
- The binary-oracle runtime registration binding now forwards that normalized readiness evidence into `StrategyBuildContext`, so runtime strategy input evidence is built from the operator-approved readiness session when the live operator evidence is present.
- The runtime literal audit was updated for the new strategy-registration schema labels.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_strategy_registration bolt_v3_registration_context_includes_operator_readiness_gate_session -- --nocapture` failed with `E0609` because `StrategyRegistrationContext` had no `readiness_evidence` field.
  - `cargo test --locked --test bolt_v3_strategy_registration binary_oracle_registration_forwards_readiness_gate_session_to_build_context -- --nocapture` failed after temporarily removing the forwarding because binary-oracle registration did not consume `context.readiness_evidence`.
- Local verification:
  - `cargo test --locked --test bolt_v3_strategy_registration bolt_v3_registration_context_includes_operator_readiness_gate_session -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_strategy_registration binary_oracle_registration_forwards_readiness_gate_session_to_build_context -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_strategy_registration -- --nocapture`: 23 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_live_canary_gate operator_evidence -- --nocapture`: 14 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 remains open for the remaining complete readiness-session consumer rewiring and any final stale provider-string replay cleanup.

## T036H17 Final Replay Readiness-Session Consumer Cleanup

- Source-owned entry-decision replay now uses `BinaryOracleEntryDecisionEvidenceSource` schema v2 with a full `readiness_session` instead of a flattened `readiness_evidence` snapshot plus top-level `price_to_beat_value`.
- Replay constructs the runtime readiness snapshot from the session and derives `market.price_to_beat` from the `resolution` gate's normalized `price_to_beat_value`, failing closed if the session has no evidence, the wrong value kind, or an unusable value.
- Source-input materialization now writes the full readiness session into the replay source JSON; generated sources no longer include a top-level replay price field.
- The runtime literal audit was updated for the v2 source schema and the normalized price field name.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_evidence_replay_derives_price_from_readiness_session -- --nocapture` failed because the current source parser rejected `readiness_session` and still expected `readiness_evidence` plus `price_to_beat_value`.
- Local verification:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_evidence_replay_derives_price_from_readiness_session -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_decision -- --nocapture`: 17 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts -- --nocapture`: 180 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks, `cargo fetch --locked`, 11 `bolt_v3_controlled_connect` tests, and 5 `bolt_v3_production_entrypoint` tests.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H17 is complete locally; T036H18 and later tasks remain open.

## T036H18 Thin Provider Readiness Source Collection

- Added `collect_entry_readiness_gate_evidence_from_source_file` under the operator-artifact surface. It reads a bounded source file, verifies its expected sha256, resolves the configured gate provider, and dispatches by configured `provider_kind`.
- The Chainlink Data Streams path consumes source-bound price/report provenance, validates the report binding against TOML, and emits normalized gate evidence with the source artifact hash plus the Chainlink report hash.
- The Hyperliquid HIP-4 and venue-native paths consume normalized metadata source files and emit normalized gate evidence without rebuilding upstream adapters.
- Collection fails closed unless the selected market, target subscription mapping, allowed provider/value kind, provider capability, and provider id all agree before evidence is normalized.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness_evidence_collection -- --nocapture` failed with unresolved imports for `EntryReadinessGateEvidenceSourceFileRequest` and `collect_entry_readiness_gate_evidence_from_source_file`.
- Local verification:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness_evidence_collection -- --nocapture`: 3 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness -- --nocapture`: 7 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts -- --nocapture`: 183 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H18 is complete locally; T036H19 and later tasks remain open.

## T036H19 Provider Rotation And No-Global-Provider Regressions

- Added rotation regressions proving readiness sessions can be satisfied without a global Chainlink provider when the selected market and target mapping require Pyth, exchange-index, Deribit/index, outcome-oracle, or test-double-backed provider evidence.
- Added a no-global-provider no-resolution regression proving explicit no-resolution readiness does not require any root `gate_providers` entry.
- Added a negative regression proving configured-but-unselected global Chainlink evidence does not satisfy a Pyth-selected market mapping.
- Extended neutral gate value-kind handling for `index` and `metadata`; binary-oracle runtime requirements still opt into only the value kinds they declare.
- RED proof before implementation:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness_gate_session_rotates -- --nocapture` first failed because `GateValueKind::Index` did not exist.
  - After adding `Index`, the same test failed for the `test_double` rotation case until the test target mapping explicitly set `allowed_provider_kinds` to the selected provider kind, proving the subscription gate mattered.
- Local verification:
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness_gate_session_rotates -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_readiness_gate_session_ -- --nocapture`: 7 passed, 0 failed.
  - `cargo test --locked --test bolt_v3_operator_artifacts -- --nocapture`: 186 passed, 0 failed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_provider_leaks.py`: `OK: Bolt-v3 provider-leak verifier passed.`
  - `python3 scripts/verify_bolt_v3_core_boundary.py`: `OK: Bolt-v3 core boundary audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

This was local fake-fixture and local source verification only. It did not use GitHub Actions, read real AWS/SSM secrets, connect to a private venue account, run no-submit, submit/cancel orders, mutate `config/live.local.toml`, transfer funds, or execute a trade. T036H19 is complete locally; T036 and later tasks remain open.

## T036I Chainlink Data Streams Report Source Materializer

- Added `operator-artifacts collect-chainlink-price-report-source` as the narrow Chainlink adapter needed by T036. The command resolves the configured `[gate_providers.<id>.chainlink_data_streams].api_key_ssm_parameter` and `.api_secret_ssm_parameter`, signs a Chainlink Data Streams REST report request, writes the bounded `feedID`/`validFromTimestamp`/`observationsTimestamp`/`fullReport` source JSON, and prints only the artifact path/hash.
- Kept the retired runtime client path retired: no `src/clients/chainlink.rs` was reintroduced. The source materializer lives under `src/bolt_v3_operator_artifacts.rs` and uses TOML-owned `rest_base_url`, `report_endpoint_path`, `http_timeout_secs`, feed id, schema version, decimal scale, and SSM credential parameter fields.
- Updated tracked config/fixtures with the TOML-owned Chainlink REST fields and updated ignored `config/live.local.toml` with the same non-secret fields.
- Added market-resolution `feed_bindings` under `[gate_providers.<id>.chainlink_data_streams]` so the materializer selects the feed by configured `resolution_identity` and `value_kind` instead of using one provider-wide feed. Runtime code still receives these values only from TOML.
- Rechecked the old Chainlink code before finalizing token mappings. Commit `5af253d9` names `0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439` as `BTC_TESTNET_FEED_ID` in `tests/config_schema.rs` and keeps it in the commented BTC live example; the same fix commit names `0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782` as `CORRECT_ETH_TESTNET_FEED_ID` and asserts the ETH operator snapshot must not use the BTC feed. The older `0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472` value remains treated as an old generic fixture value, not a shipped token mapping.
- Added regression coverage that shipped root examples contain the historical BTC/USD and ETH/USD Chainlink testnet feed ids and do not ship the old generic fixture feed as a token mapping.
- Root-cause check for the live 401: the previous implementation used two SSM parameters, `/bolt/testnet/chainlink/api-key` and `/bolt/testnet/chainlink/api-secret`. Secret-safe live probes showed those old SSM parameters still authenticated against Chainlink testnet for both BTC/USD and ETH/USD on REST and WebSocket, while the newer `/bolt/gate-providers/chainlink/testnet` JSON credential document and latest 1Password item returned 401. The fix restores the previous working two-SSM-parameter shape in the current provider TOML instead of overwriting shared SSM secrets.
- RED proof before implementation:
  - `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_chainlink_price_report_source -- --nocapture` failed with `unrecognized subcommand 'collect-chainlink-price-report-source'`.
- RED/GREEN proof for market-agnostic feed selection:
  - `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_selects_feed_by_resolution_identity -- --nocapture` first failed because the materializer used the provider-level/default feed instead of the selected market's `resolution_identity`.
  - The same command passed after feed binding selection moved to `(resolution_identity, value_kind)`.
- Local verification:
  - `cargo test --test config_parsing -- --nocapture`: 137 passed, 0 failed.
  - `cargo test --test bolt_v3_cli chainlink -- --nocapture`: 4 passed, 0 failed.
  - `cargo test --test bolt_v3_cli entry_decision_proof_sources -- --nocapture`: 2 passed, 0 failed.
  - `cargo test --test bolt_v3_cli bolt_v3_cli_collects_entry_decision_proof_sources_without_printing_inputs -- --nocapture`: 1 passed, 0 failed.
  - `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 5 passed, 0 failed.
  - `cargo fmt --check`: passed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal verifier self-tests passed.`
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: `OK: Bolt-v3 runtime literal audit passed.`
  - `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.
  - `cargo test --test bolt_v3_cli -- --nocapture`: 48 passed, 0 failed.
  - `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 187 passed, 0 failed.
  - `git diff --check`: passed.
- Current operational attempt after restoring the previous working SSM credential shape: `cargo run --locked --bin bolt-v2 -- operator-artifacts collect-chainlink-price-report-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --report-timestamp-unix-seconds 1779814423 --max-report-response-bytes 1000000 --output /private/tmp/bolt-v2-chainlink-recheck-btc.json` passed and wrote `/private/tmp/bolt-v2-chainlink-recheck-btc.json` with sha256 `64de7ece8c51736b3620452a132dd3c0837b264f12a54428aed5c0865e23c496`. The artifact has `feedID=0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439`, `validFromTimestamp=1779814423`, `observationsTimestamp=1779814423`, and a non-empty `fullReport`. No credentials or SSM secret values were printed.
- The older local probe `/private/tmp/bolt-v2-t036-chainlink-probe.json` is not usable as an approved report source: its JSON keys are only `["error"]`, its sha256 is `930617e7f3a506d4adbe4d8f0984200de140d533a31c083200e2f3578fcd7656`, and it contains no `feedID`/`fullReport` report source.

This was TDD-backed local fake-server verification plus a real AWS SSM-backed Chainlink REST collection that succeeded for the configured BTC/USD testnet feed. It did not print real SSM secret values, connect to a private venue account, run no-submit, submit/cancel orders, transfer funds, or execute a trade. T036 remains open until the source-bound decision inputs can be materialized from the real report plus real reference quote, realized-volatility, fee, market-book, pre-run, and static artifact inputs.

## T036 Current-Head Artifact Attempt And Remaining Blocker

At head `6de829eb7d63c2c46fa77f2f3cf87c666708c367`, local artifact generation was retried without using GitHub Actions or CI.

- Ignored local TOML migration needed before artifact generation: `config/live.local.toml` was updated locally with `[gate_providers.resolution_oracle_primary]`, Chainlink Data Streams feed/schema/scale provider binding, and Polymarket data auto-load retry fields; `config/strategies/binary_oracle.local.toml` was updated locally to use `[target.gate_subscriptions.resolution]` and to remove the legacy `parameters.runtime.price_to_beat_*` and `forced_flat_stale_chainlink_ms` fields. These files remain ignored and were not staged.
- `operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-024-final-6de829eb/final-artifacts --strategy-instance-id bitcoin_updown_main` passed and wrote:
  - `/private/tmp/bolt-v2-024-final-6de829eb/final-artifacts/ssm-manifest.json`: `ba25a39d7172da6d07fb3fb2e2c24e48ef69ad6911e31e42b43099456991eadd`
  - `/private/tmp/bolt-v2-024-final-6de829eb/final-artifacts/financial-envelope.json`: `008eb6afbe98ddb08c929e4103deebd3ae96f879e33ff6705d6d1ba51b6f3d56`
  - `/private/tmp/bolt-v2-024-final-6de829eb/final-artifacts/approval-nonce.json`: `7803a086617bb3332dfc66aa60335173a1a55886af9a18652f3f6b69f637daaa`
- Source collectors that do not require the missing entry-decision fee source passed:
  - `host-clock-source.json`: `ec9a10b39a23603cd9865a37dc178619d6aaa5b0f555e9c1d8fe1a825cd675db`
  - `egress-identity-source.json`: `5c31068483ec5cfd0cd39c451001c8a7f16ce52a4669905823f39e3b118df572`
  - `clob-v2-adapter-signing-source.json`: `de6a3e3c64a62634137ff92b5e5a4ed9ab8ada6dd79daa89c89526cc426489f6`
  - `clob-v2-fee-behavior-source.json`: `daa5778d0bc155ece1640a8e89938793aef240c1773744ea8dbf61d45925e2c2`
  - `venue-account-state-source.json`: `d99d615d7f33b31a3142b1798a90318c1321330aafe21b618c5b70fcc8bf4bf5`
- The venue-account source collector resolved the configured SSM-backed Polymarket credentials and queried account state only. It did not submit/cancel orders, transfer funds, run no-submit, mutate root TOML, or execute a trade.
- Fail-closed confirmation: `operator-artifacts generate-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-024-final-6de829eb/static-attempt --strategy-instance-id bitcoin_updown_main` still refused final static readiness with `market-selection remains blocked: T046 missing source-bound price-to-beat strategy decision input; T046 remains blocked: missing source-bound price-to-beat strategy decision input; T121 remains blocked: T046 source-bound pre-run state evidence is unproven; panic gate and service policy`.
- Active-window Chainlink source collection for BTC/USD 5m at timestamp `1779820200` passed and wrote `/private/tmp/bolt-v2-chainlink-btc-1779820200.json` with sha256 `b593f2f91c69c3282ac0cb6eb4289d1ca7d3e1888c4ced2fe42e034afad629ef`. The decoded report value used for the follow-on source chain was `75979.20493228`.
- `collect-chainlink-entry-decision-proof-sources` then wrote source-bound proof artifacts under `/private/tmp/bolt-v2-t036-active-1779820200/`:
  - `entry-decision-fees.json`: `619d8717590e81afcc37738cd1448cf84794f5d193ba43db729bea8fea2d0919`
  - `source-bound-price.json`: `35599eb26248e3189cb9fb9a12943b5ce538cfeb7dfff8f84148e4c67ed074a0`
  - `realized-volatility.json`: `959ef6a814cc8ec9eba6e496b9f8a74cc2a958a1b932e5f5a7e544192011e50a`
  - `reference-quote.json`: `976225f587f9a01bd1149306bbf6326482e493d25565c1f907cc68342ec28a41`
- `collect-chainlink-entry-decision-source-inputs` consumed those bounded proofs plus live Polymarket selected-market instruments/books and wrote:
  - `entry-decision-source.json`: `82c66176488b4d3333e9cbde4877c854127d7d7cbbe6a8f97153407a716352ec`
  - `instrument-source.json`: `c1391982d108b5f2f57d2b284cb10fe8cb19f3cdbb6a44b78d12c2ed2a4f471b`
- `generate-entry-decision-evidence-from-source` still failed closed for the active BTC window, but the corrected current-head diagnostic now identifies the real cause instead of mislabeling it as submit admission: `blocked_reason=Some("no_side_selected") gate_blocked_by=[] pricing_blocked_by=[] selected_side=None up_worst_case_ev_bps=Some(-3007.0050157895344) down_worst_case_ev_bps=Some(-2578.836977677897) min_worst_case_ev_bps=Some(100.0) sized_notional=None`. That source chain was valid and source-bound, but it did not produce an entry order, so it cannot create the required strategy-input/order-intent/admission evidence chain.
- Current read-only funding and collateral rechecks with the same fee source still fail on the configured account: both `collect-pre-run-funding-margin-source` and `collect-pre-run-clob-v2-collateral-accounting-source` return ``CLOB V2 source field `p_usd_allowance` is invalid or unproven``.
- Fresh read-only recheck at `2026-05-26 19:09:25 UTC` with output targets `/private/tmp/bolt-v2-t036-active-1779820200/clob-v2-collateral-accounting-source-recheck-1779822.json` and `/private/tmp/bolt-v2-t036-active-1779820200/funding-margin-source-recheck-1779822.json` still failed both collectors with ``CLOB V2 source field `p_usd_allowance` is invalid or unproven``; neither output artifact was written.
- Root-cause check at head `a1953df8d0e4cadf82b21327da295a3fd57770ce`: the materializer rejects missing `BalanceAllowance.allowance` from NT before writing either funding/collateral proof. Official Polymarket docs state `getBalanceAllowance` returns both `balance` and `allowance`, `updateBalanceAllowance` updates the cached balance/allowance, and the deposit-wallet guide calls missing allowance an approval/sync condition rather than usable collateral (`https://docs.polymarket.com/trading/clients/l2`, `https://docs.polymarket.com/trading/deposit-wallets`).
- With explicit operator approval, added and ran `operator-artifacts sync-clob-v2-balance-allowance-cache --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --acknowledge-clob-cache-mutation` at `2026-05-27 03:03:42 UTC`. The command resolved the configured SSM-backed Polymarket credentials, sent authenticated `GET /balance-allowance/update` for `asset_type=COLLATERAL` using the configured `poly_gnosis_safe` execution signature type, and printed only non-secret metadata. This was a Polymarket CLOB cache mutation only: no submit, cancel, order placement, transfer, on-chain approval, root TOML edit, or secret display was performed.
- Post-sync read-only rechecks using `/private/tmp/bolt-v2-t036-active-1779820200/entry-decision-fees.json` sha256 `619d8717590e81afcc37738cd1448cf84794f5d193ba43db729bea8fea2d0919` still failed both collectors: `collect-pre-run-clob-v2-collateral-accounting-source` targeting `/private/tmp/bolt-v2-t036-active-1779820200/clob-v2-collateral-accounting-source-after-cache-sync.json` and `collect-pre-run-funding-margin-source` targeting `/private/tmp/bolt-v2-t036-active-1779820200/funding-margin-source-after-cache-sync.json` both returned ``CLOB V2 source field `p_usd_allowance` is invalid or unproven``; neither output artifact was written. A preceding local rerun with the wrong fee-behavior file failed before live balance/allowance fetch with a schema mismatch and did not exercise the CLOB allowance path.
- Read-only signature-type comparison with temp config copies found the same missing-allowance failure for `poly_gnosis_safe`, `poly_proxy`, and `eoa`; this does not look like a simple local `poly_proxy`/`poly_gnosis_safe` enum mix-up. During that earlier comparison, no submit, cancel, trade, allowance update, fund movement, or secret display was performed.
- Read-only Polygon `eth_call` against the official Polymarket pUSD proxy (`0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB`) and official CTF Exchange / Neg Risk CTF Exchange spenders (`0xE111180000d2663C0091e4f400237545B87B996B`, `0xe2222d279d744050d28e00520010520000310F59`) showed the configured public funder has `0x...00f075c8` pUSD balance, i.e. `15.758792` pUSD at 6 decimals, and max-uint pUSD allowance to both exchange contracts (`0xffff...ffff`). This does not satisfy the current authenticated CLOB `/balance-allowance` source collector, but it narrows the blocker to the CLOB cached balance/allowance proof path rather than an observed on-chain lack of pUSD balance or exchange allowance.
- Investigated whether the missing allowance was a `POLY_1271`/signature-type-3 issue. Official current Polymarket docs say new deposit-wallet API users use `POLY_1271`/`signatureType=3`, while existing Gnosis Safe users can keep signature type `2`. Read-only Polygon `eth_getCode` and storage slot `0` for the configured funder showed a proxy with master copy `0xE51abdf814f8854941b9Fe8e3A4F65CAB4e7A4a8`, which public explorer metadata identifies as `GnosisSafeL2` and the Polymarket Safe Proxy Factory master copy. The current `poly_gnosis_safe`/`signature_type=2` config therefore matches the configured funder class; `POLY_1271` support remains a separate future compatibility gap for deposit-wallet accounts, not the current T036 allowance root cause.
- Added regression coverage for the no-entry replay path: `cargo test --test bolt_v3_operator_artifacts entry_decision_evidence_source_collector_reports_no_entry_decision -- --nocapture` passed, proving a balanced/no-edge source is reported as `no_side_selected` and is not mislabeled as an admitted submit.
- Current-head verification after the diagnostic fix:
  - `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 188 passed, 0 failed.
  - `cargo test --test config_parsing -- --nocapture`: 138 passed, 0 failed.
  - `cargo test --test bolt_v3_cli -- --nocapture`: 48 passed, 0 failed.
  - `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`: 63 passed, 0 failed.
  - `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`: 31 passed, 0 failed, 1 ignored.
  - `cargo test --test bolt_v3_live_canary_gate -- --nocapture`: 70 passed, 0 failed.
  - `cargo fmt --check`: passed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `git diff --check`: passed.
- T040 local verification at head `a1953df8d0e4cadf82b21327da295a3fd57770ce`:
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `just source-fence`: passed, including runtime-literal verifier self-tests/audit, provider-leak verifier self-tests/audit, core-boundary verifier self-tests/audit, naming verifier self-tests/audit, status-map verifier self-tests/audit, schema-current verifier self-tests/audit, pure-Rust verifier self-tests/audit, legacy-default fence, strategy-policy fence, runtime-capture YAML checks, `cargo fetch --locked`, 11 `bolt_v3_controlled_connect` tests, and 5 `bolt_v3_production_entrypoint` tests.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - Focused readiness test suites from T039 remain current for this worktree: `bolt_v3_operator_artifacts` 188 passed; `bolt_v3_cli` 48 passed; `bolt_v3_tiny_canary_preconditions` 63 passed; `bolt_v3_tiny_canary_operator` 31 passed, 1 ignored; `bolt_v3_live_canary_gate` 70 passed.

T036 remains open. The Chainlink and Polymarket source-input chain is now materializable, but the sampled active window did not produce an entry decision, and the configured account still lacks proven pUSD allowance for funding/collateral source proofs. T036/T037/T038 cannot be closed until a source-bound window produces the required no-submit evidence chain and the account-side allowance proof passes.

## T036 On-Chain pUSD Collateral Proof Repair

At head `641ccc5205a20207f65cec03c488a9b9553dc48b`, the pUSD allowance blocker above was repaired without changing the remaining strategy-entry blocker.

- Implementation commit: `641ccc52 Add on-chain pUSD collateral proof`.
- Source ownership: `collect-pre-run-clob-v2-collateral-accounting-source` now uses the existing authenticated CLOB `/balance-allowance` path unless `[clients.<id>.execution.on_chain_collateral]` is configured. When configured, it performs read-only ERC-20 `balanceOf(address)` and `allowance(address,address)` calls through `nautilus_network::http::HttpClient`, uses `nautilus_polymarket::signing::eip712::{CTF_EXCHANGE, NEG_RISK_CTF_EXCHANGE}` as the spender identities, and uses `nautilus_polymarket::common::consts::USDC_DECIMALS` for pUSD scaling. The collector is still market-agnostic and derives the selected strategy/client/required notional from TOML and existing fee-source evidence.
- RPC endpoint proof: `https://polygon-rpc.com` failed read-only JSON-RPC with `API key disabled`; `https://1rpc.io/matic` returned HTTP 200 for the pUSD `balanceOf` probe, so ignored `config/live.local.toml` and tracked `config/root.toml` now use `https://1rpc.io/matic` for the on-chain collateral source.
- Live read-only collateral command using `/private/tmp/bolt-v2-t036-active-1779820200/entry-decision-fees.json` sha256 `619d8717590e81afcc37738cd1448cf84794f5d193ba43db729bea8fea2d0919` passed and wrote `/private/tmp/bolt-v2-t036-active-1779820200/clob-v2-collateral-accounting-source-on-chain.json` with sha256 `17b04bbfcaf2d98f922ac538a455bad609625494fcfe289d681a6af1c4ff0f8a`.
- The collateral summary was `collateral_accounting_verified=true`, `p_usd_balance=15.758792`, `required_max_notional_plus_fees=1.1`, and a max-uint effective pUSD allowance to the NT CLOB V2 spender set. No private key, API key, SSM value, submit, cancel, transfer, or on-chain transaction was used.
- Live read-only funding-margin command with the same fee source passed and wrote `/private/tmp/bolt-v2-t036-active-1779820200/funding-margin-source-on-chain.json` with sha256 `837c899cdc22ca80ed0c9c11bcd8b4d86adab78a123ab35bde7c4574e57ceda8`.
- Verification after the repair passed:
  - `cargo test --test bolt_v3_cli clob_v2_collateral_accounting_source -- --nocapture`: 7 passed.
  - `cargo test --test bolt_v3_cli funding_margin_source -- --nocapture`: 4 passed.
  - `cargo test --test bolt_v3_cli -- --nocapture`: 51 passed.
  - `just source-fence`: passed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
- Rechecked the strategy decision chain at `2026-05-27 04:25:07 UTC` with `generate-entry-decision-evidence-from-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --decision-source /private/tmp/bolt-v2-t036-active-1779820200/entry-decision-source.json --max-decision-source-bytes 100000 --instrument-source /private/tmp/bolt-v2-t036-active-1779820200/instrument-source.json --max-instrument-source-bytes 100000 --max-decision-evidence-bytes 100000`. It still failed closed with `blocked_reason=Some("no_side_selected")`, `gate_blocked_by=[]`, `pricing_blocked_by=[]`, `selected_side=None`, `up_worst_case_ev_bps=Some(-3007.0050157895344)`, `down_worst_case_ev_bps=Some(-2578.836977677897)`, `min_worst_case_ev_bps=Some(100.0)`, and `sized_notional=None`.
- The configured decision-evidence JSONL remained empty after the failed no-entry run: size `0`, sha256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`, line count `0`.

T036 remains open for a narrower reason: the current source-bound market facts still do not produce an entry order, so `strategy-input.json`, `pre-run-state.json`, `static-artifacts-manifest.json`, `approval-envelope.json`, and `operator-evidence-packet.json` cannot honestly be assembled yet. The previously documented pUSD allowance blocker is no longer current for the on-chain collateral source path.

## T036I4 NT-Owned Entry-Decision Fee Proof

At current head, the entry-decision fee proof no longer comes from caller-supplied CLI fee bps.

- Implementation: `collect-chainlink-entry-decision-proof-sources` no longer accepts `--fee-bps-by-instrument-id` and no longer writes `entry-decision-fees.json`.
- Source ownership: `collect-chainlink-entry-decision-source-inputs` now writes the fee-rate source artifact after the configured Polymarket provider selects instruments and books. The provider derives effective taker fee bps from NT `nautilus_polymarket::execution::parse::instrument_taker_fee` and `compute_commission`, using the selected up/down instrument metadata and the selected book ask prices. The only repo-local math is converting NT's one-share pUSD commission into bps for the existing artifact schema.
- Market scope: the implementation is not BTC-specific. The live probes used `bitcoin_updown_main` only because the ignored local TOML strategy target is BTC up/down 5m.
- RED/GREEN verification:
  - `cargo test --test bolt_v3_cli collect_chainlink_entry_decision -- --nocapture` first failed because source-input help still exposed `--fee-rate-source` and proof-source help still exposed caller fee arguments; after the change it passed with 2 tests.
  - `cargo test --test bolt_v3_operator_artifacts entry_decision -- --nocapture` passed with 19 tests, including cleanup on fee-output write failure and local-proof-before-network validation.
  - `just source-fence` passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.
- Live artifact recheck:
  - Replaying the old `/private/tmp/bolt-v2-t036-active-1779820200` proof set through the provider failed before fee derivation with `entry decision source requires a selectable two-sided configured market`.
  - Hard evidence for that failure: the old proof timestamp is `1779820200000` (`2026-05-26 18:30:00 UTC`), and current Polymarket Gamma returns an empty array for both old required slugs `btc-updown-5m-1779820200` and `btc-updown-5m-1779820500`.
  - Current-window Gamma still returns markets for current configured slugs, proving the failure is stale-market replay, not a hardcoded BTC path.
  - A fresh SSM-backed Chainlink report collection for timestamp `1779859200` wrote `/private/tmp/bolt-v2-t036-active-1779859200/chainlink-price-report.json` with sha256 `b601f1fea423c56f6d5eff0cc29904dea47e92adebc03794e33dab8b5a74faae`; decoded report value was `75502.71227` with `valid_from_timestamp_ms=1779859200000` and `observations_timestamp_ms=1779859200000`.

T036 remains open. The fee proof is now source-owned from NT-selected instruments/books, and pUSD collateral/funding proof has an on-chain source path, but final readiness still needs an honest source-bound entry decision chain.

## T036I5 NT-Owned Reference Quote And Realized-Volatility Proof

At current head, the entry-decision reference quote and realized-volatility proof no longer come from caller-supplied CLI values.

- Implementation: `collect-chainlink-entry-decision-proof-sources` now requires a bounded `--reference-quote-observations-source` file plus byte cap. It no longer accepts `--reference-quote-venue`, `--reference-quote-price`, `--reference-quote-observed-ts-ms`, `--realized-volatility-value`, or `--realized-volatility-ready-ts-ms`.
- Source ownership: `BoltV3NoSubmitReferenceQuote` now carries NT `QuoteTick` bid/ask prices in addition to timestamps. The new `operator-artifacts collect-reference-quote-observations-source` command runs the existing NT no-submit reference quote probe and writes `bolt_v3.reference_quote_observations_source.v1`; its structured artifact summary is path/hash-only and it does not print raw quote observations or secrets.
- Replay ownership: `write_entry_decision_proof_source_files` derives the proof midpoint and realized volatility from that quote-observation source through `binary_oracle_edge_taker::derive_entry_reference_proofs_from_quote_observations`, which parses the same strategy runtime config and uses the existing `RealizedVolEstimator` logic. This is not BTC-specific; it matches the configured strategy `reference_data` client/instrument and runtime volatility settings.
- RED/GREEN verification:
  - `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_derives_reference_quote_and_volatility_from_quote_observations -- --nocapture` first failed because `EntryDecisionProofSourceMaterializationRequest` still had only raw reference quote and realized-volatility fields; after the change it passed.
  - `cargo test --test bolt_v3_operator_artifacts reference_quote_observations_source_materializer_writes_nt_quote_probe_prices -- --nocapture` first failed because NT quote evidence did not carry bid/ask prices and the source writer did not exist; after the change it passed.
  - `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_reference_quote_observations_source -- --nocapture` first failed because the collector command did not exist; after the change it passed.
- Focused verification after implementation:
  - `cargo test --test bolt_v3_operator_artifacts entry_decision -- --nocapture`: 20 passed.
  - `cargo test --test bolt_v3_operator_artifacts reference_quote_observations_source_materializer -- --nocapture`: 1 passed.
  - `cargo test --test bolt_v3_cli entry_decision -- --nocapture`: 5 passed.
  - `cargo test --test bolt_v3_cli reference_quote_observations -- --nocapture`: 1 passed.
  - `cargo test --test bolt_v3_no_submit_readiness reference -- --nocapture`: 12 passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `python3 scripts/test_verify_bolt_v3_runtime_literals.py`: passed.
  - `python3 scripts/verify_bolt_v3_runtime_literals.py`: passed.
  - `cargo clippy --locked --lib -- -D warnings`: passed.
  - `cargo clippy --locked --bin bolt-v2 -- -D warnings`: passed.
- Live no-submit quote-observation attempt:
  - Command: `cargo run --bin bolt-v2 -- operator-artifacts collect-reference-quote-observations-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-reference-quote-observations-source.json`.
  - Result: failed closed and wrote no output file.
  - Hard evidence: ignored `config/strategies/binary_oracle.local.toml` currently sets `[reference_data.primary] data_client_id = "polymarket_main"` and `instrument_id = "condition-1-UP.POLYMARKET"`, while the no-submit run loaded current Polymarket instruments and then NT rejected the subscription with `Instrument condition-1-UP.POLYMARKET not found, and auto_load_missing_instruments is disabled`. The command then stopped cleanly and returned `NoSubmitReferenceProbeFailed { reason: "reference quote probe did not observe all configured reference_data quotes within [live_canary].reference_quote_wait_timeout_seconds=20" }`.
  - Scope/safety: this was a no-submit reference-data probe only. It connected Polymarket data/execution clients for NT no-submit readiness, reconciled account state, subscribed/unsubscribed quotes, and stopped. It did not submit, cancel, trade, transfer funds, mutate CLOB allowance/cache state, mutate on-chain state, print secrets, or write a proof artifact.

T036 remains open. The source-owned quote/volatility proof code path is now present, but the current ignored local strategy reference-data config is stale/static for a rotating Polymarket market, so the live NT quote-observation source cannot yet be collected from this config.

## T036I6 Chainlink Feed-Binding Coverage

At current head, Chainlink Data Streams feed selection is now validated across the loaded root and strategy TOML instead of relying on implicit token fallback behavior.

- Implementation: `validate_strategies` now cross-checks every loaded strategy target Chainlink mapping by `(provider_id, resolution_identity, value_kind)` against `[gate_providers.<id>.chainlink_data_streams].feed_bindings`, requires exactly one matching TOML-owned binding, and rejects any Chainlink feed binding not referenced by a loaded strategy mapping.
- Scope: this is a config/validation guard only. It does not fetch Chainlink, read SSM, connect to a venue, submit/cancel orders, mutate accounts, or execute a trade.
- Canonical config cleanup required by the new guard: removed the shipped unused secondary feed binding from `config/root.toml`, `tests/fixtures/bolt_v3/root.toml`, and ignored `config/live.local.toml`; the loaded strategies only map the current configured primary resolution identity.
- RED/GREEN verification:
  - `cargo test --test config_parsing rejects_chainlink_target_mapping_without_matching_feed_binding -- --nocapture`: failed before the validator with empty validation messages, then passed after implementation.
  - `cargo test --test config_parsing rejects_unreachable_chainlink_feed_binding -- --nocapture`: failed before the validator with empty validation messages, then passed after implementation.
  - `cargo test --test config_parsing chainlink -- --nocapture`: 6 passed.
  - `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_selects_alt_feed_by_resolution_identity -- --nocapture`: passed.
  - `cargo test --test bolt_v3_cli bolt_v3_cli_collects_chainlink_price_report_source_without_printing_credentials -- --nocapture`: passed.
  - `cargo fmt`: passed.
- Follow-up hard evidence: `operator-artifacts generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-t036-check-2/base-static --strategy-instance-id bitcoin_updown_main` passed after removing the ignored local unused secondary binding and wrote `ssm-manifest.json`, `financial-envelope.json`, and `approval-nonce.json`.

T036 remains open. A fresh `collect-reference-quote-observations-source --config config/live.local.toml --strategy-instance-id bitcoin_updown_main --output /private/tmp/bolt-v2-t036-final-current/reference-quote-observations-source.json` attempt still failed closed because the ignored local strategy points `[reference_data.primary]` at `polymarket_main` / `condition-1-UP.POLYMARKET`; NT loaded current Polymarket slug instruments, rejected that stale static instrument with `Instrument condition-1-UP.POLYMARKET not found, and auto_load_missing_instruments is disabled`, and returned `NoSubmitReferenceProbeFailed { reason: "reference quote probe did not observe all configured reference_data quotes within [live_canary].reference_quote_wait_timeout_seconds=20" }`. This confirms the remaining blocker is not Chainlink feed-id tracking or pUSD allowance; it is the missing source-owned, market-agnostic decision-reference path now tracked as `T036I7`.

## T036I7 Source-Owned Decision-Reference Proof Path

At current head, the stale/static local `reference_data` blocker has been replaced by a TOML-owned `decision_reference` gate subscription and a bounded Chainlink report-sequence materializer for reference quote observations.

- Implementation: added `write_chainlink_reference_quote_observations_source_from_report_files` and the CLI command `operator-artifacts collect-chainlink-reference-quote-observations-source`. The command consumes one or more bounded Chainlink report source files plus approved sha256 values, decodes each report through the configured `(provider_id, resolution_identity, value_kind)` feed binding, and writes `bolt_v3.reference_quote_observations_source.v1` without printing raw reports.
- Runtime bridge: `binary_oracle_edge_taker::raw_taker_config` now derives `reference_venue` and `reference_instrument_id` from `target.gate_subscriptions.decision_reference` when legacy `[reference_data.<role>]` is absent. If both are configured, it fails closed instead of allowing dual reference paths.
- Local/config cleanup: ignored `config/strategies/binary_oracle.local.toml` no longer points at static `polymarket_main` / `condition-1-UP.POLYMARKET` reference data. Shipped example/fixture configs now declare `decision_reference` alongside `resolution`, and the Chainlink provider capability includes `reference_value`.
- Anti-hardcode cleanup in the touched slice: removed misleading cadence- and asset-shaped reference identities from tracked config/spec/test paths, replacing the fixture identity with `configured-reference-price`. Synthetic quote-observation helpers in the touched tests now use bid=ask for reference values instead of a fake spread.
- Scope: this does not make Chainlink globally required. The materializer dispatches only when the configured `decision_reference` provider kind is Chainlink Data Streams; provider-neutral readiness/session logic remains separate.
- RED/GREEN verification:
  - `cargo test --test bolt_v3_operator_artifacts chainlink_reference_quote_observations_source_materializer -- --nocapture`: first failed on missing public API, then passed.
  - `cargo test --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_derives_reference_quote_and_volatility_from_chainlink_reports_without_reference_data -- --nocapture`: passed.
  - `cargo test --test bolt_v3_cli bolt_v3_cli_exposes_collect_chainlink_reference_quote_observations_source -- --nocapture`: first failed because the command was missing, then passed.
  - `cargo test --test bolt_v3_cli bolt_v3_cli_collects_chainlink_reference_quote_observations_source_without_printing_reports -- --nocapture`: passed.
- Focused verification after cleanup:
  - `cargo test --test config_parsing -- --nocapture`: 142 passed.
  - `cargo test --test bolt_v3_operator_artifacts -- --nocapture`: 194 passed.
  - `cargo test --test bolt_v3_cli -- --nocapture`: 54 passed.
  - `cargo test --test bolt_v3_strategy_registration -- --nocapture`: 23 passed.
  - `cargo test selected_market_requirement --lib -- --nocapture`: 11 passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.

T036 remains open. T036I7 removed the stale/static reference-data blocker and added the source-owned decision-reference proof path, but the final packet still needs fresh source artifacts, `operator-evidence` binding, and final verification through T036-T038.

## T036 Current Source-Input Retry After T036I7

After T036I7, the previous stale `reference_data` blocker no longer reproduces. A fresh source-owned Chainlink decision-reference chain was materialized from current-window testnet reports:

- `generate-base-static --config config/live.local.toml --output-dir /private/tmp/bolt-v2-t036-current/base-static --strategy-instance-id bitcoin_updown_main` passed and wrote `ssm-manifest.json` sha256 `5eaf8a4501819c1dae10dabf7597ba77093814116f9d4112cd69ab106a49c7a1`, `financial-envelope.json` sha256 `adb344a0bdb1d03e2a0f1f24e7acf277f153e00279e054a127dd4100568aef98`, and `approval-nonce.json` sha256 `60c8314e01a7ad246529b8c6d6a0ee0a863a3ffbf686231b9b0ded1f80dccdda`.
- SSM-backed Chainlink testnet report collection succeeded for 20 current-window report timestamps from `1779870000` through `1779870190`, every 10 seconds, without printing credentials or raw report bodies.
- `collect-chainlink-reference-quote-observations-source` passed for those 20 reports and wrote `/private/tmp/bolt-v2-t036-current/reference-quote-observations-source.json` sha256 `debc59f69c023475388b9946fc220bcb75de163910d291ae71b908f3ad249909`.
- `collect-chainlink-entry-decision-proof-sources` passed for market-selection timestamp `1779870000000` and decision timestamp `1779870190000`, writing:
  - `/private/tmp/bolt-v2-t036-current/price-to-beat-source.json` sha256 `b2359dd2827e00530c38cdee7852df79847e528449e411067fe83bf8cfe9b1b1`
  - `/private/tmp/bolt-v2-t036-current/reference-quote-source.json` sha256 `172f18894a060d5514b4ca2e25a345ccdaead836369f3e9862890df3986ec88e`
  - `/private/tmp/bolt-v2-t036-current/realized-volatility-source.json` sha256 `3418af8f0291857f1152a7890a3ea6762932ae58d310ba88b2aae1e51f2afdcf`
- `collect-chainlink-entry-decision-source-inputs` then failed closed before writing `entry-decision-source.json`, `instrument-source.json`, or `fee-rate-source.json` with: `entry decision evidence source is invalid: entry decision source up book is missing best ask`.
- Immediate retry of the same source-input command failed closed earlier in the same selected-market book fetch path with: `entry decision evidence source is invalid: failed to fetch up book snapshot: HTTP error 404: {"error":"No orderbook exists for the requested token id"}`.

T036 remains open for a new reason: the configured/current source-input chain now reaches the selected market orderbook proof and is blocked by unavailable or non-two-sided up-book liquidity for the selected token. This is no longer the stale `reference_data` / static condition-id blocker, and it is not a Chainlink feed-id or pUSD allowance blocker.

## T036-T038 Final Packet Assembly And Pre-Run Verification

At head `a15495e1b471b2a24a2a234e0f505f1d0eedd99a`, the current source-input chain was refreshed through final-packet pre-run verification.

- Root cause fixed before assembly:
  - `selected_updown_market_start_uses_configured_period_not_gamma_creation_time` first failed because live Gamma `startDate` was treated as the rotating window start; the selected market now uses the configured period start from the slug/window.
  - `strategy_input_writer_emits_phase8_artifact_from_runtime_snapshot_and_market_source` first failed with `phase8 strategy input evidence strategy_instance_id does not match runtime strategy_id`; the artifact builder now preserves operator `strategy_instance_id` while validating the NT runtime `StrategyId` derived from the loaded strategy config.
- Fresh source-bound decision chain:
  - `entry-decision-source.json`: `e5d44bc6537c5c4e59e66a9db073c108e93f8229f28259a52316b55b3c377c84`
  - `instrument-source.json`: `845cb4a9326e1a5f7cdd3018df0631cdfb56dabf6df0ea636112081af50122e5`
  - `fee-rate-source.json`: `3c34ba73bcef23697f852d418ca57c68abc2b4fb1b66474cb836a0147c3f71f7`
  - Decision evidence JSONL: `0ff50f02b21aec9355b85c229444914ba5b7db70351a0b7254300825791cf135`
- T036 final artifact outputs are recorded in `specs/024-production-trade-readiness/final-packet.md`. Key hashes:
  - `static-artifacts-manifest.json`: `0c03542f6002ab2c64fed001670faa375dca215d776ef286d338bc8fbc5bd13d`
  - `approval-envelope.json`: `cb0358d2e6473fad5829d0e342c957e36b1c4d6827b7ce72ce9ae99ec59e0952`
  - `operator-evidence-packet.json`: `9637e10aafbce374ce9d95cf5de1221f870958bfa7e75b399d5439d20697a70d`
- T037 local operator-evidence patch:
  - `operator-artifacts update-operator-evidence-toml --config config/live.local.toml --operator-evidence-json /private/tmp/bolt-v2-t036-final-attempt-3/operator-evidence.json --max-operator-evidence-json-bytes 65536`
  - Result: root TOML sha256 `c02034fc0131a8bb6f5326ff771aaa5693388fbcdce17a34dd63652f7da8ce9a`.
- T038 pre-run verification:
  - Command: `operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t036-final-attempt-3/operator-evidence-packet.json --verification-stage pre-run`
  - Result: passed. Verified `approval-envelope` `cb0358d2e6473fad5829d0e342c957e36b1c4d6827b7ce72ce9ae99ec59e0952`, `operator-evidence-packet` `9637e10aafbce374ce9d95cf5de1221f870958bfa7e75b399d5439d20697a70d`, and `static-artifacts-manifest` `0c03542f6002ab2c64fed001670faa375dca215d776ef286d338bc8fbc5bd13d`.

Scope and side effects: source collection used SSM-backed Chainlink report reads, read-only configured Polymarket account/funding/collateral checks, public provider-time collection, and a read-only EC2/SSM egress identity refresh plus local mirror restore. No no-submit, tiny-capital canary, submit, cancel, transfer, on-chain mutation, root tracked TOML mutation, secret display, or trade operation was run. T036, T037, and T038 are locally closed; post-run T043/T044 evidence remains future work.

## T047 Final Hardcode And Architecture Cleanup

T047 removed the remaining generic Rust fixture hardcodes that made BTC, Binance, and 5-minute cadence look canonical outside provider-specific Binance tests.

- Generic Rust hardcode scan:
  - `rg -n "btc_updown_5m|bitcoin_updown_main|condition-1|BTCUSDT|ETHUSDT|btc-updown-5m|example-resolution-5m|underlying_asset.*BTC|cadence_slug_token.*5m|btc-usd" src tests --glob '*.rs'`: no matches.
  - `rg -n "\bBTC\b|\bETH\b|5m" src tests --glob '*.rs'`: only unrelated `tests/lake_batch.rs` timing-comment text remains.
- Cleanup scope:
  - Shared examples/fixtures use `configured_updown_main`, `configured_updown_target`, `CONFIGURED_ASSET`, `configuredwindow`, and `configured-reference-price` instead of BTC/Binance/5m-shaped identities.
  - Provider-specific Binance tests now create scoped test-local Binance clients instead of relying on the canonical root fixture to contain `binance_reference`.
  - Source-owned financial-envelope validation now binds TOML `cadence_slug_token`; schema docs were updated and `just source-fence` enforces the Rust/doc field set.
- Verification:
  - `cargo test --locked --test bolt_v3_tiny_canary_operator --test bolt_v3_strategy_registration --test nt_custom_data_catalog_integration --test nt_polymarket_filter_integration --test config_parsing -- --nocapture`: passed.
  - `cargo test --locked --lib --test bolt_v3_tiny_canary_preconditions --test bolt_v3_operator_artifacts --test bolt_v3_provider_binding --test bolt_v3_cli -- --nocapture`: passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `just source-fence`: passed.
- Final-packet refresh:
  - The prior `/private/tmp/bolt-v2-t036-final-attempt-3/operator-evidence-packet.json` failed current pre-run verification with `operator packet config_bundle_checksum does not match loaded config`.
  - After committing T047 as `48a9c0df7846c4d08cf7aa877d96cedb8043ee12`, the pre-commit T047 packet failed current pre-run verification with `[live_canary.operator_evidence].head_sha does not match build head_sha`.
  - Refreshed exact-head non-live artifacts under `/private/tmp/bolt-v2-t047-final-refresh` and patched ignored `config/live.local.toml`; root TOML sha256 after patch is `057170cf556295ff244c13c4327efa0adee445f777dd18a81a39f77f1dc794f3`.
  - `operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t047-final-refresh/operator-evidence-packet-48a9c0df.json --verification-stage pre-run`: passed. Verified `approval-envelope` `94215f1e08fe7fb94dc00f0c7c064c7bd2f188f104051bfe52c6dc81e57fed01`, `operator-evidence-packet` `e8e985b844c8628ab852606c9ad4d6a605110159bc0b62fb9d9e3d3b7e543e0b`, and `static-artifacts-manifest` `2c0ba198187487449a35dc69dd79e539596f0eeaf1c56ad8f8bd901525b0e0af`.

No no-submit, tiny-capital canary, submit, cancel, transfer, on-chain mutation, secret display, or trade operation was run during T047.

## T043 Final-Packet No-Submit

T043 passed for build head `b993299e5aa234c199c5b97cc3a2393fcf9e2c03` after the no-submit reference-readiness path was repaired to use source-owned `decision_reference` operator evidence when legacy `[reference_data]` is absent.

- RED/GREEN repair:
  - `cargo test --test bolt_v3_operator_artifacts source_owned_reference_readiness_accepts_replayable_operator_evidence_without_reference_data -- --nocapture`: first failed because `verify_source_owned_reference_readiness_from_operator_evidence` was missing, then passed.
  - `cargo test --test bolt_v3_no_submit_readiness no_submit_readiness_switches_to_source_owned_reference_when_reference_data_absent -- --nocapture`: first failed because `reference_readiness_from_no_submit_evidence` was missing, then passed.
- Focused verification:
  - `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`: 34 passed.
  - `cargo test --test bolt_v3_operator_artifacts final_packet_verifier -- --nocapture`: 32 passed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `just source-fence`: passed.
- Final-packet refresh:
  - Root TOML sha256 after local ignored `config/live.local.toml` operator-evidence patch: `f740afb999a7d2982cef7f3eecd2b493cb64784b73ec2a41a16f4fab0875f5ea`.
  - `operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t043-final-refresh-b993299e/operator-evidence-packet-b993299e.json --verification-stage pre-run`: passed. Verified `approval-envelope` `7e541dc5fe5bb90bbad3507d13cae92253eb10a006d2ff31578faf5959b38e67`, `operator-evidence-packet` `47af7a6ace5fe17da095d69084c5615caf279ebbe31391ee0ca97796be8e3372`, and `static-artifacts-manifest` `710e64947c98a8f052aaebabe1ceff4480bc018a68043dad63b32983075c8bf2`.
- No-submit report:
  - `cargo run --locked --bin bolt-v2 -- no-submit-readiness --config config/live.local.toml`: exited 0 and wrote `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`.
  - Report sha256: `ec5b5147c7816e4684d83e2ea0c5ffd5db1e353a409d98579bf267d86d7d40ef`.
  - Report generated at `2026-05-27T15:39:27Z` with all seven stages satisfied: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`, `controlled_disconnect`, and `report_write`.

Scope and side effects: this was a no-submit readiness run. It connected the configured Polymarket data and execution clients, reconciled account state, observed zero orders/fills/positions, stopped the NT runner, and wrote the readiness report. It did not submit, cancel, transfer, mutate on-chain state, mutate CLOB allowance/cache state, or execute a trade. T044 remains open and requires renewed explicit operator approval because it is a tiny-capital live canary.

## Readiness Repair Batch

A source-shard review at head `2947546c2b5ef8c88e67ec4e3b2dcbfb323fba5d` found real issues that were repaired locally before the next push.

- Review findings fixed:
  - Market-selection source evidence now rejects decision evidence whose `price_to_beat_source` does not match the TOML financial envelope.
  - Strategy-input evidence now rejects runtime snapshots whose `price_to_beat_source` does not match the TOML financial envelope.
  - Funding-margin proof collection now uses the same CLOB V2 decimal-string comparator as the funding-margin source writer.
  - Chainlink ReportDataV3 benchmark decoding now validates ABI `int192` sign extension and scales the full-width two's-complement value without `Decimal::from_i128_with_scale`, avoiding Rust Decimal max-scale panics and the previous i128 bound.
- RED evidence before fixes:
  - `cargo test --locked --test bolt_v3_operator_artifacts market_selection_source_writer_rejects_price_to_beat_source_mismatch -- --nocapture`: failed because mismatched price source was accepted.
  - `cargo test --locked --test bolt_v3_operator_artifacts strategy_input_writer_rejects_runtime_snapshot_target_gate_and_hash_mismatches -- --nocapture`: failed for the added price-source mismatch case because mismatched price source was accepted.
  - `cargo test --locked --test bolt_v3_operator_artifacts pre_run_funding_margin_source_proof_uses_source_decimal_comparator -- --nocapture`: failed because the proof collector parsed `available_collateral` through Rust Decimal instead of the source decimal-string comparator.
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_accepts_chainlink_scale_beyond_rust_decimal_limit -- --nocapture`: failed with a Rust Decimal panic, `Scale exceeds the maximum precision allowed: 29 > 28`.
  - `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_decodes_chainlink_int192_benchmark_price_without_i128_bound -- --nocapture`: failed with invalid source-bound price report provenance for a valid full-width ABI `int192` benchmark word.
- GREEN verification after fixes:
  - `cargo test --locked --test bolt_v3_operator_artifacts -- --nocapture`: 199 passed, 0 failed.
  - `cargo fmt --check`: passed.
  - `git diff --check`: passed.
  - `just source-fence`: passed.

Scope and side effects: this was local source/test/audit work only. No no-submit, tiny-capital canary, submit, cancel, transfer, on-chain mutation, secret display, GitHub CI run, or trade operation was performed. The later T041 section records the exact pushed-head CI evidence. T044 remains gated on renewed explicit operator approval.

## T041 Exact-Head GitHub CI

T041 is complete for PR #480 reviewed code head `8b95eca9c2f410ff462954cff90c4734d01593cb`.

- Local branch state before evidence update:
  - `git rev-parse HEAD`: `8b95eca9c2f410ff462954cff90c4734d01593cb`
  - `git status --short --branch`: `## goal/024-production-trade-readiness...origin/goal/024-production-trade-readiness`
- `gh pr view 480 --json number,headRefName,headRefOid,baseRefName,baseRefOid,statusCheckRollup,url`:
  - PR: https://github.com/seungpyoson/bolt-v2/pull/480
  - Base: `main` at `53fd50d2ccd05a81e9ca65575594514315511fdc`
  - Head branch: `goal/024-production-trade-readiness`
  - Head OID: `8b95eca9c2f410ff462954cff90c4734d01593cb`
  - Successful checks: `detector`, `Analyze (actions)`, `actionlint`, `Analyze (rust)`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, `nextest archive`, `build`, `nextest shard 1 of 4`, `nextest shard 2 of 4`, `nextest shard 3 of 4`, `nextest shard 4 of 4`, `test`, `gate`, and `CodeQL`.
  - Skipped checks: `same-sha-main-evidence` and `deploy`.

This evidence update is a docs-only audit-log change after the reviewed code head. Per the existing self-referential review-loop convention, the review manifests and `gh pr view` output are the authoritative proof for the reviewed head; CI will be checked again once the remaining live canary, hygiene, and ledger work is complete rather than after every docs-only update.

## T043B Selected Trade-Path Readiness

T043B is locally complete for build head `978618f85e12b81ea56dab2f2e11aa6156d022e0` using temp root `/private/tmp/bolt-v2-t043b-978618f8/live.local.toml`.

- Source-bound decision chain:
  - Market window: `1780015500`; decision timestamp: `1780015510`.
  - Last Chainlink report: `/private/tmp/bolt-v2-t043b-978618f8/reports/chainlink-price-report-1780015510.json`, sha256 `4bcca226dda6c939321158324424f24a2a23afd04807020f75d337bedaca8ed1`.
  - Strategy input: `/private/tmp/bolt-v2-t043b-978618f8/strategy-input-1780015500-plus10.json`, sha256 `cbfbffdb560fea83521b2e8480e2303e8f022150548bdda1a72debf93c624ada`.
  - Entry readiness gate session: `/private/tmp/bolt-v2-t043b-978618f8/entry-readiness-gate-session-1780015500-plus10.json`, sha256 `d106bca26d7bd347d05313102ed1e470ab051cd1aacae5b996acd4541f9b1ad1`.
- Static/pre-run artifacts:
  - `ssm-manifest.json`: `501002f491b4aad097cad6524a439ae6968d751e822d278cdb5e0816f7597c22`.
  - `financial-envelope.json`: `076b7ce1374abf89ed553adef9064f7c6c410f485484dcfcf6624d6b776afd33`.
  - `approval-nonce.json`: `193069be3ed1483dd33215ad9b2e65977cc61a90cde1a5a2dab4ac46f9771849`.
  - `pre-run-state-1780015500-plus10.json`: `f25f1b0efcb640fdb3edaebbd66a9e443133d9d1a330bef72b7245160ba63bed`.
  - `abort-plan-1780015500-plus10.json`: `8bca1bddfc927973a3819dfc5bb211795034fdf560b9ce1561a93c28aae69182`.
- Final-packet verification:
  - `operator-artifacts generate-operator-evidence-json` wrote `/private/tmp/bolt-v2-t043b-978618f8/operator-evidence-1780015500-plus10.json`, sha256 `69838a049e3ad8de272ec3fcafea5f43a31e330204992b4f5256e0f9d9827398`.
  - `operator-artifacts update-operator-evidence-toml` patched only the temp root and produced root TOML sha256 `aee5efa9f2fa27752e3f22b5a6c14ce194865dedc75869126a0528ceb849227b`.
  - `operator-artifacts write-manifest-from-operator-evidence` wrote static manifest sha256 `33e2389848f6f5db52f3fca74ff50342c77e44dcd98be9eb226c686d358c10d8`.
  - `operator-artifacts assemble-final` wrote approval envelope sha256 `8a175d6e9ec26293eb69454d9db7c15efcbd47e54ebaac335d593b0d83cfded8` and operator packet sha256 `d3e7bc375054cf4c7197f3b8ba1e902060f109bc3d40370b7908b215f83e593d`.
  - `operator-artifacts verify-final --config /private/tmp/bolt-v2-t043b-978618f8/live.local.toml --operator-packet /private/tmp/bolt-v2-t043b-978618f8/operator-evidence-packet-1780015500-plus10.json --verification-stage pre-run`: passed.
- No-submit readiness:
  - `/Users/spson/.cache/rust-verification/bolt-v2/target/debug/bolt-v2 no-submit-readiness --config /private/tmp/bolt-v2-t043b-978618f8/live.local.toml`: exited 0.
  - Report path: `var/bolt-v3-live/reports/no-submit-readiness.json`, sha256 `b89fbaef4d73d8e4e50a80afcd830ae414d2a9e0eddb62064cfe34a91f7308d7`.
  - Report schema `bolt-v3.no-submit-readiness.v2`, generated at Unix seconds `1780015972`, config bundle checksum `e745353fe5883eb49900591b4a0d3e7a313e5dc625e9e7f7d707024c80856f36`, with all seven stages satisfied: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`, `controlled_disconnect`, and `report_write`.

Scope and side effects: this was final-packet pre-run verification plus no-submit readiness. It connected and disconnected the configured selected data/execution clients, reconciled account state, and wrote the readiness report. It did not run the live runner, consume live approval, submit/cancel orders, transfer funds, mutate on-chain state, mutate CLOB allowance/cache state, print secrets, or execute a trade. Because committing this evidence changes `HEAD`, rerun final-packet verification and no-submit once more at the post-docs exact head before T044 approval consumption.

Post-doc checkpoint rerun for head `6a28cc7f1a42e8f9b580a8033038f4defe8c7597`:

- `operator-artifacts generate-operator-evidence-json` wrote `/private/tmp/bolt-v2-t043b-978618f8/operator-evidence-6a28cc7f.json`, sha256 `58e46b4c446b9c32ed65da1e2c9aaaf4abb9e639f907c3c6a301ea4afa42c109`.
- `operator-artifacts update-operator-evidence-toml` patched only the temp root and produced root TOML sha256 `e7284aa9ef78aebc21d985d0bee2df5cff8c72e89dc34f4e7d237da1df50e9ac`.
- `operator-artifacts write-manifest-from-operator-evidence` wrote static manifest sha256 `dd902343c29c71756c262edcb4b6e62dad5d427b3030c763629e6f71da6a6536`.
- `operator-artifacts assemble-final` wrote approval envelope sha256 `71d05b539a028bccf62fda555221713793a56b309e7275d4347498a43377e980` and operator packet sha256 `66342ac7efca3642bdc76e9c5f222d63718118bccee6ab4d9ad9eb0e281eb24e`.
- `operator-artifacts verify-final --config /private/tmp/bolt-v2-t043b-978618f8/live.local.toml --operator-packet /private/tmp/bolt-v2-t043b-978618f8/operator-evidence-packet-6a28cc7f.json --verification-stage pre-run`: passed.
- `/Users/spson/.cache/rust-verification/bolt-v2/target/debug/bolt-v2 no-submit-readiness --config /private/tmp/bolt-v2-t043b-978618f8/live.local.toml`: exited 0 and wrote `var/bolt-v3-live/reports/no-submit-readiness.json`, sha256 `fca8919f24cb9c94ed42e3ff5a2e341af85fdc81716627eedb2e98d01a30f946`.
- No-submit report schema `bolt-v3.no-submit-readiness.v2`, generated at Unix seconds `1780016379`, executable identity `f26954b50091d534ef04ad37efc34c760f5733d5a8bf21f9c30aa4d9a08e7c02`, config bundle checksum `d4c5067aed49e95186b1f9a0b7276b58ef4eb67666a01273b83fea78f2c72414`, with all seven stages satisfied.

Scope and side effects: the post-doc checkpoint rerun was final-packet pre-run verification plus no-submit readiness. It did not run the live runner, consume live approval, submit/cancel orders, transfer funds, mutate on-chain state, mutate CLOB allowance/cache state, print secrets, or execute a trade. This record is historical evidence for `6a28cc7f`; any later commit still requires another exact-head rerun before T044 live approval consumption.

## T044 Source-Only Retry and Boundary Price Repair

The operator approved a T044 tiny-capital canary at head `9fa1500535e7c02a2df061938d695f1a6741d903`, with max 1 live order and `max_notional_per_order = 1.00`.

Two approved source-collection attempts failed before live approval consumption:

- `/private/tmp/bolt-v2-t044-approved-9fa15005-1780019700`
- `/private/tmp/bolt-v2-t044-approved-9fa15005-1780020300`

Both attempts failed during source-owned entry-decision evidence generation with `blocked_reason = "no_side_selected"`. The live runner was not entered, no approval-consumption proof was written, no order was submitted, and no trade occurred.

Root cause: the source collection path allowed a non-boundary Chainlink report to be used as source-owned `price_to_beat`, so the market boundary price and current decision reference could both come from the same latest report. The fix requires the `price_to_beat` report's `validFromTimestamp` to equal the selected market boundary timestamp while leaving the current reference path free to use later observations.

Repair commit: `b9a15da363e1cb09750e254d77c5370d6a42e154`.

Verification:

- `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer_rejects_non_boundary_price_to_beat_report -- --nocapture`: passed.
- `cargo test --locked --test bolt_v3_operator_artifacts entry_decision_proof_source_materializer -- --nocapture`: 11 passed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.

Scope and side effects: this was source collection plus local code/test verification only. No live runner, approval consumption, no-submit run, venue submit/cancel, transfer, on-chain mutation, CLOB allowance/cache mutation, secret display, or trade operation was performed after the new approval. T044 remains open and requires an exact-head source packet, final-packet pre-run verification, no-submit readiness, and renewed explicit approval at the current head before live approval consumption.

## T044 Current-Head Source Chain Result

At head `7efad2cb7`, the current-window source collection root `/private/tmp/bolt-v2-t044-preflight-7efad2cb-current-1780021563` proved the source path past the prior boundary-report bug:

- Chainlink boundary/reference/volatility source proofs were collected for configured market window `1780021500`.
- Polymarket instrument, book, and fee-rate source proofs were collected.
- `generate-entry-decision-evidence-from-source` then failed closed with `blocked_reason = "no_side_selected"` because both configured outcome sides had negative worst-case EV at the observed prices.

No decision-evidence JSONL, strategy-input artifact, gate session, final packet, no-submit report, approval-consumption proof, canary evidence, order-submit artifact, venue-order artifact, transfer, on-chain mutation, CLOB allowance/cache mutation, secret display, or trade was produced by this attempt. T044 remains open. The exact current blocker is the configured strategy gate refusing to create an entry order for the observed market.
