# Economics Slice 1 Review Repairs

## Scope and invariant

This design repairs the review findings mapped below without expanding beyond issue #1445's atomic quote/admission cutover. It preserves `quote_only` and grants no live, deploy, readiness, or trading authority.

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

The seal occurs after all rounding and risk-reducing clamps but before prepared/submission evidence, pending-exposure mutation, admission counters, or venue mutation. A valid decision-intent fact may precede the seal because it claims no prepared or submitted order; construction failure leaves every later surface unchanged.

## One fee authority

The obsolete family fee seam is deleted: `MarketFamilyValidationBinding::maker_binary_fee_curve`, every family formula implementation and unsupported fallback, and the public lookup functions. Provider economics adapters remain the sole fee-formula authority. No compatibility wrapper or replacement family lookup is added.

Only fee-specific fixtures and assertions are removed. Tests that also cover settlement payout, maker quote-target dispatch, or unknown-family failure remain and are narrowed to those responsibilities. Compilation plus direct diff and symbol inspection prove deletion; no source-scanning test is added.

## One resting-order cancellation coordinator

`cancel_pending: bool` and the direct retry decisions for tracked maker orders are replaced by one `RestingOrderCancelCoordinator` in shared order execution. It owns attempt timing, diagnostics, and health only. NT's cache and order events remain authoritative for order status and remaining quantity.

All normal tracked-maker cancellation origins use the coordinator: economics refresh, per-leg quote lifecycle, side- or instrument-scoped cancel-all, and strategy stop. The pure quote-lifecycle machine may request cancellation, but it cannot route or retry one. Its `CancelRejected -> Cancel` action is removed; the leg retains its lifecycle state while the coordinator owns retry timing. Slice 1's kill-switch cancellation plan remains dry-run proof-only. If NT nevertheless reports an externally initiated pending cancel, the coordinator adopts that authoritative state without issuing a competing request.

A coordinator record represents exactly one outstanding cancellation intent. It is created only by a cancellation-origin request, a pending-cancel observation for a tracked maker order, or running-state fill-void reconciliation.

A focused `tracked_order_economics` module owns the public `BoltV3OrderEconomicsHandle` itself and the complete tracked-maker aggregate, not merely an inner field stored by the parent execution module. The parent module re-exports the handle but cannot name or access its fields. A handle clone shares the same private aggregate; no clone or constructor can produce a partial record. Inside the module, one opaque registry owns the lock, map, `TrackedMakerOrderRecord`, optional resting economics, query seed, and optional cancellation intent. The exhaustive cancellation reducer remains a subordinate private module rather than absorbing registration and economics responsibilities into a cancellation monolith.

Outside `tracked_order_economics`, code never receives `&mut TrackedMakerOrderRecord`, a mutable callback over the registry, an aggregate constructor, a registration guard, or a constructor for a partially initialized record. Its only interfaces are semantic operations: construct a complete handle from bound economics, quote economics, route a resting submit transaction, refresh economics from an authoritative cache observation, request one cancellation, request an instrument/side scope, reconcile one NT callback, drive all tracked orders from timer-owned cache observations through `drive_all_resting_order_economics_at_ms`, drive exactly the observations selected by a cancellation origin through `drive_observed_resting_order_economics`, inspect read-only IDs/health, and test whether draining is complete. The all-orders and exact-observation operations are distinct APIs: an empty exact observation set is a no-op and can never mean all tracked orders. The resting-submit transaction inserts a provisional record under the registry lock, releases that lock before invoking the supplied submit operation without handing it registry internals, leaves callback-reconciled state intact for `Submitted`, and reacquires the lock to remove the provisional registration for every non-submitted typed outcome while returning that outcome unchanged. Synchronous NT callbacks can therefore reconcile or retire the record without deadlocking, and no external sink is called while the registry lock is held. Registration, refresh, intent merging, terminal removal, and rollback execute inside the owner. This makes generation, deadline, backoff, query identity, record lifetime, and drive scope compiler-owned as one aggregate rather than replaceable at a parent call site.

Inside that aggregate, a private `TrackedOrderCancellation` owns both the query seed and the optional coordinator record; its idempotent `request_intent(quote_deadline_ns)` preserves an existing generation, deadline, and backoff. Because pinned `Strategy::query_order` accepts an `OrderAny` even though it reads only three identity fields, resting economics registration retains a private `NtOrderQuerySeed` wrapper around the submitted order clone inside that owner. The wrapper exposes no status or leaves accessors and can only be handed to that NT query call. Its `instrument_id` and `client_order_id` are immutable; while an authoritative cached order exists, the wrapper may replace its snapshot exactly once to capture a `venue_order_id` transition from `None` to `Some`. The seed is query-routing data only and never supplies status, leaves quantity, or terminality. Running-state fill-void reconciliation asks the aggregate to create a cancellation-only record from the authoritative cached order and request an immediate intent; it cannot assemble that record itself. A healthy resting order has no coordinator record and no timer drive can cancel it. Repeated origins merge diagnostics into the existing record without resetting its generation, deadline, or backoff. Terminal reconciliation removes the aggregate record.

