# Normalization / Transform / Catalog Plan (v1) — for adversarial review

`plan_version`: `normalization-catalog-plan.v1`
`status`: DRAFT — produced by an ultracode design workflow (8 venue mappers + 2 proof analyses + synthesis + internal critique), pending external adversarial review.

## Purpose

Turn the one-off seven-token raw S3 backfill (audit input) into a **research-ready store** the
project's specs already mandate: a NautilusTrader `ParquetDataCatalog` that NT's
`BacktestNode`/`BacktestEngine` can replay and read-only Jupyter notebooks / Research-Analytics
can consume. Raw provider payloads are **audit input, not replay input** (evidence E-002,
SOURCE_PROVEN). This document is the build plan for the raw→catalog projection layer; it does NOT
itself authorize canonical writes (those remain gated, see "Contract gate handling").

This doc is the artifact under adversarial review. Reviewers: find where the schema mapping,
fidelity rules, proof-gate, sequencing, taxonomy, or write-discipline is wrong; confirm or refute
the internal critique below; and surface anything the internal critique missed.

## Verified-at-pinned-rev facts (re-checked by the main session, not just the subagent)

NT rev `6e059dcbb59ac1e582132fc431a581936c216c3c`, crate `nautilus-persistence`:

- The catalog's replayable data classes are a **fixed `CatalogPathPrefix` set** (verified in
  `crates/persistence/src/backend/catalog.rs:4109-4123`): `QuoteTick→quotes`, `TradeTick→trades`,
  `OrderBookDelta→order_book_deltas`, `OrderBookDepth10→order_book_depths`, `Bar→bars`,
  `IndexPriceUpdate→index_prices`, `MarkPriceUpdate→mark_prices`,
  `FundingRateUpdate→funding_rate_update`, `InstrumentStatus→instrument_status`,
  `InstrumentClose→instrument_closes`, `InstrumentAny→instruments`, `AccountState→account_state`,
  plus order lifecycle types. **No NT class** exists for `open_interest`,
  `premium_index_prices`, `long_short_ratios`, `taker_buy_sell_volume`, `historical_volatility`,
  `settlements`, `delivery_prices`, `option_greeks`, `implied_volatility`,
  `order_book_snapshots_fixed_depth`, `order_book_snapshots_full`, or any `prediction_market_*`
  table.
- `nautilus-persistence` has a `cloud` feature (object_store aws/azure/gcp/http);
  `ParquetDataCatalog::from_uri` takes `storage_options`; `write_to_parquet`/`query`/`query_files`
  exist; `object_store` 0.13.2 + `datafusion` 53.1.0 are in the tree. The catalog writer uses
  `object_store` directly (verified `catalog.rs:109,160,439-450,510`).

## Catalog I/O decision

Use **Rust (`nautilus-persistence`)** as the canonical engine for the raw→catalog projection that
feeds backtests; allow **Python** (`nautilus_trader` 1.228.0 + fsspec/s3fs + V2 wranglers) only as
optional research-side convenience that writes through the SAME catalog format.

Rationale: (1) **Credential discipline** — bolt-v2 mandates SSM-only secrets via `aws-sdk-ssm`
(rule 6); Rust resolves S3 creds in-process and injects them into `from_uri` `storage_options`,
whereas the Python path needs an s3fs/fsspec credential surface that risks an env-var/AWS-CLI
fallback violating rule 6. (2) **Single build path** — Rust projection shares one
`nautilus-persistence`/`datafusion` version with the downstream `BacktestNode`, removing a
catalog-format drift surface. (3) `object_store` 0.13.2 is already in the tree transitively; the
only change is enabling the existing `cloud` feature on a **research-only** crate, adding zero new
runtime to the live binary. The pure-Rust rule binds the LIVE binary only, not research tooling
(E-025).

## Contract gate handling

The approval gate (`backfill-table-contract.md` lines 292-309) is respected by NEVER flipping the
ingest-manifest `write_mode` to `canonical_s3` and never writing under canonical `artifact_root`
prefixes until ALL gate items are approved: (a) artifact_root URI + prefix schema; (b) a
`SourceProofReport` per `(venue, product_family, table_family)` with all `required_checks` PASS;
(c) one portable sample raw payload + checksum per source family; (d) parser schema sample with
row counts + timestamp range; (e) instrument-universe manifest covering instruments active at ANY
point in the window; (f) the expanded one-row-per-`(venue,product_family,table_family)` evidence
matrix; (g) gap policy with max gap frequency/duration + forbidden_claims; (h) HIP-4 quoteToken
parser-fidelity proof before any HIP-4 normalized write; (i) idempotent/create-only/no-overwrite
write-manifest format.

