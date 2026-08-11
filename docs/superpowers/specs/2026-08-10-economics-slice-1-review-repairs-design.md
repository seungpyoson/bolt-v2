# Economics Slice 1 Review Repairs

## Scope and invariant

This design repairs the four substantive findings on PR #1544 without expanding beyond issue #1445's atomic quote/admission cutover. It preserves `quote_only` and grants no live, deploy, readiness, or trading authority.

The invariant is: one final NT order has one purpose-typed economics basis, one provider fee authority, and—when it rests—one cancellation coordinator. Callers cannot separately choose gross value, normalized fills, lifecycle, intent kind, admission purpose, or cancellation retry behavior.

## Complete economics caller matrix

Shared order execution in `src/bolt_v3_order_execution.rs` owns the typed scenarios and the final seal. The production inventory is closed as follows:

| Production path | Typed scenario | Derived submit intent | Derived lifecycle and value model |
| --- | --- | --- | --- |
| Edge-taker candidate sizing | `TerminalValueEntry` | `Entry` | `HoldToRedemption`; gross is recomputed from candidate fills and expected terminal value per unit |
| Edge-taker final entry | `TerminalValueEntry` | `Entry` | `HoldToRedemption`; gross is recomputed from final normalized fills |
| Maker final entry | `TerminalValueEntry` | `Entry` | `HoldToRedemption`; maker supplies the outcome's fair terminal value per unit, not absolute gross money |
| Planned risk-reducing exit | `PlannedRiskReducingExit` | `RiskReducingExit` | `PlannedExit`; gross compares final normalized proceeds with stored entry cost per unit |
| Kill-switch forced reduction | `ForcedReduction` | `KillSwitchForcedReduction` | `PlannedExit`; gross trading edge is fixed by the variant to zero because risk-reduction admission, not an edge gate, is authoritative |

Maker cancel-only commands do not construct economics. A maker submit without a `TerminalValueEntry` scenario fails before order evidence or mutation. The strategy helper that currently computes an absolute maker gross value is deleted.

The raw public construction paths are deleted or made private to shared execution: callers cannot construct `BoltV3OrderEconomicsIntent`, `BoltV3OrderEconomicsSubmitInput`, or `BoltV3TakerEconomicsSizingInput` with independently selected gross value, fills, lifecycle, or intent kind. No compatibility constructor remains. The scenario variant derives the submit intent, lifecycle, admission purpose, and value formula; liquidity role derives from the final order's post-only semantics.

Candidate sizing and final sealing use the same `TerminalValueEntry` value object. Candidate sizing may operate before a final order exists, but it cannot supply an absolute gross value or lifecycle. Only the final-order path can construct `FinalOrderEconomicsBasis`.

## One sealed final-order economics basis

`FinalOrderEconomicsBasis` has private fields and a fallible constructor in shared order execution. It consumes:

- the post-NT-rounding, post-Bolt-exit-clamp `OrderAny` that will be routed;
- one purpose-typed scenario from the caller matrix;
- candidate executable fill levels;
- valuation and admission context.

The constructor first validates that scenario purpose, order side, position context, and final order agree. It then binds the exact final order and handles denominations explicitly:

- for a base-quantity order, the order quantity is base quantity;
- for a quote-quantity order, candidate levels are traversed in their supplied execution order and each candidate base quantity must already be accepted by `Instrument::try_normalize_qty`. At each level the constructor divides the remaining quote budget by the positive price, floors the affordable base quantity toward zero onto the exact `Instrument::size_increment()` lattice, retains the lesser of that quantity and the level's remaining lattice-aligned capacity, and subtracts only `price * retained_base_quantity` from the quote budget. The lattice conversion uses checked Decimal division by the positive increment, truncates the quotient to an integer toward zero, multiplies by the increment, constructs the quantity at `size_precision`, and requires `Instrument::try_normalize_qty` to accept the result. A residual left by one price level remains available to every later level; it is not dust merely because it cannot buy an increment at the current price;
- a price-less market order does not invent a limit price; limit checks run only when the final order actually has one.

