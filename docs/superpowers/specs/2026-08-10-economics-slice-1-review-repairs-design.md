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
- for a quote-quantity order, each candidate level contributes `price * base_quantity` toward the submitted quote notional, and retained base quantity is `consumed_quote_notional / price`;
- a price-less market order does not invent a limit price; limit checks run only when the final order actually has one.

Over-covering candidate levels are deterministically truncated to the final order. Empty, non-positive, overflowing, under-covering, side-incompatible, scenario-incompatible, or limit-violating inputs fail. The constructor then derives normalized fill legs, planned-fill notional, gross value, provider economics, edge basis, and the final order binding from the retained levels.

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

### Authoritative NT observation matrix

Every timer or order callback re-reads the current NT cache and maps it through an exhaustive `OrderStatus` match with no wildcard:

| Observation | NT state |
| --- | --- |
| `Missing` | No cached order |
| `Retryable` | `Initialized`, `Emulated`, `Released`, `Submitted`, `Accepted`, `Triggered`, `PendingUpdate`, or `PartiallyFilled`, with positive leaves when leaves are available |
| `PendingCancel` | NT reports `PendingCancel` |
| `Terminal` | `Denied`, `Rejected`, `Canceled`, `Expired`, `Filled`, or `Voided`, or cached leaves are zero |

An `OrderFilled` callback is only a reconciliation trigger. A partial fill remains `Retryable`; the record retires only when the cache is closed or leaves are zero. NT updates its cache before strategy callbacks at the pinned revision, so callbacks never override current cache state with stale event payloads.

NT can later reopen a filled order through `OrderFillVoided`. The maker's fill-void callback re-reads the cache. If a previously retired maker order is open again, it creates a cancellation-only coordinator record with `quote_deadline_ns = observed_now_ns`; it does not recreate or reuse expired economics admission. The next timer routes through the normal coordinator and health is immediately deadline-exceeded until NT closes the order. Strategy event ownership identifies the order as this maker's; no client-order-ID string parsing or source scan is used.

### Coordinator state matrix

Routing state is deliberately small:

- `Ready`: a cancellation may be attempted now;
- `Backoff { retry_not_before_ns }`: an attempt occurred and no further attempt is allowed before the deadline;
- `PendingCancel { retry_not_before_ns }`: NT has acknowledged pending cancellation; duplicates stay suppressed.

Attempt count, last attempt outcome, and typed health are diagnostic metadata, not alternate routing states. The implementation is an exhaustive match over coordinator state × authoritative observation; adding a state or observation must produce a compiler error until every pair is handled.

| Current state | `Missing` | `Retryable` | `PendingCancel` | `Terminal` |
| --- | --- | --- | --- | --- |
| `Ready` | On an origin request or timer drive, route once | On an origin request or timer drive, route once | Adopt `PendingCancel` with `retry_not_before_ns = observed_now_ns + retry_timeout`; do not route | Remove record |
| `Backoff` | Suppress before the deadline; timer routes once at or after it | Suppress before the deadline; timer routes once at or after it | Enter `PendingCancel`; do not route | Remove record |
| `PendingCancel` | Return to `Backoff` with the existing deadline; callback does not route | Return to `Backoff` with the existing deadline; callback does not route | Stay pending; do not route | Remove record |

The identical `Missing` and `Retryable` behavior is still represented explicitly so neither branch can silently drift. `Ready × PendingCancel` covers an externally initiated cancel. A stale `CancelRejected` while `Ready` only reconciles current cache state; callbacks never route.

### Attempt outcome, retry, and health rules

Every actual cancel API invocation increments a checked attempt counter and arms `retry_not_before_ns = attempt_now_ns + cancel_retry_timeout`. This includes synchronous routing failures, so they cannot retry at timer cadence. A successful NT route enters `Backoff` while awaiting status; a synchronous failure also enters `Backoff` without claiming that NT accepted anything. `SkippedByPolicy` is not an attempt and advances neither counter nor routing state.

`CancelRejected` never routes from its callback. Reconciliation normally sees NT's restored retryable status and preserves the current not-before deadline. If callback/cache ordering is stale and NT still says pending, the coordinator remains pending. A later timer makes the authoritative decision.

At exactly `cancel_retry_escalation_attempts`, the record exposes typed `RetryEscalated` health and a loud error. Later timer drives continue rate-limited retries. If the retained quote deadline passes while the order is still retryable or missing, health becomes `CancellationDeadlineExceeded`; if it remains pending, health becomes `StuckPendingCancel`. Neither condition creates venue-paced retry churn. Attempt and health failures are isolated per record: every due sibling is processed, each primary error is retained, and one aggregate error is returned afterward.

Cancel-all selects records by the exact routed `(instrument_id, order_side)` scope. An accepted route arms backoff for only those records. A synchronous failure also rate-limits only those records while reporting failure. `SkippedByPolicy` stamps none. Uncovered records remain immediately eligible.

## NT callback and stop integration

The maker's `nautilus_strategy!` hook block implements `on_order_pending_cancel`, `on_order_cancel_rejected`, `on_order_canceled`, `on_order_filled`, `on_order_fill_voided`, `on_order_expired`, and terminal rejection handling. Each hook forwards only the client order ID and current NT actor time to coordinator reconciliation; event payload status never becomes a second authority. On every fill-void callback, current cache status and leaves alone decide whether an untracked reopened maker order needs a cancellation-only record.

