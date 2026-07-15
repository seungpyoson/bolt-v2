# Backfill Table Contract

`contract_version`: `backfill-table-contract.v1`

This contract defines canonical table families, identity columns, and row/schema
contracts for historical data backfills. It also preserves the original
evidence-state and venue/product investigation as historical context. It is not
permission to acquire data or write canonical S3 objects.

Authority boundary: this contract remains the canonical table-family, row,
identity-column, and schema vocabulary. It no longer authorizes acquisition
sources or governs venue/product coverage. `Required Evidence State`, `Product
Families To Enumerate`, `Initial Evidence Matrix`, and `Venue Notes` below are
historical, non-authorizing investigation context. Source selection, exhaustive
coverage state, and implementation ownership are governed by
`historical-data-acquisition-architecture.v1.md`. Until its exhaustive registry
lands, every Binance product/table-family cell is `excluded_by_policy` with the
reason `owner choice: Binance is intentionally excluded despite breadth loss`;
nothing in this contract may override that policy.

Canonical writes remain blocked until an `artifact_root`, source proof records,
sample schema checks, and write manifest format are approved.

## Required Evidence State

Every venue/product/table combination must be assigned one evidence state in the
source proof and ingest manifest:

- `directly_backfillable`: official archive/API can backfill the requested
  window at the required granularity.
- `owner_archive_backfillable`: user-provided or venue-download archive can
  backfill after portable raw-payload storage, checksum, schema, license, and
  completeness checks.
- `bounded_or_current_only`: official API exists, but retention, pagination,
  or semantics do not cover the requested window. This state cannot satisfy a
  one-year historical backfill claim.
- `pending_source_proof`: source is plausible, but schema, coverage, license,
  or availability has not been proved.
- `vendor_or_forward_capture_only`: official history is missing or too limited;
  use a licensed vendor or start live capture from the approval date.
- `not_applicable`: product does not expose the field. This state requires a
  written justification in the source proof.
- `excluded_from_current_scope`: product exists but is intentionally excluded
  until a separate source-proof decision includes it.

No worker may silently replace a missing granular table with a weaker aggregate.
Candles do not satisfy trades or books, best bid/ask quotes do not satisfy
full-depth books, fixed-depth books do not satisfy full-depth books, aggregated
trades do not satisfy native trades, and derived snapshot deltas do not satisfy
native order-book deltas.

## Common Identity And Type Contract

Normalized tables use these shared columns unless a source proof explicitly
documents why a column is unavailable:

| Column | Type | Rule |
| --- | --- | --- |
| `schema_version` | string | Contracted semantic schema version, for example `market_data.v1`. |
| `ingest_run_id` | string | Stable run id generated before acquisition starts. |
| `source_binding` | string | Config-selected source key; no concrete venue branching in code. |
| `venue` | string | Canonical venue key, for example `binance`, `okx`, `hyperliquid`. |
| `product_family` | string | Venue/product partition from this contract. |
| `product_category` | string | Cross-venue category such as `spot`, `perpetual`, `future`, `option`, or `prediction_market_outcome`. |
| `instrument_id` | string | Venue-native instrument id, unique only within `(venue, product_family)`. |
| `canonical_instrument_key` | string | Stable join key: `<venue>/<product_family>/<instrument_id>`. |
| `venue_symbol` | string | Display or wire symbol from the source. |
| `nt_instrument_id` | string, nullable | NautilusTrader instrument id when mapped. |
| `event_time` | int64 | Exchange/source event timestamp in Unix nanoseconds. Do not use REST response time here. |
| `capture_time` | int64 | Worker receipt/capture timestamp in Unix nanoseconds. |
| `availability_time` | int64, nullable | Time data became available to the source, when distinct from `event_time`. |
| `source_sequence` | string, nullable | Native sequence, update id, block id, or page cursor. |
| `raw_payload_id` | string | Pointer to the raw payload record consumed. |
| `source_proof_id` | string | Accepted or pending source proof governing the row. |
| `payload_hash` | string | Lowercase SHA-256 hex over canonical raw bytes or source manifest. |
| `transform_hash` | string | Lowercase SHA-256 hex over transform code/config identity. |

Numeric prices, sizes, quantities, funding rates, interest rates, and Greeks are
stored as decimal strings in normalized tables unless the downstream table
format has a stronger typed-decimal representation. Raw provider string fields
must be preserved exactly in `raw_payloads` before conversion.

