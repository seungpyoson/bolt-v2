# Normalization / Transform / Catalog Plan (v2)

`plan_version`: `normalization-catalog-plan.v2`
`supersedes`: `normalization-catalog-plan.v1`
`status`: REVISED — v1 went through an adversarial-review pass (15 findings F1–F15). v2 incorporates every grounded cluster correction. Still DRAFT pending owner sign-off on the open decisions; does NOT itself authorize canonical writes (those remain gated, see "Contract gate handling").
`NT rev`: `6e059dcbb59ac1e582132fc431a581936c216c3c` (crate `nautilus-persistence`); `object_store` 0.13.2.

## Purpose

Turn the one-off seven-token raw S3 backfill (audit input) into a **research-ready store** the
project's specs already mandate: a NautilusTrader `ParquetDataCatalog` that NT's
`BacktestNode`/`BacktestEngine` can replay, plus non-NT research-only Parquet that read-only Jupyter
notebooks / Research-Analytics can consume. Raw provider payloads are **audit input, not replay
input** (evidence E-002, SOURCE_PROVEN). This document is the build plan for the raw→catalog
projection layer; it does NOT itself authorize canonical writes (those remain gated, see "Contract
gate handling").

v2 fixes the v1 errors found in adversarial review:
- v1 conflated contract normalized table families with NT-replayable data classes (F1).
- v1 leaned on NT's own writer, which is non-atomic last-writer-wins (F2).
- v1 ran the proof step LAST, letting the backtest read un-proven provider data (F14).
- v1 carried three drifting product-family vocabularies, multiple `write_mode` spellings, and a
  literal `pending` source_proof_id (F3, F7, F8).
- v1 mis-mapped OKX/Polymarket/Deribit/HL fidelity (F4, F5, F6, F13).
- v1 over-claimed a window-complete instrument universe from current-snapshot sources (F9).
- v1 had no cost/scale estimate, no credential negative control, and a non-isolating Cargo
  feature-flag plan (F10, F11, F12).
- v1's canonical promotion re-pointed a whole staging prefix, canonicalizing orphan bytes (F15).

---

## 1. Verified-at-pinned-rev facts (re-checked by the main session)

NT rev `6e059dc`, crate `nautilus-persistence`:

- The catalog's `CatalogPathPrefix` set is fixed (`crates/persistence/src/backend/catalog.rs:4109-4146`):
  `QuoteTick→quotes`, `TradeTick→trades`, `OrderBookDelta→order_book_deltas`,
  `OrderBookDepth10→order_book_depths` (**NOT** `order_book_depth_10`, `catalog.rs:4112`),
  `Bar→bars`, `IndexPriceUpdate→index_prices`, `MarkPriceUpdate→mark_prices`,
  `FundingRateUpdate→funding_rate_update` (**NOT** `funding_rates`, `catalog.rs:4116`),
  `InstrumentStatus→instrument_status`, `InstrumentClose→instrument_closes`,
  `InstrumentAny→instruments`, `AccountState→account_state`, plus order/position/report lifecycle
  prefixes (execution outputs, out of scope here).
- The Rust `BacktestNode` replay path (`crates/backtest/src/node.rs:539-567`, `dispatch_query`)
  streams ONLY the 9-member `NautilusDataType` enum (`crates/backtest/src/config.rs:52-62`):
  `QuoteTick, TradeTick, Bar, OrderBookDelta, OrderBookDepth10, MarkPriceUpdate, IndexPriceUpdate,
  InstrumentStatus, InstrumentClose`. `FundingRateUpdate` is **absent** from `NautilusDataType` and
  from the `Data` enum (`crates/model/src/data/mod.rs:100-112`).
- `instruments` load via a separate lane: `write_instruments` / `query_instruments`
  (`node.rs:165`), NOT through `dispatch_query`.
- `MarkPriceUpdate`/`IndexPriceUpdate` are **point updates** carrying a single price + timestamp
  (`crates/model/src/data/mod.rs:107-108`), **not** OHLC bars.
- NT's writer is non-atomic: `head()` existence probe (`catalog.rs:539-542`) then unconditional
  `object_store.put` (`parquet.rs:197`, default `PutMode::Overwrite`, no If-None-Match). Filename is
  interval-keyed (`timestamps_to_filename`, `catalog.rs:535,4175-4180`). The only structural guard is
  the disjoint-interval check, bypassable with `skip_disjoint_check=true` (`catalog.rs:549`).
- `object_store` 0.13.2 supports `PutMode::Create` (atomic If-None-Match: *, `Error::AlreadyExists`
  on collision — `lib.rs:1702-1711`, `aws/mod.rs:181-201`) and `PutMode::Update(UpdateVersion)`
  (If-Match CAS — `aws/mod.rs:202-228`). S3 `PutMode::Create` requires `S3ConditionalPut::ETagMatch`
  (the crate default, `aws/precondition.rs:120-128`); `Disabled` makes `Create` return
  `Error::NotImplemented` (`aws/mod.rs:183-187`).
- `bolt-v2/Cargo.toml` is a **single-package binary crate** (`Cargo.toml:1-2`), NOT a workspace
  (`find` returns only `./Cargo.toml`). It already lists `nautilus-persistence` as a direct
  dependency (`Cargo.toml:39`) with **no** `cloud` feature. `nautilus-persistence` default features
  are empty (`crates/persistence/Cargo.toml:24`); `cloud = object_store/{aws,azure,gcp,http}`
  (`:25-30`); `python` transitively enables `cloud` (`:39-49`). Today `cargo tree -e features` on the
  live binary shows `object_store v0.13.2` present via datafusion but with features only
  `fs`/`tokio`/`walkdir` — the `aws` feature is NOT enabled (empirically verified at HEAD).

---

## 2. NT-class target matrix (authoritative — resolves F1)

`source_of_truth`: catalog write surface = `crates/persistence/src/backend/catalog.rs`; backtest
replay surface = `crates/backtest/src/{config.rs,node.rs}`; NT rev `6e059dc`.

### 2.1 Three-tier classification (the missing distinction)

v1 collapsed two NT surfaces that are NOT the same. There are THREE tiers, not two:

- **Tier A — NT-replayable**: type is a `NautilusDataType` member, so a Rust `BacktestNode` can
  `query::<T>` it from the catalog and stream it. Exhaustive set is the 9-member `NautilusDataType`
  enum; `dispatch_query` is the exhaustive replay dispatch.
- **Tier B — catalog-writable, NOT engine-replayable**: type has a `CatalogPathPrefix` and a typed
  `write_to_parquet` path, so it lives in the catalog with a real NT prefix, but it is NOT a
  `NautilusDataType`, so `BacktestNode` cannot stream it. Only member relevant to this tranche:
  `FundingRateUpdate → funding_rate_update`.
- **Tier C — non-NT research-only Parquet (custom data)**: no NT data class at all. Lands as
  `Data::Custom`/custom-data Parquet (`catalog.rs:431-433,449-451`) or as a plain research Parquet
  table under `normalized/`. `BacktestNode` does NOT consume it.

`instruments` is a fourth, separate lane: written via `write_instruments`, loaded via
`query_instruments` (`node.rs:165`), NOT through `dispatch_query`. It is a backtest precondition
(instrument definitions), not a streamed time-series.

### 2.2 Verified NT path-prefix set (catalog.rs:4109-4146)

| NT type | Exact prefix string | Tier | `NautilusDataType`? |
| --- | --- | --- | --- |
| `QuoteTick` | `quotes` | A | yes |
| `TradeTick` | `trades` | A | yes |
| `OrderBookDelta` | `order_book_deltas` | A | yes |
| `OrderBookDepth10` | `order_book_depths` | A | yes |
| `Bar` | `bars` | A | yes |
| `IndexPriceUpdate` | `index_prices` | A | yes |
| `MarkPriceUpdate` | `mark_prices` | A | yes |
| `InstrumentStatus` | `instrument_status` | A | yes |
| `InstrumentClose` | `instrument_closes` | A | yes |
| `FundingRateUpdate` | `funding_rate_update` | **B** | **NO** |
| `InstrumentAny` | `instruments` | separate (`write_instruments`/`query_instruments`) | n/a |
| `AccountState` | `account_state` | execution output, out of scope | no |

Confirmed exact strings: `order_book_depths` (NOT `order_book_depth_10`); `funding_rate_update`
(NOT `funding_rates`). `FundingRateUpdate` is Tier B, not Tier A. v1's blanket "NT class for funding"
is correct at the catalog-write level but MUST be qualified: funding cannot be replayed by
`BacktestNode`.

### 2.3 Authoritative table-target matrix (every contract table family)

**Provenance** (`backfill-table-contract.md:83-93`) — all non-NT:

| Contract family | Target |
| --- | --- |
| `raw_payloads` | Tier C — non-NT research-only/operational Parquet |
| `source_proofs` | Tier C — non-NT operational store |
| `ingest_manifests` | Tier C — non-NT operational store |
| `instrument_universe_snapshots` | Tier C — non-NT research-only Parquet (feeds the `instruments` lane; not itself an NT class) |

**Instruments** (`contract:96-100`):

| Contract family | Target |
| --- | --- |
| `instruments` | `InstrumentAny → instruments` (separate `write_instruments`/`query_instruments` lane; backtest precondition, not streamed) |
| `instrument_status` | Tier A — `InstrumentStatus → instrument_status` |
| `instrument_closes` | Tier A — `InstrumentClose → instrument_closes` |

**Market Data** (`contract:107-130`):

| Contract family | Target |
| --- | --- |
| `trades` | Tier A — `TradeTick → trades` (native only; `trade_source_type=aggregated` rows still write to `trades` but carry the aggregated tag + forbidden-claim) |
| `quotes` | Tier A — `QuoteTick → quotes` (carries `quote_source_type`; `reconstructed_top_of_book` rows tagged + forbidden-claim) |
| `order_book_deltas` | Tier A — `OrderBookDelta → order_book_deltas` (NATIVE L2/L3 only; see §6 F4 — OKX 400-level is NOT native here) |
| `order_book_snapshot_deltas` | Tier C — non-NT research-only Parquet (no NT class; derived clear-and-rebuild) |
| `order_book_snapshots_full` | Tier C — non-NT research-only Parquet (no NT class) |
| `order_book_snapshots_fixed_depth` | Tier C — non-NT research-only Parquet (no NT class). A top-10 projection MAY ADDITIONALLY be emitted to Tier A `order_book_depths` — see naming note below |
| `order_book_depth_10` (contract column name) | **Rename → NT prefix `order_book_depths`.** Tier A — `OrderBookDepth10 → order_book_depths`. The contract column name `order_book_depth_10` is a derived/native top-10 projection; the NT catalog prefix is `order_book_depths` (`catalog.rs:4112`). All `order_book_depth_10` references in this plan use prefix `order_book_depths` |
| `bars` | Tier A — `Bar → bars` (carries `bar_source_type`) |

**Derivatives, Carry, And Risk State** (`contract:134-143`):

| Contract family | Target |
| --- | --- |
| `mark_prices` | Tier A — `MarkPriceUpdate → mark_prices` (point update, NOT a bar — see §2.4) |
| `index_prices` | Tier A — `IndexPriceUpdate → index_prices` (point update, NOT a bar) |
| `premium_index_prices` | Tier C — non-NT research-only Parquet (no NT class) |
| `funding_rates` (contract name) | **Tier B** — `FundingRateUpdate → funding_rate_update`. Catalog-writable with a real NT prefix, but NOT in `NautilusDataType`, so NOT BacktestNode-replayable. All `funding_rates` references use prefix `funding_rate_update` where the NT class is targeted |
| `open_interest` | Tier C — non-NT research-only Parquet |
| `liquidations` | Tier C — non-NT research-only Parquet |
| `long_short_ratios` | Tier C — non-NT research-only Parquet |
| `taker_buy_sell_volume` | Tier C — non-NT research-only Parquet |
| `borrow_lending_rates` | Tier C — non-NT research-only Parquet |

**Options** (`contract:146-153`) — all Tier C (no NT class):
`option_greeks`, `implied_volatility`, `historical_volatility`, `forward_prices`, `delivery_prices`
→ Tier C. `settlements` → Tier C (event records; closest NT analogue is `InstrumentClose`, but
settlements are not 1:1 with NT instrument-close semantics — keep non-NT unless an `InstrumentClose`
mapping is separately source-proven — see Open decision §11.2).

**Prediction-Market Metadata** (`contract:164-167`) — all Tier C (no NT class):
`prediction_market_events`, `prediction_market_outcomes`, `prediction_market_settlements`,
`prediction_market_fee_models` → Tier C. The market-data side of prediction-market instruments still
routes through the common Tier A/B/C market-data tables above.

### 2.4 Bar-vs-point-update mismatch (mark/index/premium klines)

Several venue mappers read OHLC kline/candle inputs but would target `MarkPriceUpdate`/
`IndexPriceUpdate`. NT's mark/index classes are **point updates**, NOT OHLC bars. A 1-minute OHLC
kline does not losslessly become a point update. **Resolution rule:**

- Mark/index kline OHLC data is fundamentally a `Bar` series. The faithful NT target for the raw OHLC
  is **Tier A `Bar → bars`**, with `bar_source_type=provider_supplied` and the bar spec encoding the
  price source (mark/index/premium). It is NOT silently flattened into `mark_prices`/`index_prices`.
- A `MarkPriceUpdate`/`IndexPriceUpdate` point series MAY be derived from the kline (e.g.
  close-as-point) ONLY when the row is explicitly tagged as a derivation and carries a forbidden-claim
  (`mark/index point update derived from 1m OHLC close; not a native tick mark/index stream`). Absent
  that proof, do NOT populate `mark_prices`/`index_prices` from klines.
- `premium_index_prices` has no NT class → Tier C only.

### 2.5 Replay-claim rule (binds the BacktestNode subset)

**Rule P-NT-REPLAY:** The `BacktestNode`/`BacktestEngine` replay claim applies ONLY to Tier A — the
9-member `NautilusDataType` set, routed through `dispatch_query`: `quotes`, `trades`, `bars`,
`order_book_deltas`, `order_book_depths`, `mark_prices`, `index_prices`, `instrument_status`,
`instrument_closes` — plus the `instruments` precondition lane. NOTHING else is replayable:

- **Funding is NOT replayable** by `BacktestNode`. `funding_rate_update` (Tier B) is catalog-resident
  with a real NT prefix but absent from `NautilusDataType`. Funding consumption is a strategy/actor-
  side concern outside the catalog-stream path, or via custom data (Open decision §11.1).
- **All Tier C families are NOT replayable** — `open_interest`, `premium_index_prices`,
  `long_short_ratios`, `taker_buy_sell_volume`, `borrow_lending_rates`, `liquidations`,
  `historical_volatility`, `option_greeks`, `implied_volatility`, `forward_prices`, `settlements`,
  `delivery_prices`, `order_book_snapshot_deltas`, `order_book_snapshots_full`,
  `order_book_snapshots_fixed_depth`, and all `prediction_market_*`.
- Any consumption smoke test that claims replay MUST use a Tier A type. It does NOT prove replay for
  any Tier B/C family.

### 2.6 Evidence-matrix amendment

The expanded evidence matrix (`contract:303-304`) gains a per-`(venue, product_family, table_family)`
column **`nt_target`** with exactly one of: `nt_replayable:<prefix>` (Tier A),
`nt_catalog_only:funding_rate_update` (Tier B), `instruments_lane` (instruments), or
`non_nt_research_parquet` (Tier C). This column is the single source mapping each contract table to
its NT-surface tier and is what scopes the replay claim.

---

## 3. Catalog I/O decision

Use **Rust (`nautilus-persistence`)** as the canonical engine for the raw→catalog projection that
feeds backtests; allow **Python** (`nautilus_trader` + fsspec/s3fs + V2 wranglers) only as optional
research-side convenience that writes through the SAME catalog format.

Rationale: (1) **Credential discipline** — bolt-v2 mandates SSM-only secrets (rule 6); Rust resolves
S3 creds in-process and injects them into `from_uri` `storage_options`, whereas the Python path needs
an s3fs/fsspec surface that risks an env-var/AWS-CLI fallback. (2) **Single build path** — Rust
projection shares one `nautilus-persistence`/`datafusion` version with the downstream `BacktestNode`.
(3) `object_store` 0.13.2 is already in the tree transitively.

**Write surface caveat (F2):** NT's `ParquetDataCatalog::write_to_parquet`/`write_custom_data_batch`
is used **only** for read-back/query. ALL data writes (staged and canonical) go through the external
`ConditionalCatalogWriter` proven in Phase 0 (§5.2). See §4.

---

## 4. Write discipline + canonical promotion (resolves F2 + F15)

### 4.1 Why NT's writer cannot be the write surface (F2)

NT's `write_to_parquet` is non-atomic last-writer-wins, interval-keyed, not create-only:

- Existence is checked with a separate `head()` call (`catalog.rs:539-542`), then the file is written
  by an unconditional `put` (`parquet.rs:197`, default `PutMode::Overwrite`). Between `head()` and
  `put` there is a TOCTOU window: two concurrent writers both see "absent" and both PUT; on S3 the
  second silently overwrites the first.
- The filename is **interval-keyed** (`timestamps_to_filename(start_ts, end_ts)`, `catalog.rs:535`,
  `catalog.rs:4175-4180`), not content-keyed. Two different transforms over the same `(start_ts,
  end_ts)` collide on the same object path; the `head()` skip then suppresses the second write
  entirely, so a re-run with a *changed* `transform_hash` is silently dropped.
- The only structural guard is the disjoint-interval check (`catalog.rs:549-561`), bypassable with
  `skip_disjoint_check=true` (`catalog.rs:439-447`). It is not a create-only guard.

Conclusion: NT's writer cannot satisfy the contract's "Idempotent write manifest, create-only
behavior, and no-overwrite behavior" gate or `ingest_manifest.no_overwrite_proof`. **Never call NT's
`write_to_parquet`/`write_custom_data_batch` directly for any staged-or-canonical write.**

### 4.2 Why prefix re-pointing canonicalizes orphan bytes (F15)

The v1 Phase 6 step "re-point staging into artifact_root, flip write_mode→canonical_s3" treats every
object under a staging prefix as accepted. Staging prefixes are shared and accumulate orphaned objects
from failed/aborted runs (`commit_state` can be `orphan` or `superseded`, `data-model.md:135`). A
prefix flip canonicalizes those too. Promotion must enumerate **exact** accepted objects, not a
prefix.

### 4.3 `ConditionalCatalogWriter` (external conditional-create layer)

A research-only crate that wraps but never delegates the write to NT's writer:

1. **Encode in-process, write conditionally.** Reuse NT's batch encoder path
   (`write_batches_to_object_store`, `parquet.rs:170-194`) only up to the buffer; replace the final
   `object_store.put(...)` (`parquet.rs:197`) with `object_store.put_opts(path, payload, PutOptions {
   mode: PutMode::Create, .. })` (`object_store` lib.rs:752, `PutMode::Create` lib.rs:1708). On S3
   this issues `If-None-Match: *` (`aws/mod.rs:189`) and returns `Error::AlreadyExists`
   (`aws/mod.rs:194`, lib.rs:2064) on collision — atomic, no TOCTOU, no `head()` pre-check.
2. **Require conditional-put to be configured, fail loud otherwise.** S3 `PutMode::Create` requires
   `S3ConditionalPut::ETagMatch` (`aws/precondition.rs:127-128`); when `Disabled` the put returns
   `Error::NotImplemented` (`aws/mod.rs:183-187`). The writer asserts at construction that
   `conditional_put != Disabled` for the resolved store and aborts the run otherwise — never silently
   degrade to overwrite.
3. **Content+transform-hash-keyed object path, NOT interval-keyed.** The object key embeds
   `content_hash` (`sha256` of the parquet bytes, `data-model.md:131`) and `transform_hash`
   (code+config hash, `data-model.md:78`), e.g.
   `<type_prefix>/<instrument_id>/<start>_<end>__t-<transform_hash>__c-<content_hash>.parquet`. Two
   distinct transforms over the same interval are distinct objects (fixes the interval-collision
   drop); identical re-runs land on the identical key (idempotent); `Create` is the correct semantic
   — a colliding `Create` = "byte-identical artifact already present" (treat `AlreadyExists` as an
   idempotent no-op) while a *different* transform produces a *different* key.
4. **Concurrency proof (the BLOCKER acceptance criterion).** Spawn N concurrent writers racing the
   same logical artifact against the configured store (or a `PutMode::Create`-capable conformance
   store, e.g. MinIO/R2, when offline) and assert exactly one PUT wins and the losers observe
   `AlreadyExists` — never two distinct successful PUTs to one key, never a silent overwrite.
5. **`no_overwrite_proof`.** The proven layer's identity (store URI, `conditional_put` mode, the
   concurrency-proof transcript hash) is recorded as `ingest_manifest.no_overwrite_proof`; absence of
   an accepted `no_overwrite_proof` blocks any `local_staging` or `canonical_s3` write.

