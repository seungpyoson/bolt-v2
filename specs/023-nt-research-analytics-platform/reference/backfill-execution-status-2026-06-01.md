# Backfill Execution Status: 2026-06-01

This status records verified local/S3 staging work for the current backfill
effort. These are noncanonical staging writes; canonical source-proof acceptance
and normalized table writes remain gated by the table contract.

## Source Proof Metadata

- Local manifest:
  `/private/tmp/bolt-v2-backfill-source-proof-v3/ingest-manifests/v1/run=source-proof-run-52baed4dc4242a51/manifest.json`
- S3 staging prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/source-proof-v3/`
- Manifest hash:
  `eadb48dfe4223dbf86bfadec3f8b1ba1c82f09957e65b1c8e3630d749a7e7732`
- Verified contents:
  40 raw source payload records, 21 source-proof records, 21 universe records,
  zero errors, and no raw/source-proof hash mismatches.
- Polymarket PMXT v2 index coverage found:
  998 Parquet objects, estimated `366051300000` source bytes, paginated across
  20 index pages.

## Polymarket Staged Parquet Data

Local page-1 staging:

- Local manifest:
  `/private/tmp/bolt-v2-polymarket-backfill-full-v1/ingest-manifests/v1/run=archive-objects-run-f6be9ae08fd93d9b/archive-objects-manifest.json`
- S3 staging prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-page1/`
- Manifest hash:
  `a9c34c20c7f010c8715928230b9c3b95743684f4dc34f53e9857ad4d4d81ecd9`
- Verified payload:
  50 Parquet objects, `20212785634` bytes, zero errors, and 50/50 local hash
  checks passed before S3 sync.
- Time/object range:
  `polymarket_orderbook_2026-05-23T07.parquet` through
  `polymarket_orderbook_2026-05-25T08.parquet`.
- S3 verification:
  51 staged S3 objects including the manifest, `20212835400` bytes.

Streaming S3 staging beyond page 1:

