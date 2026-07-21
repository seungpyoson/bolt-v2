# Issue #789 Economic-Lifecycle Proof Boundary

**Status:** Owner-approved proof boundary; implementation and exact-head verification are in progress.

**Approval baseline:** PR #1492 at `97886122462d6f0de92621b70efe41578ea8f36f`, based on
`27b6d20e1520956d721312b8f865fa0e2d31ffbf`.

## Decision

Issue #789 proves one finite causal and economic lifecycle. It does not audit every field in
NautilusTrader's terminal objects.

The proof succeeds only when Bolt can establish, from independent inputs and NautilusTrader's sealed
logical event stream, that:

1. one quote-denominated entry was submitted and fully executed against the executable book;
2. one base-denominated normal reduction was submitted and fully executed against the executable book;
3. one synthetic settlement closed exactly the remaining normalized position;
4. commissions, realized P&L, account transitions, and terminal cash reconcile; and
5. the proof-relevant terminal order, position, and account projections identify and agree with that
   lifecycle.

No broader claim is made.

## Most Important Invariant

The validator must never certify an incorrect causal or economic result. Unsupported inputs, lifecycle
shapes, evidence, or proof-relevant terminal projections fail closed. A NautilusTrader abort produces no
proof and is not a successful validation.

## Exact Scope

The only admitted run has:

- one selected binary-option instrument and its complementary projected leg;
- one configured execution account;
- `NETTING` OMS, `CASH` account, and `L2_MBP` book;
- liquidity consumption enabled;
- market-order acknowledgements disabled;
- no configured fill, latency, or fee model;
- one quote-denominated market entry;
- one base-denominated market reduction that leaves a nonzero position;
- one synthetic expiration settlement that closes the residual;
- taker fills only;
- no reversal, reopen, additional entry, additional reduction, or partial settlement;
- zero configured fees, while all observed commissions must still reconcile exactly; and
- NautilusTrader's normalized logical event stream, not raw message-bus hop multiplicity.

Any different shape is unsupported and must not receive a successful contract report.

## Proof Authorities

The proof distinguishes economic authorities from a trusted causal carrier.

Only these sources are independent economic authorities:

- canonical resolved manifest/config bytes and their SHA-256;
- hash-bound catalog rows and the submission cursor into those rows;
- the selected instrument definitions, including precision, increments, multiplier, currencies, and
  price bounds;
- the pre-run execution-client registry for the configured account identity;
- manifest starting cash; and
- the manifest settlement mapping for the exact two projected binary legs.

The sealed NautilusTrader event store and its verified sequence are trusted only to establish that a
logical payload was observed and where it occurred relative to other payloads. Captured `SubmitOrder`
bytes therefore establish occurrence and causal intent, but their quantities, identities, and semantics
remain claims that must be bound to the authorities and fold. Other NautilusTrader events and terminal
caches are also claims to be checked. Agreement among engine-produced claims never creates an independent
economic authority.

## Captured-Claim Field Registry

The validator proves only the following fields from admitted execution payloads. Fields omitted from
this registry are not part of #789 unless the exclusion rule below brings them into scope.

