# Quickstart: NT Order Intent Layer

> **Historical feature artifact — do not execute as current verification.**
> Current `main`, `AGENTS.md`, `.github/workflows/`, and the `justfile` are
> authoritative.

## Evidence Commands

```bash
git status --short --branch
git rev-parse HEAD
rg -n "pub enum OrderType|pub enum TimeInForce" /Users/spson/.cargo/git/checkouts/nautilus_trader-*/7c2aafb/crates/model/src/enums.rs
rg -n "pub fn (market|limit|stop_market|stop_limit|market_if_touched|limit_if_touched|trailing_stop_market)" /Users/spson/.cargo/git/checkouts/nautilus_trader-*/7c2aafb/crates/common/src/factories/order.rs
rg -n "build_nt_order|check_nt_order_template_config|forced_exit_order" src/bolt_v3_order_intent.rs src/bolt_v3_archetypes/binary_oracle_edge_taker.rs src/strategies/binary_oracle_edge_taker.rs
rg -n "binary_oracle|polymarket|market_family|strategy_archetype|StrategyCore|StrategyId|PositionSide|SubmitContext|submit_order|submit_admission|BoltV3OrderIntentEvidence|Entry|Exit" src/bolt_v3_order_intent.rs
```

## TDD Commands

Run the focused red test first, then implement, then rerun the same test:

```bash
cargo test bolt_v3_archetype_accepts_mixed_maker_taker_order_configs -- --nocapture
cargo test bolt_v3_archetype_accepts_configured_forced_exit_order_template -- --nocapture
cargo test forced_flat_exit_uses_forced_exit_order_when_normal_exit_is_post_only -- --nocapture
cargo test binary_oracle_edge_taker_exit_submit_threads_managed_position_id_to_nt -- --nocapture
cargo test strategy_core_accepts_nt_hedging_oms_type -- --nocapture
cargo test bolt_v3_strategy_oms_type_accepts_nt_variants -- --nocapture
cargo test forced_flat_exit_order_object_preserves_forced_exit_reduce_only_config -- --nocapture
cargo test forced_flat_exit_order_object_uses_configured_forced_exit_template -- --nocapture
cargo test --test bolt_v3_order_intent -- --nocapture
```

After a green focused slice:

```bash
cargo fmt -- --check
git diff --check
cargo test bolt_v3_archetype_accepts_mixed_maker_taker_order_configs -- --nocapture
```

Before completion claims:

```bash
cargo test
no-mistakes status
```

## Proof Boundaries

- Passing config tests prove parsing and validation only.
- Passing strategy construction tests prove Bolt builds the expected NT `OrderAny` only.
- Passing local tests do not prove adapter or live exchange execution.
- no-mistakes output is useful only after confirming it is running for this branch and exact head.
- Live strategy-free or live-submit proof requires explicit approval and exact-head artifacts.