Over-covering candidate levels are deterministically truncated to the final order. For quote-quantity orders, the constructor may classify the final residual quote budget as non-executable quote dust only after it has traversed every supplied eligible level and proved that no level with unused candidate capacity can retain one more size increment within that budget. Aggregate candidate notional below the submitted quote quantity remains under-coverage and fails; the dust classification cannot hide missing candidate liquidity. Final dust is non-negative, is excluded from gross value, planned-fill notional, and provider fees, and leaves conservative reservation liability bound to the full submitted quote quantity. If another increment is affordable at any remaining-capacity level, stopping is an invariant failure. Empty, non-positive, overflowing, under-covering, side-incompatible, scenario-incompatible, limit-violating, zero-after-alignment, or NT-normalization-rejected inputs fail. The constructor then derives normalized fill legs, planned-fill notional, gross value, provider economics, edge basis, and the final order binding from the retained levels.

For terminal-value entries, each retained level contributes:

```text
(expected_terminal_value_per_unit - fill_price) * fill_quantity
```

For planned risk-reducing exits, each retained level contributes:

```text
(fill_price - stored_entry_cost_per_unit) * fill_quantity
```

Checked Decimal sums produce absolute gross value. Forced reduction supplies no caller-controlled gross amount. Gross value is never proportionally scaled from a pre-rounding aggregate.

Planned-fill notional and conservative reservation liability remain distinct types. Provider economics prices the retained expected fills; shared admission derives conservative liability from the same final order and binding. A buy limit may therefore reserve its limit liability while economics quotes better expected fills. Neither basis can be substituted for the other.

The seal occurs after all rounding and risk-reducing clamps but before strategy evidence, pending-exposure mutation, admission counters, or venue mutation. Construction failure leaves all those surfaces unchanged.

## One fee authority

The obsolete family fee seam is deleted: `MarketFamilyValidationBinding::maker_binary_fee_curve`, every family formula implementation and unsupported fallback, and the public lookup functions. Provider economics adapters remain the sole fee-formula authority. No compatibility wrapper or replacement family lookup is added.

Only fee-specific fixtures and assertions are removed. Tests that also cover settlement payout, maker quote-target dispatch, or unknown-family failure remain and are narrowed to those responsibilities. Compilation plus direct diff and symbol inspection prove deletion; no source-scanning test is added.

## One resting-order cancellation coordinator

`cancel_pending: bool` and the direct retry decisions for tracked maker orders are replaced by one `RestingOrderCancelCoordinator` in shared order execution. It owns attempt timing, diagnostics, and health only. NT's cache and order events remain authoritative for order status and remaining quantity.

All normal tracked-maker cancellation origins use the coordinator: economics refresh, per-leg quote lifecycle, side- or instrument-scoped cancel-all, and strategy stop. The pure quote-lifecycle machine may request cancellation, but it cannot route or retry one. Its `CancelRejected -> Cancel` action is removed; the leg retains its lifecycle state while the coordinator owns retry timing. Slice 1's kill-switch cancellation plan remains dry-run proof-only. If NT nevertheless reports an externally initiated pending cancel, the coordinator adopts that authoritative state without issuing a competing request.

A coordinator record represents exactly one outstanding cancellation intent. It is created only by a cancellation-origin request, a pending-cancel observation for a tracked maker order, or running-state fill-void reconciliation. Because pinned `Strategy::query_order` accepts an `OrderAny` even though it reads only three identity fields, resting economics registration retains a private `NtOrderQuerySeed` wrapper around the submitted order clone. The wrapper exposes no status or leaves accessors and can only be handed to that NT query call. Its `instrument_id` and `client_order_id` are immutable; while an authoritative cached order exists, the wrapper may replace its snapshot exactly once to capture a `venue_order_id` transition from `None` to `Some`. The seed is query-routing data only and never supplies status, leaves quantity, or terminality. Running-state fill-void reconciliation constructs the same private seed from the authoritative cached order when it creates a cancellation-only record. A healthy resting order has no coordinator record and no timer drive can cancel it. Repeated origins merge diagnostics into the existing record without resetting its generation, deadline, or backoff. Terminal reconciliation removes both the cancellation intent and its resting-order registration.

Before mapping cached status into an authoritative observation, one private exhaustive identity-coherence transition compares the captured and cached venue IDs:

