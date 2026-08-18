# PR #1544 Lifecycle-First Scope-Back

## Decision

PR #1544 remains the atomic economics quote/admission cutover owned by #1445. It keeps the economics, provider, replay, admission, maker, cancellation, and OMS work already in the PR. It does not add a generic exposure runtime, a restart journal, or strategy-owned reconstruction of order authority.

The repair has two parts:

1. Shared order execution treats every error returned after it calls NautilusTrader submit as `SinkInvokedUnknown`. Reservations and lifecycle participants remain committed until authoritative NT lifecycle evidence retires them.
2. The edge taker keeps only authority created by its current process. If that authority is unavailable or contradictory, it enters the existing non-routing blind-recovery state. It never reconstructs submit authority from cache observations.

This is a scope reduction, not a replacement architecture.

## Why this boundary

NautilusTrader owns order lifecycle, cache, and reconciliation. Bolt can prove its own pre-sink work, but after calling `Strategy::submit_order` it cannot infer from the returned `Result<()>` whether the command remained local or entered routing. The pinned implementation can route the command and then fail while installing the GTD timer. Shipped configuration currently disables managed GTD expiry, so that particular sequence is config-latent; the shared boundary is still unsound because the flag and future pins are not part of the return type.

The attempted upstream API in nautechsystems/nautilus_trader#4790 and PR #4791 tried to expose that handoff. Both were closed with maintainer direction that reservations should follow order lifecycle rather than a handoff result. At the pinned revision, some local denials enter the cache and publish lifecycle events while some errors returned before routing do not. Bolt cannot distinguish those cases from the returned error, so this design follows lifecycle evidence and does not reopen the same API downstream or require an NT pin change.

The conservative consequence is intentional: if NT returns an error after Bolt invokes submit and no authoritative lifecycle evidence follows, the claim remains occupied. A timeout or cache miss is not proof of absence.

## Ownership after the repair

| Owner | Responsibility |
| --- | --- |
| Strategy | Signal, intent, cooldown, current-process occupancy reduction from supplied lifecycle facts, and strategy-facing evidence |
| Shared Bolt execution/admission | Pre-sink validation, final order/economics seal, capacity reservation, submit phase, and exhaustive route settlement |
| Shared maker lifecycle/registry | Quote participant settlement, resting registration, cancellation intent, and retry coordination |
| NautilusTrader | Command routing, cache, order/position lifecycle, reconciliation, and venue translation |

No shared module learns edge-taker states, strategy names, market-family rules, or strategy configuration. No strategy calls an alternate NT submit path.

## Scope retained in PR #1544

The following work remains unchanged except where the shared submit outcome must be handled exhaustively:

- venue- and substrate-neutral economics core;
- provider economics and replay mappings;
- final-order economics sealing and shared admission;
- maker leg/instrument/scope authority;
- quote transaction and requote-budget reducers;
- resting registration and cancellation coordination;
- OMS/capability load-time rejection;
- current-process edge-taker residual reduction after canonical position proof.

## Strategy-owned recovery removed

The implementation deletes the restart and fill-void reconstruction machinery added during the review repairs. This includes the production concepts represented by:

- `BoltV3ExitAuthorityRecoveryHandle` and its release protocol;
- `BoltV3RecoveredExitCause` and recovered baselines;
- the `Recovered` exit-order-authority variant;
- `ExitAuthorityRecoveryHoldState`, its plan, and flat-recovery arm;
- `ManagedPositionOrigin`, the `origin` field, and `is_recovering()`; every routeable managed position is current-process state;
- `FlatTerminalEntryOverride`, its `last_flat_terminal_entry_override` storage, and the `PositionTruthRematerialized` evidence transition that depended on treating `RecoveryBootstrap` as routeable lineage;
- startup adoption of a cached exit as newly minted Bolt authority;
- startup materialization of a cached position as routeable `Managed` or `UnsupportedObserved` state;
- startup settlement shortcuts that turn a cached open position into `Flat`, including the
  `settled_position_keys` branch and the prior-booking-terminal transition;
- the now-unused `ApplyPriorBookingError` decision arm and key-set inputs in shared `bootstrap_recovery_from_cache`; the edge taker is their only production caller;
- fill-void reconstruction after the original current-process authority has retired;
- the cache-absence and correction arms of timer reconciliation that attempt to rebuild or release authority.

