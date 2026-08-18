# PR #1544 Lifecycle-First Scope-Back

## Decision

PR #1544 remains the atomic economics quote/admission cutover owned by #1445. It keeps the economics, provider, replay, admission, maker, cancellation, and OMS work already in the PR. It does not add a generic exposure runtime, a restart journal, or strategy-owned reconstruction of order authority.

The repair has two parts:

1. Shared order execution treats every error returned after it calls NautilusTrader submit as `SinkInvokedUnknown`. Reservations and lifecycle participants remain committed until authoritative NT lifecycle evidence retires them.
2. The edge taker keeps only authority created by its current process. If that authority is unavailable or contradictory, it enters the existing non-routing blind-recovery state. It never reconstructs submit authority from cache observations.

This is a scope reduction, not a replacement architecture.

## Why this boundary

NautilusTrader owns order lifecycle, cache, and reconciliation. Bolt can prove its own pre-sink work, but after calling `Strategy::submit_order` it cannot infer from the returned `Result<()>` whether the command remained local or entered routing. The pinned implementation can route the command and then fail while installing the GTD timer.

The attempted upstream API in nautechsystems/nautilus_trader#4790 and PR #4791 tried to expose that handoff. Both were closed with maintainer direction that reservations should follow order lifecycle rather than a handoff result. At the pinned revision, some local denials enter the cache and publish lifecycle events while some errors returned before routing do not. Bolt cannot distinguish those cases from the returned error, so this design follows lifecycle evidence and does not reopen the same API downstream or require an NT pin change.

The conservative consequence is intentional: if NT returns an error after Bolt invokes submit and no authoritative lifecycle evidence follows, the claim remains occupied. A timeout or cache miss is not proof of absence.

## Ownership after the repair

| Owner | Responsibility |
| --- | --- |
| Strategy | Signal, intent, cooldown, and strategy-facing evidence |
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
- startup adoption of a cached exit as newly minted Bolt authority;
- startup materialization of a cached position as routeable `Managed` or `UnsupportedObserved` state;
- fill-void reconstruction after the original current-process authority has retired;
- the cache-absence and correction arms of timer reconciliation that attempt to rebuild or release authority.

The associated strategy functions and evidence branches are deleted, including `bootstrapped_exposure_for`, `adopt_restart_open_exit_order_from_cache`, `recover_exit_authority_state`, `enter_exit_authority_recovery_hold`, `try_recover_exit_authority_hold`, and `try_release_exit_authority_recovery_flat`. Tests that assert reconstruction are replaced by behavior tests for non-routing recovery.

The locally submitted exit authority remains. It exists only for the process that created it, carries the exact order and position identity, and is retired only by the existing current-process lifecycle reducer.

Timer reconciliation remains only for that retained local authority. An exact cached `Working` observation preserves it, and an exact cached terminal observation may advance its existing reducer. A missing cached order leaves the state unchanged and emits evidence; it neither constructs a hold nor releases authority. Fill-void correction after retirement enters `BlindRecovery` instead of using the timer to mint authority.

### Restart behavior

Startup may consume canonical NT facts for observation and evidence, but it cannot mint a Bolt submit claim from them. Any cached open position, possibly related order, conflicting projection, or insufficient proof that no prior exit remains live places the edge taker in `BlindRecovery`, which routes no entry, exit, cancel, or modify. In particular, a cached position never becomes routeable `Managed`, and an empty attributed-exit-order list is not proof that the position has no live exit.

The existing clean-start case with zero scoped positions remains `Flat`; #1544 does not claim that this is crash-safe restart proof. Once startup enters `BlindRecovery`, later cache absence or position rematerialization cannot turn it into `Managed`, `ExitPending`, or `Flat`. Returning that state to routing requires a future separately approved capability with stronger proof.

### Running fill-void behavior

If a fill correction arrives while the original local exit authority is still retained, the existing authority and lifecycle reducer handle it. If the authority has already retired, the correction enters `BlindRecovery`. The strategy does not search the cache and construct a replacement authority.