| Payload | Included fields and invariant | Explicitly excluded fields |
|---|---|---|
| `OrderInitialized` and embedded `SubmitOrder` initialization | for normal orders: command/event identity; trader, strategy, instrument, client-order, and execution-algorithm identity; side; `Market` type; quantity; quote flag; post-only false; reconciliation false; envelope equals embedded initialization. The synthetic settlement initialization additionally includes every time-in-force, reduce-only, optional-field, trigger, and tag constraint in the exact tuple below | timestamps and diagnostic rendering; normal-order time-in-force and reduce-only, because role comes from position effect and exact full execution makes them economically inert |
| `SubmitOrder` | store sequence, command identity, client-order identity, and every registered embedded-initialization field above | transport-hop multiplicity |
| `OrderSubmitted` | event identity and sequence; trader, strategy, instrument, client-order, and configured account identity | timestamps |
| `OrderAccepted` | settlement only: event identity and sequence; trader, strategy, instrument, client-order, configured account, and venue-order identity; reconciliation false | timestamps |
| `OrderUpdated` | entry only: event identity and sequence; trader, strategy, instrument, client-order, and configured account identity; independently derived effective quantity; absent venue-order identity before acceptance; quote flag cleared; reconciliation false; absent price, trigger price, and protection price | timestamps |
| `OrderFilled` | event and trade identity; sequence; trader, strategy, instrument, client-order, venue-order, configured account, side, `Market` type, `Taker` liquidity, price, quantity, currency, commission, reconciliation flag, and settlement/normal cause | timestamps; raw fill `position_id` captured before NT attributes it, which is instead bound through position effects and the terminal position |
| `PositionOpened/Changed/Closed` | event identity and sequence; trader, strategy, instrument, position, configured account, opening order, entry side, position side, signed and absolute quantity, last quantity, last price, currency, effect kind, and realized P&L; the raw NT signed `f64` must be finite and normalize at the instrument size precision to the independently folded exposure; `PositionClosed` additionally binds the settlement closing order | timestamps, average-price/return caches, unrealized P&L, peak quantity, and diagnostic rendering |
| `AccountState` | event identity and sequence; configured account, account type, base currency, exact balance and margin maps including every key, and causal placement after each position effect. A different-account state is admitted only in the strict node-initialization prefix before the first normal `SubmitOrder`; it remains identity/integrity-covered and carries no lifecycle claim | timestamps; reported/calculated presentation status when the exact economic projection is identical |
| `InstrumentClose` | store sequence, instrument identity, close price, exact paired-leg membership, and position relative to synthetic settlement evidence | timestamps |
| `RunStarted` / `RunEnded` | exactly one of each, with every economic entry strictly inside their sequence interval | non-economic payload body |
| `SubscribeCommand` / `UnsubscribeCommand` / `TimeEvent` | admitted by type and required to remain inside the run envelope; any cardinality is allowed | non-economic payload body |

Captured rejection, denial, cancellation, expiration, trigger, adjustment, replay, fill-void, and other
order/position payload types are not alternative valid outcomes. They are rejected as unsupported
lifecycle evidence.

### Identity equivalence and uniqueness

- Every persisted command/event UUID is globally unique across all admitted logical entries.
- The `OrderInitialized` embedded in one `SubmitOrder` intentionally equals that order's separately
  captured `OrderInitialized`, including its event UUID; this is one semantic initialization represented
  in two bound carriers, not two persisted event occurrences.
- Exactly three distinct client-order IDs exist: entry, reduction, and settlement. All evidence assigned
  to one order uses that order's ID, and no evidence crosses the three equivalence classes.
- Each order has exactly one venue-order ID after routing, and the three venue-order IDs are distinct.
- Every fill has a globally unique trade ID and event UUID.
- All position effects and proof-relevant terminal fills bind to exactly one position ID.
- Every order, fill, position effect, account state, and terminal projection binds to the one configured
  account ID.

### Canonical proof-fill projection

Stored and terminal fills are compared through one canonical projection containing exactly the registered
`OrderFilled` fields above. Store sequence remains external ordering metadata. Timestamps and the raw
captured `position_id` are removed before comparison; the terminal fill's position ID is checked
separately against the single lifecycle position. "Exact fills" below always means exact equality of this
canonical projection, never whole-struct equality.

## Validation Pipeline

### 1. Admission

Before NautilusTrader executes, validate the frozen run configuration, both projected instruments,
settlement mapping, configured execution account, and every catalog delta used by the run.

Book-delta grammar is exact:

- `Clear` is structural and may use `NoOrderSide`;
- `Add`, `Update`, and `Delete` require `Buy` or `Sell`;
- non-`Clear` prices are positive, have exact instrument precision and increment, and fall within any
  configured minimum and maximum;
- `Add` and `Update` sizes are positive and use exact instrument precision and increment;
- `Delete` may carry zero size but must otherwise identify a valid side and price; and
- every row belongs to the selected instrument.

The settlement set contains exactly the two distinct projected binary instruments and exactly the
complementary payoffs `0` and `1` at the declared precision.

### 2. Capture and integrity