Before mapping cached status into an authoritative observation, one private exhaustive identity-coherence transition compares the captured and cached venue IDs:

| Captured seed ID | Cached ID | Transition |
| --- | --- | --- |
| `None` | `None` | Keep the seed unchanged |
| `None` | `Some(id)` | Atomically capture `id` in the query seed |
| `Some(id)` | `None` | Preserve the captured identity; absence in the current cache snapshot cannot weaken query capability |
| `Some(id)` | `Some(id)` | Keep the seed unchanged |
| `Some(captured)` | `Some(observed)` where they differ | Preserve the captured seed plus routing state, generation, deadline, and attempt counters; advance only actor-clock observation and any due monotonic deadline health; add typed `RecoveryIdentityConflict { captured, observed }`, perform no routing classification, NT operation, or retirement, retain every active health facet in one composed per-record error, continue due siblings, and return one aggregate error afterward |

The conflict transition runs before terminal removal as well as before cancel or query routing. It is fail-atomic for routing: generation, deadline, counters, routing state, and the captured seed do not change, and no pre-existing health facet is overwritten. It records the current actor-clock observation and adds any due monotonic deadline health alongside the conflict. Because the mismatched cached order cannot supply trusted status, conflict-held deadline classification uses the retained coordinator state: `PendingCancel` yields `StuckPendingCancel`; every other retained state yields `CancellationDeadlineExceeded`. `RecoveryIdentityConflict` is a monotonic routing hold. Once present, later callbacks and timer drives keep the record unresolved and perform no NT operation or retirement even if the cache changes again; they still validate the actor clock and accumulate due monotonic deadline health. The pre-conflict routing state is retained only for trusted coordinator health and forensic context, never as permission to resume. Pinned NT acceptance and reconciliation may replace the cached current venue ID, so a conflict is reachable and cannot be handled as an informal assertion outside this transition.

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

Operation generation, checked cancel/query counters, last outcomes, captured query identity, and typed health are diagnostic or routing metadata, not alternate order-state authority. The coordinator exposes one private event reducer, `apply_event(CancelEvent) -> CancelEffect`; routing-capable timer observations, passive observations, successfully observed NT operations, and unobserved operations are closed event variants, while no-op, record removal, cancel, and query are closed effect variants. An operation is unobserved when NT routing fails synchronously or when the actor clock/cache cannot be read after routing; both settle the already-armed generation to backoff before health collection. The prior separate plan, callback-reconcile, success-settle, and failure-settle methods do not exist. The reducer owns every cancellation-state mutation and delegates state × authoritative observation to one exhaustive match; adding an event, state, observation, or effect must produce a compiler error until every case is handled. The aggregate owns record creation/removal and invokes the reducer; no caller can bypass either layer. A thin effect runner inside the same module may execute returned NT effects outside the registry lock, but it cannot inspect or mutate generation, deadlines, routing state, or health. It is the only tracked-maker caller of the NT cancellation/query sink.

| Current state | `MissingUnqueryable` | `MissingQueryable` | `Retryable` | `PendingCancelUnqueryable` | `PendingCancelQueryable` | `Terminal` |
| --- | --- | --- | --- | --- | --- | --- |
| `Ready` | Expose `RecoveryIdentityUnavailable`; perform no NT operation and remain `Ready` | An origin request or timer drive begins an NT `query_order` recovery from the captured identity; never call cancel against a missing cache entry | An origin request or timer drive begins one cancel attempt | Adopt `PendingCancel` and arm its reconciliation deadline; do not query or cancel | Adopt `PendingCancel` and arm its reconciliation deadline; do not cancel | Remove record |
| `Attempting` | Expose `RecoveryIdentityUnavailable` and settle to `Backoff` at the already-armed deadline | Settle to `Backoff` at the already-armed deadline | Settle to `Backoff` at the already-armed deadline | Settle to `PendingCancel` at the already-armed deadline | Settle to `PendingCancel` at the already-armed deadline | Remove record |
| `Backoff` | Expose `RecoveryIdentityUnavailable`; perform no NT operation, preserving the state and deadline | Suppress before the deadline; timer begins one query at or after it | Suppress before the deadline; timer begins one cancel at or after it | Enter `PendingCancel`, preserving the armed deadline; do not query or cancel | Enter `PendingCancel`, preserving the armed deadline; do not cancel | Remove record |
| `PendingCancel` | Expose `RecoveryIdentityUnavailable` and return to `Backoff`; do not query or cancel | Return to `Backoff`; timer queries at or after the preserved deadline | Return to `Backoff`; timer cancels at or after the preserved deadline | Suppress without an NT operation; the adapter's deferred delivery of the already-issued cancel may still complete when submission supplies the venue ID | Suppress before the deadline; timer queries, rather than re-canceling, at or after it | Remove record |

