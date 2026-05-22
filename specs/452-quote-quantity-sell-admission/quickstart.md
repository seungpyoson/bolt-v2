# Quickstart: Quote-Quantity SELL Limit Admission

## Pre-Implementation Gates

1. Confirm branch and head:

```bash
git status --short --branch
git rev-parse HEAD
```

2. Review planning artifacts:

```bash
sed -n '1,220p' specs/452-quote-quantity-sell-admission/spec.md
sed -n '1,260p' specs/452-quote-quantity-sell-admission/research.md
sed -n '1,220p' specs/452-quote-quantity-sell-admission/tasks.md
```

3. Get external source-proven plan review before implementation.

## TDD Loop

Run every Phase 2 strategy-level red confirmation before production edits:

```bash
cargo test quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_missing_context_fails_closed -- --nocapture
cargo test quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_missing_context_fails_closed -- --nocapture
```

Then implement one minimal green behavior and rerun its focused strategy and helper tests.

## Required Verification After Implementation

```bash
cargo test quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_helper_floors_to_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_helper_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_limit_missing_context_fails_closed -- --nocapture
cargo test quote_quantity_sell_limit_helper_missing_context_fails_closed -- --nocapture
cargo test quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_helper_floors_to_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_helper_missing_quote_uses_submitted_quote_quantity -- --nocapture
cargo test quote_quantity_sell_stop_limit_missing_context_fails_closed -- --nocapture
cargo test quote_quantity_sell_stop_limit_helper_missing_context_fails_closed -- --nocapture
cargo test quote_quantity_inverse_sell_limit_preserves_nt_notional -- --nocapture
cargo test quote_quantity_inverse_sell_stop_limit_preserves_nt_notional -- --nocapture
cargo test quote_quantity_inverse_sell_limit_strategy_preserves_nt_notional -- --nocapture
cargo test quote_quantity_inverse_sell_stop_limit_strategy_preserves_nt_notional -- --nocapture
cargo test quote_quantity_submit_admission_helper_source_fence -- --nocapture
cargo test quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy -- --nocapture
cargo test quote_quantity_submit_admission_uses_limit_price_when_nt_cache_quote_missing -- --nocapture
cargo test quote_quantity_market_submit_admission_uses_nt_cache_quote_ask -- --nocapture
cargo test quote_quantity_market_submit_admission_uses_nt_cache_trade_when_quote_missing -- --nocapture
cargo test exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used -- --nocapture
cargo test exit_quote_quantity_order_build_is_rejected_before_nt_factory -- --nocapture
cargo test configured_short_position_contract_is_rejected_until_short_economics_exists -- --nocapture
cargo fmt -- --check
just clippy
```

Run broader Rust verification before PR:

```bash
cargo test --locked
```

## Proof Boundary

Passing local tests proves source/unit admission behavior only. It does not prove live exchange execution, short-side strategy economics, quote-sized exit enablement, or #451 generic wrapper completion.