**Interim research-ready-in-staging** (does NOT violate the gate): write normalized tables + an NT
catalog under the existing NON-canonical staging prefix using `write_mode=local_staging` and
`commit_state=staged`. Every staged artifact carries `source_proof_id`=pending, records its
`fidelity_class`, and attaches `forbidden_claims` (snapshots≠native deltas, bars≠trades,
aggTrades≠native trades, fixed-depth≠full-depth) so no notebook/backtest over-claims fidelity.
Staged writes still carry full Common Identity lineage and create-only discipline but are NEVER
promoted into canonical prefixes until proofs are accepted. Promotion is a deferred, explicit step.

## Proof-first sequence

1. **STEP 1 (FIRST, gating):** NT-catalog-on-S3 capability proof on a research-only crate at rev
   `6e059dc` with `nautilus-persistence` feature `cloud`. Negative control first (`s3://` `from_uri`
   WITHOUT cloud feature must hit the "Cloud storage support requires the cloud feature" bail), then
   positive: `from_uri(s3://.../nt-catalog, storage_options)` → `write_to_parquet` for two
   instruments (one binary-option fixture, one perps/spot fixture) → re-open → `query_files` →
   query both back, assert row counts/contents match. Closes E-037/BTE-007; on failure fall back to
   local-catalog-write-then-s3-sync and block direct-S3 claims.
2. **STEP 2:** Approve artifact_root URI + typed prefix schema (single root, no per-type knobs) +
   URI-validation tests.
3. **STEP 3:** Shared Common Identity normalization library (per-(product,family) nanos multiplier
   table, decimal-string preservation, `canonical_instrument_key` builder, `transform_hash` over
   code+config, `raw_payload_id` from object SHA-256, `source_proof_id` plumbing) + unit tests on
   the timestamp-unit hazard.
4. **STEP 4:** Idempotent create-only/no-overwrite write-manifest format + prove conditional-write
   semantics against the configured store.
5. **STEP 5:** Instrument-universe manifest (active at ANY point in window, expired/delisted
   included).
6. **STEP 6:** Per-venue raw→NT-class+contract-table projections to STAGING.
7. **STEP 7:** HIP-4 quoteToken parser-fidelity proof (mandatory before any HIP-4 normalized write).
8. **STEP 8:** Backtest read-back smoke test (NT `BacktestNode` over the staging catalog).
9. **STEP 9:** Read-only Jupyter notebook example consuming the staging catalog.
10. **STEP 10:** SourceProofReports + expanded evidence matrix + gap policy; on full acceptance, the
    deferred canonical promotion.

## Phases

### Phase 0 — Catalog capability proof (GATING) — depends: none
- Research-only crate/target on `nautilus-persistence` (rev `6e059dc`) with `features=["cloud"]`;
  confirm compile + object_store/aws resolves against the workspace datafusion constraint.
- Negative control: no-cloud build → `from_uri` on `s3://` captures the feature bail.
- Positive proof: SSM-resolved creds → `write_to_parquet` two fixtures → re-open → `query_files` →
  assert.
- Stamp an `NtCapabilityProof` (PROVEN direct-S3 + exact `storage_options` key set + credential
  source), OR document the local-write+sync fallback.
- Constraint guard: enable `cloud` ONLY on the research/backtest crate edge; build assertion that
  the live LiveNode target gains no cloud catalog path.

### Phase 1 — Artifact root + write discipline foundations — depends: Phase 0
- TOML/config-owned artifact_root S3 URI + typed prefix schema (`raw/`,
  `normalized/<schema_version>/`, `nt-catalog/`, `source-proofs/`, `backtests/`,
  `research-analytics/v1/{datasets,feature-tables,experiment-results,promotion-packages}/`,
  `artifact-index/v1/{events,snapshots,pointers}/kind=<artifact_kind>/`); single root, no per-type
  knobs.
- URI-validation tests rejecting local/cwd/temp/sibling-project + per-type root overrides.
- Common Identity fill library — per-(product,family) event_time→nanos multiplier table
  (spot=us×1000, futures=ms×1_000_000, metrics=string-datetime parse, seconds.fraction=Decimal×1e9),
  NO single hardcoded multiplier, NEVER REST response time; unit tests per unit hazard.
