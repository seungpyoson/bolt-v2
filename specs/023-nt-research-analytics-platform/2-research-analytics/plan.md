# Plan: Research Analytics

## Architecture

Research Analytics stands on NautilusTrader for every analytical primitive. It
builds only what NT lacks. The pipeline is a layered stack; each layer names the
NT facilities it consumes and the minimal we-build delta on top.

```text
L0 catalog (NT ParquetDataCatalog in S3)        <- canonical input
  -> L1 read (NT query<T> / DataBackendSession)  -> thin reader helper
  -> L2 features (NT DataActor / indicators / BarAggregator / greeks)
                                                 -> point-in-time enforcement
  -> L3 backtest (the ONE NT BacktestEngine the BTE wraps)
                                                 -> sweep orchestration + cost realism
  -> L4 evaluate (NT PortfolioAnalyzer / PortfolioStatistic)
                                                 -> domain metrics + run index
  -> L5 present (off-the-shelf BI over NT Arrow output + #409)
                                                 -> notebook ergonomics + dashboard
```

Research Analytics is flexible at the exploration layer, but research validity is
strict: every feature and result traces to evidence, no future data leaks into a
join, and the one Rust BacktestEngine is the only source of fill/PnL truth.

## Layered Architecture And We-Build Delta

Each layer reuses NT and adds only the named delta. RA never reimplements an NT
primitive.

- **L0 — Data.** NT `ParquetDataCatalog` in S3 is the single canonical input:
  `from_uri` S3 access plus the Tardis/CSV streamers and `EncodeToRecordBatch`.
  NT's own catalog write is not durable/immutable: a non-atomic `head()`
  existence check then an unconditional `object_store.put` (overwrite-by-default;
  `parquet.rs` / `backend/catalog.rs`) — NT names no `PutMode` at all
  (`grep PutMode` over NT crates returns zero hits). WE BUILD: make the
  already-proven venue converters write to
  S3 *durably and immutably* (today they reproduce-on-demand into a local catalog
  only) via the conditional create-only writer over `object_store`'s
  `PutMode::Create` (S3 If-None-Match) per
  `../reference/normalization-catalog-plan.v3.md`, plus config-driven venue
  dispatch. This is Gate 0 below.
- **L1 — Read.** NT typed `query<T>` (instrument + time + SQL `where`
  pushdown) and `DataBackendSession` (DataFusion SQL to Arrow). WE BUILD: a thin
  reader helper of a few dozen lines over those APIs.
- **L2 — Features.** NT indicators (38 `impl Indicator for` in the Rust
  indicators crate; 47 public names in NT's Python `indicators` package), the
  `BarAggregator` family, `Clock`/`TestClock`, `Cache`,
  and NT-native implied-volatility / Black-Scholes greeks
  (`crates/model/src/data/greeks.rs`) as the offline-usable primitives. NT
  `DataActor` is a live actor-runtime Component (Clock/MessageBus/Cache-bound),
  usable only for in-backtest / in-actor feature computation, not as an offline
  batch primitive over a catalog query result. WE BUILD: point-in-time / leakage
  enforcement, the one research-validity invariant NT does not provide.
- **L3 — Backtest.** THE ONE Rust `BacktestEngine` that the Backtesting Engine
  already wraps. RA never owns a runner. WE BUILD: sweep orchestration plus
  Polymarket cost realism implemented as `FeeModel` / `FillModel` /
  `LatencyModel` trait impls.
- **L4 — Evaluate.** NT `PortfolioAnalyzer`, the `PortfolioStatistic` trait
  suite, registered via the `PortfolioAnalyzer::register_statistic` method.
  WE BUILD: domain metrics as new
  `PortfolioStatistic` impls that run IN-PROCESS during the backtest (registered
  before the run via the BTE — the trait methods take live `&Returns`,
  `Vec<Box<dyn Order>>`, and `&[Position]` per `crates/analysis/src/statistic.rs`,
  not the flat stat maps the persisted `BacktestResultContract` carries).
  Post-hoc domain metrics are computed from the Contract's aggregates, or require
  the BTE to explicitly export per-period returns / `OrderFilled` /
  `PositionOpened` series; do not re-run statistics over the persisted contract.
  Plus a thin run-id -> params -> result-pointer index over
  `catalog.list_backtest_runs`.
- **L5 — Present.** Off-the-shelf BI (duckdb / polars over the NT catalog's Arrow
  output) and #409 `PortfolioSnapshot` as the single PnL read source. NT
  `reporter.py` / `tearsheet.py` are Python (pandas) analysis APIs, excluded
  because the single-engine invariant keeps RA on the Rust path and bans
  importing NT's Python/Cython engine layer — not because they need a live cache
  (the `reporter.py` helpers take plain `list[Order]` / `list[Position]` and
  `create_tearsheet_from_stats` takes precomputed stats; only the engine-driven
  `create_tearsheet` needs `engine.kernel.cache`). The supported presentation
  path is the duckdb / polars Arrow read above. WE BUILD:
  notebook ergonomics and the dashboard product (off-the-shelf first, custom UI
  only as a fallback).

## Single-Engine Invariant

RA orchestrates the ONE Rust BacktestEngine the BTE already wraps. It writes N
typed run-spec TOMLs, invokes the existing entrypoint
(`operator::run_from_run_spec` / the CLI binary), and reads the persisted
`BacktestResultContract` (the JSON the entrypoint writes; NT's in-process
`BacktestResult` is `#[derive(Debug)]`-only and never persisted, so an
out-of-process orchestrator can only read the Contract). RA MUST NEVER import
NT's Cython/Python backtest engine
(`nautilus_trader.backtest.engine` or `nautilus_trader.backtest.node`); that is a
second engine with different fill/PnL truth. This is enforced mechanically by a
notebook-boundary test that fails on importing the Cython engine. Rule #5
(pure-Rust live binary) is untouched here: it governs the live trading binary,
not research tooling.

## Source Rules

- The NT catalog is the single canonical analytics input. It is read by URI from
  the configured S3 `artifact_root` (`from_uri` S3) and is treated as
  *incrementally growing* — only Polymarket is durable today; BTE work lands more
  venues over time.
- Raw evidence is audit input. NT catalog data is canonical replay/backtest
  input. NT reports/results/events/snapshots are trading-state source data.
- Analytics tables are derived and must carry source hashes and freshness.
- Exploratory sources must be labeled non-trading-truth.
- ONE documented carve-out: latency / lead-lag receive-offset research may read
  raw archives because the current converter drops capture/receipt time. This is
  a TEMPORARY fallback with an explicit SUNSET tied to issue #677 (fix the
  converter to write `ts_init = capture_time`), NOT a permanent dual path.
