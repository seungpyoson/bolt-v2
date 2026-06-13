# Spec: Research Analytics

## Scope

Build a future research-only analytics project that STANDS ON NautilusTrader for
every analytical primitive — data catalog, typed query, indicators/aggregators,
the one Rust backtest engine, portfolio analytics — and builds ONLY what NT
lacks. It owns experiments, notebooks, point-in-time correctness,
feature/result lineage, research-validity invariants, and the subjective
GO/NO-GO verdict over NT's objective results.

This is separate from Backtesting Engine and Dashboard. It does not own the NT
runner, live trading, dashboard UI, provider procurement, or independent
PnL/account truth.

## Users

- Researcher: explores data, features, strategies, and results.
- Maintainer: reviews experiment lineage, leakage controls, and findings before
  anything can affect production runtime.

## Requirements

- Every feature/result must trace to raw evidence, catalog projection, NT result,
  NT report, or explicitly exploratory source.
- Research notebooks may read data and produce analysis artifacts only.
- Notebook/Python workflows must not submit orders, cancel orders, transfer
  funds, mutate credentials, or become production runtime.
- Feature joins must be point-in-time correct and carry as-of/freshness rules;
  future-data joins must fail closed.
- Experiments must record parameters, datasets, hashes, metrics, artifacts,
  fidelity class, and claim limits.
- Analytics/read models must not become independent PnL, position, account, or
  portfolio truth.
- Venue/provider identity remains TOML/registry-selected data, not hardcoded
  analytics branches.
- Kimchi premium features must use TOML/registry-selected Korean spot price
  source(s), reference spot/perps price source(s), FX/quote conversion
  source(s), and point-in-time joins.
- Research datasets must reference canonical NT catalog, raw evidence,
  source-proof, and backtest artifacts under the single configured S3
  `artifact_root`, content-hashed with `sha256`. Analytics must not create a
  second canonical storage root for the same artifacts.
- Research datasets and experiment outputs must preserve upstream
  `SourceProofReport` ids, fidelity classes, and claim limits. Analytics may
  narrow claims but must not upgrade source/backtest fidelity.
- Analytics must not mark upstream `SourceProofReport` records accepted or
  weaken forbidden claims for catalog/backtest use.
- Analytics must orchestrate the ONE Rust BacktestEngine through typed run-spec
  TOMLs and read its persisted `BacktestResultContract`. Analytics owns the
  subjective GO/NO-GO verdict; it must not own a runner or emit objective result
  truth.

## NautilusTrader Facility Map

Research Analytics is layered on NT facilities. Each layer reuses an NT facility
and builds only the named gap.

