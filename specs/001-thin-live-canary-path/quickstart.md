# Quickstart: Thin Bolt-v3 Live Canary Path

This quickstart is for the completed feature path. It is not approval to run live capital.

## Local Verification

For Phase 1, only docs/spec artifacts exist. The `cargo test --test ...` commands below require their later implementation phases to exist.

```bash
cargo fmt --check
cargo test --test bolt_v3_production_entrypoint
cargo test --test bolt_v3_strategy_registration
cargo test --test bolt_v3_submit_admission
cargo test --test bolt_v3_live_canary_gate
```

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

Root TOML preflight:
- use an approved bolt-v3 root TOML, not the legacy `config/live.local.toml` shape
- require bolt-v3 root sections such as `[runtime]`, `[nautilus]`, `[risk]`, `[aws]`, `[venues.*]`, `[persistence.*]`, and `[live_canary]`
- reject legacy operator config shapes with `[node]`, `[polymarket]`, `[reference.*]`, `[[rulesets]]`, or `[[strategies]]` as T037 inputs
- do not derive the approval id or report path from example or fixture files

Shape-only preflight commands, without printing secret values:

```bash
test -f "$BOLT_V3_ROOT_TOML"
rg -q '^\[runtime\]' "$BOLT_V3_ROOT_TOML"
rg -q '^\[nautilus\]' "$BOLT_V3_ROOT_TOML"
rg -q '^\[risk\]' "$BOLT_V3_ROOT_TOML"
rg -q '^\[aws\]' "$BOLT_V3_ROOT_TOML"
rg -q '^\[persistence\]' "$BOLT_V3_ROOT_TOML"
rg -q '^\[live_canary\]' "$BOLT_V3_ROOT_TOML"
rg -q '^no_submit_readiness_report_path[[:space:]]*=' "$BOLT_V3_ROOT_TOML"
! rg -q '^\[node\]|^\[polymarket\]|^\[reference|^\[\[rulesets\]\]|^\[\[strategies\]\]' "$BOLT_V3_ROOT_TOML"
```

Command shape:

```bash
BOLT_V3_ROOT_TOML=/absolute/path/to/approved-root.toml \
BOLT_V3_OPERATOR_APPROVAL_ID='<approval id matching [live_canary].approval_id>' \
cargo test --test bolt_v3_no_submit_readiness_operator \
  operator_approved_real_no_submit_readiness_writes_redacted_report \
  -- --ignored --nocapture
```

Expected result:
- real SSM resolution
- real NT venue connect/disconnect
- zero orders
- redacted no-submit readiness report
- PR #305 gate accepts the report

Evidence to capture before marking T037 complete:
- exact git SHA of the code being run
- absolute path to the approved root TOML, without printing secret values
- checksum of the approved root TOML
- `[live_canary].approval_id`, `max_live_order_count`, and `max_notional_per_order`
- configured `no_submit_readiness_report_path`
- test exit status
- printed redacted report path
- confirmation that every report stage is `satisfied`
- confirmation that the live canary gate accepted the same report

Do not accept as T037 proof:
- local mock connect/disconnect tests
- a report generated from fake SSM or fake venue clients
- a report with failed, skipped, missing, stale, or manually edited stages
- logs that expose resolved secrets, private keys, or raw credential values

Failure handling:
- connect or disconnect failure blocks tiny-capital submit
- missing, stale, or unsatisfied report blocks tiny-capital submit
- venue or NT adapter capability gaps are recorded as blockers, not worked around with Bolt-owned lifecycle or adapter code

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