- The `polymarket_parquet` tabular layer is convenience-only exploratory, NOT
  canonical, and carries a hard sunset condition tied to catalog coverage (not
  "never deleted").
- Kimchi premium sources require separate Korean spot, reference price, and
  FX/quote source proofs; Upbit/Bithumb-style sources are candidate TOML
  bindings, not hardcoded analytics branches.
- Source-proof acceptance is the only path to backtest input. RA preserves
  upstream `SourceProofReport` ids, fidelity classes, and claim limits through
  datasets, experiments, and results. It may narrow claims but cannot accept
  upstream proof, upgrade proof strength, or weaken forbidden claims.
- Analytics is read-only for upstream raw, NT catalog, source-proof, and backtest
  artifacts. It does not fork those artifacts into a second canonical root and
  does not mutate accepted proof records.
- Every dataset and result carries a single `artifact_root` reference and
  lightweight `sha256` content hashing; RA does not introduce a parallel storage
  root.

## Implementation Gates

These mirror the layered delivery. Each later gate reads what the earlier gate
writes.

0. **Durable conversion persistence (the unblocking prerequisite).** Make the
   proven venue converters write the NT catalog to S3 durably under the
   configured `artifact_root`, replacing today's reproduce-on-demand local-only
   behavior, with config-driven venue dispatch. Every later phase reads what this
   gate writes; nothing downstream is real until the canonical catalog is
   durable.
1. Define the thin L1 reader helper over NT typed `query<T>` /
   `DataBackendSession` (instrument + time + SQL `where` pushdown).
2. Define point-in-time join and leakage-check rules on the L2 feature path.
3. Define how the proven lead-lag lane is lifted onto the catalog reader and the
   NT feature/indicator/greeks primitives.
4. Define claim-limit propagation from source fidelity to research result.
5. Define the notebook permission boundary, including the Cython-engine import
   ban.
6. Define the single-engine sweep orchestration. Each run is not "TOML in,
   result out": it also supplies the accepted object bytes whose `sha256` the
   run-spec pins (`operator::run_from_run_spec(spec, gz_bytes, output_dir)`; CLI
   `--object-gz`), and reads back the persisted `BacktestResultContract`. So:
   typed run-spec TOMLs + accepted `gz_bytes` -> `operator::run_from_run_spec`
   -> persisted `BacktestResultContract`.
7. Define domain metrics as new `PortfolioStatistic` impls and the thin
   run-id -> params -> result-pointer index over `catalog.list_backtest_runs`.
8. Define the L5 presentation read path: off-the-shelf BI over NT Arrow output,
   #409 `PortfolioSnapshot` as the single PnL read source.
