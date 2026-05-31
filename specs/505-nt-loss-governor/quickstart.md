# Quickstart: NT-First Loss Governor

## Focused Verification

```bash
cargo test --locked --lib bolt_v3_loss_governor -- --nocapture
cargo test --locked --test bolt_v3_submit_admission loss_governor -- --nocapture
cargo test --locked --test config_parsing loss_governor -- --nocapture
```

## Full Verification

```bash
cargo fmt --check
cargo test --locked --lib
cargo test --locked --test bolt_v3_submit_admission
cargo test --locked --test config_parsing
cargo test --locked --test bolt_v3_decision_evidence
git diff --check
```

## Scope Check

```bash
git diff --name-only -- src/strategies/binary_oracle_edge_taker.rs
```

Expected output is empty for this slice.

## Proof Boundary

Passing tests prove pure policy behavior, config binding, live NT event-feed snapshot derivation, and submit-admission rejection before NT submit for new entry/replace risk. They do not prove cancel/flatten behavior or explicit NT RiskEngine trading-state transitions.
