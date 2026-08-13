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

A focused `tracked_order_economics` module owns the public `BoltV3OrderEconomicsHandle` itself and the complete tracked-maker aggregate, not merely an inner field stored by the parent execution module. The parent module re-exports the handle but cannot name or access its fields. A handle clone shares the same private aggregate; no clone or constructor can produce a partial record. Inside the module, one opaque registry owns the lock, map, checked registration generation, monotonic typed `RestingRegistryHealth`, `TrackedMakerOrderRecord`, optional resting economics, query seed, and optional cancellation intent. The exhaustive cancellation reducer remains a subordinate private module rather than absorbing registration and economics responsibilities into a cancellation monolith.

Outside `tracked_order_economics`, code never receives `&mut TrackedMakerOrderRecord`, a mutable callback over the registry, an aggregate constructor, a registration guard, or a constructor for a partially initialized record. Its only interfaces are semantic operations: construct a complete handle from bound economics, quote economics, route a resting submit transaction, refresh economics from an authoritative cache observation, request one cancellation, request an instrument/side scope, reconcile one NT callback, drive all tracked orders from timer-owned cache observations through `drive_all_resting_order_economics_at_ms`, drive exactly the observations selected by a cancellation origin through `drive_observed_resting_order_economics`, inspect read-only IDs/health, and test whether draining is complete. The all-orders and exact-observation operations are distinct APIs: an empty exact observation set is a no-op and can never mean all tracked orders.

The resting-submit transaction extends the typed route authority; it is not a fallible adapter around it. `BoltV3SubmitAttemptOutcome` remains the sole route result and contains only route validation, intent evidence, admission, policy skip, pre-sink, sink rejection, or submission. `BoltV3RestingSubmitTransactionOutcome` is the sole result for a resting transaction and is an exhaustive phase wrapper: `RegistrationRejected { reason }`, `Attempt(BoltV3SubmitAttemptOutcome)`, or `RollbackInvariantFailed { original: BoltV3RoutedNonSubmittedOutcome, reason }`. `BoltV3RoutedNonSubmittedOutcome` is an opaque refinement that owns the original `BoltV3SubmitAttemptOutcome`; it has no second discriminant or independently constructible variants, and shared execution can construct it only from the exhaustive non-`Submitted` branch. This composition prevents direct taker and kill-switch callers from handling impossible resting-only branches while preserving one route-classification authority.

Before routing, the resting owner validates the single positive leg, acquires the registry, rejects duplicate client IDs, increments the checked private registration generation, and inserts a provisional record. Invalid shape/quantity, duplicate ID, initial poison, and generation overflow produce `RegistrationRejected`; they cannot escape as `anyhow` and create no new provisional record. The private `RestingRegistrationTransaction { client_order_id, generation }` then releases the lock before invoking shared routing without handing registry internals to the closure. `Attempt(Submitted)` commits only that generation. For a routed non-submission, the transaction aborts only its generation; exact rollback returns `Attempt(original_route_outcome)` unchanged. Only when cleanup cannot prove removal or authoritative retirement does shared execution move that same route outcome into the opaque `BoltV3RoutedNonSubmittedOutcome` and return `RollbackInvariantFailed { original, reason }`. Absence is accepted only when a synchronous authoritative callback already retired the same generation; a different generation is never removed. The cleanup-only path recovers a poisoned write guard solely to remove the owned generation and sets monotonic `RestingRegistryHealth::Poisoned`. A private drop backstop performs the same generation-scoped removal and cannot leave this transaction's provisional record behind. Synchronous NT callbacks can therefore reconcile or retire the record without deadlocking, no external sink is called while the registry lock is held, and duplicate/precondition/rollback failures stay within the one resting-transaction result. Registration, refresh, intent merging, terminal removal, and rollback execute inside the owner. This makes registration provenance, generation, deadline, backoff, query identity, record lifetime, and drive scope compiler-owned as one aggregate rather than replaceable at a parent call site.

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

The shared submit boundary returns one opaque, private-field `BoltV3SubmitAttemptOutcome`; it no longer erases routing provenance behind `anyhow::Result<BoltV3SubmitRoutingOutcome>`. Its exhaustive public discriminant distinguishes route validation, order-intent evidence, admission, policy skip, pre-sink validation, sink rejection, and submission at the point where each outcome is produced. Every rejection carries a stable typed reason plus diagnostic context, and only shared order execution can construct a route outcome or submitted linkage. Direct taker and kill-switch callers preserve this type unchanged. Resting maker routing preserves it unchanged inside `BoltV3RestingSubmitTransactionOutcome`; callers match only the variants possible for their phase. String parsing, error downcasts, submit-shaped `Result<()>` adapters above the raw NT mutation sink, and unconditional caller-side construction of `Submitted` are forbidden. The raw `BoltV3NtVenueMutationSink::submit_order_via_nt -> Result<()>` remains the one leaf operation and is immediately converted by shared execution to `SinkRejected` or `Submitted`.

Maker dispatch nests `BoltV3RestingSubmitTransactionOutcome` inside a submit-specific `MakerOrderDispatchOutcome`; only `Attempt(Submitted)` rotates the maker leg identity. `MakerOrderCommandSink::submit_maker_order` returns the resting transaction outcome rather than `Result<()>`, and maker runtime does not place any submit outcome in its diagnostic `routing_error: String` channel. Pre-route maker build failures and cancel/modify failures use a separate stable typed command-failure phase with diagnostic text that is never parsed as submit provenance. Kill-switch fan-out returns the shared route outcome per command plus a typed aggregate report; the live-node executor matches every route variant and counts only `Submitted` as successful venue submission. Every other route or resting-transaction variant remains intact until the outer operator-health/log boundary and cannot be collapsed through `?` into `Ok(())`.

| Shared phase | Typed outcome | Mutation contract |
| --- | --- | --- |
| Resting leg/registry/duplicate validation | `BoltV3RestingSubmitTransactionOutcome::RegistrationRejected` | No route, admission, sink, or new provisional record |
| Route clock/binding/authority validation | `RouteRejected` | No admission, sink, or tracked registration |
| Required order-intent evidence | `IntentEvidenceRejected` | No admission, sink, or tracked registration |
| Live or shadow admission evaluation | `AdmissionRejected` | No committed counter/reservation and no sink |
| Successful shadow evaluation | `PolicySkipped` | Prepared order only; no submitted linkage, sink, pending exposure, or tracked registration |
| Live pre-sink clock/margin validation | `PreSinkRejected` | Admission permit drops and rolls back; no sink or tracked registration |
| NT submit call returns failure | `SinkRejected` | Admission permit and provisional registration roll back |
| NT submit call returns success | `Submitted` | Permit and submitted/tracked state commit |
| Routed non-submission inside a resting transaction | `BoltV3RestingSubmitTransactionOutcome::Attempt(BoltV3SubmitAttemptOutcome)` | Original route provenance is unchanged while exact-generation rollback succeeds |
| Routed non-submission cleanup cannot prove exact-generation rollback | `BoltV3RestingSubmitTransactionOutcome::RollbackInvariantFailed { original: BoltV3RoutedNonSubmittedOutcome, reason }` | Original route provenance remains typed; no recursion, wrong-generation removal, or successful-submission claim |

1. Record a strategy decision-intent fact that claims no prepared or submitted order.
2. Build and validate the purpose-typed final basis and a typed prepared-order linkage.
3. Capture `route_now_ns`; validate final order binding, execution authority, purpose, and remaining lifetime.
4. Record valid order intent.
5. Evaluate admission with that same explicit time.
6. For live routing, capture `pre_sink_now_ns`, revalidate remaining lifetime, call NT, then commit the permit and resting registration.

A step-2 failure may retain the valid strategy decision-intent fact, but leaves prepared/submission evidence, exposure, counters, registrations, and venue state unchanged. Failures before a live sink leave exposure `Managed`. A step-6 failure drops the permit and generation-scoped registration transaction, restoring counters/reservations and leaving no provisional record owned by that attempt. A rollback invariant failure remains a non-submission and preserves its original typed route outcome while making registry health loud. Cancellation errors advance only checked attempt/backoff diagnostics, never NT order status, and cannot starve siblings.

