# Normalization / Transform / Catalog Plan (v3)

`plan_version`: `normalization-catalog-plan.v3`
`supersedes`: `normalization-catalog-plan.v2`
`status`: DRAFT pending owner sign-off. v2 went through a six-model adversarial-review pass (B-1 convergent blocker + R-2…R-16). v3 incorporates every grounded cluster correction. Does NOT itself authorize canonical writes (those remain owner-gated, see "Contract gate handling" §5.1 / contract §292-309).
`NT rev`: `6be5a5094716790a8ca2875445fde4fa2586107e` (crate `nautilus-persistence`); `object_store` 0.13.2.
`archive note`: the superseded plan chain — `normalization-catalog-plan.v1.md`, `normalization-catalog-plan.v2.md`, `normalization-catalog-plan.v2-review-synthesis.md`, `normalization-catalog-plan.v2-reviews/` — is removed from the live tree; this document is the only living plan surface. Retrieve any archived file from git history: `git show b2beaf9c1:specs/023-nt-research-analytics-platform/reference/<name>`. File:line citations to those documents elsewhere in this plan refer to the archived copies.

## v3 changes from v2

v3 keeps the v2 design that the review did not challenge (the F1–F15 resolutions are all retained) and integrates the six-model review synthesis (`normalization-catalog-plan.v2-review-synthesis.md`, archived — see the archive note above). Every NT/object_store assertion below quotes the function at file:line at the pinned rev; no hand-waving. The changes:

- **B-1 (convergent read-path blocker, 5 of 6 reviewers):** the largest v3 change. Restructures §3, §4.3, §4.4, §4.5, §5.3 around (a) immutable per-commit NT-native catalog roots materialized only from a committed PromotionPackage, (b) NT-native interval filenames in canonical roots vs content+transform-hash names staging-only, (c) Tier-C-only physically-non-NT staging that NT's reader cannot enumerate, plus a fail-loud validator. Grounded: NT lists a single root's `data/<type>/` prefix naively with zero pointer awareness (`catalog.rs:2040-2063`) and over-includes unparseable filenames (`query_intersects_filename` returns `true` on `None`, `catalog.rs:4741`) — silent wrong-data, NOT a crash (Gemini's crash mechanism disproven).
- **R-2 (cross-kind promotion atomicity):** one PromotionPackage commits as a single immutable `SnapshotSet` advanced by ONE CAS on `pointers/set/latest.json`; per-kind pointers are derived views; the backtest pins the committed set once at run start. New §4.5; §13 open sub-question "Pointer-commit ordering across kinds" RESOLVED.
- **R-3 (idempotency digest):** staging key, PromotionPackage entry, and `ArtifactIndex.content_hash` key on a canonical LOGICAL-content digest (new §4.3b, via NT's own in-tree `arrow_row::RowConverter`), never raw parquet bytes (non-deterministic — `created_by`/SNAPPY/row-group, `parquet.rs:182-183`). §9.x cost-model note added.
- **R-4 (instruments lane):** instruments go through `ConditionalCatalogWriter` (new §4.5); NT `write_instruments` (`catalog.rs:726`, NOT node.rs:169 which is the read side) is never called for platform-root writes (§4.1 scope-boundary note); `event_time_source` guard exemption made class-correct via a table-level `time_series=false` predicate.
- **R-5 (encoder seam):** `write_batches_to_object_store` is `pub` (`parquet.rs:170`) but encode+put share one function with no byte-return seam (`parquet.rs:178-197`); Phase-0 sub-task 0.E mandates verifying visibility and choosing vendor-encode vs arrow-rs, recorded in `NtCapabilityProof`.
- **R-6 (conditional-put unprovable + multipart):** resolved S3ConditionalPut is not introspectable from a built store and NT's `create_s3_store` cannot even set it (drops unknown keys, `parquet.rs:763-765`); the writer builds its own `AmazonS3Builder` and asserts capability via a runtime probe at construction (Phase-0 prerequisite 0.6); public multipart cannot carry a create guard, so per-object size is bounded under the single-PUT limit, fail loud.
- **R-7 (server-side copy):** promotion materializes canonical objects by backend-native `object_store::copy_opts(CopyMode::Create)` → S3 `CopyObject` (`aws/mod.rs:312`), zero egress; new §4.4a; §9 cost addendum.
- **R-8 (write_mode migration atomicity):** the coverage ledger's exclusion heuristic (`backfill_coverage_ledger.py:289-293`) is replaced with positive identification; the 13 producer/ledger edits + schema-validation test ship as ONE indivisible change set (§6.2, §16 Group G-A).
- **R-9 (`:v0-pending`):** the provisional suffix is an opaque ROW-ID discriminator decoupled from the typed `source_proof_version` field (a positive integer; pending = `1`); §6.3.
- **R-10 (orphan recovery):** a distinct `recovered_orphan` `commit_state` + distinct manifest schema, required accepted+resolved `source_proof_id`, FULL (not sampled) hash verify, complete provenance, barred from coverage and promotion until human-reviewed; §6.4.
- **R-11 (Tier-A version coupling):** CI guard asserts the current `NautilusDataType` member set, including `FundingRateUpdate` and `OptionGreeks` at the repo-pinned NT rev, plus the exact projector-relevant prefix STRINGS (NOT a blanket `CatalogPathPrefix` count) + the `timestamps_to_filename` format; projector pinned to `6be5a5094716790a8ca2875445fde4fa2586107e`; §11/§12.
- **R-12 (Python dual write path):** §3 declares a single writer (Rust `ConditionalCatalogWriter`); Python is strictly read-only against both the NT catalog and Tier-C Parquet; the v2 "Python convenience that writes the same format" clause is removed.
- **R-13 (promotion TOCTOU + staging cleanup):** at-WRITE-time re-verification of the logical digest before canonical materialization (whole-package fail-loud abort), plus a fail-safe staging-cleanup policy that pins every URI a constructed-but-uncommitted PromotionPackage enumerates; §4.4 / §4.4b.
- **R-14 (synthetic↔provider root collision):** Phase-0/3 proofs write to a dedicated synthetic-only top-level root with a fail-loud disjointness assertion before any byte; §4.4 / §10.2.
- **R-15 (framing):** new §16 separates design-decided (D-1..D-7) from repo-edits-pending (atomic, file:line-targeted tasks G-A..G-C); the plan owns the list but does not itself perform the edits.
- **R-16 (funding native replay, resolved):** `FundingRateUpdate` is present in `NautilusDataType` and `dispatch_query` at repo-pinned NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`; funding uses the native NT catalog stream. Custom-data remains for non-native S6/S7 families, not funding.

Canonical-write authorization is unchanged: writes remain owner-gated; v3 does not authorize them. Status stays DRAFT pending owner sign-off.

## Purpose

Turn the one-off seven-token raw S3 backfill (audit input) into a **research-ready store** the
project's specs already mandate: a NautilusTrader `ParquetDataCatalog` that NT's
`BacktestNode`/`BacktestEngine` can replay, plus non-NT research-only Parquet that read-only Jupyter
notebooks / Research-Analytics can consume. Raw provider payloads are **audit input, not replay
input** (evidence E-002, SOURCE_PROVEN). This document is the build plan for the raw→catalog
projection layer; it does NOT itself authorize canonical writes (those remain gated, see "Contract
gate handling").

v2 fixed the v1 errors found in the first adversarial-review pass:
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

v3 fixes the second (six-model) pass: the B-1 read-path blocker and R-2…R-16 (see "v3 changes from
v2" above).

---

## 1. Verified-at-pinned-rev facts (re-checked by the main session)

NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`, crate `nautilus-persistence`:

- The catalog's `CatalogPathPrefix` set is fixed (`crates/persistence/src/backend/catalog.rs:4248-4286`):
  `QuoteTick→quotes`, `TradeTick→trades`, `OrderBookDelta→order_book_deltas`,
  `OrderBookDepth10→order_book_depths` (**NOT** `order_book_depth_10`, `catalog.rs:4251`),
  `Bar→bars`, `IndexPriceUpdate→index_prices`, `MarkPriceUpdate→mark_prices`,
  `FundingRateUpdate→funding_rate_update` (**NOT** `funding_rates`, `catalog.rs:4255`),
  `InstrumentStatus→instrument_status`, `OptionGreeks→option_greeks`, `InstrumentClose→instrument_closes`,
  `InstrumentAny→instruments`, `AccountState→account_state`, plus order/position/report lifecycle
  prefixes (execution outputs, out of scope here). The full `impl_catalog_path_prefix!` set is 39
  entries (`catalog.rs:4248-4286`) — only the load-bearing subset is asserted by the R-11 guard (§11/§12).
- The Rust `BacktestNode` replay path (`crates/backtest/src/node.rs`, `dispatch_query`)
  streams the current `NautilusDataType` enum (`crates/backtest/src/config.rs`):
  `QuoteTick, TradeTick, Bar, OrderBookDelta, OrderBookDepth10, MarkPriceUpdate, IndexPriceUpdate,
  FundingRateUpdate, InstrumentStatus, OptionGreeks, InstrumentClose`. `FundingRateUpdate` and
  `OptionGreeks` have native dispatch arms at the pinned rev.
- `instruments` load via a separate lane: write (`write_instruments`, `catalog.rs:726`) /
  read (`query_instruments`, `catalog.rs:858`, called at `node.rs:169`), NOT through `dispatch_query`.
- `MarkPriceUpdate`/`IndexPriceUpdate` are **point updates** carrying a single price + timestamp
  (`crates/model/src/data/mod.rs:107-108`), **not** OHLC bars.
- NT's writer is non-atomic: `head()` existence probe (`catalog.rs:564-567`) then unconditional
  `object_store.put` (`parquet.rs:197`, default `PutMode::Overwrite`, no If-None-Match). Filename is
  interval-keyed (`timestamps_to_filename`, `catalog.rs:560,4315-4320`). The only structural guard is
  the disjoint-interval check, bypassable with `skip_disjoint_check=true` (`catalog.rs:574`).
- NT's reader has **zero pointer/snapshot awareness**: `from_uri` stores only `base_path` +
  `object_store` (`catalog.rs:307-330`); `query_files` does a naive recursive `object_store.list` of
  `{base_path}/data/<type>/` and keeps every `*.parquet` (`catalog.rs:2040-2063`); on an unparseable
  filename `query_intersects_filename` returns **`true`** (`catalog.rs:4736-4743`), so a non-NT-native
  name is silently over-included into EVERY query window — it does NOT crash.
- `object_store` 0.13.2 supports `PutMode::Create` (atomic If-None-Match: *, `Error::AlreadyExists`
  on collision — `lib.rs:1702-1711`, `aws/mod.rs:181-201`) and `PutMode::Update(UpdateVersion)`
  (If-Match CAS — `aws/mod.rs:202-228`). S3 `PutMode::Create` requires `S3ConditionalPut::ETagMatch`
  (the crate default, `aws/precondition.rs:120-128`); `Disabled` makes `Create` return
  `Error::NotImplemented` (`aws/mod.rs:183-187`). The resolved mode is NOT introspectable from a built
  `AmazonS3` (`#[non_exhaustive]` enum, private `from_str`, `pub(crate)` client config —
  `aws/precondition.rs:117-160`, `aws/client.rs:209`). Backend-native server-side copy exists:
  `copy_opts`/`CopyOptions`/`CopyMode::{Overwrite,Create}` (`lib.rs:1111,1880-1894`) with
  `copy`/`copy_if_not_exists` wrappers (`lib.rs:1386-1394`); on S3 `copy_opts` issues `CopyObject`
  (`aws/mod.rs:312`, `x-amz-copy-source` `aws/client.rs:596-597,702`). `CopyMode::Create` requires
  `S3CopyIfNotExists` configured or returns `Error::NotSupported` (`aws/mod.rs:374-378`).
- `bolt-v2/Cargo.toml` is a **single-package binary crate** (`Cargo.toml:1-2`), NOT a workspace.
  (Staleness correction, 2026-06-12: `main` now additionally carries
  `crates/backtesting-vertical-slice/` as its OWN excluded workspace root with its own `Cargo.lock`
  and `nautilus-persistence` `cloud` feature — the live binary's resolution graph remains
  single-package; see the §12 prior-art note.) It already lists `nautilus-persistence` as a direct
  dependency (`Cargo.toml:39`) with **no** `cloud` feature. `nautilus-persistence` default features
  are empty (`crates/persistence/Cargo.toml:24`); `cloud = object_store/{aws,azure,gcp,http}`
  (`:25-30`); `python` transitively enables `cloud` (`:39-49`). Today `cargo tree -e features` on the
  live binary shows `object_store v0.13.2` present via datafusion but with features only
  `fs`/`tokio`/`walkdir` — the `aws` feature is NOT enabled (empirically verified at HEAD).

---

## 2. NT-class target matrix (authoritative — resolves F1)

`source_of_truth`: catalog write surface = `crates/persistence/src/backend/catalog.rs`; backtest
replay surface = `crates/backtest/src/{config.rs,node.rs}`; NT rev `6be5a5094716790a8ca2875445fde4fa2586107e`.

### 2.1 Three-tier classification + the instruments lane (resolves F1, R-4)

v1 collapsed two NT surfaces that are NOT the same. There are THREE tiers, not two:

- **Tier A — NT-replayable**: type is a `NautilusDataType` member, so a Rust `BacktestNode` can
  `query::<T>` it and stream it. Exhaustive set is the pinned `NautilusDataType` enum; at
  `6be5a5094716790a8ca2875445fde4fa2586107e` this includes `FundingRateUpdate` and `OptionGreeks`.
- **Tier B — catalog-writable, NOT engine-replayable**: type has a `CatalogPathPrefix` and a typed
  write path but is NOT a `NautilusDataType`. This tranche is empty for the current plan after
  `FundingRateUpdate` moved to Tier A.
- **Tier C — non-NT research-only Parquet**: no NT data class. Lands as custom-data Parquet
  (`catalog.rs:450-452,474-476`) or a plain research table under `normalized/`. `BacktestNode` does
  NOT consume it.

**`instruments` is a fourth, separate lane.** It is written by `ParquetDataCatalog::write_instruments`
(**`catalog.rs:726`**, `pub fn write_instruments`) and read back for a backtest by
`query_instruments` (**`catalog.rs:858`**); the `BacktestNode` calls only the READ side,
`catalog.query_instruments(filter)` (**`node.rs:169`** — this line is the read, not the write).
Instruments are a backtest precondition (instrument definitions), not a streamed time-series, so they
do NOT go through `dispatch_query`.

> **Instruments-lane write hazard (resolves R-4).** `write_instruments` is **not** a separate, safe
> write path — it inherits the exact F2 non-atomic last-writer-wins defect. Verified at `6be5a50`,
> `write_instruments` (`catalog.rs:726-820`) does: a `head()` existence probe
> (`catalog.rs:773`), then on miss an unconditional `write_batches_to_object_store(...)` whose final
> step is a plain `object_store.put(...)` (`catalog.rs:805-813` → `parquet.rs:197`, default
> `PutMode::Overwrite`, no If-None-Match). Its filename is interval-keyed via
> `timestamps_to_filename(start_ts, end_ts)` (`catalog.rs:768`), so two different
> instrument-snapshot transforms over the same `(start_ts, end_ts)` collide on one object path and the
> `head()` skip silently drops the second. This is the identical TOCTOU + interval-collision class as
> `write_to_parquet` (§4.1).
>
> **Class fix:** the instruments lane is NOT exempt from the write discipline. Like every Tier A/B/C
> data write, instrument-snapshot objects are written through the `ConditionalCatalogWriter`
> encode-then-conditional-create path (§4.3) and committed only via an accepted PromotionPackage
> (§4.4). NT's `write_instruments` is **never** called for any staged or canonical write — it is used
> only via the read side (`query_instruments`) when re-opening a catalog for a backtest (scope:
> platform roots; the pre-existing vertical-slice run projection on `main` is the run-scoped
> exception — see the §4.1 scope-boundary note). See §4.5 for
> the instruments-lane writer specialization.
>
> **`event_time_source` guard EXEMPTION (resolves R-4, class-correct).** The `event_time_source`
> fail-loud guard (§7.3) rejects rows from `event_time_source=none` (snapshot/current-probe) families
> from being emitted into **time-series** tables. `instruments` is **not** a time-series table — an
> instrument definition is a point-in-time listing record, not an event stream — so the instruments
> lane is **explicitly exempt** from the `event_time_source` guard. The guard's allowlist table
> (§7.3) carries an explicit `applies_to_timeseries_tables_only = true` flag, and the
> `instruments`/`instrument_universe_snapshots` families are tagged
> `time_series = false`; the guard short-circuits to a no-op for any `time_series = false` family
> rather than treating a current-snapshot instrument source as a forbidden time-series emission. This
> is a class fix (a table-level `time_series` predicate), not an instruments-only special case: any
> future point-in-time definition table inherits the exemption by setting `time_series = false`.

### 2.2 Verified NT path-prefix set (catalog.rs:4248-4286)

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
| `FundingRateUpdate` | `funding_rate_update` | **A** | **yes** |
| `OptionGreeks` | `option_greeks` | **A** | **yes** |
| `InstrumentAny` | `instruments` | separate lane — write via `ConditionalCatalogWriter` (NEVER NT `write_instruments` for platform-root writes, `catalog.rs:726`; §4.1 scope-boundary note); read via `query_instruments` (`catalog.rs:858`, called at `node.rs:169`) | n/a |
| `AccountState` | `account_state` | execution output, out of scope | no |

Confirmed exact strings: `order_book_depths` (NOT `order_book_depth_10`); `funding_rate_update`
(NOT `funding_rates`); `option_greeks`. `FundingRateUpdate` and `OptionGreeks` are Tier A at the
current repo-pinned NT rev.

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
| `instruments` | `InstrumentAny → instruments` (separate write-via-`ConditionalCatalogWriter` / read-via-`query_instruments` lane; backtest precondition, not streamed) |
| `instrument_status` | Tier A — `InstrumentStatus → instrument_status` |
| `instrument_closes` | Tier A — `InstrumentClose → instrument_closes` |

**Market Data** (`contract:107-130`):

| Contract family | Target |
| --- | --- |
| `trades` | Tier A — `TradeTick → trades` (native only; `trade_source_type=aggregated` rows still write to `trades` but carry the aggregated tag + forbidden-claim) |
| `quotes` | Tier A — `QuoteTick → quotes` (carries `quote_source_type`; `reconstructed_top_of_book` rows tagged + forbidden-claim) |
| `order_book_deltas` | Tier A — `OrderBookDelta → order_book_deltas` (NATIVE L2/L3 only; see §7.1 F4 — OKX 400-level is NOT native here) |
| `order_book_snapshot_deltas` | Tier C — non-NT research-only Parquet (no NT class; derived clear-and-rebuild) |
| `order_book_snapshots_full` | Tier C — non-NT research-only Parquet (no NT class) |
| `order_book_snapshots_fixed_depth` | Tier C — non-NT research-only Parquet (no NT class). A top-10 projection MAY ADDITIONALLY be emitted to Tier A `order_book_depths` — see naming note below |
| `order_book_depth_10` (contract column name) | **Rename → NT prefix `order_book_depths`.** Tier A — `OrderBookDepth10 → order_book_depths`. The contract column name `order_book_depth_10` is a derived/native top-10 projection; the NT catalog prefix is `order_book_depths` (`catalog.rs:4251`). All `order_book_depth_10` references in this plan use prefix `order_book_depths` |
| `bars` | Tier A — `Bar → bars` (carries `bar_source_type`) |

**Derivatives, Carry, And Risk State** (`contract:134-143`):

| Contract family | Target |
| --- | --- |
| `mark_prices` | Tier A — `MarkPriceUpdate → mark_prices` (point update, NOT a bar — see §2.4) |
| `index_prices` | Tier A — `IndexPriceUpdate → index_prices` (point update, NOT a bar) |
| `premium_index_prices` | Tier C — non-NT research-only Parquet (no NT class) |
| `funding_rates` (contract name) | Tier A — `FundingRateUpdate → funding_rate_update`. Catalog-writable and native-replayable at the current repo-pinned NT rev. All `funding_rates` references use prefix `funding_rate_update` where the NT class is targeted |
| `open_interest` | Tier C — non-NT research-only Parquet |
| `liquidations` | Tier C — non-NT research-only Parquet |
| `long_short_ratios` | Tier C — non-NT research-only Parquet |
| `taker_buy_sell_volume` | Tier C — non-NT research-only Parquet |
| `borrow_lending_rates` | Tier C — non-NT research-only Parquet |

**Options** (`contract:146-153`):

| Contract family | Target |
| --- | --- |
| `option_greeks` | Tier A — `OptionGreeks → option_greeks` at the current repo-pinned NT rev. S7 still owns source-specific canonicalization/projection and any `OptionGreeks -> on_option_greeks` engine proof |
| `implied_volatility` | Tier C — non-NT research-only Parquet |
| `historical_volatility` | Tier C — non-NT research-only Parquet |
| `forward_prices` | Tier C — non-NT research-only Parquet |
| `delivery_prices` | Tier C — non-NT research-only Parquet |
| `settlements` | Tier C — event records; closest NT analogue is `InstrumentClose`, but settlements are not 1:1 with NT instrument-close semantics — keep non-NT unless an `InstrumentClose` mapping is separately source-proven — see Open decision §13.2 |

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
current `NautilusDataType` set, routed through `dispatch_query`: `quotes`, `trades`, `bars`,
`order_book_deltas`, `order_book_depths`, `mark_prices`, `index_prices`, `instrument_status`,
`instrument_closes`, `funding_rate_update`, `option_greeks` — plus the `instruments` precondition lane. NOTHING else is replayable:

- **All Tier C families are NOT replayable** — `open_interest`, `premium_index_prices`,
  `long_short_ratios`, `taker_buy_sell_volume`, `borrow_lending_rates`, `liquidations`,
  `historical_volatility`, `implied_volatility`, `forward_prices`, `settlements`,
  `delivery_prices`, `order_book_snapshot_deltas`, `order_book_snapshots_full`,
  `order_book_snapshots_fixed_depth`, and all `prediction_market_*`.
- Any consumption smoke test that claims replay MUST use a Tier A type. It does NOT prove replay for
  any non-Tier-A family.

### 2.6 Evidence-matrix amendment

The expanded evidence matrix (`contract:303-304`) gains a per-`(venue, product_family, table_family)`
column **`nt_target`** with exactly one of: `nt_replayable:<prefix>` (Tier A),
`instruments_lane` (instruments), or
`non_nt_research_parquet` (Tier C). This column is the single source mapping each contract table to
its NT-surface tier and is what scopes the replay claim.

---

## 3. Catalog I/O decision — single writer, read-only Python (resolves R-12 NO DUAL PATHS)

### 3.1 Decision

There is exactly **one** writer of the NT catalog and the Tier-C research Parquet: the Rust
`ConditionalCatalogWriter` (§4.3). It is the only code path that ever puts staged or canonical bytes
into the platform's roots (the pre-existing vertical-slice run projection on `main` writes only its
run-scoped scratch roots — §4.1 scope-boundary note).
**Python is read-only** against both surfaces. Notebooks / Research-Analytics consume the catalog and
the Tier-C research Parquet through read-only APIs; no Python process writes, promotes, consolidates,
or mutates any object. This removes the v2 dual write path (a "Python convenience that writes the same
format" had no SSM credential discipline and no conditional-create discipline, so it directly violated
bolt rule 2 NO DUAL PATHS — review finding R-12, B-1 fix (c)).

Rationale: (1) **Single write path** — one encode-then-conditional-`Create` discipline (§4.3), one
SSM-only credential resolver (bolt rule 6), one immutable-per-commit-root promotion (§4.4). A second
producer would fork all three. (2) **Credential discipline** — bolt-v2 mandates SSM-only secrets
(rule 6); Rust resolves S3 creds in-process and injects them into `from_uri` `storage_options`
(`catalog.rs:307-322`, `storage_options` consumed at `parquet.rs:743-762`), whereas a Python writer
needs an s3fs/fsspec write surface that risks an env-var/AWS-CLI fallback. Read-only Python carries the
same credential injection but can never produce a non-conditional PUT. (3) **Single build path** — the
Rust projector shares one `nautilus-persistence`/`datafusion` version with the downstream
`BacktestNode`; `object_store` 0.13.2 is already in the tree transitively.

### 3.2 Why a Python writer is structurally unsafe (not merely "discouraged")

NT's Python `ParquetDataCatalog.write_data` is the same non-atomic last-writer-wins surface §4.1
rejects, exposed in Python: its own docstring warns "Any existing data which already exists under a
filename will be overwritten" (`nautilus_trader/persistence/catalog/parquet.py:284-285`), and it
delegates to `write_chunk` → `write_objects` over the standard catalog `put` (the same
`PutMode::Overwrite` default, no If-None-Match, that §4.1 documents at `parquet.rs:197`). The Python
catalog also exposes `consolidate_data` / `consolidate_catalog` (`parquet.py:597,652`) which rewrite
existing files in place. Allowing any of these from Python re-introduces exactly the TOCTOU /
silent-overwrite class §4.1–§4.3 exist to eliminate. Therefore the prohibition is mechanical, not
advisory: **no Python code calls `write_data`, `write_chunk`, `consolidate_*`, or any catalog mutation
method.**

### 3.3 How Python reads (read-only API surface)

Python reads the **committed** canonical NT catalog (the immutable per-snapshot-set root named by the
live artifact-index `set/latest.json` pointer — see §4.4 / §4.5) and the Tier-C research Parquet. It
never reads staging (staging is physically non-NT-layout and Tier-C-only — §5.3). Two read-only
surfaces are sanctioned, both backed by the same files the Rust writer produced:

1. **`nautilus_trader` Python `ParquetDataCatalog` read API.** Construct read-only via
   `ParquetDataCatalog.from_uri(uri, fs_storage_options=…)` (`parquet.py:198`), pointing `uri` at the
   committed snapshot-set root (never at a staging prefix, never at the hot pointer file). Query through
   `catalog.query(data_cls, identifiers=…, start=…, end=…)` (`parquet.py:1576`). For the NT built-in
   classes (`OrderBookDelta`, `OrderBookDepth10`, `QuoteTick`, `TradeTick`, `Bar`, `MarkPriceUpdate`,
   …) `query` dispatches to the Rust `_query_rust` backend session (`parquet.py:1628,1675`) — the
   **same** Rust read backend `BacktestNode` uses — so notebook reads and backtest reads see an
   identical view. For everything else it dispatches to `_query_pyarrow` (`parquet.py:2039`), a pure
   `pyarrow.dataset(...).to_table(filter=…)` read. Catalog-level read helpers
   (`list_data_types` `parquet.py:2393`, `query_last_timestamp` `parquet.py:2239`,
   `read_backtest`/`read_live_run` `parquet.py:2451,2429`) are all read-only and permitted.
2. **Direct `datafusion` / `pyarrow` read of the committed Parquet** for ad-hoc research over Tier-C
   tables (the families with no NT class) and for cross-table research joins, again pointed only at
   committed/canonical object URIs resolved through the artifact-index pointer/snapshot. This is a pure
   reader (`pyarrow.dataset` / a DataFusion `SessionContext.read_parquet`); it issues no PUTs.

Credential handling for Python reads is identical to the writer's: S3 creds resolve from SSM and are
injected via `from_uri`'s `fs_storage_options` / `fs_rust_storage_options` (`parquet.py:198-244`,
which forwards them into the Rust backend). No env-var / AWS-CLI fallback is used on the read path
either (bolt rule 6).

