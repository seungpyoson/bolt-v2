# Manipulated-pump research: data authority and managed-stack decision

**Decision date:** 2026-08-19
**Status:** public and paid CoinAPI pilot plus Surf API capability probe executed;
no provider has passed every mandatory gate

## Decision

Surf Data API must **not** be treated as the source of truth for Bolt research or
backtests. Use it as a convenient discovery, screening, and enrichment service.
An individual Surf dataset may become an accepted input only after it passes the
same source-proof gates as any other provider and its raw response is frozen.

The least-regret stack is:

1. Keep Surf Data API as a cost-metered discovery and on-chain locator, but make
   no further paid calls in the current slice. Do not automate the hard-evidence
   workflow through Surf Chat, or let Chat output or current wallet labels
   directly authorize a historical claim or trading decision.
2. Build the ex-ante event universe from inexpensive, point-in-time market data,
   including triggered successes, partials, failures, and matched controls.
3. Buy granular CEX data only for selected event and control windows after the
   no-purchase phase demonstrates value. CoinAPI has been rejected for canonical
   replay; Tardis remains an unadmitted fidelity challenger, not a selected source.
4. Use Surf Onchain SQL for the bounded on-chain pilot after rights and
   completeness pass; use Dune raw EVM tables as the full-population challenger.
   Add Allium only for a named unmet query, coverage, or latency requirement.
   Treat every entity label as versioned probabilistic enrichment.
5. Keep NautilusTrader's catalog plus Bolt's immutable source proofs as the one
   canonical experiment path. Use S3-compatible object storage and ephemeral
   batch compute; do not add QuantConnect as a second backtest truth.

This is deliberately not a premature “Tardis + Allium” selection. Provider choice
is the output of the acceptance tests below.

## Public pilot result

The fixed MYX pilot is recorded in
[`manipulated-pump-pilot/manifest.toml`](manipulated-pump-pilot/manifest.toml) and
[`manipulated-pump-pilot/evidence-ledger.md`](manipulated-pump-pilot/evidence-ledger.md).
It produced several decision-changing results:

The manifest is a non-admission observational ledger. Several source bytes were
discarded or retained only ephemerally, so the hashes prove identity only to the
party that saw those bytes; they do not let an independent reviewer replay the
pilot. No provider may be admitted from this record. An admission attempt must be
rerun after rights pass with retained or retrievable source objects, locators,
timestamps, schema/parser versions, and derivation lineage.

- Surf Onchain SQL passed a 100/100 BNB Chain receipt and log identity sample, but
  Surf still fails closed for canonical use because commercial retention rights
  and full-population recall are unproved. Surf labels also fail historical
  as-of use because they expose no source, observation time, effective interval,
  or revision identifier.
- Tardis public samples for Binance Futures, Bybit, and Gate.io Futures converted
  deterministically into NT-native events in two bounded runs. The exact six
  days, Gate's uninterrupted interval, and raw disconnect evidence require paid
  or vendor-furnished access.
- The Tardis public one-off formula prices 18 selected instrument-days at $36,
  but its minimum order makes the actual pilot purchase $300 before tax. No
  checkout, vendor contact, or purchase was performed.
- CoinAPI is the lower-cost challenger: its current S3 plan is pay as you go with
  no commitment, full order books start at $8/GB, and trades start at $24/GB.
  Exact MYX object listings require an API key, and its public material has not
  proved matching funding/open-interest files or post-subscription raw reuse. A
  public Bybit perpetual preview was inspectable, but its documented 100-row
  download did not yield a CSV through the routed fetch and was not replayed.
- The registered CoinAPI key loaded successfully through the canonical registry.
  After $5 in credits was added, CoinAPI returned all target MYX metadata and one
  exact paid Binance incident file. The file is valid normalized L2 but omits
  sequence, disconnect, and raw-payload evidence, so CoinAPI is rejected as the
  canonical replay source.
