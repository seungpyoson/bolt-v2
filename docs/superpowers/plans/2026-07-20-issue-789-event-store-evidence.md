# Issue #789 Event-Store Evidence Completion Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the exact #789 entry -> reduction -> settlement lifecycle through a closed,
causal evidence contract that cannot silently omit a captured claim.

**Architecture:** Validate Bolt-owned static inputs before NT executes, verify NT's sealed logical
event stream, assemble one explicitly closed lifecycle grammar, fold quantities and economics from
independent authorities, then use terminal state only as a cross-check. The implementation retains
NT's event store and marker registry; it does not add a Bolt ledger or second matcher.

**Tech stack:** Rust 1.97, NautilusTrader at
`d81be0bcc7a473c45d2dc8a8885638336073a218`, `nautilus-event-store`, MessagePack, Nextest.

**Field-level proof boundary:**
`docs/superpowers/specs/2026-07-21-issue-789-economic-lifecycle-proof-boundary-design.md` is the
authoritative inclusion, exclusion, identity, canonical-fill, and review-stop contract.

## Global constraints

- One Polymarket binary instrument and one single-currency CASH/NETTING account.
- Exactly one quote-denominated market entry, one base-denominated market reduction leaving a
  residual, and one synthetic settlement closing that residual.
- L2 MBP with liquidity consumption, market-order acknowledgements disabled, zero latency, no fill
  model, and no fee model.
- Classification comes from signed position effect, never timestamps or `reduce_only`.
- No Bolt ledger, second matcher, terminal-cache authority, compatibility path, #788 work, or #1447
  reversal.
- Every admitted claim is bound or explicitly waived; every other captured payload fails closed.
- Tests verify behavior, never source structure.

---

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

The NT adapter deliberately records each logical UUID once even when the same message crosses more
than one tap-visible delivery boundary. #789 validates uniqueness in that persisted logical stream;
it does not claim to audit raw bus-delivery multiplicity. Bolt must not disable the adapter's
normalization or predict NT's internal hop topology. A repeated dispatch that creates another
economic effect still fails through the recorded fill/effect/order counts, duplicate trade guard,
or NT's own fail-stop path; an identical projection republish is not a second lifecycle mutation.

`BacktestNode` does not expose the kernel event-store factory at this revision. The thinnest usable
integration is therefore a BTE-owned run wrapper that opens the existing NT event-store lifecycle
against the built engine clock immediately before `BacktestNode::run`, seals it immediately after
the run, and reads the sealed evidence. It must not implement its own capture protocol.

NT emits repeated AccountState snapshots across its bus paths. The validator first binds them to
each fill's ordered causal interval: fill, then position mutation, then account state, then the next
fill or terminal close receipt. It may collapse identical republishes only inside one such interval;
every stored account event must have a globally fresh identity, including a zero-cash-delta fill. It
independently derives the cash trajectory from fills and commissions and compares every transition
plus the current terminal account cache, including the CashAccount's transient per-instrument lock
map. Every lifecycle account base currency is also bound to the mapped manifest venue config. It
never synthesizes `CashPosted` evidence.

Two pinned ordering details are explicit. Raw `OrderFilled` evidence is captured before the
execution engine assigns `position_id`, so position identity comes from the subsequent Position
event and terminal fill comparisons ignore only that one post-capture field. Also, BacktestEngine
routes `InstrumentClose` to the exchange before publishing it through DataEngine, so the settlement
fill precedes the matching close receipt in the store. Position effect classifies settlement; its
cause additionally requires NT's synthetic expiration `OrderInitialized -> OrderAccepted -> fill`
shape, expiration ID/tag, reduce-only remainder, absence of `SubmitOrder` and `OrderSubmitted`, and
the unique configured `ContractExpired` receipt. An orphan normal fill cannot become settlement.

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

- ordered `SubmitOrder` commands preserving and cross-binding envelope and embedded trader,
  strategy, instrument, client-order, side, type, quantity, quote-denomination, post-only, and
  reconciliation semantics;
- the causally intervening `OrderUpdated` for a quote-denominated submission, bound to the same
  submitted identities and checked against independently normalized base quantity before the first
  fill;
- ordered `OrderFilled` events preserving order/trade identity, price, quantity, side, and
  commission;
- unique command/event identities across persisted logical submits, order events, fills, position
  events, and account events;
- ordered position events binding position identity, exact exposure effects, and cumulative P&L;
- the initial `AccountState` and at least one causally ordered `AccountState` after every fill,
  including fills whose balance map is unchanged;
- integrity-verified order-book cursors bound to the synchronous SubmitOrder entry and the
  hash-verified catalog;
