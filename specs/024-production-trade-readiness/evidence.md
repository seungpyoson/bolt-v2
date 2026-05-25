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
- `just source-fence`: passed, including runtime literal/provider/core/naming/status/schema/pure-Rust/default/strategy-policy/source-capture checks plus 11 `bolt_v3_controlled_connect` tests and 5 `bolt_v3_production_entrypoint` tests.

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
