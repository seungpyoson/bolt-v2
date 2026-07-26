# Money Loop PR-A Authority Inversion Design

> **Superseded architecture (2026-07-24).** Do not implement this document.
> The selected boundary is: NautilusTrader owns order/fill/position lifecycle
> and reconciliation; Bolt consumes only post-reconciliation NT state plus
> provider-only collateral spendability. The active authority record is
> `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md`, delivered by PR #1505
> for #1354.

Part of #1179.

## Goal

PR-A makes Polymarket venue REST observations the continuous runtime authority for money state, but only after the observed venue delta is causally explainable by our recorded execution history. NT account and portfolio events remain advisory; they cannot create or override spendable collateral, open-order, or position beliefs.

## Authoritative Brief

Issue #1179 and its 2026-07-03 sequencing comment are authoritative for this lane. Session 2 is Lane 3 on branch `fix/1179-money-loop`, delivered as staged PR-A through PR-D. Every PR body must say `Part of #1179` and must not use closing keywords.

The settled PR-A decision is:

- Whole-node halt scope.
- Divergence means a venue observation unexplainable by recorded in-flight actions.
- Causal explainability, not numeric equality.
- No time-tolerance windows.

This PR-A v2 amendment is design-first. Implementation must not resume until the
owner approves this amended design on PR #1185.

## Incident Context

On 2026-07-02 the live bot admitted USD 1,045 cumulative orders against a USD 50 account and submitted 40 sell orders for unheld shares. Admission records also lacked account, PnL, and equity evidence. The failure mode is structural: money beliefs have no single owner anchored to venue reality. Runtime currently binds to NT AccountState pushes that the Polymarket adapter does not emit, while REST truth sources run only before startup and are discarded.

## Approved Scope

Whole-node halt is approved for unexplainable venue truth. If a venue snapshot changes collateral, open orders, or positions in a way that cannot be explained by the recorded order-event stream or already-booked settlement evidence, runtime must latch submit admission into a non-armed kill-switch state and report the unexplainable observation loudly. PR-A does not add strategy-side gates, size caps, thresholds, tolerance bands, time windows, or fallback money paths.

The whole-node halt covers every new submission class, including
risk-reducing exits. A venue-truth divergence means the node no longer trusts the
beliefs that size exits; allowing exits through the latch repeats the incident
class.

## Source Of Truth

- Venue REST observations are authoritative for balance, allowance, open orders, and positions only after causal reconciliation succeeds.
- The causal ledger has one source: the already-captured NT order-event stream. Accepted events provide `client_order_id` and `venue_order_id`; Filled events provide executed quantities. The reconciler consumes the same recorded events the evidence/capture system writes and must not create a parallel durable bookkeeping store.
- The reconciler may use completed venue REST captures as causal watermarks.
  For venue capture N, `fence(N) := completion of capture N+1`: a
  venue-issued, monotonic, parameter-free happens-after anchor. These
  watermarks prove how far the shared event/snapshot processing context has
  advanced; they are not a second ledger and they do not explain money movement
  by themselves.
- Named invariant: the per-account user event channel is ordered. Violations are
  detectable through recorded event evidence timestamps and are themselves
  divergence.
- NT account states and portfolio snapshots are advisory and do not determine money authority for Polymarket live execution.
- Existing pre-run REST fetchers in `src/bolt_v3_providers/polymarket/venue_account_state_source.rs` and `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs` prove the venue reads are available but are insufficient because they do not update during runtime.
- Pinned NT adapter source is under `~/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6be5a50/crates/adapters/polymarket/`. PR-A should use the adapter HTTP clients where they support the required query shape.

If capture completeness is imperfect because terminal event flushing is a known Lane 5 defect, the PR body must call that dependency out plainly. PR-A may depend on the captured event stream; it must not solve the Lane 5 shutdown drain.

## Venue Snapshot

Add a Polymarket venue-truth module with a normalized snapshot:

```rust
pub struct PolymarketVenueTruthSnapshot {
    pub captured_at: UnixNanos,
    pub account_id: AccountId,
    pub collateral_balance: Money,
    pub collateral_allowance: Money,
    pub open_orders: BTreeMap<VenueOrderId, PolymarketVenueTruthOpenOrder>,
    pub positions_by_token_id: BTreeMap<String, Decimal>,
}
```