Both missing observations are invariant-recovery paths, not cancelable order states. `MissingUnqueryable` never calls `query_order`, because pinned Polymarket requires a venue order ID. `MissingQueryable` uses the captured identity only to call NT's native `query_order`; it never supplies authoritative status. The pending observations make the same query capability explicit: pinned Polymarket may locally enter `PendingCancel` before a submit response supplies the venue ID, and its adapter may defer delivery of that already-issued cancel until the ID arrives. That adapter behavior is continuation of the coordinator's one NT cancel operation, not a second cancellation origin or retry authority. The coordinator does not compete with it and does not issue an impossible query; after authoritative cache identity capture, `PendingCancelQueryable` permits bounded reconciliation queries. A dispatched query is not evidence that recovery succeeded: pinned Polymarket emits no status report for venue `not found` and may emit none after a transport failure. Only a later authoritative NT cache observation may restore cancel routing or retire the record. Otherwise the record remains unresolved, queries continue only at the configured rate when identity permits them, health becomes loud, and graceful stop does not falsely complete. `Ready × PendingCancelUnqueryable` and `Ready × PendingCancelQueryable` cover externally initiated cancels. A stale `CancelRejected` while `Ready` only reconciles current cache state; callbacks never route.

### Attempt outcome, retry, and health rules

Every actual cancel or query invocation increments its checked typed counter and arms the same checked not-before deadline. A checked total recovery-attempt counter drives escalation, so repeated queryable-cache-missing or locally-pending reconciliation cannot look healthy forever. `MissingUnqueryable` performs no fake attempt and instead exposes `RecoveryIdentityUnavailable` immediately. Synchronous routing failures remain rate-limited. `SkippedByPolicy` is not an operation and advances neither counters nor routing state.

Operations use a two-phase generation protocol because pinned NT publishes `OrderPendingCancel` synchronously before it sends the cancel command. Under the coordinator lock, `apply_event(TimerObserved { .. })` increments the generation and counters, arms the deadline, enters `Attempting`, and returns exactly one cancel or query effect; the effect runner then releases the lock before calling NT. A synchronous pending, rejection, retryable, missing, or terminal callback enters the same reducer as `PassiveObserved { .. }`; a policy-suppressed timer uses that same explicitly non-routing event, so the event name describes its authority rather than its source. After NT returns, the effect runner reacquires the lock, re-reads authoritative cache state, and feeds either `OperationSucceeded { generation, .. }` or `OperationUnobserved { generation }` into the reducer. Those events settle only if the same generation remains `Attempting`; a passive observation that already advanced or removed the record cannot be overwritten by stale return-path bookkeeping. If reconciliation of an `OperationSucceeded` observation fails, the reducer settles that same still-active generation to its already-armed backoff before returning the error, so no post-operation observation failure can strand `Attempting`.

`CancelRejected` never routes from its callback. Reconciliation normally sees NT's restored retryable status and preserves the current not-before deadline. If NT still says locally pending, the coordinator remains pending. At its deadline the timer queries NT only when an authoritative venue order ID has been captured; otherwise it performs no NT operation and exposes recoverability health while the adapter's existing deferred-cancel path remains sole owner. It never invokes `cancel_order` while locally pending, because pinned NT treats that as `Ok` without sending another command. A later authoritative status report makes the cancel-or-retire decision.

At exactly `cancel_recovery_escalation_attempts`, the record exposes `RetryEscalated` and a loud error. Later timer drives continue rate-limited recovery when an operation is possible. Recoverability, identity coherence, and liveness are separate monotonic facets: missing cache plus absent query identity exposes `RecoveryIdentityUnavailable`; a conflicting authoritative venue identity exposes `RecoveryIdentityConflict`; the retained quote deadline yields `CancellationDeadlineExceeded` while retryable or missing, and `StuckPendingCancel` while locally pending, including the normal pre-acceptance deferred-delivery case if it lasts too long. Retry escalation, recoverability, identity conflict, and liveness may coexist; none overwrites another. Exact-boundary collisions expose every applicable facet. None creates venue-paced retry churn.

One typed per-record health snapshot is the complete reporting authority for both inspection and runtime errors. It owns client order ID, checked total-recovery-attempt count, recoverability, identity-conflict, retry-escalation, and liveness fields. A healthy snapshot produces no runtime error. An unhealthy snapshot renders every active facet and the attempt count in one deterministic message; no caller reads coordinator fields separately or selects a single primary facet.

The timer drive collects that composed health exactly once per retained record, after the record's final state for the current drive is known: after no-operation reconciliation, after a synchronous NT failure has settled backoff, or after successful cancel/query settlement has re-read the cache. Health created during a synchronous callback or post-call cache reconciliation is therefore returned by the initiating drive, not delayed to another timer. Callback entrypoints reconcile state but do not create a second reporting path. Operation failures and composed health remain separate entries in the one sibling-isolated aggregate, and every due sibling is processed before it is returned.