The concrete stop hook is NT's `Strategy::stop() -> bool`, registered by `Trader`. Maker construction rejects `manage_stop = true`; the maker's tracked-order draining protocol and NT's position-closing managed stop cannot both own the same stop request. The maker archetype's canonical strategy envelope already uses `manage_stop = false`.

If no tracked orders exist, the maker stop hook returns `true`. Otherwise it enters maker `Draining`, asks the coordinator to cancel all tracked orders, and returns `false`, so NT leaves the strategy `Running`; its timer and order callbacks continue. When reconciliation removes the last tracked order, the timer or callback completes shutdown through public `Component::stop(self)` and returns immediately without further quote refresh or routing. Only then does `DataActor::on_stop` deregister the timer, unsubscribe, and deactivate the maker runtime.

This stop protocol is independent of NT's position-closing `manage_stop` policy. Process-level forced termination cannot guarantee acknowledgement delivery and is not claimed as an in-process graceful-stop path.

## Clock and deadline matrix

Production economics admission and cancellation use one clock: the NT actor clock. Shared admission receives explicit event time from the routing context; the production economics path does not call `SystemTime` or another wall clock.

| Value | Authority | Use |
| --- | --- | --- |
| `requested_at_ns` | Original economics request | Quote lineage and provider validity calculation only |
| `route_now_ns` | Fresh NT actor time at shared routing entry | Remaining-lifetime and admission evaluation before evidence or counters |
| `pre_sink_now_ns` | Fresh NT actor time immediately before venue mutation | Final remaining-lifetime guard; failure drops the uncommitted admission permit and does not call the sink |
| `retry_not_before_ns` | Checked `attempt_now_ns + retry_timeout` | Earliest timer-driven retry for success, rejection, or synchronous failure |
| `quote_deadline_ns` | Economics quote `valid_until_ns` | Cancellation health deadline |
| `last_observed_ns` | Prior coordinator observation | Clock-regression rejection |

`ExecutionEconomicsConfig` gains required, no-default positive `cancel_retry_timeout_ms` and `cancel_retry_escalation_attempts`. Every shipped economics TOML section and fixture is updated in the same branch state; serde defaults are forbidden.

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
| Final quote-quantity coherence | The same multi-level case uses a quote-quantity order and asserts quote consumption, divide-by-price base legs, planned notional, gross, and binding |
| Price-less order support | A market quote-quantity order seals without inventing a limit; invalid or under-covering levels fail |
| Fail-before-mutation construction | Under-cover, invalid price/quantity, limit violation, scenario mismatch, and exit-clamp mismatch leave evidence, pending exposure, counters, registration, sink calls, and venue state unchanged |
| Exhaustive cancellation transitions | A table-driven test covers all 12 coordinator-state × observation pairs, including `Ready × PendingCancel`, stale rejection while ready, and explicit identical missing/retryable behavior |
| Rejection and stale-event safety | `Backoff -> PendingCancel -> CancelRejected/open` becomes timer-retryable at the preserved deadline; delayed rejection against newer pending/terminal cache state cannot override it |
| Fill and correction safety | A partial `OrderFilled` with positive leaves retains cancellation tracking; a full fill or zero leaves retires it; a later reopened `OrderFillVoided` creates a cancellation-only record and routes only on the next timer |
| Retry-rate floor | Repeated synchronous failures and accepted-but-unacknowledged attempts do not retry before the timeout, retry exactly at the boundary, and remain sibling-isolated |
| Attempt escalation | The exact configured attempt threshold exposes `RetryEscalated`; a later timer still performs one bounded retry |
| Pending/deadline health | Pending suppresses duplicates and becomes `StuckPendingCancel`; retryable/missing becomes `CancellationDeadlineExceeded` at the quote deadline |
| Cancel-all scoping | One-side cancel-all affects only matching records; synchronous failure rate-limits only that scope; shadow skip affects none |
| One clock and remaining lifetime | Missing/zero/overflow/cadence config fails; delayed routing that passes total lifetime but lacks remaining margin fails before evidence; exact boundary succeeds; clock regression fails |
| Pre-sink recheck and rollback | An injected clock advance between admission and sink blocks the sink and proves counter/reservation/registration rollback |
| Real graceful stop | Maker construction rejects `manage_stop = true`; through NT's real `Strategy::stop`/`Trader` lifecycle, stop returns deferred while records remain, timer/callback processing continues, and `Component::stop` runs only after the last terminal observation |
| Fee-seam deletion without collateral loss | Provider formula and replay parity tests remain; settlement dispatch, maker quote-target dispatch, and unknown-family failure tests remain after fee assertions are removed |

Existing fail-before-mutation admission and resting-refresh tests remain regression evidence. Deletion of obsolete public paths is established by compilation and reviewer diff/symbol inspection, not by testing source text.

## Non-goals

This repair does not implement actual economics ledgers, supplemental venue actuals, lifecycle or transfer actuals, reporting closure, live economics publication, or live execution. Those remain outside Slice 1. It also does not claim retry durability across process death or forced process termination.
