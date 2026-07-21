# Issue #789 Event-Store Evidence Plan

## Decision

Restore trustworthy lifecycle evidence for the existing #789 backtest without adding a Bolt event
ledger or changing NautilusTrader execution behavior.

## Verified boundary

The pinned NautilusTrader revision already provides an append-only event store over a bus tap that
runs before publish and endpoint dispatch. Its default encoders cover trading commands, order
events, position events, execution reports, and account states. Its data-marker sidecar provides
event-sequence-bound catalog cursors for order-book data; the already hash-verified catalog remains
the source of the actual book records. Before any cursor is consumed, NT's `MarkerVerifier` checks
the sealed sidecar's record hashes, counts, sequence coverage, dictionary, and monotonicity.

`BacktestNode` does not expose the kernel event-store factory at this revision. The thinnest usable
integration is therefore a BTE-owned run wrapper that opens the existing NT event-store lifecycle
against the built engine clock immediately before `BacktestNode::run`, seals it immediately after
the run, and reads the sealed evidence. It must not implement its own capture protocol.

NT emits repeated AccountState snapshots across its bus paths. The validator first binds them to
each fill's ordered causal interval: fill, then position mutation, then account state, then the next
fill or terminal close receipt. It may collapse identical republishes only inside one such interval;
it still requires an event identity not seen in earlier intervals for a zero-cash-delta fill. It
independently derives the cash trajectory from fills and commissions and compares every transition
plus the current terminal account cache, including the CashAccount's transient per-instrument lock
map. It never synthesizes `CashPosted` evidence.

Two pinned ordering details are explicit. Raw `OrderFilled` evidence is captured before the
execution engine assigns `position_id`, so position identity comes from the subsequent Position
event and terminal fill comparisons ignore only that one post-capture field. Also, BacktestEngine
routes `InstrumentClose` to the exchange before publishing it through DataEngine, so the settlement
fill precedes the matching close receipt in the store. Position effect classifies settlement; the
unique close receipt and configured price bind its cause.

For the #789 fixture, venue latency and fill models are absent. The backtest applies each market-data
record, processes strategy callbacks, and drains the venue command queue before the next record.
The evidence probe must nevertheless prove that the data-marker cursor at every captured normal
`SubmitOrder` reproduces the book consumed by the subsequent fills. It folds snapshots before the
submit sequence and accepts at that sequence only the first synchronous snapshot carrying the
entry's exact capture time. Time-based safety flushing is disabled for this diagnostic run, so a
post-submit update cannot create a competing snapshot before the next captured entry. High-fidelity
per-record capture is deliberately not used: it creates an unbounded write lane proportional to
the full market-data corpus and can overflow without improving this synchronous boundary.

## Scope

This slice covers the existing #789 fixture only:

- one Polymarket binary instrument and one single-currency cash account;
- NETTING OMS;
- immediate market-taker entry and normal reduction;
- L2 MBP with liquidity consumption;
- no fill or latency model;
- terminal binary settlement;
- zero taker fees derived from the instrument, with explicit commission evidence on every fill and
  an exact terminal commission map by currency;
- no reversal or reopened position in this fixture.

It does not add maker/resting-order, margin, FX-conversion, compatibility, #788, artifact-store, or
general audit-framework behavior.

## Evidence model

Observed NT evidence:

- ordered `SubmitOrder` commands preserving submitted quantity and instrument identity;
- ordered `OrderFilled` events preserving order/trade identity, price, quantity, side, and
  commission;
- ordered position events binding position identity, exact exposure effects, and cumulative P&L;
- the initial `AccountState` and at least one causally ordered `AccountState` after every fill,
  including fills whose balance map is unchanged;
- integrity-verified order-book cursors bound to the synchronous SubmitOrder entry and the
  hash-verified catalog;
- `InstrumentClose`, registered through NT's existing encoder-registry extension;
- terminal orders and positions plus the account object's current balances, CashAccount transient
  per-instrument locks, and applicable margins, used only as completeness cross-checks rather than
  reconstructed from its last event;

Derived by the independent validator:

- the executable book at each normal submit cursor;
- instrument-normalized base quantity;
- entry, reduction, closure, reversal, and settlement effects from signed exposure transitions;
- remaining quantity;
- commission totals by currency;
- realized P&L and terminal cash by currency.

The validator must not call `OrderBook::simulate_fills`, `Position::apply`, or use terminal caches as
the lifecycle source of truth.

## Implementation sequence and evidence

1. Add pinned `nautilus-event-store` and MessagePack decoding as test-only BTE dependencies.
   Evidence: dependency provenance test and a focused compile.
2. Add a RED probe around the real #789 run. It requires contiguous event-store entries, lossless
   cursor markers, ordered submit/fill/position/account evidence, settlement evidence, and exact
   terminal-cache availability. Evidence: the probe fails before the wrapper exists.
3. Add the minimal run-scoped NT event-store wrapper. Capture `InstrumentClose` through the existing
   encoder-registry extension and use NT data markers for book deltas. Evidence: the RED probe turns
   green and a missing marker/entry negative control fails loudly.
4. Replace #789 post-run evidence assembly with a pure ordered fold. Port only the valid arithmetic
   and behavioral cases from the closed #1489 branch. Evidence: focused unit tests for duplicate
   identities, instrument and book identity, raw precision/increment, strict fill -> position ->
   account ordering within each causal interval, exact settlement remainder, complete commission
   maps, realized P&L, zero-cash-delta fills, and per-fill cash.
5. Bind each normal fill to the catalog rows selected by its submit cursor and independently sweep
   price levels. Enforce exactly one entry and one normal reduction, on opposing book sides, so the
   declared #789 slice cannot reuse earlier consumed liquidity without adding a second matching
   model. Evidence: multiple-reduction, later same-boundary, ambiguous-boundary, missing-cursor,
   tampered-hash, and book-drift negative controls.
6. Validate the real entry -> normal exit -> residual settlement run and compare terminal NT
   projections only after the fold succeeds. Evidence: focused #789 test and result artifact.
7. Run formatting, Clippy, focused tests, the complete BTE test battery, and internal adversarial
   review. Resolve every substantive finding before publication.
8. Commit and push normally, open one #789-only PR, request `sp-reviewer`, report the exact head SHA,
   and detach without waiting for CI.

## Stop conditions

Stop rather than add a fallback if any of these cannot be proven:

- event-store sequence does not bind the matching-time book for this zero-latency fixture;
- book-marker loss cannot be detected;
- settlement cannot be structurally distinguished from a normal submitted order;
- initial balances, all fills, Position effects, account transitions, commissions, or terminal
  balances are incomplete;
- validation requires deriving authority from `Position.events` or terminal order caches.
