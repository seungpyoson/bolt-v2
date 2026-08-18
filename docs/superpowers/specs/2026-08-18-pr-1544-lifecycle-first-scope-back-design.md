# PR #1544 Lifecycle-First Scope-Back

## Decision

PR #1544 remains the atomic economics quote/admission cutover owned by #1445. It keeps the economics, provider, replay, admission, maker, cancellation, and OMS work already in the PR. It does not add a generic exposure runtime, a restart journal, or strategy-owned reconstruction of order authority.

The repair has two parts:

1. Shared order execution treats every error returned after it calls NautilusTrader submit as `SinkInvokedUnknown`. Reservations and lifecycle participants remain committed until authoritative NT lifecycle evidence retires them.
2. The edge taker keeps only authority created by its current process. If that authority is unavailable or contradictory, it enters the existing non-routing blind-recovery state. It never reconstructs submit authority from cache observations.

This is a scope reduction, not a replacement architecture.

## Why this boundary

NautilusTrader owns order lifecycle, cache, and reconciliation. Bolt can prove its own pre-sink work, but after calling `Strategy::submit_order` it cannot infer from the returned `Result<()>` whether the command remained local or entered routing. The pinned implementation can route the command and then fail while installing the GTD timer.

The attempted upstream API in nautechsystems/nautilus_trader#4790 and PR #4791 tried to expose that handoff. Both were closed with maintainer direction that reservations should follow order lifecycle: local aborts never enter the cache, while routed orders end in `OrderDenied` or `OrderRejected`. This design follows that model. It does not reopen the same API downstream or require an NT pin change.

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
- startup adoption of a cached exit as newly minted Bolt authority;
- fill-void reconstruction after the original current-process authority has retired;
- timer-driven cache classification that attempts to rebuild or release that authority.

The associated strategy functions and evidence branches are deleted, including the `recover_exit_authority_state`, `enter_exit_authority_recovery_hold`, `try_recover_exit_authority_hold`, and `try_release_exit_authority_recovery_flat` families. Tests that assert reconstruction are replaced by behavior tests for non-routing recovery.

The locally submitted exit authority remains. It exists only for the process that created it, carries the exact order and position identity, and is retired only by the existing current-process lifecycle reducer.

### Restart behavior

Startup may consume canonical NT facts for observation and evidence, but it cannot mint a Bolt submit claim from them. If startup sees an open position, a possibly related order, conflicting projection, or insufficient proof that no prior exit remains live, the edge taker enters `BlindRecovery` and routes no entry, exit, cancel, or modify.

Restart recovery cannot become `Managed`, `ExitPending`, or `Flat` merely because an order is absent from the current cache. Returning to a routeable state requires a future separately approved capability with proof strong enough for that transition; #1544 makes no such claim.

### Running fill-void behavior

If a fill correction arrives while the original local exit authority is still retained, the existing authority and lifecycle reducer handle it. If the authority has already retired, the correction enters `BlindRecovery`. The strategy does not search the cache and construct a replacement authority.

### Missing current-process cache observation

A missing cached order does not discard a retained local authority and does not create a replacement one. The strategy keeps the original occupied lifecycle state and waits for authoritative terminal or rejection evidence. It does not route a second exit.

## One exit-routing transition

Exit eligibility is no longer assembled from correlated projections such as “has a managed position” plus “has no exit snapshot.” One strategy-local transition owns the decision:

```text
Managed -> ExitAttempting
every other exposure state -> typed non-routing outcome, state unchanged
```

The transition exhaustively matches `ExposureState`. In particular, `ExitAttempting`, `ExitPending`, `TerminalExitAwaitingPosition`, `BlindRecovery`, entry states, and unsupported observations cannot yield another exit attempt. Route code receives the managed position only from this transition.

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

The call boundary is the authority. Bolt does not inspect NT error strings or duplicate NT validation to guess whether routing occurred.

### Capacity

The admission permit crosses from refundable to sink-invoked immediately before the NT submit call. Once that phase is reached, neither `Submitted` nor `SinkInvokedUnknown` can refund it.

The existing reservation index becomes one typed reservation record containing the attribution and liability needed to survive an NT projection. Its phase is `Reserved` or `SinkInvoked`; there is no parallel unknown-reservation map. A complete open-order projection merges any still-retained sink-invoked record that is absent from the projection instead of silently clearing it. The shared capital-admission event feed triggers an exact cache observation and retires the record only when the same client order is authoritatively terminal or has zero leaves. `OrderFilled` alone is not terminal proof because it may be partial. A synchronous terminal callback may retire the record before submit returns, and completion cannot reinsert it. Cache absence, projection omission, or elapsed time cannot retire it.