Cancel-all is a scoped cancellation-intent origin, not a second NT batch-routing authority. It selects records by the exact `(instrument_id, order_side)` scope, creates or merges their intents, and drives exactly the successfully observed selected records through the coordinator. Zero matches and zero successful observations are exact empty sets, never sentinels for an all-orders timer drive; cache-read failures remain scoped errors and cannot widen reconciliation to uncovered records. It never invokes NT's scope-wide cancel API, because that API cannot exclude a selected sibling that is already pending or still in backoff. An accepted per-order operation arms backoff for only that record; a synchronous failure rate-limits only that record while sibling processing continues. `SkippedByPolicy` creates no intents or operations. Uncovered records remain immediately eligible.

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
| `route_now_ns` | Fresh NT actor time at shared routing entry | Remaining-lifetime and admission evaluation before shared order/admission evidence or counters; strategy decision-intent evidence may already exist |
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

The shared submit boundary returns one exhaustive `BoltV3SubmitAttemptOutcome`; it no longer erases routing provenance behind `anyhow::Result<BoltV3SubmitRoutingOutcome>`. Its variants distinguish route validation, order-intent evidence, admission, policy skip, pre-sink validation, sink rejection, and submission at the point where each outcome is produced. Every rejection carries a stable typed reason plus diagnostic context. Callers match the enum exhaustively; string parsing and error downcasts are forbidden.

| Shared phase | Typed outcome | Mutation contract |
| --- | --- | --- |
| Route clock/binding/authority validation | `RouteRejected` | No admission, sink, or tracked registration |
| Required order-intent evidence | `IntentEvidenceRejected` | No admission, sink, or tracked registration |
| Live or shadow admission evaluation | `AdmissionRejected` | No committed counter/reservation and no sink |
| Successful shadow evaluation | `PolicySkipped` | Prepared order only; no submitted linkage, sink, pending exposure, or tracked registration |
| Live pre-sink clock/margin validation | `PreSinkRejected` | Admission permit drops and rolls back; no sink or tracked registration |
| NT submit call returns failure | `SinkRejected` | Admission permit and provisional registration roll back |
| NT submit call returns success | `Submitted` | Permit and submitted/tracked state commit |

1. Record a strategy decision-intent fact that claims no prepared or submitted order.
2. Build and validate the purpose-typed final basis and a typed prepared-order linkage.
3. Capture `route_now_ns`; validate final order binding, execution authority, purpose, and remaining lifetime.
4. Record valid order intent.
5. Evaluate admission with that same explicit time.
6. For live routing, capture `pre_sink_now_ns`, revalidate remaining lifetime, call NT, then commit the permit and resting registration.

A step-2 failure may retain the valid strategy decision-intent fact, but leaves prepared/submission evidence, exposure, counters, registrations, and venue state unchanged. Failures before a live sink leave exposure `Managed`. A step-6 failure drops the permit and registration guards, restoring counters/reservations and leaving no tracked resting record. Cancellation errors advance only checked attempt/backoff diagnostics, never NT order status, and cannot starve siblings.

Exit exposure uses one exhaustive attempt state machine. A prepared exit arms `ExitAttempting { generation, prior_managed, prepared_order }` before shared routing. `RouteRejected`, `IntentEvidenceRejected`, `AdmissionRejected`, `PolicySkipped`, `PreSinkRejected`, and `SinkRejected` restore `Managed` only when the same generation is still attempting. `Submitted` alone commits `ExitPending`. A synchronous NT callback may advance the state while the sink call is in flight; the generation-checked return path cannot overwrite that newer state. `PolicySkipped` retains only typed prepared-order evidence, never an actual submitted-order linkage or pending-exit identity.