Instrument metadata must preserve `base_asset`, `quote_asset`, `settle_asset`,
`contract_type`, `expiry_time`, `strike_price`, `option_type`, `listing_time`,
`delisting_time`, and venue-specific product identifiers when available. For
Hyperliquid HIP-3, include `dex_name`. For HIP-4, include `outcome_encoding`,
`asset_id`, `wire_symbol`, and the raw `quoteToken`. For prediction markets,
include event, outcome, side, and resolution identifiers.

`NT` means NautilusTrader throughout this document.

## Canonical Table Families

### Provenance

- `raw_payloads`: original archive files, REST responses, WebSocket frames,
  owner-provided downloads, checksum files, and source listings.
- `source_proofs`: source proof reports with license, schema, sample, time
  range, fidelity, forbidden claims, evidence state, and acceptance status.
- `ingest_manifests`: one manifest per run, with source keys, instruments, date
  windows, counts, checksums, gaps, parser version, and retry status.
- `instrument_universe_snapshots`: generated instrument universe for each
  venue/product/date window. It must include instruments active at any point in
  the requested window, not only instruments active on the execution date.

### Instruments

- `instruments`: one row per venue instrument version.
- `instrument_status`: trading status changes where provided.
- `instrument_closes`: expiries, settlement, delivery, delisting, and
  prediction-market resolution records where provided.

### Market Data

These tables are common across spot, perpetuals, futures, options, and
prediction-market outcome instruments.

- `trades`: native trade prints only when `trade_source_type=native`.
  Aggregated prints use `trade_source_type=aggregated` and cannot satisfy a
  native-trades requirement. Include native trade id, side/aggressor where
  available, price, size, and notional.
- `quotes`: best bid/ask from native BBO/ticker feeds or reconstructed
  top-of-book, with `quote_source_type=native_bbo`, `ticker_bbo`, or
  `reconstructed_top_of_book`.
- `order_book_deltas`: native L2/L3 updates only. Derived deltas are forbidden
  in this table.
- `order_book_snapshot_deltas`: explicit derived clear-and-rebuild or
  snapshot-difference deltas. This table requires source proof that names the
  derivation rule and cannot satisfy native `order_book_deltas`.
- `order_book_snapshots_full`: source-proven full or maximum-depth snapshots.
  A fixed-depth source may write here only when the source proof establishes
  that the captured depth is the source's maximum historical depth for that
  instrument/date/source family.
- `order_book_snapshots_fixed_depth`: fixed-depth snapshots such as top 20,
  400, 500, 1000, or 5000 levels when they are not proven full depth.
- `order_book_depth_10`: derived or native top-10 projection for NT-compatible
  replay and lightweight analysis.
- `bars`: provider or derived bars. Include `bar_source_type` with values such
  as `provider_supplied`, `derived_from_trades`, or `derived_from_books`, plus
  interval, price source, OHLC, volume, quote volume, trade count, and taker
  fields where present.

### Derivatives, Carry, And Risk State

- `mark_prices`
- `index_prices`
- `premium_index_prices`, where provided separately
- `funding_rates`
- `open_interest`
- `liquidations`, where historical force-order data is source-proven
- `long_short_ratios`, when explicitly selected as a signal table
- `taker_buy_sell_volume`, when explicitly selected as a signal table
- `borrow_lending_rates`, where spot margin, borrow, or lending rates are
  source-proven and selected

### Options

- `option_greeks`
- `implied_volatility`
- `historical_volatility`
- `forward_prices`
- `settlements`
- `delivery_prices`

`settlements` are event records. `delivery_prices` are delivery or settlement
price series. Venue-specific rows may populate both only when the source exposes
both event and price semantics.

### Prediction-Market Metadata

Prediction-market instruments use the common market-data tables for trades,
quotes, books, and bars. Prediction-specific tables are metadata and resolution
tables:

- `prediction_market_events`
- `prediction_market_outcomes`
- `prediction_market_settlements`
- `prediction_market_fee_models`

HIP-4 belongs in this taxonomy. It is a Hyperliquid
prediction-market/outcome-contract family, not a perpetual family. HIP-3 belongs
to the perpetual taxonomy with `product_family=hip3_perpetual`.

## Product Families To Enumerate

The instrument universe is generated, not manually curated. For a one-year
window, workers must include instruments active at any point in the window, not
only instruments active on the execution date.

- Binance: `spot`, `usd_m_perpetual`, `usd_m_delivery`,
  `coin_m_perpetual`, `coin_m_delivery`. Binance options are
  `excluded_from_current_scope` until a separate source-proof decision adds a
  canonical option product family.