- S3 staging prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/`
- Run `archive-s3-run-2675882860bcfeaf`:
  5 Parquet objects, `1407778839` bytes, zero errors, manifest hash
  `fa84ad0ddbdb38b5e87227eb57af445101613cfe3964f0842c8ccf043729d121`.
- Run `archive-s3-run-39cad7f89c7db020`:
  20 Parquet objects, `4405904780` bytes, zero errors, manifest hash
  `545a34f2128ce6112fb84ecd5b2b2b2e667f4b326a3d7864199f331c1722097f`.
- Run `archive-s3-run-7feffd86ca35ffff`:
  50 Parquet objects, `16869413138` bytes, zero errors, manifest hash
  `4d55d5f4e9875cbe356971962c47b8872c20609271a4aae4a5fe8c39528c7f3e`.
- Run `archive-s3-run-2c470ea9090205db`:
  50 Parquet objects, `17834226203` bytes, zero errors, manifest hash
  `2bd3c0cdc618cd5e1bfd848e7b190c5ea792fd69e3db0d3845ce763c231413f5`.
- Run `archive-s3-run-32c483e5edc2c6b1`:
  50 Parquet objects, `19788634626` bytes, zero errors, manifest hash
  `a678f83c365f9c04c4744764a90ba79ed79a74379c636cc6840976e54b58b38e`.
- Run `archive-s3-run-be5d1cefbcbf0ef4`:
  50 Parquet objects, `20923752588` bytes, zero errors, manifest hash
  `9f22c84b9d924f0445cd611ed153f19ebb458385c250902fb224c20002481d1c`.
- Run `archive-s3-run-c455db41a8224f43`:
  50 Parquet objects, `21145420404` bytes, zero errors, manifest hash
  `b72ae68da6c466ae5256425190354e8b90ce84a3324f6d99b8a7140dfd8de10c`.
- Run `archive-s3-run-8e194a2ecd56d4e4`:
  50 Parquet objects, `19905365522` bytes, zero errors, manifest hash
  `fbdb16d0c241d2e98f0de9ba9d93fe16efa47d6becaca57617959fc753d1b169`.
- Run `archive-s3-run-455664443cf7a297`:
  50 Parquet objects, `18965712444` bytes, zero errors, manifest hash
  `cba5ba9b8beaea501899eb115005e51da2a1d8880362567a17f6aa126bad387f`.
- Run `archive-s3-run-00a4deb49a46a973`:
  50 Parquet objects, `18242556552` bytes, zero errors, manifest hash
  `c4d577e541fa3715c235c15d257f8556f203ee35d7ff9ddfafee1625c21f7deb`.
- Run `archive-s3-run-246834ce55ede953`:
  50 Parquet objects, `19726198714` bytes, zero errors, manifest hash
  `e989b23b0ba05ad548b6cf7f0dd75ac07ce1d09b9bd0ee47c9a8f8378adb1bdb`.
- Run `archive-s3-run-b208c36ee6e228a3`:
  50 Parquet objects, `20054979061` bytes, zero errors, manifest hash
  `25da2bee309db40d28df1ca3cbaf6ffbaf62b646aaf7aa37fb54c97da4ee9fa6`.
- S3 verification:
  537 staged S3 objects including twelve manifests, `199270483056` bytes.

Combined staged Polymarket Parquet payload:

- 575 Parquet objects.
- `219483217390` raw Parquet bytes.
- Progress against the discovered PMXT v2 archive:
  `575/998` Parquet objects (`57.6%`) and about `60.0%` of the estimated source
  bytes.
- The full PMXT v2 archive is not yet staged; remaining PMXT v2 objects require
  continued S3 streaming because local disk cannot hold the 366 GB full index.

## Binance Staged Archive Data

- Run prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/binance/`
- Main manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/binance/manifests/v1/run=binance-backfill-run-ab972ed49f03f1b0/binance-backfill-manifest.json`
- Selected-prefix remaining manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/binance/manifests/v1/run=binance-backfill-run-ab972ed49f03f1b0/binance-selected-prefix-remaining.json`
- Universe manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/binance/universes/v1/run=binance-backfill-run-ab972ed49f03f1b0/binance-universe.json`
- Manifest hashes:
  main `b4f63cf9f0b3e0dd778f26312c8f90f196ffcf8fe9fe754434622fb18d3d57a7`;
  remaining `d828a69925def492276146577fb6c6cc703c7ac13160334a20c5da86a4e190a4`.
- S3 verification:
  44 staged S3 objects, `1013516760` bytes.
- Verified payload:
  19 raw Data Vision ZIP objects, `995515432` bytes; 19 official `.CHECKSUM`
  objects, 3 exchangeInfo JSON objects, 2 manifests, and 1 universe manifest.
- Families staged:
  spot `trades`, `aggTrades`, and `klines.1m`; USD-M and COIN-M futures
  `trades`, `aggTrades`, `klines.1m`, `markPriceKlines.1m`,
  `indexPriceKlines.1m`, `premiumIndexKlines.1m`, `fundingRate`, and `metrics`.
- Coverage:
  deterministic first tranche from official archive listings: spot `0GBNB`
  `2025-09`, USD-M `0GUSDT` `2025-09`, and COIN-M `AAVEUSD_PERP` `2025-06`.
- Explicit nonclaims:
  full one-year all-universe completion, Binance options, historical L2/book
  deltas, liquidations, and unmapped separate long/short ratio families.

## OKX Staged Archive Data

- Run prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/okx/`
- Clean manifests:
  `okx-raw-5144aea47d3c021a`
  (`b02fd49969e510653ebde04d466a32c02648754e86239f679c530a9e92ab365d`),
  `okx-raw-32f8b68c5b230314`
  (`5a16ce2d97c84cc37966787a7c61197f87b08f8dc203f92322107ed5ab929e19`),
  `okx-raw-0c6a4034b903ca97`
  (`21ea67c131c90a5d87ec27c9f2ceac4fa92d962e784420de59f51535c902f272`),
  and `okx-raw-f99d37c92b726137`
  (`00db52a7fcbc508515b702e81ab10f5644efd1c0aeea60902179571f62889e39`).
- Clean manifest-bound verification:
  77 staged S3 objects including manifests, `369953123` bytes.
- Verified payload:
  13 raw payload objects, `348118809` bytes; 60 source-proof objects,
  `21648181` bytes.
- Shared-prefix note:
  the full noncanonical OKX prefix currently lists 187 objects and
  `378652523` bytes because an earlier preliminary run remains in staging.
  Accepted counts are bound to the four clean manifests above.
- Families staged:
  trades, 1m candlesticks, swap funding rates, and 400-level L2 order-book
  files.
- Coverage:
  deterministic `2025-06-01` daily tranche for spot `1INCH-EUR`, swap
  `1INCH-USDT`, futures `BTC-USD`, option `BTC-USD`, and `ALL_SWAP` funding.