Exit exposure uses one exhaustive attempt and order-authority state machine. A prepared local exit arms `ExitAttempting { generation, prior_managed, prepared_order, local_exit_authority }` before shared routing. `LocalExitAuthority` contains the original signed position, exact position key, compiled quantity, submitted-order identity, and an opaque position-authority lease acquired through shared execution before any live sink can synchronously publish a fill. `RouteRejected`, `IntentEvidenceRejected`, `AdmissionRejected`, `PolicySkipped`, `PreSinkRejected`, and `SinkRejected` restore `Managed` and drop the lease only when the same generation is still attempting. `Submitted` alone commits `ExitPending { authority: LocallySubmitted(..) }` and carries that same authority forward. A synchronous NT callback may advance the state while the sink call is in flight; the callback consumes or transfers the authority, and the generation-checked return path cannot overwrite that newer state or drop its lease. `PolicySkipped` retains only typed prepared-order evidence, never an actual submitted-order linkage or pending-exit identity.

One shared recovery constructor is the only other `ExitPending` constructor. It serves `RecoveredExitCause::{StartupAdoption, FillVoidReopen}` and must acquire the same exact-key lease, retain the attributed cached exit `OrderAny` including cumulative filled/voided quantity and the complete effective fill-ID set, and construct `ExitPending { authority: Recovered(..) }` with the adopted canonical signed quantity as a conservative ceiling. Its baseline is exhaustive: `AwaitingAuthoritativeBaseline` or `CoherentBaseline { report_generation, signed_quantity, cumulative_order_fills }`. A post-recovery exact-key raw report that matches the canonical cache establishes the coherent baseline; effective fills already present at that point are part of the baseline and only later target-order fill deltas reduce it. If exact exit attribution, lease acquisition, fill/correction snapshot, or position identity is unavailable, recovery drops any partially acquired lease and enters typed `ExitAuthorityRecoveryHold`. Its timer reducer may retry construction only from fresh authoritative cache/feed observations; it cannot route another reduction or silently fall back to `Managed`. The hold can prove flat only after acquiring a fresh exact-key lease and observing a coherent flat raw report that exactly matches the canonical cache; cache absence, `PositionClosed`, or a local timestamp alone is insufficient. No `ExitPending` value can exist without `LocallySubmitted` or `Recovered` authority and a live lease.

One lifecycle reducer owns `Filled`, `Canceled`, `Expired`, `Rejected`, `Denied`, cached `Voided`, and `OrderFillVoided` correction for both origins. It re-reads the authoritative cached target order and classifies the complete cumulative effective-fill state, not the event name. A partial `Filled` observation with positive leaves only updates the retained authority's cumulative fill snapshot and remains `ExitPending`. A fill-void correction advances the position-proof floor and recomputes effective fills. If it reopens a working exit, the same authority remains or returns to `ExitPending` and no new order routes; if the corrected order remains terminal, it enters or remains `TerminalExitAwaitingPosition`. Any authority observed through fill void cannot use a zero-fill or fill-ID-only proof after that correction; it requires a strictly post-correction exact-key report/cache proof because the position correction may itself have been projected. If a running-state fill void arrives after the prior exit already returned to `Managed` or `Flat`, the same recovery constructor uses the attributed cached exit order or enters `ExitAuthorityRecoveryHold`; there is no correction-specific route. Post-stop callbacks are not claimed, and next-start recovery uses the same constructor.

Once an uncorrected cached order is terminal or leaves are zero, only authoritative zero cumulative filled quantity and an empty fill-ID set may return directly to `Managed` or `Flat`. Any positive or unknown cumulative fill, including partial-fill-then-cancel/expire and every recovered terminal without a coherent zero-fill proof, enters `TerminalExitAwaitingPosition` with the retained authority lease. Position opened/changed/closed callbacks are reconciliation triggers into the same reducer; `PositionClosed` cannot set `Flat` while a pending/fenced/recovery-held exit exists without satisfying the same causal proof. A locally submitted fence derives its signed residual bound from the original position and every target-order fill. A recovered fence with a coherent baseline derives the bound from that baseline and only post-baseline target fill deltas; without a baseline or after fill void it can release only through a strictly post-terminal-or-correction exact-key report/cache proof within the adopted same-side ceiling. Thus cache rereads, position events, startup adoption, and fill-void recovery are not compatibility paths around causal position authority.

| Strategy phase/outcome | Exposure transition |
| --- | --- |
| `Held`, `Blocked`, or `PreparationRejected` | Remain `Managed`; no attempt generation exists |
| Prepared order | `Managed -> ExitAttempting(generation, LocalExitAuthority)` |
| Any non-submitted shared outcome for the same generation | `ExitAttempting -> Managed` |
| `Submitted` for the same generation | `ExitAttempting -> ExitPending(LocallySubmitted)` |
| Synchronous order/position callback advances or retires the generation | Callback result wins; stale route return is a no-op |
| Startup adoption or running fill-void recovery with exact authority | `Managed` or `Flat -> ExitPending(Recovered)` |
| Startup adoption or running fill-void recovery without exact authority | `Managed` or `Flat -> ExitAuthorityRecoveryHold` |
| Fill void for retained authority | Recompute effective fills; reopened working order stays/returns `ExitPending`, terminal correction stays/enters the fence, and no new order routes |
| Proven zero-fill terminal | `ExitPending -> Managed` or `Flat` |
| Positive- or unknown-fill terminal of any kind | `ExitPending` or `ExitAttempting -> TerminalExitAwaitingPosition` |
| Satisfied position fence | `TerminalExitAwaitingPosition -> Managed` residual or `Flat` |

## Behavioral evidence matrix

Tests are behavioral; no source-scanning test is added.