The snapshot is converted from:

- CLOB `/balance-allowance` for collateral balance and allowance.
- CLOB `/data/orders` for open orders.
- Data API `/positions` with `redeemable=false`, `sizeThreshold=0`, `sortBy=TOKENS`, and `sortDirection=DESC` for live non-redeemable positions.

## Causal Reconciliation

The reconciler compares the previous accepted venue snapshot to the next venue snapshot and asks whether every observed delta is explained by known events:

- New venue open order: explained by an Accepted event that maps the venue order id to a known submitted client order id.
- Open order removal or reduced open quantity: explained by terminal order events, Filled events, or both, for the mapped client order id.
- Position increase or decrease: explained by Filled events for mapped orders on the corresponding token id.
- Collateral decrease or release: explained by recorded Filled events at the
  actual fill price and fill quantity, plus an explicit fee term carried in the
  event evidence, and by terminal release or booked settlement evidence. Open
  order limit price is not sufficient collateral evidence for a fill.
- Settlement-driven position or cash deltas: explained only after PR-D books settlement evidence.

The reconciler must not decide divergence from instantaneous inequality between venue and NT beliefs. Normal in-flight fills can make cached beliefs and venue observations differ. That is not divergence when the delta is causally explained by recorded actions.

## Capture-Fenced Pending Deltas

Venue REST can expose a fill before the local NT event projection has consumed
the corresponding Filled event, and the balance, open-order, and position REST
reads are not an atomic venue cut. PR-A v2 therefore uses defer-until-explained
semantics with completed venue captures, not a time window.

The reconciler consumes recorded user events and completed venue snapshot
results in one ordered processing context. That single context is what makes
"the projection advanced since capture N" well-defined without a separate
bookkeeping store or time approximation.

For every venue snapshot:

1. Capture the venue REST snapshot and assign it the next per-account capture
   number N when all REST reads for that snapshot have completed.
2. Define `fence(N)` as completion of capture N+1 for the same account. This is
   a venue-issued, monotonic, parameter-free happens-after anchor.
3. Reconcile against the order-event projection consumed so far.
4. If every delta is explainable, accept and promote the snapshot.
5. If any delta is unexplained and capture N+1 has not completed yet, hold the
   snapshot as pending. Pending snapshots are not promoted and do not halt the
   node yet. Admission continues on the last accepted snapshot while the
   pending snapshot waits for its fence.
6. When capture N+1 completes, re-run reconciliation for the pending capture N
   after consuming the recorded user events that arrived in the same ordered
   processing context between captures N and N+1. If the delta is then
   explained, accept and promote it. If it is still unexplained, durable-halt
   the node.

This is causal, parameter-free, and window-free. No elapsed-time tolerance,
poll-count tolerance, timestamp comparison, or numeric equality gate may
substitute for the completed-capture fence.

Still-unexplained deltas at fence completion halt for one of three
alarm-classified branches:

- True divergence: venue state changed without any recorded order, fill,
  terminal, or booked settlement cause.
- Ordering violation: recorded per-account user-event evidence contradicts the
  ordered-channel invariant.
- Silent channel while venue state moved: venue state changed across captures,
  but no corresponding per-account user event appeared before `fence(N)`.

A later venue snapshot that returns to the prior value does not erase a pending
unexplained delta. The original pending delta must become explained by recorded
events or become a durable divergence halt once its fence is reached.

## Unexplainable Deltas

Unexplainable deltas are structural divergence:

- Venue open order with no known accepted client order.
- Venue fill or position change with no recorded Filled event or booked settlement.
- Collateral movement with no recorded order/fill/terminal/settlement cause.
- Manual operator deposit or withdrawal while the node is running.

Manual transfers intentionally halt the node. Operators must transfer while the node is stopped, or accept halt-and-reconcile behavior. PR-A must not add a manual-transfer exemption mechanism.

On an unexplainable delta, runtime records a `VenueTruthDivergence` kill-switch trigger and replaces submit admission kill-switch state with a non-armed state. The alarm includes account id, field, venue value, prior accepted venue value, and the missing causal explanation.

The divergence evidence is written through the fail-closed decision-evidence
writer. A log-only alarm is insufficient. If divergence evidence or kill-switch
state cannot be persisted, the node remains fail-closed and must not promote the
snapshot.

