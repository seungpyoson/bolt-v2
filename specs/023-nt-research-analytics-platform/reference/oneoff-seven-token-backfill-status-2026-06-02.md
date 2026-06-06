# One-Off Seven-Token Backfill Status - 2026-06-02

Scope: standalone one-off backfill, separate from the broader backtesting-engine work.

Approved base tickers: BTC, ETH, SOL, XRP, DOGE, HYPE, BNB.

Script names in this status are historical provenance for the one-off operator run only. This handoff does not add or bless those scripts as supported repo tooling.

Denomination rule: filter by base ticker only; retain every source-proven quote, settlement, and market variant as separate instruments. USDT, USDC, USD, inverse or coin-margined, and venue-specific spot quotes are not collapsed. The raw backfill stores denomination as source instrument identity; USD-normalized views are derived analytics only.

Hash rule: when both `Manifest SHA256 field` and `Local manifest file SHA256` appear, the field is the manifest's embedded content hash recorded by the operator run, while the local file SHA256 is the checksum of the final manifest file after the embedded hash field was written. Differences are expected for those self-referential manifests; both values are retained for audit.

Local retention rule: raw payloads must be uploaded to S3 and not retained on local disk. Local files are allowed only as transient hash/schema/upload buffers and as small manifests/source proofs. `scripts/backfill_bybit_to_s3.py` was patched on 2026-06-02 to delete transient payload files immediately after S3 upload and record `local_retention=deleted_after_s3_upload`.

Local `/private/tmp` paths below are historical audit breadcrumbs only, not durable artifact URIs.

Local cleanup evidence: on 2026-06-02, old local raw/progress caches were removed from `/private/tmp/bolt-v2-polymarket-backfill-full-v1/raw`, `/private/tmp/bolt-v2-bybit-backfill-target-full-3m/uploaded-payloads`, and `/private/tmp/bolt-v2-hyperliquid-core-3m-full-seven/manifests/progress`. Their roots now retain only small manifest or metadata files, and `/private/tmp` had 69 GiB free after cleanup.

Window used by current runs: 2026-03-01 through 2026-06-01 inclusive for date-partitioned archives; end timestamp 2026-06-02T00:00:00Z where an exclusive timestamp end is required.

## Accepted

Current accepted manifest-backed seven-token venue coverage as of 2026-06-02 16:06 KST: 28,681 payload/staged objects and 131,741,424,234 bytes, about 131.74 GB decimal or 122.69 GiB. This excludes Polymarket (PMXT source), unmanifested in-flight Bybit REST and OKX uploads, and excludes the completed-but-not-accepted OKX 2026-04-01 target manifest below because it contains a non-base-scoped `ALL_SWAP` payload selector.

Current accepted manifest-backed Polymarket (PMXT source) coverage: 748 hourly parquet objects and 286,821,012,302 bytes, about 286.82 GB decimal or 267.12 GiB. S3 physically contains 914 objects and 344,758,628,885 bytes under `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/`; the difference is not counted as accepted until covered by completed manifests.

Combined accepted manifest-backed staged coverage including Polymarket (PMXT source): 418,562,436,536 bytes, about 418.56 GB decimal or 389.82 GiB.

### Binance

- Status: accepted for this one-off scope.
- Local manifest: `/private/tmp/bolt-v2-binance-backfill-3m-final-fast-artifacts/ingest-manifests/v1/run=binance-backfill-run-d928f6666827dd47/binance-backfill-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/binance/manifests/v1/run=binance-backfill-run-d928f6666827dd47/binance-backfill-manifest.json`
- Manifest SHA256: `b37d01f30932c8af4d8b9bc112b031569afbdf3a2db3756ec1f2a79626880c36`
- Payload objects: 4,701 planned, 4,701 completed.
- Payload bytes: 42,358,207,176.
- S3 objects including payloads, checksums, exchangeInfo, universe, and manifest: 9,406.
- S3 bytes including payloads, checksums, exchangeInfo, universe, and manifest: 42,376,582,788.
- Errors: none reported.
- Base-scope proof: outside approved base payload count is 0.
- Coverage artifact: `/private/tmp/bolt-v2-binance-backfill-3m-final-fast-artifacts/binance-symbol-coverage.tsv`
- Coverage artifact SHA256: `9370eef28a8189dea879c04278c5ac33e50cb24813aabf4a5d72801ad3d83f99`
- Denomination evidence from coverage artifact includes spot quotes plus USDT, USDC, USD, and coin-settled variants; examples include BTC/USDT settled USDT, BTC/USDC settled USDC, BTC/USD settled BTC, ETH/BTC, SOL/ETH, XRP/RLUSD, and HYPE/USDT.