The associated strategy functions and evidence branches are deleted, including `bootstrapped_exposure_for`, `adopt_restart_open_exit_order_from_cache`, `recover_exit_authority_state`, `enter_exit_authority_recovery_hold`, `try_recover_exit_authority_hold`, and `try_release_exit_authority_recovery_flat`. Tests that assert reconstruction are replaced by behavior tests for non-routing recovery.

The locally submitted exit authority remains. It exists only for the process that created it, carries the exact order and position identity, and is retired only by the existing current-process lifecycle reducer.

Timer reconciliation remains only for that retained local authority. An exact cached `Working` observation preserves it, and an exact cached terminal observation may advance its existing reducer. A missing cached order leaves the state unchanged and emits evidence; it neither constructs a hold nor releases authority. Fill-void correction after retirement enters `BlindRecovery` instead of using the timer to mint authority.

### Restart behavior

Startup may consume canonical NT facts for observation and evidence, but it cannot mint a Bolt submit claim from them. The open-position count is classified before any settlement-release shortcut. Any cached open position, possibly related order, conflicting projection, or insufficient proof that no prior exit remains live places the edge taker in `BlindRecovery`, which routes no entry, exit, cancel, or modify. In particular, a cached position never becomes routeable `Managed`; an empty attributed-exit-order list is not proof that the position has no live exit; and a recovered settled-position key or prior booking-terminal fact may be recorded but cannot turn a cached open position into `Flat`.

The existing clean-start case with zero scoped positions remains `Flat`; #1544 does not claim that this is crash-safe restart proof. Once startup enters `BlindRecovery`, later cache absence or position rematerialization cannot turn it into `Managed`, `ExitPending`, or `Flat`. Returning that state to routing requires a future separately approved capability with stronger proof.

### Running fill-void behavior

If a fill correction arrives while the original local exit authority is still retained, the existing authority and lifecycle reducer handle it. If the authority has already retired, the correction enters `BlindRecovery`. The strategy does not search the cache and construct a replacement authority.

### Missing current-process cache observation

A missing cached order does not discard a retained local authority and does not create a replacement one. The strategy keeps the original occupied lifecycle state and waits for authoritative terminal or rejection evidence. It does not route a second exit.

## One local exposure owner

`ManagedPositionContext` contains only the supported position. It has neither `origin` nor `pending_entry: Option<_>`. A partially filled entry whose remainder may still work is represented by one strategy-local `EntryRemainder` state rather than multiplying outer exposure variants:

```text
EntryRemainder {
    pending_entry: exact current-process order identity,
    position: Supported(position) | Unsupported(position, reason) | CanonicallyFlat,
    cancellation: Working | Pending | Refused,
}
```

Every combination is meaningful: supportedness controls whether an eventual position may become routeable, while the cancellation phase controls only the known working entry order. There is no optional order identity. A fill or position update changes the typed position projection without dropping the entry identity. A position close or economic settlement may change the projection to `CanonicallyFlat`, but the outer state remains occupied until exact entry terminal or zero-leaves evidence arrives.

All shipped edge-taker configs currently use FOK entries, so a working entry remainder is config-latent. The TOML schema permits persistent GTC/GTD entries, and lifecycle safety cannot depend on the shipped value never changing.

Exact entry terminal evidence consumes the `EntryRemainder` state only after the same reducer obtains coherent canonical position truth: `Supported` becomes plain `Managed`, `Unsupported` becomes the existing non-routing unsupported state, and `CanonicallyFlat` becomes `Flat`. Missing or contradictory truth leaves the exact claim occupied in entry reconciliation. A partial fill alone cannot consume it.

Routeable `Managed` may be constructed only by consuming a current-process pending-entry claim or by updating an already current-process managed/exit state. A newly observed position without that lineage enters `BlindRecovery`. Position materialization exhaustively preserves `BlindRecovery`; later cache or position events cannot smuggle restart or external state back into routing.

The strategy stores one small exposure owner whose inner `ExposureState` and variants are private to the strategy-local exposure module. This encapsulates the existing edge-taker occupancy state; it does not create independent lifecycle truth. Strategy orchestration can submit typed strategy intent or NT-derived facts and consume typed actions, but cannot construct, assign, or match the inner state directly. The owner has no clock, cache, NT handle, sink, subscription, timer, retry loop, persistence, or TOML policy; orchestration supplies facts and performs returned effects through existing shared routes. This wrapper is not shared with execution/admission and carries no generic cross-strategy policy.