### 3.4 How NT resolves a catalog (the mechanic the whole read-path design rests on)

Verified at rev `6be5a50`:

- A backtest declares **one catalog root URI per `BacktestDataConfig`**: `catalog_path` +
  optional `catalog_fs_protocol` (`config.rs:662,664`). `create_catalog` builds `<protocol>://<path>`
  and calls `ParquetDataCatalog::from_uri(&uri, storage_options, …)` (`node.rs:507-516`).
- `from_uri` parses that URI into a `base_path` and stores only `base_path` + `object_store`
  (`catalog.rs:307-330`) — it has no field for a pointer, snapshot, or set. `make_path(type, id)`
  builds exactly `<base_path>/data/<type>/[<safe_id>]` (`catalog.rs:2841-2849`).
- `query_files(data_cls, ids, start, end)` does a **naive recursive `object_store.list` of
  `<base_path>/data/<type>/`** and keeps every `*.parquet` under it (`catalog.rs:2040-2063`). There is
  **zero pointer/snapshot/index awareness** — NT reads whatever files physically live under that one
  root's `data/<type>/` tree.

**Consequence (the design lever):** the set of bytes a backtest reads is *exactly* the `*.parquet`
files physically present under the single root URI it is pointed at. Controlling "what NT replays" is
therefore equivalent to controlling **which root URI is the active root** and **which files physically
exist under it**. v3's read-path design is built entirely on these two levers: an immutable per-commit
root (§4.4) and NT-native interval filenames inside it (§4.3).

### 3.5 Write surface caveat (carried from F2)

NT's `ParquetDataCatalog::write_to_parquet` / `write_custom_data_batch` (Rust:
`catalog.rs:530,632`; Python: `write_data` `parquet.py:251`) is **never** called for any write —
staged or canonical — from Rust **or** Python (scope: the platform's staging/canonical roots; the
pre-existing vertical-slice run projection on `main` writes only its run-scoped scratch roots —
§4.1 scope-boundary note). ALL platform writes go through the external
`ConditionalCatalogWriter` (§4.3); the `write_instruments` lane is no exception (it also encodes-then-
conditional-writes — see §4.3 / §4.5 / R-4). NT's catalog (Rust and Python) is used **only** for
read-back/query. There is one writer and any number of readers; there is no second write path into
the platform's roots. See §4.

---

## 4. Write discipline + canonical promotion (resolves F2 + F15 + B-1)

### 4.1 Why NT's writer cannot be the write surface (F2)

NT's `write_to_parquet` is non-atomic last-writer-wins, interval-keyed, not create-only:

- Existence is checked with a separate `head()` call (`catalog.rs:564-567`), then the file is written
  by an unconditional `put` (`parquet.rs:197`, default `PutMode::Overwrite`). Between `head()` and
  `put` there is a TOCTOU window: two concurrent writers both see "absent" and both PUT; on S3 the
  second silently overwrites the first.
- The filename is **interval-keyed** (`timestamps_to_filename(start_ts, end_ts)`, `catalog.rs:560`,
  `catalog.rs:4315-4320`), not content-keyed. Two different transforms over the same `(start_ts,
  end_ts)` collide on the same object path; the `head()` skip then suppresses the second write
  entirely, so a re-run with a *changed* `transform_hash` is silently dropped.
- The only structural guard is the disjoint-interval check (`catalog.rs:574-586`), bypassable with
  `skip_disjoint_check=true` (`catalog.rs:462-472`). It is not a create-only guard.

Conclusion: NT's writer cannot satisfy the contract's "Idempotent write manifest, create-only
behavior, and no-overwrite behavior" gate or `ingest_manifest.no_overwrite_proof`. **Never call NT's
`write_to_parquet`/`write_custom_data_batch`/`write_instruments` directly for any staged-or-canonical
write (platform roots — §4.1 scope-boundary note below).**

> **Scope boundary — pre-existing vertical-slice run projection (external review, 2026-06-12).** The
> NT-writer prohibition above (and every restatement of it: the v3-changes R-4 bullet, the §2 tier
> table, §2.1, §3.1, §3.5, §4.3, §4.5, §9.4, the §14 writer-atomicity risk bullet, §15 R-4, §16.1 D-2)
> is an invariant over THIS plan's catalog roots — the platform's staging and canonical
> (`nt-catalog/sets/<id>/`) prefixes. It is NOT a repo-wide claim: `main` already carries an
> NT-writer write path in the backtesting-vertical-slice run projection
> (`crates/backtesting-vertical-slice/src/catalog_projection.rs`, `write_instruments` +
> `write_to_parquet`), which writes per-run scratch catalog roots that the runner hash-reconciles
> against canonical rows and never promotes into any platform staging/canonical prefix. That path
> is outside this invariant today. If BTE run catalogs are later absorbed into the platform root
> family, migrating that projection onto `ConditionalCatalogWriter` becomes a tracked
> implementation task at that point.

### 4.2 Why prefix re-pointing canonicalizes orphan bytes (F15) and why a pointer alone does not keep bytes out of a read (B-1(a))

The v1 Phase 6 step "re-point staging into artifact_root, flip write_mode→canonical_s3" treats every
object under a staging prefix as accepted. Staging prefixes are shared and accumulate orphaned objects
from failed/aborted runs (`commit_state` can be `orphan` or `superseded`, `data-model.md:135`). A
prefix flip canonicalizes those too. Promotion must enumerate **exact** accepted objects, not a
prefix.

v2 went further and wrote canonical objects with `PutMode::Create` and *then* advanced the pointer,
but pointed NT at the same prefix the bytes were created under. Because NT lists the prefix directly
with **zero pointer awareness** (`catalog.rs:2040-2063`) and never consults the artifact-index
pointer/snapshot, any canonical-prefix object written **before** a failed/lost pointer CAS is an
**NT-readable orphan**: a backtest pointed at that prefix would replay it. The pointer indirection is
invisible to NT, so "the pointer didn't advance" does NOT keep the bytes out of a read. The v3 fix
(§4.4) decouples *where bytes are written* from *what NT is ever pointed at* via per-commit immutable
roots keyed by snapshot-set id.


### 4.3 `ConditionalCatalogWriter` — encode-then-conditional-create (resolves F2, B-1(b), R-4, R-5, R-6)

A research-only crate (in the separate cloud-enabled workspace, §12) that **encodes parquet bytes
itself and conditionally creates the object**. It NEVER delegates the object write to any NT catalog
write method (`write_to_parquet`, `write_custom_data_batch`, `write_instruments`; platform roots — §4.1 scope-boundary note). It is the **single
write path for every catalog and Parquet object in this system** — staging and canonical, market-data
and instruments alike (scope: the platform's roots — see the §4.1 scope-boundary note for the
pre-existing vertical-slice run projection on `main`). NT's `ParquetDataCatalog` is used **only**
for read-back/query (`query_files`, `query_instruments`).

> **Path note.** The write/encoder/store-builder code is in `crates/persistence/src/parquet.rs` (NOT
> `crates/persistence/src/backend/parquet.rs`, which does not exist at this rev). The catalog write
> methods are in `crates/persistence/src/backend/catalog.rs`.

**4.3.0 Encoder boundary — verified `pub`, but encode and `put` share one function (resolves R-5).**
At `6be5a50` the public encoder is `write_batches_to_object_store` (**`parquet.rs:170`**, declared
`pub async fn`). It IS public and callable from an external crate that depends on
`nautilus-persistence`. **However it is NOT a pure byte-encoder:** it encodes the `&[RecordBatch]` into
an in-memory `buffer: Vec<u8>` via `ArrowWriter` (`parquet.rs:178-194`) and then, in the SAME function
with no intervening seam, performs `object_store.put(path, buffer.into())` (`parquet.rs:197`, plain
`put` = default `PutMode::Overwrite`). There is no public NT function that returns the encoded bytes
without also putting them. Therefore v2's "reuse NT's encoder up to the buffer, replace the final put"
is **not achievable by calling NT** — the buffer never escapes.

> **Phase-0 sub-task 0.E (BLOCKING, resolves R-5).** Before any writer code is built, the projector
> author must VERIFY at the pinned rev that `write_batches_to_object_store` is `pub` (it is, at
> `parquet.rs:170`) AND decide the encoder strategy, because the public function couples encode+put:
> - **Primary (vendor the ~25 LOC encode body).** Lift the encode half of
>   `write_batches_to_object_store` (`parquet.rs:178-194`: `WriterProperties` builder with SNAPPY
>   default + `max_row_group_row_count(5000)`, `ArrowWriter::try_new(&mut buffer, batches[0].schema(),
>   props)`, write each batch, `writer.close()`) into the projector, returning `Vec<u8>`. This is
>   byte-compatible with NT's reader because it uses the same `ArrowWriter`/`WriterProperties` path NT
>   itself reads back via `read_parquet_from_object_store` (`parquet.rs:140`,
>   `ParquetRecordBatchReaderBuilder` at `parquet.rs:154`). The projector then calls
>   `object_store.put_opts(path, bytes.into(), PutMode::Create)` directly.
> - **Fallback (encode via arrow-rs).** If the vendored body drifts from NT's reader expectations,
>   encode with `arrow`/`parquet` crates pinned to NT's versions to a byte-stream NT's
>   `ParquetRecordBatchReaderBuilder` can decode, and assert round-trip equality in the Phase-0 proof
>   (write via projector → re-open via NT `query_files` → assert row/schema equality).
> - The chosen encoder's identity (vendored-LOC source hash or arrow-rs version set) is recorded in
>   `NtCapabilityProof` so a future NT rev bump that changes the encoder cannot silently desync. The
>   `key_value_metadata` capability (e.g. instrument `class` survival, `parquet.rs:185-187`) MUST be
>   preserved by whichever encoder is chosen.

**4.3.1 Encode in-process, then conditionally create.** The projector encodes the batches to bytes
(per 4.3.0) and writes with `object_store.put_opts(path, payload, PutOptions { mode: PutMode::Create,
.. })` (`object_store` lib.rs:752, `PutMode::Create` lib.rs:1708). On S3 this issues `If-None-Match: *`
(`aws/mod.rs:188-201`) and converts the precondition failure into `Error::AlreadyExists`
(`aws/mod.rs:193-198`, lib.rs:2064) — atomic, no TOCTOU, no `head()` pre-check.

**4.3.2 Conditional-put is UNREADABLE from the built store — assert via a runtime probe at
construction (resolves R-6).** S3 `PutMode::Create` requires `S3ConditionalPut::ETagMatch`; with
`S3ConditionalPut::Disabled` the put returns `Error::NotImplemented` (`aws/mod.rs:183-187`). The
resolved mode CANNOT be read back from a built `AmazonS3`:
- The `S3ConditionalPut` enum is `#[non_exhaustive]` with exactly two variants at 0.13.2 — `ETagMatch`
  (the `#[default]`, `aws/precondition.rs:127-128`) and `Disabled` (`:131`); its `from_str` is private
  (`:144`) and there is no public getter. The config lives at `client.config.conditional_put`
  (`aws/client.rs:209`) behind a `pub(crate)` client — invisible to an external consumer (the in-crate
  integration test reads it as `config.conditional_put`, `aws/mod.rs:625`, which an external caller
  cannot do).
- **NT's store builder cannot even set it.** `create_s3_store` (`parquet.rs:731-773`) maps only
  `endpoint_url`/`region`/`access_key_id`/`secret_access_key`/`session_token`/`allow_http` from
  `storage_options`; any other key (including a hypothetical `conditional_put`) hits the `_ =>`
  "Unknown S3 storage option" warn arm and is silently dropped (`parquet.rs:763-765`). So NT's
  `from_uri` path yields the crate-default `ETagMatch` only by default, with no caller control and no
  read-back.
>
> **Therefore (R-6 class fix):** the `ConditionalCatalogWriter` constructs its own `AmazonS3Builder`
> directly (so it controls `with_conditional_put`/region/creds explicitly) rather than relying on NT's
> `create_s3_store`, AND asserts the capability empirically with a **runtime probe at writer
> construction**:
> 1. Build the store. Pick a sentinel key under a writer-private probe prefix (e.g.
>    `<artifact_root>/.writer-probe/<uuid>`), NEVER under any data prefix.
> 2. `put_opts(sentinel, tiny_payload, PutMode::Create)` → MUST return `Ok`.
> 3. `put_opts(sentinel, tiny_payload, PutMode::Create)` again → MUST return `Error::AlreadyExists`
>    (proves `If-None-Match: *` is honored, i.e. `ETagMatch` is live). If instead it returns
>    `Error::NotImplemented`, conditional put is `Disabled` → **abort the run, fail loud** (never
>    degrade to overwrite).
> 4. Delete the sentinel.
> The probe transcript (store URI, observed `AlreadyExists`/`NotImplemented`, timestamps) is recorded
> in `NtCapabilityProof` and folded into `ingest_manifest.no_overwrite_proof` (4.3.5). This is the
> ONLY mechanism that proves capability, because the resolved mode is not introspectable. "Bucket
> supports conditional put" is elevated from open decision to a **Phase-0 prerequisite** (§13).

**4.3.3 Multipart (>single-PUT limit) conditional-create is NOT available on the public API — bound
object size (resolves R-6 multipart).** `put_opts(PutMode::Create)` is a SINGLE PUT carrying
`If-None-Match: *` (`aws/mod.rs:159-201`); it does NOT auto-chunk to multipart. Multipart
conditional-create exists in 0.13.2 ONLY internally: `complete_multipart(..., CompleteMultipartMode::Create)`
sets `If-None-Match: *` (`aws/client.rs:796`), but it is reachable only from the server-side-copy path
(`CopyMode::Create` with `S3CopyIfNotExists::Multipart`, `aws/mod.rs:327-372`). The **public**
`put_multipart_opts` (`aws/mod.rs:240-248`) takes `PutMultipartOptions` and accepts **no `PutMode`** —
so a streamed multipart upload of a large parquet object CANNOT carry an atomic create guard via the
public API.
> **Class fix:** the projector keeps every emitted object within S3's single-PUT limit so the atomic
> `put_opts(PutMode::Create)` path always applies — partition by `(venue, product_family,
> table_family, instrument, interval)` so per-object parquet stays well under the limit (the existing
> NT layout `data/{type}/{instrument}` and per-interval files already enforce this granularity, §9.4),
> and FAIL LOUD if any encoded buffer would exceed it (no silent fallback to a non-atomic multipart
> upload). Server-side promotion COPY uses `CopyMode::Create` (§4.4a, R-7) which DOES get the multipart
> create guard via `S3CopyIfNotExists::Multipart`; that is the only sanctioned >single-PUT atomic path
> and it is copy, not ingest (one size-keyed path, not a dual path — bolt rule 2 preserved).