| Requirement | Discriminating evidence |
| --- | --- |
| Every live economics caller is purpose-typed | Edge candidate sizing, edge final entry, maker final entry, and planned exit each assert derived intent, lifecycle, role, value, and admission purpose; cancel-only maker actions require no scenario, and quote-only startup rejects live forced reduction before runtime construction |
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
| Typed submit provenance | Each route phase is failed independently and produces its exact `BoltV3SubmitAttemptOutcome` without string parsing or downcasts. Resting invalid shape/quantity, duplicate ID, initial poison, and checked-generation overflow produce `RegistrationRejected`, never route, and add no provisional record. Every routed non-submission returns as `Attempt(original)` after removing only its provisional generation; synchronous callback removal is accepted; poison sets monotonic registry health; a conflicting generation produces `RollbackInvariantFailed` carrying the sealed original route outcome without removing a sibling or replacement. Prepared linkage, counters/reservations, sink calls, and exposure state are asserted at every phase. Maker shadow routing preserves nested `Attempt(PolicySkipped)`, creates no submitted leg identity, and permits a later attempt; no live kill-switch submit adapter exists in quote-only economics |
| Exit-attempt state authority | A table-driven test covers every attempt outcome. All non-submitted outcomes restore `Managed`; only `Submitted` commits `ExitPending`. A shadow-policy skip records prepared-only evidence, performs no venue/capacity mutation, leaves exposure `Managed`, and permits a later eligible evaluation. A synchronous callback that advances the generation cannot be overwritten by route-return rollback |
| Exhaustive exit-order authority | Direct submit and one recovered-exit constructor for startup adoption or running fill-void reopen are the only `ExitPending` constructors; both require a non-optional exact-key lease. Failed recovery drops partial authority, enters loud `ExitAuthorityRecoveryHold`, routes nothing, and can prove flat only through a newly leased exact-key coherent flat report/cache match. Filled, canceled, expired, rejected, denied, cached-voided, and fill-void observations all use one reducer over authoritative cumulative effective fills; position opened/changed/closed events are triggers into that reducer, not alternate state authority. A proven zero-fill terminal with no correction remanages directly; positive, unknown, or corrected fills enter `TerminalExitAwaitingPosition`. Projected partial-fill then cancel/expire, fully filled reduced IOC, recovered partial terminal, startup without an authoritative baseline, projected fill void before or after terminal release, and `PositionClosed` racing any of them cannot use the old cache-reread/flat path. Each waits for complete fill application when allowed or an exact post-terminal/correction report/cache proof, then a later evaluation routes only the proven residual |
| Position-authority key isolation | Two simultaneous leases under one execution client/account but different instruments have distinct `BoltV3PositionAuthorityKey` values, snapshots, generations, health, and teardown. Equal timestamps or different signed quantities across instruments neither evict nor conflict; each report releases only its own fence. Hedging leases also distinguish venue-position IDs, while ambiguous account-to-client attribution fails composition |
| Position-report identity at the pin | Two successive changed `PositionStatusReport::new(..., None, ...)` values receive distinct generated UUID4 IDs at pinned NT and advance the same key normally. An identical concrete ID/body dedupes; the same concrete ID with different body conflicts. No fabricated absent/default-ID identity path exists |
| Terminal/correction position authority | A reconciliation-projected terminal or fill-void event that updates the order but not the position enters or remains `TerminalExitAwaitingPosition`; an immediate timer observes no permitted fill-set proof and no post-event position-report proof, so it performs no remanagement or new reduction. Mixed projected/applied fills, an empty post-void fill set, an unrelated reconciliation fill, a side flip, a stale or pre-event raw report, feed conflict, and cache/report disagreement remain awaiting and loud. An uncorrected authority may release through all required target-order fill IDs plus the origin-specific signed residual bound; every authority may release through a strictly post-terminal/correction raw NT position report plus exact cache/report equality and the same bound. A later timer without a strategy callback then remanages only the proven residual. Reports without a lease are discarded, last-lease drop clears exact-key state, and LiveNode restart leaves exactly one subscription |
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
| GPT written-spec F1: terminal-fill remanagement trusted a position cache with no proof that it included the fill | Shared residual-position authority | Task 9 | Projected fill leaves the position unchanged; timer waits; complete fill-set or a post-terminal raw-report/cache match releases exactly the bounded residual |
| GPT written-spec F2/F3: `anyhow` erased submit-phase provenance and non-submitted outcomes had no exposure transition contract | Typed submit and exit-attempt state machines | Task 9 | Per-phase injected outcomes plus exhaustive exposure transition table, including shadow skip and synchronous callback re-entry |
| Claude R10: report leases were keyed at account/venue granularity while release required instrument/position identity | Exact per-position authority key | Task 9 | Concurrent same-account/different-instrument leases remain isolated and release independently |
| GPT R10 F1: only locally submitted full-fill exits were fenced; partial terminals and restart-adopted exits retained cache-trusting paths | Exhaustive exit-order origin and terminal reducers | Task 9 | Every constructor and terminal kind is table-driven; projected partial and recovered terminals cannot remanage until causality is proven |
| GPT R10 F3: resting registration and rollback lived outside typed submit provenance | One typed submit transaction with generation-scoped provisional ownership | Task 9 | Invalid/duplicate registration, every non-submitted rollback, callback retirement, and generation conflict preserve exact typed provenance and leave no owned provisional record |
| GPT R10 F2: Polymarket `report_id=None` was claimed to produce one reused default ID | Pinned NT concrete report identity | Task 9 | Direct pin inspection shows `unwrap_or_default()` calls `UUID4::default()`, which generates a fresh UUID4; successive `None` reports advance without a fallback identity path |
| GPT R10 F1 and Claude R9 residual: maker/live-node adapters remained able to erase the shared typed submit outcome | End-to-end submit-attempt transport | Task 9 | Maker shadow-policy retains its exact variant; the unsupported quote-only live-node flatten submit adapter is deleted rather than adapted |
| GPT R10 F2: event-local position markers did not prove aggregate target-order fills or authoritative position-report state | Shared residual-position authority | Task 9 | Mixed projected/applied fills and unrelated reconciliation remain fenced; complete fill-set or post-terminal raw-report/cache agreement releases within the signed bound |
| Internal adversarial closure: terminal-callback lease creation could miss a synchronous fill/report, while an unbounded report cache or replay could fabricate later authority | Bounded position-authority lease and generation floor | Task 9 | Lease precedes the sink; only a strictly post-event generation releases; conflict, replay, teardown, and restart tests pin bounded single-subscription behavior |
| Internal adversarial closure: `OrderFillVoided` could project an order correction without position application before or after the prior fence retired | The same recovered-exit constructor and causal position reducer | Task 9 | Working reopen stays pending; terminal or late projected correction cannot use empty fill IDs/cache absence and releases only after an exact post-correction report/cache proof |
| Final external review: kill-switch forced reduction bypassed the shared IOC compiler and position fence | Quote-only runtime boundary | Task 10 final closure | Config load rejects automatic flattening and the live flatten executor, submit-only sink, route, secondary capital-snapshot clamp, and route-only helpers are deleted; symbol/caller inspection and the config rejection test prove no bypass remains |
| Final external review: edge strategy reconstructed canonical quantity, side, fill IDs, and OMS scope independently | Shared position-authority capability | Task 9 final closure | The capability returns one sealed canonical snapshot plus lease; ambiguous netting fails before sealing, and a ten-unit snapshot compiling a five-unit IOC later releases an eight-unit residual after a two-unit fill |

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

One shared `compile_and_seal_risk_reducing_ioc` choke point acquires a sealed canonical-position snapshot and report lease from `BoltV3PositionAuthorityCapability`, rejects ambiguous netting scope, clamps the requested order to that snapshot, applies the executable-book compiler and shared venue/instrument normalization, rewrites the final `OrderAny` to the compiled quantity, derives retained fill levels and worst executable price, and seals economics from that exact order. The capability—not the strategy—derives signed quantity, side, complete trade IDs, and OMS-dependent target scope from NT cache state and configured OMS type. The compiler returns the typed compiled submission together with that same sealed snapshot and lease, so the local exit fence cannot substitute submitted quantity for the pre-submit position baseline. The strategy cannot independently choose or later mutate quantity, fills, price, scope, or baseline. A canonical-position change discovered after compilation rejects the attempt instead of performing a second silent clamp.

The compiler returns the largest positive quantity already accepted by shared execution's venue/instrument lattice and minimum rules, retained fill levels whose quantities sum exactly to it, and the worst executable price. Full depth returns the requested quantity; thin depth returns the covered executable quantity instead of rejecting the whole reduction. Sub-increment, below-minimum, or zero-after-alignment coverage fails closed. An empty or invalid executable book remains fail-closed.

Exit evidence is a typed phase machine, not a `submitted` boolean. `ExitIntentDecisionFact` records the requested strategy decision before fallible preparation and carries no claim that an order was prepared or submitted. `ExitPreparedOrderFact` replaces the ambiguous `ExitSubmissionDecisionFact` and carries the compiler's actual final quantity plus a `PreparedOrderLinkage`, explicitly making no venue-submission claim. `ExitEvaluationFact` records the exhaustive terminal attempt outcome as `Held`, `Blocked`, `PreparationRejected { stage, reason }`, `RouteRejected { prepared_order, reason }`, `IntentEvidenceRejected { prepared_order, reason }`, `AdmissionRejected { prepared_order, reason }`, `PolicySkipped { prepared_order }`, `PreSinkRejected { prepared_order, reason }`, `SinkRejected { prepared_order, reason }`, or `Submitted { submitted_order }`. `PreparedOrderLinkage` and `SubmittedOrderLinkage` are distinct types; only the shared `Submitted` outcome can construct the latter. Illegal combinations are unrepresentable, and no separate preparation-result fact duplicates the evaluation. Facts, codecs, generated contract, fixtures, and round trips change atomically.

The shared order-execution boundary owns opaque `BoltV3SubmitAttemptOutcome`, its exhaustive `BoltV3SubmitAttemptKind`, and every constructor that classifies a route result at its source. No compatibility `Result<BoltV3SubmitRoutingOutcome>` or submit-shaped `Result<()>` path remains above the one raw NT sink leaf. Edge-taker evidence maps that shared outcome exhaustively, so a clock/authority failure cannot be mislabeled as admission and a sink rejection cannot be guessed from an error string. Resting registration wraps the unchanged route outcome in `BoltV3RestingSubmitTransactionOutcome`, owns its registration and rollback variants, retains a provisional record only for `Attempt(Submitted)`, and preserves the original routed variant on every rollback. Maker dispatch carries that exact transaction outcome through its per-leg runtime result and rotates identity only for `Attempt(Submitted)`. Quote-only economics has no live kill-switch submit adapter. This is one nested phase composition, not two route authorities.

Venue routing remains after valid intent evidence, a prepared final basis, and admission. Residual remanagement is explicit over compilation and every order-lifecycle trajectory. `ExitOrderAuthority` has exactly two variants: `LocallySubmitted`, created by the generation-checked submit reducer, and `Recovered { cause: StartupAdoption | FillVoidReopen, .. }`, created only by one shared constructor after exact cached-exit attribution and lease acquisition. Every `ExitPending` contains one variant; there is no optional lease or cache-trusting legacy state. Recovery snapshots the complete cached order, cumulative effective fills/corrections, adopted signed ceiling, and an exhaustive baseline state. Failure to construct that authority enters typed `ExitAuthorityRecoveryHold`, which cannot infer flatness from cache absence or a position callback.

