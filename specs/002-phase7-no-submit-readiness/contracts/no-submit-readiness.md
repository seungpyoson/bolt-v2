# Contract: Phase 7 No-submit Readiness

## Purpose

Produce authenticated no-submit readiness evidence without entering the live runner or placing orders.

## Inputs

- Loaded bolt-v3 root TOML.
- `[live_canary]` approval id and no-submit report path.
- Operator approval id for real SSM/venue readiness.
- Existing SSM-only secret resolver.
- Existing bolt-v3 live-node build path.

## Local Contract

Local readiness tests use fake secret resolution and mock NT clients.

Required guarantees:

1. Build through current bolt-v3 live-node path.
2. Run controlled connect and controlled disconnect only.
3. Write redacted report to configured path.
4. Feed report to live-canary gate.
5. Prove source contains no submit, cancel, replace, amend, subscribe, or runner-loop call.

## Real Operator Contract

Real readiness harness is ignored by default.

Required preconditions:

1. Explicit operator approval in current runtime turn.
2. Exact head SHA recorded.
3. Approved bolt-v3 root TOML checksum recorded.
4. Approval id matches `[live_canary].approval_id`.
5. Report path comes from `[live_canary].no_submit_readiness_report_path`.

Required behavior:

1. Reject missing or mismatched approval before secret resolution.
2. Resolve secrets only through Rust AWS SDK SSM path.
3. Build production-shaped bolt-v3 runtime.
4. Perform controlled connect/readiness/disconnect.
5. Place zero orders.
6. Write redacted report.
7. Return failure when any required readiness stage is not satisfied.

## Out of Scope

- Live order.
- Soak.
- Runner loop.
- Strategy-driven submit.
- Manual cancel or reconciliation implementation.
- Alternate readiness framework.
- Stale PR #319 runtime wrapper.
