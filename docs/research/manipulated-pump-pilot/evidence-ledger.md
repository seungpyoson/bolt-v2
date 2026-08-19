# MYX provider-admission pilot evidence ledger

**As of:** 2026-08-19
**Manifest:** `docs/research/manipulated-pump-pilot/manifest.toml`
**Current verdict:** Surf Data API is discovery-only, Surf Chat automation is
rejected for this workflow, CoinAPI is rejected for canonical L2, and Tardis
remains unpurchased and unadmitted

**Evidence class:** non-admission observational ledger. Hash-only or ephemeral
artifacts are insufficient for independent replay and cannot admit a provider.

## Findings that changed the proposed design

1. **Bitget is not a valid common-window venue.** Bitget officially delisted
   MYXUSDT futures on 2025-06-13 and relaunched it on 2025-09-10. Its absence on
   the August and 2025-09-08 windows is an instrument-lifecycle fact, not missing
   Tardis bytes. Tardis's symbol metadata exposes one continuous `availableSince`
   range and therefore is insufficient by itself for exact interval coverage.
2. **Gate.io Futures replaces Bitget as the third candidate.** Its public metadata
   spans all six dates, Gate's listing notice predates them, and it had the highest
   2025-09-01 normalized trade notional among the tested alternatives. Exact-day
   files still must confirm uninterrupted coverage. The selected pilot set is
   Binance Futures, Bybit, and Gate.io Futures.
3. **The reviewed feeds provided no target-venue L3.** The selected derivatives
   feeds and provider candidates supplied L2 market-by-price data, not
   market-by-order evidence. Exact queue replay is unsupported by this record;
   backtests must use explicit queue/fill bounds.
4. **Surf Onchain SQL is materially better than Surf's typed transfer endpoint.**
   `agent.bsc_transfers` supplies `tx_index`, `evt_index`, `amount_raw`, and
   transaction/block identity fields. The typed endpoint omits those fields and
   is suitable only as a locator.
5. **Surf passed the technical identity sample, not source admission.** All 100
   stratified rows matched one BNB Chain receipt log and its canonical block
   header with zero failures. Commercial-use, retention, completeness, and
   historical label semantics remain unresolved, so Surf still fails closed as
   canonical evidence.
6. **Surf labels fail historical as-of use.** In a 100-transfer enriched sample,
   only three distinct addresses had labels. The label objects contained current
   names/types and confidence, but no provider source, observed-at, effective-time,
   or revision fields. They remain post-event annotations only.
7. **Tardis's public licence partially passes, not fully.** Clause 9.1 permits
   Permitted Use, creation of Derived Data, and storage of Data and Manipulated
   Data on the customer system. Clause 13.6 does not clearly grant continued use
   of a standalone raw replay archive after the invoice-defined term expires.
8. **At the public-only stage, CoinAPI was the cost challenger, not an
   equivalent-data winner.** Its
   public S3 product is pay as you go and offers full order books and trades.
   Exact MYX files are discoverable only with an API key, while funding and open
   interest are advertised through minute Metrics OHLCV rather than proved for
   the selected symbols and dates. The public page exposed a ten-row Bybit
   perpetual preview; its documented 100-row download URL did not yield CSV
   through the routed fetch, so no CoinAPI file entered the replay test.
9. **Before the user added $5, the CoinAPI account had no usable quota.** The
   canonical registry loaded `COINAPI_API_KEY` without exposing it. CoinAPI's REST
   symbol endpoint and Flat Files S3 endpoint both returned HTTP 403 with
   `Insufficient Usage Credits or Subscription`, organization quota $0, and
   current usage $0. No catalog rows or provider files were returned.
10. **A $5 CoinAPI pilot rejects it as canonical replay evidence.** After credits
    were added, REST metadata identified all three target perpetuals and S3
    returned the exact Binance 2025-08-29 incident object. The 39,363,049-byte
    gzip contained 4,784,711 normalized L2 rows, but no sequence field,
    disconnect marker, raw exchange payload, or non-empty order ID. It cannot
    prove whether an incident-window absence is an exchange gap, collector gap,
    or normalization artifact.