- The Surf Data API key and typed REST path were proved with a one-credit request.
  The same gateway did not prove Chat automation: a request naming `fable-5`
  completed and reported `model = "surf-2.0"`; internal aliasing or execution is
  unknown. Two `surf-2.0` `xhigh` runs of the research prompt failed to produce a
  completed answer within 20 minutes. The streaming run returned an in-progress
  response ID before the server closed the HTTP/2 stream; a follow-up retrieval
  attempt returned 404.

None of the reviewed public feeds or provider candidates exposed market-by-order
data for the selected target venues and period. The observed book evidence is L2,
so exact queue replay is unsupported and fills remain bounded assumptions.

## What “source of truth” means

No commercial aggregator is truth in the absolute sense. Bolt needs a narrower,
auditable contract:

| Evidence class | Examples | Permitted claim |
|---|---|---|
| Authoritative primitive | Finalized chain block, receipt, log or trace; venue-native message captured with sequence and timestamps | What that identified source emitted, subject to finality or a documented capture gap |
| Accepted experiment input | Immutable raw object plus hash, manifest, parser version, coverage, gaps, license, and point-in-time semantics | Canonical input to one named experiment |
| Deterministic derivation | Transfer decoded from a specific log; feature produced by pinned code from accepted inputs | Reproducible transformation, not independent truth |
| Probabilistic enrichment | Wallet/entity label, clustering, circulating-float estimate, anomaly score, queue/fill estimate | A feature with provenance, confidence, effective time, and sensitivity bounds |
| Narrative/model output | Surf Chat/Fable report, GPT/Grok analysis, causal story, actor intent | Hypothesis or research aid only |

For EVM data, the strongest primitive is a finalized chain record identified by
chain ID, block number and hash, transaction hash and index, and log index (or a
trace address where applicable), with a stated reorg/finality rule. For CEX data,
the venue is the origin, but any historical file is still a fallible capture. It
needs native event/sequence time, local receipt time, disconnect and incident
records, and an explicit gap policy.

Bolt's existing
[source-proof contract](../../specs/023-nt-research-analytics-platform/reference/backfill-source-proof-schema.md)
already requires the right boundary: access, license, schema, time semantics,
point-in-time instrument universe, coverage, retention, granularity,
completeness, NT mapping, cost, storage, forbidden claims, and immutable
supersession. “Accepted” means canonical for the experiment—not that the provider
can never be wrong.

## Surf assessment

### What is usable

- Typed exchange, token, wallet, market, and on-chain queries make Surf a strong
  research front end.
- Historical listing/delisting filters and `time_range = "max"` token prices can
  cheaply locate candidate instruments and dates before deeper collection.
- On-chain SQL exposes broad indexed tables and is useful for cohort construction
  and feature prototypes.
- Transfer rows containing transaction hash, block number, addresses, timestamp,
  token amount, and USD amount can locate evidence that is then independently
  reconstructed.
- The Onchain SQL `agent.bsc_transfers` table also exposes transaction index,
  event index, and raw integer amount. In the pilot, all 100 stratified rows
  resolved uniquely to successful BNB Chain receipt logs and canonical block
  headers.
- Label confidence is useful as a feature if the exact response and observation
  time are frozen.

### Why it is not canonical by default

- The documented exchange `depth` endpoint is a live snapshot, not historical
  L2/L3 replay. Market prices and token/on-chain datasets also have documented
  refresh intervals, so their availability time differs from event time.
- The live schema automatically reduces token price history at 180 days or more
  to daily granularity, which can miss a pump and reversal inside one UTC day.
  Its persisted exchange-candle coverage endpoint is limited to six named spot
  venues, while generic exchange klines inherit exchange-specific history limits.
  Surf alone therefore cannot prove an all-token, all-venue recall claim.
- The typed token-transfer response does not document transaction index, log
  index, raw integer amount, or block hash. Transaction hash plus block number is
  insufficient to disambiguate every log or prove receipt-level completeness.
- Surf's address intelligence is confidence-scored and may be omitted when label
  enrichment times out without failing the main request. The docs do not promise
  historical `effective_from`, `effective_to`, revision ID, or as-of label queries.
  Applying today's label to a past observation would introduce look-ahead.
