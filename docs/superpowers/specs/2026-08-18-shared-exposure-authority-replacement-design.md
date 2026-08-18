# Shared Exposure Authority Replacement Design

**Date:** 2026-08-18

**Owning issue:** #1445

**Pull request:** #1544

**Supersedes:** the strategy-local exposure-authority design in
`2026-08-10-economics-slice-1-review-repairs-design.md` and its follow-up repair
design in `2026-08-18-economics-slice-1-external-review-repairs-design.md`

**Status:** User-approved direction; written design awaiting user review

## Decision

Delete the strategy-local `GovernedExposure` implementation. Replace it with one
shared, concrete exposure-authority module under Bolt's existing order-execution
module.

The replacement is not a renamed or relocated version of the current reducer.
It has a different model:

- NautilusTrader remains the authority for orders, positions, fills, cache
  state, and reconciliation.
- Bolt stores only the authority that NT cannot provide before or during a
  submit: a provisional entry claim, an unresolved post-sink claim, and retained
  Bolt exit proof.
- Current occupancy is composed from authoritative NT position/order facts and
  those small Bolt claims. It is not represented as a single strategy-owned
  lifecycle enum.
- Strategies provide signals and order intent. They do not implement, inspect,
  or reconcile execution lifecycle state.

The new module is shared because ownership belongs in shared execution, not
because a hypothetical adapter interface is useful. There will be one concrete
implementation and no strategy adapter trait.

## Plain-English outcome

Today the edge-taker strategy decides whether to trade and also maintains a
second interpretation of whether its orders and positions are pending, filled,
closed, recovering, or uncertain. That duplicates NT and shared execution.

After this replacement:

1. The strategy decides that it wants to enter or exit.
2. Shared execution checks a generic exposure capability bound to the loaded
   strategy instance.
3. For an entry, shared execution atomically reserves the right to submit before
   the sink can run.
4. NT remains the source for whether an order exists and whether a position is
   open.
5. Shared execution keeps the reservation only while NT truth is unavailable or
   while a submitted order can still create exposure.
6. The strategy receives a submitted, rejected, or blocked result and records
   its signal evidence. It never advances an execution state machine.

The same shared interface can be configured for any strategy. It imports no
edge-taker, maker, oracle, binary-market, or book types.

## Evidence for replacement

At the current branch head:

- `src/strategies/binary_oracle_edge_taker/exposure.rs` is 4,674 lines; it was
  442 lines at `623801311`.
- `src/strategies/binary_oracle_edge_taker/mod.rs` grew from 9,456 to 11,129
  lines over the same range.
- production references to `ExposureState::` number 733 inside `exposure.rs`
  and 45 inside the strategy module.
- the strategy reads an exposure projection or raw exposure state at 44 call
  sites.
- `BlindRecovery`, `OperationSinkUnknown`, and `ObligationSaturated` retain
  recursive `Box<ExposureState>` values, which require separate recursive
  collectors, projections, rebasing operations, and retained-state reducers.
- the exact production-Rust conditional census for `623801311..83ffe0f83` is
  net `+424` `if`/`match`-bearing lines. Exposure contributes `+222`, and the
  edge-taker strategy module contributes `+56`.
- commit `f36d7b6cd`, which introduced the governed strategy exposure reducer,
  alone added net `+255` conditional-bearing production lines.

The design that produced this code explicitly required one exhaustive
state-by-event reducer. That made mutation private, but it created a shallow
module: callers still need many projections and lifecycle facts, while the
implementation repeats the same identity and phase decisions across direct,
retained, recovering, and correction paths.

This also conflicts with the repository ownership model. NT owns order and
position lifecycle and reconciliation. Bolt may add fail-closed admission and
audit authority, but a strategy-local shadow lifecycle is not strategy signal
state.

## Scope

This design owns the exposure-authority portion of the #1445 review repair:

- concurrent entry prevention;
- exact pre-sink rollback;
- fail-closed post-sink uncertainty;
- pending-entry and open-position occupancy;
- risk-reducing exit authority and terminal release;
- late fill/correction authority;
- restart reconstruction from NT truth;
- one-position policy configured independently of a strategy implementation;
- deletion of the strategy-local reducer and its strategy-local obligation
  configuration.

The design preserves the already accepted economics, fee, maker-command,
requote-budget, and tracked-maker-order changes except where their generic
submit interface must accept the new exposure capability.

## Non-goals

