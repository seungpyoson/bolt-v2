# Contract: Maker Order Config

## Entry Maker Order

Accepted TOML shape:

```toml
[parameters.entry_order]
side = "buy"
position_side = "long"
order_type = "limit"
time_in_force = "gtc"
is_post_only = true
is_reduce_only = false
is_quote_quantity = false
```

Expected NT order:

- `OrderType::Limit`
- `TimeInForce::Gtc`
- `is_post_only=true`
- `expire_time=None` for `gtc`

## Exit Maker Order

Accepted TOML shape:

```toml
[parameters.exit_order]
side = "sell"
position_side = "long"
order_type = "limit"
time_in_force = "gtc"
is_post_only = true
is_reduce_only = false
is_quote_quantity = false
```

Expected NT order:

- `OrderType::Limit`
- `TimeInForce::Gtc`
- `is_post_only=true`
- `expire_time=None` for `gtc`
- passive at the touch; not an immediate forced-flat guarantee

## Forced-Flat Semantics

If `[parameters.exit_order]` is configured as maker `limit`/`gtc`/`is_post_only=true`, freeze, stale-data, and thin-book exits use the same configured maker exit shape. The order may rest unfilled. Operators who require immediate flattening must configure the taker exit shape (`market`/`ioc`/`is_post_only=false`) until a separate TOML-owned forced-exit override exists.

## GTD Extension Contract

NT supports post-only `Gtd` limit orders, but bolt-v3 must not enable `gtd` until this contract adds an explicit TOML-owned expiry policy. Reusing `post_only_requote_interval_ms` is not accepted by current evidence.

## Existing Taker Orders

Existing entry:

- buy long limit `fok`, `is_post_only=false`

Existing exit:

- sell long market `ioc`, `is_post_only=false`

## Explicit Non-Contract

This contract does not prove live order placement, production readiness, canary readiness, or repeated live operation.