Use NautilusTrader's event store and marker sidecar. Open capture before the run, seal it afterward, and
verify hashes, dictionaries, contiguous sequence numbers, marker gaps, and submission cursors before
decoding economic evidence.

The admitted payload grammar is closed. The five store/control payloads named in the registry and the
bounded different-account `AccountState` initialization prefix are the only explicitly waived
non-economic entries. All later execution payloads must be assigned to the entry, reduction, settlement,
position, or configured-account chains. Unknown execution payload types and admitted execution payloads
that cannot be assigned fail typed.

### 3. Causal assembly

The normal entry prelude is:

`OrderInitialized -> SubmitOrder -> OrderSubmitted -> OrderUpdated`

The entry must be quote-denominated. Its single `OrderUpdated` must convert the submitted quote amount
to the independently derived base quantity between submission and the first fill.

The normal reduction prelude is:

`OrderInitialized -> SubmitOrder -> OrderSubmitted`

The reduction must be base-denominated and must have no `OrderUpdated`.

Normal orders have no `OrderAccepted` because the admitted configuration disables market-order
acknowledgements.

Each normal-order prelude is followed by one or more exact causal triplets:

`OrderFilled[i] -> PositionEffect[i] -> AccountState[i]`

For entry, the first effect is `PositionOpened`; any later entry fills produce `PositionChanged`. For
reduction, every effect is `PositionChanged`. Every triplet is strictly ordered before the next fill.
The final entry exposure is nonzero. The final reduction exposure is nonzero and smaller in magnitude
than the entry exposure.

At the pinned revision, `PositionChanged` has no closing-order field. Its causal order is therefore bound
by the immediately preceding fill in the strict triplet: matching trader, strategy, instrument, account,
position, last quantity, and last price, while `opening_order_id` remains the entry order. Only
`PositionClosed` carries `closing_order_id`, which must equal the synthetic settlement order.

The settlement grammar admits exactly one settlement fill:

`InstrumentClose input -> synthetic OrderInitialized -> OrderAccepted -> OrderFilled ->
PositionClosed -> AccountState -> InstrumentClose receipt`

Settlement has no `SubmitOrder`, `OrderSubmitted`, or `OrderUpdated`. Its synthetic identity and shape
must match the pinned expiration path, it must reuse the configured lifecycle account, and its single
fill must close the exact residual. Multiple or partial settlement fills are unsupported.

The pinned synthetic `OrderInitialized` tuple is exact:

- client-order ID has `EXPIRATION-{venue}-{UUID4}` shape;
- trader, strategy, instrument, client-order ID, and side equal the settlement fill;
- type is `Market`, time-in-force is `GTC`, quantity equals the exact residual at instrument precision,
  and quote denomination is false;
- reduce-only is true; post-only and reconciliation are false;
- price, activation price, trigger price, limit offset, trailing fields, expiry, display quantity,
  emulation trigger, trigger instrument, contingency, order-list, linkage, parent, execution-algorithm,
  algorithm-parameter, and spawn fields are absent;
- trigger type is `NoTrigger`; and
- tags contain exactly `EXPIRATION_{venue}_CLOSE`.

The single `OrderAccepted` must match the settlement trader, strategy, instrument, client-order,
configured account, and venue-order identities and have reconciliation false. This tuple is a
version-pinned causal witness derived from the pinned NautilusTrader expiration path; settlement price,
quantity, account, and economics remain independently checked against manifest and fold authorities.

### 4. Independent fold

For each normal order, reconstruct the opposing executable book from the hash-bound catalog prefix at
the submission cursor. A minimal best-price-first sweep must exactly equal the ordered fill price and
quantity pairs, leave zero unfilled quantity, and have a fill sum equal to the effective order quantity.

Classify lifecycle roles only from signed position effect:

- zero to nonzero exposure is entry;
- same-sign exposure with smaller nonzero magnitude is normal reduction; and
- the final opposite-side effect that closes the exact residual is settlement.

Timestamps never classify lifecycle roles.

For every fill:

`exposure_after = exposure_before + signed(fill_quantity)`