| Captured seed ID | Cached ID | Transition |
| --- | --- | --- |
| `None` | `None` | Keep the seed unchanged |
| `None` | `Some(id)` | Atomically capture `id` in the query seed |
| `Some(id)` | `None` | Preserve the captured identity; absence in the current cache snapshot cannot weaken query capability |
| `Some(id)` | `Some(id)` | Keep the seed unchanged |
| `Some(captured)` | `Some(observed)` where they differ | Preserve the captured seed plus routing state, generation, deadline, and attempt counters; advance only actor-clock observation and any due monotonic deadline health; add typed `RecoveryIdentityConflict { captured, observed }`, perform no routing classification, NT operation, or retirement, retain the primary error, continue due siblings, and return one aggregate error afterward |

The conflict transition runs before terminal removal as well as before cancel or query routing. It is fail-atomic for routing: generation, deadline, counters, routing state, and the captured seed do not change, and no pre-existing health facet is overwritten. It records the current actor-clock observation and adds any due monotonic deadline health alongside the conflict. `RecoveryIdentityConflict` is a monotonic routing hold. Once present, later callbacks and timer drives keep the record unresolved and perform no NT operation or retirement even if the cache changes again; they still validate the actor clock and accumulate due monotonic deadline health. The pre-conflict routing state is retained only as forensic context, not as permission to resume. Pinned NT acceptance and reconciliation may replace the cached current venue ID, so a conflict is reachable and cannot be handled as an informal assertion outside this transition.

### Authoritative NT observation matrix

After the identity-coherence transition succeeds, every timer or order callback maps the current NT cache through an exhaustive `OrderStatus` match with no wildcard:

| Observation | NT state |
| --- | --- |
| `MissingUnqueryable` | No cached order and no authoritative venue order ID has been captured |
| `MissingQueryable` | No cached order and an authoritative venue order ID has been captured |
| `Retryable` | `Initialized`, `Emulated`, `Released`, `Submitted`, `Accepted`, `Triggered`, `PendingUpdate`, or `PartiallyFilled`, with positive leaves when leaves are available |
| `PendingCancelUnqueryable` | NT reports `PendingCancel` and no authoritative venue order ID has been captured |
| `PendingCancelQueryable` | NT reports `PendingCancel` and an authoritative venue order ID has been captured |
| `Terminal` | `Denied`, `Rejected`, `Canceled`, `Expired`, `Filled`, or `Voided`, or cached leaves are zero |

An `OrderFilled` callback is only a reconciliation trigger. A partial fill remains `Retryable`; the record retires only when the cache is closed or leaves are zero. NT updates its cache before strategy callbacks at the pinned revision, so callbacks never override current cache state with stale event payloads.

NT can later reopen a filled order through `OrderFillVoided`. While the maker is `Running`, its fill-void callback re-reads the cache. If a previously retired maker order is open again, it creates a cancellation-only coordinator record with `quote_deadline_ns = observed_now_ns`; it does not recreate or reuse expired economics admission. The next timer routes through the normal coordinator and health is immediately deadline-exceeded until NT closes the order. Strategy event ownership identifies the order as this maker's; no client-order-ID string parsing or source scan is used.

This guarantee ends when `Component::stop` completes: pinned NT logs residual order events but does not dispatch strategy callbacks outside `Running`. NT exposes no authoritative event proving that a fill can never later be voided, so keeping a fill tombstone until an invented finality deadline would make graceful stop non-convergent. A post-stop reopen remains visible to NT cache/reconciliation and the existing next-start open-order fail-closed gate, but this live-disabled Slice 1 does not claim automatic post-stop cancellation or cross-process retry durability.

### Coordinator state matrix

Routing state is deliberately small:

- `Ready`: a cancellation may be attempted now;
- `Attempting { generation, operation, not_before_ns }`: a cancel or query operation has been armed under coordinator ownership and the coordinator lock has been released for the NT call;
- `Backoff { not_before_ns }`: an operation occurred and no further operation is allowed before the deadline;
- `PendingCancel { not_before_ns }`: NT reports its local pending-cancel request state; this is not a venue acknowledgment, and duplicate cancel calls stay suppressed.

Operation generation, checked cancel/query counters, last outcomes, captured query identity, and typed health are diagnostic or routing metadata, not alternate order-state authority. The implementation is an exhaustive match over coordinator state × authoritative observation; adding a state or observation must produce a compiler error until every pair is handled.