- Common Identity — exact decimal-string preservation, `canonical_instrument_key` builder,
  capture/event/availability time separation.
- Lineage — `raw_payload_id` from object SHA-256, `transform_hash` over code+config,
  `source_proof_id` plumbing; test that transform_hash changes on parser version change.
- Idempotent write-manifest format — `write_mode` (dry_run|local_staging|canonical_s3) +
  `no_overwrite_proof`; create-only via S3 If-None-Match (or a commit coordinator/table format);
  prove conditional-write semantics.

### Phase 2 — Instrument universe — depends: Phase 1
- Universe manifest covering instruments active at ANY point in window (expired/delisted included),
  per venue/product_family; emit `instrument_universe_snapshots` with
  base/quote/settle/contract_type/expiry/strike/option_type/listing/delisting + dex_name (HIP-3) /
  outcome_encoding+asset_id+wire_symbol+quoteToken (HIP-4).
- Reconcile current-snapshot gaps (Binance single exchangeInfo; Bybit/Deribit/HL current-listing);
  wire archive-symbol inference where available.
- `instrument_status` + `instrument_closes` population.

### Phase 3 — Per-venue raw→NT-class projections (to staging) — depends: Phase 2
- **Binance** — spot/futures_um/futures_cm trades(native)+aggTrades(aggregated, tagged)+klines→bars;
  mark/index/premium klines→mark_prices/index_prices/premium_index_prices; fundingRate→funding_rates;
  metrics→open_interest (+long_short/taker_buy_sell gated). Branch parser on header-vs-headerless +
  per-product timestamp unit.
- **OKX** — trades(native), candles→bars, funding_rates, order_book_400→order_book_deltas
  (snapshot=CLEAR+ADD, update=UPDATE/DELETE size=0) + order_book_depth_10; instrument_id from in-row
  instId not partition selector.
- **Bybit** — spot vs derivatives tick_trades schema branch (ms-int vs seconds.fraction-Decimal);
  kline/mark/index/premium→bars+series; funding_rate, open_interest_1d, delivery_price,
  historical_volatility; product_category via contractType join.
- **Deribit** — trades_seq_history→trades(native, source_sequence=trade_seq); bars_1m→bars;
  funding_history→funding_rates+index_prices; instrument_metadata→instruments;
  settlements/delivery→settlements/delivery_prices; historical_volatility. perpetual→product_family=
  future, product_category=perpetual. Flag index family (no event_time) as unusable for index_prices.
- **Hyperliquid-core** — l2Book→order_book_snapshots_fixed_depth(20)+order_book_depth_10 (inner
  raw.data.time event_time, outer time=capture_time); fundingHistory→funding_rates; asset_ctxs
  split into mark/index/open_interest/funding/premium (impact/mid as reconstructed_top_of_book);
  meta→instruments. node_fills_by_block deferred until schema verified + dedupe/completeness proof.
- **Hyperliquid HIP-3** — fundingHistory→funding_rates; candleSnapshot→bars;
  allPerpMetas/metaAndAssetCtxs→instruments (current-snapshot, no time-series claim); preserve
  dex_name + synthetic asset_id as derived join helper only.
- **Polymarket** — demux multiplexed Parquet by event_type: last_trade_price→trades(native),
  book→order_book_snapshots_full_depth_l2 (depth fidelity pending)+order_book_depth_10,
  price_change→quotes(reconstructed_top_of_book, NOT native deltas),
  tick_size_change→instrument_status. Key off acceptance-manifest family + event_type, NOT legacy
  fixed_depth key path.

### Phase 4 — HIP-4 quoteToken parser-fidelity proof — depends: Phase 2
- HIP-4 outcomeMeta projection — encode identity (encoding=10*outcome+side, wire_symbol=#<encoding>,
  asset_id=100000000+encoding), preserve raw quoteToken verbatim; → instruments +
  prediction_market_events/outcomes/questions.
- quoteToken parser-fidelity proof harness — assert correct quoteToken→quote_asset for EVERY emitted
  HIP-4 row; block the HIP-4 write until it passes.
- HIP-4 market-data projection (gated) — candleSnapshot→bars, recentTrades→trades(recent-only),
  l2Book→order_book_snapshots_fixed_depth+quotes(reconstructed_top_of_book); attach forbidden_claims.