**4.3.4 Object-key strategy is TWO different schemes by location (the heart of B-1(b)).**
- **Staging keys are logical-content+transform-hash-keyed and live under a physically non-NT path
  layout** (see §5.3). NT must never read staging, so staging filenames need not be NT-parseable. The
  staging key embeds `transform_hash` (code+config hash, `data-model.md:78`) and the **logical**
  content digest `content_digest` (R-3, §4.3b — a `sha256` over the *sorted, canonicalized logical
  rows*, NOT over the parquet bytes, which are non-deterministic — SNAPPY + 5000-row-group default +
  unpinned `created_by` at `parquet.rs:182-183`). Example staging key:
  `staged-research/<family>/<instrument_id>/<start>_<end>__t-<transform_hash>__c-<content_digest>.parquet`.
  Two distinct transforms over one interval are distinct objects (fixes the interval-collision drop);
  two re-runs producing the *same logical rows* land on the identical key regardless of parquet-encoding
  drift (idempotent); a colliding `PutMode::Create` (`AlreadyExists`, `aws/mod.rs:194`) is treated as an
  idempotent no-op.
- **Canonical NT-catalog keys are NT-native interval filenames** — see 4.3.4(canonical) below. Content
  / transform hashes NEVER appear in a canonical NT filename.

**4.3.4 (canonical) Canonical NT-catalog filenames MUST be NT-native, or NT silently over-includes
(B-1(b), verified).** When the writer materializes a canonical NT-catalog root it names each file with
the **exact `timestamps_to_filename` format** NT's reader expects: `format!("{ts1}_{ts2}.parquet")`
(`catalog.rs:4315-4320`), e.g.
`2026-01-01T00-00-00-000000000Z_2026-01-02T00-00-00-000000000Z.parquet`. This is mandatory, proven by
the reader:
- NT prunes by filename: `query_files` retains a file iff `query_intersects_filename` is true
  (`catalog.rs:2112`), which parses the name via `parse_filename_timestamps` (`catalog.rs:4767-4780`):
  `strip_suffix(".parquet")` → `split_once('_')` → ISO-parse each half.
- A hash-suffixed name `<start>_<end>__t-<hash>__c-<hash>.parquet` splits at the **first** `_`, so the
  "second half" is `<end>__t-<hash>__c-<hash>` → ISO-parse fails → `parse_filename_timestamps` returns
  `None` (`catalog.rs:4769-4777`).
