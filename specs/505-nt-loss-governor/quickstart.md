# Quickstart: NT-First Loss Governor

## Focused Verification

```bash
cargo test --locked --lib bolt_v3_loss_governor -- --nocapture
cargo test --locked --test config_parsing loss_governor -- --nocapture
cargo test --locked --test bolt_v3_loss_runtime_feed -- --nocapture
cargo test --locked --test bolt_v3_strategy_registration nt_market_exit_strategy_reaches_running_stub_strategy_hook -- --nocapture
```

## Full Verification

```bash
cargo fmt --check
cargo test --locked --lib
cargo test --locked --test config_parsing
cargo test --locked --test bolt_v3_loss_runtime_feed
cargo test --locked --test bolt_v3_submit_admission
git diff --check
just source-fence
```

## Scope Check

```bash
git diff --name-only -- src/strategies/binary_oracle_edge_taker.rs
```

Expected output is empty for this slice.

## Proof Boundary

Passing tests in PR #507 and the market-exit follow-on prove pure policy behavior, config binding, NT-derived sizing-state validation, worst-case binary liability sizing, capital-reservation reserve/release/rebuild/revalue behavior, pure already-attributed lifecycle update dispatch, configured submit-admission loss rejection before NT submit, configured NT event-feed snapshot derivation, configured NT `RiskEngine::set_trading_state` halt actions, and configured NT `Trader::market_exit_strategy` dispatch to registered strategies. They do not prove the account is flat, implement the operator clear-to-Active command surface, or close the remaining production-grade position-sizer gaps.