| Strategy phase/outcome | Exposure transition |
| --- | --- |
| `Held`, `Blocked`, or `PreparationRejected` | Remain `Managed`; no attempt generation exists |
| Prepared order | `Managed -> ExitAttempting(generation)` |
| Any non-submitted shared outcome for the same generation | `ExitAttempting -> Managed` |
| `Submitted` for the same generation | `ExitAttempting -> ExitPending` |
| Synchronous order/position callback advances or retires the generation | Callback result wins; stale route return is a no-op |
| Terminal full fill with unresolved position causality | `ExitPending` or `ExitAttempting -> TerminalFillAwaitingPosition` |
| Satisfied position fence | `TerminalFillAwaitingPosition -> Managed` residual or `Flat` |

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
| Opaque aggregate ownership | `BoltV3OrderEconomicsHandle` and every tracked record field are defined inside `tracked_order_economics`; the parent re-exports only the handle and read-only health value types, never record/state owners. Resting registration, economics refresh, quote-lifecycle cancellation, scoped cancel-all, strategy drain, pending-cancel adoption, fill-void recovery, terminal removal, and rollback are exercised only through semantic operations. A submit test re-enters reconciliation synchronously and proves the registry lock is not held across the sink. Production compilation and direct API/diff review prove that no outside caller can construct, clone, replace, destructure, or mutably borrow a tracked record or cancellation field; no source-scanning test is added |
| Cancellation-intent gate | A healthy tracked resting order has no coordinator record and survives repeated timer drives without a cancel; every origin reaches the aggregate's one idempotent construction/merge operation, and repeated origins preserve the first generation, deadline, and backoff |
| Exhaustive cancellation transitions | A table-driven test covers all 24 coordinator-state × observation pairs, including `Attempting` re-entry, both `Ready × PendingCancel*` cases, stale rejection while ready, and the distinct unqueryable-missing/queryable-missing/retryable/unqueryable-pending/queryable-pending actions; a reducer test proves routing-capable timer, passive-observation, operation-success, and operation-unobserved events are the only state-mutation interface and produce one closed effect |
| Venue-identity coherence gate | A table-driven test covers absent/absent, absent/present capture, equal present IDs, captured-present/cached-absent retention, and conflicting present IDs. A conflict combined with retry escalation and deadline health preserves routing state, generation, deadline, counters, seed, and pre-existing health; advances actor-clock observation and due monotonic health; classifies a retained `PendingCancel` hold as `StuckPendingCancel` and other holds as `CancellationDeadlineExceeded`; adds the conflict hold/error; performs no query, cancel, or retirement even when cached status is terminal or later changes again; processes a due sibling; and returns one composed per-record error containing every active facet through the real drive/aggregate path |
| Complete health authority | The public snapshot contains the checked total-attempt count and every monotonic facet. Healthy snapshots emit no error. A literal expected message proves deterministic composition and fails if any active facet is omitted or a second coordinator-field read is required |
| Post-operation health sampling | A cancel sink changes the cached venue ID during the NT call; post-call settlement discovers the conflict at the retained deadline. The initiating drive returns that conflict plus liveness exactly once while a due sibling still routes. A synchronous route failure likewise settles first, then reports any newly due escalation/liveness without waiting for another timer |
| Missing-cache and pending-submit recovery | Through the real NT query/reconciliation boundary: pre-acceptance cache loss without a venue ID performs no query or cancel and exposes `RecoveryIdentityUnavailable`; a local pending cancel without a venue ID lets the adapter continue delivery of the coordinator's already-issued operation and does not query or create a second cancel; authoritative cache observation captures the venue ID once; a queryable missing or pending order is queried and never passed to a competing cancel; restoration or a terminal report resumes or retires it; Polymarket `not found` without a report and query transport failure cause no fabricated retirement, continue bounded queries, expose loud health, and do not starve siblings |
| Reentrant rejection and stale-event safety | Synchronous pending, rejection, and terminal callbacks before the NT call returns cannot be overwritten; `Backoff -> PendingCancel -> CancelRejected/open` becomes timer-retryable at the preserved deadline; delayed rejection against newer pending/terminal cache state cannot override it |
| Fill and correction safety | While `Running`, a partial `OrderFilled` with positive leaves retains cancellation tracking; a full fill or zero leaves retires it; a later reopened `OrderFillVoided` creates a cancellation-only record and routes only on the next timer; after `Component::stop`, residual events are observable through NT but no strategy callback or automatic cancellation is claimed |
| Retry-rate floor | Repeated synchronous failures, missing-cache queries, and locally pending attempts do not perform another operation before the timeout, perform exactly one at the boundary, and remain sibling-isolated |
| Attempt escalation and health composition | The exact configured total recovery-attempt threshold exposes `RetryEscalated`; a later timer still performs one bounded operation when query/cancel identity permits it; identity-unavailable and threshold/deadline collisions retain every applicable retry, recoverability, and liveness facet |
| Pending/deadline health | Local pending suppresses duplicate cancel commands and becomes `StuckPendingCancel`; retryable/missing becomes `CancellationDeadlineExceeded` at the quote deadline |
| Cancel-all scoping | One-side cancel-all creates intents only for matching records and fans out through the per-order coordinator; zero matches and a selected-record cache-read failure never refresh, reconcile, or cancel an uncovered record; a repeated origin cannot bypass an existing pending/backoff deadline, one synchronous failure does not starve siblings, and shadow skip affects none |
| One clock and remaining lifetime | Missing/zero/overflow/cadence config fails; delayed routing that passes total lifetime but lacks remaining margin fails before evidence; exact boundary succeeds; clock regression fails |
| Pre-sink recheck and rollback | An injected clock advance between admission and sink blocks the sink and proves counter/reservation/registration rollback |
| Typed submit provenance | Each shared routing phase is failed independently and produces its exact typed attempt outcome without string parsing or downcasts; prepared linkage, counters/reservations, sink calls, and exposure state are asserted for route validation, intent evidence, admission, policy skip, pre-sink, sink rejection, and submission |
| Exit-attempt state authority | A table-driven test covers every attempt outcome. All non-submitted outcomes restore `Managed`; only `Submitted` commits `ExitPending`. A shadow-policy skip records prepared-only evidence, performs no venue/capacity mutation, leaves exposure `Managed`, and permits a later eligible evaluation. A synchronous callback that advances the generation cannot be overwritten by route-return rollback |
| Terminal-fill position causality | A reconciliation-projected terminal fill that updates the order but not the position enters `TerminalFillAwaitingPosition`; an immediate timer observes the unchanged position watermark and performs no remanagement or new reduction. Only a matching position event or cache snapshot that proves the terminal fill is incorporated releases the state; a later timer without a callback then remanages the exact residual and the next evaluation routes only that residual |
| Real graceful stop | Maker construction rejects `manage_stop = true`; through NT's real `Strategy::stop`/`Trader` lifecycle under active quoting conditions, stop returns deferred while records remain, timer/callback processing continues, zero new quote/admission/submit calls occur after the request, and `Component::stop` runs only after the last authoritatively reconciled record retires; an identity-unavailable or no-report query case remains Running and loud rather than falsely completing |
| Fee-seam deletion without collateral loss | Provider formula and replay parity tests remain; settlement dispatch, maker quote-target dispatch, and unknown-family failure tests remain after fee assertions are removed |