- Instrument universe:
  official endpoints returned 1262 spot, 355 swap, 46 futures, BTC/ETH option
  underlyings, and 2150 option instrument ids.
- Explicit nonclaims:
  full one-year all-universe completion, 5000-level L2 full-year coverage,
  delisted/window-expired instruments beyond current public endpoint output,
  canonical roots, normalized tables, and NT catalog writes.

## Bybit Staged Archive/REST Data

- Run id:
  `bybit-backfill-run-695742b4e003d335`
- Manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/run=bybit-backfill-run-695742b4e003d335/bybit-backfill-manifest.json`
- Manifest hash:
  `ff16a04c14b08b3e6fe72ae12027782dc6c345cab2d7975416a4b31031b0d71e`
- Final-run S3 verification:
  30 staged S3 objects including the manifest, `3561835` bytes.
- Verified payload:
  29 payload objects, `3511723` payload bytes.
- Shared-prefix note:
  `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/` currently has 61
  objects and `7181754` bytes because earlier Bybit attempt manifests/payloads
  remain in the noncanonical staging prefix. The accepted count above is bound
  to run `bybit-backfill-run-695742b4e003d335`.
- Universe generated through official pagination:
  611 spot instruments, 677 linear instruments, 26 inverse instruments, and
  1388 option instruments.
- Families staged:
  `instrument_universe`, `tick_trades`, `kline_1m`, `mark_price_kline_1m`,
  `index_price_kline_1m`, `premium_index_price_kline_1m`, `funding_rate`,
  `open_interest_1d`, `delivery_price`, and `historical_volatility`.
- Coverage:
  deterministic first tranche: spot `1INCHUSDT`, linear
  `1000000BABYDOGEUSDT`, inverse `AAVEUSD`, and BTC/ETH/SOL option volatility
  surfaces where official REST returned rows.
- Explicit nonclaims:
  full one-year all-symbol staging, historical L2 deltas, liquidations, option
  trade archives, and expired/delisted historical universe coverage.

## Deribit Staged REST Data

- Run prefix:
  `s3://bolt-parquet/backfill-staging/2026-06-01/deribit/run_id=deribit-rest-20260601T1236Z/`
- Manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/deribit/run_id=deribit-rest-20260601T1236Z/manifests/deribit-s3-manifest-cfe6245747a8880956d31ff96cbc1344fac78fe03cbef1af01c4b939c4e8d92a.json`
- Manifest hash:
  `cfe6245747a8880956d31ff96cbc1344fac78fe03cbef1af01c4b939c4e8d92a`
- S3 verification:
  34 staged S3 objects including the manifest, `395210` bytes.
- Verified payload:
  33 REST payload objects, `347381` payload bytes.
- Families staged:
  `instrument_universe`, `instrument_metadata`, `trades`, `bars_1m`,
  `funding_history`, `index_chart`, `delivery_prices`, `settlements`,
  `historical_volatility`, `volatility_index`, and `mark_price_history_probe`.
- Coverage:
  deterministic first tranche only: start-day spot/future instruments from
  `2025-06-01T00:00:00Z` to `2025-06-02T00:00:00Z`, plus option-day probes from
  `2026-05-31T00:00:00Z` to `2026-06-01T00:00:00Z`.
- Explicit nonclaims:
  historical books, book deltas, open-interest series, per-strike Greeks, and
  complete one-year expired instrument universe.

## Hyperliquid HIP-3 Staged Data

- Run id:
  `run-20260601T123750Z-239ea134b9f0`
- Manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip3/manifests/v1/run=run-20260601T123750Z-239ea134b9f0/manifest.json`
- Manifest hash:
  `ee92eb69c1191062774bd9c951fc865201ddaf5342d8412df03b3a1352e52681`
- Successful-run S3 verification:
  202 staged S3 objects, `15704266` bytes.
- Shared-prefix note:
  the full noncanonical HIP-3 prefix currently lists 340 objects and
  `25940899` bytes because an earlier partial run remains in staging. Accepted
  counts are bound to run `run-20260601T123750Z-239ea134b9f0`.
- Families staged:
  `info.perpDexs`, `info.allPerpMetas`, `info.metaAndAssetCtxs`, and
  `info.fundingHistory`.
- Verified payload:
  196 raw payloads, `2651281` raw bytes, plus staged dex universe, instrument
  universe, funding rows, source proof, and funding coverage files.