Every staged and canonical data object — without exception — goes through this layer. NT's
`ParquetDataCatalog` is used **only** for read-back/query (`query_files`), never for writes.

### 4.4 Canonical promotion via explicit PACKAGE + conditional pointer commit (F15)

Promotion is the commit of an explicit promotion package, never a prefix operation. No staging prefix
is ever re-pointed, renamed, or copied wholesale.

1. **Build a `PromotionPackage`** (typed artifact under `research-analytics/v1/promotion-packages/`,
   `data-model.md:149`). It enumerates, by exact value, every object being promoted: exact accepted
   object URI (the content+transform-hash-keyed staging key); `content_hash` (`sha256`,
   `data-model.md:131`); `source_proof_id` + `source_proof_version`, which MUST be an `accepted`
   `SourceProofReport` (`data-model.md:84-86`); `transform_hash` (`data-model.md:78`). No prefix,
   glob, or "everything under X" enumeration is permitted.
2. **Reject anything unaccepted/orphan/failed-run.** Package construction fails loud (does not
   silently skip) if any enumerated object: (a) has no `accepted` SourceProofReport; (b) has
   `commit_state` of `orphan` or `superseded` (`data-model.md:135`); (c) whose recomputed `sha256`
   does not match the recorded `content_hash`; (d) whose `transform_hash` is not the one tied to the
   accepted proof. Any staged object NOT enumerated is simply never promoted.
3. **Write canonical objects via the SAME Phase 0 conditional-create layer.** Each promoted object is
   materialized at its canonical `nt-catalog/` (or `normalized/<schema_version>/`) URI with
   `PutMode::Create`. A canonical-path collision = `AlreadyExists` = a prior accepted identical
   artifact (idempotent); never an overwrite.