The owner exposes three mutation protocols: arm one strategy entry, apply one typed lifecycle/position observation, or request an exit and settle its returned effect. `arm_entry` accepts a typed strategy-local entry claim only from `Flat`, allocates an owner-local generation, installs the exact pending client order before any NT call, and returns a non-cloneable capability for that generation. Strategy order/economics preparation finishes before arming. Any required submit-linked evidence write then occurs under the capability; failure aborts that exact pre-sink generation, and the shared route follows on success. The capability's exhaustive settlement consumes the shared route outcome: policy skip or a pre-sink outcome may abort only that exact generation, while `Submitted` and `SinkInvokedUnknown` retain it. A synchronous lifecycle callback that already advanced or retired the generation wins, and later route completion cannot recreate it. The capability is strategy-local and is never inspected by shared execution.

`request_exit` returns either a typed non-routing result, an exit-attempt capability, or an exact cancellation effect carrying its own settlement token. Orchestration performs the shared route and must return its exhaustive outcome through that token; it cannot mutate cancellation or exit phase directly. This is one protocol rather than a separate projection/setter pair.

Observation reduction delegates every vacancy decision to one private release reducer. That release reducer is the only runtime path that may produce `Flat`; clean startup with zero scoped positions is the only constructor exception. `Flat` is not used as a temporary `mem::replace` sentinel: each transition computes a complete next state before committing it. Settlement success, terminal booking error, position close, entry terminal, exit terminal, and cancel rejection all enter through the observation enum. It preserves `EntryRemainder`, retained exit authority, and `BlindRecovery` rather than treating position or settlement evidence as order-lifecycle proof.

Exit eligibility is no longer assembled from correlated projections such as “has a managed position,” “has no exit snapshot,” and “has no pending entry.” One exhaustive transition owns the decision:

```text
Managed -> ExitAttempting
EntryRemainder { cancellation: Working, .. } -> install Pending; request exact entry cancellation; no exit
EntryRemainder { cancellation: Pending, .. } -> typed non-routing outcome; no exit
EntryRemainder { cancellation: Refused, .. } -> typed unhealthy non-routing outcome; no exit or retry
every other exposure state -> typed non-routing outcome; state unchanged
```

The cancellation-only transition applies to supported, unsupported, and canonically flat position projections. Forced flat invokes it for a supported remainder. Materialization or release that produces `Unsupported` or `CanonicallyFlat` invokes it immediately. Each installs `Pending` before returning the one cancellation effect, so unsupported materialization, position close, and settlement cannot leave a known remainder working.

The shared cancel boundary returns one exhaustive attempt outcome: `SkippedByPolicy`, `NtCallReturned`, or `NtCallErrored`. `SkippedByPolicy` may restore the exact `Working` state only while the same entry identity remains present. In live mode, both other variants mean only that NT was called; neither restores the state or proves that a cancel command left the process. This replaces the misleading low-level `Canceled` name and prevents callers from inferring routing from `Result<()>`. After either live outcome, an exact client-order cache observation may settle terminal or zero-leaves state; pending-cancel and ambiguous working observations remain pending. Because NT does not expose the cancel command ID, `OrderCancelRejected` cannot be attributed to a particular attempt. An exact rejection for the retained client order therefore changes `Pending` to `Refused`, emits operator-visible evidence, and grants neither release nor retry authority. Duplicate or replayed rejection events are idempotent in `Refused`. A missing order remains pending. `NtCallErrored` emits an error-level diagnostic and typed non-routing evidence; observability does not grant retry or release authority.

The reducer installs `Pending` before the cancel call. A synchronous terminal callback consumes the exact entry identity and cannot be overwritten by the return path. Route completion reduces only while that identity and pending phase still match. Each exact entry identity can make at most one live cancellation call: only policy skip restores `Working`, because it proves NT was not called. Cancellation uses the existing shared order-execution policy and event/cache truth; this design adds no cancellation map, timer, backoff, attempt counter, or second retry algorithm.

Authority-mutating matches enumerate every private `ExposureState` variant without a wildcard. Position-close and unsupported-position observations enter the same reducers instead of matching state in strategy orchestration. Read-only projections may group states inside the owner, but they cannot authorize a route or discard an order/position claim.

## Shared submit outcome

`BoltV3SubmitAttemptState`, `BoltV3SubmitAttemptKind`, and route-participant completion replace the submit-only `SinkRejected` classification with `SinkInvokedUnknown`. The old name claimed more than the `Result<()>` proves. The replacement carries the prepared-order linkage and diagnostic, but is neither a rejection nor a claim of venue acceptance.