- On `None`, `query_intersects_filename` returns **`true`** (`catalog.rs:4740-4742`) — the file is
  included in **every** requested `[start,end]` window. It does **not** crash (Gemini's "reader will
  crash" mechanism is wrong — disproven at `catalog.rs:4741`). The real failure is worse and silent:
  the file is loaded **regardless of the query window**, and because the list is naive
  (`catalog.rs:2040-2063`) **every transform version present for an interval loads together** — silent
  over-inclusion of out-of-window and superseded bytes into `BacktestNode` (CLAUDE.md rule 2 violation).
- Therefore canonical roots are **interval-disjoint with exactly one live `*.parquet` per
  `(type, instrument, interval)`** (mirrors NT's own `are_intervals_disjoint` invariant,
  `catalog.rs:4808-4828`). Content/transform-hash keying is **staging-only**; it is structurally
  impossible for a hash-suffixed name to exist under a canonical NT-catalog root because canonical
  roots are materialized only by the promotion path (§4.4), which writes NT-native names exclusively.

**4.3.5 Concurrency proof + `no_overwrite_proof` (BLOCKER acceptance criterion).** Spawn N concurrent
writers racing the same logical artifact against the configured store (or a `PutMode::Create`-capable
conformance store — MinIO/R2 with `ETagMatch`, `aws/precondition.rs:120-128` — when offline;
LocalFileSystem's Create semantics differ and do NOT exercise the S3 `If-None-Match` path) and assert
exactly one PUT wins and the losers observe `AlreadyExists` (`aws/mod.rs:194`) — never two distinct
successful PUTs to one key, never a silent overwrite. The race uses two independently-encoded parquet
buffers built from the *identical logical rows* (force a parquet-byte difference, e.g. by toggling
row-group size) to prove the key — and therefore the collision — is decided by the logical digest
(§4.3b), not by the parquet bytes. The proven layer's identity (store URI, the construction-time
conditional-put probe transcript from 4.3.2, the encoder identity from 4.3.0, the concurrency-proof
transcript hash) is recorded as `ingest_manifest.no_overwrite_proof` (`backfill-source-proof-schema.md:95`);
absence of an accepted `no_overwrite_proof` blocks any `local_staging` or `canonical_s3` write.

Every staged and canonical object — market data, custom data, AND instruments (§4.5) — goes through
this layer. NT's `ParquetDataCatalog` write methods are never called for platform-root writes
(§4.1 scope-boundary note).


### 4.3b Canonical logical-content digest (the single definition; resolves R-3)

`content_digest` is a `sha256` over a **canonical, encoding-independent byte image of the logical row
values**, never over the parquet container. It is defined once here; every staging key, every
`PromotionPackage` entry (§4.4), and the `ArtifactIndex.content_hash` field (`data-model.md:131`) for a
parquet data object resolve to THIS value (single source of truth). The canonicalization is fully
specified so two implementations (and the §4.3.5 race) agree byte-for-byte:

1. **Decode to the logical relation.** Take the encoded `RecordBatch` set (the same batches passed to
   `write_batches_to_object_store`, before `ArrowWriter::close`). The digest is a function of the
   Arrow logical values, not of any parquet artifact.
2. **Canonical schema.** Sort fields by name (UTF-8 bytewise). Each field contributes `name` +
   fully-qualified Arrow `DataType` (including timestamp unit/timezone, decimal precision/scale, and
   list/struct child types) + nullability, serialized as a fixed, documented tag sequence. This pins
   the column set and types so a schema change (e.g. a unit drift) changes the digest. Parquet-only
   concerns — compression, row-group size, dictionary encoding, `created_by`, page statistics — are
   NOT part of the schema image.
3. **Canonical row order.** Concatenate all batches into one logical table, then sort rows by the
   table's declared sort key, defaulting to `(nt_instrument_id, event_time_ns, source_sequence,
   <all remaining columns in canonical schema order>)` so the order is total and deterministic
   regardless of how batches were chunked or how the transform happened to emit them.
4. **Canonical per-row, per-value encoding.** Encode each row's values with **Arrow's row format via
   `arrow_row::RowConverter`** — the exact primitive NT already uses for its own deterministic row
   identity in `deduplicate_record_batches` (`parquet.rs:211-249`: `RowConverter::new(fields)` then
   `converter.convert_columns(...)`, comparing rows by their canonical `Vec<u8>` byte sequence). This
   reuses an in-tree dependency (`arrow-row`, `crates/persistence/Cargo.toml:69`) — no new crate — and
   gives an order-preserving, type-aware, NULL-distinguishing byte image of the logical values that is
   independent of parquet entirely. Decimal/price/size columns are preserved as their exact
   decimal-string / fixed-scale logical values (Common Identity decimal-preservation rule, S3), so
   floating-point serialization drift cannot perturb the digest.
5. **Digest.** `content_digest = sha256( canonical_schema_image || 0x00 || concat(row_images in
   canonical order) )`. The `0x00` domain separator prevents schema/row-boundary ambiguity.

> **Why NOT hash the parquet bytes (R-3 root cause).** Parquet serialization is not deterministic
> across runs or builds. NT's encoder (`parquet.rs:181-194`) builds an `ArrowWriter` with
> `Compression::SNAPPY` and `max_row_group_row_count(5000)` but never pins `created_by`, so the
> parquet footer embeds `DEFAULT_CREATED_BY = concat!("parquet-rs version ", env!("CARGO_PKG_VERSION"))`
> (`parquet-58.3.0/src/file/properties.rs:51`, defaulted at `:609`). The bolt-v2 cargo registry already
> holds parquet-rs `57.3.0`, `58.1.0`, and `58.3.0` — so the *same logical rows* produce a *different*
> footer (hence a different `sha256` over the parquet bytes) the moment the transitive parquet-rs
> version moves. Compression-codec version, row-group boundary placement, dictionary/encoding
> selection, and floating-point value serialization compound this. Hashing parquet bytes (v2 §4.3
> step 3 said "sha256 of the parquet bytes") therefore makes the key change on a no-op rebuild,
> defeating `PutMode::Create` dedup: every re-run writes a *new* object, inflating object counts and
> falsifying the idempotency claim. `data-model.md:131` only states `content_hash` is "a `sha256`
> value; S3 ETag is not the content hash" — it never mandates hashing parquet bytes; the byte-hash
> qualifier was a v2 error and is removed.

**Determinism contract (test, ships with the §4.3 BLOCKER task).** (a) Encode the *same* logical rows
twice with deliberately different parquet `WriterProperties` (toggle `max_row_group_row_count` 5000 vs
500; toggle SNAPPY vs uncompressed; override `created_by`) → assert the parquet `sha256`s **differ**
but the `content_digest`s are **identical**. (b) Re-order the input batches and re-chunk them → assert
`content_digest` is unchanged. (c) Change one logical value (one price tick) → assert `content_digest`
changes. (d) Run the §4.3.5 N-writer race with the two byte-different/logically-identical buffers →
assert exactly one `Create` wins on the digest-keyed path. This is the mechanical proof that the
idempotency claim holds across parquet-encoding drift, closing R-3.

---

### 4.4 Canonical promotion = immutable per-commit NT-native root + SnapshotSet CAS — resolves F15, B-1(a)+(b), R-2, R-7, R-10, R-13, R-14

This is the largest v3 change. **The canonical NT catalog is never an in-place, mutated, shared prefix.
It is materialized fresh, per commit, into an immutable root keyed by snapshot-set id, AFTER the
pointer CAS, with NT-native filenames only.** No staging prefix is ever re-pointed, renamed, or copied
wholesale. Promotion is the commit of an explicit promotion package, never a prefix operation.

**The class fix — per-commit immutable roots + a single set-atomic pointer:**

1. **Build a `PromotionPackage`** (typed artifact under `research-analytics/v1/promotion-packages/`,
   `data-model.md:149`). It enumerates, by exact value, every object being promoted: exact accepted
   staging object URI (the content+transform-hash-keyed key); `content_digest` (§4.3b, recorded as
   `ArtifactIndex.content_hash`, `data-model.md:131`); `source_proof_id` + `source_proof_version`,
   which MUST resolve to a real `accepted` `SourceProofReport` (`data-model.md:84-86,107`);
   `transform_hash` (`data-model.md:78`); and each source object's recorded byte size (used by the
   §4.4a copy-size routing). No prefix, glob, or "everything under X" enumeration is permitted (closes
   F15's prefix-flip). The package records `built_at` and is itself an immutable typed artifact with
   `commit_state=staged` until the pointer commit (step 5).

2. **Reject anything unaccepted/orphan/failed-run at BUILD time (R-10).** Package construction fails
   loud (never silently skips) if any enumerated object: (a) has no `accepted` SourceProofReport; (b)
   has `commit_state` of `orphan`, `superseded`, or `recovered_orphan` (`data-model.md:135` + §6.4);
   (c) whose recomputed §4.3b `content_digest` does not match the recorded value; (d) whose
   `transform_hash` is not the one tied to the accepted proof. Any staged object NOT enumerated is
   simply never promoted. Recovered-orphan bytes are never silently promotable (R-10):
   `backfill_accept_staged_objects.py --from-s3-keys` carries a distinct `recovered_orphan` state (§6.4)
   and may enter a package only after a human-reviewed `recovered_orphan → staged` transition, a
   *resolvable accepted* `source_proof_id`, a **full** (not sampled) hash verify, and complete
   provenance.

3. **Re-verify each object's content_digest AT WRITE time and abort on mismatch (resolves R-13).**
   There is a TOCTOU window between package build (T1) and canonical materialization (T2): a staging
   object can be re-labeled `orphan`/`superseded`, re-written, or removed in between. Therefore, for
   EVERY enumerated object, immediately before materializing it canonically the promoter (i) re-reads
   the live staging object, (ii) recomputes the §4.3b `content_digest` from its logical rows, and
   (iii) asserts it equals the value recorded in the package (which equals the `__c-<content_digest>`
   segment of the staging key). On any mismatch — or if the staging object is missing, or its
   `commit_state` is no longer `staged` — the promotion **aborts the entire package** (fail-loud, no
   partial commit) and emits no pointer. Re-verifying the LOGICAL digest (not parquet bytes) means a
   benign re-encode of identical logical rows does NOT spuriously abort, while any logical change does.
   This at-write check is mandatory even though step 2 already checked at build time; only the at-write
   check closes the T1→T2 window.

4. **One commit spans ALL kinds for a run (R-2).** A run that produces both an `nt_catalog` kind and a
   `normalized`/Tier-C kind (and the `instruments` lane) is promoted in **one** PromotionPackage whose
   committed `SnapshotSet` enumerates the object set for *every* kind. Readers pin that single committed
   set, never per-kind `latest.json` pointers independently, so a backtest can never observe
   `nt_catalog` from commit K+1 alongside `normalized` from commit K. (See §4.5 for the read-side pin
   and the proof; resolves §13 open sub-question.)

5. **Materialize a fresh immutable NT-catalog root keyed by snapshot-set id, via server-side copy
   (R-7, §4.4a).** The promotion allocates a new, never-before-used canonical root URI keyed by the
   snapshot-set id, e.g. `nt-catalog/sets/<snapshot_set_id>/` (each becomes a distinct NT `base_path`,
   `catalog.rs:318-322`). Each promoted object is placed at its NT-native interval path under that root
   — `<root>/data/<type>/<safe_instrument_id>/<start>_<end>.parquet` (`make_path` `catalog.rs:2841-2849`
   + `timestamps_to_filename` `catalog.rs:4315-4320`), interval-disjoint, exactly one live file per
   interval (§4.3.4 canonical). Materialization uses **backend-native server-side copy** (§4.4a), never
   download+reupload. The new root is **immutable**: once a snapshot-set id is committed, its root is
   never appended to or mutated.

   > **Instrument-directory charset constraint (campaign-branch finding, 2026-06).** `object_store`'s
   > path layer percent-encodes, at write time, every non-ASCII character plus a specific INVALID
   > ASCII set in each path segment (controls plus backslash, braces, caret, percent, backtick,
   > square brackets, double quote, angle brackets, tilde, hash, pipe, asterisk, question mark, CR,
   > LF — note `~` IS encoded despite being RFC 3986 unreserved; other ASCII punctuation is stored
   > verbatim). NT's identifier-filtered catalog queries cannot match a percent-encoded
   > `<safe_instrument_id>` directory (upstream nautechsystems/nautilus_trader#4259); the
   > backtesting-vertical-slice read path on the campaign branch works around this with explicit
   > file-list queries plus a manifest-level instrument-id charset validator admitting only
   > alphanumeric / `.` / `_` / `-` — a deliberately CONSERVATIVE strict subset of what object_store
   > stores verbatim, so admission never depends on per-character encode-set knowledge (over-strict
   > early rejection is recoverable; an admitted-but-encoded id is a guaranteed late failure). The
   > `ConditionalCatalogWriter` and canonical-root naming MUST fix the charset policy at design
   > time: canonical `<safe_instrument_id>` directory names are restricted to the same conservative
   > subset, and ids outside it are rejected fail-loud at admission — a canonical root must never
   > depend on NT's filtered-query path matching a percent-encoded directory name.

6. **Commit the whole package as ONE snapshot SET, advanced by a single CAS (R-2).** Promotion becomes
   "live" only after every object in step 5 is materialized, by atomically advancing exactly ONE hot
   pointer:
   - For every kind in the package, append an immutable index event (`event_uri` under
     `artifact-index/v1/events/kind=<kind>/`, `data-model.md:124-125`) and write an immutable committed
     per-kind snapshot (`snapshot_uri` under `artifact-index/v1/snapshots/kind=<kind>/`,
     `data-model.md:127-128`) referencing the PromotionPackage, that kind's enumerated object set, AND
     the immutable NT-catalog root URI for this commit, via `PutMode::Create` (`aws/mod.rs:189`). These
     are immutable and may be written in any order — none is reachable by a reader yet (§4.5: readers
     resolve the SET, not per-kind snapshots, so a half-written set is invisible).
   - Write **one immutable `SnapshotSet` record** that names, by exact `snapshot_id` + `content_hash`,
     the committed snapshot of **every** kind in the package (an `artifact_index`-kind artifact,
     `data-model.md:117`; its `lineage_ids` carry the per-kind snapshot ids + `sha256`,
     `data-model.md:132-133`), via `PutMode::Create`.
   - Advance **exactly one** hot pointer — `artifact-index/v1/pointers/set/latest.json` — from the
     prior `SnapshotSet` id to the new one by compare-and-swap: `put_opts(pointer, payload,
     PutMode::Update(UpdateVersion { e_tag, .. }))` (lib.rs:1711; S3 `If-Match: <etag>` `aws/mod.rs:208-210`;
     documented 409-retry `aws/mod.rs:215`). A lost race surfaces `Error::Precondition`
     (`aws/mod.rs:223-224`) and the promotion **rebuilds the package against the new set and retries**
     — never blind-overwrites.
   - The per-kind `latest_pointer_uri` (`data-model.md:129-130`) is **generated from the committed set,
     never independently CAS'd.** It is a derived convenience pointer (regenerated to match the live
     `SnapshotSet`), never the authority a reader resolves. This removes the dual-pointer race at its
     root (bolt rule 2 NO DUAL PATHS — one authority for "what is live").
   - Flip the package's and enumerated artifacts' `commit_state` `staged → committed`
     (`data-model.md:135`) only as recorded *inside* the committed `SnapshotSet`, never by mutating
     staging objects.

   The single `set/latest.json` CAS is the **only** linearization point. Before it: the entire new set
   (all kinds) is invisible. After it: the entire new set is live. There is no intermediate state in
   which one kind is new and another is old.

7. **The hot pointer names the active root; NT is pointed at THAT root.** A backtest resolves its
   `catalog_path` from the committed `set/latest.json` → `SnapshotSet` → per-kind snapshot, which names
   the immutable `nt-catalog/sets/<snapshot_set_id>/` root. NT is pointed at that root URI
   (`create_catalog` `node.rs:507-516`), so it lists only that one immutable, NT-native,
   interval-disjoint `data/` tree. **A lost CAS leaves an unreferenced root** that no committed pointer
   names: NT is never pointed at it, `query_files` never lists it, and it is garbage-collectible — it
   is structurally impossible for a backtest to read it. Because every root is immutable and the pointer
   flips atomically between whole roots, there is no window in which NT reads a half-built or superseded
   view. The over-inclusion failure (§4.3.4 canonical) cannot occur because canonical roots contain only
   NT-native interval names.

8. **Synthetic vs provider root isolation (R-14).** Phase-0/Phase-3 synthetic capability proofs commit
   to a **distinct synthetic catalog-root URI** (e.g. `nt-catalog-synthetic-proof/<run_uuid>/`, §10.2)
   that is never commingled with any provider root; a provider backtest can never accidentally resolve a
   synthetic root through the pointer. The proof harness asserts (fail-loud) that its configured catalog
   root matches the synthetic-root pattern and is disjoint from every provider/canonical prefix before
   it writes a single byte — mechanical, not naming-convention.

`write_mode` reaches `canonical_s3` and `commit_state` reaches `committed` only as a consequence of a
successful pointer commit of an accepted package into a fresh immutable root — never as a manual flag
flip on a prefix.


### 4.4a Promotion materialization uses backend-native server-side copy, never download+reupload (resolves R-7)

A download-then-reupload materialization (get each accepted staging object into the build host,
re-buffer, `put_opts(Create)` it back) would stream **~390 GiB** of S3 GET egress + ~390 GiB of PUT
ingress through the build host (volumes per §9.1: ~122.69 GiB excl. Polymarket + ~267.12 GiB
Polymarket) for objects whose bytes are **already final** — promotion is a key relocation, not a
re-derivation. That is wasted egress, wall-clock, and a fresh corruption surface (a re-encode can change
bytes, R-3). It is the wrong primitive.

**Class fix: materialize canonical objects by backend-native server-side copy.** The promoted object's
canonical bytes are identical to the accepted staging object's bytes; promotion only changes the
**object key** (staging `<family>/<instrument_id>/<start>_<end>__t-<transform_hash>__c-<content_digest>.parquet`
→ canonical NT-native `<root>/data/<type>/<instrument_id>/<start>_<end>.parquet`). A same-store key
relocation is exactly what `object_store`'s copy primitive does.

**Verified API at `object_store` 0.13.2 (this rev ships the unified `copy_opts`/`CopyOptions` surface):**

- The trait method is `ObjectStore::copy_opts(&self, from: &Path, to: &Path, options: CopyOptions)`
  (`lib.rs:1111`). `CopyOptions { mode: CopyMode, extensions }` (`lib.rs:1891`); `CopyMode::{Overwrite,
  Create}` (`lib.rs:1880-1888`). Ergonomic wrappers exist on `ObjectStoreExt`: `copy` →
  `CopyMode::Overwrite` (`lib.rs:1386-1389`) and `copy_if_not_exists` → `CopyMode::Create`
  (`lib.rs:1391-1394`). `from` and `to` are arbitrary paths in the **same** store, so the
  staging→canonical key rename is a first-class operation.
- On the S3 backend, `copy_opts` (`aws/mod.rs:312`) issues a server-side **`CopyObject`**: it calls
  `copy_request(from, to)` which sets the `x-amz-copy-source: <bucket>/<from>` header
  (`aws/client.rs:596-597` doc-links the AWS `CopyObject` API; header emitted at `aws/client.rs:702`).
  The bytes are copied S3-side; **no object data transits the build host.** GET egress for canonical
  materialization is therefore **zero**; the only per-object cost is one `CopyObject` request.

**Promotion uses the create-only copy variant, preserving §4.4's no-overwrite guarantee:**

- `CopyMode::Create` (`copy_if_not_exists`) is the correct semantic: a canonical-path collision must be
  `AlreadyExists` (a prior accepted identical artifact = idempotent no-op, `aws/mod.rs:354-357,386-389`),
  never a silent overwrite — the same guarantee §4.4 step 5 previously got from `PutMode::Create`.
- **Critical capability nuance (Phase-0 prerequisite, ties to R-6):** S3 `CopyMode::Create` is a
  **distinct** capability from `put`'s conditional create. It requires the store's `copy_if_not_exists`
  to be configured (`S3CopyIfNotExists::{Header, HeaderWithStatus, Multipart}`,
  `aws/precondition.rs:28-62`); when unset, `copy_opts(CopyMode::Create)` returns `Error::NotSupported`
  ("S3 does not support copy-if-not-exists", `aws/mod.rs:374-378`). This is **separate from** the
  `S3ConditionalPut::ETagMatch` the `ConditionalCatalogWriter` already probes (§4.3.2, R-6). The
  `Multipart` variant performs the create-conditional copy as a server-side `UploadPartCopy`
  (`PutPartPayload::Copy` sets `x-amz-copy-source`, `aws/client.rs:701-704`) whose `complete_multipart`
  uses `CompleteMultipartMode::Create` → `If-None-Match: *` (`aws/client.rs:790-796`) and maps the
  precondition failure to `Error::AlreadyExists` (`aws/mod.rs:354-357`) — still fully server-side.
- **Object-size routing (one path, size-keyed — not a dual path).** S3 single-request `CopyObject` caps
  at 5 GiB; objects above that must use multipart `UploadPartCopy`. The promotion writer selects on the
  source object's known size (from the PromotionPackage's recorded metadata, §4.4 step 1): ≤5 GiB →
  single-request server-side copy; >5 GiB → multipart server-side copy. Both are the same
  `copy_opts`/`CopyMode::Create` call on the configured `S3CopyIfNotExists::Multipart` store, which
  internally drives `UploadPartCopy` — selection within one server-side primitive, not a second
  materialization route. Per §9.1 the largest family (Polymarket) averages ~0.36 GiB/object, so the
  multipart-copy branch is expected to be unused in practice but is specified so an oversized object
  never silently falls back to download+reupload.
- The bytes copied are the exact accepted bytes, so the canonical object's `content_hash` is identical
  to the verified staging object's (§4.3b logical digest) — no re-encode, no R-3 nondeterminism
  (re-keying ≠ re-deriving).

**Phase-0 capability prerequisite (folds into §4.3.2 / R-6).** The construction-time capability probe
for the canonical store must assert **both** `conditional_put` live (for staging `PutMode::Create`,
§4.3.2) **and** `copy_if_not_exists` configured (for promotion `CopyMode::Create`). A store that
supports one but not the other aborts before any promotion. Probe via a sentinel server-side copy
(`copy_if_not_exists` of a tiny sentinel to a fresh key, assert success; a second `copy_if_not_exists`
to the same key asserting `AlreadyExists`; then delete) at writer construction — mirroring the §4.3.2
conditional-put probe.

### 4.4b Staging-cleanup policy (resolves R-13)

Staging is durable until explicitly reclaimed; cleanup is a policy that MUST be safe against in-flight
promotions. There is no cleanup/delete/lifecycle code in the staging path today
(`backfill_accept_staged_objects.py` only *binds* and content-verifies objects — it never deletes;
grep for `delete`/`cleanup`/`lifecycle`/`prune` over the script returns nothing), so this is a new,
single-owner policy added with the promotion task — not a retrofit onto existing deletion logic.

1. **A staged object referenced by ANY constructed-but-uncommitted `PromotionPackage` is never
   deleted.** Cleanup MUST treat every `PromotionPackage` with `commit_state=staged` (built, pointer
   not yet committed) as a live pin over every staging URI it enumerates. The cleanup planner reads the
   set of staged packages, unions their enumerated object URIs into a protected set, and excludes that
   set from any delete. This is the structural fix for the R-13 build-at-T1 / write-at-T2 window: an
   object cannot be both pinned by an uncommitted package and reclaimed.
2. **Eligible-for-cleanup set is the complement of: (a) the protected set above, (b) any object whose
   `commit_state` is `staged` and is younger than the configured retention floor, and (c) any object
   referenced by the live committed `SnapshotSet`.** Objects already promoted (canonical bytes live
   under `nt-catalog/sets/<snapshot_set_id>/`) whose staging copy is neither pinned nor within retention
   are reclaimable; `orphan`/`superseded`/un-reviewed `recovered_orphan` objects with no package pin are
   the primary reclaim target.
3. **Cleanup is fail-safe, never fail-deleting.** If the cleanup planner cannot enumerate the staged
   `PromotionPackage` set (e.g. the package prefix LIST fails), it aborts and deletes nothing — it never
   falls back to a time-only or prefix-only delete that could remove a pinned object. Deletion is
   per-exact-URI (the same discipline as promotion enumeration in §4.4 step 1); never a prefix or glob
   delete.
4. **Promotion is robust to a concurrent cleanup regardless of policy.** Even if (1) were violated, the
   §4.4 step-3 at-write re-verification aborts the whole package the moment a referenced object is
   missing or its digest changed — so a deleted/mutated pinned object can never silently produce a wrong
   canonical commit. The cleanup pin (1) prevents the *spurious abort*; the at-write re-verify (§4.4
   step 3) prevents the *wrong commit*. Both are required; neither alone closes R-13.

**Test (ships with the promotion task).** (a) Build a `PromotionPackage` over staging object X (do not
commit), run the cleanup planner → assert X is in the protected set and not deleted. (b) Mark X
`orphan` and delete it out-of-band, then run promotion → assert the package aborts at the §4.4 step-3
re-verify with no pointer advance and no partial canonical objects left readable. (c) Re-encode X's
identical logical rows with different parquet `WriterProperties` between build and promote → assert the
at-write re-verify PASSES (logical digest unchanged) and the promotion commits. (d) Change one logical
value in X between build and promote → assert the at-write re-verify FAILS and the package aborts.

### 4.5 Cross-kind atomicity, the run-pinned reader rule, and the instruments-lane writer (resolves R-2, R-4)

**Why a per-kind pointer is not enough (NT read-path, verified at `6be5a50`).** NT's reader has zero
pointer/snapshot/set awareness and re-resolves the catalog *independently, multiple times, at different
wall-clock moments within one run*:

- A `ParquetDataCatalog` is bound to exactly ONE `base_path` + ONE `object_store` at construction;
  `from_uri` stores `location.base_path` / `location.object_store` and nothing else
  (`catalog.rs:307-330`). It has no field for a pointer, snapshot, or set.
- Every read recursively LISTs `{base_path}/data/<type>/`. `query_files` builds
  `base_dir = self.make_path(data_cls, None)` then `self.object_store.list(Some(&prefix))` over
  `"{base_dir}/"` (`catalog.rs:2040-2051`); `query_instruments_filtered` does the identical fresh LIST
  over `data/instruments/` (`catalog.rs:883-896`). `make_path` proves all kinds are sibling subtrees of
  one root: `vec!["data", type_name]` joined to `self.base_path` (`catalog.rs:2841-2851`). So to NT,
  `nt_catalog` time-series, `normalized`, and `instruments` are **`data/<type>/` siblings under one
  `base_path`** — discovered by independent LIST calls, each a fresh snapshot of whatever bytes exist at
  that instant.
- `BacktestNode` LISTs **at least twice per run, on N separately-constructed catalogs**: `build()`
  constructs a fresh catalog **per `data_config`** and queries instruments (`node.rs:160-182`, via
  `create_catalog`→`from_uri`, `node.rs:507-517`); then `run()` re-constructs a fresh catalog **per
  `data_config`** and LISTs the time-series at load time (`run_oneshot` `node.rs:378-386` → `load_data`
  `node.rs:519-527`; multi-config streaming `run_streaming`/`load_and_merge_data` `node.rs:397-418,
  492-505`). Each `create_catalog` is an independent `from_uri` over that config's `catalog_path`
  (`node.rs:508-516`).

**The race (concrete).** A run with a `data_config` for trades + a `data_config` for quotes + an
instruments load is, in NT, three-plus independent LISTs at three different instants. If a promotion
advances the live set between the `build()` instruments LIST and the `run()` trades LIST — or between
the trades LIST and the quotes LIST — the engine ingests instruments from the old universe and trades
from the new one, or trades and quotes from two different transform generations. NT never crashes and
never warns: each LIST simply returns whatever `.parquet` objects exist under that prefix at that
instant. This is a silent mixed read — exactly the fail-loud violation R-2 names (CLAUDE.md rule 2).

**Class fix — the reader pins one committed `SnapshotSet` for the entire run.** The fix is NOT to make
NT pointer-aware (it cannot be, at the pin). It is to resolve every `catalog_path` a run will touch from
ONE committed `SnapshotSet` captured ONCE at run start, before any NT catalog is constructed, and to
point NT at **immutable per-set roots** so even concurrent promotion cannot mutate the bytes under a
path NT is enumerating:

1. **Immutable per-set NT roots (builds on §4.4).** Canonical NT bytes for a committed set live under an
   immutable, set-id-keyed root, `nt-catalog/sets/<snapshot_set_id>/data/<type>/...` with NT-native
   filenames (`<start>_<end>.parquet`, `catalog.rs:4319`). A new promotion writes a NEW set root; it
   never adds, renames, or deletes a `.parquet` under an already-committed set root. A run whose catalogs
   are rooted at `<snapshot_set_id>` reads a frozen byte set regardless of any in-flight promotion — the
   LIST-at-different-instants hazard is neutralized because the listed prefix is immutable for that set's
   lifetime.
2. **Run-start pin = one read of `set/latest.json`.** At run construction, the replay driver reads
   `artifact-index/v1/pointers/set/latest.json` exactly once, records the resolved `snapshot_set_id`
   (+ its `content_hash`) into the run manifest (the same run manifest that already carries
   `source_proof_ids`, §5.2 rule 3), and resolves the per-kind committed snapshots **from that pinned
   set only**. This is the single point at which "what is live" is observed for the whole run.
3. **Every `catalog_path` is derived from the pinned set, never from a live pointer.** The driver
   constructs each `BacktestDataConfig.catalog_path` (`config.rs:662`) as the immutable per-set root for
   that kind. Because NT's `from_uri` simply stores whatever base it is given (`catalog.rs:321-329`),
   pinning is achieved entirely on the bolt side by what string we hand NT — no NT change is required.
   All N `data_config` catalogs in `build()` and `run()` resolve to the SAME pinned set, so the multiple
   independent LISTs (`node.rs:160-182` build, `node.rs:378-386`/`492-505` run) all enumerate the same
   immutable roots. A promotion that lands mid-run advances `set/latest.json` to a new set root the
   running node never references; the next run picks it up. **A backtest started mid-promotion sees
   either the old set or the new set, never a mix** — the acceptance criterion for R-2.
4. **Set membership is consistency-checked at pin time, fail loud.** When the driver resolves the pinned
   `SnapshotSet`, it asserts that every kind the run's `data_config`s require is present in that set with
   a matching `content_hash`, and that all referenced `source_proof_id`s resolve to `accepted`
   (`backfill-source-proof-schema.md:90`; reuses the §5.2 rule-3 assertion). A run that requests a kind
   absent from the pinned set, or whose recomputed set `content_hash` mismatches, aborts before any
   catalog is constructed — never silently falls back to a live LIST.

**Reader rule (normative, one sentence).** A backtest run resolves
`artifact-index/v1/pointers/set/latest.json` exactly once at run start, pins the resulting
`snapshot_set_id`, and derives every `catalog_path` (all kinds: `nt_catalog` time-series, `normalized`,
`instruments`) from that one immutable set root for the whole run; the per-kind `latest.json` pointers
are derived convenience views, never the resolution authority. (Added to §15.)

**Acceptance proof (Phase 0, alongside the §4.3.5 conditional-write concurrency proof).** Spawn a
backtest run and a competing promotion against a `PutMode::Create`-capable store; the promotion advances
`set/latest.json` to a new set after the run's pin but before its trades LIST. Assert the run's
instruments LIST and trades LIST both resolve to objects under the **pinned** set root (single
`snapshot_set_id` in the run manifest), never a mix of old-instruments + new-trades. A second run after
the swap must observe the new set. This is a BLOCKER acceptance criterion, same tier as the §4.3.5
no-overwrite concurrency proof.

**Instruments-lane writer (resolves R-4).** The instruments lane needs its own thin writer because NT's
only instrument-write entry point (`write_instruments`, `catalog.rs:726`) is non-atomic (§2.1 hazard)
and is therefore never called for platform-root writes (§4.1 scope-boundary note):

1. **Encode.** Build the same `InstrumentAny` `RecordBatch`es NT would (via NT's
   `data_to_record_batches` path used inside `write_instruments`, `catalog.rs:762`, reused read-only),
   then encode to bytes with the projector encoder (§4.3.0). Preserve the `class` `key_value_metadata`
   NT relies on for instrument round-trip (`catalog.rs:802-803` notes the ARROW:schema `class`
   metadata; the encoder MUST carry it via `key_value_metadata`, `parquet.rs:185-187`) — otherwise
   `query_instruments` cannot reconstruct the concrete instrument type.
2. **Write conditionally.** `put_opts(path, bytes, PutMode::Create)` to the canonical NT layout
   `data/instruments/<safe_instrument_id>/<iso_start>_<iso_end>.parquet` (NT-native filename so
   `query_instruments` can enumerate it). Staging instruments use the non-NT
   `staged-research/instruments/...` layout (§5.3); promotion materializes the NT-native canonical path
   via §4.4a server-side copy.
3. **Exempt from the `event_time_source` guard.** Per §2.1, the instruments lane is tagged
   `time_series = false`; the §7.3 guard is a no-op for it. A current-snapshot instrument source is a
   valid instrument definition, not a forbidden time-series emission.
4. **Read-back proof.** The Phase-0 capability proof writes ≥1 synthetic instrument via this lane and
   re-opens it with `query_instruments` (`catalog.rs:858`) to assert the concrete instrument type and
   `class` metadata survive — proving the projector encoder is `query_instruments`-compatible.


---

## 5. Contract gate handling + proof-acceptance precedence (resolves F14)

### 5.1 The gate (unchanged discipline)

The approval gate (`backfill-table-contract.md:292-309`) is respected by NEVER flipping the
ingest-manifest `write_mode` to `canonical_s3` and never writing under canonical `artifact_root`
prefixes until ALL gate items are approved: (a) artifact_root URI + prefix schema; (b) a
`SourceProofReport` per `(venue, product_family, table_family)` with all `required_checks` PASS;
(c) one portable sample raw payload + checksum per source family; (d) parser schema sample with row
counts + timestamp range; (e) instrument-universe manifest (best-effort + completeness gap record per
§8); (f) the expanded one-row-per-`(venue,product_family,table_family)` evidence matrix incl.
`nt_target`; (g) gap policy with max gap frequency/duration + forbidden_claims; (h) HIP-4 quoteToken
parser-fidelity proof before any HIP-4 normalized write; (i) idempotent/create-only/no-overwrite
write-manifest format (the proven `ConditionalCatalogWriter`, §4.3); (j) owner-declared minimum
instrument-universe completeness bar (§8c). **v3 does not authorize any of these — they remain the
owner gate.**

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
bytes and STEP 8 ran `BacktestNode` over them — a backtest from an un-proven source. The fact that the
write lands in a non-canonical staging prefix narrows the canonical-write violation but does NOT cure
the contract clause, which gates *backtest input* (any replay read), not just *canonical writes*.

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
   replay step asserts this before reading (at the §4.5 run-start pin) and fails loud otherwise.

This keeps E-002 intact: NT `ParquetDataCatalog` remains the replay/backtest projection target; raw
provider payloads remain audit input, not canonical replay input (`spec.md:101`).

### 5.3 Interim research-ready-in-staging is Tier-C-ONLY and physically NON-NT (resolves F14 hole / B-1(c))

v2's §5.3 allowed staging to generate "normalized tables **+ an NT catalog**" under a non-canonical
prefix guarded only by "humans won't point `BacktestNode` at it." That guard is **social, not
mechanical** (Kimi NF-3): a pending-proof, NT-layout, provider-byte path that NT *could* enumerate if
ever pointed at it. v3 removes the failure class by making it **structurally impossible for NT to read
staging at all.**

**Three structural rules:**

1. **Staging NEVER emits a Tier A/B NT-replayable catalog.** Interim staging materializes only Tier-C
   research Parquet (and the operational provenance tables). It does **not** write any
   `OrderBookDepth10`/`QuoteTick`/`TradeTick`/… catalog under a Tier A prefix. The NT-replayable catalog exists **only** as a committed per-commit root produced
   by promotion (§4.4). There is no NT catalog in staging to be mis-read.

2. **Staged data lives under a physically non-NT path layout NT cannot enumerate.** Staged research
   Parquet is written under a prefix that is NOT `data/<type>/` — e.g.
   `staged-research/<family>/<instrument_id>/<file>` (the hash-keyed staging filename, §4.3.4). NT's
   reader only ever lists `<base_path>/data/<type>/` (`make_path` `catalog.rs:2841-2849`; `query_files`
   lists `{base_dir}/` `catalog.rs:2040-2045`; `list_parquet_files` lists `{directory}/`
   `catalog.rs:1273-1287`). Because no staged object is under a `data/<type>/` path, NT's
   `make_path`/`query_files`/`list_parquet_files` **cannot enumerate any staged object even if a root
   URI were mistakenly aimed at the staging prefix** — the `data/<type>/` subtree it lists is empty. The
   hash-suffixed staging filenames are also non-NT-parseable (`parse_filename_timestamps`→`None`,
   `catalog.rs:4769-4777`), a second, independent barrier.

3. **A manifest/catalog validator enforces both, fail-loud.** A validator rejects, before any
   `BacktestResultContract` is emitted: (a) any object carrying a `v0-pending`/non-accepted
   `source_proof_id` (the row-id provisional segment of §6.3 — R-9: this segment is a row-id marker, NOT
   the `source_proof_version` field, which is the positive integer `1` on a `status=pending` record)
   that is present under ANY Tier A/B NT prefix (`data/quotes/`, `data/trades/`, `data/order_book_deltas/`,
   `data/order_book_depths/`, `data/bars/`, `data/index_prices/`, `data/mark_prices/`,
   `data/funding_rate_update/`, `data/instrument_status/`, `data/instrument_closes/`, `data/instruments/`);
   (b) any non-NT-native filename (one not matching `timestamps_to_filename`, `catalog.rs:4315-4320`)
   under any committed canonical NT-catalog root. The validator runs as a gate before promotion (§4.4)
   and before any provider-derived `BacktestResultContract` (§5.2 rule 3).

**Staging mechanics (unchanged discipline).** Write Tier-C normalized tables + the operational
provenance tables under the NON-canonical staging prefix using `write_mode=local_staging` +
`staging_location=s3_noncanonical` (§6.2) and `commit_state=staged`. Every staged artifact carries the
deterministic provisional `source_proof_id` (§6.3 — never literal `pending`), records its
`fidelity_class`, and attaches `forbidden_claims` (snapshots≠native deltas, bars≠trades, aggTrades≠native
trades, fixed-depth≠full-depth) so no notebook over-claims fidelity. Staged writes carry full Common
Identity lineage and go through the `ConditionalCatalogWriter` create-only discipline (§4.3), but are
NEVER promoted into a canonical NT root until proofs are accepted. Promotion is a deferred, explicit
PromotionPackage commit into a fresh immutable root (§4.4).

This keeps E-002 and the gate intact: the NT `ParquetDataCatalog` replay target exists only as a
committed root of accepted bytes; raw provider payloads remain audit input; and the
"`canonical_s3` forbidden until every referenced proof is accepted" clause
(`backfill-source-proof-schema.md:97-98`) is now enforced *mechanically* — pending-proof provider bytes
physically cannot occupy an NT-replayable path.


---

## 6. Single-source taxonomies & vocabulary (resolves F3, F7, F8, R-8, R-9, R-10)

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
`(venue, product_family, table_family)` and records it in the universe completeness gap (§8),
per the granularity rule `contract:35-39`.

**Reconcile the binding TOML.** Rename the field on the two fetch-only instrument-universe bindings
(`backfill-source-bindings.v1.toml:24,37`) from `product_family` to `acquisition_group = "usd_m"` /
`"coin_m"`, and add `normalized_product_families = ["usd_m_perpetual","usd_m_delivery"]` /
`["coin_m_perpetual","coin_m_delivery"]` documenting that the single fetch fans out to two canonical
families at normalize. This removes the `*_or_delivery` spelling entirely. (Owner alternative: split
each endpoint into two bindings; both eliminate the spelling — see Open decision §13.3.) This edit is a
repo-edit-pending — see §16 Group G-B; the plan describes it but does not apply it.

**Acceptance test (ships with Phase 5 Binance).** A dated DELIVERY symbol lands in
`coin_m_delivery`/`usd_m_delivery`; a `*_PERP`/`PERPETUAL` symbol lands in
`coin_m_perpetual`/`usd_m_perpetual`; assert NO normalized row, `canonical_instrument_key`, or
partition is ever emitted with `futures_um`, `futures_cm`, `usd_m_perpetual_or_delivery`, or
`coin_m_perpetual_or_delivery`; assert `product_category` agrees with the derived suffix on every row.

### 6.2 F7 / R-8 — One sanctioned `write_mode` enum, migrated ATOMICALLY across producers + ledger + test

**Authority.** The enum is defined in exactly one place: `backfill-source-proof.v1` (`backfill-source-proof-schema.md:87`):

```
write_mode ∈ { dry_run, local_staging, canonical_s3 }
```

**Decision: `s3_staging` / `s3_staging_only` are ALIASES of `local_staging`, NOT a fourth value.** The
contract recognizes exactly two staging-vs-canonical states: non-canonical staging (`local_staging`) vs
accepted canonical (`canonical_s3`). Where the staged bytes physically live (local disk vs a
non-canonical S3 staging prefix) is a storage-location detail, NOT a commit-state. The "is it on S3?"
fact is recorded in a separate, additive manifest field (NOT a new `write_mode`):
`staging_location ∈ { local, s3_noncanonical }`. `canonical_s3` remains the only value that asserts a
canonical, gate-passed write. Adding a fourth `write_mode` value would re-introduce the dual-path the
gate prevents (bolt rule 2 NO DUAL PATHS).

**R-8 blocker — the migration is NOT a producer-only rename; it is a coupled change set that MUST land
atomically.** The live coverage ledger does not parse `write_mode` positively; it uses an *exclusion*
heuristic. `backfill_coverage_ledger.py:196` reads `write_mode = str(doc.get("write_mode", ""))`
straight from the manifest, and `:289-293` computes:

```python
accepted_binding = (
    len(payload_keys) > 0
    and selector_violations == 0
    and write_mode not in ("local_staging", "dry_run")   # :292
)
```

So a manifest is counted as S3-staging **only because** its mode is NOT `local_staging`/`dry_run` (the
docstring at `:24` states this intent; the comment at `:285-288` explicitly hardcodes the deribit
"(unset)" special-case as another accepted branch). The naive producer migration (`s3_staging` →
`write_mode="local_staging"` + `staging_location="s3_noncanonical"`) would flip every migrated
manifest's `write_mode` *into* the exclusion set, making `accepted_binding` evaluate **False** —
silently **un-counting** every S3-staged binding the moment producers migrate (`:340` and `:436` would
drop them). A producer-only PR would corrupt the live coverage ledger. This is the fail-loud violation
R-8 targets.

**Class fix: replace the exclusion heuristic with positive identification, and ship producers + ledger
+ schema-validation test as ONE atomic change set.** After migration the ledger reads the S3-staging
fact from the explicit fields, not "not in (...)":

```python
# REPLACES backfill_coverage_ledger.py:285-293
staging_location = str(doc.get("staging_location", ""))
KNOWN_WRITE_MODES = ("dry_run", "local_staging", "canonical_s3")
if write_mode not in KNOWN_WRITE_MODES:
    raise ValueError(f"unknown/missing write_mode {write_mode!r} in manifest {run_id}")  # fail loud
