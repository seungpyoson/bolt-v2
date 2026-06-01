# NT Position Sizer Submit Admission Slice

## Scope

This slice wires the Bolt-owned capital reservation ledger into the shared submit-admission gate for one configured prediction-market binary capital pool.

The invariant is asset-agnostic: never admit a submit that would reserve more worst-case liability than the configured capital pool can cover after all live commitments resolve against Bolt.

## Current Enforcement Boundary

Submit admission may enable exactly one capital pool with `enforce_submit_admission = true`.

When enabled, submit admission must:

- reject before NT submit if no fresh NT-derived sizing state exists;
- reject duplicate `client_order_id` reservations;
- reject compiled-order evidence that does not match configured venue, account, product kind, or collateral currency;
- reject prediction-market orders whose YES/NO outcome does not match the instrument state;
- reserve liability before NT submit and roll it back if evidence recording or submit handoff fails;
- keep submitted reservations live until terminal lifecycle evidence releases them.
- subscribe the live runtime to NT terminal order events so cancel/reject/expire/deny releases committed reservations by client order id.

Only `prediction_market_binary` is implemented in this slice. The compiled order kind is explicit, but only `Limit` exists in the current sizing interface.

## Production Caveat

This slice is not production-grade by itself. `enforce_submit_admission = true` is not safe for live deployment until the remaining runtime feeds are connected:

- NT account/portfolio state feed updates `NtDerivedSizingState`;
- NT open-order and non-terminal fill lifecycle events revalue live reservations;
- startup and reconnect rebuild reservations from authoritative NT/open-order state before opening admission;
- halt actions cancel or flatten when configured loss/capital thresholds are breached.

Until those feeds exist, this code is a submit-admission integration slice, not a complete live positional sizer.
