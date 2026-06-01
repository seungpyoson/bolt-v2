# T038 Operator Config Snapshot Port Audit

Date: 2026-05-25
Scope: read-only audit of `origin/t038-operator-config-snapshot` against current `main` / PR #480 no-submit readiness code.

No source or docs were edited during the investigation. No live, no-submit, AWS, SSM, trading, submit, cancel, replace, transfer, or deployment command was run.

## Commands And Evidence

- `gh pr view 480 --json number,title,headRefName,headRefOid,baseRefName,baseRefOid,state,url`
  - PR #480 head: `58f258314aea93e44b5c158766a9ec9d9bd4bfbf`
  - base `main`: `3a444a57cfdcdc31d58cbfe8d22857eb86f8bad9`
- `git show-ref --verify refs/remotes/origin/t038-operator-config-snapshot`
  - old branch head: `53c43608e74d7d8293c8830f57ed180d94bb7c5a`
- `git log --oneline origin/main..origin/t038-operator-config-snapshot`
  - unique old commits: `bced44fe`, `48201c32`, `36a50aa1`, `b7c4d419`, `f6e3dcc8`, `33f6c738`, `2849fb73`, `53c43608`
- `git cherry -v origin/main origin/t038-operator-config-snapshot`
  - all eight commits were patch-unique, so behavior-level comparison was required.
- `git diff --quiet origin/main...HEAD -- src/bolt_v3_live_node.rs src/bolt_v3_no_submit_readiness.rs tests/bolt_v3_no_submit_readiness.rs tests/bolt_v3_controlled_connect.rs src/bolt_v3_providers/binance.rs Cargo.toml config/root.toml`
  - exit `0`, meaning PR #480 does not alter the relevant current no-submit/SBE code from `main`.
- Targeted inspection covered:
  - `src/bolt_v3_live_node.rs`
  - `src/bolt_v3_no_submit_readiness.rs`
  - `tests/bolt_v3_no_submit_readiness.rs`
  - `tests/config_parsing.rs`
  - `config/root.toml`
  - `specs/001-thin-live-canary-path/tasks.md`

## Commit-Level Disposition

- `bced44fe` operator snapshot: historical operator snapshot evidence only; not a current behavior patch to port.
- `48201c32` build-before-async-readiness: superseded by current tests requiring `build_bolt_v3_no_submit_live_node(loaded)` before `LocalSet` / readiness runtime creation.
- `36a50aa1` clients-only no-submit construction and strict connect path: superseded by current strategy-free helper and later runner-loop readiness path.
- `b7c4d419` old live config/client vocabulary alignment: superseded by the current `[clients.*]` schema and later config/name work.
- `f6e3dcc8` Binance SBE/NT pin: superseded by current NT rev `7c2aafb30fb143069c915a3f2057bb12174405f6`, SBE endpoint config, and tests rejecting Binance JSON WS endpoints.
- `33f6c738`, `2849fb73`, `53c43608` NT start/event-pump/client-registration sequence: superseded by current no-submit readiness code using `node.run()`, runner polling while waiting for reference quotes, `LiveNodeHandle` stop, execution-account evidence checks, and reference-quote evidence checks.

## Verdict

Missing behavior to port: **none**.

The old branch should remain reference-only. Do not port it wholesale into PR #480. Historical T038 EC2/EIP no-submit evidence remains useful as issue context, but final-packet T131/T122 no-submit proof must still be rerun after the verified final packet exists.