Cash, realized P&L, and commissions are independently folded using instrument-derived precision,
increment, multiplier, and currencies. Every position effect and account transition must match the fold
in causal sequence, including a zero-cash-delta transition.

### 5. Proof-relevant terminal projections

Terminal caches are checked only after the causal fold succeeds. They are completeness cross-checks, not
the source of the lifecycle story.

#### Terminal order fields included in the proof

For every entry, reduction, and settlement order, bind:

- trader ID;
- strategy ID;
- instrument ID;
- client order ID;
- configured account ID;
- venue order ID;
- position ID;
- side and order type;
- original quantity and original quote-denomination flag;
- effective quantity and current quote-denomination flag;
- terminal status;
- filled and leaves quantities;
- ordered canonical proof-fill projections and trade IDs; and
- commission map with both currency keys and `Money.currency` checked.

The terminal order must represent the same causal order, be `Filled`, have zero leaves, and satisfy:

`effective quantity = summed causal fills = terminal filled quantity`.

Post-only and reconciliation are enforced on captured normal intent. The pinned synthetic settlement
witness enforces its time-in-force and reduce-only shape. Normal-order time-in-force and reduce-only are
not lifecycle classifiers and need not be independently re-proven from the terminal cache.

Terminal-order expected values come from one of four places: static identity and flags from the bound
`OrderInitialized`; account identity from the pre-run execution-client registry and `OrderSubmitted`;
venue/position/trade identities and ordered fills from the sealed causal events; and quantities, status,
commissions, and zero leaves from the independent fold.

#### Terminal position fields included in the proof

Bind:

- trader, strategy, instrument, position, and configured account IDs;
- opening order ID and settlement closing order ID;
- entry side;
- exact ordered canonical proof-fill projections and exact trade-ID set;
- closed/flat state, zero quantity, and zero signed quantity;
- realized P&L; and
- the exact keyed commission map with currency-key consistency.

Because the admitted lifecycle is fill-only, terminal adjustments and fill voids must be empty. At the
pinned NT revision, `Position::apply` records the causal fills in `replay_events`; that collection must be
the exact ordered fill-only canonical mirror. An adjusted or otherwise unexplained replay entry is
unsupported economic state, not harmless diagnostics.

Terminal-position expected values come from the bound initialization/submission identities, the single
position-effect chain, the ordered causal fills, and the independent exposure/P&L/commission fold. No
terminal position field is allowed to become an authority for the fold that checks it.

#### Terminal account fields included in the proof

Bind:

- configured account ID;
- `CASH` account type;
- manifest-derived base currency;
- exact balance map including every key;
- exact cash-lock map including every key; and
- exact margin maps including every key.

The admitted account has one expected currency, no margins, and no locks. Initial cash, every causal
account transition, final `AccountState`, current account cache, folded P&L, and commissions must agree.

Terminal-account expected values come from the configured execution account, manifest venue/base
currency/starting cash, and the independent per-fill cash and commission fold.

## Explicit Exclusions

The proof does not claim correctness of terminal fields that cannot alter the accepted causal or economic
conclusion, including:

- event debug strings and denial diagnostics after a successful lifecycle;
- previous status history;
- normal-order time-in-force and reduce-only after exact full execution has already been proven;
- terminal-order voided/overfill counters and non-fill cached event history when the sealed event grammar,
  exact fills, filled quantity, zero leaves, and `Filled` status all agree;
- event, submission, acceptance, update, or close timestamps beyond causal store sequence;
- duration fields;
- average-price caches and realized-return convenience fields when exact fills and folded P&L already
  provide the authority;
- peak, aggregate buy, and aggregate sell quantities when exact fills and exposure are already checked;
- raw message-bus delivery multiplicity normalized away by NautilusTrader's event store; and
- support for other instruments, account modes, order types, maker behavior, latency, fill models,
  nonzero fee models, margin, FX conversion, reversals, multiple reductions, or physical exercise.

An excluded field becomes in scope only if a concrete example shows that changing it can preserve every
included check while changing the causal or economic conclusion. Expanding product behavior requires a
separate issue.

## Failure Contract