- OKX: `spot`, `swap`, `future`, `option`. `swap` has
  `product_category=perpetual`. Options must be enumerated by underlying or
  instrument family, not by a global option request.
- Bybit: `spot`, `linear`, `inverse`, `option`, with pagination.
  `contract_type` determines whether `linear` or `inverse` instruments are
  perpetuals or delivery futures.
- Deribit: `spot`, `future`, `option`. Deribit perpetual instruments use
  `product_family=future` and `product_category=perpetual`.
- Deribit combos are `excluded_from_current_scope` until a separate combo
  source-proof decision defines combo identity columns.
- Hyperliquid: `core_perpetual`, `spot`, `hip3_perpetual`.
- Hyperliquid HIP-4: `prediction_market_outcome`.
- Polymarket: `prediction_market_outcome`.

## Initial Evidence Matrix

This grouped matrix is the starting contract. Each source proof and ingest
manifest must expand it into concrete `(venue, product_family, table_family)`
rows before any canonical write.

| Venue/product group | `directly_backfillable` | `owner_archive_backfillable` | `bounded_or_current_only` | `pending_source_proof` | `vendor_or_forward_capture_only` / exclusions |
| --- | --- | --- | --- | --- | --- |
| Binance spot | instruments, native trades, aggregated trades, provider bars | none | current books, current BBO, recent REST trades | none | historical spot book deltas, historical spot full-depth books, spot liquidations not applicable |
| Binance USD-M/COIN-M futures | instruments, native or aggregated trades where archive proves family, provider bars, funding rates, mark prices, index prices, premium index prices | none | REST current books, recent trades, one-month open-interest stats | `bookDepth` and `metrics` until sample schema proves mapping | true historical book deltas, complete historical liquidations; Binance options excluded |
| OKX spot/swap/future | instruments for currently listed instruments, current books, provider bars, mark/index where product exposes them | venue historical download files after portable raw storage and checksum proof | REST history trades and funding history when retention is shorter than requested window | open interest history, 5000-depth historical books until source/download proof | historical native deltas without archive proof |
| OKX option | current instruments by underlying/family, current option summary, current Greeks/IV | venue historical download files after proof | current or bounded REST option summaries | historical per-strike Greeks/IV/forward prices until archive proof | one-year option Greeks from REST alone |
| Bybit spot | instruments for currently listed instruments, provider bars | official archive trades after schema proof | current books, recent trades | order-book archive URL/schema | historical native deltas without archive proof |
| Bybit linear/inverse | instruments for currently listed instruments, provider bars, funding rates, open interest, mark/index/premium klines | official archive trades after schema proof | current books, recent trades | order-book archive URL/schema, liquidation archive if any | historical native deltas and complete liquidations without archive proof |
| Bybit option | instruments for currently listed instruments, historical volatility where endpoint covers requested window | archive trades/books after proof | live/current per-strike Greeks and IV | per-strike historical Greeks/IV beyond volatility endpoint | one-year per-strike Greeks from REST alone |
| Deribit spot | instruments where active/recent-expired coverage proves window, trades by time where rate/window proof passes, provider bars | none | current books, current quotes | complete one-year expired universe | historical books, historical book deltas, historical quotes |
| Deribit future/perpetual | instruments where active/recent-expired coverage proves window, trades by time where rate/window proof passes, provider bars, index prices, funding rates, settlements/deliveries | none | current books, current quotes, current open interest | complete one-year expired universe | historical books, historical book deltas, historical open interest series |
| Deribit option | instruments where active/recent-expired coverage proves window, trades by time where rate/window proof passes, provider bars, settlements/deliveries, limited mark history for supported volatility-index subsets | none | current books, current quotes, current Greeks/IV/open interest | complete one-year expired universe | one-year per-strike Greeks, full books, native deltas, open interest series |
| Hyperliquid core perpetual | instruments, paged funding history | official S3 L2 snapshots, daily asset contexts/open interest, and node fills after requester-pays listing, archive-lag, and schema proof | HTTP current books, recent candles, current contexts | node fills/trades dedupe, mark/index mapping, archive lag | native L2 deltas; historical BBO quotes unless archive proof exists |
| Hyperliquid spot | instruments | none until all-fills spot parsing is proved | HTTP current books, recent candles, current contexts | spot fills/table coverage | one-year spot L2 books, candles, and asset contexts where official S3 says unavailable |
| Hyperliquid HIP-3 perpetual | instruments per `dex_name`, funding history where API/source covers dex and listing age | none until dex-qualified all-fills parsing is proved | current dex fields, current `l2Book`, recent candles | dex-qualified archive coverage, fills parse, market listing age | one-year HIP-3 L2 replay until a dex-qualified archive proof exists |
| Hyperliquid HIP-4 outcome | none for one-year history until proof exists | none | current `outcomeMeta`, current `l2Book`, current/recent trades, bounded candles | outcome history, quote-token parser fidelity, fills parse | one-year outcome L2/trade replay until archive proof and quote-token mapping pass |
| Polymarket outcome | events, outcomes, native trades where source proves history, provider bars where source proves semantics | public Polymarket Parquet archive candidate after license/schema/coverage proof | current API books/trades where historical coverage is absent | native deltas vs snapshots per archive schema | using snapshots to satisfy native deltas; fee/sizing code vendoring |

