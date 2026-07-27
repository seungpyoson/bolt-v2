# NT Position Sizer Submit Admission Slice

## Scope

This slice wires the Bolt-owned capital reservation ledger into the shared submit-admission gate for one configured prediction-market binary capital pool.

The invariant is asset-agnostic: never admit a submit that would reserve more worst-case liability than the configured capital pool can cover after all live commitments resolve against Bolt.

## Current Enforcement Boundary

Submit admission may enable exactly one capital pool with `enforce_submit_admission = true`.

When enabled, submit admission must:

- reject before NT submit if no fresh NT-derived sizing state exists;
- compose sizing state only inside submit admission from NT-derived components plus the Bolt-owned reservation ledger;
- start with its reservation ledger unreconciled and reject entry reservations until startup/open-order reconciliation succeeds;
- accept an explicit open-order reservation rebuild from live-node NT cache as the only way to open a clean submit sizer;
- keep startup/reconnect admission closed when cache reports open orders that cannot be attributed to known reservations;
- reject duplicate `client_order_id` reservations;
- reject compiled-order evidence that does not match configured venue, account, product kind, or collateral currency;
- reject prediction-market orders whose YES/NO outcome does not match the instrument state;
- reserve liability before NT submit and roll it back if evidence recording or submit handoff fails;
- block on admission-lock contention when rolling back an uncommitted reservation;
- keep submitted reservations live until terminal lifecycle evidence releases them;
- subscribe the live runtime to NT account, portfolio, position, and order events for the configured capital pool;
- publish account/portfolio/product/order-lifecycle components without allowing the feed to inject reservation-ledger evidence;
- seed order-lifecycle count and configured YES/NO inventory from NT cache at startup/reconnect;
- track account-bound submitted/accepted order ids with set semantics so cache snapshots and live events cannot double count;
- keep stale cache snapshots from resurrecting a client order id after terminal evidence;
- release committed reservations on terminal order events so cancel/reject/expire/deny releases committed reservations by client order id;
- release matching account-less NT `OrderDenied` events by client order id, because NT does not attach an account id to that event type;
- track submit-time reservation metadata for orders admitted after process start;
- revalue residual liability from authoritative NT partial-fill events when submit-time metadata exists;
- release reservations and open-order count from authoritative NT full-fill events when submit-time metadata exists.

Only `prediction_market_binary` is implemented in this slice. The compiled order kind is explicit, but only `Limit` exists in the current sizing interface.

## Production Caveat

This slice is not production-grade by itself. It adds the live NT component feed and startup/reconnect cache rebuild boundary, but `enforce_submit_admission = true` is not safe for full production deployment until the remaining liability and operations gaps are closed:

- the configured venue's pinned adapter attests startup reconciliation completeness. At the current
  pin the Polymarket adapter logs and skips venue open orders and positions it cannot represent and
  still returns a successful mass status, so the reconciled NT open-order set can silently omit live
  liability. Bolt reads only that set and cannot detect the omission, and this slice removed the
  independent venue-truth cross-check that previously read venue open orders directly. This is
  machine-enforced: `ProviderBinding::reconciliation_unmet` lists the open conditions per provider and
  startup validation rejects `enforce_submit_admission = true` while that list is non-empty. Emptying
  it asserts that **every** listed condition is closed, not just this one, and asserts it about the
  NautilusTrader revision pinned at that moment, so a later pin bump requires re-establishing it;
- the pinned adapter reports filled quantity without capping it below what is already known. At the
  current pin `cap_order_reports_to_confirmed_fills` in the adapter's `generate_mass_status` passes a
  zero local-filled floor, while the sibling report path passes `cached_filled.max(tracked_filled)`.
  Only `Confirmed` trades become fill reports, so a matched-but-unconfirmed trade caps an order's
  filled quantity downward with nothing to stop it, overstating remaining quantity in the same
  projection enforced admission consumes. This needs no reconciliation lookback to occur, so it is
  independent of the completeness item above and is listed as its own condition on the same gate;
- the pinned adapter accounts for fills on orders that have matched but are not yet confirmed. At the
  current pin `build_fill_reports_from_trades` discards any trade whose status is not `Confirmed` with
  a bare `continue` that runs before the `filtered` counter -- no fill report, no counter, no log --
  while the two other skips in the same function both count and are reported in its summary log. The
  order itself does remain visible: `GetOrdersParams` carries no status filter, so a matched order is
  still returned by `/data/orders` and maps to `OrderStatus::Filled`. What is not visible is that its
  fills were dropped, and the condition above then caps that order's filled quantity to zero. A mass
  status that discarded settlement-pending fills is therefore indistinguishable from one that had
  none, so the resulting understatement cannot be detected from the report. Closing the zero floor
  does not make this observable, which is why it is listed as its own condition;
- filled positions consume pool liability. Capacity is computed as `max_pool_liability` minus
  `committed_liability` minus live reserved liability (`bolt_v3_capital_reservation.rs`), but every
  production construction of `CapitalPoolSnapshot` sets `committed_liability` to zero and no path ever
  writes another value, so that term is inert; live reserved liability covers open orders only, and BUY
  liability charges the incoming order alone. Exposure that has converted from an order into a position
  is therefore charged by nothing: after a restart holding a cap-sized filled position and no open
  orders, the full pool ceiling reads as available. This is Bolt-side capital accounting rather than an
  adapter reconciliation gap, so it is not one of the `reconciliation_unmet` conditions. Nor is it
  gated like them: nothing under `src/bolt_v3_validate/` reads `committed_liability`, so no startup
  check refuses `enforce_submit_admission = true` on account of it. Unlike every condition above, this
  one is disclosed here and enforced nowhere -- arming enforcement while it stands is a decision a
  reader has to make deliberately, not one validation will refuse;
- residual liability for rebuilt pre-existing orders is attributable to known Bolt reservation metadata;
- non-empty pre-existing NT/exchange open orders can be rebuilt only when their liability is attributable to known Bolt reservations;
- adapter-produced live collateral spendability and venue/instrument allowance evidence exists beyond the optional configured source binding and NT account free-balance fallback;
- safe replace/amend transitions are implemented before `ReplaceSubmit` is enabled;
- maker quote sets reserve simultaneous adverse fills;
- operator clear-to-Active recovery and flat-position proof are implemented for configured loss halts.

Until those items exist, this code is a submit-admission and live-state-feed slice, not a complete production-grade positional sizer.