4. **Commit via a conditional artifact-index pointer update, NOT a prefix flip.** Promotion becomes
   "live" by atomically advancing the `artifact-index` pointer:
   - append an immutable index event (`event_uri` under `artifact-index/v1/events/kind=nt_catalog/`,
     `data-model.md:124-125`) via `PutMode::Create`;
   - write an immutable committed snapshot (`snapshot_uri` under
     `artifact-index/v1/snapshots/kind=<kind>/`, `data-model.md:127-128`) referencing the
     PromotionPackage and its enumerated object set, via `PutMode::Create`;
   - advance the hot pointer (`latest_pointer_uri = artifact-index/v1/pointers/kind=<kind>/latest.json`,
     `data-model.md:129-130`) via compare-and-swap: `put_opts(pointer, payload,
     PutMode::Update(UpdateVersion { e_tag, .. }))` (lib.rs:1711). On S3 this issues `If-Match:
     <etag>` (`aws/mod.rs:208-210`) and retries the documented 409-conflict case (`aws/mod.rs:215`);
     a lost CAS race surfaces `Error::Precondition` (`aws/mod.rs:223-224`) and the promotion retries
     against the new pointer.
   - flip the package's and enumerated artifacts' `commit_state` `staged → committed`
     (`data-model.md:135`) only as recorded *in the committed snapshot*, never by mutating staging
     objects.
5. **Canonical reads resolve only through the committed pointer/snapshot**, so the canonical view is
   exactly the package-enumerated set. There is no path by which a non-enumerated staging object
   becomes part of the canonical catalog.

`write_mode` reaches `canonical_s3` and `commit_state` reaches `committed` only as a consequence of a
successful pointer commit of an accepted package — not as a manual flag flip on a prefix.

---

## 5. Contract gate handling + proof-acceptance precedence (resolves F14)

### 5.1 The gate (unchanged discipline)

The approval gate (`backfill-table-contract.md:292-309`) is respected by NEVER flipping the
ingest-manifest `write_mode` to `canonical_s3` and never writing under canonical `artifact_root`
prefixes until ALL gate items are approved: (a) artifact_root URI + prefix schema; (b) a
`SourceProofReport` per `(venue, product_family, table_family)` with all `required_checks` PASS;
(c) one portable sample raw payload + checksum per source family; (d) parser schema sample with row
counts + timestamp range; (e) instrument-universe manifest (best-effort + completeness gap record per
§7); (f) the expanded one-row-per-`(venue,product_family,table_family)` evidence matrix incl.
`nt_target`; (g) gap policy with max gap frequency/duration + forbidden_claims; (h) HIP-4 quoteToken
parser-fidelity proof before any HIP-4 normalized write; (i) idempotent/create-only/no-overwrite
write-manifest format (the proven `ConditionalCatalogWriter`, §4.3); (j) owner-declared minimum
instrument-universe completeness bar (§7c).

### 5.2 Proof-acceptance precedence (F14)

> **No source family's data is projected into an NT-replayable catalog path or read by
> `BacktestNode`/`BacktestEngine` until that family holds a `SourceProofReport` with `status=accepted`
> and (for the NT-replayable subset) `nt_mapping_status=accepted`.** Per `1-backtesting-engine/
> spec.md:52-54` and E-040 (`:131`), accepted proof is required before any source becomes catalog or
> backtest input; per `backfill-source-proof-schema.md:97-98`, `canonical_s3` is forbidden until
> every referenced proof is accepted. Early capability and consumption smoke tests (Phase 0, Phase 3)
> replay **synthetic in-repo fixtures only** and stamp results `provenance=synthetic`, so they can
> never be read as provider-source backtest evidence. A provider-derived `BacktestResultContract` is
> emitted only when every `source_proof_id` feeding its catalog input is accepted.

v1's error: it ordered the proof work LAST (v1 STEP 10 / Phase 6) while STEP 6 projected provider
bytes and STEP 8 ran `BacktestNode` over them — a backtest from an un-proven source. The fact that
the write lands in a non-canonical staging prefix narrows the canonical-write violation but does NOT
cure the contract clause, which gates *backtest input* (any replay read), not just *canonical writes*.

Three rules make the class of problem impossible:

1. **Per-family proof-acceptance precedence gate.** Before any source family's data is projected into
   an NT-replayable catalog path OR read by `BacktestNode`, that family must hold a `SourceProofReport`
   with `status=accepted` and (for the NT-replayable subset) `nt_mapping_status=accepted`. The
   catalog-projection and replay steps consume an accepted-proof allowlist; a `pending`/`rejected`
   family cannot enter either path.
2. **Early smoke is SYNTHETIC-only.** The Phase-0 capability proof and the Phase-3 consumption smoke
   replay only synthetic, in-repo, deterministically generated NT-class fixtures (one `binary option`,
   one `perps/spot`, per `spec.md:33-34,37-38`). Synthetic fixtures carry
   `source_proof_id=synthetic-fixture`, `fidelity_class` set, and any `backtests/` artifact is stamped
   `result_kind=capability-smoke` / `provenance=synthetic`. No provider-derived bytes touch
   `BacktestNode` until rule 1 is satisfied.
3. **Provider-derived backtest gate.** A provider-derived `BacktestResultContract` may be emitted ONLY
   when every source family feeding its catalog input has `status=accepted` (and, for
   L2/execution-quality claims, the matching fidelity proof per `spec.md:47-51`). The run manifest's
   `source_proof_ids` (`backfill-source-proof-schema.md:90`) must all resolve to accepted records; the
   replay step asserts this before reading and fails loud otherwise.

This keeps E-002 intact: NT `ParquetDataCatalog` remains the replay/backtest projection target; raw
provider payloads remain audit input, not canonical replay input (`spec.md:101`).

### 5.3 Interim research-ready-in-staging (does NOT violate the gate)

Write normalized tables + an NT catalog under the existing NON-canonical staging prefix using
`write_mode=local_staging` + `staging_location=s3_noncanonical` (§6 F7) and `commit_state=staged`.
Every staged artifact carries the deterministic provisional `source_proof_id` (§6 F8 — never literal
`pending`), records its `fidelity_class`, and attaches `forbidden_claims` (snapshots≠native deltas,
bars≠trades, aggTrades≠native trades, fixed-depth≠full-depth) so no notebook/backtest over-claims
fidelity. Staged writes carry full Common Identity lineage and go through the
`ConditionalCatalogWriter` create-only discipline, but are NEVER promoted into canonical prefixes
until proofs are accepted. Promotion is a deferred, explicit PromotionPackage commit (§4.4).

---

## 6. Single-source taxonomies & vocabulary (resolves F3, F7, F8)

Three drifting vocabularies converge on one authority each: `backfill-table-contract.v1` is the single
source of truth for product-family names; `backfill-source-proof.v1` schema is the single source of
truth for the `write_mode` enum; the normalization library is the single place that derives both
`product_family` and `nt_instrument_id`. No other file redefines these; every other file points here.

### 6.1 F3 — Binance product-family taxonomy: one taxonomy, derived at normalize

**Authority.** The canonical Binance product families are exactly the four named at
`backfill-table-contract.md:179-180` and mirrored at `backfill-evidence-matrix.v1.toml:60`:
`usd_m_perpetual`, `usd_m_delivery`, `coin_m_perpetual`, `coin_m_delivery` (plus `spot`; Binance
options stay `excluded_from_current_scope`). These four — not `futures_um`/`futures_cm`, not
`usd_m_perpetual_or_delivery` — are the values written to the `product_family` column and to
`canonical_instrument_key = <venue>/<product_family>/<instrument_id>` (`contract:55`).

**Why three vocabularies exist (do not collapse blindly).** They live at three layers; only ONE is
canonical:

| Layer | Vocabulary | Source | Role |
| --- | --- | --- | --- |
| Raw S3 / Data Vision physical | `futures_um`, `futures_cm` | `backfill_binance_to_s3.py:42,46` → roots `data/futures/um`, `data/futures/cm` (`:131-138`) | **Raw partition only.** Binance's own archive path layout (USD-M vs COIN-M margin), legitimately coarser. Stays as the `raw/.../product=` partition key; NOT the normalized `product_family`. |
| Source-binding instrument fetch | `usd_m_perpetual_or_delivery`, `coin_m_perpetual_or_delivery` | `backfill-source-bindings.v1.toml:24,37` | A single `exchangeInfo` endpoint returns BOTH perpetuals and delivery, so the *fetch* cannot pre-split. An acquisition grouping, not a normalized family. |
| Normalized / canonical | the four canonical values | `contract:179-180` | **Canonical `product_family`.** The only vocabulary that may appear in normalized tables, `canonical_instrument_key`, partition layout, evidence-matrix rows, and source proofs. |

**Derivation rule (normalize step, Phase 5 Binance).** `product_family` is derived per-instrument
from `contractType` joined with the margin class, NOT carried through from the raw partition:

1. Margin class from the raw partition: `futures_um → usd_m`, `futures_cm → coin_m`, `spot → spot`
   (terminal).
2. Suffix from Binance `contractType`, captured raw at `backfill_binance_to_s3.py:249` and inferred
   for archive-only symbols from the `_PERP` suffix at `:300-304`:
   - `contractType == "PERPETUAL"` → suffix `perpetual`
   - `contractType` in the dated-delivery set (`CURRENT_QUARTER`, `NEXT_QUARTER`, `CURRENT_MONTH`,
     `NEXT_MONTH`, and the generic `DELIVERY` the archive inferrer emits at `:304`) → suffix `delivery`
3. `product_family = f"{margin_class}_{suffix}"` → one of the four canonical values.
4. `product_category` (`contract:53`) is set *consistently with* the derived suffix: `perpetual →
   product_category=perpetual`, `delivery → product_category=future`. Same `contractType` source,
   never independent.

**Fail-loud guard.** If `contractType` is absent/unmappable for a futures row (the expired-symbol
hazard, `contract:177`), the normalizer MUST NOT default to a perpetual/delivery guess and MUST NOT
emit the row under a `*_um`/`*_cm` family. It fails the row to `pending_source_proof` for that
`(venue, product_family, table_family)` and records it in the universe completeness gap (§7),
per the granularity rule `contract:35-39`.

**Reconcile the binding TOML.** Rename the field on the two fetch-only instrument-universe bindings
(`backfill-source-bindings.v1.toml:24,37`) from `product_family` to `acquisition_group = "usd_m"` /
`"coin_m"`, and add `normalized_product_families = ["usd_m_perpetual","usd_m_delivery"]` /
`["coin_m_perpetual","coin_m_delivery"]` documenting that the single fetch fans out to two canonical
families at normalize. This removes the `*_or_delivery` spelling entirely. (Owner alternative: split
each endpoint into two bindings; both eliminate the spelling — see Open decision §11.3.)

**Acceptance test (ships with Phase 5 Binance).** A dated DELIVERY symbol lands in
`coin_m_delivery`/`usd_m_delivery`; a `*_PERP`/`PERPETUAL` symbol lands in
`coin_m_perpetual`/`usd_m_perpetual`; assert NO normalized row, `canonical_instrument_key`, or
partition is ever emitted with `futures_um`, `futures_cm`, `usd_m_perpetual_or_delivery`, or
`coin_m_perpetual_or_delivery`; assert `product_category` agrees with the derived suffix on every row.

### 6.2 F7 — One sanctioned `write_mode` enum; `s3_staging` is an ALIAS of `local_staging`

**Authority.** The enum is defined in exactly one place: `backfill-source-proof.v1`
(`backfill-source-proof-schema.md:87`):

```
write_mode ∈ { dry_run, local_staging, canonical_s3 }
```

