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
2. **Python NT version correspondence — VERIFIED 2026-05-30.** At rev `6e059dc` the
   repo declares Python package **1.228.0** (`pyproject.toml`) and Rust crates
   **0.58.0** (`Cargo.toml`) — the same commit, two numbering schemes. **But 1.228.0
   is not published on PyPI** (latest published = 1.227.0 per `pip index versions`),
   and GitHub shows `6e059dc` is a mid-development feature commit ("Improve Blockchain
   snapshot fail-closed path"), **not a tagged release**. So `pip install
   nautilus_trader` yields 1.227.0 — a *different* commit than our Rust crates; the
   two engines would not be the same source, making any Python-vs-Rust disagreement
   ambiguous. Note: production is therefore pinned to an **unreleased dev commit**.
   **DECISION OPEN** — for a trustworthy comparison, either (a) re-pin bolt-v2 to a
   tagged NT release that ships both a PyPI wheel and the Rust crates (also moves
   production onto a real release), or (b) build the Python package from the exact
   local `6e059dc` checkout (heavy build; Python-3.14 support unverified — see #3).
3. **Python runtime / wheels — VERIFIED 2026-05-31, RESOLVED.** NT 1.227.0 ships
   `cp312`/`cp313`/**`cp314`** wheels (`requires_python <3.15,>=3.12`), so NT installs
   on this host's Python 3.14. 3.14 support is **not** a blocker.

## Re-pin investigation (2026-05-31)

- bolt pins **16** NT crates at one rev in the root `Cargo.toml:28-65`; a re-pin is
  mechanically one rev value (+ `Cargo.lock` + rebuild/re-validate).
- The latest *released* wheel is **1.227.0** = commit `280ae176`, **Rust crates
  0.57.0**. Our pinned dev rev (`6e059dc`, Rust **0.58.0**) is **135 commits ahead /
  57 diverged** from it. **No released wheel exists for our current Rust 0.58.0**
  (1.228.0 is unreleased).
- Therefore "re-pin to a release" = a Rust **downgrade** 0.58.0 → 0.57.0, and the
  real cost is re-validating bolt + the gate proof against the 0.57.0 API — not the
  edit. The alternative (build Python from the exact `6e059dc`) keeps Rust 0.58.0
  and is de-risked now that 3.14 is confirmed supported.

## Method

Grounded by direct repo reads (2026-05-30) of `CLAUDE.md`, the `reference/` contracts,
the `1-/2-` vertical specs, and `gate-proofs.md`; the version facts (OPEN #2/#3) by
`pip index versions nautilus_trader` + `python3 --version` on the dev host. Nothing
here rests on assertion alone.