## Venue Notes

### Binance

Use official public archives as the primary one-year source where available.
REST is metadata, recent-gap fill, and current snapshot support. Fixed-depth
`bookDepth` archives map to `order_book_snapshots_fixed_depth` unless sample
proof establishes full or maximum-depth semantics.

Pinned NT supports spot and USD-M/COIN-M futures. Binance options are excluded
from current scope until a separate source proof selects product family,
instrument universe, table mapping, and NT or non-NT acquisition path.

### OKX

Use OKX historical downloads for one-year L2/trade/funding where REST retention
is too short. Owner-provided local sample files are raw evidence candidates
only; they must first be copied into portable `raw_payloads` with checksums and
schema samples before any source proof may cite them.

Open interest and 5000-depth full-book history need a non-NT acquisition route
or an NT extension because the pinned NT data-client path does not cover every
historical OKX surface.

### Bybit

Use official archives for historical trades and verified order-book archives.
Use REST for instruments, klines, funding, open interest, mark/index/premium
klines, and historical volatility only where endpoint retention covers the
requested window. Per-strike historical option Greeks remain
`vendor_or_forward_capture_only` unless an archive proves them.

### Deribit

Use official REST for the granular historical surfaces that actually exist.
Do not claim full historical L2, historical quotes, historical open interest,
or historical per-strike Greeks from official public REST. A complete one-year
expired instrument universe requires proof beyond current active instruments.

### Hyperliquid

Use official S3 archives where coverage is proven. HTTP alone is not a complete
one-year market replay source. Core perpetual S3 L2 is snapshot/archive data,
not native order-book deltas, and the archive has lag that must appear in source
proofs. HIP-3 uses the same perpetual schema with `dex_name` and dex-qualified
instruments, but current dex metadata and live `l2Book` do not prove historical
dex-qualified L2 coverage.

### Hyperliquid HIP-4

HIP-4 is a prediction-market outcome-contract source. Current `outcomeMeta`,
current `l2Book`, bounded candles, and recent trades do not prove one-year
history. Historical archive coverage for `#<encoding>` outcome coins must be
checked with authenticated requester-pays listings before any L2 replay claim.

No canonical HIP-4 normalized write may proceed until the raw `quoteToken` is
preserved and a parser-fidelity proof shows correct `quoteToken` to `quote_asset`
mapping for every emitted row.

### Polymarket

Use public Polymarket Parquet archive candidates or official-compatible
API/source files as raw evidence and convert to prediction-market metadata plus
the common market-data tables. Snapshots and deltas are separate evidence
claims: source-proven snapshots may populate snapshot tables, but they do not
satisfy native `order_book_deltas`.

Preserve string-encoded price/size fields exactly in raw payloads before
Decimal conversion. Add fee-model and binary sizing formulas as independent
facts in `prediction_market_fee_models`; do not vendor LGPL code.

## Approval Gate Before Canonical Upload

Before writing canonical S3 data, produce and approve:

- `artifact_root` URI and top-level prefixes.
- Source proof report per venue/product/table family, including all
  `not_applicable`, `excluded_from_current_scope`, and
  `bounded_or_current_only` justifications.
- One portable sample raw payload and checksum per source family.
- Parser schema sample with row counts and timestamp range.
- Instrument-universe manifest for the requested date window.
- Expanded evidence matrix with one row per `(venue, product_family,
  table_family)`.
- Gap policy with maximum tolerated gap frequency, gap duration, and explicit
  forbidden claims for missing historical books, quotes, open interest, and
  Greeks.
- HIP-4 quote-token parser-fidelity proof before any HIP-4 normalized write.
- Idempotent write manifest, create-only behavior, and no-overwrite behavior.