**Decision: `s3_staging` / `s3_staging_only` are ALIASES of `local_staging`, NOT a fourth value.**
The contract recognizes exactly two staging-vs-canonical states: non-canonical staging
(`local_staging`) vs accepted canonical (`canonical_s3`). Where the staged bytes physically live
(local disk vs a non-canonical S3 staging prefix) is a storage-location detail, NOT a commit-state.
The coverage ledger already encodes this equivalence (`backfill_coverage_ledger.py:285-293` accepts
any manifest whose `write_mode` is NOT in `(local_staging, dry_run)` as an S3-staging binding, proven
by bound `s3_uri` payloads). Adding a fourth value would re-introduce the dual-path the gate prevents.

Therefore: the single sanctioned enum stays three-valued. The "is it on S3?" fact is recorded in a
separate, additive manifest field (NOT a new `write_mode`): `staging_location ∈ { local,
s3_noncanonical }`. `canonical_s3` remains the only value that asserts a canonical, gate-passed write.

**Migration list (exhaustive, grep-verified):**

| File:line | Current value | Migrate to |
| --- | --- | --- |
| `scripts/backfill_archive_objects_to_s3.py:136` | `s3_staging` | `write_mode="local_staging"`, `staging_location="s3_noncanonical"` |
| `scripts/backfill_binance_to_s3.py:840` | `s3_staging` | same |
| `scripts/backfill_accept_staged_objects.py:209` | `s3_staging` | same |
| `scripts/backfill_accept_staged_objects.py:330` | `s3_staging` | same |
| `scripts/backfill_okx_to_s3.py:614` | `s3_staging` | same |
| `scripts/backfill_hyperliquid_hip4_to_s3.py:762` | `s3_staging` | same |
| `scripts/backfill_hyperliquid_hip3_to_s3.py:764` | `s3_staging` | same |
| `scripts/backfill_bybit_to_s3.py:985` | `s3_staging_only` | same |
| `scripts/backfill_hyperliquid_core_to_s3.py:749` | `s3_staging_only` | same |
| `scripts/backfill_deribit_to_s3.py:965-970` | (no top-level `write_mode`; only `write_policy.staging_only=True`) | add `write_mode="local_staging"`, `staging_location="s3_noncanonical"` so it stops being a third "(unset)" path the ledger special-cases |
| `scripts/backfill_archive_objects.py:159` | `local_staging` | already sanctioned; add `staging_location="local"` |
| `scripts/backfill_source_proof.py:382` | `local_staging` | already sanctioned; add `staging_location="local"` |

**Validation test (Phase 1 write-manifest task).** A schema-validation test rejects any manifest whose
`write_mode ∉ {dry_run, local_staging, canonical_s3}` and asserts `s3_staging`/`s3_staging_only`/
missing-mode all fail; a second test asserts the coverage ledger no longer needs the `write_mode not
in (...)` heuristic (the `(unset)`/`s3_staging` branches become dead and are removed).

### 6.3 F8 — Deterministic provisional `source_proof_id` + explicit `nt_instrument_id` rule

**Problem.** `contract:63` requires `source_proof_id` per row and `backfill-source-proof-schema.md:18`
requires it to be a "stable id"; v1 stamped the literal `pending` (`v1:70,227`). A literal `pending`
is not stable — two different unproven sources collide on it and accepted proofs can never be
back-linked. Separately, `nt_instrument_id` is `string, nullable` (`contract:57`) with NO population
rule for the NT-replayable subset.

**Provisional `source_proof_id` scheme (deterministic, never bare `pending`).** Mint one provisional
id per `(venue, product_family, table_family)` — the grain the gate already requires one
`SourceProofReport` per (`contract:60-61,303`):

```
source_proof_id = "sp:" + <contract_version> + ":" + <venue> + "/" + <product_family> + "/" + <table_family> + ":v0-pending"
```

- The `v0-pending` segment maps to the schema's `status=pending` + `source_proof_version`
  (`backfill-source-proof-schema.md:17,19`). `v0` denotes "no accepted version yet"; the first
  accepted proof is `v1` and supersedes per the immutability rule (`:47-48`).
- Deterministic (pure function of the contract triple), so re-running a staging script produces the
  identical id (create-only/idempotency friendly).
- Back-linkable: on acceptance the row's id is rewritten from `…:v0-pending` to `…:v1` (a new
  immutable id superseding the prior), and lineage is preserved because both ids share the
  `<venue>/<product_family>/<table_family>` stem.
- Because `product_family` now comes from the F3-canonical four-family set, a Binance futures
  provisional id is e.g. `sp:backfill-table-contract.v1:binance/usd_m_perpetual/funding_rates:v0-pending`
  — never `futures_um`.

The literal `pending` token is removed from the plan (was `v1:70,227`) and replaced with this scheme.

**`nt_instrument_id` population rule (NT-replayable subset).** Bind population to the schema's existing
`nt_mapping_status` field (`backfill-source-proof-schema.md:38`):

1. `nt_instrument_id` is populated (non-null) ONLY when the governing source proof has
   `nt_mapping_status = accepted` AND the row's `table_family` maps to a Tier A/B NT prefix (§2.2:
   `trades`, `quotes`, `order_book_deltas`, `order_book_depths`, `bars`, `index_prices`, `mark_prices`,
   `funding_rate_update`, `instrument_status`, `instrument_closes`, `instruments`). For all Tier C
   research-only tables `nt_instrument_id` stays NULL (the contract types it nullable).
2. When `nt_mapping_status ∈ {pending, rejected, not_applicable}`, `nt_instrument_id` is NULL. A row
   in an NT-replayable table family with `nt_mapping_status != accepted` MUST NOT be handed to
   `BacktestNode` — it stays research-only Parquet until the mapping is accepted.
3. The NT instrument id itself is the NT-native id produced by the Common Identity builder from
   `(venue, product_family, instrument_id)` (`contract:54-55`); this design does NOT invent an NT id
   format — it defers the exact string to the Common Identity normalization library (Phase 1) and only
   fixes *when* the column is populated.
4. **Phase 3 / Phase 6 precondition.** Any BacktestNode read-back fixture in an NT-replayable family
   MUST have `nt_mapping_status=accepted` and a non-null `nt_instrument_id`; the smoke test asserts
   this as a hard precondition and fails loud otherwise. (Synthetic fixtures use
   `source_proof_id=synthetic-fixture` with a synthetic accepted nt_mapping, §5.2.)

**Test.** A row whose proof is `nt_mapping_status=pending` (or whose table_family is Tier C) has
`nt_instrument_id IS NULL`; a row whose proof is `accepted` and table_family is Tier A/B has a
non-null id; provisional `source_proof_id` round-trips deterministically across two runs and rewrites
to `…:v1` on acceptance.

---

## 7. Per-venue fidelity corrections (resolves F4, F5, F6, F13)

The shared rule these enforce is the contract's no-downgrade clause: "No worker may silently replace a
missing granular table with a weaker aggregate... derived snapshot deltas do not satisfy native
order-book deltas" (`contract:35-39`). The single source of truth for "what was actually staged and
what family it is" is the **accepted acceptance-manifest / coverage-ledger**, not the binding TOML and
not the key-path strings.

### 7.1 F4 — OKX `order_book_400` is a derived snapshot family, NOT native `order_book_deltas`

Replace the OKX Phase 5 bullet and the evidence-matrix OKX `order_book_deltas` claim
(`backfill-evidence-matrix.v1.toml:76,86`):

- **OKX** — trades(native), candles→bars, funding_rates, instrument_id from in-row `instId` not the
  partition selector. The OKX `order_book_400` archive (staged by `backfill_okx_to_s3.py`,
  `MODULE_ORDER_BOOK_400 = "4"`) is a daily fixed-400-level book file. It does NOT carry a native
  per-update L2 sequence id; the script stages the raw daily payload only. Frame ordering recovers
  only from the file line ordinal.
  - **Snapshot frames → `order_book_snapshots_fixed_depth`** (Tier C; `contract:123-125`), plus the
    NT-compatible top-10 projection to Tier A `order_book_depths` (`contract:125-127`).
  - **Reconstructed update stream → `order_book_snapshot_deltas`** (Tier C) with an explicit,
    source-proof-named derivation rule `okx_400level_snapshot_clear_add_then_update_delete`, ordering
    key = file line ordinal (NOT a native seqId). The contract requires this table to "name the
    derivation rule" and states it "cannot satisfy native `order_book_deltas`" (`contract:116-118`).
  - **FORBIDDEN_CLAIM (OKX):** `okx order_book_400 MUST NOT populate order_book_deltas`. Native
    `order_book_deltas` is reserved for native L2/L3 updates only (`contract:114-115`).
- **Evidence-matrix downgrade:** remove `order_book_deltas` from OKX `directly_backfillable` for both
  `["spot","swap","future"]` and `["option"]`; move it to `pending_source_proof` keyed
  `okx_native_seqid_l2_archive`. Keep `order_book_snapshots_fixed_depth` in `directly_backfillable`;
  add `order_book_snapshot_deltas` to `directly_backfillable` for the snapshot-derived family. The
  existing "Official 400-level L2 archive may support one-year replay" note is retained only for the
  snapshot families, not for native deltas.

### 7.2 F5 — Polymarket single authoritative family = accepted manifest = `order_book_snapshots_fixed_depth`

The acceptance manifest / coverage ledger is the single authoritative Polymarket family source. Every
accepted Polymarket manifest declares exactly one family, `order_book_snapshots_fixed_depth` (page1
`archive-objects-run-f6be9ae08fd93d9b`, acceptance `archive-s3-accept-page1-f6be9ae08fd93d9b`,
streaming `polymarket-pmxt-v2-streaming`). There is NO accepted `bars` family and NO accepted
`snapshot_deltas` family. v1's relabel to `order_book_snapshots_full_depth_l2` is unproven.

Replace the Polymarket Phase 5 bullet:

- **Polymarket (PMXT source)** — reported venue name `Polymarket (PMXT source)`; never report PMXT as
  a separate venue (preserve `pmxt`/`PMXT` only in URLs, source-proof names, raw lineage). Demux the
  multiplexed hourly Parquet by `event_type`:
  - `last_trade_price → trades` (native, `trade_source_type=native`)
  - `book → order_book_snapshots_fixed_depth` (Tier C) + top-10 projection to `order_book_depths`
    (Tier A). The fixed-depth captured depth is NOT proven to be the source maximum, so it MUST NOT be
    promoted to `order_book_snapshots_full` (`contract:119-122`) and MUST NOT be relabeled
    `full_depth_l2`.
  - `price_change → quotes` (`quote_source_type=reconstructed_top_of_book`, NOT native deltas)
  - `tick_size_change → instrument_status`
  - **Exact staged host / prefix:** source host is `archive.pmxt.dev/Polymarket/v2` (binding
    `polymarket-parquet-archive-index`, `backfill-source-bindings.v1.toml:287`; evidence E-030 /
    `evidence.md:55`). The earlier `r2v2.pmxt.dev` host string is **unconfirmed** and is removed.
    Staged S3 prefixes are `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-page1/`
    and `.../polymarket-pmxt-v2-streaming/`.
  - **FORBIDDEN_CLAIM (Polymarket):** `polymarket order_book_snapshots_fixed_depth MUST NOT satisfy
    order_book_deltas or order_book_snapshots_full` (`contract:284-286`).

**Reconcile the binding TOML** (`backfill-source-bindings.v1.toml:294`): change
`table_families = ["order_book_snapshots_fixed_depth", "order_book_snapshot_deltas", "bars"]` to
`table_families = ["order_book_snapshots_fixed_depth"]`. `bars` and `order_book_snapshot_deltas` are
NOT present in any accepted Polymarket manifest; re-add ONLY when a future demux + acceptance manifest
proves the PMXT Parquet carries that `event_type`.

