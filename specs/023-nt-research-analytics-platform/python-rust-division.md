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

1. **Canonical-catalog writer — Direction A + agreement PROVEN (2026-05-31).** The
   cross-engine test (`scripts/bte_cross_engine_proof.py` + the Rust
   `binary_option_cross_engine_write` test) shows the Python engine reads the *exact
   bytes* of a Rust-written catalog, and a strategy-less backtest over those shared
   bytes yields identical counters in both engines (`iterations 5, total_events 0,
   total_orders 0, total_positions 0`, run-id + range present).
   **Finding:** the parquet *data format* is cross-compatible, but the two engines'
   `ParquetDataCatalog` use different *directory names* — Rust `trades`/`instruments`
   vs Python `trade_tick`/`<type>` (they agree on `order_book_deltas`). A direct read
   misses trades + instruments; a thin directory-name shim (instrument dir = config
   `kind`) bridges it fully — no deep incompatibility.
   **Implication:** Rust can be the canonical writer and the Python research lane
   reads it through that small name-normalization shim.
   **Still pending:** Direction B (Python-written catalog read by Rust) and a
   production-grade shim. So **partially resolved.**
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

### Build-from-rev — PROVEN (2026-05-31)

**Decision: B** — keep Rust pinned at 0.58.0 (production untouched); build Python NT
from the *same* rev. Done: `pip install "nautilus_trader @ git+…@6e059dc"` into an
isolated research venv (outside the repo, Python 3.14) installs **`nautilus_trader
1.228.0`** — same source as the Rust crates (0.58.0); `BacktestEngine` /
`BacktestNode` / `ParquetDataCatalog` / `TradeTick` / `OrderBookDelta` all import.

One environment fix was needed: NT's `build.py` forces the `stable` rustup channel,
which was a stale **1.94.0** < the crates' **1.95.0** MSRV → `rustup update stable`
(now 1.96.0). **Bolt's pinned `1.95.0` and the rustup default were untouched** — the
production Rust build was not compromised. So **OPEN #2 is RESOLVED via B**: the two
engines are now on identical source. (Implication for the sync script: ensure
`stable` ≥ the pinned rev's required rustc before building.)

## Method

Grounded by direct repo reads (2026-05-30) of `CLAUDE.md`, the `reference/` contracts,
the `1-/2-` vertical specs, and `gate-proofs.md`; the version facts (OPEN #2/#3) by
`pip index versions nautilus_trader` + `python3 --version` on the dev host. Nothing
here rests on assertion alone.
