# Issue #409 PortfolioSnapshot Evidence

Date: 2026-05-25
Scope: read-only acceptance investigation for #409 PortfolioSnapshot runtime capture.

No source or docs were edited during the investigation. No live, no-submit, AWS, SSM, trading, submit, cancel, replace, transfer, or deployment command was run.

## Commands And Results

- `gh issue view 409 --json number,title,state,body,labels,assignees,comments,url`
- `rg` targeted `PortfolioSnapshot`, `portfolio_snapshot`, and related capture strings across `src`, `tests`, `docs`, `scripts`, and `specs`.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_verify_runtime_capture_yaml.py`
  - result: `Ran 64 tests ... OK`
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify_runtime_capture_yaml.py`
  - result: `OK: all 15 runtime-capture YAML checks passed.`
- `cargo test --test nt_runtime_capture captures_broad_nt_runtime_jsonl_records_outside_hot_path -- --nocapture`
  - sandbox cache-lock run failed on `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`
  - approved rerun passed: `1 passed`
- `just source-fence`
  - failed at investigation time only on the guarded Speckit pointer mismatch, not on PortfolioSnapshot behavior.

## Acceptance Mapping

- Runtime subscription for `events.portfolio.*`: satisfied by `src/nt_runtime_capture.rs`.
- Stable JSONL path under runtime spool: satisfied by the `portfolio_snapshot/snapshots.jsonl` path in `src/nt_runtime_capture.rs`.
- Persist `PortfolioSnapshot`: satisfied by `CaptureMessage::PortfolioSnapshot` writing to the portfolio snapshot JSONL path.
- Behavior test publishes through NT msgbus and proves artifact row: satisfied by `tests/nt_runtime_capture.rs`.
- `nt-msgbus-surfaces.yaml` updated to `captured_now`: satisfied in `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`.
- `bolt-current-capture.yaml` lists current coverage: satisfied in `docs/bolt-v3/research/runtime-capture/bolt-current-capture.yaml`.
- Runtime-capture verifier enforces PortfolioSnapshot proof: satisfied in `scripts/verify_runtime_capture_yaml.py`.

## Verdict

PortfolioSnapshot-specific source, tests, docs, and runtime-capture verifier acceptance are satisfied at PR #480 head `58f258314aea93e44b5c158766a9ec9d9bd4bfbf`.

Do not close #409 solely from this evidence until PR #480 CI is green, or until the operator explicitly waives the unrelated CI requirement for the #409 slice. The remaining blocker observed during this investigation was source-fence configuration, not PortfolioSnapshot implementation; a later local `just source-fence` rerun passed after restoring the guarded Speckit pointer.