**Evidence-matrix reconcile** (`backfill-evidence-matrix.v1.toml:180`): keep
`order_book_snapshots_fixed_depth` as the only `owner_archive_backfillable` book/snapshot family that
is manifest-backed today; move `order_book_snapshot_deltas`, `bars`, and `trades` to
`pending_source_proof` keyed `polymarket_pmxt_event_type_demux` until a demux + acceptance manifest
proves each `event_type` is present and schema-mapped. `order_book_snapshots_full`/`full_depth_l2`
stays `pending_source_proof` keyed `polymarket_max_depth_proof`. `order_book_deltas` stays
`vendor_or_forward_capture_only` (already correct, line 183).

### 7.3 F6 — Deribit `get_index_price` MUST NOT populate `index_prices.event_time`

Replace the Deribit Phase 5 bullet's index note with an enforced forbidden_claim + fail-loud guard:

- **Deribit** — trades_seq_history→trades(native, `source_sequence=trade_seq`); bars_1m→bars;
  funding_history→funding_rates **+ index_prices** (the `index_price` field on each
  `get_funding_rate_history` row IS the index-history source); instrument_metadata→instruments;
  settlements/delivery→settlements/delivery_prices; historical_volatility. perpetual→
  `product_family=future`, `product_category=perpetual`.
  - **FORBIDDEN_CLAIM (Deribit):** `deribit get_index_price MUST NOT populate index_prices.event_time`.
    The `index` family calls `get_index_price` with `{"index_name": index_name}` only
    (`backfill_deribit_to_s3.py:743`); the response is a point dict (`index_price`,
    `estimated_delivery_price`) with NO event timestamp — `result_rows()` returns `[]` and
    `coverage_from_rows()` returns `{returned_start_utc: None, returned_end_utc: None}` (row_times key
    set `("timestamp","tick","time")`, line 138). The only timestamps are REST transport `usIn`/`usOut`,
    and the contract forbids REST response time in `event_time` (`contract:58`).
  - **Index history source:** `index_prices` rows come ONLY from `funding_history`
    (`get_funding_rate_history`, lines 532-535), whose per-row `index_price` carries the row
    `timestamp` as event_time (matches `backfill-evidence-matrix.v1.toml:131`).
  - **Normalization-library fail-loud guard (Phase 1):** the Common-Identity fill library MUST reject
    (raise, not skip) any `index_prices` row whose lineage `source_family == "index"`. Reusable
    signature: a per-(family) `event_time_source` allowlist — a family flagged
    `event_time_source=none` (snapshot/current-probe only) may never emit a row into any time-series
    table.
  - **Apply the SAME guard to all snapshot-only / capture_time-only families (class fix, not instance
    patch).** Families with `event_time_source=none` that MUST be rejected from time-series tables:
    Deribit `index`, `mark_price_history_probe`, `trades_recent_probe`; Hyperliquid
    `meta`/`metaAndAssetCtxs`/`spotMeta`/`spotMetaAndAssetCtxs` and the HIP-3 current-snapshot contexts
    (confirm the exact HIP-3 set against `backfill_hyperliquid_hip3_to_s3.py` — Open decision §11.5).
    The guard is one shared assertion keyed off the per-family `event_time_source` table, so adding a
    new snapshot-only family is a one-line table entry.

### 7.4 F13 — HL-core has NO native trade tape this tranche

Replace the Hyperliquid-core Phase 5 bullet's deferral language with a plain not-satisfiable statement
+ forbidden_claim:

- **Hyperliquid-core** — `l2Book → order_book_snapshots_fixed_depth(20)` (Tier C) + top-10 projection
  to `order_book_depths` (Tier A) (event_time from inner `raw.data.time`; outer `time` is
  `capture_time`); `fundingHistory → funding_rates`; `asset_ctxs` split into
  mark/index/open_interest/funding/premium (impact/mid as `reconstructed_top_of_book`); `meta →
  instruments`.
  - **FORBIDDEN_CLAIM (HL-core):** `no HL-core native trade tape this tranche`. The accepted HL-core
    manifest families are exactly `asset_ctxs`, `fundingHistory`, `l2Book`, `meta`,
    `metaAndAssetCtxs`, `spotMeta`, `spotMetaAndAssetCtxs`
    (`oneoff-seven-token-backfill-status-2026-06-02.md:373-380`) — no `trades`, no `node_fills`, no
    `node_fills_by_block`. In `backfill_hyperliquid_core_to_s3.py`, `node_trades` is a listing-only
    probe explicitly NOT uploaded (lines 1264-1266; `remaining_scope.node_trades` line 1282), and
    `node_fills_by_block` is only an optional `--node-family` choice (line 1338) gated behind a schema
    probe (`schema_probe_lz4`, lines 1236-1239).
  - **Evidence-matrix:** HL-core `trades` MUST move out of any backfillable column into
    `pending_source_proof` keyed `node_fills_trade_dedupe` (the matrix already lists
    `node_fills_trade_dedupe` under `pending_source_proof`, `backfill-evidence-matrix.v1.toml:150`;
    additionally relocate `trades` from `owner_archive_backfillable` line 148 into the same pending
    key).
  - **`node_fills_by_block` is a separate gated future task** (Open decision §11.5): it requires (a)
    the lz4 schema probe to PASS, (b) a dedupe/completeness proof, and (c) a requester-pays
    cost/egress estimate before any HL-core `trades` write. It MUST NOT appear as a Phase-5
    deliverable implying it will land in this tranche.

---

## 8. Instrument universe — best-effort + completeness gap record (resolves F9)

**Reality the design must encode.** Every staged instrument-universe source is a CURRENT snapshot,
not a window-historical universe. The bindings prove this: Deribit spot/future/option all query
`expired=false` and carry `evidence_state = "bounded_or_current_only"`
(`backfill-source-bindings.v1.toml:188,191-192` / `:202,205-206` / `:216,219-220`); HIP-4 outcome is
`bounded_or_current_only` (`:276-277`); Binance/Bybit/HL meta endpoints return only currently-listed
instruments. A delisted/expired instrument absent from every staged object cannot be recovered from
these snapshots. The contract requires the universe to include "instruments active at any point in the
requested window, not only instruments active on the execution date" (`contract:92-93,175-177`), so
the current snapshot CANNOT satisfy that requirement as written. Phase 2 is therefore demoted from
"window-complete universe" to **best-effort universe + an explicit, machine-readable completeness gap
record**.

**(a) Deliverable demotion.**
- Emit `instrument_universe_snapshots` as a BEST-EFFORT universe: the union of (1) every instrument
  observed in a current snapshot binding, plus (2) every distinct instrument symbol observed in any
  staged market-data object for the window (archive symbols already enumerated by the per-venue
  scripts, e.g. Binance `universe["archive_symbols"]`, `backfill_binance_to_s3.py:464`). This recovers
  delisted instruments that left a data footprint; it CANNOT recover instruments that left no
  footprint.
- Each universe row carries `discovery_basis ∈ {current_snapshot, staged_data_footprint,
  symbol_shape_parsed}` naming exactly how that instrument entered — no silent merge.
- The per-venue `source_proof_id` governing the universe is set so its `evidence_state` is
  `bounded_or_current_only` for every venue whose only listing source is a current snapshot
  (`contract:24-25`). The matching source-proof `instrument_universe` check
  (`backfill-source-proof-schema.md:59`) is PENDING/FAIL — not PASS — and the `coverage` check
  (`:60-61`) is marked `bounded/current only` until an `expired=true` (or equivalent venue-historical)
  listing proof passes.
- The ingest manifest's `instrument_universe_records` (`backfill-source-proof-schema.md:91-93`) gains
  a `completeness` block per `(venue, product_family)`: `{ basis_counts, expired_listing_proof:
  present|absent, symbols_in_staged_data_without_universe_metadata, completeness_class }`, sourced from
  the gap data the scripts already compute (`metadata_gaps` →
  `archive_symbols_without_exchange_info_metadata`, `backfill_binance_to_s3.py:455,513-523,873`).
  `completeness_class ∈ {window_complete_proven, best_effort_current_plus_footprint,
  current_snapshot_only}`.
- The source proof records the gap explicitly via `forbidden_claims`
  (`backfill-source-proof-schema.md:42`), e.g. "MUST NOT claim window-complete instrument universe for
  `<venue>/<product_family>`; staged listing is `bounded_or_current_only` and omits instruments
  delisted before the snapshot with no staged-data footprint."

**(b) DECLARED, source-proof-cited symbol-shape parser (replaces silent heuristic inference).**
- The current path is silent heuristic inference: `infer_archive_symbol_metadata`
  (`backfill_binance_to_s3.py:284-327`) only sets `contract_type` when the suffix contains `_`
  (`:302-304`) and otherwise returns `None` (`:327`); a non-matching symbol is dropped into
  `family_metadata_gaps` (`:474-477`) and skipped — exactly F9's failure.
- Replace it with a DECLARED symbol-shape parser per `(venue, product_family)` whose grammar lives in
  config, is cited by `source_proof_id`, and whose output is recorded as an evidence-bearing field:
  - Maps a venue-native expired/active symbol → `product_category` (`contract:53` enum
    `spot|perpetual|future|option|prediction_market_outcome`) using ONLY a declared rule (e.g. Binance
    USD-M `<BASE><QUOTE>_PERP` → `perpetual`; `<BASE><QUOTE>_<YYMMDD>` → `future`). The rule set is
    named in the source proof's `schema_sample_uri`/parser proof and its identity flows into
    `transform_hash`.
  - `product_category` resolved by the parser is tagged `category_source =
    "symbol_shape_parser:<rule_id>"`, distinguishing it from `category_source = "contractType"`. When
    `contractType` is present it wins; the parser is consulted only for expired contracts where
    `contractType` is absent.
  - **Any instrument the declared parser cannot resolve is a `forbidden_claim`, never silently
    dropped.** A universe row is still emitted with `product_category = null` and `category_source =
    "unresolved"`, plus a `forbidden_claim` on the governing source proof: "MUST NOT assign
    `product_category` to `<canonical_instrument_key>`; symbol shape unresolved by declared parser
    `<rule_id>` and `contractType` absent."
  - The source-proof `schema` check (`backfill-source-proof-schema.md:58`) covers the parser grammar;
    the `instrument_universe` check (`:59`) PASSes only when the unresolved-`product_category` count is
    within the owner-approved bar (see (c)). Bybit's `product_category` continues to derive from
    `contractType` (`contract:188`); the symbol-shape parser is the declared fallback for expired Bybit
    contracts.

**(c) Minimum acceptable universe completeness = gate-blocking owner decision.**
- New gate item (extends §5.1 list): the owner must DECLARE, per `(venue, product_family)`, the
  minimum acceptable `completeness_class` and the maximum tolerated count/fraction of (i) instruments
  with `discovery_basis = current_snapshot` only and (ii) instruments with `category_source =
  "unresolved"`. Recorded as a gate token decision, not a script default.
- Until that bar is declared and met, the `instrument_universe` and `coverage` source-proof checks for
  that family stay PENDING/FAIL, keeping the proof out of canonical selection
  (`backfill-source-proof-schema.md:72-73`) and forbidding `canonical_s3` promotion (`:97`). The
  deferred canonical promotion (§4.4) is blocked on this owner decision exactly like the other gate
  items.
- Open decision §11.2 ("Coverage completeness bar") is updated to state the universe-completeness bar
  is now a BLOCKING gate token, not merely an open question.