The shared route phases are:

| Phase/outcome | Meaning | Settlement |
| --- | --- | --- |
| Route, intent, admission, or pre-sink rejection | NT submit was not called | Refund/remove the exact provisional generation |
| Policy skip | No live submit was attempted | Preserve existing shadow semantics |
| `SinkInvokedUnknown` | NT submit was called and returned an error | Retain capacity and every lifecycle participant |
| `Submitted` | NT submit returned success | Retain capacity and every lifecycle participant |

The call boundary is the authority. Bolt proves its own prepared-order and admission preconditions before that boundary, but it does not inspect NT error strings or duplicate NT-owned status, cache-borrow, or routing validation to guess whether routing occurred. A pre-routing NT error may therefore remain conservatively occupied; that is surfaced explicitly rather than released unsafely.

### One prepared submit boundary

Shared execution owns one typed `PreparedSubmitBoundary`, not independently committed reservations. It has exactly two shapes: capital only, or capital plus the existing lifecycle participant. After all route validation, evidence, admission, time, and economics checks, each owner performs a fallible preflight that leaves its state refundable and returns an opaque prepared capability. Preflight uses one fixed acquisition order: capital first, then lifecycle. Failure drops every prepared capability through its pre-sink unwind and NT is not called.

Crossing the boundary consumes the complete aggregate in one infallible, non-panicking transition. The prepared capabilities already contain the exact reservation and lifecycle generations, so this transition performs no lookup, validation, allocation, clock read, evidence write, or fallible lock acquisition. It marks capital and the optional lifecycle participant sink-invoked, releases any internal guards, and returns a committed aggregate. Shared execution then calls NT directly, with no intervening Bolt work. No lock is held across the NT call, so synchronous callbacks can settle the committed generations. There is no state in which one participant is post-sink while another remains refundable; if that property cannot be encoded by the prepared capabilities, the route remains pre-sink and fails before NT.

### Capacity

The admission permit crosses from refundable to sink-invoked immediately before the NT submit call. Its already-committed admission-evidence receipt is consumed at that phase transition rather than waiting for `Submitted`; rollback authority exists only before the transition. No fallible Bolt work occurs between the completed participant phase transition and the direct NT call. Once that phase is reached, neither `Submitted` nor `SinkInvokedUnknown` can refund it.

The existing reservation index becomes one order-ID-keyed typed reservation record containing the immutable committed attribution, exact ledger reservation ID, lifecycle phase, and that phase's evidence baseline. It does not own a second mutable liability number. Its phase is `Reserved`, `SinkInvoked`, or `ObservedOpen`; there is no parallel unknown-reservation map. `Reserved` is refundable before the call boundary. `SinkInvoked` is a carried commitment whose authority is the completed admission plus the Bolt-to-NT call boundary. `ObservedOpen` retains the exact fresh NT open-order identity and observation timestamp that advanced it. An exact open observation may advance `SinkInvoked` to `ObservedOpen`; projection omission cannot do so.

Capital rebuild accepts one exhaustive evidence enum inside the existing gate:

- `NtOpenOrder` uses the current freshness, pool-snapshot, and liability-validation rules;
- `RetainedLifecycleReservation` carries the exact already-live ledger reservation selected by a nonterminal `SinkInvoked` or `ObservedOpen` record and cross-checks it against the original committed attribution and retained phase.

`RetainedLifecycleReservation` is not a new admission request. It therefore does not pretend that its original invocation or open-observation time is fresh NT evidence, does not copy the current projection time into either field, and is not rejected merely because the pool snapshot has advanced. The original timestamps remain lifecycle baselines and diagnostics. The carry path validates exact request, pool, collateral-group, client-order, phase, and positive-liability identity, then clones that existing live reservation into the candidate `ReservationLedger`. It does not reapply current available-balance or minimum-balance policy to erase an obligation that is already committed. If carried obligations now exceed the pool, their full numerical liability remains visible and later admissions fail closed.

The transition to `SinkInvoked` is atomic with proving that the exact live ledger reservation exists. A missing ledger entry is an invariant failure before the NT call, not a reason to reconstruct a number later. Subsequent invalidation paths preserve both objects together.