This ensures both the numerical reservation and the admission gate remain occupied; merely setting liability to zero behind an unreconciled flag is not an acceptable substitute.

### Resting maker registration

`SinkInvokedUnknown` retains the exact provisional resting registration instead of removing it. Synchronous callbacks may retire or advance that generation before the route call returns; the return path must recognize that retirement and must not recreate the record. The retained record continues through the existing cache/event-driven coordinator.

`BoltV3RoutedNonSubmittedOutcome` excludes `SinkInvokedUnknown`, because rollback is not sound after sink invocation.

The resting transaction replaces the misleading `is_submitted()` decision helper with `retains_order_identity()`. It is true for both `Submitted` and `SinkInvokedUnknown`, so the maker promotes the pre-minted client order ID to the active slot in either case. Evidence can still distinguish the two typed outcomes; identity retention does not assert submission.

### Quote and requote participants

Every participant is marked sink-invoked before NT submit. `SinkInvokedUnknown` uses the existing post-sink liability/invariant path: it does not refund quote budget, reopen a leg, or return a prepaid token. Later NT events settle the exact generation.

### Edge-taker entry and exit

A sink-invoked-unknown entry keeps its pending-entry occupancy. A sink-invoked-unknown exit keeps the local exit authority and exact client order ID in the current-process pending lifecycle. Neither becomes `Managed` or `Flat` from the synchronous error, and neither records the false diagnostic that the order “did not reach the venue.”

The strategy records the typed outcome but does not decide its accounting or lifecycle settlement.

## Failure and recovery rules

- No timeout, cache absence, actor restart, or string-matched error proves that a sink-invoked command did not exist.
- `OrderDenied`, `OrderRejected`, `OrderCanceled`, `OrderExpired`, `OrderFilled`, and corrections are reconciled through current authoritative NT cache/lifecycle handling.
- A contradictory or insufficient observation preserves occupancy or moves to `BlindRecovery`; it never releases capacity or authorizes another order.
- A locally rejected command before the shared call boundary remains refundable.
- There is no second cancellation algorithm. Existing shared maker cancellation remains the only cancellation owner in this slice.

## Required behavior tests

Tests exercise public behavior and event sequences; no source-scanning test is added.

1. A sink double records that it was invoked and then returns an error. The result is `SinkInvokedUnknown`; capital, quote liability, and resting registration remain retained.
2. The same sequence for edge entry and exit leaves the exact pending client order occupied. A later evaluation produces zero additional sink calls.
3. A pre-sink failure produces the existing pre-sink outcome and refunds/removes only its exact provisional generation.
4. A synchronous NT callback that advances or retires a participant before submit returns cannot be overwritten by either `Submitted` or `SinkInvokedUnknown` settlement.
5. A current-process exit whose cached order is temporarily absent remains occupied, and later trading triggers produce zero additional exit calls.
6. Startup with an open position/order ambiguity enters `BlindRecovery` and routes nothing.
7. A fill void after local authority retirement enters `BlindRecovery` and routes nothing.
8. A terminal zero-fill exit and a positive-fill residual exit continue through their existing current-process lifecycle, with the proven residual remanaged exactly once.
9. Maker registration and quote-budget tests prove that an error after invocation cannot use rollback paths reserved for pre-sink failure.

## Conditional-debt constraint

This repair is deletion-led:

- no production conditional is added under `src/strategies/`; existing submit-outcome arms are renamed or deleted while recovery arms are removed;
- strategy restart/reconstruction functions and their branches are deleted rather than renamed or wrapped;
- the shared submit change adds only exhaustive enum arms needed for the new outcome;
- exit routing uses one exhaustive transition, not multiple boolean/projection checks;
- no wildcard hides a future `ExposureState` or submit-outcome variant.

The final review records a production conditional census for the exact repair range and the complete PR range, but the census is diagnostic evidence, not a substitute for the sequence tests.

## Atomic cutover

One production commit performs the behavior cutover:

- adds `SinkInvokedUnknown` settlement;
- changes all shared participants and admission capacity to retain after invocation;
- changes edge entry/exit settlement to remain occupied;
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