| Current state | `MissingUnqueryable` | `MissingQueryable` | `Retryable` | `PendingCancelUnqueryable` | `PendingCancelQueryable` | `Terminal` |
| --- | --- | --- | --- | --- | --- | --- |
| `Ready` | Expose `RecoveryIdentityUnavailable`; perform no NT operation and remain `Ready` | An origin request or timer drive begins an NT `query_order` recovery from the captured identity; never call cancel against a missing cache entry | An origin request or timer drive begins one cancel attempt | Adopt `PendingCancel` and arm its reconciliation deadline; do not query or cancel | Adopt `PendingCancel` and arm its reconciliation deadline; do not cancel | Remove record |
| `Attempting` | Expose `RecoveryIdentityUnavailable` and settle to `Backoff` at the already-armed deadline | Settle to `Backoff` at the already-armed deadline | Settle to `Backoff` at the already-armed deadline | Settle to `PendingCancel` at the already-armed deadline | Settle to `PendingCancel` at the already-armed deadline | Remove record |
| `Backoff` | Expose `RecoveryIdentityUnavailable`; perform no NT operation, preserving the state and deadline | Suppress before the deadline; timer begins one query at or after it | Suppress before the deadline; timer begins one cancel at or after it | Enter `PendingCancel`, preserving the armed deadline; do not query or cancel | Enter `PendingCancel`, preserving the armed deadline; do not cancel | Remove record |
| `PendingCancel` | Expose `RecoveryIdentityUnavailable` and return to `Backoff`; do not query or cancel | Return to `Backoff`; timer queries at or after the preserved deadline | Return to `Backoff`; timer cancels at or after the preserved deadline | Suppress without an NT operation; the adapter's deferred delivery of the already-issued cancel may still complete when submission supplies the venue ID | Suppress before the deadline; timer queries, rather than re-canceling, at or after it | Remove record |

Both missing observations are invariant-recovery paths, not cancelable order states. `MissingUnqueryable` never calls `query_order`, because pinned Polymarket requires a venue order ID. `MissingQueryable` uses the captured identity only to call NT's native `query_order`; it never supplies authoritative status. The pending observations make the same query capability explicit: pinned Polymarket may locally enter `PendingCancel` before a submit response supplies the venue ID, and its adapter may defer delivery of that already-issued cancel until the ID arrives. That adapter behavior is continuation of the coordinator's one NT cancel operation, not a second cancellation origin or retry authority. The coordinator does not compete with it and does not issue an impossible query; after authoritative cache identity capture, `PendingCancelQueryable` permits bounded reconciliation queries. A dispatched query is not evidence that recovery succeeded: pinned Polymarket emits no status report for venue `not found` and may emit none after a transport failure. Only a later authoritative NT cache observation may restore cancel routing or retire the record. Otherwise the record remains unresolved, queries continue only at the configured rate when identity permits them, health becomes loud, and graceful stop does not falsely complete. `Ready × PendingCancelUnqueryable` and `Ready × PendingCancelQueryable` cover externally initiated cancels. A stale `CancelRejected` while `Ready` only reconciles current cache state; callbacks never route.

### Attempt outcome, retry, and health rules

Every actual cancel or query invocation increments its checked typed counter and arms the same checked not-before deadline. A checked total recovery-attempt counter drives escalation, so repeated queryable-cache-missing or locally-pending reconciliation cannot look healthy forever. `MissingUnqueryable` performs no fake attempt and instead exposes `RecoveryIdentityUnavailable` immediately. Synchronous routing failures remain rate-limited. `SkippedByPolicy` is not an operation and advances neither counters nor routing state.

Operations use a two-phase generation protocol because pinned NT publishes `OrderPendingCancel` synchronously before it sends the cancel command. Under the coordinator lock, the timer increments the generation and counters, arms the deadline, and enters `Attempting`; it then releases the lock before calling NT. A synchronous pending, rejection, retryable, missing, or terminal callback reconciles that generation through the exhaustive matrix. After NT returns, the caller reacquires the lock, re-reads authoritative cache state, and settles the result only if the same generation is still `Attempting`; a callback that already advanced or removed the record cannot be overwritten by stale return-path bookkeeping.

