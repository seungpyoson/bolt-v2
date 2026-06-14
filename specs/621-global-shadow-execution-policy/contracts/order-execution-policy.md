# Contract: Order Execution Policy

## Config Contract

Root config owns the execution mode.

```toml
[runtime]
mode = "Live"
order_execution_mode = "shadow"
```

Valid values:

- `live`
- `shadow`

Invalid or missing values fail config loading. A `submit_orders` key under any strategy `[parameters]` block fails archetype parameter deserialization.

## Strategy-Originated Venue Mutation Surface

All Bolt strategy-originated NT venue mutation APIs must route through `src/bolt_v3_order_execution.rs`.

The pinned NT strategy surface includes these strategy-callable mutation methods:

- `submit_order(...)`
- `submit_order_list(...)`
- `modify_order(...)`
- `cancel_order(...)`
- `cancel_orders(...)`
- `cancel_all_orders(...)`
- `close_position(...)`
- `close_all_positions(...)`

The current implementation slice provides explicit submit and cancel routing because current production code only uses `submit_order(...)` and `cancel_order(...)`. A source-fence/static verifier must reject direct calls to any listed method from production source outside `src/bolt_v3_order_execution.rs`; it also fences private raw-adapter wrapper names and near-neighbor parameterized/in-place mutation variants. Adding support for another method requires adding a shared helper and live/shadow tests before any strategy may call it.

## Shared Submit Contract

Inputs:

- shared `OrderExecutionPolicy`
- shared decision evidence writer
- shared submit admission state
- compiled `BoltV3OrderIntentEvidence`
- compiled `BoltV3SubmitAdmissionRequest`
- `SubmitContext`
- mutable NT `Strategy` reference; the shared module constructs a module-private
  adapter for the final NT `submit_order(...)` call

Live behavior:

1. Record order intent evidence.
2. Call submit admission `admit(...)`.
3. If admission rejects, return the existing admission error and do not call NT submit.
4. If admission admits, consume capacity and call NT submit through the private adapter exactly once.
5. Return submitted outcome.

Shadow behavior:

1. Record order intent evidence.
2. Call submit admission `evaluate_and_record_without_consuming_capacity(...)`.
3. If admission rejects, return the existing admission error and do not call NT submit.
4. If admission admits, do not consume live capacity and do not call NT submit.
5. Return skipped-by-policy outcome.

## Shared Cancel Contract

Inputs:

- shared `OrderExecutionPolicy`
- mutable NT `Strategy` reference; the shared module constructs a module-private
  adapter for the final NT `cancel_order(...)` call

Live behavior:

1. Call NT cancel through the private adapter exactly once.
2. Return canceled outcome.

Shadow behavior:

1. Do not call NT cancel.
2. Return skipped-by-policy outcome.

## Source-Fence Contract

Verification must fail if production source outside `src/bolt_v3_order_execution.rs`
directly calls any listed NT venue mutation method or private raw-adapter method.
Strategy source must also fail if it constructs or overrides execution policy locally
instead of using `StrategyBuildContext`.

## Boundary Contract

The shared execution module may depend on:

- `bolt_v3_decision_evidence`
- `bolt_v3_submit_admission`
- NautilusTrader model/common types needed for submit context

The shared execution module must not depend on:

- `src/strategies/*`
- `bolt_v3_archetypes::*`
- provider modules
- market-family modules
- venue-specific adapters

`src/bolt_v3_order_intent.rs` must remain free of execution mode, submit admission, and cancellation policy.