- `InstrumentClose`, registered through NT's existing encoder-registry extension;
- synthetic expiration initialization and acceptance events binding the settlement origin;
- terminal orders (including initialization, effective quantity, quote flag, side, type, status,
  filled quantity, and fills), positions, plus the account object's current balances, CashAccount
  transient per-instrument locks, and applicable margins, used only as completeness cross-checks
  rather than reconstructed from their last events;

Derived by the independent validator:

- the executable book at each normal submit cursor;
- instrument-normalized base quantity;
- entry, reduction, closure, reversal, and settlement effects from signed exposure transitions;
- remaining quantity;
- commission totals by currency;
- realized P&L and terminal cash by currency.

The validator must not call `OrderBook::simulate_fills`, `Position::apply`, or use terminal caches as
the lifecycle source of truth.

## Frozen completion contract

### Validation phases

1. **Static admission:** Before `BacktestNode::run`, validate the resolved #789 run shape, selected
   BinaryOption constraints, settlement payoff, and every catalog book row that NT may execute.
   This is domain validation only: it does not choose fills or emulate matching. Tradable book
   prices use the instrument's precision, increment, and optional min/max; settlement remains the
   separately configured binary payoff and may legitimately be `0` or `1`.
2. **Integrity:** Open NT's event store and markers before the run, then seal and verify both before
   decoding any payload.
3. **Closed causal assembly:** Admit only the payload grammar below. Every admitted payload must be
   assigned to exactly one lifecycle record; an unknown, orphaned, duplicate, or unexplained entry
   is a typed failure.
4. **Independent fold:** Reconstruct each submit-cursor book, normalize quote quantity, require a
   fully satisfied minimal sweep, and fold exposure, commissions, realized P&L, and cash.
5. **Terminal cross-check:** Compare the successful fold with the exact proof-relevant live order,
   position, account, and result fields enumerated by the field-level boundary. Explicitly excluded
   bookkeeping is not claimed. Terminal state never supplies lifecycle authority.

No `catch_unwind` is part of this contract. The known malformed-input path is prevented by static
admission. Panic containment would require a separate, proven unwind-safe NT lifecycle contract and
is not silently introduced here.

### Closed lifecycle grammar

- Quote entry prelude: `OrderInitialized -> SubmitOrder -> OrderSubmitted -> OrderUpdated`.
- Base reduction prelude: `OrderInitialized -> SubmitOrder -> OrderSubmitted` and no `OrderUpdated`.
- Each normal prelude is followed by `(OrderFilled -> PositionEffect -> AccountState)+` in strict
  store-sequence order. Role is bound to denomination: entry is quote; reduction is base.
- Settlement: synthetic `OrderInitialized -> OrderAccepted -> OrderFilled -> PositionClosed ->
  AccountState -> InstrumentClose` receipt, with exactly one fill and no `SubmitOrder`,
  `OrderSubmitted`, or `OrderUpdated`.
- Timestamps are diagnostic only.
- `RunStarted` and `RunEnded` are integrity envelopes. `SubscribeCommand`, `UnsubscribeCommand`,
  and `TimeEvent` are explicit control-plane waivers: their bytes remain sealed and verified, but
  they carry no order, fill, position, or account claim.
- Economic payload types are `SubmitOrder`, `OrderInitialized`, `OrderSubmitted`, `OrderAccepted`,
  `OrderUpdated`, `OrderFilled`, `InstrumentClose`, `AccountState`, `PositionOpened`,
  `PositionChanged`, and `PositionClosed`. `PositionAdjusted` and every other payload type fail
  closed for this fixture.

The execution account created from the primary venue configuration is captured from the built
execution-client registry before the run. Each normal `OrderSubmitted.account_id` must equal that independent
authority and both submissions must agree. The configured identity then binds every update, fill,
position effect, lifecycle AccountState, settlement event, and terminal projection. No downstream
engine event is its own routing authority.

### Closed evidence matrix