### OKX - 2026-03-01 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260301/manifests/v1/run=okx-3m-d812548c6c5871b5/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-d812548c6c5871b5/okx-raw-staging-manifest.json`
- Manifest SHA256: `853d8c41c48c7493af5406a7400f073b9433b8869a1eda3767b7e6213db715cf`
- Payload objects: 200.
- Payload bytes: 6,279,257,571.
- Source proof objects: 242.
- Source proof bytes: 5,762,506.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.
- Example retained denominations: USD, USDC, USDT, EUR, TRY, BRL, AUD, AED, BTC.

### OKX - 2026-03-02 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260302/manifests/v1/run=okx-3m-77ca318f9c535649/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-77ca318f9c535649/okx-raw-staging-manifest.json`
- Manifest SHA256: `7d5993de12994f7b03b6e9a2f74c5d3628c3eca43e2368caf2f118ca0f118021`
- Payload objects: 200.
- Payload bytes: 8,117,171,456.
- Source proof objects: 242.
- Source proof bytes: 5,773,548.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.

### OKX - 2026-03-03 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260303/manifests/v1/run=okx-3m-a804f7a97436670c/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-a804f7a97436670c/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `ac85276112f13f7f5b2a76dc5426a1d0ba36b2207493b0830f42d680ff00d825`
- Local manifest file SHA256: `ced6545fc9eece8d5ecedf0f37a213cef15afef83076e55915453b27f5ee7473`
- Payload objects: 200.
- Payload bytes: 8,149,787,000.
- Source proof objects: 242.
- Source proof bytes: 5,776,289.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.

### OKX - 2026-03-04 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260304-retry1/manifests/v1/run=okx-3m-788bb7f0641ca706/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-788bb7f0641ca706/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `0fa3427e89bafc35bc64955468d180b7d90fe6b040d04687556817d645150d5a`
- Local manifest file SHA256: `90d98305be9d426608e1f1936d552f1ebe4bbf7d6dd59578d2102aad57541b7f`
- Payload objects: 200.
- Payload bytes: 8,373,848,046.
- Source proof objects: 242.
- Source proof bytes: 5,776,286.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.
- Note: first 2026-03-04 run hit OKX HTTP 429 during download-link resolution and was not accepted; this accepted artifact is the retry run.

### OKX - 2026-03-05 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260305/manifests/v1/run=okx-3m-318e14d8bfc7b032/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-318e14d8bfc7b032/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `904ff5b3b4885fd9487dd2872e96e98a3bc457623885475fab4f26f6bb9fd49b`
- Local manifest file SHA256: `9944f8666373c05957ff81f812880b5773f262137d764317fccaea00df817699`
- Payload objects: 200.
- Payload bytes: 7,023,787,870.
- Source proof objects: 242.
- Source proof bytes: 5,776,274.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.

### OKX - 2026-03-06 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260306/manifests/v1/run=okx-3m-952d463ae2029885/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-952d463ae2029885/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `1c9a4a7be357c87951f08d7dc702afea1a983b1a9aa34a4d69a0500b45b06941`
- Local manifest file SHA256: `3cbd1c0896745e1ed463f9f9ba17322006a14f5e5b5b9a244a45d90ead0a0ca1`
- Payload objects: 200.
- Payload bytes: 6,126,589,011.
- Source proof objects: 242.
- Source proof bytes: 5,776,229.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.

### OKX - 2026-03-07 Daily Tranche