11. **The Surf key and typed Data API path work.** An authenticated BTC price
    request returned HTTP 200, 720 hourly points, and an explicit one-credit charge.
    This proves the configured credential and typed request path, not universal
    dataset authority.
12. **Fable 5 is not proved selectable through the API.** A minimal request naming
    `fable-5` completed, but the response reported `model = "surf-2.0"`.
    Internal aliasing or execution remains unknown. The public Chat API also
    documents only `surf-2.0` and `surf-2.0-instant`.
13. **Surf Chat failed the long-running research probe.** The non-streaming exact
    prompt returned no bytes before a 1,200-second timeout. The evidence-constrained
    streaming run reached `in_progress`, then ended with an HTTP/2 internal error
    after approximately 1,200 seconds and no output deltas. Retrieval by response
    ID returned 404. The balance endpoint returned 401, so failed-call charges
    remain unreconciled. No more Chat credit probes are justified without a
    documented completion, retrieval, cancellation, and billing contract.
14. **The pilot selection is exploratory, not provider-neutral.** Its incident
    control came from a Tardis disclosure and its third-venue rule used Tardis
    catalog coverage. That is useful for challenging Tardis, but it lets the
    candidate shape its own test. Any admission run must reselect venues and
    incidents from official or independent sources before inspecting candidates.

## CEX public-sample evidence

The Tardis first-of-month samples are sufficient to test file integrity, schema,
rough byte volume, and NT conversion. They are not substitutes for the six fixed
non-sample dates.

- Binance Futures: 339,104 trades (3,461,051 bytes) and 5,150,702 L2 rows
  (34,461,085 compressed bytes).
- Bybit: 189,075 trades (5,602,203 bytes) and 12,999,693 L2 rows
  (62,511,922 compressed bytes).
- Gate.io Futures: 13,176 trades (185,535 bytes) and 11,668,600 L2 rows
  (41,632,785 compressed bytes).
- Bitget Futures: zero rows in both 20-byte gzip files, correctly reflecting its
  official delisting interval.

Tardis discloses a Binance exchange-side data outage on 2025-08-29 from
06:18:03.205 through 06:37:04.820 UTC. This is the fixed incident control. Its
CSV format does not include disconnect markers; those require raw replay access.

The nine selected core sample files total 150,931,278 compressed bytes for one
quiet day. The event days cannot be extrapolated safely from that figure because
message volume rose sharply during the pump.

## Published pilot cost

Tardis's public one-off formula is $2 per selected instrument-day. Three venues
times six dates is 18 instrument-days, or $36 before the published $300 minimum.
The actual Tardis purchase floor for this pilot is therefore **$300**, before tax.
No checkout was initiated.

CoinAPI now has a verifiable public rate card: pay-as-you-go access has no
commitment; the first 1 GB of full order-book transfer per billing day and SKU is
$8/GB, and the first 0.5 GB of trades is $24/GB. Minute Metrics OHLCV starts at
$8/GB. These rates indicate a much lower entry cost, but they are not an exact
pilot quote because the API-key-gated catalog has not supplied MYX object sizes
or proved funding/open-interest coverage. Cross-provider compressed byte sizes
were not used as a false exact estimate.

After $5 in credits was added, the registered key returned MYX metadata and S3
catalog results. CoinAPI's documented S3 path with a slash after the bucket name
returned a false-empty result; standard path-style `/coinapi?prefix=...` worked.
Date-root listings were complete but non-recursive, requiring an additional
request per exchange directory. Using the 0.25-credit API-call assumption in
CoinAPI's published Flat Files use case, the full 3-venue × 6-date × 2-dataset
drill-down would require 9 credits before file transfer. No dated credit-to-USD
parity was recorded, so those credits are not presented as dollars. Discovery
cost therefore matters even though byte-transfer prices are low.