**Phase 2 task list (corrected).**
- Universe manifest = best-effort union (current snapshots + staged-data footprint), per
  venue/product_family, with `base/quote/settle/contract_type/expiry/strike/option_type/listing/
  delisting + dex_name (HIP-3) / outcome_encoding+asset_id+wire_symbol+quoteToken (HIP-4)`
  (`contract:72-77`), plus `discovery_basis`, `category_source`, `completeness_class` columns.
- Declared symbol-shape parser (config-owned grammar, source-proof-cited, `transform_hash`-bound)
  replacing `infer_archive_symbol_metadata`; emit unresolved instruments with `product_category=null`
  + a `forbidden_claim`, never drop them.
- Per-`(venue,product_family)` source proofs set `evidence_state=bounded_or_current_only` and leave
  `instrument_universe`/`coverage` checks PENDING/FAIL for current-snapshot-only venues.
- Completeness gap record in `instrument_universe_records.completeness`, fed from existing
  `metadata_gaps`.
- `instrument_status` + `instrument_closes` population; rows known only by symbol-shape inference carry
  `category_source` so downstream cannot treat inferred categories as authoritative.
- Gate item: owner-declared minimum completeness bar blocks canonical promotion.

---

## 9. Cost / scale projection (resolves F10) — gating the full one-year run

The full raw→catalog projection is gated on a costed estimate being produced and approved before the
one-year run. The single source of truth for volumes is the accepted backfill coverage ledger
(`backfill-coverage-status-2026-06-02.md:64-73`) — NOT the stale "Deribit ~35,287 objects" figure in
the v1 internal critique (which counted attempted/scanned, not accepted).

### 9.1 Per-venue accepted volume (source of truth: coverage ledger lines 64-73)

| Venue | Accepted objects | Accepted bytes | Notes |
|---|--:|--:|---|
| Binance | 4,720 | ~40.38 GiB | aggTrades/trades/klines/mark-index-premium/metrics/funding |
| OKX | 7,509 | ~79.39 GiB | includes `order_book_400` across spot/swap/future/option — per-instrument-per-day fan-out |
| Bybit | 707 | ~0.93 GiB | tick_trades + universe; REST run still LIVE (count rising) |
| Deribit | 16,605 | ~0.04 GiB | many tiny objects → worst object-count/byte ratio → highest LIST/head amplification per byte |
| Hyperliquid core | 14,649 | ~12.23 GiB | l2Book/asset_ctxs/funding/meta; `node_fills` deferred (~19 MB/object when enabled) |
| Hyperliquid HIP-3 | 548 | ~0.06 GiB | single snapshot |
| Hyperliquid HIP-4 | 247 | ~0.08 GiB | single snapshot (prediction-market) |
| Polymarket (PMXT) | 748 | ~267.12 GiB | order_book_snapshots hourly parquet; ~0.36 GiB/obj average |
| **Total (excl. Polymarket)** | **28,686** | **~122.69 GiB** | per ledger line 17 |
| **Total (incl. Polymarket)** | **~29,434** | **~390 GiB** | dominated by Polymarket bytes and Deribit object-count |

### 9.2 S3 request amplification from NT's per-write pattern (grounded in catalog.rs)

NT's `write_to_parquet` issues, per write call: (1) one `head()` existence probe (`catalog.rs:540`);
then (2) unless `skip_disjoint_check=true`, a `get_directory_intervals()` doing a full
`object_store.list()` over the target prefix (`catalog.rs:550` → `catalog.rs:2648-2680`, list at
`:2658`); then (3) one `put` (`catalog.rs:570` → `parquet.rs:197`, a plain PUT, no
`PutMode::Create`). Amplification: each output write = 1 HEAD + 1 LIST + 1 PUT. As a directory
accumulates files, the per-write LIST grows with the file count; for a partition holding D daily files,
cumulative LIST cost over a sequential one-year backfill is O(D²) enumerations worst-case. At Deribit's
16,605 tiny objects and OKX's `order_book_400` fan-out (thousands of partitions) this dominates request
cost independent of bytes.

**Note:** the `ConditionalCatalogWriter` (§4.3) replaces NT's `put` with `PutMode::Create`; the
following mitigation applies to the encoder/list path NT still drives.

**Mitigation baked into the projection:** write each (type, instrument) interval directory in a single
pass with intervals pre-sorted so disjointness holds, and pass `skip_disjoint_check=true` on the bulk
path to eliminate the per-write LIST (`catalog.rs:549`), relying on the `ConditionalCatalogWriter`
create-only guarantee for dedupe. The `head()` probe is bypassed because writes go through the external
layer, not NT's `write_to_parquet`; budget the irreducible per-object `PutMode::Create` HEAD/PUT
explicitly.

### 9.3 Requester-pays egress for Hyperliquid archives (hard NT constraint)

The HL archive is requester-pays (`contract:215,274`; coverage doc line 99 records the lag).
`object_store` supports it via `with_request_payer(true)` (`object_store-0.13.2/src/aws/builder.rs:191,
442-444,1063-1064`). **However, NT's `create_s3_store` does NOT pass it through** — its
`storage_options` match handles only `endpoint_url`/`region`/`access_key_id`/`secret_access_key`/
`session_token`/`allow_http`; any `request_payer` key falls into the `_ =>` "Unknown S3 storage option"
arm and is silently dropped (`crates/persistence/src/parquet.rs:692-714`). **Consequence:** NT's
catalog reader/writer CANNOT issue `x-amz-request-payer: requester`, so HL requester-pays archives
cannot be read directly through the NT catalog. The projection must **pre-stage HL raw archives into our
own (requester-owned) bucket** via a separate copy step that DOES set requester-pays (the existing
backfill scripts / a direct `object_store` client with `with_request_payer(true)`), then project from
the owned copy. Budget the HL requester-pays GET egress as a one-time pre-stage cost (HL-core ~12.23 GiB
accepted; `node_fills`, if later enabled, adds ~19 MB/object — deferred). This pre-stage requirement is
a gating line item, not an optimization.

### 9.4 Partitioning / parallelism strategy

- **Partition** by `(venue, product_family, table_family, instrument)` matching NT's `make_path` layout
  (`data/{type_name}/{safe_instrument_id}`, `catalog.rs:2729-2737`) so directory LISTs stay scoped to
  one instrument's interval set.
- **Parallelize** across instrument partitions (disjoint directories → no LIST contention, and the
  `ConditionalCatalogWriter` makes concurrent writers safe). Do NOT parallelize multiple writers into
  the SAME interval directory under NT's native writer (the head()-then-put race, §4.1).
- **Bulk path** uses `skip_disjoint_check=true` + one-pass-per-directory + the external conditional-PUT
  layer.

### 9.5 Gate

The full one-year projection does NOT run until this costed estimate (object counts, bytes,
HEAD/LIST/PUT request counts per venue, HL requester-pays pre-stage egress, and wall-clock under the
chosen parallelism) is produced from the ledger numbers above and approved. Tie this gate to Open
decision §11.2 (coverage completeness bar) so cost and completeness are signed off together.

---

## 10. Proof-first sequence (reordered per F14) and Phases

The capability proof and conditional-write/foundation work stay first (source-independent). The change
vs v1: **per-family SourceProofReport creation + acceptance moves to immediately BEFORE that family's
projection-to-catalog and any BacktestNode read of provider data.** Proof work is no longer a single
trailing phase; it splits into a per-family acceptance gate plus a final aggregate evidence-matrix +
canonical-promotion step.

### 10.1 Step order

| New step | Action | Maps to v1 | Why moved |
|---|---|---|---|
| **S1** | NT-catalog-on-S3 capability proof on a research-only crate (`cloud` feature, rev `6e059dc`); negative control + invalid-creds control + positive write/re-open/query — **over SYNTHETIC fixtures only** (one synthetic `binary option`, one synthetic `perps/spot`). | v1 STEP 1 (now synthetic-bound) | Source-independent; must not read provider bytes. |
| **S2** | Approve `artifact_root` URI + typed prefix schema + URI-validation tests. | v1 STEP 2 | Unchanged. |
| **S3** | Shared Common Identity normalization library (nanos multiplier table, decimal-string preservation, `canonical_instrument_key`, `transform_hash` over code+config, `raw_payload_id`, deterministic provisional `source_proof_id` plumbing) + timestamp-unit unit tests + the `event_time_source` fail-loud guard (§7.3). | v1 STEP 3 | Unchanged (source-independent). |
| **S4** | The `ConditionalCatalogWriter` create-only/no-overwrite write layer (§4.3); concurrency proof; `no_overwrite_proof`. | v1 STEP 4 (promoted to BLOCKER) | Source-independent; blocking. |
| **S5** | Best-effort instrument-universe manifest + completeness gap record (§8), per venue/product_family. | v1 STEP 5 | Feeds `instrument_universe` proof check. |
| **S6** | **Early synthetic consumption smoke** — NT `BacktestNode`/`BacktestEngine` replays the **synthetic** S1 fixtures over the staging catalog; assert a `backtests/` result stamped `provenance=synthetic`, `result_kind=capability-smoke`. Read-only Jupyter notebook consumes the same synthetic catalog. **No provider data.** | v1 STEP 8 + 9 (moved earlier; synthetic-bound) | Proves the consumption pipeline before any provider proof, without producing provider-source evidence. |
| **S7 (per source family, gating)** | Build the `SourceProofReport` (portable raw sample + SHA-256, schema sample + row counts + timestamp range, license/retention refs, `fidelity_class`, `forbidden_claims`, `gap_policy_id`) and run all `required_checks` (`backfill-source-proof-schema.md:50-73`); set `status=accepted` only when every check (incl. `nt_mapping`) passes; ambiguous/failed stays `pending`/`rejected`. HIP-4 quoteToken parser-fidelity proof is part of the HIP-4 family's `schema`/`nt_mapping` checks and must pass before HIP-4 acceptance. | v1 STEP 7 (HIP-4) + v1 STEP 10 proof half (split per-family, moved BEFORE projection) | Enforces `spec.md:52-54`: accepted proof precedes catalog/backtest use. |
| **S8 (per accepted family)** | Raw→NT-class + contract-table projection to STAGING via `ConditionalCatalogWriter` — **only for families whose S7 proof is `accepted`**. Each staged row carries the accepted `source_proof_id`, `fidelity_class`, `forbidden_claims`. | v1 STEP 6 (now gated by S7) | Projection consumes the accepted-proof allowlist. |
| **S9 (per accepted family)** | Provider-derived consumption — BacktestNode replay over the accepted-family staging catalog (Tier A only) and read-only notebook consumption; emit `BacktestResultContract` only when all `source_proof_ids` resolve to `accepted` (`backfill-source-proof-schema.md:90`). | (new gate over v1 STEP 8/9 for provider data) | Provider-source result requires accepted-proof sources. |
| **S10** | Aggregate evidence matrix (one row per `(venue, product_family, table_family)`, incl. `nt_target`) + gap policy with `forbidden_claims`; on full acceptance, the deferred canonical promotion via PromotionPackage + pointer CAS (§4.4). | v1 STEP 10 aggregate/promotion half | `canonical_s3` forbidden until every referenced proof is accepted. |

### 10.2 Phases