### Missing current-process cache observation

A missing cached order does not discard a retained local authority and does not create a replacement one. The strategy keeps the original occupied lifecycle state and waits for authoritative terminal or rejection evidence. It does not route a second exit.

## Explicit exposure states and one exit-routing transition

`ManagedPositionContext` contains only the position. It has neither `origin` nor `pending_entry: Option<_>`. A partially filled entry whose remainder may still work is represented by explicit strategy-local states:

- `ManagedWithEntryRemainder { position, pending_entry }` before cancellation is requested;
- `ManagedEntryCancellationPending { position, pending_entry }` installed before invoking the existing shared order-execution cancellation route.
- `UnsupportedObservedWithEntryRemainder { position, pending_entry, reason }` when current-process entry lineage materializes an unsupported position; it retains the exact entry identity but routes no order operation.

All three states remain occupied. An updated fill changes the explicit position without dropping the entry identity. Exact terminal or zero-leaves order evidence moves an entry-remainder state only after the same reducer obtains a coherent canonical position projection: a supported open projection becomes `Managed`, an unsupported open projection remains non-routing, a flat projection becomes `Flat`, and missing or contradictory position truth remains occupied in the existing entry-reconciliation state. If the position becomes flat before order-terminal proof, the state becomes the existing pending-entry/reconciliation state rather than `Flat`.

Routeable `Managed` may be constructed only by consuming a current-process pending-entry claim or by updating an already current-process managed/exit state. A newly observed position without that lineage enters `BlindRecovery`. Position materialization exhaustively preserves `BlindRecovery`; later cache or position events cannot smuggle restart or external state back into routing.

Exit eligibility is no longer assembled from correlated projections such as “has a managed position,” “has no exit snapshot,” and “has no pending entry.” One strategy-local transition owns the decision:

```text
Managed -> ExitAttempting
ManagedWithEntryRemainder -> request exact entry cancellation; no exit
ManagedEntryCancellationPending -> typed non-routing outcome; no exit
every other exposure state -> typed non-routing outcome, state unchanged
```

The forced-flat path follows the same transition. It cannot bypass the working-entry state, fire an asynchronous cancellation, and immediately submit an exit. The reducer installs `ManagedEntryCancellationPending` before the cancel call; a proven pre-sink cancel failure may restore `ManagedWithEntryRemainder` only if that exact client-order state is still present. A synchronous terminal callback wins and cannot be overwritten by the return path. Only exact terminal or zero-leaves evidence for the entry remainder plus coherent canonical position truth permits `Managed`, after which one later evaluation may route the exit. Cancellation uses the existing shared order-execution policy; this design adds no cancellation map or algorithm.

The transition exhaustively matches `ExposureState`. In particular, `ExitAttempting`, `ExitPending`, `TerminalExitAwaitingPosition`, `BlindRecovery`, entry states, entry-remainder states, and unsupported observations cannot yield another exit attempt. Route code receives the managed position only from this transition.

This removes the current projection mismatch where a recovery hold appears position-bearing to one helper but not exit-pending to another. It also prevents another helper pair from recreating the same bug.

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

### Capacity

The admission permit crosses from refundable to sink-invoked immediately before the NT submit call. Its already-committed admission-evidence receipt is consumed at that phase transition rather than waiting for `Submitted`; rollback authority exists only before the transition. No fallible Bolt work occurs between the completed participant phase transition and the direct NT call. Once that phase is reached, neither `Submitted` nor `SinkInvokedUnknown` can refund it.

The existing reservation index becomes one typed reservation record containing the attribution and liability needed to survive an NT projection. Its phase is `Reserved` or `SinkInvoked`; there is no parallel unknown-reservation map. A complete open-order projection merges any still-retained sink-invoked record that is absent from the projection instead of silently clearing it. An incomplete projection keeps the typed records, marks the existing admission gate unreconciled, and blocks admission until a complete rebuild; it does not clear the records. The merge lives in shared admission state, not `LiveNode`.