The exact paid incident object was
`T-LIMITBOOK_FULL/D-20250829/E-BINANCEFTS/IDDI-39868900+SC-BINANCEFTS_PERP_MYX_USDT+S-MYXUSDT.csv.gz`.
Its compressed SHA-256 is
`45d82dd070136f9ba05fe5c04815252839f9246f409447b6fc0b524162866fc4`.
The stream contained 4,774,621 `SET`, 5,572 `SNAPSHOT`, 4,287 `SUB`, and
231 `ADD` rows; zero order IDs; one exchange-time reversal; two CoinAPI-time
reversals; and no consecutive duplicate rows. The temporary file was closed and
discarded after streaming verification.

## NT conversion determinism

An isolated Rust probe used Bolt's exact pinned NautilusTrader revision `e4167fd`.
It loaded trades, L2 deltas, and funding updates for each selected venue, capped at
100,000 NT-native events per stream, serialized the resulting typed events, and
hashed them. A second clean invocation produced the same count and SHA-256 for all
nine streams. This passes the public-sample conversion smoke test. It is not the
full acceptance replay: exact six-window files, uncapped conversion, engine
features, and decision traces remain unavailable.

## On-chain identity result

- Contract: `0xd82544bf0dfe8385ef8fa34d67e6e4940cc63e16`
- Source table: Surf `agent.bsc_transfers`
- Sampling: deterministic ranks across all six dates, 100 rows total
- Transactions: 84
- Receipt/log matches: 100
- Identity failures or ambiguities: 0
- Canonical amount: `amount_raw`; Surf's floating `amount` and USD fields are
  derived values and are not accepted as exact quantities
- Surf credits observed: 5 for the SQL query

The verification checked contract address, Transfer topic, sender, recipient,
raw amount, transaction hash, block number/hash, transaction index, event index,
receipt success, and block timestamp against the official BNB Chain public RPC.

Surf reported 16,611; 11,781; 8,012; 9,059; 511,484; and 145,744 transfers
on the six respective dates. Those counts are frozen but not yet accepted as
complete: checking 100 receipts proves row identity, not population recall. A
second full-population indexed source or archival export is still required.

The separate `include=labels` transfer call used one credit. Seven sender rows and
eight recipient rows were labeled, representing three distinct addresses. Removing
the additive label fields produced the same canonicalized transfer-row SHA-256 as
the unenriched response, so enrichment did not mutate the underlying rows.

## Surf API capability result

- Typed Data API control: HTTP 200; 720 BTC hourly price points; one credit.
- Requested Chat alias: `fable-5`; returned model: `surf-2.0`.
- Exact-prompt `xhigh` control: client timeout after 1,200 seconds; no response
  body.
- Evidence-contract `xhigh` stream: in-progress response created; no output
  deltas or completion; server-side HTTP/2 stream failure after approximately
  1,200 seconds.
- Response retrieval: 404. Credit-balance request: 401.
- Decision: typed responses remain eligible for dataset-specific admission;
  automated Surf Chat research is rejected until the missing operational and
  billing contracts are proved.

## Open mandatory gates

- Post-term raw replay rights for the selected CEX source, plus Surf commercial
  use and retention rights
- Exact six-day CEX files, raw disconnect messages, and window-level gap metrics
- Two identical NT runs over those exact accepted files
- A historical label source with effective-time/revision evidence; Surf's current
  enrichment failed this gate and remains interpretation-only
- A raw/native CEX source with sequence, disconnect, and exact-window evidence;
  the paid CoinAPI normalized L2 sample failed this gate
- Retained or durably retrievable source objects plus locators, retrieval times,
  schema/parser versions, and derivation lineage. The current hash-only artifacts
  cannot be independently replayed and therefore cannot support admission.

Vendor contact, quote requests, a Tardis purchase, and durable retention of
provider rows were deliberately not performed. The only paid challenger action
was the user-authorized $5 CoinAPI pilot; Surf probes used the account's existing
credits. The manifest now caps incremental spend and paid queries at zero. The
legal gates remain open.