- Do not implement #869's deferred maker NT-event-to-lifecycle event fence.
- Do not implement economics Slices 2-5.
- Do not change strategy entry or exit signal policy.
- Do not create a general workflow engine or state-machine framework.
- Do not add a trait with only one adapter.
- Do not recreate an NT order, position, portfolio, or reconciliation cache.
- Do not preserve the old exposure interface as a compatibility path.
- Do not reduce conditional counts by replacing `if` with equivalent syntax.

## Ownership after the cutover

### NautilusTrader

NT owns:

- order state and order events;
- fill state and corrections;
- position state and position events;
- cache contents;
- portfolio/account state;
- venue translation and reconciliation.

An NT event is a reconciliation trigger. Releasing a Bolt claim uses the
authoritative cached order and canonical position authority; it does not trust
the event payload alone when money-moving truth can still be uncertain.

### Shared Bolt execution

Shared execution owns:

- the configured exposure policy for a registered strategy instance;
- provisional and sink-invoked entry claims;
- the one-use permit participating in the submit transaction;
- a client-order-ID index from active claims to registered exposure scope;
- retained exit-order authority needed for late fills or corrections;
- fail-closed health when NT truth is missing, conflicting, or unavailable;
- generic exposure admission outcomes and generic audit evidence.

### Strategy

A strategy owns only:

- signal calculations;
- strategy-local market/book observations;
- entry and exit intent;
- cooldown and decision policy;
- mapping generic execution outcomes into strategy decision evidence.

A strategy may retain a small signal context keyed by an NT position episode so
it can explain which market observation caused a decision. That context cannot
contain order status, position quantity, fill truth, an exit authority handle,
or any fact used to release shared execution authority.

## Module placement and dependency rule

Add the concrete implementation at:

`src/bolt_v3_order_execution/exposure_authority.rs`

The module may depend on:

- NT order and position identities and events;
- NT cache access supplied by the existing shared runtime;
- `bolt_v3_position_authority_feed`;
- shared submit admission and order execution;
- shared current-evidence handles;
- generic loaded strategy registration identities and configuration.

The module must not import anything under `crate::strategies`,
`bolt_v3_book_sizing::OutcomeBookState`, a market-family binding, an oracle
provider, or a concrete strategy archetype.

The interface is concrete. No `ExposureAuthorityAdapter` trait is introduced.

## Configuration and construction

Exposure policy becomes a required common strategy-envelope block rather than
an edge-taker parameter:

```toml
[exposure]
mode = "single_position"
max_retained_exit_authorities = 256
max_observations_per_retained_exit = 256
```

Strategies that intentionally permit concurrent exposure declare the same
interface with the other tagged form:

```toml
[exposure]
mode = "unrestricted"
```

The tagged enum rejects single-position limits on `unrestricted` and requires
both limits on `single_position`. There is no default and no archetype-name
switch selecting policy in Rust.

The existing edge-taker-local `[parameters.runtime.exposure_obligations]` block
is deleted from the schema and all shipped strategy files. Limits move with the
shared authority whose retained records they bound. This is a hard cutover;
both formats never coexist.

LiveNode constructs one `BoltV3ExposureAuthorityRuntime`, just as it constructs
one position-authority runtime. Strategy registration binds a capability using:

- NT `StrategyId` derived from `strategy_instance_id`;
- configured execution client;
- resolved execution account;
- configured OMS type; and
- the common exposure policy.

The runtime rejects duplicate scope registration or ambiguous account/client
binding. Every loaded strategy receives one non-optional capability. The
capability itself represents `Unrestricted` or `SinglePosition`; callers do not
choose between optional execution paths.

## Domain model

### Exposure scope

`BoltV3ExposureScopeKey` is private and contains the registered NT strategy,
execution-client, and account identities. `single_position` means at most one
exposure-producing order/position episode for that complete registered scope,
regardless of instrument. This preserves the edge taker's existing cross-market
one-position policy without naming that strategy in shared code.

The runtime mints `BoltV3ExposureAuthorityCapability`. Its fields are private,
and callers cannot forge a different strategy or execution binding.

### Occupancy is composed, not transitioned

There is no `ExposureState` enum.

For a single-position scope, an entry is blocked when any of these facts exists:

- a provisional submit claim;
- an unresolved sink-invoked entry claim;
- an NT order in the scope that can still create exposure;
- one or more canonical open NT positions in the scope;
- retained money-moving exit truth that cannot yet be reconciled; or
- unhealthy/missing/conflicting authority.