- On-chain SQL is indexed and refreshed rather than a direct node receipt. It has
  row and synchronous-runtime limits. Some documented aggregates, such as active
  addresses, are approximate.
- The general terms disclaim output accuracy and completeness and, as currently
  written, do not establish the commercial storage and derived-data rights Bolt
  requires. They restrict commercial output use without written consent and also
  contain a generic automated-extraction prohibition whose relationship to the
  agent-oriented Data API is unclear. API access and unused credits are not
  evidence of the rights Bolt needs.
- Chat is a research engine. Its prose, source selection, causal interpretation,
  and entity claims are model outputs, not reproducible datasets. The public Chat
  API documents `surf-2.0` and `surf-2.0-instant`; it does not expose “Fable 5” as
  a selectable model ID.

### Chat/API capability probe

The authenticated Data API path is usable. A BTC price request returned HTTP 200,
720 hourly observations, and `credits_used = 1`. This proves key resolution and a
structured typed-data response; it does not prove repeated-byte determinism, or
the provenance or completeness of every Surf dataset.

The Chat-compatible endpoint behaved differently:

- A minimal request with `model = "fable-5"` returned HTTP 200 and
  `status = "completed"`, but the response identified its model as `surf-2.0`.
  Fable 5 is therefore not a proved API-selectable model.
- The exact manipulated-pump prompt sent to `surf-2.0` with `xhigh` reasoning and
  a 16,000-token output cap returned no bytes before a 1,200-second client timeout.
- A stricter evidence-contract version sent as SSE returned `status = "in_progress"`
  and response ID `resp_e077daef8da902ee4441ac3bdc632ecf`, then the server closed
  the HTTP/2 stream after approximately 1,200 seconds without output deltas or a
  completion event. `GET /gateway/v1/responses/{id}` returned 404.
- The attempted credit-balance endpoint returned 401, and the live Data API
  OpenAPI document exposes neither the Chat response path nor a response-retrieval
  path. Actual charge reconciliation for the failed calls is therefore unresolved.

Do not spend more Surf Chat credits or build an automated hard-evidence workflow
on this endpoint until Surf supplies a working long-running completion/retrieval
contract, supported model identifiers, cancellation semantics, and auditable
billing. Typed Data API remains technically testable, but this decision authorizes
no additional paid query.

Accordingly, Surf's typed transfer endpoint remains `METADATA_ONLY`. The tested
Onchain SQL transfer table passed technical row-identity proof, but remains
non-canonical until licensing/retention and full-population completeness pass.

There are two distinct failure modes. A wallet label or fill estimate is
intrinsically probabilistic. A missing transfer, stale watermark, duplicate row,
or unrecorded CEX disconnect is instead a completeness/availability failure. A
transaction hash can be deterministic once verified against a finalized receipt,
while the claim that its wallet belongs to an operator remains probabilistic.
Confidence scores do not repair missing data, and checksums do not make an
interpretation true.

## Work that remains probabilistic

These are not bugs that a more intelligent model can eliminate:

| Inference | Why uncertain | Required treatment |
|---|---|---|
| Wallet/entity identity and clustering | Shared infrastructure, deposit addresses, bridges, custodians, and changing attribution | Freeze vendor, query time, label, confidence, and evidence; include an unknown class and rerun without labels |
| Actor intent or “manipulation” | Transactions and orders reveal actions, not mental state or common control | Describe observable mechanism; never state identity or intent without independent public evidence |
| Exchange inflow as operator inventory | Arbitrageurs, market makers, users, and internal exchange movements can create the same flow | Classify venue role; separate gross and net flow; require alternative-explanation tests |
| Circulating float and concentration | Supply classifications, vesting, bridges, burns, and entity clusters are revised | Version denominator and availability time; publish results across plausible float bounds |
| USD value of transfers | Price source, timestamp alignment, liquidity, and decimals can differ | Preserve raw units and price provenance; do not use USD value as the primary event identity |
| L2 queue position and fills | L2 lacks order IDs and hidden/iceberg liquidity; network and own-order latency are unobserved | Report conservative/base/aggressive fill bounds; no exact-PnL claim |
| Cross-venue ordering | Exchange clocks and local receipt clocks differ; outages and batching reorder apparent events | Preserve both timestamps, estimate clock error, and reject claims inside the uncertainty interval |
| Anomaly/pump probability | Regime shift, selection bias, survivorship, and small samples | Walk-forward/out-of-sample evaluation with calibration, base rates, failures, and controls |
| AI-produced causal synthesis | Model source selection and reasoning are nondeterministic and may invent unsupported links | Use it to generate tests; every final claim must trace to frozen rows and code |