- Status: accepted as one strict daily tranche only; not full three-month OKX completion.
- Local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260307/manifests/v1/run=okx-3m-48432a1d79ff622e/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-48432a1d79ff622e/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `4032fbc65c3fafb888e380e0ea6d9ab091c8e5dd04d65af53a9b4ba082a4ef43`
- Local manifest file SHA256: `891c1629299dbe27c9b03b2849a3d16f5ae9915089c3b03fd4853ecfe1314fa4`
- Payload objects: 200.
- Payload bytes: 3,095,066,423.
- Source proof objects: 242.
- Source proof bytes: 5,781,600.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.

### OKX - 2026-03-08 Through 2026-03-11 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 800.
- Aggregate payload bytes: 23,293,686,593.
- Aggregate source proof objects: 968.
- Aggregate source proof bytes: 23,279,148.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, order_book_400 across spot, swap, futures, option where source links existed.
- 2026-03-08 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260308/manifests/v1/run=okx-3m-bb3be815440d1ad9/okx-raw-staging-manifest.json`
- 2026-03-08 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-bb3be815440d1ad9/okx-raw-staging-manifest.json`
- 2026-03-08 manifest SHA256: `c3484a4b61d96bdab3a20f34babd491fb567f419597be405f0c376cc64e5da6d`; payload objects 200; payload bytes 4,329,667,441.
- 2026-03-09 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260309/manifests/v1/run=okx-3m-787cc68a41c8e47c/okx-raw-staging-manifest.json`
- 2026-03-09 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-787cc68a41c8e47c/okx-raw-staging-manifest.json`
- 2026-03-09 manifest SHA256: `e0e9412db6bd33080be9ac1ef52c51d51b9f77ac97f67163924a438aad4fd52f`; payload objects 200; payload bytes 6,728,454,817.
- 2026-03-10 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260310/manifests/v1/run=okx-3m-ff6a57f35585ac70/okx-raw-staging-manifest.json`
- 2026-03-10 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-ff6a57f35585ac70/okx-raw-staging-manifest.json`
- 2026-03-10 manifest SHA256: `629fbd6041a68668a18d17babd02186f1290e24c03df034faae2a4440c3ad1e7`; payload objects 200; payload bytes 6,561,087,619.
- 2026-03-11 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260311/manifests/v1/run=okx-3m-f030e1ee411cd48e/okx-raw-staging-manifest.json`
- 2026-03-11 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-f030e1ee411cd48e/okx-raw-staging-manifest.json`
- 2026-03-11 manifest SHA256: `751c42bc4147ddb8105808535e0d8f1d1dab53a2e7c5e8d1b27324a6e501889d`; payload objects 200; payload bytes 5,674,476,716.

### OKX - 2026-03-12 Through 2026-03-15 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 596.
- Aggregate payload bytes: 1,519,914,140.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,157,488.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-03-12 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260312/manifests/v1/run=okx-3m-47b0e2a49c937de9/okx-raw-staging-manifest.json`
- 2026-03-12 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-47b0e2a49c937de9/okx-raw-staging-manifest.json`
- 2026-03-12 manifest SHA256: `9b939619124fd9cdb90fb20b6989eba890e246c543a20ea6f5c5e5e721218edb`; payload objects 146; payload bytes 393,318,527.
- 2026-03-13 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260313/manifests/v1/run=okx-3m-121d7e6be0a73df5/okx-raw-staging-manifest.json`
- 2026-03-13 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-121d7e6be0a73df5/okx-raw-staging-manifest.json`
- 2026-03-13 manifest SHA256: `4ead77b804e0337f3e599ec3666b05ec0c4fa1055af0bf1bd8a60dea27b39200`; payload objects 143; payload bytes 383,062,879.
- 2026-03-14 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260314/manifests/v1/run=okx-3m-2b62540690bb928d/okx-raw-staging-manifest.json`
- 2026-03-14 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-2b62540690bb928d/okx-raw-staging-manifest.json`
- 2026-03-14 manifest SHA256: `28088eca11c5ab09a123a626031511a3eaeff721492943bb727add2e85a771d2`; payload objects 154; payload bytes 370,028,317.
- 2026-03-15 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260315/manifests/v1/run=okx-3m-15763342c39485f3/okx-raw-staging-manifest.json`
- 2026-03-15 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-15763342c39485f3/okx-raw-staging-manifest.json`
- 2026-03-15 manifest SHA256: `fd97d47e180fc890c1ba6ea329cd29d508143a2eaa5000ec6557eb0a28d1c4c8`; payload objects 153; payload bytes 373,504,417.