Occupancy is computed from these facts. A partial fill with a working remainder
is naturally occupied by both an open position and an order claim. A replacement
position is naturally occupied because canonical NT positions are non-empty.
There is no `ReplacementConflict` adoption transition and no state to overwrite.

Multiple canonical positions or an unavailable projection is a fail-closed
health result, not another lifecycle variant. The slot remains blocked until a
later authoritative reconciliation produces a coherent result.

### Entry claim

The runtime stores one small claim record per client order ID:

- registered exposure scope;
- client order ID and instrument ID;
- exact attempt generation;
- `Provisional` or `SinkInvoked` phase; and
- whether authoritative terminal zero-fill proof has retired it.

It does not store book state, position quantity, fill collections, lifecycle
projections, or a prior copy of another exposure state.

A concrete, opaque `BoltV3EntryExposurePermit` owns the claim during routing.
Shared execution—not the strategy—creates and consumes it.

### Exit authority record

Risk-reducing exit compilation already creates a shared
`BoltV3ExitOrderAuthorityHandle` backed by canonical position authority. The new
runtime retains that shared handle in an order-ID-keyed record until its late
fill/correction authority is final.

The record stores only shared execution data:

- scope and order identity;
- the existing exit authority handle;
- bounded authoritative observations needed by recovery; and
- retention/fail-closed health.

The current strategy-local `ReleasedExitProvenance`,
`DeferredExitObligation`, `HistoricalExitObservation`, and recursive saturation
wrapper are deleted. A limit breach marks the affected shared scope unhealthy
and blocks admission; it never wraps or copies another state.

## Shared interface

The exact Rust spelling may be refined during planning, but the interface has
only these responsibilities:

1. registration binds a capability to a loaded strategy/execution scope;
2. shared entry routing requests one claim for the final prepared order;
3. the claim participates in pre-sink, sink-invoked, and completion settlement;
4. shared risk-reducing exit routing registers its existing exit authority;
5. global NT order/position event subscriptions trigger reconciliation;
6. readiness and audit code reads generic health/occupancy snapshots.

There is no public `state()` method and no public mutation event enum. A
strategy cannot match execution phases. It receives only:

- an admitted/rejected submit outcome;
- a typed exposure rejection reason suitable for evidence; and
- an optional read-only canonical position view for producing an exit intent.

The canonical position view is constructed from NT authority at the time of the
request. It is not a mutable or retained strategy projection.

## Entry flow

1. The strategy computes an entry signal and creates ordinary order intent.
2. Shared execution prepares the final order and resolves its registered
   exposure capability.
3. After all fallible preparation that does not require a claim, but before any
   sink call, the runtime atomically checks composed occupancy and creates a
   provisional claim for the final client order ID.
4. A second concurrent request for the same scope is rejected because the
   provisional claim exists.
5. Failures before sink invocation drop the permit and remove the exact
   provisional generation.
6. Immediately before the NT sink, the permit becomes `SinkInvoked`.
7. Submitted or synchronously rejected results settle the same claim. If the
   sink outcome cannot prove absence, the claim stays blocked.
8. Global NT events trigger a cache/canonical-authority reconciliation. Exact
   terminal zero-fill truth retires the claim. A working order or open position
   keeps the scope occupied.

The exposure claim is one participant in the existing shared submit
transaction. Exposure, economics, capital admission, provisional registration,
and any other pre-sink authority are committed or aborted from the same route
outcome. Acquiring an exposure claim cannot leave a separately committed
capital/economics reservation, and failure in another participant cannot leave
the exposure claim behind. No participant invokes another while holding its
own lock.

This preserves callback-wins without storing a strategy state snapshot. An
event that already reconciled the exact generation makes a later permit drop or
completion idempotent.

## Exit flow

1. The strategy produces an exit signal against a fresh canonical position
   view.
2. Shared risk-reducing compilation seals the canonical position and quantity,
   as it does today.
3. Shared execution creates the existing exit-order authority and registers it
   in the shared exposure runtime before sink invocation.
4. NT order and position events update the shared authority record through the
   existing exit-authority methods.
5. Working, partial-fill, terminal, fill-void, and residual decisions use the
   shared authority and canonical position feed. The strategy does not maintain
   an `ExitPending` or recovery variant.
6. Once shared execution proves residual or flat truth, it reports the generic
   result. The strategy may update cooldown/signal context, but that update does
   not grant or release execution authority.

