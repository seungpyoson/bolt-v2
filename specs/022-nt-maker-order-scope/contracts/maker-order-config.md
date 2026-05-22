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

This section is superseded by `specs/023-nt-order-intent-layer/contracts/order-intent-layer.md` for the active order-intent layer.
Freeze, stale-data, and thin-book exits now use the TOML-owned `[parameters.forced_exit_order]` template, separate from normal `[parameters.exit_order]`.
Operators who require immediate flattening configure that forced-exit template as a taker shape such as `market`/`ioc`/`is_post_only=false`.

## GTD Extension Contract

This section is superseded by `specs/023-nt-order-intent-layer/contracts/order-intent-layer.md` for the active order-intent layer.
GTD order templates are valid only when the selected NT order type accepts GTD and the template supplies explicit TOML-owned `expire_time_unix_nanos`.
Reusing `post_only_requote_interval_ms` is not accepted by current evidence.

## Existing Taker Orders

Existing entry:

- buy long limit `fok`, `is_post_only=false`

Existing exit:

- sell long market `ioc`, `is_post_only=false`

## Explicit Non-Contract

This contract does not prove live order placement, production readiness, canary readiness, or repeated live operation.