### Phase 5 — Consumption smoke tests — depends: Phase 3
- Backtest read-back — NT `BacktestNode`/`BacktestEngine` over the staging catalog replays the
  two-instrument fixture; assert it emits a result under `backtests/`.
- Read-only Jupyter notebook — consume via fsspec/s3fs; surface fidelity_class + forbidden_claims;
  confirm RA writes only under `research-analytics/v1/`.

### Phase 6 — Source proofs, evidence matrix, canonical promotion (gated) — depends: Phases 3,4,5
- SourceProofReport per source family (portable sample + SHA-256, schema sample + row counts +
  timestamp range, license_ref); all required_checks PASS.
- Expanded evidence matrix — one row per (venue, product_family, table_family).
- Gap policy — max gap frequency/duration + forbidden_claims; encode known gaps (HL core 799,
  Deribit 1118 errors, OKX post-Apr-5 unaccepted, Polymarket 914-physical-vs-748-manifest).
- Deferred canonical promotion (only on full acceptance) — re-point staging into artifact_root,
  flip write_mode→canonical_s3, commit_state→committed, register artifact-index events.

## Open decisions for the owner

1. **Artifact_root home** — reuse `bolt-parquet` or a separate bucket; top-level prefix?
   (`normalized/` name is not contract-given.) E-034 is DECISION_NEEDED.
2. **Coverage completeness bar** — what counts as "done" per table given materially partial coverage
   (OKX post-Apr-5 unaccepted, Deribit 1118 errors, HL core 799 gaps, Polymarket 914 vs 748,
   funding_rates only 4 OKX daily files, current-snapshot-only universe for several venues)?
3. **Cloud-feature isolation** — confirm `cloud` ONLY on the research crate, OFF the live binary
   (recommended).
4. **Conditional-write backend** — if S3 lacks If-None-Match create-only here, adopt a commit
   coordinator / table format? Which?
5. **HL node_fills authority** — invest in the fills dedupe/completeness proof now, or defer the
   Hyperliquid native-trades tape?
6. **Signal tables scope** — populate long_short_ratios / taker_buy_sell_volume now or keep excluded?
7. **Bars carry granularity** — mark/index/premium are 1m-OHLC-only; is 1m acceptable or is
   sub-minute required?

## Risks (from synthesis)

- Phase 0 catalog proof is NOT yet executed (read-only analysis only); do not mark E-037
  SOURCE_PROVEN-positive until the write+query runs end to end.
- Cargo feature-resolution risk activating object_store/aws (must stay in lockstep with datafusion's
  pre-1.0 object_store constraint).
- Per-product timestamp-unit hazard is the highest silent-corruption risk (spot=µs, futures=ms,
  metrics=string-datetime, Bybit derivatives=seconds.fraction); never fall back to REST response
  time.
- Scientific-notation/decimal fields must be parsed as Decimal from the exact raw string.
- Several NT classes are unsatisfiable from this tranche → record as forbidden_claims, not faked
  (no order books/quotes for Binance/Bybit/HIP-3/Deribit historical; no per-strike Greeks/IV;
  Deribit index_prices has no source event_time; Polymarket full-depth pending; HIP-4/Polymarket
  trades are recent/bounded).
- Identity traps (OKX/Bybit perpetual-vs-dated contractType join; OKX instrument_id from payload not
  partition; Polymarket family mislabeled in key path).
- Interim staging writes must be strictly labeled (write_mode=local_staging, source_proof_id pending,
  commit_state=staged, forbidden_claims) or be mistaken for canonical.
- transform_hash must hash CODE + CONFIG, not config alone.
- Source bindings today are almost entirely instrument_universe/instruments; market-data bindings
  needed for backtests are largely not yet declared.
- Python research path (if used) introduces a second credential surface (s3fs) that can drift from
  SSM-only.

---

## Internal adversarial critique (to be confirmed/refuted/extended by external reviewers)

**Overall verdict (internal):** Directionally sound and unusually disciplined on fidelity labeling,
proof-first gating, and the canonical-write gate; core feasibility VERIFIED at the pinned rev. But
serious COMPLETENESS/CORRECTNESS gaps block clean execution. Do NOT proceed to canonical promotion
as written; fix the NT-class-vs-table mapping, NT writer's conditional-write reality, and the
taxonomy/vocabulary single-source conflicts before Phase 3, and estimate cost/scale before a full
one-year projection.