## Event and restart reconciliation

The repository already has verified patterns for global NT subscriptions:

- `subscribe_order_events` with `OrderEventAny`;
- `subscribe_position_events` with `PositionEvent`; and
- the raw position-status-report subscription used by
  `BoltV3PositionAuthorityRuntime`.

`BoltV3ExposureAuthorityRuntime` owns order- and position-event subscription
guards. Events are filtered by registered NT strategy and execution identities.
`OrderDenied`, which lacks account identity, is scoped by its NT strategy ID and
the runtime's registered client-order claim.

Events trigger reconciliation against NT cache and canonical position authority.
They do not directly declare a scope vacant from payload absence alone.

At restart, the runtime reconstructs scope occupancy from:

- registered strategies and exposure policy;
- live/open NT orders in cache;
- canonical NT positions;
- recovered shared exit-order authority; and
- current reconciliation health.

Every single-position scope starts blocked. It can become vacant only after NT
startup reconciliation is complete and one coherent reconstruction proves no
live exposure-producing order, no open position, and no unresolved retained
authority. This covers an order that reached the venue immediately before the
prior process stopped but had not yet appeared in local cache. Ambiguity,
incomplete startup reconciliation, or unavailable truth leaves the affected
scope blocked. There is no strategy-specific bootstrap/recovery state.

## Strategy cutover

The edge-taker strategy loses:

- `exposure.rs` and every `ExposureState` type;
- operation grants implemented by the strategy;
- direct handling of pending/working/terminal execution phases;
- canonical adoption, replacement conflict, blind recovery, sink-unknown, and
  saturation state transitions;
- released-exit and deferred-correction maps;
- raw order/position callback policy used to mutate exposure;
- direct `state()`, occupancy, pending-entry, exit-lifecycle, and recovery-hold
  projections.

The strategy keeps:

- construction of entry and exit signal intent;
- book and market lifecycle data needed for signal decisions;
- a small signal-only position episode association when needed for evidence or
  cooldown;
- mapping generic shared rejection/results to edge-taker evidence enums.

NT callbacks may still invoke ordinary strategy signal maintenance required by
NT's `Strategy` interface, but they do not forward lifecycle policy into a
strategy-owned authority. Shared runtime subscriptions receive execution truth
independently.

## Failure handling

- Missing or conflicting NT position truth blocks a single-position scope.
- A missing cached order after sink invocation does not release a claim by
  itself.
- Unknown order identity cannot mutate another scope.
- Duplicate events are idempotent by client order ID and attempt generation.
- Conflicting events preserve the prior authority and mark health unhealthy.
- Capacity-limit exhaustion marks the affected scope unhealthy and blocks new
  entries; it never evicts money-moving truth.
- A poisoned runtime lock or lost capability returns an error and blocks
  routing.
- No runtime-invalid state combination is guarded with `panic!`, `assert!`, or
  `unreachable!`.

## Invariants

The final implementation must make these statements true:

1. No production module under `src/strategies` implements NT order/position
   reconciliation or submit-attempt authority. Strategy-held quote/signal
   policy handles may call a shared module, but cannot become execution truth.
2. Shared exposure code imports no concrete strategy or market-family type.
3. There is one configured exposure-policy format and one runtime path.
4. NT cache and position authority are the only order/position truth.
5. A provisional claim is created before the sink and prevents a concurrent
   claim for the same single-position scope.
6. Pre-sink failure releases exactly its provisional generation.
7. Post-sink uncertainty remains occupied until authoritative reconciliation.
8. A partial fill plus working remainder remains occupied without a composite
   lifecycle state.
9. Late fill/correction authority is retained in shared execution, never in a
   strategy.
10. No recursive exposure state, copied prior state, or parallel retained-state
    reducer exists.
11. Strategies cannot obtain a mutable exposure projection or implement an
    execution-attempt participant.
12. Unrestricted and single-position policies use the same shared submit
    interface; config selects policy, not strategy source code.
13. A single-position scope is blocked from process construction until exact
    startup reconciliation proves it coherent and vacant or occupied.
14. Exposure, economics, capital, and registration participants settle from one
    shared route outcome; none forms an independently committed submit path.

## Migration sequence

Implementation will use a hard cutover, with behavior locked before deletion:

1. Inventory the existing behavior tests by invariant rather than by current
   `ExposureState` variant.
