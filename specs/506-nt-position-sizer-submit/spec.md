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
  pin the Polymarket adapter drops venue open orders and positions it cannot represent and still
  returns a successful mass status, so the reconciled NT open-order set can silently omit live
  liability. The drops are not uniformly reported: an unmappable open order increments a counter but
  logs nothing (`build_order_reports_from_orders`), an unrepresentable position logs a warning but
  increments no counter, its builder having none, and no counter reaches `ExecutionMassStatus` at
  all -- the totals surface only in a `log::debug!` line. Bolt reads only that set and cannot detect
  the omission, and this slice removed the
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
  a bare `continue`, before the `filtered` counter and with no log entry, so no `FillReport` is
  produced. Of the six `continue` statements in that function two increment `filtered`, for an
  unmappable instrument, and four are silent. This condition's own discard is one of the four; the
  other three are harmless in this configuration, selecting a counterparty's maker entry or firing
  only under a scoped instrument filter that the mandated empty reconciliation filter never sets.
  Closing the condition above does **not** close this one; it changes how it fails. The order carries
  `size_matched` as `filled_qty`, so once the zero floor stops erasing it,
  `generate_external_order_status_events` synthesizes an inferred fill and the quantity is recovered.
  But that fill is identified by `create_inferred_reconciliation_trade_id`, derived from the order's
  own fields, rather than by the venue's trade id -- which the adapter never supplied, having
  discarded the trade. NT deduplicates fills by trade id
  (`crates/execution/src/reconciliation/orders.rs`), so when the venue later reports that same trade
  as `Confirmed`, it does not match the inferred fill and is applied on top of it, and the recovered
  quantity can be counted twice. The trade's execution price and fee are lost either way, since an
  inferred fill reconstructs neither. This condition is listed on its own because it survives the
  condition above being closed, in the opposite direction;
- the pinned NautilusTrader engine keeps every position report the adapter produced. It does not, and
  this one is not an adapter defect: `reconcile_position_report_netting` in
  `crates/live/src/execution/manager.rs` resolves the instrument with `self.get_instrument(&id)?`, so
  a report whose instrument never loaded into the cache returns `None`, and the caller consumes it
  with a bare `if let Some(...)` -- no else branch, no counter, no log at any level. The adapter's own
  position builder never checks instrument availability, so it faithfully produces the report and the
  engine discards it. This matters for the gate's premise rather than for one venue: it survives every
  adapter condition above being closed, so a list emptied on adapter fixes alone would attest
  completeness that the engine still does not provide. A restart holding a position in a market whose
  instrument is absent -- an expired or unlisted one -- understates liability with no signal;
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

### Accepted upstream behaviour, not awaiting a fix

The pinned Polymarket adapter filters positions smaller than
`DUST_POSITION_THRESHOLD` (`0.01` shares, `common/consts.rs`) out of the reports it
returns, logging each at debug level. A holding below that size is therefore absent
from Bolt's projection of venue state, and remains absent no matter what happens to
the conditions above.

This is deliberate upstream behaviour rather than a defect, so no upstream change
will ever close it and it is not a `reconciliation_unmet` condition -- a condition
is something that can be closed, and listing this one would make the list
permanently non-empty. It is recorded here instead, as accepted: the understatement
it can produce is bounded by the threshold times the number of dust holdings, which
is immaterial against any pool ceiling this system configures. If that ceiling ever
becomes small enough for sub-cent holdings to matter, this acceptance has to be
revisited rather than assumed to still hold.