| Stage | Independent authority | NT claim / projection | Required invariant | Phase | Required behavior control |
|---|---|---|---|---|---|
| Run shape | Hash-bound manifest, resolved config, and pre-run execution-client registry | Built run configuration | BinaryOption; one lifecycle instrument and configured execution account; CASH/NETTING/L2; liquidity consumption; market acknowledgements disabled; supported deterministic models | Admission | Wrong account/OMS/book/model/acknowledgement setting rejected before run |
| Book domain | Catalog rows plus instrument | None | Add/Update/Delete require Buy/Sell; prices positive, exact precision/increment, within optional bounds; Add/Update sizes positive and aligned; Clear is structural and may use NoOrderSide | Admission | Wrong action/side, zero, out-of-range, wrong precision/increment price; invalid size; valid Clear/Delete |
| Settlement input | Manifest close plus the two projected BinaryOptions | `InstrumentClose` | Exact unique projected leg set; complementary `0`/`1` payoffs; quantity-independent from trading min/max | Admission/assembly | Wrong/missing/duplicate leg, fractional/non-complementary payoff; valid zero/one pair |
| Store integrity | Sealed store and marker sidecar | Entries, dictionaries, cursors | Hashes/counts/sequence/dictionaries valid; no gaps; one unambiguous submit cursor | Integrity | Tampered/gapped/missing/ambiguous evidence |
| Event surface | Frozen grammar | Every captured payload | Each type admitted and assigned, or typed rejection; no silent wildcard | Assembly | Unknown payload and orphan admitted payload |
| Normal submission | `SubmitOrder` intent plus pre-run configured execution account | `OrderInitialized`, `OrderSubmitted` | Envelope equals embedded init; exact identity and causal order; exactly three order-identity classes; both normal submissions equal the configured account | Assembly | Missing/duplicate/reordered/identity/account drift, including correlated downstream drift and cross-order identity reuse |
| Quote conversion | Submitted quote quantity plus cursor book and instrument | `OrderUpdated` | Entry is quote with exactly one update; reduction is base with no update; independently normalized quantity; submitted account and pinned shape | Fold | Swapped denomination; missing/duplicate/orphan/reordered and every field mutation |
| Normal fills | Cursor-bound book and submitted semantics | `OrderFilled+` | Exact ordered canonical-fill projection; sweep remainder zero; sum fills equals effective quantity; unique event/trade/venue identities | Fold | Insufficient depth, missing/extra/reordered/drifted/duplicate fill or identity |
| Position effects | Prior folded exposure and fills | `PositionOpened/Changed/Closed` | One causal effect per fill; ownership/order links, kind and signed exposure derived; one position/account; no reversal/reopen | Fold | Missing/reordered/replayed/wrong kind, quantity, ownership, order link, identity, or account |
| Account effects | Starting cash, fills, multiplier, commissions | `AccountState` | Initial state matches config; one fresh causal state after each fill including zero delta; cash/currency/account exact | Fold | Missing/replayed/conflicting/trailing/wrong account or cash |
| Settlement | Manifest payoff plus remaining exposure | Synthetic expiration events and close receipt | Exact pinned initialization/acceptance tuple; no normal submission path; anchored account; exactly one fill closes the exact remainder; final exposure zero | Assembly/fold | Orphan masquerade, multiple fill, and every witness-shape/order/account/price/quantity mutation |
| Terminal order | Completed causal order record | Live order cache | Exact field-registry identities/routing/canonical fills/trade IDs/keyed commissions; filled sum equals effective and terminal filled quantity; leaves zero; status Filled | Terminal | Missing/extra order; routing, scalar, fill/trade/commission, partial-Filled, and ID drift |
| Terminal position | Completed exposure/economics fold | Live position cache and result | Exactly one proof-relevant position projection: ownership/order links, canonical fills/trades, empty adjustments/voids, exact ordered fill-only replay mirror, closed flat state, P&L, keyed commissions, and counts exact | Terminal | Extra/missing/nonflat/ownership/order/fill/trade/adjustment/replay-drift/void/P&L/commission/count drift |
| Terminal account | Starting account plus economics fold | Live account cache | Exact anchored identity/type/base/balances; no margins/locks/extra currencies; cash equation exact | Terminal | Correlated account/cash drift, hidden locks, margin, currency, metadata drift |

This matrix is governed by the field-level design's finite review stop rule. A later blocking finding
must demonstrate a missing admitted claim, an unenforced included invariant, a failing behavior control,
materially ambiguous boundary language, an excluded field that concretely changes the accepted causal or
economic conclusion, or a repository MUST violation. Supporting another lifecycle, execution mode,
bookkeeping audit, or transport-hop audit is separate work.

### Payload binding closure