### Bybit Archive-Only Tick Trades

- Status: accepted for archive tick-trade tranche with instrument-universe source evidence; Bybit REST/delivery/historical-volatility is still active separately.
- Aggregate archive manifests: 4.
- Aggregate payload objects excluding manifests: 319.
- Aggregate payload bytes excluding manifests: 305,822,707.
- Aggregate S3 objects including manifests: 323.
- Aggregate S3 bytes including manifests: 306,337,890.
- Errors: none reported.
- Families: instrument_universe, tick_trades.
- 2026-03-01 through 2026-03-13 local manifest: `/private/tmp/bolt-v2-bybit-backfill-archive-20260301-20260313/ingest-manifests/v1/run=bybit-backfill-run-fdcc0758bbd03113/bybit-backfill-manifest.json`
- 2026-03-01 through 2026-03-13 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/run=bybit-backfill-run-fdcc0758bbd03113/bybit-backfill-manifest.json`; payload objects 49; payload bytes 70,920,988.
- 2026-03-14 through 2026-03-31 local manifest: `/private/tmp/bolt-v2-bybit-backfill-archive-20260314-20260331/ingest-manifests/v1/run=bybit-backfill-run-03b1b64f603be705/bybit-backfill-manifest.json`
- 2026-03-14 through 2026-03-31 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/run=bybit-backfill-run-03b1b64f603be705/bybit-backfill-manifest.json`; payload objects 64; payload bytes 70,993,486.
- 2026-04-01 through 2026-04-30 local manifest: `/private/tmp/bolt-v2-bybit-backfill-archive-20260401-20260430/ingest-manifests/v1/run=bybit-backfill-run-86a4c2cd08b485f1/bybit-backfill-manifest.json`
- 2026-04-01 through 2026-04-30 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/run=bybit-backfill-run-86a4c2cd08b485f1/bybit-backfill-manifest.json`; payload objects 100; payload bytes 87,230,065.
- 2026-05-01 through 2026-06-01 local manifest: `/private/tmp/bolt-v2-bybit-backfill-archive-20260501-20260601/ingest-manifests/v1/run=bybit-backfill-run-e282ae46e9dec2ba/bybit-backfill-manifest.json`
- 2026-05-01 through 2026-06-01 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/run=bybit-backfill-run-e282ae46e9dec2ba/bybit-backfill-manifest.json`; payload objects 106; payload bytes 76,678,168.

### OKX - 2026-03-16 Through 2026-03-19 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 587.
- Aggregate payload bytes: 1,698,077,429.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,160,600.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-03-16 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-d2d4cfeb0a837dce/okx-raw-staging-manifest.json`; manifest SHA256 `3385b44695e45ba82f84e33b4ee34de1e8c882416bec4eb3ab6c5f199d54d62c`; payload objects 146; payload bytes 439,965,044.
- 2026-03-17 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-b82bc721b902fc20/okx-raw-staging-manifest.json`; manifest SHA256 `42a9285dde308821ac72807a38bdfd1be0b70976a81b5f9980ece48a12e3d14f`; payload objects 147; payload bytes 411,289,878.
- 2026-03-18 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-1bb84cc94f638ecd/okx-raw-staging-manifest.json`; manifest SHA256 `43920ad538342ae2e3ce694ac4c66abaa29464552822709beb82fda96281fee4`; payload objects 149; payload bytes 465,569,235.
- 2026-03-19 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-3b22537b4f2eac0c/okx-raw-staging-manifest.json`; manifest SHA256 `4ca381e4dcac1adb125ee80b191170363597751130c46cb57d6ddc732f8c6209`; payload objects 145; payload bytes 381,253,272.

