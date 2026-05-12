# Quickstart: Thin Bolt-v3 Live Canary Path

This quickstart is for the completed feature path. It is not approval to run live capital.

## Local Verification

```bash
cargo fmt --check
cargo test --test bolt_v3_production_entrypoint
cargo test --test bolt_v3_strategy_registration
cargo test --test bolt_v3_submit_admission
cargo test --test bolt_v3_live_canary_gate
```

## no-mistakes Triage During Issue #780 Soak

```bash
/private/tmp/no-mistakes-soak-bin status
/private/tmp/no-mistakes-soak-bin runs --limit 5
```

Capture:
- repo and branch
- run id
- final status
- final error code
- whether TUI or `runs` showed `error_code`
- whether user-selected ask-user findings resurfaced after a fix
- whether unrelated low/info findings caused continued auto-fixing instead of pause
- daemon log anomalies

## Operator No-submit Readiness

Preconditions:
- exact commit SHA selected
- TOML config checksum recorded
- SSM paths reviewed without printing secret values
- `[live_canary]` approval id and caps configured
- operator approves zero-order readiness run

Expected result:
- real SSM resolution
- real NT venue connect/disconnect
- zero orders
- redacted no-submit readiness report
- PR #305 gate accepts the report

## Tiny-capital Canary

Preconditions:
- all local gates pass
- no-submit readiness report accepted
- submit admission consumes live canary report
- explicit operator approval
- max live order count and notional cap configured in TOML

Expected result:
- at most one NT-submitted order
- venue accept, fill, or reject captured
- strategy-driven cancel if open
- restart reconciliation through NT
- redacted canary evidence artifact
