# Data Model: NT-Matched Maker Order Scope

## OrderParams

Source: `[parameters.entry_order]` and `[parameters.exit_order]`.

Fields:

- `side`: NT `OrderSide`
- `position_side`: NT `PositionSide`
- `order_type`: NT `OrderType`
- `time_in_force`: NT `TimeInForce`
- `is_post_only`: bool
- `is_reduce_only`: bool
- `is_quote_quantity`: bool

Validation:

- Entry maker: buy long limit with `Gtc`, `is_post_only=true`, no reduce-only or quote-quantity.
- Exit maker: sell long limit with `Gtc`, `is_post_only=true`, no reduce-only or quote-quantity.
- Existing taker entry/exit remains unchanged.

## MakerOrderScope

Represents NT-proven maker capability for this slice.

Allowed values:

- `limit + gtc + post_only`

Rejected values:

- market + post-only
- limit + post-only + `fok`/`ioc`
- limit + post-only + `gtd` until a TOML-owned expiry policy is approved
- quote quantity or reduce-only in this strategy slice

## GtdExpiryPolicy

Maps TOML-owned timing into NT `expire_time`. This policy is not implemented and is outside the approved current scope.

Input:

- NEEDS APPROVED DESIGN: explicit TOML expiry field or equivalent config-owned value
- current NT strategy clock timestamp, if relative expiry is approved

Output:

- NT `expire_time` for `TimeInForce::Gtd`

Constraint:

- Overflow must fail before submit.
- `post_only_requote_interval_ms` must not be reused as GTD expiry without explicit review approval.

## ReviewGate

Tracks required gates.

Fields:

- gate name
- provider or agent
- source scope
- verdict
- findings
- block reason when unavailable
- command evidence