### OKX - 2026-03-20 Through 2026-03-23 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 589.
- Aggregate payload bytes: 1,436,923,344.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,159,768.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-03-20 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-3b747550777cdfdb/okx-raw-staging-manifest.json`; manifest SHA256 `fe2b0ae2780c720809931fc3dae5de926ef25003986d5e9d604ae8dd4161bcdf`; payload objects 145; payload bytes 376,856,363.
- 2026-03-21 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-0aac2b05d2ac1456/okx-raw-staging-manifest.json`; manifest SHA256 `b8a4649d9b71c90a842b7f8d2d07f7621c8978865402637e5f804c65ad42db28`; payload objects 156; payload bytes 366,838,976.
- 2026-03-22 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-d2ff417fa79358d4/okx-raw-staging-manifest.json`; manifest SHA256 `1783a2d9064346704530719be68a5bae66da87465c46cd17a57b42d4a7434031`; payload objects 149; payload bytes 376,895,110.
- 2026-03-23 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-64748ac9d0b41dc9/okx-raw-staging-manifest.json`; manifest SHA256 `125488d6144ce0e561c46b2aaa62dbd11c2d81d4b5a09a3f298b7084d65ecd27`; payload objects 139; payload bytes 316,332,895.

### OKX - 2026-03-24 Through 2026-03-27 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 591.
- Aggregate payload bytes: 1,577,253,569.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,186,866.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-03-24 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-cb2adaa2665e6b85/okx-raw-staging-manifest.json`; manifest SHA256 `519884eed35bbf955b73ae4eab21a7dc2fb1b4b3d1219064ee667486953a6e03`; payload objects 145; payload bytes 391,532,146.
- 2026-03-25 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-6b02f7a55ab46258/okx-raw-staging-manifest.json`; manifest SHA256 `560363a6691c02cec025adfaaf1517b59998d8599688c5192de158aa1220cab4`; payload objects 148; payload bytes 347,345,541.
- 2026-03-26 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-29ce938fa8691193/okx-raw-staging-manifest.json`; manifest SHA256 `b92b60fc8bea19b967acdb537d3e1859033bbc512b617d1f83b21cc36adde90c`; payload objects 149; payload bytes 409,323,668.
- 2026-03-27 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-6700d18512c3c0c5/okx-raw-staging-manifest.json`; manifest SHA256 `17e87c247f343f25828985308c9494f9c1ef2d840dc4dccffbb27de58b30c7a4`; payload objects 149; payload bytes 429,052,214.

### OKX - 2026-03-28 Through 2026-03-31 Daily Tranches

- Status: accepted as four strict daily tranches only; not full three-month OKX completion.
- Aggregate payload objects: 606.
- Aggregate payload bytes: 1,600,761,503.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,230,974.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-03-28 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-ac6194cec35c535d/okx-raw-staging-manifest.json`; manifest SHA256 `81e6cca7a3ac4c81bc2724df779711dd33fc7c8eba647da830ab03c3a3b13001`; payload objects 154; payload bytes 366,105,452.
- 2026-03-29 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-5789f8f7f44243cf/okx-raw-staging-manifest.json`; manifest SHA256 `3a20b3dd984589517cc6c640e39d2aee04c7ad10872a83e8dea7e1f10bbf7347`; payload objects 154; payload bytes 388,038,580.
- 2026-03-30 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-2e363f2a46518159/okx-raw-staging-manifest.json`; manifest SHA256 `668a565633e2169f0e552f2ae683c2ac4e260d87cfe289509e1d79d6c9feadfc`; payload objects 150; payload bytes 425,187,342.
- 2026-03-31 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-5ce56d7a56b6d226/okx-raw-staging-manifest.json`; manifest SHA256 `5abb7ccb64c18175909dc1c1ef301e60aa2d05e2a6ca3c164124216ed3644044`; payload objects 148; payload bytes 421,430,129.

### OKX - 2026-04-01 Through 2026-04-04 Daily Tranches