9. Define cost refresh and provider/license proof triggers for selected data.

## Backtest Phase Prerequisite

The BTE runner today wires only an NT example strategy
(`HurstVpinDirectional`) over a single venue (`bybit-spot`). Bolt's
`binary_oracle_edge_taker` strategy and venue normalization must be wired into
the BTE engine before any Phase-3 sweep is real. Surface this prerequisite
explicitly in the backtest phase; do not hide it. NT's pyo3
`add_native_strategy` is `#[cfg(feature = "examples")]` and can only run NT
example strategies, not bolt's, so this wiring is a hard precondition, not an
optional optimization.

## Point-In-Time Rules

- Every feature must declare event time, availability time, and join key.
- Joins must use as-of semantics, never future observations.
- Dataset snapshots must carry source hashes and query/config hashes.
- Research output must preserve source fidelity and forbidden claims.
- Cross-market premium features must align Korean spot price, reference price,
  and FX/quote observations by event time and availability time.

## Verdict And Re-Measurement Rules

RA owns the subjective verdict; the BTE emits objective results only. There is no
standing promotion machine, no multi-state package enum, no proof-pin/run-purpose
enum threading, and no promotion-package-specific Artifact Index or lifecycle
layer. The verdict is an ordinary field on the RA `experiment-results` artifact,
which commits into the shared `research-analytics` Artifact Index snapshot and
records its lifecycle state like every other RA artifact (see
`../reference/data-model.md` and `../reference/contracts.md`) — there is simply no
second, promotion-only index or lifecycle machine layered on top of that.

- The BTE emits an objective `BacktestResult` — the in-process NT object the
  persisted `BacktestResultContract` is built from. RA, out-of-process, reads the
  Contract and owns the SUBJECTIVE verdict on top of it.
- A finding is recorded with the proven lead-lag lane's GO / NO-GO verdict plus a
  re-measurement cadence, not a six-state package or an approved-for-config
  checklist.
- A promotion gate is added only WHEN a real finding exists to promote. Until
  then there is nothing to gate.
- Notebook code cannot become production runtime. When a finding is promoted, the
  only output is typed TOML/NT-compatible config for later implementation and
  review — never an auto-merge, an auto-enabled strategy, a live-trading
  schedule, an SSM credential touch, or a production-runtime mutation.
- Promotion must not bypass SSM-only live credential handling or the Rust-only
  production runtime rules.

## Issue Payload

Title: `Plan: NT-first research analytics over the durable NT catalog`

Accepted scope: define the L0->L5 layered RA stack on NT primitives — durable
catalog persistence (Gate 0), the thin read helper, point-in-time features +
lifting the lead-lag lane, single-engine sweeps, evaluate with domain metrics +
run index, and present — plus the source-trace, leakage, and notebook-boundary
correctness invariants.

Out of scope: building the analytics DB/read model, notebook implementation,
strategy productionization, owning or replacing the NT backtest runner, and
replacing NT reports.

## Test Plan

- Leakage fixtures fail when future data is joined.
- Point-in-time fixtures fail closed when a feature observes future data.
- The notebook boundary fails on any production mutation capability (order,
  cancel, transfer, or credential mutation).
- The notebook boundary fails on importing NT's Cython backtest engine
  (`nautilus_trader.backtest.engine` or `nautilus_trader.backtest.node`); only
  the Rust BacktestEngine path via `operator::run_from_run_spec` is allowed.
- Gate 0: the durable conversion writes the NT catalog to S3 under the configured
  `artifact_root`, and the L1 reader reads back exactly what was written; a
  reproduce-on-demand local-only result is not accepted as canonical.
- One no-hardcoded-venue test proves venue/provider identity is TOML/registry
  selected data across reader, feature, sweep, and verdict paths.
- Experiment manifests fail when source hashes or as-of bounds are missing.
- Every feature and result traces to an evidence reference; an untraced feature
  or result fails closed.

## Residual Risks

- The NT catalog is durable only for Polymarket today; other venues become
  canonical only as BTE conversion work lands them, so coverage is incremental.
- The lead-lag raw-archive carve-out persists until issue #677 fixes the
  converter to write `ts_init = capture_time`; until then that one path reads raw
  archives.
- Phase-3 sweeps are not real until `binary_oracle_edge_taker` and venue
  normalization are wired into the BTE engine (today only `HurstVpinDirectional`
  over `bybit-spot`).
- Research results can overclaim if fidelity labels are dropped.
- A second engine (NT's Cython/Python backtest) would split fill/PnL truth; the
  notebook-boundary import ban is the only thing preventing it.