The same lifecycle reducer consumes every exit `Filled`, `Canceled`, `Expired`, `Rejected`, `Denied`, cached `Voided`, and `OrderFillVoided` observation together with the authoritative cached order. A partial fill with positive leaves updates authority and stays pending. A fill void recomputes effective fills and advances the proof floor; a working reopen stays/returns pending, while a terminal correction stays/enters the fence. Once terminal, only a complete zero-fill proof with no later correction bypasses fencing. Any positive, unknown, or corrected cumulative fill enters `TerminalExitAwaitingPosition` carrying a `PositionReductionFence`: exact authority key, order identity, origin, relevant baseline, compiled/adopted quantity ceiling, cumulative effective filled quantity, complete target-order fill/correction identities, terminal-or-correction timestamp, the already-active raw-position-authority lease, and its coherent observation generation captured as the proof floor. Locally submitted bounds start from the pre-submit signed position. Recovered bounds start from a coherent post-recovery baseline and subtract only later effective target fills; if terminality or correction precedes a baseline, only a strictly post-event report/cache proof within the adopted same-side ceiling can release. Checked signed arithmetic derives the maximum same-side residual; a side flip is never inferred to be a risk-reducing residual.

One shared `BoltV3PositionAuthorityFeed`, owned and subscribed by LiveNode and exposed through an opaque strategy-context capability, consumes pinned NT's raw `PositionStatusReport` topic. `BoltV3PositionAuthorityKey` is exact: execution-client ID + account ID + instrument ID, plus the target venue-position ID in hedging mode. In netting mode the absent venue-position ID is part of the key and composition requires one target position for that account/instrument. Loaded bindings provide only execution-client/account/venue attribution; the armed exit supplies instrument and target-position identity. Venue is validated during attribution but is not substituted for instrument in the reducer key. Shared execution begins a keyed lease when the exit attempt is armed, before any live sink call; reports for keys with no active lease are discarded. Reports on another instrument under the same account cannot replace, conflict with, or release this key.

For active keys the feed stores only the latest coherent report's exact key, signed quantity/side, concrete report ID, `ts_last`, `ts_init`, and a checked local observation generation. At pinned NT, `PositionStatusReport::new(..., None, ...)` uses `UUID4::default()`, which generates a fresh UUID v4; the feed therefore never invents an absent/default-ID compatibility identity. A private exhaustive per-key reducer admits the first report after lease creation, dedupes an identical concrete report ID/body without advancing generation, treats a lower `ts_last` as typed stale health without changing authority, enters a monotonic typed conflict hold on the same concrete report ID with different contents or equal-`ts_last` reports with conflicting signed state, and admits a distinct coherent equal-or-newer snapshot with one checked generation increment. Checked-generation overflow also enters the conflict hold. A conflicted lease can never satisfy either release proof. Dropping or resolving a lease removes its interest, and dropping the last lease for a key deletes the stored snapshot and health, so the feed cannot become an unbounded historical position cache. Its RAII subscription guard unsubscribes during LiveNode teardown and restart tests prove no duplicate handler remains.

This is observation of NT/provider reconciliation, not a second position implementation: NT cache remains the only order/position state, the feed cannot mutate it, and the existing `PolymarketNtExecutionReconciliation` boundary-registry row remains the provider-byte authority. Loaded execution-client account/venue bindings derive unambiguous client attribution; the active exit completes the exact report key with instrument and hedging position identity. Ambiguous attribution or target position fails at composition/adoption rather than selecting one. The feed subscription is registered once before strategy contexts are built, shared execution owns lease creation and the release predicate, and strategy code owns only its local exposure-state transition.

The fill, terminal-order, position, and timer callbacks all invoke one shared terminal-exit reducer. Its release stage has exactly two sufficient proofs, and both apply the origin-specific checked signed residual bound and same-side-or-flat rule:

1. **Complete fill application:** when the origin has a coherent local or recovered baseline, the canonical NT position contains every target-order fill trade ID required after that baseline, not merely the terminal fill ID, and its signed quantity is no larger than the derived residual bound. `AwaitingAuthoritativeBaseline` cannot use this proof.
2. **Post-event venue position authority:** the active, non-conflicted lease observes a coherent raw `PositionStatusReport` at a generation strictly newer than the terminal/correction proof floor, matching its exact `BoltV3PositionAuthorityKey`; the report's venue `ts_last` is at or after the latest terminal or fill-void event, its signed quantity is same-side-or-flat and within the local residual bound or recovered adopted ceiling, and the canonical NT cache for that key exactly equals the report's signed quantity and side. A hedging report must carry the target venue-position ID. A netting report without one is sufficient only when the cache contains exactly one position for the report's account/instrument and that position is the fenced target; ambiguous multi-position aggregation remains awaiting and loud.

A cached closed position is flat only through one of those same proofs. Cache absence, one matching trade ID, a generic later timestamp, an `OrderFilled.reconciliation` flag, a coincidentally smaller quantity, raw-report arrival without cache agreement, cache agreement with a report at or before the terminal/correction generation floor, or any conflicted lease is insufficient. Thus a two-fill order with one projected fill and one normally applied terminal fill remains fenced even though the position contains the terminal trade ID; a projected partial fill followed by cancel/expire does not trust the terminal cache reread; a recovered order without a baseline does not trust its boot/correction cache; a projected fill void cannot use the now-empty fill set as flat proof; an unrelated reconciliation fill cannot release either; and a side flip remains loud. Once either permitted proof holds, the reducer transitions to `Managed` with `ResidualRemanaged` evidence (or flat), and the next evaluation can route only the proven residual. Position-before-fill, fill-before-position, mixed projected/applied fills, partial-fill terminal kinds, startup adoption, fill-void before and after terminal release, unrelated reconciliation, stale/new raw reports, report/cache disagreement, callback-free cache convergence after a new report, multi-instrument isolation, side flip, duplicate observations, feed conflict, and lease teardown/restart are explicit tests.

### Honest runtime and provider authority

`quote_only` has no live authoritative publisher and therefore no live forced-reduction route. One loaded-config validator owns the cross-section decision after root and strategy files are loaded; `validate_root_only` retains only root/block-local checks. Startup rejects `flatten_open_positions_on_breach=true` unconditionally for the only supported economics slice, and shipped configurations retain `flatten_open_positions_on_breach=false` atomically. LiveNode contains no flatten executor, command fan-out, submit-only sink, forced-reduction route, or secondary capital-admission clamp. The kill-switch still moves NT to `Reducing`; planning/proof types remain inert and cannot submit.

Provider-neutral economics validation is also root-owned: `validate_clients_block(root)` loads each configured provider economics block through the provider registry and invokes `ExecutionEconomicsConfig::validate_common` with `root.economics.reporting` before runtime binding, including for unselected clients. Provider-local validators remain responsible only for provider-shaped fields.

Because quote-only startup rejects automatic flattening before runtime construction, no forced-reduction order reaches economics sealing or admission. The former live flatten router and its seal-rejection adapter are deleted together; retaining either would be a dormant second execution authority.

Polymarket models only authoritative platform fees in Slice 1. Market `mbf`/`tbf` metadata does not create a Bolt builder charge, and the builder component/config fixtures are deleted. Fee rounding and sub-quantum behavior are validated as one coherent pair, and provider economics configuration is validated during config loading rather than deferred until scope construction. Hyperliquid drops unused aligned-product policy inputs, and an attached builder charge on an unsupported spot-buy shape fails closed instead of being silently waived. Dead provider cache knobs are removed from schema, shipped TOML, and fixtures.

Forecast-only drift is diagnostic state, not admission authority: resting-order equivalence compares the core quote, core edge, binding, and reservation terms that authorized the order. A successful refresh replaces the stored admission with the refreshed forecast fields and returns a typed forecast-drift diagnostic without forcing cancellation. The equivalence test is paired with negative controls proving that each core quote, core edge, binding, and reservation change still produces the fail-closed refresh outcome. Shadow-PnL and manifest fixtures gain fail-closed behavioral coverage for their newly introduced contracts.

## Takeover review closure: governed exit-exposure authority

The takeover census of the reviewed head established that exit-lifecycle state
is representational but ungoverned: forty-five direct exposure assignments with
no transition authority, three divergent occupancy projections answering the
same policy question, optional strategy-local identity duplicating the sealed
authority handle, and unconditional adoption of NT position truth with a
cardinality-only safety check. Every repair below changes an owner or a type;
none adds a local conditional.