## Durable Halt And Baseline Semantics

Venue-truth divergence halts are durable. Runtime writes the halt through
`KillSwitchStore` and transitions from the recovered current kill-switch state,
not from a hardcoded assumed `Armed` state. If the store already contains a
more-restrictive or later halt state, PR-A v2 must not downgrade or overwrite it
with a weaker in-memory replacement.

Restart must not launder a divergence. On startup:

- Load recovered kill-switch state before any live submit path can become ready.
- If recovered state is non-armed because of venue-truth divergence, submit
  admission stays non-armed until the existing operator recovery path clears it.
- The first post-start venue snapshot is not blindly accepted for a dirty
  baseline. Dirty means recovered reservations, recovered open orders, recovered
  positions, recovered pending orders, or a recovered non-armed kill-switch
  state.
- A clean baseline may be accepted only when recovered submit state has no
  outstanding money exposure and the kill-switch store is armed.
- A dirty baseline must be reconciled against recovered reservations,
  positions, open orders, and the durable order-event stream. If that evidence
  cannot explain the baseline, the node requires explicit operator
  reconciliation while stopped; it must not convert the dirty first snapshot
  into an accepted venue-truth baseline during live boot.

The baseline rule is deliberately not a time gate. It is a recovered-state and
evidence rule.

## Runtime Authority

After reconciliation succeeds, runtime promotes the accepted venue snapshot into capital admission:

- Venue balance and allowance own spendability.
- Venue open orders own open-order lifecycle state.
- Venue positions own prediction-market inventory state.
- NT events remain useful as the causal event stream and advisory latency hints, but not as money authority.

For Polymarket, accepted venue truth alone satisfies money readiness for capital
admission. NT `AccountState` absence must not keep admission unready, and no
Polymarket readiness path may `min` venue spendability with an NT account value.
If an NT `AccountState` arrives, it is diagnostic/advisory only and cannot
increase, decrease, or veto the accepted venue-truth spendability, open-order,
or position state.

The live node owns the poller lifecycle. The poll cadence is configured in TOML under the Polymarket execution config; it is not hardcoded. Missing or zero cadence is invalid whenever capital admission is enforced for Polymarket live execution.

Per owner decision in PR #1185 comment 4874494874, a venue REST capture
failure is degraded authority, not divergence. Capture failure must not write a
durable halt and must not enter the venue-truth divergence path. Runtime records
loud degraded-authority evidence containing source, endpoint, error class, and
consecutive captures missed; suspends all admission classes, including
risk-reducing exits, until the next successful reconciled capture; and then
automatically resumes without operator recovery. Durable halt remains reserved
for still-unexplained deltas at the completed capture fence and for failure to
persist required divergence evidence.

## Out Of Scope

- PR-B governance mode.
- PR-C exit quantity clamp.
- PR-D settlement booking and offline assertion.
- Strategy-local submit mechanics or strategy-local money gates.
- Runtime constants for money, size, poll cadence, or timing.
- Lane 5 evidence writer shutdown drain.

PR-D acceptance must include a hold-to-resolution replay test proving a resolution payout is booked and does not trigger a false venue-truth halt.

## Evidence

- Unit tests for token-id extraction and snapshot conversion.
- Unit tests for event-derived causal reconciliation: accepted-order mapping, fills, terminal events, and unexplainable deltas.
- Runtime feed tests proving only explainable venue truth is promoted into capital admission.
- Runtime feed tests proving unexplainable venue deltas latch whole-node submit admission halt.
- Runtime feed tests proving capture-fenced pending deltas do not halt before
  capture N+1 completes, continue trading on the last accepted snapshot while
  pending, then accept if explained or durable-halt if still unexplained at the
  fence.
- Runtime/poller tests proving venue REST capture failure records degraded
  authority evidence, suspends all submission classes, automatically resumes
  after the next accepted venue-truth capture, and does not write a durable
  halt.
- Startup tests proving a venue-truth halt survives restart and first-snapshot
  dirty baselines cannot be blindly accepted.
- Admission readiness tests proving accepted Polymarket venue truth is sufficient
  when NT `AccountState` is absent and that NT account values remain advisory.
- Config tests proving missing or zero venue-truth poll cadence fails closed when Polymarket capital admission is enforced.
- Allowed static checks and exact-head remote CI before completion.