2. Add shared-module behavior tests for entry claims, exit authority,
   reconciliation, restart, scope isolation, and fail-closed health.
3. Add the common tagged exposure configuration and shared runtime construction.
4. Bind the non-optional capability through strategy registration and
   `StrategyBuildContext`.
5. Integrate the concrete entry permit and exit record with shared order
   execution.
6. Install global NT event subscriptions and prove teardown/restart does not
   duplicate handlers.
7. Switch edge-taker intent routing and signal reads to the shared interface.
8. Delete the complete strategy-local exposure module, callback lifecycle
   policy, old config, and tests that inspect private strategy state.
9. Migrate surviving behavior tests to the shared interface and retain strategy
   tests only for signal/intent/evidence mapping.
10. Run exact-head verification and request fresh review.

No compatibility adapter or feature flag keeps both authorities alive at the
final head.

## Behavior evidence

The shared module must prove at least:

- two concurrent entry attempts produce one permit and one occupied rejection;
- a preparation or pre-sink failure releases the exact claim;
- sink-invoked rejection/unknown remains occupied until NT proves terminal
  zero-fill truth;
- synchronous callback before route completion wins without stale rollback;
- a partial fill with a still-working entry remains occupied;
- an open position with no working entry remains occupied;
- position A closing while position B is canonical remains occupied by B with no
  adoption state;
- multiple positions or failed canonical projection block admission;
- an exit partial fill remains fenced until shared position authority proves the
  residual;
- fill void before and after terminal observation refines the retained shared
  exit record;
- unrelated order, strategy, account, and instrument events cannot change a
  scope;
- restart reconstructs occupied, vacant, and unhealthy scopes from NT truth;
- subscription teardown and restart leave one handler;
- retained-authority limit exhaustion is fail-closed and does not evict truth;
- unrestricted policy routes through the same interface without acquiring a
  single-position claim.

Tests verify behavior through the shared interface. No source-scanning or
structure test is added.

## Structural evidence and complexity budget

The replacement is incomplete unless the old structure is deleted. Before a
fresh review request:

- `src/strategies/binary_oracle_edge_taker/exposure.rs` does not exist;
- production code under `src/strategies/**` contains no `ExposureState`,
  `GovernedExposure`, `BoltV3RouteAttemptParticipant` implementation, or raw
  execution-lifecycle reducer;
- the shared exposure module has no recursive self-state payload and no
  state-by-event Cartesian reducer;
- the conditional census for production `src/strategies/**` is non-positive
  versus `623801311`;
- the full production-Rust `if`/`match` net versus `623801311` is below `+250`;
  and
- the exact census command/script, range, exclusions, and per-file results are
  attached to the review request.

The numeric budget is diagnostic, not proof of correctness. It prevents another
repair from claiming success after moving the same decisions between files.

Verification also includes formatting, workspace Clippy, isolated backtesting
Clippy, the focused behavior suites, root nextest, and exact-head remote
advisory evidence under the repository's verification rules.

## Review and scope disclosure

The stable PR body must disclose that the strategy-local takeover authority was
replaced by shared NT-backed exposure admission and reconciliation. The
exact-head review request must identify this design as superseding the two prior
exposure designs and must disclose the measured deletion.

The #869 maker event-fence remainder remains unchanged and separately tracked.
No merge is requested until local findings are resolved, the exact head is
pushed, the required native reviewer is requested, and fresh external review
adjudicates this replacement.

## Rejected alternatives

### Continue pruning local conditionals

Rejected. The strategy and its exposure module would continue making parallel
lifecycle decisions. Smaller helper methods would only redistribute the same
policy.

### Move `GovernedExposure` into a shared file

Rejected. Renaming strategy types and changing the path would preserve the
recursive state, Cartesian reducer, copied NT truth, and shallow interface.

### Normalize the current 13-state enum in place

Rejected as the final design. It could reduce repetition, but the strategy would
still own lifecycle and reconciliation that belong to NT/shared execution.

### Add a generic exposure adapter trait

Rejected. There is one concrete runtime and one NT integration. A trait would
increase the interface without a second adapter.

### Release claims from event payloads alone

Rejected. Event ordering and synchronous callbacks can expose incomplete or
stale views. Money-moving authority releases only after cache and canonical
position reconciliation.

### Roll back to mutable strategy exposure fields

Rejected. That would remove the new complexity but restore the original
overlapping-evaluation and out-of-band mutation defects. The replacement keeps
the safety properties at the correct shared seam.
