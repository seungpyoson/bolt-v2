# Contract: Live Canary Gates

## Gate Order

1. Load TOML.
2. Validate config.
3. Resolve SSM secrets.
4. Map provider adapters into NT config.
5. Register NT clients.
6. Register configured strategies.
7. Validate no-submit readiness via `[live_canary]`.
8. Invoke `run_bolt_v3_live_node`.
9. Inside `run_bolt_v3_live_node`, arm submit admission from the validated `BoltV3LiveCanaryGateReport` returned by the existing gate before runtime capture is wired.
10. Before every live submit, persist mandatory decision evidence.
11. After evidence persistence succeeds, consume `BoltV3LiveCanaryGateReport` count/notional bounds through submit admission.
12. Submit through NT only if admission accepts.

## Fail-closed Conditions

- missing `[live_canary]`
- missing approval id
- missing or unsatisfied no-submit readiness report
- stale or oversized readiness report
- missing submit admission state
- unarmed submit admission state
- second attempt to arm submit admission for the same built node
- exhausted live order count
- non-positive proposed notional
- proposed notional above canary cap
- missing mandatory decision evidence
- decision evidence persistence failure
- strategy construction without required reference roles
- unsupported provider, market family, or strategy binding

## Live Proof Requirements

No-submit readiness proof requires explicit operator approval, real SSM resolution, and real NT venue connect/disconnect with zero orders.

Tiny-capital proof requires exact commit SHA, config checksum, time-bound approval id, one-shot approval nonce consumption evidence, cap values, NT submit evidence, venue accept/fill/reject evidence, strategy-driven cancel evidence if order remains open, and restart reconciliation evidence.

## Phase 6 Boundary

Phase 6 does not produce no-submit readiness evidence or tiny-capital live canary evidence. It defines only the submit-time admission boundary that consumes the already validated gate report before NT submit.

The count budget is consumed on admission accept, before NT submit. This prevents retry loops from exceeding the tiny canary budget if NT submit returns an error after admission accepts.

Every order submit candidate consumes the same global admission budget, including entry, exit, and replace-submit paths. Plain cancel requests are not submit candidates and do not consume budget.

Decision evidence is intent evidence. It is not NT submit evidence unless a later NT order event proves submit reached NT.