Existing fail-before-mutation admission and resting-refresh tests remain regression evidence. Deletion of obsolete public paths is established by compilation and reviewer diff/symbol inspection, not by testing source text.

## Final external-review closure

The final reviews exposed related boundary defects that are repaired by changing their owners and types rather than by adding local conditionals.

### Finding-to-repair map

| Originating finding | Contract owner | Implementation task | Discriminating evidence |
|---|---|---|---|
| Claude R7 M1/L2/L3: retired public batch-cancel/modify wrappers, stale quote-lifecycle authority comments, and the ambiguous test alias | One tracked-order cancellation authority | Task 8B | Removed-symbol/caller inspection plus exact-scope cancellation behavior |
| GPT workspace finding: `economics-core` tests absent from governed root CI | Governed neutral core | Task 8A | Root metadata plus workspace fmt/Clippy/nextest showing the core targets |
| Kimi H1: thin books suppress the whole risk-reducing IOC | Executable risk-reduction compilation | Task 9 | Ten-unit position compiles to a valid five-unit IOC and submits five |
| Kimi M1: exit decision/evaluation evidence disappears on preparation failure | Typed exit-attempt evidence phases | Task 9 | Preparation rejection records intent plus a typed failed preparation and no false submission linkage |
| Kimi M2: quote-only economics silently disables automatic flattening | Root-owned runtime/economics compatibility | Task 10 | Root validation rejects the incompatible configuration before runtime construction |
| Kimi M3: Polymarket `mbf`/`tbf` are modeled as a chargeable builder-fee rate | Provider-owned economics authority | Task 10 | Provider fixtures quote the platform fee only and contain no fabricated builder charge |
| Kimi lows: missing guaranteed point, duplicate stale validator, unused public admission helper, wrong non-positive-position error, and untyped gross currency | Neutral economics core | Task 11 | Focused core behavior tests plus compiler-enforced API deletion |
| Kimi low: forecast-only drift de-authorizes an unchanged resting order | Admission-authoritative resting equivalence | Task 11 | Forecast-only drift retains; each core quote/edge/binding/reservation drift cancels |
| Kimi lows: dead Polymarket cache knob, dead Hyperliquid aligned-product knob, silently waived spot-buy builder charge, incoherent Polymarket rounding pair, and weak provider numeric fixtures | Provider config and adapters | Task 10 | Load-time invalid-config tests plus numeric provider fixtures |
| Kimi lows: maker zero floor, stale entry-state comment, missing shadow-PnL negative evidence, and invalid backtest manifest fixtures | Typed maker policy and evidence/fixture cleanup | Task 11 | Negative-maker-edge rejection, shadow-PnL fail-closed test, and affected fixture suites |
| Plan review: isolated BTE workspace, residual `ExitPending`, unrepresentable `submitted=false`, pre-admission forced-reduction seal failure, quantity-lattice validity, and missing root-aware economics validator | The specific owners below | Tasks 8A, 9, and 10 | The named workspace, event-sequence, codec, shared-router, lattice, and root-validation tests below |
| Plan review: Task 7 published and requested review before later repairs | Exact-head closure authority | Task 12 | Direct plan inspection proves Task 12 is the only push/publication/review gate and every earlier checkpoint is inert |
| Claude written-spec F1/F2: Task 9 omitted the shared execution owner and the dead NT order-management contract still advertised retired batch authority | Shared execution ownership and one cancellation authority | Tasks 8B and 9 | Task file inventory plus removed-symbol/caller inspection |
| GPT written-spec F1: terminal-fill remanagement trusted a position cache with no proof that it included the fill | Causal residual-position fence | Task 9 | Projected fill leaves the position unchanged; timer waits; causally newer position truth releases exactly the residual |
| GPT written-spec F2/F3: `anyhow` erased submit-phase provenance and non-submitted outcomes had no exposure transition contract | Typed submit and exit-attempt state machines | Task 9 | Per-phase injected outcomes plus exhaustive exposure transition table, including shadow skip and synchronous callback re-entry |