The shared capital-admission event feed triggers an exact cache observation and retires the record only when the same client order is authoritatively terminal or has zero leaves. `OrderFilled` alone is not terminal proof because it may be partial. Exact generation and projection-epoch ordering ensure that a synchronous terminal callback or newer event may retire the record before submit/rebuild completion and that stale completion cannot reinsert it. Cache absence, projection omission, or elapsed time cannot retire it.

`SinkInvokedUnknown` records are not silent. Creation emits an error-level diagnostic and typed outcome evidence. Existing admission rebuild evidence exposes a derived `unresolved_sink_invocation_count` while a retained sink-invoked record is absent from a complete projection; logging occurs when that derived health state changes, not on every projection. This is observability over the one typed record, not another lifecycle authority or release path.

This ensures both the numerical reservation and the admission gate remain occupied; merely setting liability to zero behind an unreconciled flag is not an acceptable substitute.

### Resting maker registration

`SinkInvokedUnknown` retains the exact provisional resting registration instead of removing it. Synchronous callbacks may retire or advance that generation before the route call returns; the return path must recognize that retirement and must not recreate the record. The retained record continues through the existing cache/event-driven coordinator.

`BoltV3RoutedNonSubmittedOutcome` excludes `SinkInvokedUnknown`, because rollback is not sound after sink invocation.

The resting transaction replaces the misleading `is_submitted()` Boolean with an exhaustive exact-generation identity disposition produced by registry settlement:

- `RetainedActive` means the exact generation is still registered, so the maker promotes the pre-minted client order ID;
- `RetiredByCallback` means a synchronous terminal callback won, so the maker consumes the pre-minted ID without promoting it;
- `NotRetained` covers pre-sink/registration rejection; invariant failure poisons the existing lifecycle, clears the pre-minted identity, and never promotes it.

Both `Submitted` and `SinkInvokedUnknown` may settle as `RetainedActive` or `RetiredByCallback`; submit classification alone cannot decide identity. The disposition is shared resting-lifecycle data and contains no strategy-specific state.

### Quote and requote participants

Every participant is marked sink-invoked before NT submit. `SinkInvokedUnknown` uses the existing post-sink liability/invariant path: it does not refund quote budget, reopen a leg, or return a prepaid token. Later NT events settle the exact generation.

### Edge-taker entry and exit

A sink-invoked-unknown entry keeps its pending-entry occupancy. A partial fill with a potentially working remainder moves to `ManagedWithEntryRemainder`, never to plain `Managed`. A sink-invoked-unknown exit keeps the local exit authority and exact client order ID in the current-process pending lifecycle. Route completion reduces only the exact attempt generation still present, so a synchronous callback that already advanced or retired it wins. None becomes `Managed` or `Flat` from the synchronous error, and none records the false diagnostic that the order “did not reach the venue.”

The strategy records the typed outcome but does not decide its accounting or lifecycle settlement.

## Failure and recovery rules

- No timeout, cache absence, actor restart, or string-matched error proves that a sink-invoked command did not exist.
- `OrderDenied`, `OrderRejected`, `OrderCanceled`, `OrderExpired`, fills, and corrections are reconciled through current authoritative NT cache/lifecycle handling; a fill retires an order claim only with exact terminal or zero-leaves proof.
- A contradictory or insufficient observation preserves occupancy or moves to `BlindRecovery`; it never releases capacity or authorizes another order.
- A locally rejected command before the shared call boundary remains refundable.
- There is no second cancellation algorithm. The edge taker uses the existing shared order-execution cancellation route for its entry remainder, while maker cancellation remains with the existing shared coordinator.

## Required behavior tests

Tests exercise public behavior and event sequences; no source-scanning test is added.