- Invalid owned inputs fail typed during admission before the run.
- Invalid, incomplete, ambiguous, duplicated, orphaned, or reordered captured evidence fails typed after
  integrity verification.
- Divergent proof-relevant terminal projections fail typed.
- Unsupported lifecycle shapes fail typed when observed.
- An unexpected NautilusTrader panic aborts the run and produces no contract report. It is never converted
  into successful or partial proof. General panic containment is outside #789.

## Behavioral Evidence Matrix

Each row requires a positive behavior test and the listed negative class.

| Boundary | Required negative behavior |
|---|---|
| Run configuration | Wrong OMS/account/book/liquidity/models or enabled market acknowledgements |
| Run envelope | Missing, duplicate, reversed, or non-boundary `RunStarted`/`RunEnded`; any economic, subscription, unsubscription, or time payload outside the unique envelope |
| Identity model | Reused command/event UUID, client-order ID, venue-order ID, trade ID, or cross-order evidence; mismatched embedded initialization |
| Book grammar | Wrong instrument, invalid action/side pairing, zero/out-of-range/misaligned price, invalid size |
| Entry role | Coherent base-denominated entry without an update |
| Reduction role | Coherent quote-denominated reduction with a valid update |
| Quote conversion | Missing, duplicate, orphaned, reordered, or field-drifted update |
| Normal execution | Missing/extra/reordered fill, insufficient depth, price/quantity/identity drift |
| Position effects | Missing/reordered/wrong kind, identity, account, quantity, P&L, reversal, or reopen |
| Account effects | Missing/replayed/conflicting/trailing state or account/cash/currency drift |
| Settlement | Wrong leg set, payoff, origin, order, account, side, quantity, price, or residual |
| Terminal order | Identity/routing, quantity/status, fill/trade, quote-state, or commission-map drift |
| Terminal position | Identity/order/trade, nonflat, economic, commission-key, adjustment/replay/void drift |
| Terminal account | Identity/type/base/balance/lock/margin/cash drift |
| Integrity and closure | Tampered/gapped/ambiguous store, unknown payload, or admitted orphan payload |

Tests mutate behavior and data. They do not scan source structure.

## Review Stop Rule

A new blocking finding must demonstrate one of:

1. a malformed in-scope lifecycle that receives a successful proof;
2. a valid declared lifecycle that is rejected;
3. an admitted input or event that can be silently omitted or can bypass the independent fold;
4. an included field or equation that is not enforced;
5. an excluded field that can concretely change the causal or economic conclusion while every included
   invariant still passes; or
6. ambiguous, contradictory, or unbounded wording that permits materially different conforming
   implementations; or
7. a repository `MUST` violation.

A request to validate excluded bookkeeping without demonstrating item 5 is scope expansion. A request to
support another execution shape belongs in a separate issue.

## Consequences for the Current Review Findings

Under this boundary:

- role-to-denomination binding is a valid defect;
- non-`Clear` `NoOrderSide` book rows are a valid admission defect because they can be silently ignored;
- terminal order routing and causal identity fields listed above are valid projection gaps;
- terminal position causal identity, trade IDs, commission keys, nonempty adjustment/void collections,
  and replay drift from the exact fill-only mirror are valid projection gaps;
- terminal timestamps, duration, cached average prices, and aggregate convenience quantities are
  explicitly excluded;
- disabled market-order acknowledgements are part of admission because the causal grammar depends on
  that setting; and
- general NautilusTrader panic containment remains excluded because an abort cannot produce a successful
  proof.

## Required Alignment Before Implementation

After owner approval, the existing #1492 plan must adopt this document verbatim as its proof boundary.
Unqualified phrases such as "complete terminal projection" must be replaced with "proof-relevant terminal
projection" and a link to the exact field registry above. The plan and implementation may not retain a
broader or narrower terminal claim.

No Rust change is authorized until that alignment and the architecture review are approved.

## Acceptance Criteria

The design is settled when:

- the owner approves this exact claim and field boundary;
- an architecture-only review finds no ambiguity, contradiction, or unbounded term;
- every included field has one authority or derived invariant;
- every exclusion is explicit;
- the behavioral matrix is finite; and
- implementation remains paused until those conditions are met.