Even L3 is not perfect truth because hidden liquidity and counterfactual reaction
to our own orders remain unknowable. More importantly, none of the reviewed public
feeds for Binance, Bybit, and Gate.io supplied true L3 order-level data for the
target history. No candidate proved that it could recover absent order IDs;
target-perp replay is therefore limited to L2 evidence in this decision.

## Requirement-to-source boundary

| Requirement | Primary candidate | Not acceptable as substitute |
|---|---|---|
| Current and delisted perp universe | Venue metadata/archive, optionally CoinGlass as cross-check | Today's surviving symbols only |
| Trades, funding, liquidations and OI screen | Venue archives/API or proven vendor tables | Inferring OI from funding; daily bars for intraday ordering |
| Historical L2 event-window replay | An unselected raw/native CEX challenger admitted on the fixed windows | CoinAPI normalized L2, Surf live depth, QuantConnect top-of-book quotes, snapshots called “L2” |
| Historical EVM event identity | Surf Onchain SQL or Dune raw logs after full-population proof, plus sampled archival-node receipt verification | Surf typed transfer alone; current explorer page |
| Wallet/entity hypotheses | Surf or Arkham frozen enrichment | A label presented as certain or retroactively applied |
| Replay and experiment record | Existing NT catalog, manifests, hashes, source proofs, reports and snapshots | Vendor UI backtest or QuantConnect as a second canonical engine |

Allium and Amberdata remain challengers, not defaults. Allium has useful raw EVM
blocks, transactions, logs, traces, and transfers, but its relevant API offering
is enterprise-oriented. Amberdata is the closest one-contract managed market plus
on-chain option on paper, but exact coverage, bulk history, rights, and price are
quote-dependent. Either may win only by passing the same test at lower total
burden.

QuantConnect is a good cloud idea-testing environment at trade/bar/top-of-book
fidelity. Its documented crypto history limits and fill models do not meet this
project's 2–3-year L2 replay requirement, and adopting it as authoritative would
create a second simulation path beside NT.

## Three bounded operating options

### A. Discovery and event-manifest construction — recommended now

Use permitted official/free trade, kline, funding, and metadata sources. Treat the
already-frozen Surf pilot only as design evidence. Build the universe and evaluate
coarse signal timing, but explicitly forbid order-book, queue, fill, and
executable-PnL claims.

Choose this until a predeclared strategy family shows enough out-of-sample signal
to justify granular data. This uses no additional provider credit, subscription,
or multi-year local archive.

### B. Targeted replay pilot — deferred

Keep A for universe construction, then acquire L2 only for every deterministic
trigger plus matched non-trigger and failed-event controls. Test one unadmitted
raw-replay candidate on the fixed symbol-windows; CoinAPI is excluded from this
role by the paid fidelity result. Admit Surf Onchain SQL for EVM events only after
rights and recall pass; use Dune as the full-population challenger. Either
provider's curated transfer table is a locator until raw event identity is
verified. Convert only accepted objects into NT and store immutable raw plus
normalized artifacts in S3-compatible storage.