`CancelRejected` never routes from its callback. Reconciliation normally sees NT's restored retryable status and preserves the current not-before deadline. If NT still says locally pending, the coordinator remains pending. At its deadline the timer queries NT only when an authoritative venue order ID has been captured; otherwise it performs no NT operation and exposes recoverability health while the adapter's existing deferred-cancel path remains sole owner. It never invokes `cancel_order` while locally pending, because pinned NT treats that as `Ok` without sending another command. A later authoritative status report makes the cancel-or-retire decision.

At exactly `cancel_recovery_escalation_attempts`, the record exposes `RetryEscalated` and a loud error. Later timer drives continue rate-limited recovery when an operation is possible. Recoverability, identity coherence, and liveness are separate monotonic facets: missing cache plus absent query identity exposes `RecoveryIdentityUnavailable`; a conflicting authoritative venue identity exposes `RecoveryIdentityConflict`; the retained quote deadline yields `CancellationDeadlineExceeded` while retryable or missing, and `StuckPendingCancel` while locally pending, including the normal pre-acceptance deferred-delivery case if it lasts too long. Retry escalation, recoverability, identity conflict, and liveness may coexist; none overwrites another. Exact-boundary collisions expose every applicable facet. None creates venue-paced retry churn. Operation and health failures are isolated per record: every due sibling is processed, each primary error is retained, and one aggregate error is returned afterward.

Cancel-all is a scoped cancellation-intent origin, not a second NT batch-routing authority. It selects records by the exact `(instrument_id, order_side)` scope, creates or merges their intents, and lets the coordinator fan out only the per-order cancel/query operations permitted by each record's state and deadline. It never invokes NT's scope-wide cancel API, because that API cannot exclude a selected sibling that is already pending or still in backoff. An accepted per-order operation arms backoff for only that record; a synchronous failure rate-limits only that record while sibling processing continues. `SkippedByPolicy` creates no intents or operations. Uncovered records remain immediately eligible.

## NT callback and stop integration

The maker's `nautilus_strategy!` hook block implements `on_order_pending_cancel`, `on_order_cancel_rejected`, `on_order_canceled`, `on_order_filled`, `on_order_fill_voided`, `on_order_expired`, and terminal rejection handling. Each hook forwards only the client order ID and current NT actor time to coordinator reconciliation; event payload status never becomes a second authority. On every fill-void callback, current cache status and leaves alone decide whether an untracked reopened maker order needs a cancellation-only record.

The concrete stop hook is NT's `Strategy::stop() -> bool`, registered by `Trader`. Maker construction rejects `manage_stop = true`; the maker's tracked-order draining protocol and NT's position-closing managed stop cannot both own the same stop request. The maker archetype's canonical strategy envelope already uses `manage_stop = false`.

If no tracked orders exist, the maker stop hook returns `true`. Otherwise it enters maker `Draining`, creates or merges a cancellation intent for every tracked order, and returns `false`, so NT leaves the strategy `Running`; its timer and order callbacks continue. While `Draining`, the timer drives only coordinator reconciliation and cancellation recovery. Quote planning, active-market refresh that can re-quote, new economics admission, and new submission are disabled with a typed loud skip; no new resting registration can be created. When authoritative reconciliation removes the last tracked order and cancellation record, the timer or callback completes shutdown through public `Component::stop(self)` and returns immediately without further work. An identity-unavailable order, an adapter `not found` response without a report, or a query transport failure cannot be treated as terminal: draining remains deferred with loud recoverability/liveness health until NT later supplies an authoritative observation or the operator uses the separately governed process-level forced-termination path. Only after successful authoritative reconciliation does `DataActor::on_stop` deregister the timer, unsubscribe, and deactivate the maker runtime.

This stop protocol is independent of NT's position-closing `manage_stop` policy. Process-level forced termination cannot guarantee acknowledgement delivery and is not claimed as an in-process graceful-stop path.

## Clock and deadline matrix

Production economics admission and cancellation use one clock: the NT actor clock. Shared admission receives explicit event time from the routing context; the production economics path does not call `SystemTime` or another wall clock.

