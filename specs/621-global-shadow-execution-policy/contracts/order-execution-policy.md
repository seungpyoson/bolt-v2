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

## Shared Submit Contract

Inputs:

- shared `OrderExecutionPolicy`
- shared decision evidence writer
- shared submit admission state
- compiled `BoltV3OrderIntentEvidence`
- compiled `BoltV3SubmitAdmissionRequest`
- `SubmitContext`
- closure that performs the NT `submit_order(...)` call

Live behavior:

1. Record order intent evidence.
2. Call submit admission `admit(...)`.
3. If admission rejects, return the existing admission error and do not call NT submit.
4. If admission admits, consume capacity and call the NT submit closure exactly once.
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
- closure that performs the NT `cancel_order(...)` call

Live behavior:

1. Call the NT cancel closure exactly once.
2. Return canceled outcome.

Shadow behavior:

1. Do not call the NT cancel closure.
2. Return skipped-by-policy outcome.

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