| Layer | Stands on (NT facility) | WE BUILD (NT lacks) |
|---|---|---|
| L0 data | `ParquetDataCatalog` in S3 = single canonical input (`from_uri` S3, Tardis/CSV streamers + `EncodeToRecordBatch`). NT's own catalog write is non-atomic — a `head()` existence check then an unconditional `object_store.put` (overwrite-by-default; `parquet.rs` / `backend/catalog.rs`); NT names no `PutMode` at all (`grep PutMode` over NT crates returns zero hits). | Make the proven converters write to S3 DURABLY + IMMUTABLY (today reproduce-on-demand local only) via the conditional create-only writer over `object_store`'s `PutMode::Create` (S3 If-None-Match) per `../reference/normalization-catalog-plan.v3.md`, plus config-driven venue dispatch. |
| L1 read | NT typed `query<T>` (instrument + time + SQL where pushdown) + `DataBackendSession` (DataFusion SQL → Arrow). | A thin reader helper (~dozens LOC). |
| L2 features | NT indicators (38 `impl Indicator for` in the Rust indicators crate; 47 public names in NT's Python `indicators` package), `BarAggregator` family, `Clock`/`TestClock`, `Cache`, and NT-native implied-vol / Black-Scholes greeks (`crates/model/src/data/greeks.rs`) as the offline-usable primitives. NT `DataActor` is a live actor-runtime Component (Clock/MessageBus/Cache-bound) — usable only for in-backtest / in-actor feature computation, NOT as an offline batch primitive over a catalog query result. | Point-in-time / leakage enforcement (the one research-validity invariant NT lacks). |
| L3 backtest | THE ONE Rust `BacktestEngine` the BTE already wraps. RA NEVER owns a runner. | Sweep orchestration + Polymarket cost realism as `FeeModel` / `FillModel` / `LatencyModel` trait impls. |
| L4 evaluate | NT `PortfolioAnalyzer` + the `PortfolioStatistic` trait suite, registered via the `PortfolioAnalyzer::register_statistic` method. | NEW `PortfolioStatistic` impls run IN-PROCESS during the backtest (registered before the run via the BTE; the trait methods take live `&Returns` / `Vec<Box<dyn Order>>` / `&[Position]` — `crates/analysis/src/statistic.rs`, NOT the flat stat maps the persisted `BacktestResultContract` carries). Post-hoc domain metrics are computed from the Contract's aggregates, or require the BTE to explicitly export per-period returns / `OrderFilled` / `PositionOpened` series. Do not re-run statistics over the persisted contract. Plus a thin run-id → params → result-pointer index over `catalog.list_backtest_runs`. |
| L5 present | Off-the-shelf BI (duckdb/polars over NT catalog Arrow output) + #409 `PortfolioSnapshot` as the single PnL read source. NT `reporter.py` / `tearsheet.py` are Python (pandas) analysis APIs — excluded because the single-engine invariant keeps RA on the Rust path and bans importing NT's Python/Cython engine layer, not because they need a live cache (the `reporter.py` helpers take plain `list[Order]`/`list[Position]` and `tearsheet.py`'s `create_tearsheet_from_stats` takes precomputed stats; only the engine-driven `create_tearsheet` needs `engine.kernel.cache`). The supported presentation path is the duckdb/polars Arrow read above. | Notebook ergonomics + the dashboard product (off-the-shelf, custom UI fallback only). |

### Single-Engine Invariant

RA orchestrates the ONE Rust BTE: write N typed run-spec TOMLs, invoke the
existing entrypoint (`operator::run_from_run_spec` / the CLI binary), read the
persisted `BacktestResultContract` (NT's in-process `BacktestResult` is
`#[derive(Debug)]`-only and never persisted; the Contract is the JSON
`run_from_run_spec` writes and is what RA, out-of-process, reads). RA MUST NEVER
import NT's Cython/Python backtest engine
(`nautilus_trader.backtest.engine` / `.node`) — that is a SECOND engine with
different fill/PnL truth. Verified: pyo3 `add_native_strategy` is
`#[cfg(feature="examples")]` and can only run NT example strategies, not bolt's.
Enforce mechanically: a notebook-boundary test that FAILS on importing the
Cython engine. (Project rule #5, pure-Rust live binary, is untouched — it
governs the live trading binary, not research tooling.)

Known prerequisite (do not hide): the BTE runner today registers only an NT
example strategy (`HurstVpinDirectional`) over one venue (`bybit-spot`); bolt's
`binary_oracle_edge_taker` + venue normalization must be wired into the BTE
before Phase-3 sweeps are real.

### Canonical Input

The NT catalog is canonical and is treated as INCREMENTALLY growing — only
Polymarket is durable today; BTE work lands more venues over time. There is ONE
documented carve-out: latency / lead-lag receive-offset research may read raw
archives because the current converter drops capture/receipt time. This is a
TEMPORARY fallback with an explicit SUNSET tied to issue #677 (fix the converter
to write `ts_init = capture_time`), NOT a permanent dual path. The
`polymarket_parquet` tabular layer is convenience-only exploratory, NOT
canonical. Its sunset has a concrete, measurable trigger and an owner, parallel
to the #677 carve-out: it is sunset (deleted, not "never deleted") when the NT
catalog covers the Polymarket instruments / windows the lead-lag lane needs;
tracked under #676.

## Evidence And Decisions

| Row | Status | Meaning for this project |
|---|---|---|
| E-002 | SOURCE_PROVEN | NT catalog projection is the replay/backtest data basis analytics should reference. |
| E-014 | SOURCE_PROVEN | Polymarket discovery, data, and CLOB sources are separate source families; lineage must preserve provenance. |
| E-015 | SOURCE_PROVEN + DECISION_NEEDED | Telonex is a Polymarket historical-data candidate; Plus is personal-use priced and commercial/team use needs license proof. |
| E-016 | SOURCE_PROVEN + DECISION_NEEDED | Goldsky can support Polymarket on-chain/provenance indexing, but it is usage-metered and should be selected only after event/storage/query estimates. |
| E-017 | SOURCE_PROVEN | NT reports/events/snapshots can be analytics inputs for trading-state-derived metrics. |
| E-020 | SOURCE_PROVEN | Existing issues overlap with data lake, strategy, and readiness work; do not create one broad duplicate issue. |
| E-024 | USER_ASSUMPTION + DECISION_NEEDED | Model best-fidelity data/product choices first, then expose all-in cost for user review. |
| E-025 | SOURCE_PROVEN | Python/Jupyter is research-only and cannot become the production trading runtime. |
| E-026 | SOURCE_PROVEN | Venue/product/provider identity is TOML/registry-selected data. |
| E-029 | SOURCE_PROVEN | Live credentials must remain AWS SSM-only; analytics and promotion flows must not introduce alternate secret sources. |
| E-030 | SOURCE_PROVEN + DECISION_NEEDED | MarketLens, PMXT, PolyBackTest, PolymarketData, and Goldsky are candidates after schema/license/sample proof. |
| E-031 | SOURCE_PROVEN | Lean, Qlib, Freqtrade, and Feast support separation of research/backtest/live lifecycle and leakage controls as prior art. |
| E-033 | USER_ASSUMPTION + DECISION_NEEDED | Kimchi premium is a required cross-market feature/source family; Upbit/Bithumb-style Korean spot prices are candidate bindings, not hardcoded analytics branches. |
| E-041 | SOURCE_PROVEN | Backtesting Engine emits objective result contracts only; Research Analytics owns the subjective GO/NO-GO verdict over those results. |