1. A sink double records that it was invoked and then returns an error. The result is `SinkInvokedUnknown`; capital, quote liability, and resting registration remain retained.
2. The same sequence for edge entry and exit leaves the exact pending client order occupied. A later evaluation produces zero additional sink calls.
3. A pre-sink failure produces the existing pre-sink outcome and refunds/removes only its exact provisional generation.
4. A synchronous NT callback that advances or retires a participant before submit returns cannot be overwritten by either `Submitted` or `SinkInvokedUnknown` settlement.
5. Maker rotation promotes only `RetainedActive`; synchronous `RetiredByCallback` consumes the pre-minted identity without creating an active binding for both submit outcomes.
6. A current-process exit whose cached order is temporarily absent remains occupied, while a later exact cached terminal observation retires through the existing reducer. Later trading triggers produce zero additional exit calls before retirement.
7. Startup with a cached open position and zero attributed exit orders enters `BlindRecovery` and routes nothing. A newly observed position without current-process entry/managed lineage does the same. Later position/cache events cannot materialize either as `Managed`.
8. A partial-fill entry remainder blocks every exit, including forced flat. Forced flat requests cancellation exactly once through the existing route, zero exit calls occur before exact entry terminal proof plus coherent canonical position truth, and one later exit may route after the state becomes plain `Managed`. A synchronous cancel-terminal callback cannot be overwritten by cancel return, and position-flat-before-entry-terminal remains occupied by the entry claim.
9. A sink-invoked record absent from a complete projection remains reserved and exposes nonzero `unresolved_sink_invocation_count`; an incomplete projection preserves the record and leaves the gate unreconciled.
10. A fill void after local authority retirement enters `BlindRecovery` and routes nothing.
11. A terminal zero-fill exit and a positive-fill residual exit continue through their existing current-process lifecycle, with the proven residual remanaged exactly once.
12. Maker registration and quote-budget tests prove that an error after invocation cannot use rollback paths reserved for pre-sink failure.

## Conditional-debt constraint

The conditional census counts production source lines containing the lexical Rust keyword `if` or `match` after stripping comments and string/character literals. It covers `src/**` and `crates/*/src/**`, excludes test paths and `#[cfg(test)]` items, and reports added, removed, and net lines per file for both the exact repair range and the complete PR range.

This repair is deletion-led, with hard review constraints:

- no new production `if` keyword line is added under `src/strategies/`; explicit enum arms are used instead of correlated predicates;
- strategy restart/reconstruction functions and their branches are deleted rather than renamed or wrapped;
- the shared submit change adds only exhaustive enum/disposition arms needed for the replacement outcome;
- exit routing uses one exhaustive transition over explicit states, not multiple Boolean/projection checks;
- the exact repair range is net negative for production `if`/`match` lines under `src/strategies/`, including separately net-negative results for `binary_oracle_edge_taker/mod.rs` and `exposure.rs`;
- the complete PR range ends below a net `+250` production `if`/`match` lines;
- no wildcard hides a future variant in the position-materialization reducer, exit-routing reducer, resting-identity disposition, or submit-outcome matches. Existing projection helpers outside those authorities are not expanded merely to satisfy a source metric.

The census is required review evidence but is not a source-scanning test and is not a substitute for the sequence tests. If a safety repair needs an explicit state or match arm, correctness wins; equivalent obsolete branches must then be deleted so the hard net budgets still hold.

## Atomic cutover

One production commit performs the behavior cutover:

- adds `SinkInvokedUnknown` settlement;
- changes all shared participants and admission capacity to retain after invocation;
- adds the exact-generation resting identity disposition;
- changes edge entry/exit settlement to remain occupied;
- replaces optional managed-entry and restart-origin fields with explicit non-routeable states;
- deletes recovered exit authority and strategy reconstruction in the same commit;
- replaces reconstruction tests with the required non-routing sequences.

No intermediate commit may contain both reconstructed authority and the final non-routing recovery path. Documentation and test-fixture preparation may precede the cutover only when inert.

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
- exact-range conditional census with per-file totals;
- internal adversarial review before external review;
- stable PR-body disclosure of this lifecycle-first scope and the unchanged #869/Slices 2–5 remainder.

Exact-head verification belongs in the review request or review record, not the PR body.