| Value | Authority | Use |
| --- | --- | --- |
| `requested_at_ns` | Original economics request | Quote lineage and provider validity calculation only |
| `route_now_ns` | Fresh NT actor time at shared routing entry | Remaining-lifetime and admission evaluation before evidence or counters |
| `pre_sink_now_ns` | Fresh NT actor time immediately before venue mutation | Final remaining-lifetime guard; failure drops the uncommitted admission permit and does not call the sink |
| `operation_not_before_ns` | Checked `attempt_now_ns + retry_timeout` | Earliest next cancel or query operation after success, rejection, or synchronous failure |
| `quote_deadline_ns` | Economics quote `valid_until_ns` | Cancellation health deadline |
| `last_observed_ns` | Prior coordinator observation | Clock-regression rejection |

`ExecutionEconomicsConfig` gains required, no-default positive `cancel_retry_timeout_ms` and `cancel_recovery_escalation_attempts`. The latter counts all coordinator recovery operations, including invariant-recovery queries, rather than only cancel commands. Every shipped economics TOML section and fixture is updated in the same branch state; serde defaults are forbidden.

If `C` is maker timer cadence, `R` retry timeout, and `ceil_to_cadence(R)` the first timer-observable retry delay, startup accepts configuration only when checked arithmetic proves:

```text
C + ceil_to_cadence(R) < resting_order_refresh_margin
```

The timeout must also be positive and shorter than the refresh margin. Missing, zero, overflowing, clock-regressing, source-horizon-incompatible, or cadence-incompatible inputs fail closed.

At shared routing entry, the sealed quote must satisfy `valid_until_ns - route_now_ns >= resting_order_refresh_margin_ns`; `valid_until_ns - requested_at_ns` is not sufficient. The same remaining-margin check runs with fresh `pre_sink_now_ns` immediately before a live sink call. If the second check fails, the admission permit's drop rollback restores counters and reservations; valid order-intent evidence may remain because routing began with a valid intent, but no venue mutation occurs. Shadow evaluation uses the same explicit `route_now_ns` and never reaches a sink.

## Error and mutation ordering

1. Build and validate the purpose-typed final basis.
2. Capture `route_now_ns`; validate final order binding, execution authority, purpose, and remaining lifetime.
3. Record valid order intent.
4. Evaluate admission with that same explicit time.
5. For live routing, capture `pre_sink_now_ns`, revalidate remaining lifetime, call NT, then commit the permit and resting registration.

Failures in steps 1–2 leave evidence, pending exposure, counters, registrations, and venue state unchanged. A step-5 failure drops the permit and registration guards, restoring counters/reservations and leaving no tracked resting record. Cancellation errors advance only checked attempt/backoff diagnostics, never NT order status, and cannot starve siblings.

## Behavioral evidence matrix

Tests are behavioral; no source-scanning test is added.