accepted_binding = (
    len(payload_keys) > 0
    and selector_violations == 0
    and write_mode == "local_staging"
    and staging_location == "s3_noncanonical"
)
```

The `(unset)` / `s3_staging` / `s3_staging_only` branches (and the deribit "(unset)" special-case at
`:285-288`) become dead and are deleted in the same diff. Unknown or missing `write_mode` is now a hard
error, not a silently-accepted binding. `canonical_s3` is never an `accepted_binding` for *staging*
coverage — it is canonical and flows through the §4.4 committed-`SnapshotSet` path, not the staging
ledger.

The full producer/ledger/test edit list and its atomicity constraint are tracked as the indivisible
**Group G-A** in §16.2 (merge order: verify → ledger+test → producers, so the schema-validation test
guards the rename). `staging_location ∈ {local, s3_noncanonical}` is an **additive** manifest field,
NOT a fourth `write_mode` value (the sanctioned enum stays three-valued, `backfill-source-proof-schema.md:87`).

### 6.3 F8 / R-9 — Deterministic provisional `source_proof_id` (ROW-ID segment) + explicit `nt_instrument_id` rule

**Problem.** `contract:63` requires `source_proof_id` per row and `backfill-source-proof-schema.md:18`
requires it to be a "stable id"; v1 stamped the literal `pending` (`v1:70,227`). A literal `pending` is
not stable — two different unproven sources collide on it and accepted proofs can never be back-linked.
Separately, `nt_instrument_id` is `string, nullable` (`contract:57`) with NO population rule for the
NT-replayable subset.

**R-9 correction — the `v0-pending` suffix is a ROW-ID segment, NOT the `source_proof_version` field.
Decouple them.** v2 conflated two distinct things: the *row-id string* used to reference the
not-yet-accepted proof, and the *schema field* `source_proof_version`. The schema requires
`source_proof_version` to be a **positive integer** (`backfill-source-proof-schema.md:18`) and
`SourceProofReport.source_proof_version` is "Immutable version for this proof record"
(`data-model.md:85`). `v0` is **not** a positive integer, so v2's claim that the `v0-pending` segment
"maps to ... `source_proof_version`" (v2 §6.3) is schema-invalid. The two are now decoupled:

- **The provisional id is a row-id (a string handle), nothing more.** It is the value stamped in the
  per-row `source_proof_id` column / manifest reference so unproven rows have a stable, collision-free
  handle:

```
source_proof_id = "sp:" + <contract_version> + ":" + <venue> + "/" + <product_family> + "/" + <table_family> + ":v0-pending"
```

  The `:v0-pending` segment is a literal id-string discriminator meaning "this handle points at a proof
  that has no accepted version yet." It is opaque to the schema's typed fields. It is minted at the
  `(venue, product_family, table_family)` grain — the grain the gate already requires exactly one
  `SourceProofReport` (`contract:60-61,303`). It is deterministic (pure function of the contract triple),
  so re-running a staging script produces the identical id (create-only / idempotency friendly).

- **The provisional id MUST resolve to a real pending `SourceProofReport`.** A dangling row-id is not
  allowed. Minting the provisional id creates (or, idempotently, references) an actual
  `SourceProofReport` with **`source_proof_version = 1`** (the schema-valid positive integer) and
  **`status = pending`** (`data-model.md:85-86`, `backfill-source-proof-schema.md:18,20`). Its
  `required_checks` are all `pending`/`fail` until proven (`schema:50-73`). The provisional
  `source_proof_id` row-id resolves to *this* record; the staging-write validator (§5.3) rejects any
  provisional `source_proof_id` that does not resolve to an existing pending record (no orphan handles).

- **Acceptance creates a NEW immutable record, not a version bump of the pending one.** Per the schema
  immutability rule (`backfill-source-proof-schema.md:46-48`: "Accepted records are immutable. A new ...
  finding creates a new `source_proof_version` or a new `source_proof_id`"), acceptance writes a new
  `SourceProofReport` with `status=accepted` and `source_proof_version` incremented (the first accepted
  version), `supersedes_source_proof_id` pointing at the pending record (`data-model.md:87`). The row's
  `source_proof_id` handle is rewritten from `…:v0-pending` to the accepted id (e.g. `…:v1`); lineage is
  preserved because both share the `<venue>/<product_family>/<table_family>` stem. The pending record is
  never mutated in place — it is superseded.

- Because `product_family` now comes from the F3-canonical four-family set (§6.1), a Binance futures
  provisional id is e.g. `sp:backfill-table-contract.v1:binance/usd_m_perpetual/funding_rates:v0-pending`
  — never `futures_um`.

The literal `pending` token is removed from the plan (was `v1:70,227`) and replaced with this scheme.
**Net rule: `source_proof_version` is always a positive integer (pending = 1, first accepted ≥ 1 per
immutability); the only place "pending" appears is the `status` field and the opaque `:v0-pending`
row-id discriminator.**

**`nt_instrument_id` population rule (NT-replayable subset).** Bind population to the schema's existing
`nt_mapping_status` field (`backfill-source-proof-schema.md:38`):

1. `nt_instrument_id` is populated (non-null) ONLY when the governing source proof has
   `nt_mapping_status = accepted` AND the row's `table_family` maps to a Tier A/B NT prefix (§2.2:
   `trades`, `quotes`, `order_book_deltas`, `order_book_depths`, `bars`, `index_prices`, `mark_prices`,
   `funding_rate_update`, `instrument_status`, `instrument_closes`, `instruments`). For all Tier C
   research-only tables `nt_instrument_id` stays NULL (the contract types it nullable).
2. When `nt_mapping_status ∈ {pending, rejected, not_applicable}`, `nt_instrument_id` is NULL. A row in
   an NT-replayable table family with `nt_mapping_status != accepted` MUST NOT be handed to
   `BacktestNode` — it stays research-only Parquet until the mapping is accepted.
3. The NT instrument id itself is the NT-native id produced by the Common Identity builder from
   `(venue, product_family, instrument_id)` (`contract:54-55`); this design does NOT invent an NT id
   format — it defers the exact string to the Common Identity normalization library (Phase 1) and only
   fixes *when* the column is populated.
4. **Phase 3 / Phase 6 precondition.** Any BacktestNode read-back fixture in an NT-replayable family
   MUST have `nt_mapping_status=accepted` and a non-null `nt_instrument_id`; the smoke test asserts this
   as a hard precondition and fails loud otherwise. (Synthetic fixtures use
   `source_proof_id=synthetic-fixture` with a synthetic accepted nt_mapping, §5.2.)

**Tests.** (a) A provisional `source_proof_id` resolves to a `SourceProofReport` with
`source_proof_version == 1` (integer) and `status == "pending"`; minting twice for the same triple is
idempotent (same id, same single pending record). (b) Schema validation REJECTS any `SourceProofReport`
whose `source_proof_version` is non-integer or `< 1` (proves `v0` can never reach the field). (c) On
acceptance, a NEW record with `status=accepted`, `supersedes_source_proof_id`=pending-id is created
(pending record unchanged), and the row handle is rewritten off `:v0-pending`. (d) A row whose proof is
`nt_mapping_status=pending` (or whose table_family is Tier C) has `nt_instrument_id IS NULL`; a row
whose proof is `accepted` and table_family is Tier A/B has a non-null id.

### 6.4 R-10 — Orphan recovery is a DISTINCT `recovered_orphan` state, never counted accepted until reviewed

**Problem.** `backfill_accept_staged_objects.py` has two acceptance paths that v2 treated as equivalent.
They are NOT. The manifest-sourced path (`main`, `:275-330`) joins every reconstructed record to a *real
prior local-staging manifest* (`pick_records`, `:79-83`), refuses on any `missing` hash or
`byte_mismatch` (`:303-308` — full join over all records), and only then samples hashes. The orphan path
`--from-s3-keys` (`accept_from_s3_keys`, `:151-248`) has materially weaker guarantees and currently mints
the SAME `write_mode="s3_staging"` (`:209`) and the SAME acceptance manifest schema
(`backfill-archive-s3-acceptance-manifest.v1`, `:202`) as the strong path. The coverage ledger then
counts both identically. Three concrete weaknesses make the orphan path untrustworthy as "accepted":

1. **`source_proof_id` is OPTIONAL.** `--source-proof-id` has no `required=True` (`:262`); the orphan
   manifest stamps `"source_proof_id": args.source_proof_id` (`:208`) which may be `None`. So
   unmanifested orphan bytes can be "accepted" with no governing proof at all — directly undercutting
   F15's "orphans can't be trusted" premise (§4.2) and the gate's per-`(venue,product_family,table_family)`
   accepted-proof requirement (`contract:303`).
2. **Hash verification is SAMPLED, not full.** `accept_from_s3_keys` calls
   `verify_sample(..., max(1, args.verify_sample))` (`:186-187`) with `--verify-sample` defaulting to
   `1` (`:265`). `verify_sample` streams only `hashes[:n]` and computes sha256 (`:86-102`). So with
   default flags exactly ONE object is hash-confirmed; every other object is only byte-size-confirmed
   against live S3 (`all_records_byte_confirmed`, `:226`). A corrupted/swapped object whose key still
   parses as `object=<64hex>` is not detected.
3. **Provenance is RECONSTRUCTED, not authoritative.** Records are rebuilt from the object key path KV
   pairs (`parse_key_provenance`, `:105-108`; `provenance_method="reconstructed_from_s3_key_path"`,
   `:184`) and `source_url` is back-filled from an endpoint map learned from *other* manifests
   (`build_endpoint_map`, `:111-148`, `:165-169`). Any key field absent from the path yields a `None`
   field — incomplete provenance the strong path would never produce.

**Class fix: a distinct `recovered_orphan` state with stricter (not equal) preconditions, never counted
accepted until reviewed.** Orphan recovery is not "acceptance via a different input"; it is a forensic
recovery that produces a *quarantined* artifact requiring an explicit review step before it can ever
participate in coverage or promotion. The `--from-s3-keys` path is restructured so it can NEVER emit a
record indistinguishable from a strong acceptance:

- **(a) Distinct manifest schema + `commit_state`.** The orphan path emits
  `schema_version="backfill-archive-s3-recovered-orphan-manifest.v1"` (NOT the shared
  `…-acceptance-manifest.v1`) and stamps `commit_state="recovered_orphan"` — a state outside the
  `accepted_binding` set. It does NOT stamp `write_mode="s3_staging"`; per §6.2 it carries
  `write_mode="local_staging"`, `staging_location="s3_noncanonical"`, plus the `recovered_orphan` marker.
  The coverage ledger (§6.2) MUST treat a `recovered_orphan` manifest as NOT an `accepted_binding`
  regardless of its `write_mode`/`staging_location`, and MUST surface a separate `recovered_orphan_count`
  so these objects are visible-but-uncounted (fail-loud, not silently merged into accepted coverage).
- **(b) A resolvable, ACCEPTED `source_proof_id` is REQUIRED.** Make `--source-proof-id` required in
  `--from-s3-keys` mode (add an `ap.error(...)` analogous to `:273-274` when `from_s3_keys and not
  source_proof_id`), and the path MUST verify that the supplied id resolves to a `SourceProofReport`
  with `status=accepted` (`data-model.md:84-86`) before writing anything; abort (`return != 0`)
  otherwise. A provisional `:v0-pending` id (§6.3) is NOT acceptable here — orphan recovery binds to an
  already-accepted proof or it does not run.
- **(c) FULL (not sampled) hash verify.** Replace the sampled call (`:186`) for the orphan path with a
  full pass: every object is streamed and its sha256 confirmed equal to the `object=<sha256>` segment in
  its key (reuse `verify_sample`'s per-object logic, `:90-101`, over `len(bound)` objects, no `[:n]`
  slice). Any single mismatch aborts (`:188-191`). Byte-size confirmation alone is insufficient for the
  orphan path because there is no prior manifest to cross-check.
- **(d) Complete provenance required.** Every recovered record MUST have non-null `family`, `source`
  (and the time fields needed for coverage). If `parse_key_provenance` (`:105-108`) yields any missing
  required field, that object is NOT recoverable from keys alone — it is reported in an `unrecoverable`
  list and excluded; recovery does not silently emit partial-provenance records. `source_url`
  reconstructed from `build_endpoint_map` is allowed only as supplementary context, never as a
  substitute for a present, parsed `family`/`source`.
- **(e) Never counted accepted until reviewed.** A `recovered_orphan` artifact is never eligible for the
  §4.4 PromotionPackage (which rejects `commit_state ∈ {orphan, superseded, recovered_orphan}`, §4.4
  step 2) and is never an `accepted_binding` in the ledger. Promotion of recovered bytes requires an
  explicit, separate human-reviewed transition (`recovered_orphan → staged`) recorded in the artifact
  index, gated on the accepted `source_proof_id` from (b) and the full hash pass from (c). Until that
  review, the bytes exist on S3 but are inert for coverage, normalization, and backtest input.

**Tests.** (a) `--from-s3-keys` without `--source-proof-id` ERRORS (exit ≠ 0). (b) `--from-s3-keys` with
a `source_proof_id` whose record is `status=pending`/`rejected` or missing ABORTS. (c) Orphan recovery
hashes EVERY object (assert verify count == object count, not 1) and aborts on a single planted mismatch.
(d) An object whose key omits `family`/`source` lands in `unrecoverable`, not in the emitted records.
(e) A `recovered_orphan` manifest is counted in `recovered_orphan_count` and is NOT in
`accepted_binding_manifest_count` by the coverage ledger, and is rejected by §4.4 PromotionPackage
construction.


---

## 7. Per-venue fidelity corrections (resolves F4, F5, F6, F13)

The shared rule these enforce is the contract's no-downgrade clause: "No worker may silently replace a
missing granular table with a weaker aggregate... derived snapshot deltas do not satisfy native
order-book deltas" (`contract:35-39`). The single source of truth for "what was actually staged and
what family it is" is the **accepted acceptance-manifest / coverage-ledger**, not the binding TOML and
not the key-path strings. The evidence-matrix/binding edits these sections describe are
repo-edits-pending — tracked as **Group G-C** in §16.2; the plan describes them, implementation applies
them.

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
`vendor_or_forward_capture_only` (already correct, `backfill-evidence-matrix.v1.toml:183`).

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
    table. The guard carries the `applies_to_timeseries_tables_only = true` flag and short-circuits to a
    no-op for any `time_series = false` family (the instruments-lane exemption, §2.1 / R-4).
  - **Apply the SAME guard to all snapshot-only / capture_time-only families (class fix, not instance
    patch).** Families with `event_time_source=none` that MUST be rejected from time-series tables:
    Deribit `index`, `mark_price_history_probe`, `trades_recent_probe`; Hyperliquid
    `meta`/`metaAndAssetCtxs`/`spotMeta`/`spotMetaAndAssetCtxs` and the HIP-3 current-snapshot contexts
    (confirm the exact HIP-3 set against `backfill_hyperliquid_hip3_to_s3.py` — Open decision §13.5).
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
  - **`node_fills_by_block` is a separate gated future task** (Open decision §13.5): it requires (a)
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
- Open decision §13.2 ("Coverage completeness bar") is updated to state the universe-completeness bar
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

NT's `write_to_parquet` issues, per write call: (1) one `head()` existence probe (`catalog.rs:565`);
then (2) unless `skip_disjoint_check=true`, a `get_directory_intervals()` doing a full
`object_store.list()` over the target prefix (`catalog.rs:575` → `catalog.rs:2760-2792`, list at
`:2770`); then (3) one `put` (`catalog.rs:595` → `parquet.rs:197`, a plain PUT, no
`PutMode::Create`). Amplification: each output write = 1 HEAD + 1 LIST + 1 PUT. As a directory
accumulates files, the per-write LIST grows with the file count; for a partition holding D daily files,
cumulative LIST cost over a sequential one-year backfill is O(D²) enumerations worst-case. At Deribit's
16,605 tiny objects and OKX's `order_book_400` fan-out (thousands of partitions) this dominates request
cost independent of bytes.

**Note:** the `ConditionalCatalogWriter` (§4.3) replaces NT's `put` with `PutMode::Create` and never
calls NT's `write_to_parquet`, so NT's HEAD+LIST amplification does not apply to the projector's own
writes; the irreducible per-object `PutMode::Create` HEAD/PUT is budgeted explicitly.

### 9.3 Requester-pays egress for Hyperliquid archives (hard NT constraint)

The HL archive is requester-pays (`contract:215,274`; coverage doc line 99 records the lag).
`object_store` supports it via `with_request_payer(true)` (`object_store-0.13.2/src/aws/builder.rs:191,
442-444,1063-1064`). **However, NT's `create_s3_store` does NOT pass it through** — its
`storage_options` match handles only `endpoint_url`/`region`/`access_key_id`/`secret_access_key`/
`session_token`/`allow_http`; any `request_payer` key falls into the `_ =>` "Unknown S3 storage option"
arm and is silently dropped (`crates/persistence/src/parquet.rs:743-765`). **Consequence:** NT's
catalog reader/writer CANNOT issue `x-amz-request-payer: requester`, so HL requester-pays archives
cannot be read directly through the NT catalog. The projection must **pre-stage HL raw archives into our
own (requester-owned) bucket** via a separate copy step that DOES set requester-pays (the existing
backfill scripts / a direct `object_store` client with `with_request_payer(true)`), then project from
the owned copy. Budget the HL requester-pays GET egress as a one-time pre-stage cost (HL-core ~12.23 GiB
accepted; `node_fills`, if later enabled, adds ~19 MB/object — deferred). This pre-stage requirement is
a gating line item, not an optimization. (Note: this is the only sanctioned cross-bucket byte movement
in the pipeline that incurs egress; canonical promotion within the staging/canonical bucket uses
server-side copy with zero egress, §9.x / §4.4a.)

### 9.4 Partitioning / parallelism strategy

- **Partition** by `(venue, product_family, table_family, instrument)` matching NT's `make_path` layout
  (`data/{type_name}/{safe_instrument_id}`, `catalog.rs:2841-2849`) so directory LISTs stay scoped to
  one instrument's interval set, and so per-object parquet stays well under the S3 single-PUT limit
  (§4.3.3 — the atomic `put_opts(PutMode::Create)` path requires it).
- **Parallelize** across instrument partitions (disjoint directories → no LIST contention, and the
  `ConditionalCatalogWriter` makes concurrent writers safe). Do NOT parallelize multiple writers into
  the SAME interval directory under NT's native writer (the head()-then-put race, §4.1) — but NT's
  native writer is never called for platform writes anyway (§4.1; see the §4.1 scope-boundary note
  for the pre-existing vertical-slice run-scoped exception on `main`).
- **Bulk path** uses one-pass-per-directory + the external conditional-PUT layer.

### 9.x Cost-model note — object counts assume logical-digest idempotency + server-side copy (resolves R-3, R-7)

The per-venue accepted object counts in §9.1 (and the HEAD/LIST/PUT amplification in §9.2) are only
valid because re-running a transform over unchanged inputs produces the **identical** staging key and
therefore zero new objects (the `PutMode::Create` collision is treated as an idempotent no-op, §4.3.4).
This holds ONLY with the §4.3b logical-content digest. **If the key were hashed over the parquet bytes**
(the rejected v2 design), every re-run after any transitive parquet-rs version bump (the registry
already carries parquet-rs 57.3.0 / 58.1.0 / 58.3.0, and the footer embeds `DEFAULT_CREATED_BY` = the
crate version per `parquet-58.3.0/src/file/properties.rs:51`) would mint a **new** object for the same
logical content, multiplying object counts and PUT/HEAD request cost by the number of re-runs and
inflating the 267 GiB Polymarket / 16,605-object Deribit footprint without limit. The cost projection
assumes logical-digest keying; any switch back to byte-hash keying invalidates these numbers.

Canonical promotion (§4.4a) materializes via **backend-native server-side copy**
(`object_store::copy_opts(CopyMode::Create)` → S3 `CopyObject`, `aws/mod.rs:312`,
`aws/client.rs:596-597,702`), so staging→canonical for the full ~390 GiB ledger incurs **zero GET
egress and zero PUT-of-bytes** — only one `CopyObject` request per promoted object (multipart
`UploadPartCopy` for the rare >5 GiB object, §4.4a). Promotion request cost is therefore ~one CopyObject
per accepted object (~29,434 requests at full scale), not a second ~390 GiB byte movement. This is what
makes per-commit immutable canonical roots (a fresh full materialization per commit) affordable: each
promotion is request-priced, not byte-priced.

### 9.5 Gate

The full one-year projection does NOT run until this costed estimate (object counts, bytes,
HEAD/LIST/PUT request counts per venue, per-promotion CopyObject request counts, HL requester-pays
pre-stage egress, and wall-clock under the chosen parallelism) is produced from the ledger numbers above
and approved. Tie this gate to Open decision §13.2 (coverage completeness bar) so cost and completeness
are signed off together.


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
| **S1** | NT-catalog-on-S3 capability proof on a research-only crate (`cloud` feature, rev `6be5a50`); negative control + invalid-creds control + positive write/re-open/query — **over SYNTHETIC fixtures only** (one synthetic `binary option`, one synthetic `perps/spot`), into a DISTINCT synthetic-only root (R-14, §10.2). | v1 STEP 1 (now synthetic-bound) | Source-independent; must not read provider bytes. |
| **S2** | Approve `artifact_root` URI + typed prefix schema + URI-validation tests. | v1 STEP 2 | Unchanged. |
| **S3** | Shared Common Identity normalization library (nanos multiplier table, decimal-string preservation, `canonical_instrument_key`, `transform_hash` over code+config, `raw_payload_id`, deterministic provisional `source_proof_id` plumbing, the §4.3b logical-content digest) + timestamp-unit unit tests + the `event_time_source` fail-loud guard (§7.3). | v1 STEP 3 | Unchanged (source-independent). |
| **S4** | The `ConditionalCatalogWriter` create-only/no-overwrite write layer (§4.3) incl. encoder-boundary decision (0.E), conditional-put + copy-if-not-exists probe (0.6); concurrency proof; `no_overwrite_proof`. | v1 STEP 4 (promoted to BLOCKER) | Source-independent; blocking. |
| **S5** | Best-effort instrument-universe manifest + completeness gap record (§8), per venue/product_family. | v1 STEP 5 | Feeds `instrument_universe` proof check. |
| **S6** | **Early synthetic consumption smoke** — NT `BacktestNode`/`BacktestEngine` replays the **synthetic** S1 fixtures from the synthetic-only committed set root; assert a `backtests/` result stamped `provenance=synthetic`, `result_kind=capability-smoke`; includes the §4.5 run-pinned-set + competing-promotion proof. Read-only Jupyter notebook consumes the same synthetic catalog. **No provider data.** | v1 STEP 8 + 9 (moved earlier; synthetic-bound) | Proves the consumption pipeline before any provider proof, without producing provider-source evidence. |
| **S7 (per source family, gating)** | Build the `SourceProofReport` (portable raw sample + SHA-256, schema sample + row counts + timestamp range, license/retention refs, `fidelity_class`, `forbidden_claims`, `gap_policy_id`) and run all `required_checks` (`backfill-source-proof-schema.md:50-73`); set `status=accepted` only when every check (incl. `nt_mapping`) passes; ambiguous/failed stays `pending`/`rejected`. HIP-4 quoteToken parser-fidelity proof is part of the HIP-4 family's `schema`/`nt_mapping` checks and must pass before HIP-4 acceptance. | v1 STEP 7 (HIP-4) + v1 STEP 10 proof half (split per-family, moved BEFORE projection) | Enforces `spec.md:52-54`: accepted proof precedes catalog/backtest use. |
| **S8 (per accepted family)** | Raw→NT-class + contract-table projection to STAGING (Tier-C-only, physically non-NT, §5.3) via `ConditionalCatalogWriter` — **only for families whose S7 proof is `accepted`**. Each staged row carries the accepted `source_proof_id`, `fidelity_class`, `forbidden_claims`. | v1 STEP 6 (now gated by S7) | Projection consumes the accepted-proof allowlist. |
| **S9 (per accepted family)** | Provider-derived consumption — BacktestNode replay over a committed per-set root (Tier A only, run-pinned §4.5) and read-only notebook consumption; emit `BacktestResultContract` only when all `source_proof_ids` resolve to `accepted` (`backfill-source-proof-schema.md:90`). | (new gate over v1 STEP 8/9 for provider data) | Provider-source result requires accepted-proof sources. |
| **S10** | Aggregate evidence matrix (one row per `(venue, product_family, table_family)`, incl. `nt_target`) + gap policy with `forbidden_claims`; on full acceptance, the deferred canonical promotion via PromotionPackage + single-`SnapshotSet` CAS into a fresh immutable root (§4.4). | v1 STEP 10 aggregate/promotion half | `canonical_s3` forbidden until every referenced proof is accepted. |

### 10.2 Phases

- **Phase 0 — Catalog capability proof + write layer (GATING), synthetic-only** (= S1, S4). Capability-
  proof fixtures are SYNTHETIC, in-repo, deterministically generated NT-class rows; this phase MUST NOT
  read any provider-derived payload. Any emitted result is stamped `provenance=synthetic`. Includes the
  Cargo separate-workspace isolation (§11-iso/§12) and the `ConditionalCatalogWriter` concurrency proof.
  Sub-tasks:
  - **0.0 Structural isolation (F12, §12).** Cloud-enabled projector in a SEPARATE workspace/lockfile.
  - **0.1 Falsifiable `cargo tree -e features` build guard (F12, §12).**
  - **0.2 Negative control 1 — feature gate (F12).** No-cloud build → `from_uri` on `s3://` hits the
    `crates/persistence/src/parquet.rs:549` bail.
  - **0.3 Negative control 2 — credential attribution (F11, §11-cred).** Cloud ON + scrubbed ambient
    creds (env + IMDS) + no/invalid `storage_options` creds → write FAILS; same write with valid SSM
    creds → SUCCEEDS.
  - **0.4 Positive proof.** SSM-resolved creds → write two SYNTHETIC fixtures → re-open → `query_files`
    → assert; stamp `NtCapabilityProof` (exact `storage_options` key set consumed, credential source =
    SSM).
  - **0.E Encoder-boundary verification (BLOCKING, R-5).** Verify at `6be5a50` that
    `write_batches_to_object_store` is `pub` (`parquet.rs:170`) and that the encode and `put` share one
    function (`parquet.rs:178-197`); choose and lock the encoder strategy (vendor the ~25-LOC encode body
    OR arrow-rs byte-compatible encode) per §4.3.0; record the encoder identity in `NtCapabilityProof`.
  - **0.6 Conditional-put + copy-if-not-exists PREREQUISITE probe (BLOCKING, R-6 + R-7).** Run the
    writer-construction runtime probe (§4.3.2) against the target store; abort if conditional-put returns
    `NotImplemented`. ADDITIONALLY probe `copy_if_not_exists` (§4.4a sentinel server-side copy); abort if
    it returns `NotSupported` (`aws/mod.rs:374-378`). This elevates "bucket supports conditional put AND
    copy-if-not-exists" from a v2 open decision to a **Phase-0 prerequisite** (see §13 amendment). Cover
    the multipart bound (§4.3.3): assert no synthetic fixture exceeds the single-PUT limit.
  - **0.W `ConditionalCatalogWriter` BLOCKER (F2, §4.3).** Create-only, content+transform-hash-keyed
    (staging) / NT-native (canonical), with the §4.3.5 concurrency proof; absence blocks all writes.
  - **0.R Run-pinned-set + competing-promotion BLOCKER (R-2, §4.5).** A backtest run and a competing
    promotion against a `PutMode::Create`-capable store; the promotion advances `set/latest.json` after
    the run's pin but before its trades LIST; assert the run reads only its pinned set (no
    old-instruments + new-trades mix); a second run observes the new set.

  > **Distinct synthetic catalog-root URI (resolves R-14).** Every Phase-0 (and Phase-3) capability/
  > consumption proof writes to a DEDICATED synthetic catalog root that is NEVER commingled with any
  > provider or canonical root. Concretely: the synthetic root is its own top-level URI segment, e.g.
  > `<artifact_root>/nt-catalog-synthetic-proof/<run_uuid>/` (and the offline-store equivalent under a
  > `synthetic-proof/` MinIO/R2 bucket-prefix), distinct from `nt-catalog/`,
  > `normalized/<schema_version>/`, `staged-research/`, and `data/<type>/`; all synthetic objects carry
  > `source_proof_id=synthetic-fixture` and `provenance=synthetic` (§5.2) AND live under this
  > synthetic-only root so a stray `query_files`/`query_instruments` against a provider/canonical root can
  > never enumerate them; the proof harness asserts (fail-loud) that its configured catalog root string
  > matches the synthetic-root pattern and is disjoint from every provider/canonical prefix before it
  > writes a single byte — making synthetic↔provider commingling mechanically impossible.