The previously approved venue-identity conflict decision remains explicit: a captured `Some(A)` followed by authoritative `Some(B)` is an identity-corruption hold, not a recoverable status transition. The coordinator performs no further venue operation or retirement, preserves all prior state and health facets, continues processing siblings, and keeps graceful stop deferred. Static inspection cannot establish a production frequency for this pinned-NT transition; the safety choice is nevertheless intentional for this live-disabled slice because automatically selecting either identity can cancel or retire the wrong venue order. A process-level governed recovery is the only escape.

### Governed neutral core

The repository root is a Cargo workspace containing the root `bolt-v2` package and `crates/economics-core`. Those two packages share the root `Cargo.lock`. `crates/backtesting-vertical-slice` remains explicitly excluded because its isolated workspace and lockfile keep backtest/cloud features out of the LiveNode dependency graph. The ignored `crates/economics-core/Cargo.lock` is deleted and its `.gitignore` rule is removed; the backtesting lockfile and its verification commands remain intact.

Repository `fmt`, Clippy, and nextest commands in both the `justfile` and advisory workflow select the root workspace, so the neutral core's unit and synthetic-extension tests cannot be green locally while absent from exact-head CI. Advisory evidence must enumerate the core test targets at the new head; `cargo metadata` is only the local structural check.

The core also makes reporting currency part of gross expected value. `fold_net_edge` accepts a typed gross amount and rejects a currency that differs from the quote reporting currency. Required guaranteed components fail at the point of aggregation when their point valuation is absent. Duplicate stale-source validation and unused public admission helpers are deleted; non-positive position quantity and missing holding horizon remain distinct typed errors.

Maker entry economics use a typed breakeven core-edge policy rather than a bare `Decimal::ZERO` at the call site. Terminal-value-derived negative gross is rejected; it is never floored to zero and admitted. This is a domain policy variant, not an operator runtime value or a compatibility constructor.

### One tracked-order cancellation authority

The public `route_cancel_all` and `route_modify` convenience wrappers are deleted. The caller-less public `BoltV3NtOrderManagementContract` and `nt_order_management_contract()` census are deleted too; they must not continue advertising retired batch authority while evading dead-code lint through public visibility. Tracked maker scope cancellation remains the only production cancel-all route and fans out through the per-order coordinator. The dormant NT batch-cancel sink, its outcome enum, its trait methods, and its differential-only test are deleted as well; shadow tracked cancellation returns the typed policy skip without calling a batch sink. The private modify sink remains because maker routing uses it as the single fail-closed in-place-modify boundary. Quote-lifecycle documentation names coordinator scope rather than NT's batch API. Tests call `drive_observed_resting_order_economics` by its exact name; the ambiguous compatibility alias is deleted.

### Executable risk-reduction compilation

A shared execution compiler, not the strategy, converts a requested risk-reducing intent plus the authoritative book, configured depth bound, and shared venue/instrument quantity rules into one typed market-IOC compilation. The only supported template is `Market` + `IOC` + base quantity + not post-only, with no trigger/trailing fields. The venue `is_reduce_only` flag is passed through but is not the risk proof: the typed Bolt intent plus the canonical NT position clamp own that invariant, matching the shipped Polymarket templates where `is_reduce_only=false`. Every other non-post-only exit template fails configuration validation through this single predicate.

One shared `compile_and_seal_risk_reducing_ioc` choke point first clamps the requested order to the canonical NT position, then applies the executable-book compiler and shared venue/instrument normalization, rewrites the final `OrderAny` to the compiled quantity, derives retained fill levels and worst executable price, and seals economics from that exact order. It returns a typed compiled submission; the strategy cannot independently choose or later mutate its quantity, fills, or price. A canonical-position change discovered after compilation rejects the attempt instead of performing a second silent clamp.

The compiler returns the largest positive quantity already accepted by shared execution's venue/instrument lattice and minimum rules, retained fill levels whose quantities sum exactly to it, and the worst executable price. Full depth returns the requested quantity; thin depth returns the covered executable quantity instead of rejecting the whole reduction. Sub-increment, below-minimum, or zero-after-alignment coverage fails closed. An empty or invalid executable book remains fail-closed.

Exit evidence is a typed phase machine, not a `submitted` boolean. `ExitIntentDecisionFact` records the requested strategy decision before fallible preparation and carries no claim that an order was prepared or submitted. `ExitPreparedOrderFact` replaces the ambiguous `ExitSubmissionDecisionFact` and carries the compiler's actual final quantity plus a `PreparedOrderLinkage`, explicitly making no venue-submission claim. `ExitEvaluationFact` records the exhaustive terminal attempt outcome as `Held`, `Blocked`, `PreparationRejected { stage, reason }`, `RouteRejected { prepared_order, reason }`, `IntentEvidenceRejected { prepared_order, reason }`, `AdmissionRejected { prepared_order, reason }`, `PolicySkipped { prepared_order }`, `PreSinkRejected { prepared_order, reason }`, `SinkRejected { prepared_order, reason }`, or `Submitted { submitted_order }`. `PreparedOrderLinkage` and `SubmittedOrderLinkage` are distinct types; only the shared `Submitted` outcome can construct the latter. Illegal combinations are unrepresentable, and no separate preparation-result fact duplicates the evaluation. Facts, codecs, generated contract, fixtures, and round trips change atomically.

