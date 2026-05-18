# Contract: Phase 7 No-submit Readiness

## Purpose

Produce authenticated no-submit readiness evidence without entering the live runner or placing orders.

## Inputs

- Loaded bolt-v3 root TOML.
- `[live_canary]` approval id and no-submit report path.
- Existing SSM-only secret resolver.
- Existing bolt-v3 live-node build path.

## Local Contract

Local readiness tests use fake secret resolution and mock NT clients.

Required guarantees:

1. Build through current bolt-v3 live-node path.
2. Run controlled NT start/reference-cache-readiness/stop only.
3. Write redacted report to configured path.
4. Feed report to live-canary gate.
5. Prove source contains no submit, cancel, replace, amend, or runner-loop call.

## Real Operator Contract

Real readiness runs through the production `bolt-v2` binary.

Required preconditions:

1. Explicit operator approval in current runtime turn.
2. Approved bolt-v3 root TOML path.
3. `[live_canary].approval_id` is present.
4. Report path comes from `[live_canary].no_submit_readiness_report_path`.

Required behavior:

1. Reject missing configured approval before secret resolution.
2. Resolve secrets only through Rust AWS SDK SSM path.
3. Build production-shaped bolt-v3 runtime.
4. Perform controlled NT start/readiness/stop.
5. Place zero orders.
6. Write redacted report.
7. Record `approval_id_hash`, `executable_identity`, and `config_bundle_checksum`.
8. Return failure when any required readiness stage is not satisfied.

## Reference-readiness Stage

The `reference_readiness` stage passes only when every configured `[reference_data.*]` instrument required by every loaded strategy is present in NT cache after controlled NT start. Missing configured Chainlink or exchange reference instruments, wrong market or instrument, auth failure, geo block, cache-population timeout, and stop failure all fail closed.

Phase 7 does not prove live price-stream freshness for Chainlink or exchange references because it never enters the NT runner loop. Feed/source freshness, realized market-data values, and strategy-input economics remain Phase 8 safety-audit scope before any live-capital action. Phase 7 must not implement an alternate market-data cache, direct provider read path, or reference simulator to satisfy this stage.

## Out of Scope

- Live order.
- Soak.
- Runner loop.
- Strategy-driven submit.
- Manual cancel or reconciliation implementation.
- Alternate readiness framework.
- Stale PR #319 runtime wrapper.
