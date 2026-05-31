# Quickstart: NT-First Loss Governor

## Focused Verification

```bash
cargo test --locked --lib bolt_v3_loss_governor -- --nocapture
cargo test --locked --test config_parsing loss_governor -- --nocapture
```

## Full Verification

```bash
cargo fmt --check
cargo test --locked --lib
cargo test --locked --test config_parsing
git diff --check
just source-fence
```

## Scope Check

```bash
git diff --name-only -- src/strategies/binary_oracle_edge_taker.rs
```

Expected output is empty for this slice.

## Proof Boundary

Passing tests in PR #507 prove pure policy behavior, config binding, NT-derived sizing-state validation, worst-case binary liability sizing, capital-reservation behavior, and configured submit-admission loss rejection before NT submit. They do not prove live NT event-feed snapshot derivation, positional-sizer live-path enforcement, cancel/flatten behavior, or explicit NT RiskEngine trading-state transitions.