Public core samples for MYX on Binance, Bybit, and Gate.io totaled approximately
150.9 MB compressed on 2025-09-01. This quiet-day value must not be multiplied into
an event-day storage claim: the 2025-09-08 Binance quote volume was over 100 times
the sample day's measured notional. Exact event files determine storage. The
minimal design downloads only manifest windows into object storage and deletes
ephemeral conversion/replay workers after hashes and reports are retained.

### C. Managed warehouse — lowest operational burden, quote-gated

Ask Amberdata for an exact startup quote and sample delivered through its managed
warehouse/object-store path. Select it only if exact venue/symbol/date coverage,
raw fidelity, incident evidence, rights, and total cost beat B. Do not infer this
from the product page.

## Acceptance tests and stop/go gates

### Gate 1 — legal and retention

Before storing provider data, obtain durable evidence that research, commercial
use, derived tables, model training/backtesting, and retained raw/normalized
artifacts are allowed. Record redistribution restrictions. A personal-only,
unknown, or ambiguous scope fails closed.

**Surf stop condition:** no written commercial and retention clarification for
the exact Data API outputs used by Bolt. Surf remains an interactive discovery
tool in that case.

Tardis's public Terms clause 9.1 permits Permitted Use, Derived Data creation,
and storage of Data and Manipulated Data on the customer system. That is enough
for an active-term internal pilot. The Term is invoice-defined, however, and
clause 13.6 does not clearly grant continued use of a standalone raw archive
after expiry; it only says previously incorporated Data or Derived Data need not
be removed from products. Long-term raw replay therefore remains fail-closed.

CoinAPI's public agreement grants Cloud Service use during the Subscription Term
and says external data-source restrictions may require additional licences. It
does not expressly resolve continued use of downloaded market-data files after
termination. CoinAPI therefore has the same unresolved raw-retention gate.

### Gate 2 — exact catalog coverage

For MYX, IP/DATA, MAVIA, TUT, RARE, ALPACA, TRB, AERGO, and a predeclared control
set, require machine-readable listings for exact venue, instrument, data type,
first/last timestamp, delisting/rebrand lineage, and known incidents. Coverage
must be checked before payment; a general “seven years of crypto data” claim does
not pass.

Tardis's public metadata was queried on 2026-08-19 and reports MYX plus trades,
incremental L2, quotes, snapshots, and derivative ticker on Binance Futures from
2025-06-18, Bybit from 2025-08-05, and Gate.io Futures from 2025-08-07. The same
metadata reports a 19-minute Binance exchange-side data gap on 2025-08-29 and
Bybit data-loss incidents on 2025-08-14, 2025-09-22, and 2025-10-10.

Bitget was rejected as a common-window venue. Its metadata reports one continuous
range from 2025-05-14, while Bitget's official notices show a 2025-06-13 delisting
and 2025-09-10 relisting. Public Tardis files are correctly empty inside that
interval. Exact coverage therefore requires listing intervals, not only
`availableSince`. CoinAPI's paid metadata covered all three target perpetuals and
the six broad date ranges. Its billed S3 listings proved one exact Binance
incident object; the other venue/date/type tuples were not queried because
listings are non-recursive and consume credits. The representative file then
failed the mandatory raw replay fidelity gate, so those remaining tuples are no
longer worth paid discovery for this role.

### Gate 3 — sample fidelity

Request the same six days from each CEX challenger: three quiet days, two pump
days, and one documented incident window. Require:

- exchange-native raw payload or a lossless normalized representation;
- event/sequence time and local receipt time;
- snapshot plus incremental-delta semantics and disconnect markers;
- trade side semantics, funding, OI provenance, and liquidation provenance;
- stable object identity, byte size, checksum, and parser/schema version;
- duplicate, sequence-gap, crossed-book, negative-depth, and snapshot-recovery
  measurements;
- explicit forbidden claims for absent fields.

Reject a provider whose sample cannot reconstruct the book deterministically or
whose unexplained gaps exceed the predeclared policy. An incident is not itself a
failure; an undisclosed or unmeasurable incident is.

### Gate 4 — on-chain identity and leakage