The shared order-execution boundary owns the matching exhaustive `BoltV3SubmitAttemptOutcome` and classifies each failure at its source. Every submit caller migrates to that single result; no compatibility `Result<BoltV3SubmitRoutingOutcome>` path remains. Edge-taker evidence maps the shared outcome exhaustively, so a clock/authority failure cannot be mislabeled as admission and a sink rejection cannot be guessed from an error string. Resting-submit registration consumes the same outcome, retaining a provisional tracked record only for `Submitted` and removing it for every other variant without collapsing their provenance.

Venue routing remains after valid intent evidence, a prepared final basis, and admission. Residual remanagement is explicit over both reduction layers. If the venue only partially fills an IOC and then cancels/expires the remainder, the existing terminal event re-reads the canonical NT position and remanages it. If a reduced-size IOC fills its entire submitted quantity (`leaves_qty == 0`) while the position remains open, the pending exit enters a typed `TerminalFillAwaitingPosition` state carrying a `PositionReductionFence`: position ID, client order ID, compiled submitted quantity, cumulative filled quantity, terminal trade/event identity, terminal event timestamp, and a pre-submit canonical position stamp containing quantity, `ts_last`, and the last position-event identity.

The fill callback, position callback, and timer all invoke one reducer. It may release the fence only when authoritative NT position state demonstrably includes the terminal fill: either the cached position contains the terminal trade ID, or its last position event is explicitly `reconciliation=true`, is at/after the terminal event, differs from the captured pre-submit event stamp, and reports quantity no greater than the checked `pre_submit_quantity - cumulative_filled_quantity` bound. A cached closed position satisfying the same causal proof is flat. Cache absence, a generic later `ts_last`, or a coincidentally smaller quantity alone is not terminal proof. A reconciliation-projected fill with `apply_position=false` therefore stays awaiting and loud while the position still shows the pre-fill stamp; timer polling cannot route another reduction from that stale value. Once the fence is satisfied, the reducer transitions to `Managed` with `ResidualRemanaged` evidence (or flat), and the next evaluation can route only the authoritative residual. Position-before-fill, fill-before-position, projected-fill-with-stale-position, unrelated later position activity, callback-free authoritative reconciliation, and duplicate reconciliation are explicit tests.

### Honest runtime and provider authority

`quote_only` has no live authoritative publisher. One loaded-config resolver owns the cross-section decision over the kill-switch block, configured execution clients, provider economics bindings, economics reporting block, and economics slice after root and strategy files are loaded. Loaded-config validation and live-node construction consume that resolver's typed result; neither duplicates its predicates or messages. `validate_root_only` retains only root/block-local checks. Startup therefore rejects `flatten_open_positions_on_breach=true` while any execution client the flatten router would select remains quote-only, and shipped configurations retain `flatten_open_positions_on_breach=false` atomically.

Provider-neutral economics validation is also root-owned: `validate_clients_block(root)` loads each configured provider economics block through the provider registry and invokes `ExecutionEconomicsConfig::validate_common` with `root.economics.reporting` before runtime binding, including for unselected clients. Provider-local validators remain responsible only for provider-shaped fields.

The shared kill-switch flatten router owns pre-admission sealing failures. If `build_order_economics_submit_admission` fails, it derives admission details from the already compiled final order and calls one submit-admission evidence API that records exactly one `ForcedReductionAdmissionFact` with a typed `EconomicsSealRejected` reason before returning. No admission counter, reservation, or venue sink mutates. Callers and live-node wiring do not duplicate this evidence path.

Polymarket models only authoritative platform fees in Slice 1. Market `mbf`/`tbf` metadata does not create a Bolt builder charge, and the builder component/config fixtures are deleted. Fee rounding and sub-quantum behavior are validated as one coherent pair, and provider economics configuration is validated during config loading rather than deferred until scope construction. Hyperliquid drops unused aligned-product policy inputs, and an attached builder charge on an unsupported spot-buy shape fails closed instead of being silently waived. Dead provider cache knobs are removed from schema, shipped TOML, and fixtures.

Forecast-only drift is diagnostic state, not admission authority: resting-order equivalence compares the core quote, core edge, binding, and reservation terms that authorized the order. A successful refresh replaces the stored admission with the refreshed forecast fields and returns a typed forecast-drift diagnostic without forcing cancellation. The equivalence test is paired with negative controls proving that each core quote, core edge, binding, and reservation change still produces the fail-closed refresh outcome. Shadow-PnL and manifest fixtures gain fail-closed behavioral coverage for their newly introduced contracts.

## Non-goals

This repair does not implement actual economics ledgers, supplemental venue actuals, lifecycle or transfer actuals, reporting closure, live economics publication, or live execution. Those remain outside Slice 1. It also does not claim retry durability across process death or forced process termination.