### Finding-to-repair map (takeover round)

| Finding | Contract owner | Task | Discriminating evidence |
|---|---|---|---|
| H1: exit evaluation gates on `managed_position` + `exit_pending_snapshot`, neither of which recognizes `ExitAuthorityRecoveryHold`; reachable from cold start via restart-adoption recovery failure | One occupancy authority at every boundary | Task 13 | Hold + each exit trigger yields a typed occupied rejection; the startup-created hold is covered by the same test row |
| H2: terminal release silently returns false on absent optional strategy-local identity while the sealed handle carries it; second repro via recovered `pending_exit` never back-filled | Sealed-handle-only identity | Task 13 | The optional duplicate fields are deleted, so the silent branch is unrepresentable; release with handle-only identity succeeds |
| H3: an `OrderFillVoided` for an untracked order force-enters a recovery hold, destroying the active exit/entry state | Governed reducer + typed quarantine | Task 13 | A stale void for a foreign client-order ID preserves the active state and records a typed quarantined outcome; the active order's later events still reconcile |
| Census: `materialize_position_from_truth` catch-all collapses six states into `Managed` for the incoming event's position with no identity continuity check; fed by a cardinality-only NT projection | Identity-checked adoption transition | Task 13 | A position-truth event naming a different position while `Managed`/`BlindRecovery` yields a typed identity conflict, not a silent swap |
| Census: `PositionClosed` for an untracked position is a total silent no-op | Evidence-recording unknown-event transition | Task 13 | The untracked close records typed evidence and health |
| Census: `BlindRecovery` has no governed exit; any tradable position-truth event silently clears the quarantine | Governed re-bootstrap transition | Task 13 | Quarantine clears only through an identity-matched re-bootstrap; a foreign event leaves it in place, loudly |
| M6: exit-to-flat release records no terminal lifecycle evidence (unlike the residual and recovery-flat siblings) | One terminal transition records evidence | Task 13 | Flat release emits terminal evidence; the negative control proves the residual path still does |
| L12: fill-void recovery stamps the strategy's active market ID instead of the recovered position's lifecycle market | Position-lifecycle-derived identity | Task 13 | A void arriving after market roll arms cooldown on the position's market |
| Census: the submit generation check runs only after routing, so overlapping evaluations can route twice | Generation check inside the submit reducer, before the sink | Task 13 | Two overlapping evaluations produce one routed order and one typed stale-generation outcome |
| H4: valuation routes and TOML can construct only `Currency` origins while Hyperliquid spot-BUY protocol fees are `Asset`-denominated, so every spot BUY fails admission | Kind-tagged valuation origins | Task 14 | Spot BUY admission passes end-to-end through runtime-built routes; an unknown origin kind fails config load |
| M5 class: resting-refresh equivalence divides every component by order-leg quantity regardless of `EconomicScope` (instance unreachable today: the `TradingEdge` gate implies `TerminalValueEntry`, which pins `position: None`) | Scope-bound comparison basis | Task 14 | A `PositionInterval` component with unchanged position passes refresh under a partial fill; a changed position fails it (paired negative control) |
| M7: exit-vs-hold timing is fee-blind at the strategy and ungated for `RiskReduction` at admission; the admission comment claims otherwise | Shared economics seals a fee-aware exit-vs-hold result; explicit `RiskReduction` policy | Task 14 | A fee-bearing venue flips a gross-favorable/net-unfavorable exit to hold; the false comment is corrected by the explicit policy |
| L11: Bolt truncates fees toward zero on exact decimals while pinned NT rounds half-even on floats at five decimals | Pinned-NT-aligned point accounting with a conservative debit bound | Task 14 | Numeric fixtures match NT `calculate_commission` across rounding-boundary prices; the bound remains at or above the point value |
| L14: an absent Polymarket fee descriptor collapses to fee-free through a typed variant with no vendor evidence and no audit component | Typed unknown that fails closed | Task 14 | An absent descriptor without an explicit config assertion fails admission; the asserted fee-free path emits a proven-zero audit component |
| M8: the quote-lifecycle FSM and requote budget advance at planning time and non-submitted outcomes never restore them; the NT-event reconciliation that would unwind them is deferred #817 surface | Shared-execution completion step: registration, FSM advance, and budget settle together via a commit-participant seam, with per-command settlement | Task 15 | Each pre-sink outcome restores FSM and budget alongside the registration abort; sink-invoked attempts commit their charge regardless of outcome; `Attempt(Submitted)` commits all three; issued cancel/modify mutations never restore; the #817 event-fence surface is neither deleted nor wired and is named as accepted scope |
| L15 class: no load-time check that the configured OMS mode is supported by the venue's position-report capability; Hedging-mode Polymarket (and Hyperliquid) can never reconcile because both adapters pin `venue_position_id: None`, and `observe()` silently drops key misses | Load-time OMS-capability authority | Task 16 | A Hedging configuration for a client whose adapter declares no venue-position identity fails config load; Netting loads; the capability is declared per client, never matched on venue name |
| L13, L16, L17, L18, duplicate ns-per-ms constant, fee-free-without-audit inconsistency, `observe()`/`acquire()` normalization divergence | Deletion and single sources | Task 16 | Compiler-enforced deletions, schema-doc correction, one clock constant, one key-normalization seam, parameterized fixture helpers |

Dismissed with evidence, requiring no repair: the cancellation clock is already a
single nanosecond domain with one conversion at each boundary; the
position-authority lease key is already exact
(client + account + instrument + hedging venue-position ID) with the concurrent
distinct-instrument test present; the submit-admission permit already rolls back
counters and reservations through its drop guard on every non-submitted path;
the kill-switch planning module remains intentionally inert per the quote-only
runtime boundary above and is not a deletion target.

### One governed exposure authority

The strategy's exposure state becomes a `GovernedExposure` wrapper owning a
private `ExposureState`. The exposure module exposes exactly one transition
entry point — an exhaustive reducer over a typed exposure event — plus
read-only projections. The mutable projections
(`managed_position_context_mut`, `tracked_position_context_mut`, and the
pending-entry equivalent) are deleted: context refreshes become typed reducer
effects that can update context data but cannot change the variant. Field
privacy plus the absence of any mutable accessor makes both variant assignment
and out-of-band context mutation outside the exposure module compile errors;
there is no secondary mutation path to audit afterward.

The reducer's input is a closed set of typed event families, each carrying the
identity and provenance it claims to act for:

- **Entry lifecycle**: entry submit resolution and every entry-order
  observation (fill, cancel, reject, deny, expire) keyed by the entry
  client-order ID.
- **Exit lifecycle**: every tracked exit observation — fill, partial fill,
  cancel, reject, deny, expire, cached `Voided`, and `OrderFillVoided`
  corrections — keyed by the sealed authority's client-order identity.
- **Historically attributed exit corrections**: an `OrderFillVoided` or cached
  correction whose identity matches no *active* authority but resolves — via
  retained release provenance or the authoritative cached order — to an exit
  this strategy previously released. These are never quarantined as foreign,
  but they do not displace a live authority either: when the exposure slot
  holds no conflicting authority (`Flat`, or `Managed` for the same released
  position), the correction enters the locked recovered-exit constructor with
  the `FillVoidReopen` cause immediately, exactly as the existing post-release
  regression requires. When any authority is active — a pending entry, an
  active exit, a recovery hold for a different exit — or the state is
  quarantined, the correction is recorded as a **deferred, identity-bound
  obligation** beside the exposure state: typed, loud, keyed by the released
  exit's identity, and accumulating that order's subsequent observations
  (fills, corrections, cancels) so discharge always constructs from complete
  history. An obligation discharges through the same recovered-exit
  constructor under the same compatibility predicate that governs immediate
  construction: only into an unoccupied slot, or `Managed` for the same
  released position. When the blocking authority resolves into an
  *incompatible* occupant — a pending entry that filled into position B, a
  residual managed B after an exit release, a healed hold for B — the
  obligation stays queued and loud, and the reducer retries discharge on
  every subsequent transition; there is no time-based trigger and no forced
  displacement. In `BlindRecovery` the obligation is recorded but quarantine
  stands, and discharge follows the provenance recovery rules — a raw
  correction never clears quarantine. The obligation set is bounded, not a
  hot-path backlog: obligations are keyed by released-order identity and
  idempotently compacted (a re-delivered correction for the same identity
  updates the one obligation, never appends), retained history per obligation
  and total obligation count carry TOML-configured limits with no code
  defaults, and reaching either limit enters a typed, loud, non-routing
  saturation state that preserves every active authority rather than dropping
  or silently evicting an obligation. Cap, duplicate-delivery, and stress
  tests prove bounded memory and per-transition cost.