For at least 100 stratified transfer rows per chain/query family, resolve the
provider record to archival-node block hash, transaction receipt, log index,
contract, topics/data, raw amount, decimals, and finality. Require zero identity
ambiguity. Quantities and labels are tested separately.

Freeze every label response with provider, source, query timestamp, confidence,
and effective-time fields if available. Historical features may use only
information demonstrably available at the decision timestamp. If historical
label revisions cannot be queried, labels are excluded from primary backtests and
used only in post-event interpretation or sensitivity analysis.

The Surf SQL pilot selected 100 rows across the six dates. All 100 uniquely
matched the official BNB Chain RPC by transaction/log identity, raw amount, block
hash, transaction index, event index, receipt status, and block time. This passes
row identity only. Surf's six daily counts remain unverified against a second
full-population index, so completeness is still open.

In a separate 100-row Surf label-enrichment call, only three distinct addresses
were labeled. No label contained source, observation time, effective interval, or
revision fields. Surf labels therefore fail historical as-of admission.

### Gate 5 — NT replay reproducibility

Map accepted rows to NT-native trades, order-book deltas, custom funding/OI data,
and metadata with explicit units and timestamps. Two clean replays from the same
manifest must produce identical input hashes, event counts, feature hashes, and
decision traces. Replay differences fail the source or converter; they are not
averaged away.

The pinned NT `e4167fd` probe loaded the three public Tardis data types for all
three selected venues twice. Counts and serialized NT-event hashes were identical
for all nine bounded streams. This passes conversion determinism only; exact-day
uncapped engine replay and decision traces remain pending.

Execution results must show fill sensitivity under conservative/base/aggressive
latency and queue assumptions. A strategy passes research review on robust ranges,
not one favorable fill model.

### Gate 6 — prospective validity

Define trigger, cohort, features, exclusions, event clock, labels available at
time T, and metrics before inspecting granular event outcomes. Run:

- a universe-first sweep including delisted instruments;
- every trigger, not hand-selected winners;
- failed and aborted setups plus matched non-events;
- walk-forward splits by time and venue;
- ablations without labels, USD valuation, and each provider-specific feature;
- precision/recall, calibration, base rate, drawdown, capacity, and costs—not only
  example returns.

No report, including the Fable analysis, can substitute for this test. It can
define candidate mechanisms and queries; it cannot establish out-of-sample edge.

### Gate 7 — cost before scale

Obtain an exact one-off backfill quote and a recurring-forward quote only after
the sample passes. Price the actual selected bytes, venues, symbols, windows,
egress, query scans, conversion compute, object storage, and renewal/termination
rights. Stop if the provider requires a broad recurring plan before furnishing a
verifiable sample.

CoinAPI was the first cost test because it offers no-commitment pay-as-you-go
S3 access. As of 2026-08-19, full order books start at $8/GB, trades at $24/GB,
and minute Metrics OHLCV at $8/GB. The paid incident object was 39,363,049 bytes,
making its published byte-transfer component approximately $0.315. CoinAPI's use
case separately assumes 0.25 credits per GET. Date-root results are non-recursive,
so the full declared 3-venue × 6-date × 2-dataset drill-down would require an
assumed 9 credits in listing calls. No credit-to-USD parity was recorded, so those
credits are not converted to dollars here.

More importantly, the exact CoinAPI file has only `time_exchange`,
`time_coinapi`, `update_type`, side, price, size, and `order_id`. All 4,784,711
order IDs were empty; there is no sequence number, disconnect marker, or raw
exchange payload. One exchange-time and two receipt-time reversals were observed.
CoinAPI therefore loses the fidelity bake-off regardless of its lower byte price.

Tardis remains an unadmitted fidelity challenger because exact MYX coverage is
publicly inspectable and its raw replay preserves exchange-native messages. Its
18 pilot instrument-days calculate to $36, but the published minimum makes the
purchase $300. No provider is selected until exact files, legal rights, and replay
tests pass. Amberdata wins only if managed-delivery savings outweigh its quote
without weakening fidelity or ownership of experiment inputs.

