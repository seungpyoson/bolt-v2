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
  produced and the trade's execution price and fee are lost outright. Closing the condition above does
  **not** close this one; it changes how it fails, and the direction it changes in is the opposite of
  what three review rounds recorded here.

  What the engine does with the order report instead is asserted against the pinned engine in
  `tests/nt_external_order_recovery.rs` rather than described here. Four successive revisions of this
  paragraph traced that control flow in prose and each was wrong in a new way -- naming a function
  that does not exist at the pin, and placing the `create_inferred_fill` calls in a function that
  contains none of them -- because a description of a dependency's internals has nothing to fail
  against. The assertion added last round did not help: it checked that Bolt's own condition string
  contained the words "canceled or expired", so it stayed green while the claim those words made was
  false. Only the dependency can contradict a claim about the dependency, so the claims now live as
  calls into it, and a pin bump that changes the behaviour fails a test instead of quietly
  invalidating this section.

  What those tests establish. Bolt runs the execution engine with no cache database
  (`bolt_v3_live_node::live_node_config` sets `cache: None`) and requires
  `filter_unclaimed_external_orders = false`, so at startup reconciliation the engine's order cache is
  empty and every venue order report is unknown to it. Unknown reports are projected as external
  orders. A report the venue still calls working yields no fill at any `filled_qty`. A terminal report
  yields no fill either -- but only because the condition above has already capped its `filled_qty` to
  zero, and nothing is inferred from a zero quantity. The zero is what prevents it, not the terminal
  status: the same path given a non-zero filled quantity infers a fill, identified by
  `create_inferred_reconciliation_trade_id` from the order's own fields rather than by the venue trade
  id the adapter never supplied. NT deduplicates fills by trade-id equality, so the venue's own later
  `Confirmed` report of that same trade does not match it and is applied on top -- the same executed
  quantity counted twice, up to the order's quantity, past which Bolt's `allow_overfills = false`
  makes the engine drop the venue's *real* fill instead. So closing the zero floor on its own would
  convert a permanent understatement into a double count, which is the reverse of the intuition that
  closing one condition can only help, and the reason both are listed and neither is closed alone;
- the pinned adapter reports every confirmed fill the account earned as maker. At the current pin
  `build_fill_reports_from_trades` selects the account's own entries out of a confirmed maker trade
  with a bare `continue`; when the trade holds no entry the account owns, the loop body never runs,
  no report is produced, `filtered` is not incremented, nothing is logged, and the function returns
  success. Of the six `continue` statements in that function two increment `filtered`, for an
  unmappable instrument; of the four that are silent, two are this condition and the one above, and
  the remaining two fire only under a scoped instrument filter that the mandated empty reconciliation
  filter never sets. Ownership is `maker_address == user_address || owner == api_key` compared as
  exact strings, and `user_address` is the configured funder taken verbatim where one is set, so a
  funder written in the checksummed form the block explorers display fails the address test against
  a lowercase payload for every one of the account's own orders -- leaving the API-key test alone to
  carry it, and that one fails for anything placed under a different key of the same account. The
  adapter compares an address without regard to case elsewhere (`execution/mod.rs`), so this is an
  inconsistency at the pin rather than a venue constraint;
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

### Dust filtering: deliberate upstream, gated as the same missing channel

The pinned Polymarket adapter filters positions smaller than
`DUST_POSITION_THRESHOLD` (`0.01` shares, `common/consts.rs`) out of the reports it
returns, logging each at debug level. A holding below that size is therefore absent
from Bolt's projection of venue state.

An earlier version of this section excluded it from the `reconciliation_unmet`
conditions on the grounds that a condition is something that can be closed and this
one never could. That reasoning does not survive its own last paragraph, which said
closing it needs the adapter to report what it filtered -- the identical remedy the
first condition names. The filtering is deliberate upstream behaviour and the
*silence about it* is not: a count of what was filtered would let Bolt know its
projection is partial without upstream changing the threshold at all. So the first
condition now covers both omissions, and this section records the magnitude rather
than arguing an exemption.

The understatement is the threshold times the number of dust holdings, and only the
first factor is bounded. Nothing caps the second: the Data API paginates every
position the account holds, and Bolt configures no maximum holding count.

Two arguments that this section previously used to bound the count are both
withdrawn, and neither is replaced.

The first was that fees bound it, since a strategy placing thousands of sub-cent
orders would exhaust the pool. The pinned adapter passes `Decimal::ZERO` as the fee
rate for every maker fill (`execution/parse.rs`, citing the venue's published
schedule), so maker activity bounds nothing.

The second was that the per-order floor bounds it. Every marketable BUY this system
places must clear `MARKET_QUOTE_BUY_MIN_NOTIONAL` -- one dollar, recorded in
`src/bolt_v3_providers/polymarket.rs` from a captured venue reject because the pinned
adapter leaves `min_quantity` and `min_notional` unset deliberately (`http/parse.rs`)
and lets the venue reject instead -- and `make_market_quote_buy_quantity` returns
`BelowMinimum` before an order exists. That floor is real, but the bound drawn from
it was not: it read a $P pool as holding at most $P of positions at once, and the
capital-accounting condition above establishes that filled positions are charged by
nothing. Committed liability is inert and reserved liability covers open orders only,
so capacity returns in full as each order fills and the pool ceiling never limits how
much position the system accumulates. The floor bounds what a single order can be,
not how many orders there can be.

Accumulated remnants were never bounded by either argument in any case. A position
opened at a dollar and exited down to less than `0.01` shares returns its capital and
leaves its remnant behind, so the count of remnants tracks partial exits performed
rather than capital committed -- it grows with runtime, and no ceiling in this system
limits it.

So the magnitude is unbounded and recorded as such: an accepted risk with no proof
attached, deliberately not dressed as one. It cannot be closed on the Bolt side,
because the filtered holdings never reach Bolt and no invariant here could observe
the count it would need to bound. Two things make it matter sooner and both are
observable: a pool ceiling small enough for a handful of remnants to matter, or an
account that accumulates positions this system did not open -- airdrops, transfers in,
or a second trader on the same account. Neither is checked anywhere in code.