Reconciliation status gates new admission and replacement of the whole ledger; it does not make an exact existing reservation immutable. While reconciliation is invalid, two identity-preserving mutations remain available: exact pre-sink rollback of a `Reserved` generation, and exact terminal/zero-leaves retirement of a `SinkInvoked` or `ObservedOpen` generation using lifecycle evidence newer than the retained baseline. Each validates the typed record and ledger entry together. Terminal retirement records its release evidence and rechecks the projection epoch before removing both atomically; evidence failure or a newer epoch preserves both. Rollback and retirement return typed decisions, and the index record is removed only after the matching ledger removal succeeds. Missing, stale, or mismatched identity preserves both and reports an invariant failure. Partial-fill revaluation remains blocked while unreconciled, so the full prior liability stays carried until a complete rebuild or terminal retirement.

Rebuild is candidate-first and atomic. It builds a candidate ledger and candidate typed index without clearing the live state. `refresh_capital_admission_state_from_components` may update NT-derived components and call non-destructive invalidation, but it never replaces the gate. `CapitalAdmissionGate::rebuild_open_order_reservations` constructs and returns a candidate without resetting the live ledger; every rejected decision reports the preserved live liability. The candidate carries its expected projection epoch. Only a complete, valid candidate whose rebuild-evidence write succeeds and whose epoch still matches replaces both ledger and index; the final swap rechecks the epoch so a callback during evidence writing cannot be overwritten. Incomplete projection, rejected reservation evidence, fill-evidence-integrity failure, missing capital state, duplicate attribution, stale epoch, or rebuild-evidence write failure preserves the existing records and numerical ledger, calls the gate's non-destructive reconciliation invalidation, and blocks new admission. Every current-process invalidation uses that non-destructive operation; an empty `CapitalAdmissionGate::unreconciled()` is used only for initial construction or restart before any current-process reservation exists. No error path clears a nonterminal record. The merge and commit live in shared admission state, not `LiveNode`.

A complete open-order projection merges every retained nonterminal record absent from the projection as `RetainedLifecycleReservation`, whether its phase is `SinkInvoked` or `ObservedOpen`. If the exact client order is present, its fresh attribution-checked observation advances `SinkInvoked` to `ObservedOpen` or refreshes the existing `ObservedOpen` evidence; it is not inserted a second time, and an identity mismatch rejects the candidate. The shared capital-admission event feed triggers an exact cache observation and atomically removes the record and ledger reservation only when the same client order is authoritatively terminal or has zero leaves. `OrderFilled` alone is not terminal proof because it may be partial. Exact generation and projection-epoch ordering ensure that a synchronous terminal callback or newer event may retire the record before submit/rebuild completion and that stale completion cannot reinsert it. Cache absence, projection omission, elapsed time, or another rebuild failure cannot retire either nonterminal phase.

`SinkInvokedUnknown` creation emits an error-level diagnostic and typed outcome evidence. Existing admission rebuild evidence exposes one derived `unresolved_lifecycle_reservation_count` while any retained `SinkInvoked` or `ObservedOpen` record is absent from a complete projection; the evidence includes the retained phase, and logging occurs when that derived health state changes, not on every projection. This also covers a successful NT return whose order has not yet appeared and a previously observed order later omitted without terminal proof. It is observability over the one typed record, not another lifecycle authority or release path.

This keeps one accounting authority: the existing gate and ledger own both observed and carried liabilities. Merely retaining an attribution record while replacing the ledger with zero liability behind an unreconciled flag is not an acceptable substitute.

### Resting maker registration

`SinkInvokedUnknown` retains the exact provisional resting registration instead of removing it. Synchronous callbacks may retire or advance that generation before the route call returns; the return path must recognize that retirement and must not recreate the record. The retained record continues through the existing cache/event-driven coordinator.

`BoltV3RoutedNonSubmittedOutcome` excludes `SinkInvokedUnknown`, because rollback is not sound after sink invocation.

The resting transaction replaces the misleading `is_submitted()` Boolean with an exhaustive exact-generation identity disposition produced by registry settlement:

- `RetainedActive` means the exact generation is still registered, so the maker promotes the pre-minted client order ID;
- `RetiredByCallback` means a synchronous terminal callback won, so the maker consumes the pre-minted ID without promoting it;
- `NotRetained` covers pre-sink/registration rejection; invariant failure poisons the existing lifecycle, clears the pre-minted identity, and never promotes it.

Both `Submitted` and `SinkInvokedUnknown` may settle as `RetainedActive` or `RetiredByCallback`; submit classification alone cannot decide identity. The disposition is shared resting-lifecycle data and contains no strategy-specific state.