## Adversarial review results

### Fidelity attack

**Challenge:** Tardis was being treated as truth and L3 was being implied.
**Finding:** Tardis records real incident gaps, OI may originate from REST polling,
and target venues expose L2 rather than L3.
**Resolution:** raw capture is accepted only per symbol-window with gap evidence;
fillability remains bounded and probabilistic.

### Look-ahead attack

**Challenge:** current entity labels, token supply, delisting knowledge, and a
winner-led cohort can make a historical signal look predictive.
**Finding:** Surf and Arkham labels are evolving confidence-scored intelligence;
the initial Fable work also acknowledges a recall ceiling without a complete
universe-first sweep.
**Resolution:** freeze availability time, exclude non-as-of labels from primary
tests, construct the universe before outcomes, and retain failures/controls.

### Cost/storage attack

**Challenge:** granular history implies years of all-token L2 stored locally.
**Finding:** the three-venue public quiet-day core sample is 150.9 MB compressed,
but the event-day size is unknown and cannot be inferred from it.
**Resolution:** index first, purchase/download only deterministic windows, retain
immutable raw objects in object storage, and run ephemeral batch replays.

### Coverage and proof-strength attack

**Challenge:** metadata, a 100-row receipt match, and two equal converter hashes
were being allowed to prove more than they measure.
**Finding:** Tardis `availableSince` hid Bitget's delist/relist interval; receipt
identity does not prove Surf population recall; bounded NT conversion does not
prove an uncapped engine replay or strategy trace.
**Resolution:** Gate remains exact-file-gated, Surf retains a separate completeness
gate, and NT replay stays partial until an admitted candidate's exact six-window
inputs become available under a separate user-authorized pilot.

### Operational-burden attack

**Challenge:** Dune/Surf/Tardis/NT appears to be another brittle in-house platform.
**Finding:** the repo already has NT catalog/replay, object-store, manifest, hash,
coverage, cost, license, and source-proof boundaries. The missing work is narrow
provider admission and conversion, not a new backtester or data warehouse.
**Resolution:** use managed providers for acquisition/indexing and the existing NT
path for the experiment record; reject QuantConnect as a second authority.

### Single-provider attack

**Challenge:** one managed vendor would be simpler.
**Finding:** no candidate is publicly proven to cover exact long-tail CEX L2,
on-chain raw evidence, historical entity intelligence, licensing, and cost at the
required standard.
**Resolution:** keep the acquisition interfaces narrow and run the bake-off.
Amberdata remains the single-provider challenger, not an assumed winner.

## Immediate next action

Do not add more CoinAPI credits, buy Tardis, automate Surf Chat, or begin a
2–3-year backfill. The paid CoinAPI pilot already rejected it for canonical L2,
and the Surf capability probe rejected the Chat endpoint for this automated
hard-evidence workflow.

The next slice is zero-incremental-spend universe construction: define the
point-in-time token/venue universe, deterministic pump trigger, failures, and
matched controls from already-frozen observations and permitted official/free
sources. Make no Surf, CoinAPI, Tardis, paid Dune, or Allium call. Use the existing
MYX evidence only to draft the contract; it cannot admit a source or establish a
strategy result. A later named missing fact requires a separate, explicit,
cost-capped authorization. This phase must not claim queue position, fills, exact
executable PnL, actor identity, or manipulative intent.

Keep acquisition and screening outside Bolt's live runtime. Token-screener may
emit versioned source observations, coverage, and lineage; it does not define
canonical research truth. Bolt owns the point-in-time universe, trigger rules,
source admission, frozen experiment manifest, historical/as-of replay, controls,
execution assumptions, and outcomes through its NT/TOML path. Any production
trigger must graduate to typed Rust and TOML. Do not add a Surf adapter to either
repository during this slice. A later setup-specific adapter or L2 pilot remains a
separate user-authorized decision after the zero-spend phase identifies one exact
blocker and a provider passes retention and exact-file gates.

