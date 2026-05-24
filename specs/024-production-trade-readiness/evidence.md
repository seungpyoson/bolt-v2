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
- `collect_pre_run_market_window_source_proof`
- `collect_pre_run_single_runner_lock_source_proof`
- `collect_pre_run_egress_identity_source_proof`

The same search found no `pub fn collect_pre_run_clob...`, `collect_abort_plan_nt...`, `collect_abort_plan_partial...`, `collect_abort_plan_network...`, or `collect_abort_plan_panic...` functions.

Implication: the active readiness branch has some source-owned collectors, but most T126/T127 fields are still satisfied only by caller-supplied proof bundles or fixtures.

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