Maker cancellation remains owned by the existing tracked cancellation coordinator, which consumes the low-level `SkippedByPolicy | NtCallReturned | NtCallErrored` attempt outcome and retains or retires the exact registry record from lifecycle/cache evidence. The maker dispatch result is therefore renamed from `Canceled`/`CanceledAll` to `CancelIntentHandled`/`CancelScopeHandled`. Each handled result carries an exhaustive coordinator disposition for every active client order identity it covers: retained by the coordinator or authoritatively terminal. A missing or mismatched active identity is an invariant failure that preserves the planning binding and poisons the existing registry health; an unrouted pre-minted `next_order` may be discarded. The names do not mean NT routed a command or the order is terminal. Maker runtime clears an active planning binding only after its exact disposition is accounted for. This is not a second cancel authority and does not rely on the pinned NT method's internal `?` ordering.

### Quote and requote participants

The optional quote/resting participant crosses the same prepared aggregate boundary as capital. Its fallible arming remains pre-sink and refundable; its prepared capability makes the aggregate sink-invoked transition infallible. `SinkInvokedUnknown` uses the existing post-sink liability/invariant path: it does not refund quote budget, reopen a leg, or return a prepaid token. Later NT events settle the exact generation.

### Edge-taker entry and exit

An entry is armed through the private exposure owner before shared routing. A sink-invoked-unknown entry keeps that exact pending-entry generation. A partial fill with a potentially working remainder moves to `EntryRemainder`, never to plain `Managed`. A sink-invoked-unknown exit keeps the local exit authority and exact client order ID in the current-process pending lifecycle. Route completion reduces only the exact attempt generation still present, so a synchronous callback that already advanced or retired it wins. None becomes `Managed` or `Flat` from the synchronous error, and none records the false diagnostic that the order “did not reach the venue.”

The strategy records the typed outcome but does not decide its accounting or lifecycle settlement.

## Failure and recovery rules

- No timeout, cache absence, actor restart, or string-matched error proves that a sink-invoked command did not exist.
- `OrderDenied`, `OrderRejected`, `OrderCanceled`, `OrderExpired`, fills, and corrections are reconciled through current authoritative NT cache/lifecycle handling; a fill retires an order claim only with exact terminal or zero-leaves proof.
- A contradictory or insufficient observation preserves occupancy or moves to `BlindRecovery`; it never releases capacity or authorizes another order.
- A locally rejected command before the shared call boundary remains refundable.
- A live cancel return or error carries no release authority. Policy skip is the only route result that proves NT was not called; terminal/zero-leaves cache truth and exact lifecycle events settle cancellation.
- `OrderCancelRejected` retains the entry claim in `Refused`. It is not attributable to a cancel attempt at the pinned API and grants no retry; duplicate or replayed rejection is idempotent.
- There is no second cancellation algorithm. The edge taker uses the existing shared order-execution cancellation route for its entry remainder, while maker cancellation remains with the existing shared coordinator.

## Required behavior tests

Tests exercise public behavior and event sequences; no source-scanning test is added.