- Status: accepted for this one-off scope.
- Aggregate payload objects: 656.
- Aggregate payload bytes: 1,560,711,346.
- Aggregate source proof objects: 872.
- Aggregate source proof bytes: 23,272,950.
- Errors: none reported.
- Selector-scope violations: 0.
- Payload selector bases: BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Families: trades, candlesticks, and source-available order_book_400 across spot, swap, futures, option.
- 2026-04-01 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260401/manifests/v1/run=okx-3m-3d45cca7b21fcbb3/okx-raw-staging-manifest.json`
- 2026-04-01 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-3d45cca7b21fcbb3/okx-raw-staging-manifest.json`; manifest SHA256 `065af8beb258fdc00b8b5648559c209df053d1b6e43f47e44a8fd69f9a4ac45a`; payload objects 165; payload bytes 454,295,387.
- 2026-04-02 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260402/manifests/v1/run=okx-3m-a86f246d8176eed3/okx-raw-staging-manifest.json`
- 2026-04-02 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-a86f246d8176eed3/okx-raw-staging-manifest.json`; payload objects 158; payload bytes 400,506,348.
- 2026-04-03 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260403/manifests/v1/run=okx-3m-6572316ed2cfaf02/okx-raw-staging-manifest.json`
- 2026-04-03 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-6572316ed2cfaf02/okx-raw-staging-manifest.json`; payload objects 167; payload bytes 416,303,570.
- 2026-04-04 local manifest: `/private/tmp/bolt-v2-okx-strict-seven-20260404/manifests/v1/run=okx-3m-4bd77adb8d317c3c/okx-raw-staging-manifest.json`
- 2026-04-04 S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-4bd77adb8d317c3c/okx-raw-staging-manifest.json`; manifest SHA256 `be6f530b4a1213e2df56c908cf678928ea2027828c21a4d615d2af549fd8faa9`; payload objects 166; payload bytes 289,606,041.

### Polymarket (PMXT Source)

- Status: accepted as manifest-backed PMXT hourly parquet staging for Polymarket; not complete three-month Polymarket coverage.
- Source binding: `polymarket-parquet-archive-index`.
- User-facing venue name: Polymarket (PMXT source). Do not report PMXT as a separate venue.
- Source provenance: preserve `pmxt` and `PMXT` in URLs, source proof names, source bindings, and raw lineage fields where they identify upstream origin.
- S3 prefix: `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/`.
- Manifest-backed completed objects: 748.
- Manifest-backed completed bytes: 286,821,012,302.
- Manifest-backed errors: 0.
- Current S3 physical objects: 914.
- Current S3 physical bytes: 344,758,628,885.
- Coverage basis: 748 accepted objects out of 1,148 planned objects across local PMXT S3 streaming manifests, or 65.16% of the manifest-backed attempted plan. Against the 2026-03-01 through 2026-06-01 93-day hourly target, 748 accepted hourly parquet objects is 33.51% calendar-hour coverage.

### Deribit

- Status: uploaded with gaps; accepted as partial evidence only, not complete three-month market-data coverage.
- Local manifest: `/private/tmp/bolt-v2-deribit-3m-allbases-20260602T0500Z/ingest-manifests/v1/run=deribit-3m-35afe0a04aa50c41/deribit-backfill-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/deribit/ingest-manifests/v1/run=deribit-3m-35afe0a04aa50c41/deribit-backfill-manifest.json`
- Manifest field SHA256: `75d5bcc1d432931a7595f338bc76c5e0929e1203cca4fe0d95883e3daf0b26fb`
- Local and downloaded S3 manifest SHA256: `c02dcb1de2cddadbce6d27d43ff42286fa976a71df5c676d60a35e9045ad7d86`
- Raw objects: 7,544.
- Raw bytes: 15,346,229.
- Manifest bytes: 13,788,244.
- Total objects including manifest: 7,545.
- Total bytes including manifest: 29,134,473.
- Source-proven bases: BNB, BTC, ETH, SOL, XRP.
- Unproven requested bases: DOGE, HYPE.
- Selected instruments: spot 10, perpetual 2, future 26, option 1,668.
- Uploaded families: bars_1m, delivery, funding_history, historical_volatility, index, index_price_names, instrument_metadata, instrument_universe, mark_price_history_probe, settlements, trades, trades_recent_probe, volatility_index.
- Gaps: 1,118 errors reported, mostly HTTP 429; bars/trades/volatility are bounded probes rather than full three-month market-data sweeps.

### Hyperliquid HIP-3

- Status: accepted as completed targeted HIP-3 tranche.
- Local manifest: `/private/tmp/bolt-v2-hyperliquid-hip3-backfill-target-3m/manifests/v1/run=run-20260601T165241Z-9f211af74fde/manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip3/manifests/v1/run=run-20260601T165241Z-9f211af74fde/manifest.json`
- Manifest hash: `59b8a5dcf3949a4c2cbd96da2c2008de0874ea4ee3a61ef5b21e5d1b4845d982`
- Uploaded objects excluding manifest: 25.
- Uploaded bytes excluding manifest: 2,184,326.
- Errors: 0.

### Hyperliquid HIP-4

- Status: accepted as completed HIP-4 prediction-market tranche, but not mapped to the seven crypto bases.
- Local manifest: `/private/tmp/bolt-v2-hyperliquid-hip4-backfill-3m-retry-slow/manifests/v1/run=run-20260601T164023Z-3973110148dc/manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip4/manifests/v1/run=run-20260601T164023Z-3973110148dc/manifest.json`
- Manifest hash: `412c7f05b922a35446b4044c8c11ae6deb9b10b8e52482013984650c528d8548`
- Uploaded objects excluding manifest: 87.
- Uploaded bytes excluding manifest: 44,592,796.
- Errors: 0.

### Hyperliquid Core

- Status: accepted with source-availability gaps; no script errors.
- Local manifest: `/private/tmp/bolt-v2-hyperliquid-core-3m-full-20260601-base-tickers-v2/manifests/hyperliquid-core-565ad06dcfdb4144.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-core/manifests/v1/run=hyperliquid-core-565ad06dcfdb4144/hyperliquid-core-backfill-manifest.json`
- Manifest SHA256 field: `48ac86f475b9828260bf5ba7c746bdb1f3307ead82060d9c4638cc45cd65fbf2`
- Local and S3 manifest file SHA256: `772c9e3fa278234171e249c8c44dde22280001c3862a1f48f5bb94296d83b20e`
- Completed objects: 10,180.
- Completed bytes: 9,162,435,699.
- Errors: 0.
- Gaps: 799 source-availability gaps.
- Families:
  - asset_ctxs: 61 objects, 38,535,655 bytes.
  - fundingHistory: 35 objects, 1,394,763 bytes.
  - l2Book: 10,080 objects, 9,122,490,657 bytes.
  - meta: 1 object.
  - metaAndAssetCtxs: 1 object.
  - spotMeta: 1 object.
  - spotMetaAndAssetCtxs: 1 object.
- Perp coverage: BTC, ETH, SOL, BNB, DOGE, XRP, HYPE as perpetual markets; all source-marked not delisted.
- Spot coverage from spotMeta filter: HYPE/USDC, HYPE/USDT0, HYPE/USDH, HYPE/USDE.
- l2Book coverage: 1,440 hourly objects each for BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- fundingHistory coverage: 5 pages and 2,208 rows each for BNB, BTC, DOGE, ETH, HYPE, SOL, XRP.
- Source gaps: asset_ctxs upstream missing 2026-05-01 through 2026-05-31; l2Book source date prefix not listed for 2026-04-30 through 2026-05-31.

## Active

### Bybit

- Status: archive-only tick-trade chunks accepted; REST, delivery, and historical-volatility pass remains active.
- Duplicate-worker finding: the previous four Bybit archive chunks also ran the full three-month REST tranche independently. Those workers were stopped on 2026-06-02, and `scripts/backfill_bybit_to_s3.py` was patched with explicit stage skip flags so REST is no longer duplicated across archive chunks.
- Active scratch roots:
  - REST, delivery, historical-volatility pass with archive skipped: `/private/tmp/bolt-v2-bybit-backfill-rest-20260301-20260601`.
- 2026-06-02 15:55 KST audit: no final manifest exists under `/private/tmp/bolt-v2-bybit-backfill-rest-20260301-20260601`.
- S3 manifest audit under `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/ingest-manifests/v1/` found target/REST-like manifests such as `run=bybit-backfill-run-d7698a37f210ee6b`, `run=bybit-backfill-run-138015d0e411ace0`, and `run=bybit-backfill-run-faffecb840525fb2`; each reports `Full one-year all-symbol REST pagination remains after this first tranche`, so no Bybit REST manifest is accepted.
- Current evidence before disk block: 5,615 local uploaded CSV payload mirrors existed across Bybit chunk roots; those mirrors were deleted locally after the retention patch. Final acceptance still requires a completed REST manifest with no remaining REST pagination work.
- Local duplicate monolithic run `/private/tmp/bolt-v2-bybit-backfill-target-full-3m` ended without a final manifest and is not used as accepted evidence.

### OKX - Active April Tranches

- Status: strict-seven daily tranches remain active after accepted 2026-04-01 through 2026-04-04.
- 2026-04-01 through 2026-04-04 scratch roots: `/private/tmp/bolt-v2-okx-strict-seven-20260401` through `/private/tmp/bolt-v2-okx-strict-seven-20260404`; accepted manifests listed above.
- 2026-04-05 through 2026-04-12 scratch roots: `/private/tmp/bolt-v2-okx-strict-seven-20260405` through `/private/tmp/bolt-v2-okx-strict-seven-20260412`.
- 2026-04-13 and 2026-04-14 family-split retry roots: `/private/tmp/bolt-v2-okx-strict-seven-20260413-family-*-fixed` and `/private/tmp/bolt-v2-okx-strict-seven-20260414-family-*-fixed`.
- 2026-06-02 16:06 KST local audit: accepted strict-seven manifests exist for 2026-04-01 through 2026-04-04. April 5-14 remain unaccepted until completed manifests prove zero errors and zero selector-scope violations.
- Completed but not accepted target manifest: `/private/tmp/bolt-v2-okx-target-20260401/manifests/v1/run=okx-3m-8e300a494d2bd6e1/okx-raw-staging-manifest.json`
- S3 manifest: `s3://bolt-parquet/backfill-staging/2026-06-01/okx/manifests/v1/run=okx-3m-8e300a494d2bd6e1/okx-raw-staging-manifest.json`
- Manifest SHA256 field: `1b8cba97f790205fa487c7e7a58af6a1b6ea531dec5037c9d872d2b39aa7b4d9`
- Local and downloaded S3 manifest file SHA256: `c1af1b622b98c338e81ada6d918e6b03aef17a22e1f169c1d2f5aa51597b3f0c`
- Payload objects: 13.
- Payload bytes: 536,770,725.
- Source proof objects: 24.
- Source proof bytes: 5,499,229.
- Errors: none reported.
- Selector violation: contains `selector=ALL_SWAP` as a payload record for OKX funding rates; this is not base-ticker-filtered evidence, so this completed target manifest is not added to accepted totals.
- Payload selectors in the target manifest: `ALL_SWAP`, `BNB-EUR`, `BNB-USDT`, `BTC-USD`.
- Script retry behavior: `scripts/backfill_okx_to_s3.py` now retries HTTP 429 and retryable 5xx responses for source-link resolution and payload download.

### Deribit Retry Audit

- 2026-06-02 15:55 KST audit: no completed retry manifest supersedes the accepted partial Deribit manifest.
- S3 retry candidates under `s3://bolt-parquet/backfill-staging/2026-06-01/deribit/ingest-manifests/v1/` include `run=deribit-3m-a408f74764b315f9` with 9 errors, `run=deribit-3m-ba13a4ea11ab6776` with 0 errors but only 66 objects and known gaps, `run=deribit-3m-5927a839831487ec` with 23 errors, and the already accepted partial `run=deribit-3m-35afe0a04aa50c41` with 1,118 errors.
- Deribit remains partial; do not treat it as complete without a new retry manifest proving complete three-month market-data coverage.

## Still Open

- OKX full three-month daily coverage after accepted 2026-03-01 through 2026-03-31 tranches.
- Bybit chunk manifests and REST completion.
- Deribit 429 retry or slower completion pass if complete Deribit market-data coverage is required beyond the partial artifact.
- Polymarket (PMXT source) manifest recovery or rerun for PMXT S3 objects that are physically present but not yet covered by completed manifests, plus continuation toward full 2026-03-01 through 2026-06-01 hourly coverage.