- Coverage:
  8 dex records, 186 HIP-3 instrument records, 186 funding requests, and 26467
  staged funding rows. Requested window is `2025-06-01T00:00:00Z` through
  `2026-06-01T00:00:00Z`; actual funding tranche is
  `2026-05-25T00:00:00Z` through `2026-06-01T00:00:00Z`.
- Explicit nonclaims:
  one-year HIP-3 level-two replay, trades, order-book deltas, all-fills archive
  parsing, dex-qualified archive coverage, and listing-age proof from empty
  funding responses.

## Hyperliquid Core Staged Data

- Run id:
  `hyperliquid-core-590bd835423f490b`
- Manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-core/manifests/v1/run=hyperliquid-core-590bd835423f490b/hyperliquid-core-backfill-manifest.json`
- Manifest hash:
  `8ff1405c5e1c395c8f1c69958cafada9adf3d164d4e1e3eb826c0b54db8ebe20`
- S3 verification:
  222 staged S3 objects including the manifest, `99467537` bytes.
- Verified raw payload:
  221 payload objects, `99218749` bytes.
- Families staged:
  `l2Book`, `asset_ctxs`, `fundingHistory`, `meta`, and `metaAndAssetCtxs`.
- Coverage:
  deterministic first tranche for `2025-06-01` hour `0`: all 198 listed
  official `l2Book` coins for that hour, `asset_ctxs/20250601.csv.lz4`, current
  official meta, current `metaAndAssetCtxs`, and one-day funding pages for the
  first 20 active official-meta coins.
- Universe:
  official `meta` returned 230 instruments: 179 active and 51 delisted.
- Explicit nonclaims:
  native L2 deltas, full one-year all-hour coverage, node fills/trades schema
  acceptance, and fresh-to-2026-06-01 archive coverage. The run observed archive
  lag: latest checked `market_data` prefixes ended at `20260429` and latest
  checked `asset_ctxs` object was `asset_ctxs/20260430.csv.lz4`.

## Hyperliquid HIP-4 Staged Data

- Run id:
  `run-20260601T124449Z-93e6ce55b6be`
- Manifest:
  `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip4/manifests/v1/run=run-20260601T124449Z-93e6ce55b6be/manifest.json`
- Manifest hash:
  `5bb08687f7fad418075eb942d6939ef1ad66af55e592a3e4acaea409670d4f4c`
- S3 verification:
  88 staged S3 objects, `1660769` bytes.
- Taxonomy:
  `prediction_market_outcome`, not perpetual.
- Families staged:
  `info.outcomeMeta`, `info.l2Book`, `info.recentTrades`, and
  `info.candleSnapshot`.
- Verified payload:
  79 raw payloads, `240769` raw bytes.
- Staged rows:
  13 events, 26 outcome sides, 2 questions, 26 L2 snapshots, 196 recent trades,
  and 1232 bounded bars.
- Explicit nonclaims:
  historical outcome metadata coverage, downstream `quoteToken` parser
  fidelity, one-year outcome L2 replay, one-year outcome trade replay,
  official archive outcome filename coverage, order-book deltas, and settlement
  history.

## Evidence Updates Applied

- Binance: official public archives cover spot trades/aggregated trades/bars and
  futures trades/aggregated trades/bars/mark/index/premium/funding/metrics, but
  do not prove official one-year L2 replay.
- OKX: official historical downloads cover trades, 1m candles, funding, and
  400-level L2 snapshots/updates; 5000-level L2 is partial from 2025-11-01 and
  must not support full one-year claims.
- Bybit: public archives cover tick trades where exact paths/schema are proven;
  L2 deltas and complete liquidation history remain unproven.
- Deribit: official REST covers trades/bars/funding/settlements and some option
  history, but not one-year historical books, book deltas, open-interest series,
  or per-strike Greeks.
- Hyperliquid: core perpetual S3 has archive-lagged L2 snapshots, daily asset
  contexts/open interest, and node fills requiring parsing; spot, HIP-3, and
  HIP-4 are mostly live/recent or parse-required and do not currently prove
  one-year L2 replay.

## Active Limits

- No canonical S3 write has been claimed.
- No source proof has been accepted for canonical NT catalog input.
- Polymarket PMXT v2 license, schema, replay semantics, and coverage still
  require acceptance before normalized table conversion.
- Local disk is too constrained for the complete PMXT v2 archive; S3 streaming
  is the required path for the remaining archive objects.