### HIGH severity

1. **NT data-class mis-assignment / catalog write feasibility.** The plan conflates contract
   normalized table families with NT-replayable data classes. The catalog's `CatalogPathPrefix` set
   is fixed and small (`quotes`, `trades`, `order_book_deltas`, `order_book_depths` — NOT
   `order_book_depth_10`; `bars`, `index_prices`, `mark_prices`, `funding_rate_update` — NOT
   `funding_rates`; `instrument_status`, `instrument_closes`, `instruments`). No NT class for
   open_interest, premium_index_prices, long_short_ratios, taker_buy_sell_volume,
   historical_volatility, settlements, delivery_prices, option_greeks, implied_volatility,
   order_book_snapshots_fixed_depth/full, prediction_market_*. Binance mark/index/premium klines map
   to a `Bar` OHLC series but NT's mark/index classes are point updates, not bars. Phase 5's
   BacktestNode can only replay trades/quotes/bars/order_book_deltas/depth/instruments; everything
   else is non-NT Parquet (custom data) the BacktestNode will NOT consume.
   **Fix:** split deliverable into (a) NT-replayable classes (exact NT prefix names) vs (b) non-NT
   research-only Parquet/custom data; rename `order_book_depth_10`→`order_book_depths`,
   `funding_rates`→`funding_rate_update` where the NT class is used; add a per-table_family→(NT|non-NT)
   column to the evidence matrix; scope Phase 5's replay claim to the NT-replayable subset only.

2. **Idempotency / create-only / no-overwrite.** NT's `write_to_parquet` implements no-overwrite as
   a non-atomic `head()`-then-PUT (TOCTOU): it checks existence and skips, then unconditionally
   writes; concurrent writers both see absent and both PUT (last-writer-wins). PUT is not issued with
   If-None-Match. Filename is interval-keyed (`timestamps_to_filename`), not content/transform-hash
   keyed; the only guard is the disjoint-interval check, bypassable with `skip_disjoint_check=true`.
   So NT's own writer does NOT give the create-only atomicity the contract gate and
   `ingest_manifest.no_overwrite_proof` require, yet the plan leans on NT as the canonical engine.
   **Fix:** wrap catalog writes behind an external conditional-PUT layer (object_store
   PutMode::Create / S3 If-None-Match) and prove it as a first-class Phase 1 task, OR adopt a commit
   coordinator/table format and never call NT's writer directly for canonical writes; make the E-038
   conditional-write decision blocking for Phase 0 sign-off; key dedupe on content+transform_hash,
   not just the interval.

3. **Binance product-family taxonomy mismatch.** The contract defines four families
   (usd_m_perpetual, usd_m_delivery, coin_m_perpetual, coin_m_delivery); the plan/VENUE_MAPPINGS
   collapse to `futures_um`/`futures_cm` (product_family) and resolve perpetual-vs-delivery only via
   product_category; the source-bindings TOML uses a third spelling
   (usd_m_perpetual_or_delivery). Since `canonical_instrument_key = <venue>/<product_family>/...`,
   using `futures_um` produces non-contract keys + partition layout. Three vocabularies violate
   single-source-of-truth.
   **Fix:** adopt the contract's four-family taxonomy as single source; derive product_family (not
   just category) from contractType at normalize; reconcile the TOML + mappings; add a test that a
   dated DELIVERY symbol lands in `*_delivery` and no row is emitted with `futures_um/futures_cm`.

4. **OKX order_book_deltas native-vs-derived contract violation.** OKX `order_book_400` (snapshot
   CLEAR+ADD, update UPDATE/DELETE) is mapped to native `order_book_deltas`, but the contract
   reserves that table for NATIVE L2/L3 and forbids derived deltas there. The 400-level lines carry
   NO native seqId (ordering falls back to file line ordinal) → that is an
   `order_book_snapshot_deltas` derivation, which "cannot satisfy native order_book_deltas."
   **Fix:** map to `order_book_snapshot_deltas` (and/or `order_book_snapshots_fixed_depth`) with a
   named derivation rule + source proof, NOT native order_book_deltas, unless a native-sequence OKX
   L2 archive is separately proven; downgrade the evidence-matrix OKX order_book_deltas claim to
   pending_source_proof until then.

