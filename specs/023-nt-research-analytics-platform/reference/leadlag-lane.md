# Lead-Lag Lane: First-Class RA Seed Model

Related: #676 (RA rearchitecture), #677 (converter capture-timestamp sunset).

---

## What the Lane Is

`scripts/leadlag_*.py` is a read-only research lane that produced a real
GO/NO-GO trading verdict (issues #617 / #626 / #631) with a defined
re-measurement cadence — no ceremony, no Artifact Index, no promotion
machinery.

Key properties:

- **Data path.** Reads the S3 lake directly via duckdb/polars. Sources: pmxt
  Parquet archive and Hyperliquid snapshot objects (both staged under
  `s3://bolt-parquet/backfill-staging/`). No order submission, no credential
  mutation, no live host access.
- **Dependency management.** PEP 723 inline `# /// script` blocks + `uv run`.
  One command, pinned deps, no separate environment setup.
- **Verdict output.** Per-asset GO/NO-GO table (spread reality, fee reality,
  lead-lag event study, market-implied calibration). The re-measurement harness
  (`leadlag_remeasure.py`) re-runs the full pipeline on a fresh window; a
  re-measurement is a re-run, never a re-derivation.
- **Re-measurement cadence** (owned by `leadlag_remeasure.py` docstring, not
  repeated here): once after the #630 pilot closes, monthly thereafter, and
  after structural market changes. Minimum window: 4 consecutive days (#631).
- **Zero ceremony used.** No `PromotionPackage` enum, no `proof_pin_reason`,
  no `lifecycle_state`, no `>=2-binding` fixture obligation. The GO/NO-GO
  verdict is the gate; evidence is a dated report file in `docs/research/`.

---

## Why It Is the RA Template

The lane demonstrates the correct RA shape:

1. **Thin reader + stats, not a second engine.** duckdb/polars read Arrow data
   and produce statistics. The BTE (the one Rust `BacktestEngine`) is not
   touched; RA orchestrates it for strategy-level sweeps but does not own a
   parallel runner. RA must never import NT's Cython/Python backtest engine
   (`nautilus_trader.backtest.engine` / `.node`) — see `../2-research-analytics/spec.md` Single-Engine
   Invariant.
2. **Verdict over artifact index.** The lead-lag lane shipped a trading decision
   with a markdown report and a re-run script. No index file tracked the
   artifact. This is the pattern: lightweight provenance (content hash +
   report path) not a lifecycle state machine.
3. **Cadence over gates.** Re-measurement is scheduled (cadence policy in
   docstring) not gated on a `promotion_state` transition. Add a gate only when
   a real finding exists to gate on.

---

## Migration: Lift Strategy-Fidelity Reads onto NT Catalog

The lane currently reads raw tabular archives for all measurements, including
both strategy-fidelity analysis (spread, fee, event study) and receive-offset /
latency measurement.

**Target state:**

- **Strategy-fidelity portions** (spread, fee, PM event study, calibration):
  replace raw tabular reads with NT `DataBackendSession` / typed `query<T>`
  calls against the NT `ParquetDataCatalog` in S3 (`from_uri` S3 path,
  config-driven). This is a thin substitution — a few dozen lines — because the
  output is the same Arrow/polars frame; only the read call changes.
  Prerequisite: the catalog covers Polymarket for the measurement window (today
  proven for Polymarket; other venues land as BTE work progresses per #676).

- **Receive-offset / latency measurement** (`leadlag_clock_alignment.py`,
  `leadlag_subsecond.py`): keep reading raw archives. The current converter
  drops capture/receipt time — it sets `ts_init` to the source event time
  (duplicating `ts_event`) instead of `capture_time` — so the catalog cannot
  support this measurement today. This is the documented
  carve-out; the raw read stays until issue #677 fixes the converter to write
  `ts_init = capture_time`. Once the catalog provides `ts_init = capture_time`
  for receive-offset measurement (the fix delivered by #677), even this read
  moves to the catalog and the raw-archive path is retired.
  Note: `availability_time` (the point-in-time leakage field) has no NT slot
  and remains a permanently bolt-tracked field; #677 does not provide it.

**Migration is incremental:** lift one analysis sub-stage at a time, verify the
output matches the raw-path baseline before removing the raw read. The
carve-out boundary is `ts_init` fidelity, not script identity — a script may
mix catalog reads (strategy fidelity) and raw reads (latency) during the
transition, as long as the split is explicit in the script's docstring.