- **Phase 1 — Artifact root + write-discipline foundations** (= S2, S3). TOML/config-owned
  artifact_root + typed prefix schema (`raw/`, `normalized/<schema_version>/`,
  `nt-catalog/sets/<snapshot_set_id>/`, `nt-catalog-synthetic-proof/`, `staged-research/`,
  `source-proofs/`, `backtests/`, `research-analytics/v1/{datasets,feature-tables,experiment-results,
  promotion-packages}/`, `artifact-index/v1/{events,snapshots,pointers/{kind=<artifact_kind>,set}}/`);
  single root, no per-type knobs. URI-validation tests. Common Identity fill library (per-(product,family)
  event_time→nanos multiplier table, NO single hardcoded multiplier, NEVER REST response time;
  decimal-string preservation; `canonical_instrument_key`; lineage
  `raw_payload_id`/`transform_hash`/deterministic `source_proof_id`; the §4.3b logical-content digest;
  the `event_time_source` allowlist guard §7.3). Write-manifest format with `write_mode ∈ {dry_run,
  local_staging, canonical_s3}` + additive `staging_location` (§6.2) + `no_overwrite_proof`.
- **Phase 2 — Instrument universe (best-effort + gap record)** (= S5, §8).
- **Phase 3 — Synthetic consumption smoke** (= S6). NT `BacktestNode` + notebook over the SYNTHETIC
  committed-set root only (R-14 distinct root). Provider replay is deferred to Phase 6.
- **Phase 4 — Per-family source proofs + acceptance (GATING)** (= S7). For each source family produce
  the `SourceProofReport`, run `required_checks`, accept only on all-pass; HIP-4 quoteToken fidelity is
  a required check inside the HIP-4 family proof. No family is projected until its proof is `accepted`.
  HIP-4 outcomeMeta identity (encoding=10·outcome+side, wire_symbol=#<encoding>,
  asset_id=100000000+encoding; preserve raw quoteToken verbatim) and the quoteToken parser-fidelity
  proof harness live here.
- **Phase 5 — Per-accepted-family projection to staging** (= S8). Project raw→NT-class for accepted
  families only via `ConditionalCatalogWriter` into Tier-C-only physically-non-NT staging (§5.3); stamp
  `source_proof_id`/`fidelity_class`/`forbidden_claims` on every staged row. Per-venue mappers
  (corrected): Binance (§6.1 derivation), OKX (§7.1), Bybit (spot vs derivatives tick_trades schema
  branch; mark/index/premium klines→`bars` per §2.4; funding→funding_rate_update;
  open_interest/historical_volatility→Tier C; product_category via contractType), Deribit (§7.3),
  Hyperliquid-core (§7.4), Hyperliquid HIP-3 (fundingHistory→funding_rate_update; candleSnapshot→bars;
  meta/allPerpMetas→instruments current-snapshot; preserve dex_name + synthetic asset_id as derived join
  helper), Polymarket (§7.2), HIP-4 market-data (gated; candleSnapshot→bars, recentTrades→trades(recent-only),
  l2Book→order_book_snapshots_fixed_depth + quotes(reconstructed_top_of_book); forbidden_claims attached).
- **Phase 6 — Provider-derived consumption + canonical promotion (GATING)** (= S9 + S10). Provider
  BacktestNode replay (Tier A only, run-pinned §4.5) and notebook consumption, emitting
  `BacktestResultContract` only when all manifest `source_proof_ids` are accepted; then the aggregate
  evidence matrix (incl. `nt_target`), gap policy, and the deferred canonical promotion via explicit
  PromotionPackage + single-`SnapshotSet` CAS into a fresh immutable root (§4.4) — NEVER a prefix
  re-point.


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

### 12. Cargo separate-workspace isolation (resolves F12) + R-11 NT-surface drift guard

- The cloud-enabled catalog projection MUST NOT be a feature-flag on the existing `bolt-v2` package.
  Cargo features are **additive and unified per dependency-resolution graph**; within one package (or
  one workspace sharing a lockfile/resolution), `nautilus-persistence/cloud` enabled anywhere unifies
  into the live binary.
- Create the research/backtest projector as its own package **with its own workspace root and its own
  lockfile**, outside `bolt-v2`'s dependency resolution. Concretely: a sibling directory (e.g.
  `tools/catalog-projector/`) carrying its own `[workspace]` + `Cargo.lock`, declared in the live
  `bolt-v2/Cargo.toml` via `[workspace] exclude` so it never joins the live binary's resolution graph;
  OR a fully separate repo/path checkout. **Prior art already in-repo (do not duplicate scaffolding
  unknowingly):** `crates/backtesting-vertical-slice/` on `main` implements exactly this pattern —
  its own `[workspace]` root + `Cargo.lock` with `nautilus-persistence = { rev = "6be5a509…",
  features = ["cloud"] }`, outside the live binary's graph. Phase-0 decides at build time whether to
  extend that crate or scaffold the sibling projector crate. It depends on `nautilus-persistence = { rev = "6be5a509...",
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

**R-11 drift guard (CI, pinned to `6be5a5094716790a8ca2875445fde4fa2586107e`).** The tier matrix (§2), the replay-claim rule (§2.5), the
projector's NT-class routing, and the NT-native canonical filename assumption (B-1) all assume a FIXED
NT surface at `6be5a5094716790a8ca2875445fde4fa2586107e`; an NT rev bump could silently invalidate them. The separate cloud-enabled
projector workspace pins `nautilus-persistence` / `nautilus-backtest` to
`rev = "6be5a5094716790a8ca2875445fde4fa2586107e"` (no floating rev, no version range). A CI test in the
projector workspace asserts, against the pinned source, ALL of:
1. `NautilusDataType` includes the pinned replay surface (`crates/backtest/src/config.rs`:
   `QuoteTick, TradeTick, Bar, OrderBookDelta, OrderBookDepth10, MarkPriceUpdate, IndexPriceUpdate,
   FundingRateUpdate, InstrumentStatus, OptionGreeks, InstrumentClose`). A removal or unreviewed member drift fails
   the guard.
2. The **exact** `CatalogPathPrefix` strings the projector depends on
   (`crates/persistence/src/backend/catalog.rs:4248-4259`) are byte-for-byte unchanged:
   `QuoteTick→"quotes"`, `TradeTick→"trades"`, `OrderBookDelta→"order_book_deltas"`,
   `OrderBookDepth10→"order_book_depths"`, `Bar→"bars"`, `IndexPriceUpdate→"index_prices"`,
   `MarkPriceUpdate→"mark_prices"`, `FundingRateUpdate→"funding_rate_update"`,
   `InstrumentStatus→"instrument_status"`, `OptionGreeks→"option_greeks"`, `InstrumentClose→"instrument_closes"`,
   `InstrumentAny→"instruments"`.
3. `timestamps_to_filename` still produces `"{iso1}_{iso2}.parquet"` (`catalog.rs:4315-4320`) — the
   NT-native canonical filename the reader parses (B-1).

> **Why assert the enum count AND the explicit strings, not a blanket "CatalogPathPrefix count":** the
> full `impl_catalog_path_prefix!` set is broader than the replay surface (`catalog.rs:4248-4286`) — the replayable types
> + `InstrumentAny` (`:4259`) + `AccountState` (`:4260`) + execution-output prefixes
> order/position/report execution-output prefixes (`:4261-4286`) the projector does NOT map. A blanket
> member-count assertion (e.g. "== 9" against `CatalogPathPrefix`) would be **wrong** (the set is 39),
> brittle (it would fire on an unrelated execution-type addition that doesn't affect this projection),
> AND insufficient (a renamed-but-same-count prefix would slip through). The guard keys on the
> load-bearing surface only: the replay enum + the exact projector-relevant prefix strings (the
> replayable prefixes, including `funding_rate_update`, + `instruments`) + the canonical filename format. On ANY drift
> the guard FAILS and blocks the projector until the tier matrix (§2) is re-verified against the new rev.

This guard runs alongside the `cargo tree -e features` guard; both live in the projector workspace's CI,
keyed to the pinned rev.

### 11-cred. Credential negative control (resolves F11, empirically proves rule-6 SSM-only)

- **0.2 Negative control 1 — feature gate (unchanged):** a no-cloud build calling `from_uri` on an
  `s3://` URI must hit the bail `"Cloud storage support requires the 'cloud' feature: {uri}"`
  (`crates/persistence/src/parquet.rs:549`, the `#[cfg(not(feature = "cloud"))]` arm `:540-550`). This
  proves cloud is feature-driven — but NOT that SSM creds (vs ambient creds) drive the positive write.
