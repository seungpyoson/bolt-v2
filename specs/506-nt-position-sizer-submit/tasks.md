# Tasks

## Completed In This Slice

- [x] Add submit-admission position-sizer config and constructor wiring.
- [x] Fail closed when submit sizing is enabled without fresh NT-derived state.
- [x] Reserve before NT submit and roll back uncommitted reservations when the permit is dropped.
- [x] Keep committed reservations live until terminal lifecycle evidence releases them.
- [x] Reject duplicate `client_order_id` reservations.
- [x] Carry explicit venue, product kind, collateral currency, order kind, and prediction-market outcome evidence into submit admission.
- [x] Validate that only one pool can enable submit-admission enforcement.
- [x] Reject submit-admission enforcement with `explicit_clip_to_available`.

## Remaining For Production Grade

- [ ] Build the live NT-derived sizing-state feed from account, portfolio, position, and open-order state.
- [ ] Wire NT order lifecycle events into reservation revalue/release updates.
- [ ] Add startup/reconnect reservation rebuild before admission can arm.
- [ ] Add configured halt actions for threshold breach: stop entries, cancel orders, and optional flatten.
- [ ] Replace strategy-local prediction-market outcome derivation with configured YES/NO market metadata.
- [ ] Add non-binary product calculators before enabling spot leverage, futures/perps, or options.
- [ ] Run external review after the exact PR head is pushed and CI is green.

