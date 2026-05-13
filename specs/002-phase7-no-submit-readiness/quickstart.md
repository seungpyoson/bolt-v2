# Phase 7 Quickstart

## Local-only Verification

Expected default path after implementation:

```bash
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
cargo test --test bolt_v3_no_submit_readiness_operator -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
cargo fmt --check
git diff --check
```

Expected behavior:

- Local readiness tests use fake secret resolution and mock NT clients.
- Operator test is ignored by default.
- No SSM, venue, live capital, or soak action occurs.
- Report fixture is accepted by live-canary gate.

## Real No-submit Readiness

Do not run without explicit operator approval in current thread.

Required proof before approved run:

- Exact head SHA.
- Approved bolt-v3 root TOML path.
- Root TOML checksum.
- `[live_canary]` approval id present.
- `[live_canary].no_submit_readiness_report_path` present.
- Operator approval id matches config.

Approved command shape:

```bash
BOLT_V3_ROOT_TOML='<approved bolt-v3 root toml path>' \
BOLT_V3_OPERATOR_APPROVAL_ID='<approval id matching [live_canary].approval_id>' \
cargo test --test bolt_v3_no_submit_readiness_operator \
  operator_approved_real_no_submit_readiness_writes_redacted_report \
  -- --ignored --nocapture
```

Post-run proof:

- Command exit status.
- Exact head SHA.
- Root TOML checksum.
- Redacted report path.
- Live-canary gate acceptance of report.

## Phase 8 Boundary

Phase 8 live action remains blocked until:

- Real no-submit report exists.
- Report is accepted by live-canary gate.
- `eth_chainlink_taker` strategy-input safety audit approves Chainlink feed path, reference venues, market selection, volatility, kurtosis, theta, fee/slippage model, caps, and edge economics.
- User explicitly approves exact head and live command.
