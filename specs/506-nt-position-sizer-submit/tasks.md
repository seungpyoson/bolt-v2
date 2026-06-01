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

## Remaining For Production Grade

- [ ] Build the live NT-derived sizing-state feed from account, portfolio, position, and open-order state.
- [ ] Wire non-terminal NT order lifecycle and partial-fill events into reservation revalue/residual-liability updates.
- [ ] Release or revalue fully filled orders from authoritative NT fill/order-state evidence.
- [ ] Rebuild pre-existing NT/exchange committed liability into the capital pool before admission can open.
- [ ] Add startup/reconnect reservation rebuild before admission can arm.
- [ ] Add configured halt actions for threshold breach: stop entries, cancel orders, and optional flatten.
- [ ] Document and implement a safe replace-submit model before enabling `ReplaceSubmit`.
- [ ] Replace strategy-local prediction-market outcome derivation with configured YES/NO market metadata.
- [ ] Add maker/post-only quote-set reservation metadata before enabling maker submit enforcement.
- [ ] Add non-binary product calculators before enabling spot leverage, futures/perps, or options.
- [ ] Run external review after the exact PR head is pushed and CI is green.