## Official sources checked

- Surf: [Data API overview](https://docs.asksurf.ai/data-api/overview),
  [token transfers](https://docs.asksurf.ai/data-api/token/transfers),
  [on-chain SQL](https://docs.asksurf.ai/data-api/onchain/sql),
  [chain-metric approximation notes](https://docs.asksurf.ai/data-catalog/chain-metrics),
  [wallet history](https://docs.asksurf.ai/data-api/wallet/history),
  [Chat API](https://docs.asksurf.ai/chat/responses),
  [pricing](https://docs.asksurf.ai/pricing), and
  [terms](https://asksurf.ai/terms-of-service).
- Arkham: [API documentation and attribution principles](https://arkm.com/api/docs).
- Dune: [data catalog](https://docs.dune.com/data-catalog/overview),
  [BNB raw logs](https://docs.dune.com/data-catalog/evm/bnb/raw/logs), and
  [EVM token transfers](https://docs.dune.com/data-catalog/curated/token-transfers/evm/token-transfers).
- Allium: [BSC historical data](https://docs.allium.so/historical-data/supported-blockchains/evm/bsc)
  and [API overview](https://docs.allium.so/api/overview).
- Tardis: [data details](https://docs.tardis.dev/faq/data),
  [current configurator/pricing](https://tardis.dev/),
  [billing/subscriptions](https://docs.tardis.dev/faq/billing-and-subscriptions),
  [terms](https://docs.tardis.dev/legal/terms-of-service),
  [Binance Futures coverage](https://docs.tardis.dev/historical-data-details/binance-futures),
  [Gate.io Futures coverage](https://docs.tardis.dev/historical-data-details/gate-io-futures),
  [raw HTTP replay](https://docs.tardis.dev/api/http-api-reference), and public
  exchange metadata for [Binance Futures](https://api.tardis.dev/v1/exchanges/binance-futures),
  [Bybit](https://api.tardis.dev/v1/exchanges/bybit),
  [Gate.io Futures](https://api.tardis.dev/v1/exchanges/gate-io-futures), and
  [Bitget Futures](https://api.tardis.dev/v1/exchanges/bitget-futures).
- Bitget: official MYXUSDT futures
  [relisting notice](https://www.bitget.com/support/articles/12560603837424) and
  [delisting notice](https://www.bitget.com/en-CA/support/articles/12560603829111).
- Gate.io: official MYXUSDT perpetual
  [listing notice](https://www.gate.com/announcements/article/46489) and
  [funding-frequency notice](https://www.gate.com/announcements/article/46667).
- CoinAPI: [full limit-order-book files](https://www.coinapi.io/products/flat-files/docs/datasets/limitbook),
  [pricing](https://www.coinapi.io/products/flat-files/pricing), and
  [one-time backfill](https://www.coinapi.io/products/flat-files/docs/use-cases),
  [S3 API](https://www.coinapi.io/products/flat-files/docs/s3-api),
  [derivatives Metrics OHLCV](https://www.coinapi.io/datasets/metrics-ohlcv), and
  [APIBricks agreement](https://www.apibricks.io/legal).
- Amberdata: [pricing](https://www.amberdata.io/pricing) and
  [historical order-book REST limit](https://docs.amberdata.io/changelog/rest-access-historical-order-book-limited).
- QuantConnect: [crypto data handling](https://www.quantconnect.com/docs/v2/writing-algorithms/securities/asset-classes/crypto/handling-data),
  [crypto dataset limits](https://www.quantconnect.com/docs/v2/cloud-platform/datasets/quantconnect/crypto), and
  [Object Store](https://www.quantconnect.com/docs/v2/cloud-platform/object-store).
- CoinGlass: [historical OI](https://docs.coinglass.com/reference/oi-ohlc-histroy)
  and [pricing/history limits](https://www.coinglass.com/pricing).
- AWS: [Athena pricing](https://aws.amazon.com/athena/pricing/) and
  [Batch pricing model](https://aws.amazon.com/batch/faqs/).