5. **Polymarket family mislabel + source URI/host inconsistency.** Three sources disagree: S3 key
   path says `order_book_snapshots_fixed_depth`; binding TOML lists
   `[fixed_depth, snapshot_deltas, bars]`; plan/manifest reclassify to
   `order_book_snapshots_full_depth_l2`. Host differs: binding `archive.pmxt.dev/Polymarket/v2` vs
   plan/mappings `r2v2.pmxt.dev`. Binding lists a `bars` family the demux does not find.
   **Fix:** make the acceptance manifest the single authoritative family source; update the binding
   TOML + key-path note to match; state the exact staged host; remove `bars` from the Polymarket
   binding (or prove it); keep full_depth_l2 at pending_source_proof until a max-depth proof passes.

6. **Deribit index_prices event-time blocker.** The Deribit index family (`get_index_price`) payload
   has NO timestamp (only REST usIn/usOut) → event_time impossible, a hard "do not use REST response
   time" violation; index history must come from `funding_history.index_price`. The plan notes this
   but does not encode it as an explicit forbidden_claim or a fail-loud guard.
   **Fix:** add forbidden_claim "deribit get_index_price MUST NOT populate index_prices.event_time"
   + a normalization-library assertion rejecting any index_prices row from get_index_price; same
   fail-loud guard for all snapshot-only families that carry only capture_time.

### MEDIUM severity

7. **write_mode vocabulary fragmentation.** Schema sanctions `dry_run|local_staging|canonical_s3`,
   but staging scripts emit `s3_staging` (×7), `s3_staging_only` (×2), `local_staging` (×2). The
   plan's "flip local_staging→canonical_s3" promotion assumes a starting value the artifacts do not
   carry. **Fix:** define the enum in one place; decide if S3-staging is distinct or an alias;
   migrate every script + manifest; add a validation test.

8. **transform_hash / raw_payload_id / source_proof_id plumbing completeness.** A literal "pending"
   source_proof_id is not a stable id (later distinct proofs can't be back-linked). nt_instrument_id
   population is unspecified for the NT-replayable subset. **Fix:** assign deterministic provisional
   source_proof_id per (venue,product_family,table_family); add an nt_instrument_id population rule +
   a Phase 5 precondition that the two fixtures have accepted nt_mapping_status.

9. **Instrument-universe active-at-any-point completeness.** Every venue's staged universe is a
   CURRENT snapshot; inference is heuristic and cannot recover a delisted instrument absent from all
   staged objects; expired-contract product_category is unresolvable when contractType is absent.
   **Fix:** demote Phase 2 to "best-effort universe + explicit completeness gap record"; add a
   declared, source-proof-cited symbol-shape parser for expired product_category; make minimum
   universe completeness a gate-blocking decision.

10. **Cost/scale unaddressed.** No volume/compute estimate despite large inputs (Deribit 35,287
    objects; HL node_fills ~19MB/object; OKX order_book_400 thousands of partitions; Polymarket
    260-520MB/hour). NT's writer does head()+get_directory_intervals() per write (per-file S3
    round-trips) — slow/expensive at tens of thousands of objects; requester-pays egress for HL.
    **Fix:** add a costed estimate (object counts, bytes, S3 request amplification, requester-pays
    egress, wall-clock) + a partitioning/parallelism strategy; gate the full projection on it.

11. **Phase 0 negative-control logic gap.** The no-cloud bail proves the gate is feature-driven but
    not that SSM creds cause the positive write to succeed (could be ambient AWS env/instance-profile
    fallback). **Fix:** add a second control — cloud ON but absent/invalid creds → capture auth
    failure — so success is attributable to SSM creds (also empirically proves rule-6 compliance).

### LOW severity

12. **Cloud-feature isolation vs Cargo feature unification.** "Enable cloud only on the research
    crate edge" is insufficient under one-workspace additive feature unification — cloud could be
    pulled into the live binary. **Fix:** put the research/backtest crate in a separate workspace (or
    a package not sharing the live binary's resolution); add a `cargo tree -e features` assertion on
    the live target.

13. **HL node_fills lane authority unresolved but on the native-trades critical path.** HL-core
    native trades depend on node_fills_by_block (schema unverified, lane authority contradicted), so
    HL-core has NO confirmed native-trades source this tranche. **Fix:** state "no HL-core native
    trade tape" as a forbidden_claim until proven; keep node_fills as a separate gated future task.
