# Money Loop PR-A Authority Inversion Design

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

## Incident Context

On 2026-07-02 the live bot admitted USD 1,045 cumulative orders against a USD 50 account and submitted 40 sell orders for unheld shares. Admission records also lacked account, PnL, and equity evidence. The failure mode is structural: money beliefs have no single owner anchored to venue reality. Runtime currently binds to NT AccountState pushes that the Polymarket adapter does not emit, while REST truth sources run only before startup and are discarded.

## Approved Scope

Whole-node halt is approved for unexplainable venue truth. If a venue snapshot changes collateral, open orders, or positions in a way that cannot be explained by the recorded order-event stream or already-booked settlement evidence, runtime must latch submit admission into a non-armed kill-switch state and report the unexplainable observation loudly. PR-A does not add strategy-side gates, size caps, thresholds, tolerance bands, time windows, or fallback money paths.

## Source Of Truth

- Venue REST observations are authoritative for balance, allowance, open orders, and positions only after causal reconciliation succeeds.
- The causal ledger has one source: the already-captured NT order-event stream. Accepted events provide `client_order_id` and `venue_order_id`; Filled events provide executed quantities. The reconciler consumes the same recorded events the evidence/capture system writes and must not create a parallel durable bookkeeping store.
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
- Collateral decrease or release: explained by the liability/cash effects of mapped accepted, filled, terminal, or booked settlement evidence.
- Settlement-driven position or cash deltas: explained only after PR-D books settlement evidence.

The reconciler must not decide divergence from instantaneous inequality between venue and NT beliefs. Normal in-flight fills can make cached beliefs and venue observations differ. That is not divergence when the delta is causally explained by recorded actions.

## Unexplainable Deltas

Unexplainable deltas are structural divergence:

- Venue open order with no known accepted client order.
- Venue fill or position change with no recorded Filled event or booked settlement.
- Collateral movement with no recorded order/fill/terminal/settlement cause.
- Manual operator deposit or withdrawal while the node is running.

Manual transfers intentionally halt the node. Operators must transfer while the node is stopped, or accept halt-and-reconcile behavior. PR-A must not add a manual-transfer exemption mechanism.

On an unexplainable delta, runtime records a `VenueTruthDivergence` kill-switch trigger and replaces submit admission kill-switch state with a non-armed state. The alarm includes account id, field, venue value, prior accepted venue value, and the missing causal explanation.

## Runtime Authority

After reconciliation succeeds, runtime promotes the accepted venue snapshot into capital admission:

- Venue balance and allowance own spendability.
- Venue open orders own open-order lifecycle state.
- Venue positions own prediction-market inventory state.
- NT events remain useful as the causal event stream and advisory latency hints, but not as money authority.

The live node owns the poller lifecycle. The poll cadence is configured in TOML under the Polymarket execution config; it is not hardcoded. Missing or zero cadence is invalid whenever capital admission is enforced for Polymarket live execution.

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
- Config tests proving missing or zero venue-truth poll cadence fails closed when Polymarket capital admission is enforced.
- Allowed static checks and exact-head remote CI before completion.
