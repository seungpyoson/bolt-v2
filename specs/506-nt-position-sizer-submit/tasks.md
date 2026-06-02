# Tasks

## Completed In This Slice

- [x] Add submit-admission position-sizer config and constructor wiring.
- [x] Fail closed when submit sizing is enabled without fresh NT-derived state.
- [x] Reserve before NT submit and roll back uncommitted reservations when the permit is dropped.
- [x] Keep committed reservations live until terminal lifecycle evidence releases them.
- [x] Reject duplicate `client_order_id` reservations.
- [x] Carry explicit venue, product kind, collateral currency, order kind, and prediction-market outcome evidence into submit admission.
- [x] Enforce configured account id against NT-derived portfolio state.
- [x] Refresh bootstrap capital-pool evidence from the NT/Bolt reservation snapshot when sizing state updates.
- [x] Support both YES and NO binary outcome instruments for submit admission and outcome-specific sell inventory checks.
- [x] Subscribe the live runtime to NT terminal order events and release committed submit reservations by client order id.
- [x] Release matching account-less NT `OrderDenied` events by client order id.
- [x] Block uncommitted permit rollback on admission-lock contention instead of skipping the rollback.
- [x] Validate that only one pool can enable submit-admission enforcement.
- [x] Reject submit-admission enforcement with `explicit_clip_to_available`.
- [x] Start configured submit sizing unreconciled and reject entry reservations until explicit open-order rebuild succeeds.
- [x] Add a submit-level open-order reservation rebuild API that atomically rebuilds the reservation ledger and client-order release index.
- [x] Add configured prediction-market binary YES/NO product metadata for submit-enforced capital pools.
- [x] Route direct sizing-state updates through submit-admission composition so caller-supplied reservation evidence is discarded.
- [x] Subscribe the position-sizer runtime feed to NT account, portfolio, order, and position events.
- [x] Map matching NT account/portfolio events into submit-admission component state without letting the feed own the reservation ledger.
- [x] Add live-node startup/reconnect cache rebuild entrypoint that reads only `self.node.kernel().cache()`.
- [x] Keep the sizer unreconciled when cache reports unattributed open orders.
- [x] Seed open-order lifecycle count and configured YES/NO inventory from NT cache.
- [x] Use set semantics for cache and live order events so concurrent/stale snapshots do not double count or resurrect terminal orders.
- [x] Track account-bound submitted/accepted order ids and remove them on terminal events.
- [x] Track submit-time reservation metadata for orders admitted after process start.
- [x] Revalue residual liability from authoritative NT partial-fill events when submit-time metadata exists.
- [x] Release reservations and open-order count from authoritative NT full-fill events when submit-time metadata exists.
- [x] Preserve known Bolt reservation metadata for process-restart open orders admitted after process start.
- [x] Rebuild non-empty pre-existing NT cache open orders into the capital pool only when each order can be attributed to known Bolt reservation metadata.
- [x] Revalue residual liability for rebuilt pre-existing orders from known Bolt reservation metadata and authoritative NT fill events.
- [x] Release fully filled rebuilt pre-existing orders from known Bolt reservation metadata and authoritative NT fill events.
- [x] Invoke NT cache open-order reconciliation from the live runner before submit admission can arm.
- [x] Block live-runner startup before submit admission arming when NT cache reports pre-existing open orders that cannot be reconciled.
- [x] Emit durable position-sizer rebuild audit evidence with source, observation time, attribution status, attempted/recovered counts, acceptance reason, and live reserved liability.

## Remaining For Production Grade

- [ ] Add a safe NT-owned active cancel/flatten control path for loss halts; submit admission and NT trading-state entry/reduce gating are implemented, but working orders and open positions may remain live.
- [ ] Add an operator clear-to-Active surface for manual recovery after a latched loss halt.
- [ ] Document and implement a safe replace-submit model before enabling `ReplaceSubmit`.
- [ ] Replace static configured prediction-market metadata with dynamic market-selection metadata when rotating markets.
- [ ] Add adapter/venue evidence for collateral spendability and venue/instrument allowance.
- [ ] Add maker/post-only quote-set reservation metadata before enabling maker submit enforcement.
- [ ] Add non-binary product calculators before enabling spot leverage, futures/perps, or options.
- [ ] Add reconnect/runtime tests against the actual NT path beyond unit-level cache entrypoint coverage.
- [ ] Run external review after the exact PR head is pushed and CI is green.
