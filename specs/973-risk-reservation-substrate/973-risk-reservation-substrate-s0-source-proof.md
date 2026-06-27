# S0 Source-Proof Notes

## Reuse As Single Sources

- NT submission identity/idempotency boundary: Nautilus tracks submission commands and order lookup by `ClientOrderId` in its execution order manager/client surfaces, and live/reconciliation events are keyed by client/venue order IDs. The substrate must bind admitted orders to NT client-order identity; durable admission-token retry/idempotent-create is still a substrate S4 gap, not an S0 order lifecycle rewrite.
- NT order reconciliation: Nautilus order reconciliation is sourced from `OrderStatusReport`, `FillReport`, `ClientOrderId`, `VenueOrderId`, `event_id`, `ts_event`, and `ts_init` in the pinned `nautilus_trader` rev `6be5a50` execution reconciliation code. The substrate must bind to NT reports; S0 does not create a second order lifecycle.
- NT order-state and restart recovery: NT live node startup performs execution-client mass status, order/fill/position reconciliation, cache flushing, and periodic reconciliation/maintenance. Bolt already exposes these knobs through `nautilus.exec_engine`; the substrate successor path must reconcile against NT truth before admitting new risk.
- Event sequence source: NT order events expose `event_id` plus `ts_event`/`ts_init`, and Bolt's consolidated feed emits `nt_order_lifecycle_seed` and `nt_order_event` inputs into `bolt_v3_capital_admission_runtime_feed`. No authoritative monotonic substrate event sequence exists today; S8 must add/store the substrate watermark instead of assuming NT provides one.
- Loss governor: `src/bolt_v3_loss_governor.rs` remains the sole realized-loss accumulator and admission headroom source. The substrate contracts carry governor views; S0 does not add a second loss accumulator.
- Kill switch and safety action routing: `src/bolt_v3_kill_switch*.rs` remains the safety state machine, durable recovery source, and cancel/flatten routing boundary. S0 only defines the `SafetyAction` contract and does not create another cancel/flatten path.
- Reservation ledger: `src/bolt_v3_capital_reservation.rs` remains the reservation lifecycle source. S0 wraps it with one substrate `risk_state_version` and fencing seam instead of forking reservation accounting.
- NT feeds: `src/bolt_v3_capital_admission_runtime_feed.rs` is the existing consolidated NT-derived feed for order, account, portfolio, and position events. S0 reuses that feed surface; later slices connect logic to it.

## Gaps Carried Forward

- Pool ownership and fencing were missing. S0 adds a configured lease authority seam plus authoritative shared-store token validation for durable risk-state/reservation mutations.
- The existing reservation ledger has no single monotonic substrate `risk_state_version`; S0 skeletons that domain over the existing ledger.
- The NT/Bolt feed surface has event IDs and timestamps, but no durable monotonic event-sequence watermark for FR-047 gap detection. This remains a later-slice gap.
- Durable submission-intent persistence, idempotent create semantics, classifier/kernel logic, epoch cutover, and kill-switch proof integration remain stubbed for later slices.

## Phase-1 Lease Authority

Phase 1 names a configured DynamoDB conditional-write lease authority: `backend = "dynamo_db_conditional_write"` with the pool lease dependency supplied by TOML as `dependency_name`. The S0 shared store treats lease loss, authority mismatch, and ambiguous store state as fail-closed.