- **Untracked order events**: any order event whose identity matches neither
  an active authority nor a historical attribution.
- **Position truth**: opened/changed events and cache materializations. A
  position event is a reconciliation *trigger*, not a truth payload: the
  reducer resolves the typed canonical result — `None`, `ExactlyOne`,
  `Multiple`, or `ProbeFailed` — through the cardinality-checked canonical
  projection, and classifies continuity against what the current state tracks.
- **Position closed**: tracked and untracked.
- **Timer reconciliation**: cache-probe outcomes, hold recovery attempts, and
  terminal-release retries.
- **Bootstrap and adoption**: cache bootstrap, restart exit-order adoption,
  settlement recovery, and governed NT-convergence adoption.
- **Settlement runtime effects.**

The reducer's match over state × event family is exhaustive with no catch-all
arm. Identity continuity is defined per state: exit-carrying states key on the
sealed handle; `PendingEntry`/`EntryReconcilePending` key on instrument and
entry client-order ID; `UnsupportedObserved` keys on its recorded position;
`BlindRecovery` carries a non-optional, reason-specific
`BlindRecoveryProvenance` (see below). The corrected target matrix:

| State \ Event | Exit-operation request | Submit resolution | Tracked exit observation | Historically attributed exit correction | Untracked order event | Canonical reconciliation (position truth/closed trigger) |
|---|---|---|---|---|---|---|
| `Flat` | rejected: unoccupied | stale-generation | n/a (none tracked) | recovered-exit constructor (`FillVoidReopen`) | quarantine | `ExactlyOne` → governed bootstrap adoption; `None` → remain; `Multiple`/`ProbeFailed` → `BlindRecovery`, loud |
| `PendingEntry` / `EntryReconcilePending` | rejected: occupied | entry-scoped | n/a | deferred obligation; entry authority preserved | quarantine | entry authority is preserved: adoption of a different position requires entry-order terminal or cancel proof first; until then typed conflict, entry tracking retained |
| `Managed` | granted | n/a | n/a | same released position → recovered-exit constructor; otherwise deferred obligation | quarantine | same identity → refresh context; `ExactlyOne` different → conflict fact + replacement-conflict hold, adoption only after the retained episode's own causal resolution; stale event vs canonical same → preserve; external close releases only via the close-proof conjunction; `Multiple`/`ProbeFailed` → `BlindRecovery`, loud |
| `ExitAttempting` | rejected: occupied | advance or typed stale | reconcile | deferred obligation; attempt preserved | quarantine | refresh on same identity; divergent canonical → typed conflict, authority never displaced |
| `ExitPending` / `TerminalExitAwaitingPosition` | rejected: occupied | stale-generation | reconcile / terminal fence | deferred obligation; authority preserved | quarantine | refresh on same identity; divergent canonical → typed conflict, authority never displaced; tracked close → terminal release with evidence |
| `ExitAuthorityRecoveryHold` | rejected: occupied | stale-generation | update retained order history and proof floors; preserve hold | same-ID: update hold; different released exit → deferred obligation, hold preserved | quarantine | heal identity in place on compatible; divergent canonical → typed conflict, hold preserved; tracked close → recovery release with evidence |
| `UnsupportedObserved` | rejected: occupied | stale-generation | n/a | deferred obligation | quarantine | refresh on same position; release only via the close-proof conjunction (episode `PositionClosed` + fresh `None`; either half alone preserves with awaiting health); otherwise conflict + probe-based reconciliation |
| `BlindRecovery` | rejected: occupied | stale-generation | identity-matched: update retained authority snapshot, fills, corrections, and proof floors without clearing quarantine; foreign: quarantine | obligation recorded; quarantine stands | quarantine | recovery only via fresh canonical probe per its provenance rules; raw events never clear quarantine |

"Quarantine" is a typed, evidence-recorded outcome that preserves the current
state and every active authority; it is never an overwrite. Untracked closes
and any event the matrix does not accept record quarantine evidence in place.
A tracked exit observation arriving while the recovery hold is in place
updates the hold's retained cached-order history, cumulative effective fills,
and terminal/correction proof floors — the same immutable inputs the locked
recovery constructor consumes — without leaving the hold, so authority
reconstruction can never proceed from stale fills because an observation was
dropped.

Convergence is canonical reconciliation, never event adoption. A position
event triggers the cardinality-checked canonical projection and the reducer
acts on its typed result. `ExactlyOne` with a **different** fingerprint is
never an immediate adoption, because the projection is only the current cache
view and carries no causal proof that the retained episode resolved: the
retained position enters a typed **replacement-conflict hold** — loud, and
non-routing for new operations — until the retained episode has its own
causal resolution. During a genuine replacement the plain close-proof
conjunction is unsatisfiable — canonical truth is `ExactlyOne(B)`, never
`None` — so the hold has its own atomic discharge: an exact episode-matched
`PositionClosed(A)` **plus** a fresh `ExactlyOne(B)` matching the held
candidate resolves A and adopts B in one governed transition, in either
arrival order; if the candidate disappears instead, the standard conjunction
applies. Only such a resolution discharges the hold and adopts the
replacement with recorded provenance (prior state, adopted position, cause).
Transient absence or occlusion of the retained position while another is
visible therefore cannot discard it, and B's presence alone never displaces
A. A stale event whose
canonical truth still matches the current position preserves it; `Multiple`
and `ProbeFailed` never adopt — they enter `BlindRecovery` loudly, preserving
the one-position invariant. Authority-bearing states are never displaced:
while a sealed exit authority or a working entry order is active, a divergent
canonical view is held as typed loud conflict, and adoption waits for the
authority's own terminal, cancel, or release proof. A permanent wedge is
impossible because canonical reconciliation re-runs on every subsequent
trigger; an implicit swap is impossible because adoption exists only as this
explicit transition.

`None` has an explicit contract in every state, and it is asymmetric by
design. In `Flat` it is a no-op. In every authority-bearing or
position-holding state — `Managed`, pending entry, active exit, terminal
fence, recovery hold — an event-triggered empty canonical projection
**preserves** the state and records typed awaiting/loud health: the locked
release proofs already name cache absence as insufficient for flatness, so a
transient empty cache can neither release exposure nor permit a second
position, and states carrying an exit authority release to `Flat` only
through the terminal reducer's own proofs. Only `BlindRecovery`'s
provenance-free reasons treat a coherent fresh-probe `None` as recovery —
there the strategy holds no local authority claim, and the probe is an
explicit governed recovery action rather than a passive event.

A plain `Managed` position with no exit authority still has a positive
externally-closed release path, or an external or settlement close would
occupy the slot forever. Its typed close proof is causal conjunction: a
tracked `PositionClosed` for the exact position episode **and** a fresh
canonical `None` confirming nothing remains open. Either input alone
preserves `Managed` with typed awaiting health — a close event with a
still-populated projection waits, and an empty projection with no close event
waits. `UnsupportedObserved` releases through the same conjunction for its
recorded position.

`BlindRecoveryProvenance` is non-optional and reason-specific, and the
classification covers the complete reachable reason set, not a sample:

- **Identity-bearing** (invalid bootstrapped position, invalid live
  position): provenance is the recorded position identity and sides; recovery
  is identity-continuity re-bootstrap.
- **Probe-class, provenance-free** (cache-probe failure, multiple open
  positions, settlement-evidence recovery failure): recovery **only** from a
  fresh canonical cache probe returning a coherent `None` or `ExactlyOne`
  result.
- **Restart-adoption failures** (ambiguous restart open exit orders,
  unattributed restart open exit order): provenance is the recorded
  ambiguous/unattributed order set; recovery re-runs restart adoption against
  a fresh canonical probe and the authoritative order cache — never against a
  raw event.
- **Foreign-venue position**: provenance is the foreign instrument identity;
  the state recovers only when a fresh strategy-scoped canonical probe no
  longer reports the foreign position.