- **Phase 0 — Catalog capability proof + write layer (GATING), synthetic-only** (= S1, S4). Capability-
  proof fixtures are SYNTHETIC, in-repo, deterministically generated NT-class rows; this phase MUST NOT
  read any provider-derived payload. Any emitted result is stamped `provenance=synthetic`. Includes the
  Cargo separate-workspace isolation (§11-iso/§12) and the
  `ConditionalCatalogWriter` concurrency proof. Sub-tasks:
  - **0.0 Structural isolation (F12, §12).** Cloud-enabled projector in a SEPARATE workspace/lockfile.
  - **0.1 Falsifiable `cargo tree -e features` build guard (F12, §12).**
  - **0.2 Negative control 1 — feature gate (F12).** No-cloud build → `from_uri` on `s3://` hits the
    `parquet.rs:539` bail.
  - **0.3 Negative control 2 — credential attribution (F11, §11-cred).** Cloud ON + scrubbed
    ambient creds (env + IMDS) + no/invalid `storage_options` creds → write FAILS; same write with valid
    SSM creds → SUCCEEDS.
  - **0.4 Positive proof.** SSM-resolved creds → write two SYNTHETIC fixtures → re-open → `query_files`
    → assert; stamp `NtCapabilityProof` (exact `storage_options` key set consumed, credential source =
    SSM).
  - **0.W `ConditionalCatalogWriter` BLOCKER (F2, §4.3).** Create-only, content+transform-hash-keyed,
    with the concurrency proof; absence blocks all writes.
- **Phase 1 — Artifact root + write-discipline foundations** (= S2, S3). TOML/config-owned
  artifact_root + typed prefix schema (`raw/`, `normalized/<schema_version>/`, `nt-catalog/`,
  `source-proofs/`, `backtests/`, `research-analytics/v1/{datasets,feature-tables,experiment-results,
  promotion-packages}/`, `artifact-index/v1/{events,snapshots,pointers}/kind=<artifact_kind>/`); single
  root, no per-type knobs. URI-validation tests. Common Identity fill library (per-(product,family)
  event_time→nanos multiplier table, NO single hardcoded multiplier, NEVER REST response time;
  decimal-string preservation; `canonical_instrument_key`; lineage `raw_payload_id`/`transform_hash`/
  deterministic `source_proof_id`; the `event_time_source` allowlist guard §7.3). Write-manifest format
  with `write_mode ∈ {dry_run, local_staging, canonical_s3}` + additive `staging_location` (§6.2) +
  `no_overwrite_proof`.
- **Phase 2 — Instrument universe (best-effort + gap record)** (= S5, §8).
- **Phase 3 — Synthetic consumption smoke** (= S6). NT `BacktestNode` + notebook over the SYNTHETIC
  catalog only. Provider replay is deferred to Phase 6.
- **Phase 4 — Per-family source proofs + acceptance (GATING)** (= S7). For each source family produce
  the `SourceProofReport`, run `required_checks`, accept only on all-pass; HIP-4 quoteToken fidelity is
  a required check inside the HIP-4 family proof. No family is projected until its proof is `accepted`.
  HIP-4 outcomeMeta identity (encoding=10·outcome+side, wire_symbol=#<encoding>,
  asset_id=100000000+encoding; preserve raw quoteToken verbatim) and the quoteToken parser-fidelity
  proof harness live here.
- **Phase 5 — Per-accepted-family projection to staging** (= S8). Project raw→NT-class for accepted
  families only via `ConditionalCatalogWriter`; stamp `source_proof_id`/`fidelity_class`/
  `forbidden_claims` on every staged row. Per-venue mappers (corrected): Binance (§6.1 derivation),
  OKX (§7.1), Bybit (spot vs derivatives tick_trades schema branch; mark/index/premium klines→`bars`
  per §2.4; funding→funding_rate_update; open_interest/historical_volatility→Tier C;
  product_category via contractType), Deribit (§7.3), Hyperliquid-core (§7.4), Hyperliquid HIP-3
  (fundingHistory→funding_rate_update; candleSnapshot→bars; meta/allPerpMetas→instruments
  current-snapshot; preserve dex_name + synthetic asset_id as derived join helper), Polymarket (§7.2),
  HIP-4 market-data (gated; candleSnapshot→bars, recentTrades→trades(recent-only),
  l2Book→order_book_snapshots_fixed_depth + quotes(reconstructed_top_of_book); forbidden_claims
  attached).
- **Phase 6 — Provider-derived consumption + canonical promotion (GATING)** (= S9 + S10). Provider
  BacktestNode replay (Tier A only) and notebook consumption, emitting `BacktestResultContract` only
  when all manifest `source_proof_ids` are accepted; then the aggregate evidence matrix (incl.
  `nt_target`), gap policy, and the deferred canonical promotion via explicit PromotionPackage + pointer
  CAS (§4.4) — NEVER a prefix re-point.

---

## 11. Credential negative control + Cargo isolation + cost (F10, F11, F12)

### 11-iso. CRITICAL STRUCTURAL CORRECTION (invalidates v1's F12 premise)

v1 assumed a "research/backtest crate edge" inside a workspace where `cloud` could be enabled "only on
the research crate". **No such edge exists today.** `bolt-v2/Cargo.toml` is a single-package binary
crate (`Cargo.toml:1-2`), NOT a virtual workspace (`find` returns only `./Cargo.toml`). It already
lists `nautilus-persistence` as a direct dependency (`Cargo.toml:39`). Adding `features = ["cloud"]`
to that one line enables `object_store/aws` directly in the live `bolt-v2` LiveNode binary. F12 is the
**default outcome** of editing the existing manifest in place. The fix below makes isolation structural
and falsifiable.

### 12. Cargo separate-workspace isolation (resolves F12)

- The cloud-enabled catalog projection MUST NOT be a feature-flag on the existing `bolt-v2` package.
  Cargo features are **additive and unified per dependency-resolution graph**; within one package (or
  one workspace sharing a lockfile/resolution), `nautilus-persistence/cloud` enabled anywhere unifies
  into the live binary.
- Create the research/backtest projector as its own package **with its own workspace root and its own
  lockfile**, outside `bolt-v2`'s dependency resolution. Concretely: a sibling directory (e.g.
  `tools/catalog-projector/`) carrying its own `[workspace]` + `Cargo.lock`, declared in the live
  `bolt-v2/Cargo.toml` via `[workspace] exclude` so it never joins the live binary's resolution graph;
  OR a fully separate repo/path checkout. It depends on `nautilus-persistence = { rev = "6e059dc...",
  features = ["cloud"] }` (`cloud = object_store/{aws,azure,gcp,http}`, `crates/persistence/
  Cargo.toml:25-30`). The live `bolt-v2` package keeps its dependency line (`Cargo.toml:39`) with NO
  `cloud` feature.
- The cloud-enabled crate and the live binary are NEVER co-members of one virtual workspace and NEVER
  share a `Cargo.lock`. This is the only mechanism that survives additive feature unification — "enable
  cloud only on the research edge" within a shared graph does NOT.

**0.1 Falsifiable build assertion (verified empirically).** Add a CI gate that FAILS if the live
target's resolution graph contains the AWS cloud surface:
- Baseline (today, must stay true): `cargo tree -e features` in `bolt-v2` shows `object_store v0.13.2`
  present transitively (datafusion → nautilus-persistence default → bolt-v2) with features only
  `fs`/`tokio`/`walkdir` — the `aws` feature NOT enabled.
- The guard runs `cargo tree -e features` against the live `bolt-v2` target and FAILS if it finds
  either (a) `object_store feature "aws"`, or (b) `nautilus-persistence feature "cloud"` (or `"python"`,
  which transitively enables `cloud`, `crates/persistence/Cargo.toml:39-49`). It keys on the **feature**,
  not the crate's presence, because `object_store` is unavoidably in the tree via datafusion. A green
  guard is the empirical proof that no path pulled cloud/aws into the live LiveNode.

### 11-cred. Credential negative control (resolves F11, empirically proves rule-6 SSM-only)

- **0.2 Negative control 1 — feature gate (unchanged):** a no-cloud build calling `from_uri` on an
  `s3://` URI must hit the bail `"Cloud storage support requires the 'cloud' feature: {uri}"`
  (`crates/persistence/src/parquet.rs:539`, the `#[cfg(not(feature = "cloud"))]` arm `:530-540`). This
  proves cloud is feature-driven — but NOT that SSM creds (vs ambient creds) drive the positive write.