| Payload | Bound fields / disposition |
|---|---|
| `RunStarted`, `RunEnded` | Unique and exactly first/last store entries. |
| `SubscribeCommand`, `UnsubscribeCommand`, `TimeEvent` | Explicit control-plane waiver; integrity-covered, no economic claim. |
| `SubmitOrder` | Unique command ID; envelope IDs equal embedded initialization; store sequence and `ts_init` bind the book cursor. Client/position/parameter/correlation metadata is diagnostic under the frozen immediate-market slice. |
| Normal `OrderInitialized` | Full structural equality with the initialization embedded in `SubmitOrder`; unique event ID. |
| Settlement `OrderInitialized` | Pinned expiration ID, tag, identities, Market/GTC/reduce-only/NoTrigger shape, exact fill quantity, and absence of every price/contingent/algo field; unique event ID. |
| `OrderSubmitted` | Exactly one per normal order; identities equal intent; account equals the pre-run configured execution account; strict causal interval; unique event ID. |
| `OrderAccepted` | Exactly one for settlement and none for normal orders; identities/account/venue order/reconciliation and causal interval pinned; unique event ID. |
| `OrderUpdated` | Exactly one for the quote entry and none for base reduction/settlement; all identities, account, quantity, quote/reconciliation flags, venue-order and optional prices pinned; unique event ID and causal interval. Timestamps are diagnostic. |
| `OrderFilled` | Exact submitted identities/account/side/type, common venue order, currency, taker/reconciliation shape, unique event/trade IDs, instrument precision/increments, exact cursor-book price/quantity sequence, commission and causal position/account bindings. `info`, causation, and pre-attribution `position_id` are diagnostic. |
| `PositionOpened/Changed/Closed` | Unique event ID; position/instrument/account, side, quantity, last fill, effect kind and realized P&L equal the independent exposure/economics fold. Timestamps are diagnostic. |
| `AccountState` | Unique event ID; non-lifecycle accounts permitted only in the strict node-initialization prefix before the first normal `SubmitOrder`; lifecycle account/type/base/balance/margin/lock projection bound initially and after every fill. Timestamps are diagnostic. |
| `InstrumentClose` | Complete configured paired-binary close set; exactly one held-leg ContractExpired receipt causes fills; instrument, payoff, configured settlement input and strict settlement ordering bound. |

## Closure implementation tasks

### Task 1: Static admission and closed intake

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Test: `crates/backtesting-vertical-slice/src/runner.rs`

**Interfaces:**
- Consumes the mapped #789 manifest, projections, and selected `InstrumentAny` before
  `BacktestNode::run`.
- Produces a typed admission result and an explicitly exhaustive evidence decoder.

- [x] Add behavior tests proving zero/out-of-range/misaligned executable prices and invalid sizes
  fail before the run boundary, while Clear/Delete and binary settlement `0`/`1` remain valid.
- [x] Run each focused test and confirm RED at the absent admission helper.
- [x] Implement the smallest static admission helper and invoke it before `node.run()`.
- [x] Add unknown-payload and orphan-payload behavior tests; confirm the current wildcard accepts
  the unknown payload before replacing it.
- [x] Replace silent wildcard decoding with the frozen grammar and typed rejection.
- [x] Run the focused admission/intake tests and confirm GREEN.

### Task 2: Account-routing closure

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Test: both modules above

**Interfaces:**
- Consumes the account created from the primary venue configuration before execution and one causal
  `OrderSubmitted` per normal order; both submitted account IDs must equal that authority.
- Produces a submitted-order trace whose account anchors updates, fills, effects, account states,
  settlement, and terminal projections.

- [x] Add RED controls for missing/reordered `OrderSubmitted`, submitted-account drift, and
  correlated downstream account drift with the submission left unchanged.
- [x] Capture the primary venue's configured execution account before the run, then extend the
  submitted trace and causal assembler with that authority and exact event order.
- [x] Bind every downstream account-bearing claim and terminal projection to that authority.
- [x] Run account-routing controls and the real #789 replay; confirm GREEN.

### Task 3: Quantity-conservation closure

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/execution_contract.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Test: both modules above

**Interfaces:**
- Consumes independently normalized effective quantity and ordered fills.
- Produces only fully filled normal-order records with zero sweep remainder and consistent terminal
  quantity/status projections.

- [x] Add RED controls for insufficient book depth, coherent partial fills marked Filled, terminal
  fill-vector length/content drift, and terminal client-order drift.
- [x] Require the independent sweep remainder to be zero and fill sum to equal effective quantity.
- [x] Require terminal effective quantity, filled quantity, summed fills, zero leaves, and Filled
  status to agree.
- [x] Run quantity and terminal-order controls; confirm GREEN.

### Task 4: Matrix closure and exact-head evidence

**Files:**
- Modify only the two Rust modules and this plan if an admitted field needs an explicit waiver.

- [x] Map every admitted type and relevant field to its check or documented diagnostic-only waiver.
- [x] Add missing settlement-witness, duplicate/orphan update, non-contiguous-fill, and bidirectional
  completeness controls identified by the matrix.
- [x] Run BTE formatting, strict Clippy, focused controls, the full BTE Nextest battery, and the real
  #789 replay.
- [x] Conduct an internal adversarial review against this matrix and resolve every substantive
  finding.
- [x] Commit one closure change, push the exact head, post evidence mapped to matrix rows, and request
  fresh review from `sp-reviewer` without waiting for CI.

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
   identities, instrument and book identity, submitted raw precision/increment, quote-conversion
   identity/quantity/order, strict fill -> position -> account ordering within each causal interval,
   globally unique semantic identities, synthetic expiration origin, exact settlement remainder,
   manifest-bound account metadata, proof-relevant terminal-order/account projections, commission maps,
   realized P&L, zero-cash-delta fills, and per-fill cash.
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
