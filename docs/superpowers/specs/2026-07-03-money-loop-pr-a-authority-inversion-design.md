# Money Loop PR-A Authority Inversion Design

## Goal

PR-A makes venue REST truth the continuous runtime authority for Polymarket money state. NT account and portfolio events remain fast advisory inputs, but they cannot create or override spendable collateral, open-order, or position beliefs after a venue truth snapshot is present.

## Incident Context

On 2026-07-02 the live bot admitted USD 1,045 cumulative orders against a USD 50 account and submitted 40 sell orders for unheld shares. Admission records also lacked account, PnL, and equity evidence. The failure mode is structural: money beliefs have no single owner anchored to venue reality. Runtime currently binds to NT AccountState pushes that the Polymarket adapter does not emit, while REST truth sources run only before startup and are discarded.

## Approved Scope

Whole-node halt is approved for venue truth divergence. If runtime venue truth conflicts with NT advisory state for money-critical facts, the node must latch submit admission into a non-armed kill-switch state and report the divergence loudly. PR-A does not add strategy-side gates, size caps, thresholds, or fallback money paths.

## Source Of Truth

- Venue REST truth is authoritative for available collateral, conditional spendability, open orders, and positions.
- NT events are advisory and useful for latency, but never authoritative for money once venue truth has started.
- Existing pre-run REST fetchers in `src/bolt_v3_providers/polymarket/venue_account_state_source.rs` and `src/bolt_v3_providers/polymarket/collateral_accounting_source.rs` prove the source is available but are not enough because they do not update during runtime.
- Pinned NT adapter source is under `~/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6be5a50/crates/adapters/polymarket/`. PR-A should use or extend the adapter HTTP clients where they support the required query shape.

## Design

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

The runtime feed accepts venue-truth snapshots as the owner of capital-admission state. NT account states and cache snapshots may still be recorded as advisory evidence, but after a venue snapshot has been observed they cannot increase spendability, invent positions, or mark reconciliation green.

The live node owns the poller lifecycle. The poll cadence is configured in TOML under the Polymarket execution config; it is not hardcoded. Missing or zero cadence is invalid whenever capital admission is enforced for Polymarket live execution.

## Divergence

Divergence is structural, not threshold-based:

- NT advisory free collateral disagrees with venue collateral.
- NT advisory position quantity for a configured binary instrument disagrees with venue position quantity keyed by token id.
- NT advisory open-order terminal/live belief conflicts with the venue open-order set.

On divergence, runtime records a `VenueTruthDivergence` kill-switch trigger and replaces submit admission kill-switch state with a non-armed state. The report includes account id, instrument id when applicable, venue field, venue value, advisory source, and advisory value.

## Out Of Scope

- PR-B governance mode.
- PR-C exit quantity clamp.
- PR-D settlement booking and offline assertion.
- Strategy-local submit mechanics or strategy-local money gates.
- Runtime constants for money, size, poll cadence, or timing.

## Evidence

- Unit tests for token-id extraction and snapshot conversion.
- Runtime feed tests proving venue truth replaces NT account/cache money authority.
- Runtime feed tests proving NT advisory conflicts after venue truth latch whole-node submit admission halt.
- Config tests proving missing or zero venue-truth poll cadence fails closed when Polymarket capital admission is enforced.
- Allowed static checks and exact-head remote CI before completion.