- **0.3 Negative control 2 — credential attribution:** the no-cloud bail proves nothing about WHERE the
  positive write's credentials came from. With cloud ON, the write could succeed via ambient AWS env
  vars, an AWS profile, or an EC2 instance profile (IMDS) rather than the SSM-injected
  `storage_options`. Add a second control:
  - **Setup:** cloud ON; `ParquetDataCatalog::from_uri(s3://…, storage_options)` where
    `storage_options` carries NO `access_key_id`/`secret_access_key`/`session_token` (or carries
    deliberately invalid ones). Simultaneously SCRUB every ambient AWS credential source.
  - **Why scrubbing both env AND IMDS is required (grounded):** NT's `create_s3_store` uses
    `AmazonS3Builder::new()` (not `from_env()`) and only calls `with_access_key_id`/
    `with_secret_access_key`/`with_token`/`with_endpoint`/`with_region`/`with_allow_http` from
    `storage_options` (`parquet.rs:687-718`). So env-var creds are NOT auto-loaded by NT. HOWEVER,
    `AmazonS3Builder::build()` does NOT fail on missing static creds; when both keys are `None` it
    falls through WebIdentity / Task / EKS Pod / `InstanceCredentialProvider` (IMDS at
    `http://169.254.169.254`) (`object_store-0.13.2/src/aws/builder.rs:1090-1179`, default endpoint
    `builder.rs:43`). The builder constructs successfully and only fails at REQUEST time. So the proof
    is the WRITE failing, not the build failing.
  - **Scrub set (exact):** unset all `AWS_*` env vars the builder recognizes — `AWS_ACCESS_KEY_ID`,
    `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_DEFAULT_REGION`/`AWS_REGION`,
    `AWS_ENDPOINT`/`AWS_ENDPOINT_URL_S3`, `AWS_WEB_IDENTITY_TOKEN_FILE`, `AWS_ROLE_ARN`,
    `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `AWS_CONTAINER_CREDENTIALS_FULL_URI`, `AWS_PROFILE`
    (enumerated in `builder.rs:557-577`), AND point any AWS config/credentials profile path away from a
    real file. Block IMDS so `InstanceCredentialProvider` cannot silently succeed: run the control
    off-instance, OR set an unroutable `metadata_endpoint`/IMDS timeout (Open decision §11.4).
  - **Assertion:** with creds scrubbed and `storage_options` carrying no/invalid creds, the write (or
    its first object-store `put`/`head`) MUST FAIL with an authentication error. Re-run the SAME write
    with VALID SSM-injected `storage_options` and assert SUCCESS. The delta — failure when SSM creds
    absent, success when present, with ambient sources held constant-scrubbed in both — attributes the
    positive write to SSM creds specifically and empirically demonstrates rule-6 for the catalog path.
- **0.4 Positive proof (unchanged):** SSM-resolved creds → write two SYNTHETIC fixtures → re-open →
  `query_files` → assert. Stamp `NtCapabilityProof` recording PROVEN direct-S3, the EXACT
  `storage_options` key set consumed by NT (`endpoint_url`, `region`, `access_key_id`/`key`,
  `secret_access_key`/`secret`, `session_token`/`token`, `allow_http` — `parquet.rs:692-711`; any other
  key is silently dropped via the `_ =>` warn arm `parquet.rs:712-714`), and credential source = SSM.
  On failure, document the local-write-then-s3-sync fallback and block direct-S3 claims.

(The cost/scale section for F10 is §9 above.)

---

## 13. Open decisions for the owner

1. **Artifact_root home** — reuse `bolt-parquet` or a separate bucket; top-level prefix?
   (`normalized/` name is not contract-given.) E-034 is DECISION_NEEDED. Tied to where the separate
   cloud-enabled projector crate lives (§12).
2. **Coverage completeness bar (NOW A BLOCKING GATE TOKEN, not merely an open question).** The owner
   must declare, per `(venue, product_family)`, the minimum acceptable `completeness_class` and max
   tolerated fraction of current-snapshot-only / unresolved-product_category instruments (§8c). This
   gate also binds the cost/scale sign-off (§9.5) so cost and completeness are approved together.
   Concrete partial-coverage triggers: OKX post-Apr-5 unaccepted, Deribit 1118 errors, HL core 799
   gaps, Polymarket 914-physical-vs-748-manifest, funding only 4 OKX daily files, current-snapshot-only
   universe for several venues.
3. **Cloud-feature isolation (UPGRADED to a structural requirement).** The cloud-enabled projector lives
   in a SEPARATE workspace/lockfile excluded from the live binary's resolution (§12), enforced by the
   `cargo tree -e features` guard. The v1 phrasing "enable cloud only on the research crate edge" is
   struck — a feature edge within one graph does not isolate under additive unification. Owner choice:
   sibling `tools/catalog-projector/` with `[workspace] exclude` + own `Cargo.lock` (recommended) vs a
   fully separate repo.
4. **Negative-control IMDS block mechanism on the real run host.** Running off-instance is cleanest; if
   Phase 0 runs ON an EC2 instance with an instance profile, the control needs a concrete IMDS-disable
   (unroutable metadata_endpoint, blocked 169.254.169.254 route, or `AWS_EC2_METADATA_DISABLED=true`).
   Confirm which is available so a write failure is from missing SSM creds, not a misconfigured block.
5. **HL node_fills authority + native-trades tape.** Invest in the `node_fills_by_block` lz4 schema
   probe + dedupe/completeness proof + requester-pays cost estimate now, or defer the Hyperliquid
   native-trades tape? The specific lane-authority contradiction (which doc claims `node_fills` vs
   `node_fills_by_block` is authoritative) was not located in this pass and is the substance of this
   decision. HL-core has NO native trade tape this tranche regardless (§7.4 forbidden_claim).
6. **Signal tables scope** — populate `long_short_ratios` / `taker_buy_sell_volume` (Tier C) now or
   keep excluded?
7. **Bars carry granularity** — mark/index/premium are 1m-OHLC-only; is 1m acceptable or is sub-minute
   required? Overlaps §2.4 (kline-derived mark/index point updates) and the Phase-6 mark/index replay
   fidelity question.
8. **Cost/scale sign-off (NEW).** Approve the costed projection (object counts, bytes, HEAD/LIST/PUT
   request counts per venue, HL requester-pays pre-stage egress, wall-clock under chosen parallelism)
   and the HL requester-pays pre-stage line item before the one-year run (§9.5). Tied to decision 2.

### Residual sub-questions carried from the corrections (not yet owner-blocking)

- **Tier B funding consumption path:** if bolt's backtests need funding applied during replay, funding
  cannot ride the catalog BacktestNode stream (not a `NautilusDataType`) and needs an explicit
  actor/strategy-side injection or a custom-data subscription — a Phase-6 design gap.
- **settlements → InstrumentClose:** whether Deribit/Bybit settlement event records should map to Tier A
  `InstrumentClose` (`instrument_closes`) instead of Tier C; left Tier C pending a source-proof that
  settlement semantics match NT InstrumentClose.
- **Optional top-10 → `order_book_depths`:** whether the fixed-depth→top-10 projection (Binance
  bookDepth, HL l2Book, OKX/Polymarket fixed-depth) is in-scope this tranche or deferred — affects
  whether `order_book_depths` gets populated at all given no native L2 delta source exists for most
  venues.
- **CI conditional-create conformance store:** the `ConditionalCatalogWriter` concurrency BLOCKER needs
  a `PutMode::Create`-capable store offline (MinIO/R2, ETagMatch-capable per
  `aws/precondition.rs:121-122`); LocalFileSystem's Create semantics differ and would not exercise the
  S3 If-None-Match path.
- **Conditional-put enabled on the real bucket:** verify the production/staging bucket (and any R2
  mirror) actually has `conditional_put` enabled; if disabled, `PutMode::Create` returns
  `NotImplemented` (`aws/mod.rs:183`) and the run must abort, not degrade.
- **Pointer-commit ordering across kinds:** a PromotionPackage may span multiple `artifact_kind`
  pointers (`nt_catalog` vs `normalized`); decide whether cross-kind promotion needs a single
  transactional barrier or per-kind eventual consistency (with the PromotionPackage snapshot as the join
  record) is acceptable.
- **Binance dated-delivery contractType value set:** the live-exchangeInfo `contractType` strings for
  dated futures are assumed from API convention, not verified against a captured raw payload; Phase 5
  Binance must verify against a staged exchangeInfo sample before locking the delivery-suffix mapping.
- **PMXT event_type schema probe:** confirm `last_trade_price`/`book`/`price_change`/`tick_size_change`
  against a real PMXT Parquet schema sample before declaring trades/quotes/instrument_status
  backfillable.
- **OKX `order_book_400` internal frame structure + option book existence:** lock the
  `okx_400level_snapshot_clear_add_then_update_delete` derivation rule against a real Parquet/CSV
  schema sample; confirm OKX historical-download actually serves a 400-level option book before keeping
  any option book family.
- **Synthetic-fixture generator home:** does the Phase-0/Phase-3 synthetic NT-class fixture generator
  live in the research-only crate alongside the capability proof, or a shared test-fixtures module?
- **Acceptance authority/automation boundary:** E-040 allows automated acceptance "when all robust
  checks pass" but the plan does not yet specify who/what flips `status=accepted` for the per-family
  S7 gate, nor where the acceptance record is stored under `source-proofs/`.
- **Mixed-fidelity run manifests:** when one BacktestNode run reads multiple families, S9 requires ALL
  their `source_proof_ids` accepted — confirm a run with any pending-proof family fails loud / is
  rejected at manifest validation.
- **`staging_location` field name** — `staging_location ∈ {local, s3_noncanonical}` is a naming choice;
  the load-bearing decision is that it is NOT a fourth `write_mode` value.
- **Symbol-shape grammar per venue beyond Binance USD-M** (COIN-M dated, Bybit linear/inverse expired,
  OKX FUTURES expiry-coded) must be authored and source-proof-cited; only the Binance USD-M
  PERP/DELIVERY rule is currently evidenced in code.

---

## 14. Risks

- Phase 0 catalog proof + `ConditionalCatalogWriter` concurrency proof NOT yet executed (read-only
  analysis only); do not mark E-037 SOURCE_PROVEN-positive until the write+query and the concurrency
  race run end to end.
- Per-product timestamp-unit hazard is the highest silent-corruption risk (spot=µs, futures=ms,
  metrics=string-datetime, Bybit derivatives=seconds.fraction); never fall back to REST response time.
- Scientific-notation/decimal fields must be parsed as Decimal from the exact raw string.
- Several NT classes are unsatisfiable from this tranche → record as forbidden_claims, not faked (no
  native order books/quotes for Binance/Bybit/HIP-3/Deribit historical; no per-strike Greeks/IV; Deribit
  index_prices has no source event_time from `get_index_price`; Polymarket full-depth pending;
  HIP-4/Polymarket trades are recent/bounded; HL-core has no native trade tape).
- Identity traps (OKX/Bybit perpetual-vs-dated contractType join; OKX instrument_id from payload not
  partition; Polymarket family mislabeled in key path; Binance four-family taxonomy must not leak
  `futures_um`/`*_or_delivery`).
- Interim staging writes must be strictly labeled (`write_mode=local_staging`,
  `staging_location=s3_noncanonical`, deterministic provisional `source_proof_id`, `commit_state=staged`,
  forbidden_claims) or be mistaken for canonical.
- `transform_hash` must hash CODE + CONFIG, not config alone.
- NT's writer is non-atomic (head-then-put TOCTOU, interval-keyed, default Overwrite); ALL writes go
  through `ConditionalCatalogWriter`, never NT's `write_to_parquet` directly.
- Canonical promotion must be an explicit accepted-object PromotionPackage + pointer CAS, never a prefix
  re-point (would canonicalize orphan/superseded bytes).
- Instrument universe is best-effort, NOT window-complete; current-snapshot listing sources omit
  instruments delisted before the snapshot with no staged-data footprint; recorded as
  `bounded_or_current_only` + forbidden_claim, never silently presented as complete. Expired-contract
  `product_category` is resolved only by a declared, source-proof-cited symbol-shape parser; unresolved
  instruments are forbidden_claims, not dropped.
- Cargo feature unification: cloud/aws must NOT reach the live binary; enforced by the separate workspace
  + `cargo tree -e features` guard.
- Cost/scale: Deribit object-count, OKX `order_book_400` fan-out, and NT per-write HEAD+LIST dominate
  request cost; HL requester-pays archives need a pre-stage copy because NT drops the `request_payer`
  storage option.
- Source bindings today are largely instrument_universe/instruments; market-data bindings needed for
  backtests are largely not yet declared.
- Python research path (if used) introduces a second credential surface (s3fs) that can drift from
  SSM-only.

---

## 15. Findings resolution summary (F1–F15)

Every adversarial-review finding is resolved in v2 as follows. Detailed mechanics are in the sections
referenced.

- **F1** NT data-class mis-assignment → §2 three-tier matrix + `nt_target` evidence column; replay claim
  scoped to Tier A (§2.5).
- **F2** Non-atomic NT writer → §4.1, §4.3 `ConditionalCatalogWriter` (`PutMode::Create`,
  content+transform-hash key, concurrency proof) as a Phase-0 BLOCKER.
- **F3** Binance product-family taxonomy → §6.1 four-family single source, derived from contractType at
  normalize; binding TOML reconciled; fail-loud guard + acceptance test.
- **F4** OKX `order_book_400` → §7.1 `order_book_snapshots_fixed_depth` + `order_book_snapshot_deltas`
  (named derivation rule) + forbidden_claim; matrix downgrade.
- **F5** Polymarket family/host → §7.2 single authoritative `order_book_snapshots_fixed_depth` from
  accepted manifest; host `archive.pmxt.dev/Polymarket/v2`; binding + matrix reconciled.
- **F6** Deribit index event_time → §7.3 forbidden_claim + `event_time_source` fail-loud guard applied
  to all snapshot-only families.
- **F7** `write_mode` fragmentation → §6.2 one three-valued enum; `s3_staging` aliased to
  `local_staging` + additive `staging_location`; full migration list + validation test.
- **F8** Provisional `source_proof_id` / `nt_instrument_id` → §6.3 deterministic provisional id scheme +
  `nt_mapping_status`-bound population rule.
- **F9** Instrument-universe completeness → §8 best-effort + completeness gap record + declared
  symbol-shape parser + owner-declared blocking completeness bar.
- **F10** Cost/scale → §9 ledger-grounded volumes, NT request amplification, HL requester-pays pre-stage,
  partitioning/parallelism, gating estimate.
- **F11** Credential negative control → §11-cred 0.3 control (scrub env + IMDS; SSM-present vs SSM-absent
  delta).
- **F12** Cargo isolation → §12 separate workspace/lockfile + `cargo tree -e features` build guard.
- **F13** HL-core native trades → §7.4 `no HL-core native trade tape this tranche` forbidden_claim;
  `node_fills_by_block` a separate gated future task.
- **F14** Proof-acceptance sequencing → §5.2 per-family precedence gate, synthetic-only early smoke,
  provider-derived backtest gate; §10 reordered S1–S10 / Phases 0–6.
- **F15** Prefix-repoint promotion → §4.4 explicit accepted-object PromotionPackage + pointer CAS.