Recovery provenance depends on the source state as well as the reason. When
`BlindRecovery` is entered from an occupied state — a managed position,
pending entry, or exit authority was live at entry, as in the
settlement-recovery failure that captures a managed position, or a
`Multiple`/`ProbeFailed` reconciliation during `Managed` — the provenance
retains that prior authority snapshot, and a fresh `None` probe can never
recover to `Flat`: an empty projection is not a close proof for the retained
claim. Recovery from an occupied entry requires the retained authority's own
causal resolution — a coherent `ExactlyOne` continuity match with the
retained position, its terminal proof, or governed re-adoption. Fresh-`None`
recovery to `Flat` is permitted only when quarantine was entered
authority-free (bootstrap-time probe failures before any adoption).

A raw order or position event can never clear quarantine directly in any
class — but when quarantine was entered from an occupied state, lifecycle
events that identity-match the **retained** authority (its fills, terminal
events, and corrections) update the retained snapshot and proof floors
without clearing `BlindRecovery`, exactly as the recovery hold does; the
retained authority's proofs must be able to accumulate or the occupied-source
recovery rule could never be satisfied. Foreign events remain quarantined. The implementation censuses every surviving `BlindRecoveryReason`
variant against this classification; a variant that fits no class is deleted
or reclassified in the same change, and each class carries a raw-event
negative test plus an authorized-recovery test, with chained tests from every
occupied source state proving a probe failure or `Multiple` followed by a
transient fresh `None` preserves the retained authority.

Boundaries do not read occupancy as a boolean. A boundary that wants to start
work requests a typed operation start — entry, exit, bootstrap, or
correction — and the reducer either rejects it with a typed, state-specific
outcome (occupied by hold, occupied by pending exit, blind recovery, and so
on) or returns a one-use operation grant with an explicit two-phase RAII
lifecycle: minting provisionally arms the authority at an exact generation, so
a second mint at the same generation is impossible; dropping an unconsumed
grant — hold decision, preparation failure, evidence or admission rejection,
unwind — performs exact-generation rollback of the arm and strands nothing.
Consumption is operation-specific, because only routed operations reach
shared execution: **route grants** (entry, exit) are consumed at shared
execution's final pre-sink boundary and convert the arm into the in-flight
attempt; **bootstrap and correction grants** are consumed atomically by the
reducer transition they authorize — the successful transition itself settles
the grant, and an unwound transition drops it with exact-generation rollback.
Consumption does not end a route grant's protection: the in-flight attempt it
becomes is itself a participant in the sink-phase transaction. An unwind
after consumption but before the sink-invoked marker rolls the exposure arm
back at its exact generation — no venue call happened, so nothing strands.
An unwind after the sink-invoked marker has an unknown dispatch outcome: the
exposure claim enters a typed, **operation-tagged sink-unknown hold** — a
first-class reducer state with entry and exit variants, carrying the order's
client identity and the attempt generation — rather than rolling back or
retrying, preserving callback-wins exactly as the maker transaction does. The
hold is non-routing and discharges only through authoritative reconciliation
against the NT cache and order/position reports, with explicit transitions
per outcome: submitted/accepted evidence converts it into the normal pending
state for its operation; a terminal rejection or cancel proof rolls the
exposure claim back per that outcome; a fill materializes through the
standard position-truth arms; and a **proven-absent** result — authoritative
cache/report proof the command never reached the venue — releases the arm.
With no callback and no authoritative proof, the hold stays non-routing; it
never wedges silently (typed loud health, timer-driven reconciliation) and
never clears on absence of evidence alone. Each grant family carries a
successful-consumption test plus distinct pre-consumption,
post-consumption/pre-sink, and sink-invoked unwind tests (entry and exit
both), plus discharge tests for each hold outcome and a
remains-non-routing-without-proof control. Callback-wins is preserved: a synchronous callback that
advances the generation invalidates the outstanding grant, whose drop then
no-ops rather than rolling back the newer state. Overlapping evaluations
therefore cannot route twice and cannot deadlock the slot — the double-route
and the stranded-arm both die at the type level. The strategy remains
intent-only under repository rule 9: the grant governs the strategy's own
exposure claim, while admission, venue gating, and submission mechanics remain
entirely in shared execution.

Identity is the sealed handle. `PendingExitState` loses its optional
position-identity and optional position-context duplicates wherever an
authority handle exists; release, reconciliation, cooldown market attribution,
and evidence all read the handle's non-optional accessors or the position's
lifecycle identity. The silent-refusal branches become unrepresentable rather
than logged.

Position identity is episodic, because NT netting reuses `PositionId` for
later positions on the same instrument. The episode is not a locally invented
token: it is a **fingerprint derived from authoritative NT lifecycle
fields** — instrument, `PositionId`, `opening_order_id`, and `ts_opened` —
all of which the pinned NT carries on the cache `Position`, on
`PositionChanged`/`PositionClosed` events, and (with `ts_opened` equal to
`ts_event`) on `PositionOpened`. The reducer derives the fingerprint at every
adoption or materialization and carries it in the `Managed` context, the
sealed exit authority, release provenance, and deferred obligations. Every
compatibility predicate — "same released position", historical attribution,
obligation discharge, refresh continuity, and the close-proof
conjunction — compares fingerprints, never raw `PositionId` plus instrument.
An ordinary refresh whose event carries the same fingerprint preserves the
episode (no reminting, so a legitimate close cannot wedge); an event carrying
a changed fingerprint is a different episode. A delayed `PositionClosed` for
episode A after episode B reopened under the same `PositionId` therefore
authenticates as A's close — it can discharge A-scoped obligations or
half-proofs but can never satisfy B's close-proof conjunction, even combined
with a canonical `None`; and A's late corrections stay deferred until a slot
compatible with episode A (or flat) exists.

A changed fingerprint is not always a different episode: pinned NT's fill-void
replay (`Position::apply_fill_void` → `rebuild_from_replay` →
`reset_derived_state`, then re-application of surviving fills) lawfully
re-derives `opening_order_id` and `ts_opened`, and replay re-derives them **per
flat-crossing segment** — the post-replay fingerprint can belong to a later
reopened segment, not the corrected episode. The reducer therefore has an
**authenticated episode-rebase transition** with segment continuity, not a
blanket rebase: an `OrderFillVoided` that binds to the retained episode's own
recorded opening order and fill identities is an episode-correction event, and
the reducer then proves **replay-segment continuity** — the post-replay
position state descends from the retained episode's surviving fills without
crossing flat into a later segment — before rebasing. Only carriers belonging
to that continuous segment rebase, atomically: `Managed` context, sealed
authority, release provenance, and that episode's deferred obligations. If the
post-replay lineage crosses flat into a different segment, the retained
episode is treated as correction-closed for its own carriers while the later
segment remains a distinct episode under the standard isolation rules. A
correction also **invalidates and re-floors every pre-correction close or
terminal half-proof** for the affected episode: a stored `PositionClosed`
half-proof from before the correction can never combine with a later canonical
`None` to release a position the correction left open. If no fill survives,
release follows a **correction-specific proof** bound to the voided opening
fill rather than the exact-fingerprint conjunction. Rebase authenticates only
through the retained episode's own order/fill identities — an event that
matches none of them remains a different episode — so late-A-versus-reopened-B
isolation is not weakened.

The reducer's typed outcomes are evidence-domain contracts, not diagnostics:
the exit-blocked reason set gains recovery-hold-occupied and stale-generation
variants, and quarantine and identity-conflict outcomes are new lifecycle
facts. Facts, codecs, the generated contract, fixtures, and round trips change
atomically, per the existing evidence contract.

All bootstrap and restart-adoption paths construct exposure through the same
reducer events, so the startup-created recovery hold, adopted exit orders, and
bootstrapped positions obey the same occupancy and identity rules as live
transitions.

### Economics authority closures (takeover round)

Valuation-route origins become kind-tagged in both TOML and the runtime
builder: an origin is a currency or an asset, parsed as such, and the route
table can express every native-unit kind a provider emits. The closed-world
check is structural — the native-unit kind enum and the route-origin
constructor are the same type, so a new kind extends both or fails to compile.
Admission evidence includes an end-to-end spot-BUY quote through runtime-built
routes.

Resting-refresh equivalence binds its comparison basis to `EconomicScope`:
decision- and action-scoped components compare on the order-leg quantity basis;
position-interval components compare on the position basis their producers
price against. The gate's scenario coupling (`TradingEdge` implies
`TerminalValueEntry`, which pins no position context) is today's reachability
fence, not the correctness argument; the scope binding closes the class before
any future scenario variant reopens it.