1. A sink double records that it was invoked and then returns an error. The result is `SinkInvokedUnknown`; capital, quote liability, and resting registration remain retained.
2. The same sequence for edge entry and exit leaves the exact pending client order occupied. A later evaluation produces zero additional sink calls.
3. Entry arming succeeds only from `Flat` and installs an exact owner-local generation before shared routing. Policy skip or pre-sink failure aborts only that generation; a synchronous terminal callback wins over route completion; `Submitted` and `SinkInvokedUnknown` retain it.
4. Shared submit preflight failure leaves capital and the optional lifecycle participant refundable. Aggregate commitment changes both to sink-invoked without an intervening fallible step; a sink error afterward retains both. A synchronous NT callback that advances or retires a participant before submit returns cannot be overwritten by either route outcome.
5. Maker rotation promotes only `RetainedActive`; synchronous `RetiredByCallback` consumes the pre-minted identity without creating an active binding for both submit outcomes.
6. A current-process exit whose cached order is temporarily absent remains occupied, while a later exact cached terminal observation retires through the existing reducer. Later trading triggers produce zero additional exit calls before retirement.
7. Startup with any cached open position enters `BlindRecovery` and routes nothing, including zero attributed exit orders, a recovered settled-position key, and prior booking-terminal evidence. A newly observed position without current-process lineage does the same. Later position/cache events cannot materialize any of them as `Managed` or `Flat`.
8. A partial-fill entry remainder blocks every exit, including forced flat. Supported, unsupported, and canonically flat projections all retain the same exact order identity and may issue cancellation only through the one transition. Position close, settlement success, and booking-terminal settlement cannot release the order claim.
9. Cancel policy skip restores only the exact entry identity. A live NT return and a live NT error remain `Pending`; missing or unchanged working cache observations do not retry. Exact terminal/zero-leaves truth settles it and a synchronous terminal callback cannot be overwritten. `OrderCancelRejected` changes the exact retained identity to `Refused`; duplicate, replayed, or later rejection events issue zero additional cancel calls. Zero exit calls occur before entry retirement plus coherent supported position truth.
10. A carried sink-invoked reservation whose original admission timestamp predates a newer pool projection rebuilds without `StaleRequest`, keeps that original timestamp, remains numerically present in `live_reserved_liability`, and reduces available capacity by its exact carried liability.
11. A `SinkInvoked` record absent from a complete projection remains reserved and exposes nonzero `unresolved_lifecycle_reservation_count`. After one exact open observation advances it to `ObservedOpen`, later projection omission carries the same ledger reservation and remains unresolved rather than releasing it.
12. Incomplete projection, rejected rebuild input, fill-evidence-integrity failure, missing capital state, duplicate attribution, component refresh, and evidence-write failure preserve the record and numerical liability while invalidating admission reconciliation. Rejection evidence reports the preserved live liability. While invalid, exact `Reserved` rollback and evidence-recorded exact terminal retirement remove ledger and record together; stale or mismatched removal and terminal-evidence write failure preserve both.
13. A terminal callback that retires a record before a stale rebuild completes cannot be reinserted by that rebuild.
14. A fill void after local authority retirement enters `BlindRecovery` and routes nothing.
15. A terminal zero-fill exit and a positive-fill residual exit continue through their existing current-process lifecycle, with the proven residual remanaged exactly once.
16. Maker registration and quote-budget tests prove that an error after invocation cannot use rollback paths reserved for pre-sink failure.
17. Maker `CancelIntentHandled` and `CancelScopeHandled` clear only active planning identities whose exact coordinator disposition is retained or authoritatively terminal. Missing or mismatched identity preserves the binding and poisons registry health; neither handled outcome is terminal or route evidence.

## Conditional-debt constraint

The primary census counts production source lines containing the lexical Rust keyword `if` or `match` after stripping comments and string/character literals. It covers `src/**` and `crates/*/src/**`, excludes test paths and complete `#[cfg(test)]` items, and reports added, removed, and net lines per file.

The immutable endpoints are:

- implementation repair: `23960a0dcf4232c818db4b539a41ac5b4bb928d7..IMPLEMENTATION_HEAD`;
- complete PR: `e62584045629208e81d2dce1fce608720ea01fbf..IMPLEMENTATION_HEAD`.

`IMPLEMENTATION_HEAD` means the exact final production commit named in the implementation review record, not the current documentation head.

`623801311` is the historical pre-takeover comparison only. If reported, it is labeled that way and is never called the complete PR range. The implementation review record includes the exact scanner source, its SHA-256, the invocation, and raw per-file output for both required ranges. The scanner is review evidence, not a repository test or a new verifier subsystem.

A companion decision-construct report has three explicit line-count columns over the same endpoints: `match_arm_lines` counts distinct lines containing `=>`; `alternate_conditional_lines` counts distinct lines containing `matches!`, `&&`, `||`, `is_some_and`, `is_none_or`, `map_or`, `filter`, `and_then`, `or_else`, or `then_some`; and `companion_union_lines` counts the union of those two sets, once per source line. The hard companion budget below applies to `companion_union_lines`; the two component columns remain visible so exhaustiveness and syntax substitution cannot hide inside one total. Review rejects moving a reducer between files or replacing `if`/`match` with an equivalent combinator merely to improve any number.

This repair is deletion-led, with hard review constraints:

- no route or release decision is added as a new Boolean/projection chain under `src/strategies/`; authority decisions use the typed reducers and exhaustive enum arms;
- strategy restart/reconstruction functions and their branches are deleted rather than renamed or wrapped;
- the shared submit change adds only exhaustive enum/disposition arms needed for the replacement outcome;
- exit routing uses one exhaustive transition over explicit states, not multiple Boolean/projection checks;
- the implementation repair range is net negative for both the primary count and `companion_union_lines` under `src/strategies/`, including separately net-negative combined results for `binary_oracle_edge_taker/mod.rs` plus `exposure.rs`; there is no per-file quota that rewards moving the reducer;
- the actual complete PR range from `e62584045629208e81d2dce1fce608720ea01fbf` ends below a net `+250` primary `if`/`match` lines;
- no wildcard hides a future variant in an authority-mutating exposure reducer, resting-identity disposition, submit-outcome match, reservation rebuild evidence match, or cancel-settlement match. Read-only projections are not expanded merely to satisfy a source metric.

