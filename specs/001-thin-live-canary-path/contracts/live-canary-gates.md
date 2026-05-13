# Contract: Live Canary Gates

## Gate Order

1. Load TOML.
2. Validate config.
3. Resolve SSM secrets.
4. Map provider adapters into NT config.
5. Register NT clients.
6. Register configured strategies.
7. Validate no-submit readiness via `[live_canary]`.
8. Enter NT runner through `run_bolt_v3_live_node`.
9. Before every live submit, consume `BoltV3LiveCanaryGateReport` through submit admission.
10. Submit through NT only if admission accepts.

## Fail-closed Conditions

- missing `[live_canary]`
- missing approval id
- missing or unsatisfied no-submit readiness report
- stale or oversized readiness report
- missing submit admission state
- exhausted live order count
- proposed notional above canary cap
- missing mandatory decision evidence
- decision evidence persistence failure
- strategy construction without required reference roles
- unsupported provider, market family, or strategy binding

## Live Proof Requirements

No-submit readiness proof requires explicit operator approval, real SSM resolution, and real NT venue connect/disconnect with zero orders.

Tiny-capital proof requires exact commit SHA, config checksum, approval id, cap values, NT submit evidence, venue accept/fill/reject evidence, strategy-driven cancel evidence if order remains open, and restart reconciliation evidence.
