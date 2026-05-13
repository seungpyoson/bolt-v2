# Quickstart: Thin Bolt-v3 Live Canary Path

This quickstart is for the completed feature path. It is not approval to run live capital.

## Local Verification

For Phase 6, run only after the Phase 6 implementation branch exists and the first red test has been captured. Do not run live capital from this quickstart.

```bash
cargo fmt --check
cargo test --test bolt_v3_submit_admission
cargo test --test bolt_v3_decision_evidence
cargo test --test bolt_v3_strategy_registration
cargo test --test bolt_v3_live_canary_gate
git diff --check
python3 scripts/verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_provider_leaks.py
python3 scripts/verify_bolt_v3_naming.py
python3 scripts/verify_bolt_v3_core_boundary.py
```

Phase 6 green criteria:
- missing or unarmed gate report rejects before NT submit with a distinct diagnostic
- exhausted count cap rejects before NT submit
- over notional cap rejects before NT submit, while notional equal to the cap admits
- decision evidence failure rejects before admission budget consumption
- valid submit path orders as decision evidence write, submit admission, NT submit
- entry, exit, and replace-submit candidates consume one global budget
- plain cancel requests do not consume submit admission budget
- double-arm and stale-arm behavior are defined and covered
- runtime capture around `run_bolt_v3_live_node` is preserved
- decision evidence alone is not NT submit proof; live proof must use NT order events
- restart resets Phase 6 in-memory admission budget, so Phase 8 operator procedure must not treat restart as budget preservation

## no-mistakes Triage During Issue #780 Soak

Use the active no-mistakes binary for the environment. If an issue-specific soak binary is active, set `NO_MISTAKES_BIN` to the operator-provided path outside this repo.

```bash
"${NO_MISTAKES_BIN:-no-mistakes}" status
"${NO_MISTAKES_BIN:-no-mistakes}" runs --limit 5
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
