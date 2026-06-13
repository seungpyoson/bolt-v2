# Data Model: Global Shadow Execution Policy

## OrderExecutionMode

Root-TOML runtime mode for venue mutation.

Values:

- `live`: admitted Bolt strategy actions may call NT venue mutation APIs through the shared execution module.
- `shadow`: strategies still produce evidence and admission decisions, but shared routing suppresses Bolt-strategy-originated NT venue mutations.

Validation:

- required under root `[runtime]`
- unknown values fail parse or validation
- strategy-local `submit_orders` is rejected

## OrderExecutionPolicy

Runtime object derived from `OrderExecutionMode`.

Responsibilities:

- expose whether venue mutation is allowed
- route submit behavior through live or shadow outcome
- route cancel behavior through live or shadow outcome
- provide the only allowed strategy-to-NT venue mutation chokepoint

Non-responsibilities:

- no order construction
- no order notional calculation
- no venue capability policy
- no strategy state transition policy
- no NT lifecycle replacement

## SubmitContext

Shared NT submit arguments outside the compiled order.

Fields:

- optional `client_id`
- optional `position_id`
- optional `params`

Validation:

- no adapter-specific param keys are introduced by this feature
- strategies may provide already-typed params, but the shared policy does not interpret them

## SubmitRoutingOutcome

Shared result of routing a compiled order through policy and admission.

Values:

- `Submitted`: live mode admitted and called NT submit
- `SkippedByPolicy`: shadow mode admitted or rejected consistently with submit admission, then suppressed NT submit

The Rust outcome names are `BoltV3SubmitRoutingOutcome::Submitted` and `BoltV3SubmitRoutingOutcome::SkippedByPolicy`. Tests must distinguish live submit from shadow skip without reading strategy config.

## CancelRoutingOutcome

Shared result of routing a cancel through policy.

Values:

- `BoltV3CancelRoutingOutcome::Canceled`: live mode called NT cancel
- `BoltV3CancelRoutingOutcome::SkippedByPolicy`: shadow mode suppressed NT cancel

## VenueMutationFence

Source-fence/static rule over production strategy code.

Forbidden direct strategy calls outside `src/bolt_v3_order_execution.rs`:

- `submit_order(...)`
- `submit_order_list(...)`
- `modify_order(...)`
- `cancel_order(...)`
- `cancel_orders(...)`
- `cancel_all_orders(...)`
- `close_position(...)`
- `close_all_positions(...)`

The current implementation slice only needs shared submit and cancel helpers because the current production strategy only calls `submit_order(...)` and `cancel_order(...)`. If a future strategy needs another NT mutation API, the fence must fail until that action is routed through `src/bolt_v3_order_execution.rs` with tests for live and shadow behavior.

## ManagedVenueActionGuard

Config validation rule applied to every loaded strategy when root mode is shadow.

Rejected strategy fields:

- `manage_stop = true`
- `manage_gtd_expiry = true`
- `manage_contingent_orders = true`
- non-empty `external_order_claims`

Reason:

These NT-managed features can mutate the venue outside Bolt's explicit shared routing helpers. Other pinned NT `StrategyConfig` fields are not independent mutation enablers: identity fields only affect IDs, `oms_type` affects position accounting, market-exit tuning fields are only active when `manage_stop` is true, and logging flags do not emit venue commands.