| Requirement | Discriminating evidence |
| --- | --- |
| Every economics caller is purpose-typed | Edge candidate sizing, edge final entry, maker final entry, planned exit, and forced reduction each assert derived intent, lifecycle, role, value, and admission purpose; cancel-only maker actions require no scenario |
| Cross-purpose pairing is impossible | Scenario constructors derive intent/lifecycle privately; compilation and direct API review confirm raw constructors are gone |
| Final base-quantity coherence | Multi-level coarse rounding and over-cover truncation assert retained legs, recomputed gross, provider fee, edge basis, planned notional, distinct reservation liability, and final binding |
| Final quote-quantity coherence | A multi-level quote-quantity order with a non-power-of-ten size increment such as `0.05` leaves a residual at one level that can fund an increment at a later lower-priced level. Evidence asserts the later leg is retained, only retained executable notional is subtracted at each step, every retained leg passes `try_normalize_qty`, final dust is classified only after every remaining-capacity level is tested, and planned notional, gross, provider fee, full reservation liability, and binding include the later leg |
| Price-less order support | A market quote-quantity order seals without inventing a limit; invalid or under-covering levels fail |
| Fail-before-mutation construction | Under-cover, invalid price/quantity, limit violation, scenario mismatch, and exit-clamp mismatch leave evidence, pending exposure, counters, registration, sink calls, and venue state unchanged |
| Cancellation-intent gate | A healthy tracked resting order has no coordinator record and survives repeated timer drives without a cancel; each origin creates or merges exactly one intent |
| Exhaustive cancellation transitions | A table-driven test covers all 24 coordinator-state × observation pairs, including `Attempting` re-entry, both `Ready × PendingCancel*` cases, stale rejection while ready, and the distinct unqueryable-missing/queryable-missing/retryable/unqueryable-pending/queryable-pending actions |
| Venue-identity coherence gate | A table-driven test covers absent/absent, absent/present capture, equal present IDs, captured-present/cached-absent retention, and conflicting present IDs. A conflict combined with retry escalation and deadline health preserves routing state, generation, deadline, counters, seed, and pre-existing health; advances actor-clock observation and due monotonic health; adds the conflict hold/error; performs no query, cancel, or retirement even when cached status is terminal or later changes again; processes a due sibling; and returns the conflict in the aggregate error |
| Missing-cache and pending-submit recovery | Through the real NT query/reconciliation boundary: pre-acceptance cache loss without a venue ID performs no query or cancel and exposes `RecoveryIdentityUnavailable`; a local pending cancel without a venue ID lets the adapter continue delivery of the coordinator's already-issued operation and does not query or create a second cancel; authoritative cache observation captures the venue ID once; a queryable missing or pending order is queried and never passed to a competing cancel; restoration or a terminal report resumes or retires it; Polymarket `not found` without a report and query transport failure cause no fabricated retirement, continue bounded queries, expose loud health, and do not starve siblings |
| Reentrant rejection and stale-event safety | Synchronous pending, rejection, and terminal callbacks before the NT call returns cannot be overwritten; `Backoff -> PendingCancel -> CancelRejected/open` becomes timer-retryable at the preserved deadline; delayed rejection against newer pending/terminal cache state cannot override it |
| Fill and correction safety | While `Running`, a partial `OrderFilled` with positive leaves retains cancellation tracking; a full fill or zero leaves retires it; a later reopened `OrderFillVoided` creates a cancellation-only record and routes only on the next timer; after `Component::stop`, residual events are observable through NT but no strategy callback or automatic cancellation is claimed |
| Retry-rate floor | Repeated synchronous failures, missing-cache queries, and locally pending attempts do not perform another operation before the timeout, perform exactly one at the boundary, and remain sibling-isolated |
| Attempt escalation and health composition | The exact configured total recovery-attempt threshold exposes `RetryEscalated`; a later timer still performs one bounded operation when query/cancel identity permits it; identity-unavailable and threshold/deadline collisions retain every applicable retry, recoverability, and liveness facet |
| Pending/deadline health | Local pending suppresses duplicate cancel commands and becomes `StuckPendingCancel`; retryable/missing becomes `CancellationDeadlineExceeded` at the quote deadline |
| Cancel-all scoping | One-side cancel-all creates intents only for matching records and fans out through the per-order coordinator; a repeated origin cannot bypass an existing pending/backoff deadline, one synchronous failure does not starve siblings, and shadow skip affects none |
| One clock and remaining lifetime | Missing/zero/overflow/cadence config fails; delayed routing that passes total lifetime but lacks remaining margin fails before evidence; exact boundary succeeds; clock regression fails |
| Pre-sink recheck and rollback | An injected clock advance between admission and sink blocks the sink and proves counter/reservation/registration rollback |
| Real graceful stop | Maker construction rejects `manage_stop = true`; through NT's real `Strategy::stop`/`Trader` lifecycle under active quoting conditions, stop returns deferred while records remain, timer/callback processing continues, zero new quote/admission/submit calls occur after the request, and `Component::stop` runs only after the last authoritatively reconciled record retires; an identity-unavailable or no-report query case remains Running and loud rather than falsely completing |
| Fee-seam deletion without collateral loss | Provider formula and replay parity tests remain; settlement dispatch, maker quote-target dispatch, and unknown-family failure tests remain after fee assertions are removed |

Existing fail-before-mutation admission and resting-refresh tests remain regression evidence. Deletion of obsolete public paths is established by compilation and reviewer diff/symbol inspection, not by testing source text.

## Non-goals

This repair does not implement actual economics ledgers, supplemental venue actuals, lifecycle or transfer actuals, reporting closure, live economics publication, or live execution. Those remain outside Slice 1. It also does not claim retry durability across process death or forced process termination.
