"""BTE cross-engine proof — Python side (Direction A + engine agreement).

Proves that NautilusTrader's *Python* engine reads a catalog written by NT's
*Rust* engine (Direction A), and that a strategy-less backtest over that SAME
on-disk catalog yields the SAME result counters in both engines.

Runs in the research venv (NT Python 1.228.0, built from the same rev `6e059dc`
as the Rust crates 0.58.0) — never imported by the live binary. Sequence:

    1. cargo test --features bte-gate-proof --test bte_gate1_backtest_proof \\
           binary_option_cross_engine_write      # writes rust_written + rust JSON
    2. <research-venv>/bin/python scripts/bte_cross_engine_proof.py   # this file

FINDING (recorded by this proof): the Rust and Python `ParquetDataCatalog` write
data in a cross-compatible parquet format, but use *different directory names* for
trades (`trades` vs `trade_tick`) and instruments (`instruments` vs the type name,
e.g. `binary_option`); they agree on `order_book_deltas`. The data is therefore
interchangeable through a thin directory-name shim, which this script builds (the
instrument dir name is the config `kind`, so no hardcode).

All runtime values come from tests/fixtures/bte_market_families.toml. Exits
non-zero (FAIL LOUD) on any mismatch.
"""

import json
import shutil
import sys
import tomllib
from decimal import Decimal
from pathlib import Path

from nautilus_trader.backtest.engine import BacktestEngine
from nautilus_trader.config import BacktestEngineConfig
from nautilus_trader.model.enums import AccountType, BookType, OmsType
from nautilus_trader.model.identifiers import Venue
from nautilus_trader.model.objects import Currency, Money
from nautilus_trader.persistence.catalog import ParquetDataCatalog

REPO = Path(__file__).resolve().parents[1]
CFG = tomllib.loads((REPO / "tests/fixtures/bte_market_families.toml").read_text())
ROOT = REPO / CFG["cross_engine_root"]
F = next(f for f in CFG["family"] if f["label"] == "binary-option")

COMPARED_KEYS = (
    "iterations",
    "total_events",
    "total_orders",
    "total_positions",
    "run_id_present",
    "backtest_range_present",
)


def fail(msg: str) -> None:
    print(f"CROSS-ENGINE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def catalog_counts(path: Path) -> tuple[int, int, int]:
    c = ParquetDataCatalog(path=str(path))
    return len(c.instruments()), len(c.trade_ticks()), len(c.order_book_deltas())


# Rust -> Python directory-name map. The data-class names are NT framework
# constants; the instrument dir is the config `kind` (no hardcode).
RUST_TO_PY_DIR = {"trades": "trade_tick", "instruments": F["kind"]}


def normalized_view(rust_catalog: Path, view: Path) -> Path:
    """Symlink the Rust catalog's data dirs under Python's naming convention."""
    if view.exists():
        shutil.rmtree(view)
    (view / "data").mkdir(parents=True)
    for sub in sorted((rust_catalog / "data").iterdir()):
        (view / "data" / RUST_TO_PY_DIR.get(sub.name, sub.name)).symlink_to(sub)
    return view


# --- Direction A: the Python engine reads the Rust-written catalog -------------
rust_catalog = ROOT / "rust_written"
if not rust_catalog.exists():
    fail(
        f"{rust_catalog} missing — run the Rust write test first "
        "(cargo test --features bte-gate-proof --test bte_gate1_backtest_proof "
        "binary_option_cross_engine_write)"
    )

# Document the naming difference: a direct read misses trades + instruments.
direct = catalog_counts(rust_catalog)
print(f"FINDING: direct read of the Rust catalog = {direct} (instruments, trades, deltas) "
      "— Rust's `trades`/`instruments` dirs are invisible to Python's reader; deltas align.")

# Apply the thin name shim and read again — now the data is fully visible.
view = normalized_view(rust_catalog, ROOT / "rust_written_pyview")
c = ParquetDataCatalog(path=str(view))
instruments, trades, deltas = c.instruments(), c.trade_ticks(), c.order_book_deltas()
counts = (len(instruments), len(trades), len(deltas))
if counts != (1, 3, 2):
    fail(f"Rust->Python (normalized) round-trip counts {counts}, expected (1, 3, 2)")
if str(instruments[0].id) != F["instrument_id"]:
    fail(f"instrument id {instruments[0].id} != {F['instrument_id']}")
if abs(float(str(trades[0].price)) - float(F["trade_price"])) > 1e-9:
    fail(f"trade price {trades[0].price} != {F['trade_price']}")
if abs(float(str(trades[0].size)) - float(F["trade_size"])) > 1e-9:
    fail(f"trade size {trades[0].size} != {F['trade_size']}")
print(f"Direction A (Rust->Python, via name shim) OK: {counts} with exact values")

# --- Engine agreement: strategy-less Python backtest over the Rust catalog -----
amount, ccy = F["starting_balance"].split()
engine = BacktestEngine(config=BacktestEngineConfig())
engine.add_venue(
    venue=Venue(F["venue"]),
    oms_type=OmsType.NETTING,
    account_type=AccountType.CASH if F["account_type"] == "cash" else AccountType.MARGIN,
    starting_balances=[Money(Decimal(amount), Currency.from_str(ccy))],
    book_type=BookType.L2_MBP,  # L2 venue requires book data (the 2 deltas)
)
engine.add_instrument(instruments[0])
engine.add_data(trades)
engine.add_data(deltas)
engine.run()
res = engine.get_result()
engine.dispose()

py_summary = {
    "engine": "python",
    "family": F["label"],
    "catalog_source": "rust_written",
    "iterations": res.iterations,
    "total_events": res.total_events,
    "total_orders": res.total_orders,
    "total_positions": res.total_positions,
    "run_id_present": res.run_id is not None,
    "backtest_range_present": res.backtest_start is not None and res.backtest_end is not None,
}
(ROOT / "python.binary-option.result.json").write_text(
    json.dumps(py_summary, indent=2, sort_keys=True)
)

# --- Comparator: Python result must match the Rust result on the shared keys ---
rust_json_path = ROOT / "rust.binary-option.result.json"
if not rust_json_path.exists():
    fail(f"{rust_json_path} missing — the Rust write test must run first")
rust_summary = json.loads(rust_json_path.read_text())

mismatches = {
    k: (py_summary[k], rust_summary[k])
    for k in COMPARED_KEYS
    if py_summary[k] != rust_summary[k]
}
if mismatches:
    fail(f"engine result mismatch (python, rust): {mismatches}")

print("CROSS-ENGINE AGREEMENT OK:", {k: py_summary[k] for k in COMPARED_KEYS})
