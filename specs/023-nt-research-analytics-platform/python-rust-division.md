# Python / Rust runtime division

Cross-cutting architecture for the NT-first package (backtesting → research-analytics
→ dashboard). **Every claim here cites its source.** Items marked **OPEN** are not
yet decided or verified — do not treat them as settled.

## The rule (updated 2026-05-30)

- **The live trading binary is pure Rust** — a standalone Rust `LiveNode` on NT's
  Rust API, no PyO3/maturin/pip in its build or runtime (`CLAUDE.md` Rule 5).
- **Python is permitted for everything else** — research, backtest-for-research,
  data ingest/capture, analysis — as a *separate* workspace (own venv) that is
  never imported by or shipped in the live binary. The constraint is on the
  *live-trading runtime*, not the repository (`CLAUDE.md` Rule 5, updated).
- The spec already contemplates this boundary: "Python/Jupyter is research-only
  and cannot become the production trading runtime" (`2-research-analytics/spec.md`
  E-025); the backtester rejects Python *strategy* paths (`1-backtesting-engine/tasks.md`
  BTE-014; `archive/open-questions.md` OQ-008). Research *using* Python is allowed;
  a Python *strategy/trading runtime* is not.

## Ownership map

| Surface | Owner | Lake (catalog) | Source |
|---|---|---|---|
| Live trading (real money) | **Rust** — one NT `LiveNode` | does not read the historical lake; emits its own decision-evidence | `CLAUDE.md` Rule 5 |
| Capture live market data → lake | **Python** (e.g. cryptofeed) | writes (archiving, off the trading hot path) | this doc (decision) |
| Historical ingest → lake | **Python** loaders (config-driven) | writes — *see OPEN #1* | NT Python loaders; `Rule 1` (config-driven) |
| Backtest / replay (research) | **Python** `BacktestEngine` | reads | `archive/research.md`:49; E-025 |
| Backtest / replay (final/authoritative) | **Rust** `BacktestNode` | reads | `spec.md` E-001; `gate-proofs.md` Gate 1/4 |
| Research / strategy discovery | **Python** / Jupyter | reads; writes only to `research_analytics/` | E-025; `data-model.md` ArtifactIndex |
| Strategy graduation | **Python → Rust** port + result-match | — | BTE-014; OQ-008 |

## Two backtest engines (both are NT's; their APIs differ)

- NT ships both `BacktestEngine` and `BacktestNode`; docs recommend `BacktestNode`
  for the production workflow and it requires a Parquet catalog
  (`archive/research.md`:49; `reference/evidence.md` E-001).
- The Rust and Python configs are **not identical**: the Rust `BacktestEngineConfig`
  carries **no** strategies field — strategies are added imperatively — *unlike the
  Python config* (`1-backtesting-engine/gate-proofs.md`:130). This API divergence is
  exactly why a Python-vs-Rust result-agreement check matters.
- Division by purpose: **Python backtest = research/discovery** (fast iteration);
  **Rust backtest (`BacktestNode`) = final/authoritative**, mirrors production.
- Graduation path: prototype + Python-backtest → port to Rust → Rust-backtest →
  **results must match** → live. No Python strategy ever ships (BTE-014, OQ-008).

## The shared boundary: the catalog (data lake)

- One `ParquetDataCatalog`, **S3-canonical** — "local filesystem paths are cache or
  development fixtures only" (`1-backtesting-engine/plan.md`:51-52). Both lanes read it.
- Artifact kinds are fixed: `raw`, `nt_catalog`, `source_proofs`, `backtests`,
  `artifact_index`, `research_analytics` (`reference/data-model.md` ArtifactIndex).
  Python research outputs land under `research_analytics/`, **never** overwriting the
  canonical `nt_catalog/`.

## Hard invariants

1. The live binary is pure Rust and never imports Python.
2. Python research writes only to `research_analytics/`, never the canonical
   `nt_catalog/`.
3. Whatever the Rust backtest **reads**, the catalog must be Rust-consistent — which
   forces the writer question below.

## OPEN items (not decided / not verified)

1. **Canonical-catalog writer.** Rust-only vs Python-allowed for writing the
   `nt_catalog/` the Rust backtest reads. To be settled by the cross-engine test
   (write-both-directions round-trip + Python-vs-Rust result agreement). **OPEN.**
2. **Python NT version correspondence.** PyPI `nautilus_trader` is versioned **1.x**
   (latest 1.227.0); the Rust crates at the pinned rev `6e059dc` are **0.58.0**.
   It is **not established** that any PyPI release corresponds to rev `6e059dc`. A
   valid Python-vs-Rust comparison needs the *same* engine version on both sides —
   resolve before relying on the Python backtest. **OPEN / feasibility risk.**
3. **Python runtime / wheels.** The dev host has Python 3.14; NT wheel availability
   for 3.14 is unverified. **OPEN.**

## Method

Grounded by direct repo reads (2026-05-30) of `CLAUDE.md`, the `reference/` contracts,
the `1-/2-` vertical specs, and `gate-proofs.md`; the version facts (OPEN #2/#3) by
`pip index versions nautilus_trader` + `python3 --version` on the dev host. Nothing
here rests on assertion alone.
