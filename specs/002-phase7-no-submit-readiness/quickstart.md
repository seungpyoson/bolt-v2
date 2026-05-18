# Phase 7 Quickstart

## Local-only Verification

Expected default path after implementation:

```bash
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
cargo test --test bolt_v3_cli bolt_v3_cli_exposes_no_submit_readiness_operator_command -- --nocapture
cargo test --test bolt_v3_live_canary_gate -- --nocapture
cargo fmt --check
git diff --check
```

Expected behavior:

- Local readiness tests use fake secret resolution and mock NT clients.
- Operator no-submit readiness is exposed by the production `bolt-v2 no-submit-readiness --config <path>` command.
- No SSM, venue, live capital, or soak action occurs.
- Report fixture is accepted by live-canary gate.

## Real No-submit Readiness

Do not run without explicit operator approval in current thread.

Required proof before approved run:

- Approved bolt-v3 root TOML path.
- `[live_canary]` approval id present.
- `[live_canary].no_submit_readiness_report_path` present.
- Empty or segregated live account approved for read-only startup reconciliation.

Approved command shape:

```bash
bolt-v2 no-submit-readiness --config '<approved bolt-v3 root toml path>'
```

Post-run proof:

- Command exit status.
- Report `executable_identity`.
- Report `config_bundle_checksum`.
- Redacted report path.
- Live-canary gate acceptance when `bolt-v2 run --config '<approved bolt-v3 root toml path>'` starts.

Reference-readiness rule: do not treat controlled-connect success as reference readiness. A real report can satisfy the gate only when controlled NT start populates NT cache with every `[reference_data.*]` instrument required by loaded strategies before the bounded timeout, then controlled stop succeeds.

Freshness scope: Phase 7 proves configured reference instruments are reachable through NT cache after authenticated startup. It does not prove live Chainlink or exchange price-stream freshness because `LiveNode::run()` is not entered. Freshness and strategy-input economics remain Phase 8 safety-audit scope before any live-capital action.

## Phase 8 Boundary

Phase 8 live action remains blocked until:

- Real no-submit report exists.
- Report is accepted by live-canary gate.
- `binary_oracle_edge_taker` strategy-input safety audit approves Chainlink feed path, reference venues, market selection, volatility, kurtosis, theta, fee/slippage model, caps, and edge economics.
- User explicitly approves exact head and live command.