- **0.3 Negative control 2 — credential attribution:** the no-cloud bail proves nothing about WHERE the
  positive write's credentials came from. With cloud ON, the write could succeed via ambient AWS env
  vars, an AWS profile, or an EC2 instance profile (IMDS) rather than the SSM-injected `storage_options`.
  Add a second control:
  - **Setup:** cloud ON; the writer builds its own `AmazonS3Builder` (§4.3.2) carrying NO
    `access_key_id`/`secret_access_key`/`session_token` (or deliberately invalid ones). Simultaneously
    SCRUB every ambient AWS credential source.
  - **Why scrubbing both env AND IMDS is required (grounded):** the projector's `AmazonS3Builder` (and
    NT's `create_s3_store`, which uses `AmazonS3Builder::new()` not `from_env()` and only sets keys from
    `storage_options`, `parquet.rs:738-769`) does NOT auto-load env-var creds. HOWEVER,
    `AmazonS3Builder::build()` does NOT fail on missing static creds; when both keys are `None` it falls
    through WebIdentity / Task / EKS Pod / `InstanceCredentialProvider` (IMDS at
    `http://169.254.169.254`) (`object_store-0.13.2/src/aws/builder.rs:1090-1179`, default endpoint
    `builder.rs:43`). The builder constructs successfully and only fails at REQUEST time. So the proof is
    the WRITE failing, not the build failing.
  - **Scrub set (exact):** unset all `AWS_*` env vars the builder recognizes — `AWS_ACCESS_KEY_ID`,
    `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_DEFAULT_REGION`/`AWS_REGION`,
    `AWS_ENDPOINT`/`AWS_ENDPOINT_URL_S3`, `AWS_WEB_IDENTITY_TOKEN_FILE`, `AWS_ROLE_ARN`,
    `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `AWS_CONTAINER_CREDENTIALS_FULL_URI`, `AWS_PROFILE`
    (enumerated in `builder.rs:557-577`), AND point any AWS config/credentials profile path away from a
    real file. Block IMDS so `InstanceCredentialProvider` cannot silently succeed: run the control
    off-instance, OR set an unroutable `metadata_endpoint`/IMDS timeout (Open decision §13.4).
  - **Assertion:** with creds scrubbed and the builder carrying no/invalid creds, the write (or its
    first object-store `put`/`head`) MUST FAIL with an authentication error. Re-run the SAME write with
    VALID SSM-injected creds and assert SUCCESS. The delta — failure when SSM creds absent, success when
    present, with ambient sources held constant-scrubbed in both — attributes the positive write to SSM
    creds specifically and empirically demonstrates rule-6 for the catalog path.
- **0.4 Positive proof (unchanged):** SSM-resolved creds → write two SYNTHETIC fixtures → re-open →
  `query_files` → assert. Stamp `NtCapabilityProof` recording PROVEN direct-S3, the EXACT
  `storage_options`/builder key set consumed (`endpoint_url`, `region`, `access_key_id`/`key`,
  `secret_access_key`/`secret`, `session_token`/`token`, `allow_http` — `parquet.rs:743-762`; any other
  key is silently dropped via the `_ =>` warn arm `parquet.rs:763-765`), and credential source = SSM. On
  failure, document the local-write-then-s3-sync fallback and block direct-S3 claims.

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
   request counts per venue, per-promotion CopyObject request counts, HL requester-pays pre-stage
   egress, wall-clock under chosen parallelism) and the HL requester-pays pre-stage line item before the
   one-year run (§9.5). Tied to decision 2.

### 13.R16 — RESOLVED: funding (`funding_rate_update`) is NT-native at the repo-pinned rev

`FundingRateUpdate → funding_rate_update` is Tier A (§2.2) at
`6be5a5094716790a8ca2875445fde4fa2586107e`: it is catalog-writable, present in
`NautilusDataType`, and has a `dispatch_query` arm. Funding no longer needs the custom-data or
actor-injection path that earlier revisions named as a candidate.

**Decision:** funding uses the native NT `FundingRateUpdate` catalog stream. The custom-data path
remains relevant for S6/S7 families that NT still does not model natively, such as open interest,
implied volatility, historical volatility, forward/delivery prices, and settlements. `OptionGreeks`
is also NT-native at the pinned rev, but its source-specific S7 mapping/projection remains separate
from this funding slice. A bolt-side engine replay claim for funding still
requires a focused `FundingRateUpdate -> on_funding_rate` proof; catalog projection/readback alone is
not that proof.

### 13.6 — RESOLVED: "Pointer-commit ordering across kinds"

**DECIDED (not an open question): single set-atomic commit + run-pinned set read.** Per-kind eventual
consistency is rejected — it admits the silent mixed read proven in §4.5 (NT re-LISTs each kind
independently at different instants within one run: `node.rs:160-182`, `378-386`, `492-505`). A
PromotionPackage spanning multiple `artifact_kind`s commits as ONE immutable `SnapshotSet` advanced by a
single CAS on `artifact-index/v1/pointers/set/latest.json` (§4.4 step 6), and readers pin that committed
set for the entire run (§4.5 reader rule). The `SnapshotSet` is the join record across kinds; the
per-kind `latest.json` pointers are derived from it, never independently advanced. This is a design
decision, not an owner gate — it is fully specified and verified feasible against `object_store` 0.13.2
conditional puts. (What remains owner-gated is unchanged: the canonical write authorization itself,
contract §292-309.)

### 13.7 — RESOLVED: "Conditional-put + copy-if-not-exists enabled on the real bucket"

**UPGRADED to a Phase-0 PREREQUISITE (R-6 + R-7).** The `ConditionalCatalogWriter` proves BOTH
capabilities at construction via the runtime probe (§4.3.2 conditional-put + §4.4a copy-if-not-exists,
Phase-0 sub-task 0.6); a bucket lacking either aborts the run. No longer an open question — a hard gate
the writer enforces empirically, because the resolved `S3ConditionalPut` mode is not introspectable from
the built store (`aws/precondition.rs:117-160`, `aws/client.rs:209`).

### Residual sub-questions carried from the corrections (not yet owner-blocking)

- **settlements → InstrumentClose:** whether Deribit/Bybit settlement event records should map to Tier A
  `InstrumentClose` (`instrument_closes`) instead of Tier C; left Tier C pending a source-proof that
  settlement semantics match NT InstrumentClose.
- **Optional top-10 → `order_book_depths`:** whether the fixed-depth→top-10 projection (Binance
  bookDepth, HL l2Book, OKX/Polymarket fixed-depth) is in-scope this tranche or deferred — affects
  whether `order_book_depths` gets populated at all given no native L2 delta source exists for most
  venues.
- **CI conditional-create conformance store:** the `ConditionalCatalogWriter` concurrency BLOCKER needs
  a `PutMode::Create`-capable store offline (MinIO/R2, ETagMatch-capable per `aws/precondition.rs:121-122`);
  LocalFileSystem's Create semantics differ and would not exercise the S3 If-None-Match path. The same
  store must also support `copy_if_not_exists` for the §4.4a promotion proof.
- **Binance dated-delivery contractType value set:** the live-exchangeInfo `contractType` strings for
  dated futures are assumed from API convention, not verified against a captured raw payload; Phase 5
  Binance must verify against a staged exchangeInfo sample before locking the delivery-suffix mapping.
- **PMXT event_type schema probe:** confirm `last_trade_price`/`book`/`price_change`/`tick_size_change`
  against a real PMXT Parquet schema sample before declaring trades/quotes/instrument_status
  backfillable.
- **OKX `order_book_400` internal frame structure + option book existence:** lock the
  `okx_400level_snapshot_clear_add_then_update_delete` derivation rule against a real Parquet/CSV schema
  sample; confirm OKX historical-download actually serves a 400-level option book before keeping any
  option book family.
- **Synthetic-fixture generator home:** does the Phase-0/Phase-3 synthetic NT-class fixture generator
  live in the research-only crate alongside the capability proof, or a shared test-fixtures module?
- **Acceptance authority/automation boundary:** E-040 allows automated acceptance "when all robust
  checks pass" but the plan does not yet specify who/what flips `status=accepted` for the per-family S7
  gate, nor where the acceptance record is stored under `source-proofs/`. This same authority owns the
  `recovered_orphan → staged` review transition (§6.4 e).
- **Mixed-fidelity run manifests:** when one BacktestNode run reads multiple families, S9 requires ALL
  their `source_proof_ids` accepted — confirmed: the §4.5 run-start pin asserts every required kind is in
  the pinned set with accepted proofs and fails loud otherwise.
- **`staging_location` field name** — `staging_location ∈ {local, s3_noncanonical}` is a naming choice;
  the load-bearing decision is that it is NOT a fourth `write_mode` value.
- **Symbol-shape grammar per venue beyond Binance USD-M** (COIN-M dated, Bybit linear/inverse expired,
  OKX FUTURES expiry-coded) must be authored and source-proof-cited; only the Binance USD-M
  PERP/DELIVERY rule is currently evidenced in code.
- **`snapshot_set_id` allocation scheme:** the exact id scheme for `nt-catalog/sets/<snapshot_set_id>/`
  (monotonic counter vs content-derived vs uuid) is a naming choice; the load-bearing decision (one
  immutable root per committed set, named by the single `set/latest.json` pointer) is fixed in §4.4/§4.5.


---

## 14. Risks

- Phase 0 catalog proof + `ConditionalCatalogWriter` concurrency proof + the §4.5 run-pinned-set /
  competing-promotion proof NOT yet executed (read-only analysis only); do not mark E-037
  SOURCE_PROVEN-positive until the write+query, the conditional-put + copy-if-not-exists probes, the
  no-overwrite race, and the mid-promotion read race all run end to end.
- Per-product timestamp-unit hazard is the highest silent-corruption risk (spot=µs, futures=ms,
  metrics=string-datetime, Bybit derivatives=seconds.fraction); never fall back to REST response time.
- Scientific-notation/decimal fields must be parsed as Decimal from the exact raw string.
- Several NT classes are unsatisfiable from this tranche → record as forbidden_claims, not faked (no
  native order books/quotes for Binance/Bybit/HIP-3/Deribit historical; no per-strike Greeks/IV; Deribit
  index_prices has no source event_time from `get_index_price`; Polymarket full-depth pending;
  HIP-4/Polymarket trades are recent/bounded; HL-core has no native trade tape).
- **Read-path corruption (B-1):** a non-NT-native canonical filename is silently OVER-INCLUDED by NT's
  reader (`query_intersects_filename` returns `true` on `None`, `catalog.rs:4741`), and NT lists a root's
  `data/<type>/` prefix naively with zero pointer awareness (`catalog.rs:2040-2063`). Canonical roots
  MUST use NT-native interval filenames only; staging MUST be physically non-NT; the pointer MUST name an
  immutable per-set root NT is pointed at, never a shared mutated prefix. A pointer alone does NOT keep
  pre-CAS bytes out of a read.
- **Cross-kind / mid-promotion read race (R-2):** NT re-LISTs each `data_config` independently at
  different instants in one run (`node.rs:160-182`/`378-386`/`492-505`); a per-kind pointer admits a
  silent mixed read. Mitigated by the single-`SnapshotSet` CAS + run-start pin (§4.4/§4.5).
- **Idempotency-key non-determinism (R-3):** parquet bytes are not deterministic (`created_by` =
  parquet-rs version, SNAPPY, row-group sizing, `parquet.rs:182-183`); the staging/canonical key and
  `content_hash` MUST be the §4.3b LOGICAL digest, never the parquet bytes, or re-runs mint duplicate
  objects.
- Identity traps (OKX/Bybit perpetual-vs-dated contractType join; OKX instrument_id from payload not
  partition; Polymarket family mislabeled in key path; Binance four-family taxonomy must not leak
  `futures_um`/`*_or_delivery`).
- Interim staging writes must be strictly labeled (`write_mode=local_staging`,
  `staging_location=s3_noncanonical`, deterministic provisional `source_proof_id`, `commit_state=staged`,
  forbidden_claims) AND physically non-NT (Tier-C-only, never `data/<type>/`, §5.3) or be mistaken for
  canonical / be NT-enumerable.
- `transform_hash` must hash CODE + CONFIG, not config alone.
- NT's writer is non-atomic (head-then-put TOCTOU, interval-keyed, default Overwrite) — Rust
  `write_to_parquet`/`write_custom_data_batch`/`write_instruments` AND Python
  `write_data`/`consolidate_*` (`parquet.py:251,284-285,597,652`). ALL platform writes go through the
  Rust `ConditionalCatalogWriter`; NT's writer (Rust or Python) is never called for platform-root
  writes (§4.1 scope-boundary note). Python is read-only (§3).
- **`write_mode` migration is a coupled change set (R-8):** migrating producers to `local_staging`
  without the ledger-logic edit silently un-counts every S3 binding at
  `backfill_coverage_ledger.py:292`. Producers + ledger + schema-validation test ship atomically (§16
  Group G-A).
- **Orphan recovery (R-10):** the `--from-s3-keys` path must carry a distinct `recovered_orphan` state,
  a resolved accepted `source_proof_id`, a FULL hash verify, and complete provenance; never an
  `accepted_binding`, never promotable, until human-reviewed.
- Canonical promotion must be an explicit accepted-object PromotionPackage + single-`SnapshotSet` CAS
  into a fresh immutable root, never a prefix re-point (would canonicalize orphan/superseded bytes) and
  never an in-place mutation of a shared root.
- Instrument universe is best-effort, NOT window-complete; current-snapshot listing sources omit
  instruments delisted before the snapshot with no staged-data footprint; recorded as
  `bounded_or_current_only` + forbidden_claim, never silently presented as complete. Expired-contract
  `product_category` is resolved only by a declared, source-proof-cited symbol-shape parser; unresolved
  instruments are forbidden_claims, not dropped.
- Cargo feature unification: cloud/aws must NOT reach the live binary; enforced by the separate workspace
  + `cargo tree -e features` guard. NT-surface drift enforced by the R-11 guard (§12).
- Cost/scale: Deribit object-count, OKX `order_book_400` fan-out, and NT per-write HEAD+LIST dominate
  request cost; HL requester-pays archives need a pre-stage copy because NT drops the `request_payer`
  storage option; promotion uses server-side copy (zero egress, request-priced, §4.4a/§9.x).
- Source bindings today are largely instrument_universe/instruments; market-data bindings needed for
  backtests are largely not yet declared.
- Python research path is READ-ONLY; it must not introduce a second credential surface or a second write
  path (R-12) — read creds also resolve from SSM via `from_uri` `storage_options` (§3.3).

---

## 15. Findings resolution summary (F1–F15 + B-1, R-2…R-16)

### 15.1 v1→v2 findings (F1–F15) — retained from v2, mechanics in the referenced sections

- **F1** NT data-class mis-assignment → §2 three-tier matrix + `nt_target` evidence column; replay claim
  scoped to Tier A (§2.5).
- **F2** Non-atomic NT writer → §4.1, §4.3 `ConditionalCatalogWriter` (`PutMode::Create`,
  content+transform-hash key, concurrency proof) as a Phase-0 BLOCKER.
- **F3** Binance product-family taxonomy → §6.1 four-family single source, derived from contractType at
  normalize; binding TOML reconciled (§16 G-B); fail-loud guard + acceptance test.
- **F4** OKX `order_book_400` → §7.1 `order_book_snapshots_fixed_depth` + `order_book_snapshot_deltas`
  (named derivation rule) + forbidden_claim; matrix downgrade (§16 G-C).
- **F5** Polymarket family/host → §7.2 single authoritative `order_book_snapshots_fixed_depth` from
  accepted manifest; host `archive.pmxt.dev/Polymarket/v2`; binding + matrix reconciled (§16 G-C).
- **F6** Deribit index event_time → §7.3 forbidden_claim + `event_time_source` fail-loud guard applied
  to all snapshot-only families.
- **F7** `write_mode` fragmentation → §6.2 one three-valued enum; `s3_staging` aliased to `local_staging`
  + additive `staging_location`; migration is an atomic change set (R-8, §16 G-A).
- **F8** Provisional `source_proof_id` / `nt_instrument_id` → §6.3 deterministic provisional id scheme
  (decoupled from `source_proof_version`, R-9) + `nt_mapping_status`-bound population rule.
- **F9** Instrument-universe completeness → §8 best-effort + completeness gap record + declared
  symbol-shape parser + owner-declared blocking completeness bar.
- **F10** Cost/scale → §9 ledger-grounded volumes, NT request amplification, HL requester-pays pre-stage,
  server-side-copy promotion, partitioning/parallelism, gating estimate.
- **F11** Credential negative control → §11-cred 0.3 control (scrub env + IMDS; SSM-present vs SSM-absent
  delta), now against the projector's own `AmazonS3Builder`.
- **F12** Cargo isolation → §12 separate workspace/lockfile + `cargo tree -e features` build guard.
- **F13** HL-core native trades → §7.4 `no HL-core native trade tape this tranche` forbidden_claim;
  `node_fills_by_block` a separate gated future task.
- **F14** Proof-acceptance sequencing → §5.2 per-family precedence gate, synthetic-only early smoke,
  provider-derived backtest gate; §10 reordered S1–S10 / Phases 0–6; the §5.3 staging hole is closed by
  Tier-C-only physically-non-NT staging (B-1(c)).
- **F15** Prefix-repoint promotion → §4.4 explicit accepted-object PromotionPackage + single-`SnapshotSet`
  CAS; B-1(a) closes the pre-CAS-orphan hole via immutable per-set roots.

### 15.2 v2→v3 findings (B-1, R-2…R-16) — resolved in v3

| Finding | Resolution in v3 |
|---|---|
| **B-1** Catalog read-path incompatibility (3 defects: hash-suffixed names over-included; pointer never consulted so pre-CAS canonical bytes are NT-readable orphans; interim-staging NT catalog guarded only socially) | RESOLVED. §4.3.4: canonical roots use NT-native `timestamps_to_filename` names only, interval-disjoint, one live file per interval; content+transform-hash names are staging-only (NT over-includes unparseable names — `query_intersects_filename` `true` on `None`, `catalog.rs:4741`; NOT a crash). §4.4: canonical NT catalog is materialized into a FRESH IMMUTABLE per-set root via server-side copy AFTER the pointer CAS; the pointer names the active root and NT is pointed at THAT root, so a lost CAS leaves an unreferenced root NT never lists (`catalog.rs:307-322,2040-2063`). §5.3: staging is Tier-C-only under `staged-research/…` (never `data/<type>/`), so NT's `make_path`/`query_files`/`list_parquet_files` cannot enumerate it; a fail-loud validator rejects pending-proof objects under any Tier A/B prefix and non-NT-native names under any canonical root. |
| **R-2** Cross-kind promotion atomicity | RESOLVED. §4.4 step 6: one PromotionPackage commits as a single immutable `SnapshotSet` advanced by ONE CAS on `pointers/set/latest.json`; per-kind pointers are derived. §4.5: the backtest pins the committed set ONCE at run start and derives every `catalog_path` from one immutable per-set root, so NT's independent per-config LISTs (`node.rs:160-182`/`378-386`/`492-505`) all enumerate the same frozen byte set; a run sees the old set or the new set, never a mix. §13.6 resolved. BLOCKER Phase-0 acceptance proof 0.R. |
| **R-3** Idempotency digest over non-deterministic parquet bytes | RESOLVED. §4.3b: one canonical LOGICAL-content digest via `arrow_row::RowConverter` (`parquet.rs:211-249`, in-tree dep `Cargo.toml:69`) over a fixed schema image + total row order + decimal-preserved values; every staging key, PromotionPackage entry, and `ArtifactIndex.content_hash` resolves to it. Determinism contract test. §9.x cost note. |
| **R-4** Instruments lane contradiction (non-atomic `write_instruments`) | RESOLVED. §2.1 + §4.5: instruments go through `ConditionalCatalogWriter`; NT `write_instruments` (`catalog.rs:726`, NOT node.rs:169 = read) never called for platform writes (scope: see §4.1 scope-boundary note for the pre-existing vertical-slice run projection on `main`); `event_time_source` exemption via table-level `time_series=false`. |
| **R-5** NT encoder reuse / buffer seam | RESOLVED. §4.3.0: `write_batches_to_object_store` is `pub` (`parquet.rs:170`) but encode+put share one function (`:178-197`); Phase-0 0.E mandates vendor-encode vs arrow-rs decision + round-trip proof + encoder identity in `NtCapabilityProof`. |
| **R-6** Conditional-put unprovable + multipart | RESOLVED. §4.3.2: resolved mode not introspectable (`aws/precondition.rs:117-160`, `aws/client.rs:209`) and NT can't even set it (`parquet.rs:763-765`); writer builds its own `AmazonS3Builder` + runtime probe at construction (Phase-0 prereq 0.6). §4.3.3: public multipart carries no `PutMode`; per-object size bounded under single-PUT, fail loud. |
| **R-7** Promotion egress / no atomic cross-prefix copy | RESOLVED. §4.4a: server-side `copy_opts(CopyMode::Create)` → S3 `CopyObject` (`aws/mod.rs:312`, `aws/client.rs:596-597,702`), zero egress; size-keyed single vs multipart `UploadPartCopy` (one path); `copy_if_not_exists` capability probed Phase-0 (0.6). §9.x cost addendum. |
| **R-8** `write_mode` migration not atomic | RESOLVED. §6.2: ledger flips from exclusion heuristic (`backfill_coverage_ledger.py:289-293`) to positive identification (`local_staging`+`staging_location=s3_noncanonical`); unknown/missing = hard error; producers + ledger + schema-validation test ship as one indivisible change set (§16 G-A). |
| **R-9** `:v0-pending` violates `source_proof_version` schema | RESOLVED. §6.3: `:v0-pending` is an opaque ROW-ID discriminator; `source_proof_version` is a positive integer (pending = `1`, `status=pending`); provisional id must resolve to a real pending record; acceptance creates a NEW immutable record. Schema-validation test rejects non-integer/<1 versions. |
| **R-10** Orphan acceptance path too weak | RESOLVED. §6.4: distinct `recovered_orphan` state + distinct manifest schema; required resolved ACCEPTED `source_proof_id`; FULL hash verify; complete provenance (incomplete→`unrecoverable`); barred from `accepted_binding` and §4.4 PromotionPackage until a human-reviewed `recovered_orphan → staged` transition. |
| **R-11** Tier-A version coupling | RESOLVED. §12: CI guard pins projector to `6be5a5094716790a8ca2875445fde4fa2586107e` and asserts the pinned `NautilusDataType` replay surface, including `FundingRateUpdate` and `OptionGreeks`, plus exact projector-relevant prefix STRINGS (NOT a blanket count) + `timestamps_to_filename` format. |
| **R-12** Python dual write path | RESOLVED. §3: single Rust writer; Python strictly read-only against the NT catalog (`parquet.py:198,1576,1628,1675,2039`) and Tier-C Parquet; forbidden methods named (`write_data`/`write_chunk`/`consolidate_*`); v2 "Python writes the same format" clause removed; read creds SSM-only. |
| **R-13** Promotion TOCTOU + staging cleanup | RESOLVED. §4.4 step 3: at-WRITE-time re-verification of the §4.3b logical digest before canonical materialization, whole-package fail-loud abort. §4.4b: fail-safe staging-cleanup policy pins every URI a constructed-but-uncommitted PromotionPackage enumerates; per-exact-URI, never prefix/glob; aborts deleting nothing if it cannot enumerate staged packages. |
| **R-14** Synthetic vs provider root collision | RESOLVED. §4.4 step 8 / §10.2: Phase-0/3 proofs write to a dedicated synthetic-only top-level root (`nt-catalog-synthetic-proof/<run_uuid>/`) with a fail-loud disjointness assertion before any byte. |
| **R-15** Design-decided vs repo-edits-pending framing | RESOLVED. §16: D-1..D-7 design-decided vs Group G-A/G-B/G-C repo-edits-pending (atomic, file:line-targeted, with per-group acceptance tests). The plan owns the list; implementation applies the edits. |
| **R-16** Funding native replay boundary | RESOLVED. §13.R16: funding is native `FundingRateUpdate` at the repo-pinned NT rev; custom-data/actor injection is not the funding path and remains reserved for non-native S6/S7 families. |

---

## 16. Design-decided vs repo-edits-pending — implementation-task ledger (resolves R-15, R-8)

This plan is a **design document**, not a code change. Several sections (§6.1, §6.2, §6.3, §7.1, §7.2,
§7.4) describe edits to checked-in repo artifacts (`backfill-source-bindings.v1.toml`,
`backfill-evidence-matrix.v1.toml`, the venue producer scripts, and
`scripts/backfill_coverage_ledger.py`). **Those edits are NOT applied yet.** Adversarial review marked
several findings "PARTIAL" precisely because reviewers checked *is it fixed in the repo* and found the
cited lines still at their pre-edit state, while the plan had *decided the design*. This section draws
the line explicitly and converts every pending edit into an atomic, file:line-targeted implementation
task. **This plan does not itself perform these edits — implementation does — but this plan OWNS the
list and the atomicity constraints so nothing is lost and nothing ships half-migrated.**

Verification basis (every pre-edit line below was re-read at the worktree HEAD of the branch that
carries the targeted artifact — NOT this docs branch, whose tree does not carry the producer/ledger
scripts): the eleven producer `write_mode` lines, the Deribit `write_policy` block, the coverage-ledger
acceptance heuristic, and the binding/matrix lines all still show the **pre-edit** value. Citations are
to those re-read lines. Scope note for the owner: the producer scripts and
`scripts/backfill_coverage_ledger.py` live on `feat/023-venue-data-backfill` (re-verified 2026-06-12:
present at that branch's tip, absent from `main` and from this docs branch; the cited
`backfill_coverage_ledger.py` line numbers match that copy exactly), and the reference TOMLs
(`backfill-source-bindings.v1.toml`, `backfill-evidence-matrix.v1.toml`) now exist on `main`
(re-verified 2026-06-12). v3's §16 line numbers are authored against the branch that carries these
scripts. Because those scripts have not merged to `main`, that merge is itself a precondition the G-A
ledger tasks depend on. A hard "no other `s3_staging` string exists anywhere" guarantee was NOT independently grep-verified
in this pass (the grep-before-read / enforce-dedicated-tools hooks blocked raw grep); each of the 11
producer + Deribit + ledger lines the plan enumerates was verified individually by direct Read and is
pre-edit. Implementation MUST run one repo-wide `s3_staging`/`s3_staging_only` grep and record it as the
completeness check for Group G-A — flagged as the single residual limitation.

### 16.1 Design-decided (in this plan; no repo edit pending)

These are resolved purely by the prose/design of v2/v3 and require no edit to a checked-in artifact.
They are listed so reviewers do not mistake a design decision for an un-applied edit:

| # | Decision | Where decided | Why no repo edit |
| --- | --- | --- | --- |
| D-1 | Three-tier NT-class matrix + `nt_target` column scoping the replay claim | §2 | The `nt_target` column is added to the **expanded evidence matrix produced at S10** (a Phase-6 deliverable), not to a checked-in file today. |
| D-2 | `ConditionalCatalogWriter` create-only write layer (encode-then-`PutMode::Create`); NT's writer (Rust + Python) used only for read-back **within the platform's staging/canonical roots** (the pre-existing vertical-slice run projection on `main` calls NT's writer for run-scoped scratch roots and is outside this invariant — §4.1 scope-boundary note); instruments lane via the same writer | §4.1, §4.3, §4.5, §3 | New code built in Phase 0; no existing file is mutated to *decide* this (the decision record needs no repo edit — this row is not a claim that no existing code calls NT's writer; see the §4.1 scope-boundary note). |
| D-3 | Immutable per-commit/per-set NT-native catalog roots + single-`SnapshotSet` CAS + run-pinned reader + Tier-C-only physically-isolated staging + server-side-copy promotion (B-1 / R-2 / R-7 / R-13 / R-14 class fix) | §4.3, §4.3b, §4.4, §4.4a, §4.4b, §4.5, §5.3 | New build behavior; no checked-in artifact carries the old prefix-flip / per-kind-pointer / parquet-byte-hash design as data. |
| D-4 | Proof-acceptance precedence (synthetic-only early smoke; provider-derived backtest gate) and the reordered S1–S10 / Phases 0–6 | §5.2, §10 | Sequencing decision; no file edit. |
| D-5 | Deterministic provisional `source_proof_id` scheme (never bare `pending`) and the `v0-pending` row-id-vs-`source_proof_version` decoupling (R-9) | §6.3 | The literal `pending` lived only in the **superseded** v1 plan (`normalization-catalog-plan.v1.md:70,227`, archived — see archive note; re-read before archival — confirmed still literal `pending` there); v1 is superseded by this doc, so no live artifact carries it. The scheme is consumed by new Phase-1 library code, not by editing a checked-in data file. |
| D-6 | Best-effort instrument universe + completeness gap record + declared symbol-shape parser | §8 | New Phase-2 code + a new owner gate item; no existing file edit. |
| D-7 | Cost/scale model; Cargo separate-workspace isolation; credential negative control; R-11 NT-surface drift guard | §9, §11, §12 | New crate/CI/cost work; no checked-in data artifact carries the old claim. |

### 16.2 Repo-edits-pending (this plan DESCRIBES them; repo still at pre-edit state)

Each row is a task with the **exact pre-edit state re-read at HEAD**, the target post-edit state, the
section that specifies it, and the atomic group it belongs to. **A task is "done" only when its
acceptance test (16.4) passes — describing the edit in this plan does not close it.**

#### Group G-A — `write_mode` migration MUST ship atomically (R-8 hazard)

**Why atomic (the live-ledger trap, grounded).** The coverage ledger's `parse_manifest`
(`scripts/backfill_coverage_ledger.py:192`) reads `write_mode = str(doc.get("write_mode", ""))`
(`:196`) and accepts a binding only when `write_mode not in ("local_staging", "dry_run")` (`:289-293`,
comment `:285-288`). Today every S3 producer emits `s3_staging` / `s3_staging_only` (or, for Deribit,
**no** top-level `write_mode` → defaults to `""`), so all pass. **If the producers are migrated to
`write_mode="local_staging"` without simultaneously changing the ledger logic, the acceptance test at
`:292` flips every migrated producer from accepted to un-counted** — because `local_staging` *is* in the
disqualifying tuple — and any producer NOT yet migrated still emits `s3_staging`, so the two halves
disagree mid-migration. The ledger must instead count **`local_staging` + `staging_location=s3_noncanonical`**
as S3-staging and reject unknown/missing modes. Therefore **all eleven producer edits + the Deribit edit
+ the ledger logic edit + the schema-validation test land in ONE change set; none ships alone.**

| Task | File:line (pre-edit, re-read) | Pre-edit value | Target | Spec |
| --- | --- | --- | --- | --- |
| G-A.1 | `scripts/backfill_archive_objects_to_s3.py:136` | `"write_mode": "s3_staging"` | `write_mode="local_staging"`, `staging_location="s3_noncanonical"` | §6.2 |
| G-A.2 | `scripts/backfill_binance_to_s3.py:840` | `"write_mode": "s3_staging"` | same | §6.2 |
| G-A.3 | `scripts/backfill_accept_staged_objects.py:209` | `"write_mode": "s3_staging"` | same (orphan-recovery path; this object also gets the `recovered_orphan` state, NOT a plain accepted binding — §6.4) | §6.2/§6.4 |
| G-A.4 | `scripts/backfill_accept_staged_objects.py:330` | `"write_mode": "s3_staging"` | same (manifest-sourced acceptance path) | §6.2 |
| G-A.5 | `scripts/backfill_okx_to_s3.py:614` | `"write_mode": "s3_staging"` | same | §6.2 |
| G-A.6 | `scripts/backfill_hyperliquid_hip4_to_s3.py:762` | `"write_mode": "s3_staging"` | same | §6.2 |
| G-A.7 | `scripts/backfill_hyperliquid_hip3_to_s3.py:764` | `"write_mode": "s3_staging"` | same | §6.2 |
| G-A.8 | `scripts/backfill_bybit_to_s3.py:985` | `"write_mode": "s3_staging_only"` | `write_mode="local_staging"`, `staging_location="s3_noncanonical"` | §6.2 |
| G-A.9 | `scripts/backfill_hyperliquid_core_to_s3.py:749` | `"write_mode": "s3_staging_only"` | same | §6.2 |
| G-A.10 | `scripts/backfill_deribit_to_s3.py:965-970` | `write_policy {…}`, **no** top-level `write_mode` | add `write_mode="local_staging"`, `staging_location="s3_noncanonical"` so it stops being a third "(unset)" path the ledger special-cases | §6.2 |
| G-A.11 | `scripts/backfill_archive_objects.py:159` | `"write_mode": "local_staging"` (already sanctioned) | add `staging_location="local"` | §6.2 |
| G-A.12 | `scripts/backfill_source_proof.py:382` | `"write_mode": "local_staging"` (already sanctioned) | add `staging_location="local"` | §6.2 |
| G-A.13 | `scripts/backfill_coverage_ledger.py:192-196,285-293,298` | `write_mode=str(doc.get("write_mode",""))` (:196); accept iff `write_mode not in ("local_staging","dry_run")` (:292); `(unset)` display (:298); deribit-omits-mode comment (:285-288) | Accept iff `(write_mode=="local_staging" AND staging_location=="s3_noncanonical")` OR (canonical, via the §4.4 set path — never as a staging `accepted_binding`); **reject unknown/missing modes** (the `""` / `s3_staging` / `(unset)` branches become dead and are removed); surface a separate `recovered_orphan_count` (§6.4). Single authority: `backfill-source-proof-schema.md:87`. | §6.2/§6.4 |
| G-A.14 | new test (no pre-edit file) | — | Schema-validation test: any manifest with `write_mode ∉ {dry_run, local_staging, canonical_s3}` is rejected; `s3_staging`/`s3_staging_only`/missing-mode all FAIL; `local_staging` without `staging_location` FAILs; a `local_staging`+`staging_location=s3_noncanonical` manifest counts as `accepted_binding`, a `local_staging`+`staging_location=local` does NOT; the ledger raises on unknown/missing mode; a `recovered_orphan` manifest is in `recovered_orphan_count`, not `accepted_binding_manifest_count`. | §6.2/§6.4 |

`staging_location ∈ {local, s3_noncanonical}` is an **additive** manifest field, NOT a fourth
`write_mode` value (the sanctioned enum stays three-valued, `backfill-source-proof-schema.md:87`,
re-read). `canonical_s3` remains the only value asserting a canonical, gate-passed write
(`backfill-source-proof-schema.md:97-98`, re-read).

#### Group G-B — Binance four-family taxonomy binding reconcile (F3)

Independent of G-A (touches bindings, not producers/ledger), but G-B.1 and G-B.2 ship together because
they are the same `*_or_delivery → acquisition_group` rename on the two Binance instrument bindings.

| Task | File:line (pre-edit, re-read) | Pre-edit value | Target | Spec |
| --- | --- | --- | --- | --- |
| G-B.1 | `backfill-source-bindings.v1.toml:24` | `product_family = "usd_m_perpetual_or_delivery"` | `acquisition_group = "usd_m"` + `normalized_product_families = ["usd_m_perpetual","usd_m_delivery"]`; remove `product_family` | §6.1 |
| G-B.2 | `backfill-source-bindings.v1.toml:37` | `product_family = "coin_m_perpetual_or_delivery"` | `acquisition_group = "coin_m"` + `normalized_product_families = ["coin_m_perpetual","coin_m_delivery"]`; remove `product_family` | §6.1 |

#### Group G-C — Per-venue fidelity downgrades in the evidence matrix + bindings (F4, F5, F13)

Each venue's matrix-row edit and its paired binding edit ship together (a binding that still lists a
family the matrix has downgraded is the exact desync reviewers flagged).

| Task | File:line (pre-edit, re-read) | Pre-edit value | Target | Spec |
| --- | --- | --- | --- | --- |
| G-C.1 (OKX spot/swap/future) | `backfill-evidence-matrix.v1.toml:76` | `order_book_deltas` present in `directly_backfillable` | remove `order_book_deltas` from `directly_backfillable`; move to `pending_source_proof` keyed `okx_native_seqid_l2_archive`; add `order_book_snapshot_deltas` to `directly_backfillable` | §7.1 |
| G-C.2 (OKX option) | `backfill-evidence-matrix.v1.toml:86` | `order_book_deltas` present in `directly_backfillable` | remove from `directly_backfillable`; same `pending_source_proof` key | §7.1 |
| G-C.3 (HL-core) | `backfill-evidence-matrix.v1.toml:148` | `trades` present in `owner_archive_backfillable`; `node_fills_trade_dedupe` already in `pending_source_proof` (`:150`, re-read — confirmed) | relocate `trades` from `owner_archive_backfillable` (:148) into the existing `pending_source_proof` key `node_fills_trade_dedupe` (:150) | §7.4 |
| G-C.4 (Polymarket matrix) | `backfill-evidence-matrix.v1.toml:180` | `order_book_snapshot_deltas`, `bars`, `trades` present in `owner_archive_backfillable` | keep only `order_book_snapshots_fixed_depth` as manifest-backed; move `order_book_snapshot_deltas`+`bars`+`trades` to `pending_source_proof` keyed `polymarket_pmxt_event_type_demux`; `order_book_deltas` stays `vendor_or_forward_capture_only` (`:183`, re-read — already correct, no edit) | §7.2 |
| G-C.5 (Polymarket binding) | `backfill-source-bindings.v1.toml:294` | `table_families = ["order_book_snapshots_fixed_depth", "order_book_snapshot_deltas", "bars"]` | `table_families = ["order_book_snapshots_fixed_depth"]` | §7.2 |

### 16.3 Atomicity / ordering summary

- **G-A is one indivisible change set (14 tasks).** Shipping any producer edit without the ledger edit
  (G-A.13) silently un-counts that producer at `backfill_coverage_ledger.py:292`; shipping the ledger
  edit without the producers leaves live producers emitting `s3_staging`, which the new logic rejects.
  Merge order is verify → ledger+test (G-A.13, G-A.14) → producers, so the schema-validation test guards
  the rename. The schema-validation test is the fail-loud proof both halves agree.
- **G-B is one change set (2 tasks)** — the paired Binance binding rename.
- **G-C pairs by venue:** G-C.1+G-C.2 (OKX), G-C.3 (HL-core, single line), G-C.4+G-C.5 (Polymarket
  matrix + binding together). A matrix downgrade without the paired binding edit re-creates the
  family-vocabulary desync the review flagged.
- **G-A, G-B, G-C are independent of one another** and may ship as separate PRs, each one declared scope
  (CLAUDE.md rule 9). None of them is authorized by *this* plan to perform a `canonical_s3` write — they
  only correct staging-side labeling and evidence-matrix truth (the canonical gate, §5.1, is untouched).

### 16.4 Per-group acceptance tests (a task is not done until its test passes)

- **G-A:** (1) every producer manifest serializes `write_mode="local_staging"` (or `canonical_s3`) with
  a non-null `staging_location`; no manifest serializes `s3_staging`/`s3_staging_only`/missing mode.
  (2) `backfill_coverage_ledger.py` re-run over the migrated manifests counts the same accepted object
  set as before migration (no producer silently dropped) — the regression guard against the R-8 trap.
  (3) the schema-validation test (G-A.14) rejects `s3_staging`, `s3_staging_only`, and missing mode,
  asserts the `write_mode not in (...)` heuristic is gone, and asserts `recovered_orphan` is uncounted.
  (4) the repo-wide `s3_staging`/`s3_staging_only` grep (the residual-limitation completeness check)
  returns no occurrences outside the migrated set.
- **G-B:** parsing the two Binance bindings yields `acquisition_group` ∈ {`usd_m`,`coin_m`} and the two
  `normalized_product_families` lists; assert **no** binding contains `usd_m_perpetual_or_delivery` /
  `coin_m_perpetual_or_delivery` anywhere.
- **G-C:** assert the evidence matrix lists `order_book_deltas` for OKX only under `pending_source_proof`
  (key `okx_native_seqid_l2_archive`), HL-core `trades` only under `pending_source_proof` (key
  `node_fills_trade_dedupe`), and Polymarket `owner_archive_backfillable` contains only
  `order_book_snapshots_fixed_depth`; assert the Polymarket binding `table_families` equals
  `["order_book_snapshots_fixed_depth"]`.