## Fidelity Class Reference

Research Analytics consumes fidelity labels from Backtesting Engine and data
source contracts. It must preserve them in datasets, experiments, metrics, and
findings.

| Class | Meaning for analytics |
|---|---|
| `L2_REPLAY` | Execution-quality claims may be analyzed only for the proven venue/source/instrument scope. |
| `TRADE_BAR_REPLAY` | Price, alpha, trade/fill, candle, or bar research only; no queue/execution-quality claims. |
| `SIGNAL_ONLY` | Signal, feature, provenance, or dashboard context only. |
| `FORWARD_CAPTURE_PENDING` | Capture may start now; historical analytics cannot claim replay coverage yet. |

## Prior-Art Rules

- Lean: keep research, backtest, optimize, and live promotion as separate
  lifecycle gates.
- Qlib: experiments need dataset, model, parameter, metric, artifact, and
  analysis lineage.
- Freqtrade lookahead analysis: backtest/research validity can fail when signals
  see future data; add leakage fixtures.
- Feast: feature sets need point-in-time semantics.
- Kimchi premium: treat premium as a derived cross-market feature from Korean
  spot price, reference price, and FX/quote sources; never join future reference
  or FX observations into earlier signals.

## Data Model

- `ResearchDataset`: raw evidence records, catalog projections, NT result/report
  references, artifact URIs under configured `artifact_root`, source hashes,
  source proof ids, fidelity classes, claim limits, and as-of bounds.
- `RawEvidenceRecord`: source family, source URI or redacted pointer, capture
  time, source time range, payload hash, schema/version, license reference, and
  lineage parent.
- `CatalogProjection`: projection id, source records, NT pointer, catalog path,
  NT data type, instrument ids, transform/config hash, fidelity class, and
  validation status.
- `ExperimentRun`: parameters, code/artifact refs, dataset refs, metrics,
  result hashes, consumed `BacktestResultContract` refs, fidelity class, and
  claim limits.
- `FeatureDefinition`: source fields, join keys, event time, availability time,
  and leakage checks.

## Findings & Promotion

Findings are recorded with the lead-lag lane's GO / NO-GO verdict model and a
re-measurement cadence — see `../reference/leadlag-lane.md` as the seed RA model.
BTE emits the OBJECTIVE results; RA owns the SUBJECTIVE verdict: a finding is
GO, NO-GO, or conditional-GO over a stated venue/instrument scope, tied to the
evidence rows and fidelity class it was measured under, with a re-measurement
cadence so a verdict cannot silently go stale.

A promotion gate (typed TOML/NT-compatible config for the Backtesting Engine) is
added only WHEN a real GO finding exists to promote. There is no standing
promotion machine: promotion produces a typed config artifact for later
implementation/review, never a Python strategy runtime path, and never
auto-merges, auto-enables a strategy, schedules live trading, touches SSM
credentials, or mutates production runtime config.

## Issue Dependencies

Link or depend on #19, #20, #21, #22, #24, #34, #39, #75, #148, #158, #176,
#236, #407, and #677 as applicable. Existing data-lake and strategy issues do
not fully cover research analytics, alpha exploration, or findings.

## Non-Goals

- No NT backtest runner implementation, and no second (Cython/Python) backtest
  engine.
- No dashboard UI or operator control plane.
- No live trading or credential mutation.
- No provider recorder or data-lake capture expansion.
- No custom PnL/account truth.
- No standing promotion machine (6-state enum, approved-for-config checklist,
  promotion-package-specific lifecycle/Artifact-Index layer); a promotion gate
  exists only when a real finding needs one. The verdict itself still rides on the
  ordinary RA `experiment-results` artifact and the shared `research-analytics`
  Artifact Index (see `../reference/data-model.md`).

## Acceptance

- Reviewer can see the NautilusTrader facility map (L0–L5) and the single-engine
  boundary (no Cython/Python engine import).
- Reviewer can reproduce each research result from recorded source hashes and
  parameters.
- Reviewer can identify which claims are execution-quality, lower-fidelity, or
  exploratory.
- Reviewer can see point-in-time / leakage proof (future-data joins fail closed).
- Reviewer can prove notebooks have no production mutation path.
- Reviewer can see a GO/NO-GO finding with its re-measurement cadence — not a
  6-state promotion package.