The census is required review evidence but is not a source-scanning test and is not a substitute for sequence tests or structural review. Safety-required states and arms are not removed or hidden to meet the budget. If either repair-range budget or the actual-base target cannot be met by deleting obsolete lifecycle/recovery policy inside this slice, implementation stops and returns for an explicit scope decision instead of refactoring unrelated code or changing syntax to game the count.

## Atomic cutover

One production commit performs the behavior cutover:

- adds `SinkInvokedUnknown` settlement;
- replaces independently fallible participant commitment with the one prepared submit boundary and changes all shared participants and admission capacity to retain after invocation through one atomic candidate rebuild;
- replaces the shared cancel `Result`/`Canceled` surface with the exhaustive `SkippedByPolicy | NtCallReturned | NtCallErrored` attempt outcome and makes it non-authoritative for lifecycle settlement;
- renames maker coordinator results to `CancelIntentHandled`/`CancelScopeHandled` so clearing a planning binding cannot be read as terminal or NT-route evidence;
- adds the exact-generation resting identity disposition;
- changes edge entry/exit settlement to remain occupied and moves every runtime `Flat` transition into the local release reducer;
- replaces optional managed-entry and restart-origin fields with the typed local `EntryRemainder` state;
- deletes recovered exit authority and strategy reconstruction in the same commit;
- replaces reconstruction tests with the required non-routing sequences.

No intermediate commit may contain both reconstructed authority and the final non-routing recovery path. Documentation and test-fixture preparation may precede the cutover only when inert. One production preparatory commit may encapsulate the existing edge-taker `ExposureState` behind the private owner, move existing constructors/mutations into it, and update callers without changing any variant, transition, route result, or release behavior. That commit must leave no parallel direct-mutation path and must carry structural-equivalence evidence plus the existing behavior suite. The behavior cutover above remains atomic; new states, new outcomes, and deletion of reconstructed authority do not land in the preparatory commit.

## Explicit non-goals

- no generic cross-strategy exposure authority;
- no durable Bolt order-claim journal;
- no NT patch, pin change, or second attempt at #4790/#4791;
- no claim that restart can safely resume trading with an existing position;
- no direct maker NT-event-to-leg event fence from #869;
- no economics Slices 2–5;
- no live, deploy, readiness, or trading authorization.

Crash-safe restart authority is not accepted scope of #1445 and is not left as an unfinished part of #1544. If it is proposed later, it requires its own approved issue and an upstream RFC centered on reconciliation completeness and attribution, not submit handoff. Closed issue #4790 and PR #4791 are historical evidence for that future discussion, not an active dependency.

## Verification and review evidence

The implementation plan must map each rule above to behavior evidence. At minimum:

- focused shared submit/admission, maker registration, quote lifecycle, and edge exposure tests;
- all existing maker/taker integration targets affected by the outcome enum;
- root workspace formatting, Clippy with warnings denied, and tests through the repository's remote-first workflow;
- isolated backtesting checks when the changed shared surface compiles there;
- `git diff --check` for the repair and complete PR ranges;
- symbol inspection proving recovered-authority production surfaces are deleted;
- symbol inspection proving the private exposure owner is the only runtime `Flat` producer and current-process admission invalidation no longer clears live typed records or replaces their ledger with an empty gate;
- structural inspection proving entry creation and route settlement use only the exact `arm_entry` capability, the prepared submit aggregate has no fallible work after commitment, and no shared module learns edge-taker state;
- behavior evidence that unreconciled-but-populated ledgers still permit exact `Reserved` rollback and evidence-gated terminal retirement while rejecting new admission, and that both omitted nonterminal phases remain numerically carried;
- maker cancellation evidence that every cleared active binding has an exact coordinator disposition and no handled outcome is treated as NT-route or terminal proof;
- exact-range conditional census with per-file totals;
- internal adversarial review before external review;
- stable PR-body disclosure of this lifecycle-first scope and the unchanged #869/Slices 2–5 remainder.

Exact-head verification belongs in the review request or review record, not the PR body.