Exit-vs-hold timing becomes a sealed, fee-aware comparison owned by shared
economics: the strategy consumes a typed result that already nets venue fees on
both legs, and the `RiskReduction` admission purpose carries an explicit
policy — risk-reducing exits remain admissible regardless of edge, stated as
policy rather than implied by a missing branch, and the admission comment that
claimed a universal non-positive-edge rejection is corrected by the type.

Polymarket point accounting aligns with pinned NT's commission arithmetic, with
numeric behavioral fixtures at rounding boundaries, while the reserved debit
bound remains provably at or above the point value. An absent fee descriptor is
a typed unknown that fails closed; only an explicit configuration assertion
selects fee-free, and that path emits a proven-zero audit component consistent
with the carry pattern.

### Maker quote transaction boundary (takeover round)

The decision to submit no longer advances the leg FSM or charges the requote
budget as a side effect of planning. Because the tracked resting registration
already commits or aborts inside shared execution before dispatch sees the
outcome, an outer transaction cannot merely wrap the result: shared execution
owns one multi-participant transaction whose participants — provisional
registration, leg FSM advance, and budget settlement — move through the same
explicit phases:

1. **Proposal**: planning mints typed proposals (leg transition, budget
   reservation) with no side effect.
2. **Pre-sink provisional arm**: before any sink call, every participant arms
   provisionally at the attempt's exact generation — the registration
   provisional record, the FSM's proposed transition, and a generation-bearing
   per-leg budget reservation token.
3. **Generation-checked commit/abort**: the completion step settles all
   participants under the same generation check. No external participant runs
   under the registry lock; settlement is ordered outside it, and the
   synchronous-callback disposition is recorded during the arm so a terminal
   callback arriving mid-attempt settles the FSM correctly (callback-wins,
   the registry's existing exact-generation retirement property, extended to
   every participant).
4. **Sink-invoked marker**: an irreversible sink-invoked phase is recorded
   immediately before the raw sink call. It is the accounting boundary: all
   restoration and prepaid-token reuse applies strictly before it; once
   crossed, the attempted command/REST charge is committed regardless of
   outcome.
5. **Drop guard**: an unwind before the sink-invoked phase rolls back every
   armed participant at its exact generation or poisons loudly; it can never
   settle one participant and strand another. An unwind **after** the
   sink-invoked phase — panic or synchronous-callback unwind with the outcome
   unknown — always commits the command/REST charge and poisons **only the
   participants still armed at the transaction's generation** into the
   non-routing reconciliation hold: the command may have been dispatched, so
   neither a refund nor a routable state is permitted for an armed
   participant. Precedence over callback-wins is explicit: a participant
   already retired by a synchronous terminal callback stays retired — poison
   never overwrites a completed callback disposition, and the charge commits
   in every case. Pre-sink unwind, post-sink unwind, and the combined
   sink-invoked → terminal callback → unwind sequence carry distinct tests.

Settlement is per command and per leg, not blanket:

- **Submit**: registration, FSM advance, and the leg's budget token commit
  together on `Attempt(Submitted)`; every **pre-sink** outcome rolls back all
  three. Participants settle independently once the sink is invoked:
  `SinkRejected` aborts the registration and FSM advance but **commits** the
  attempted command/REST charge — the venue received the call, and refunding
  it would let repeated rejection bypass the submit and egress caps. Two-leg dispatch settles each leg's token independently and
  phase-qualified: a submitted YES sibling keeps its charge; a
  pre-sink-rejected NO sibling restores its token; a sink-rejected NO sibling
  commits its charge — and the inverses hold symmetrically. No shared
  snapshot rollback can erase a sibling's committed charge or restore a
  sink-rejected one's. The blanket rollback rule has one carve-out: a
  **replacement submit after a confirmed cancellation** never rolls the leg
  back to its pre-cancel state, because the cancel already executed at the
  venue and the sole `Canceled` event that produced the replacement proposal
  will not recur. A non-submitted replacement outcome whose transaction
  proves an exact abort with no retained registration enters a typed
  replacement-pending backoff state and retries the replacement on the
  coordinator's timed cadence until it routes or the quote lifecycle retires
  the leg — no strand, no dependence on the unwired event fence. Charging
  follows the sink boundary: a pre-sink replacement failure retains the
  prepaid token for the next attempt, while a sink-rejected replacement
  consumed a real venue call — its charge commits, and every subsequent
  retry that reaches the sink acquires a fresh generation-bearing attempt
  reservation, so repeated rejection is bounded by the caps rather than
  hidden behind one prepaid token. A
  `RollbackInvariantFailed` outcome never retries: exact cleanup was not
  proved and another generation may remain authoritative, so the leg enters a
  non-routing poisoned reconciliation hold that retains the prepaid token and
  makes no sink call until governed recovery resolves it.
- **Cancel-resubmit reprice**: the existing atomic one-submit-plus-two-REST
  acquisition remains ONE prepaid, generation-bearing token reserved before
  the cancel is issued; the confirmation-driven resubmit consumes the same
  token, never a second charge, and a cancel is never issued without its
  replacement capacity already reserved. Failures before the cancel is issued
  release the token; after issuance, the FSM's pending advance and the token
  survive — restoring either would permit duplicate commands or a
  cancel-without-resubmit strand. The prepaid token covers the first cancel
  REST call and the replacement submit only: **every other coordinator REST
  effect** — each cancellation retry for a retryable or unobserved cancel and
  each query attempt for a queryable missing or pending order — acquires and
  settles its own generation-bearing REST reservation before routing, so
  retry and query storms are bounded by the venue egress cap rather than
  hidden inside one acquisition or exempted entirely. Reservation acquisition
  precedes the coordinator's attempt arming: a denied reservation is a typed
  reservation-denied outcome that enters the coordinator's backoff without
  arming `Attempting`, without incrementing routed-attempt counters or
  escalation, and with zero sink calls — budget denial is never laundered
  through `OperationUnobserved`, and the generation binds the reservation to
  the attempt so an eventually granted retry routes and is charged exactly
  once per sink-reaching attempt. The prepaid token covers the first
  replacement attempt only; no charging path refunds a real venue call, and
  repeated rejection is bounded by the caps. The cap equation is defined over
  **current emitted charges plus age-independent outstanding liabilities**: a
  reserved token never ages out of the sliding window while outstanding,
  later commands are denied while its capacity is reserved, and consumption
  is an atomic zero-net conversion of the liability into an emitted
  charge — a delayed replacement therefore always has its capacity by
  construction. Consumption-time revalidation exists for exactly one named
  cause — a cap or configuration generation change since reservation — and
  that case enters the non-routing recovery state with zero sink calls; it is
  specified and tested explicitly, not as a generic window-full failure.
- **Modify**: pending advance commits at issuance; pre-issuance failures roll
  back.

The dormant event-fence reconciliation functions are pre-existing #817 surface
whose module also carries load-bearing maker identity types; this slice
neither deletes nor wires them. That accepted #817 scope is named in the
review request per the repository's slice-scope rule.

### Load-time OMS-capability authority (takeover round)

Whether a venue's position reports carry venue-position identity is a declared
adapter capability, owned where execution clients are registered. Configuration
load rejects an OMS mode the declared capability cannot support — Hedging
without venue-position identity — before runtime construction, for every
client, including unselected ones. The check names capabilities, never venue
identifiers. The position-authority feed's report-key derivation and its
observation path normalize identity through one shared seam, so a report and a
lease cannot disagree about what the key is. An observation whose key matches
no active lease returns a transient typed outcome that the subscription
handler surfaces as operator telemetry and then drops: nothing is stored, no
key is created, and dropping the last lease still removes every snapshot and
health record — the locked lease-bounded feed contract is unchanged.

### Surface deletions and single sources (takeover round)

`evidence_fixture_id` is deleted from the schema and every TOML. The schema
document drops the deleted fee-cache field. The caller-less public
`forecast_available` is deleted rather than fenced; the exhaustive
evidence-domain census found no other dead public item, and its three
feature-gated `_for_test` methods tighten to `pub(crate)` so no shippable-build
surface remains public without a reachable caller. The duplicate
nanoseconds-per-millisecond constant collapses to one definition. The
shared-fixture seed helpers take the order side and position side they seed, so
linkage assertions can no longer agree with production by construction; the
existing hardcoded-side convenience wrappers are deleted, not aliased.



This repair does not implement actual economics ledgers, supplemental venue actuals, lifecycle or transfer actuals, reporting closure, live economics publication, or live execution. Those remain outside Slice 1. It also does not claim retry durability across process death or forced process termination.
