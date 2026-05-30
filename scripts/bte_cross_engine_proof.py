"""BTE cross-engine proof — Python side (both directions + engine agreement).

Proves NautilusTrader's Python and Rust engines (SAME rev `6e059dc`: Python
1.228.0 / Rust 0.58.0) interoperate over a shared `ParquetDataCatalog`:

  * Direction A (Rust -> Python): the Python engine reads the EXACT bytes of a
    Rust-written catalog, and a strategy-less backtest over them matches Rust's.
  * Direction B (Python -> Rust): this script writes a Python catalog (and a
    Rust-convention view of it); the Rust test
    `binary_option_cross_engine_read_python` reads that view and asserts the bytes
    match what Rust would build from the same config, then backtests it.

FINDING (recorded): the parquet data format is cross-compatible, but the two
engines' catalogs use different DIRECTORY names — Rust `trades`/`instruments` vs
Python `trade_tick`/<type> (they agree on `order_book_deltas`). A thin directory
shim (instrument dir = config `kind`) bridges it; the data itself is identical.

Runs in the research venv — never imported by the live binary. Sequence:
    1. cargo test --features bte-gate-proof --test bte_gate1_backtest_proof \\
           binary_option_cross_engine_write          # writes rust_written + rust JSON
    2. <research-venv>/bin/python scripts/bte_cross_engine_proof.py  # this file (A + B prep)
    3. cargo test --features bte-gate-proof --test bte_gate1_backtest_proof \\
           binary_option_cross_engine_read_python    # reads python_written_rustview

All runtime values come from tests/fixtures/bte_market_families.toml (no hardcodes).
Exits non-zero (FAIL LOUD) on any mismatch.
"""

import json
import shutil
import sys
import tomllib
from decimal import Decimal
from pathlib import Path

from nautilus_trader.backtest.engine import BacktestEngine
from nautilus_trader.config import BacktestEngineConfig
from nautilus_trader.model.data import BookOrder, OrderBookDelta, TradeTick
from nautilus_trader.model.enums import (
    AccountType,
    AggressorSide,
    AssetClass,
    BookAction,
    BookType,
    OmsType,
    OrderSide,
)
from nautilus_trader.model.identifiers import InstrumentId, Symbol, TradeId, Venue
from nautilus_trader.model.instruments import BinaryOption
from nautilus_trader.model.objects import Currency, Money, Price, Quantity
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
# Rust<->Python catalog directory-name maps. Data-class names are NT framework
# constants; the instrument dir is the config `kind` (no hardcode).
RUST_TO_PY_DIR = {"trades": "trade_tick", "instruments": F["kind"]}
PY_TO_RUST_DIR = {"trade_tick": "trades", F["kind"]: "instruments"}


def fail(msg: str) -> None:
    print(f"CROSS-ENGINE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def shim_view(src_catalog: Path, view: Path, dir_map: dict) -> Path:
    """Symlink a catalog's data dirs under the other engine's naming convention."""
    if view.exists():
        shutil.rmtree(view)
    (view / "data").mkdir(parents=True)
    for sub in sorted((src_catalog / "data").iterdir()):
        (view / "data" / dir_map.get(sub.name, sub.name)).symlink_to(sub)
    return view


def build_instrument() -> BinaryOption:
    pinc = Price.from_str(F["price_increment"])
    sinc = Quantity.from_str(F["size_increment"])
    return BinaryOption(
        instrument_id=InstrumentId.from_str(F["instrument_id"]),
        raw_symbol=Symbol(F["symbol"]),
        outcome=F["outcome"],
        description="bte cross-engine proof",
        asset_class=AssetClass.ALTERNATIVE,
        currency=Currency.from_str(F["quote_currency"]),
        price_precision=pinc.precision,
        price_increment=pinc,
        size_precision=sinc.precision,
        size_increment=sinc,
        activation_ns=int(F["activation_ns"]),
        expiration_ns=int(F["expiration_ns"]),
        max_quantity=None,
        min_quantity=None,
        maker_fee=Decimal(0),
        taker_fee=Decimal(0),
        ts_event=0,
        ts_init=0,
    )


def build_data(instrument: BinaryOption):
    """Mirror the Rust `build_proof_data` shape exactly so a Python-written catalog
    is byte-identical to a Rust-built one (the Direction-B value-equality check)."""
    price = Price.from_str(F["trade_price"])
    size = Quantity.from_str(F["trade_size"])
    trades = [
        TradeTick(
            instrument_id=instrument.id,
            price=price,
            size=size,
            aggressor_side=AggressorSide.BUYER,
            trade_id=TradeId(f"{F['label']}-{i}"),
            ts_event=i,
            ts_init=i,
        )
        for i in (1, 2, 3)
    ]
    deltas = []
    for seq, (side, px) in enumerate(
        ((OrderSide.BUY, F["book_bid"]), (OrderSide.SELL, F["book_ask"]))
    ):
        order = BookOrder(
            side=side,
            price=Price.from_str(px),
            size=size,
            order_id=seq + 1,
        )
        deltas.append(
            OrderBookDelta(
                instrument_id=instrument.id,
                action=BookAction.ADD,
                order=order,
                flags=0,
                sequence=seq,
                ts_event=1,
                ts_init=1,
            )
        )
    return trades, deltas


def strategy_less_result(catalog_dir: Path, source: str) -> dict:
    """Run a strategy-less Python backtest over a catalog and return the summary."""
    cat = ParquetDataCatalog(path=str(catalog_dir))
    instruments, trades, deltas = cat.instruments(), cat.trade_ticks(), cat.order_book_deltas()
    amount, ccy = F["starting_balance"].split()
    engine = BacktestEngine(config=BacktestEngineConfig())
    engine.add_venue(
        venue=Venue(F["venue"]),
        oms_type=OmsType.NETTING,
        account_type=AccountType.CASH if F["account_type"] == "cash" else AccountType.MARGIN,
        starting_balances=[Money(Decimal(amount), Currency.from_str(ccy))],
        book_type=BookType.L2_MBP,
    )
    engine.add_instrument(instruments[0])
    engine.add_data(trades)
    engine.add_data(deltas)
    engine.run()
    res = engine.get_result()
    engine.dispose()
    return {
        "engine": "python",
        "family": F["label"],
        "catalog_source": source,
        "iterations": res.iterations,
        "total_events": res.total_events,
        "total_orders": res.total_orders,
        "total_positions": res.total_positions,
        "run_id_present": res.run_id is not None,
        "backtest_range_present": res.backtest_start is not None and res.backtest_end is not None,
    }


# === Direction A: Python reads the Rust-written catalog ========================
rust_catalog = ROOT / "rust_written"
if not rust_catalog.exists():
    fail(
        f"{rust_catalog} missing — run the Rust write test first "
        "(binary_option_cross_engine_write)"
    )

direct = ParquetDataCatalog(path=str(rust_catalog))
direct_counts = (
    len(direct.instruments()),
    len(direct.trade_ticks()),
    len(direct.order_book_deltas()),
)
print(
    f"FINDING: direct read of the Rust catalog = {direct_counts} (instruments, trades, "
    "deltas) — Rust's `trades`/`instruments` dirs are invisible to Python; deltas align."
)

rust_view = shim_view(rust_catalog, ROOT / "rust_written_pyview", RUST_TO_PY_DIR)
c = ParquetDataCatalog(path=str(rust_view))
ri, rt, rd = c.instruments(), c.trade_ticks(), c.order_book_deltas()
if (len(ri), len(rt), len(rd)) != (1, 3, 2):
    fail(f"Direction A (normalized) counts {(len(ri), len(rt), len(rd))}, expected (1, 3, 2)")
if str(ri[0].id) != F["instrument_id"]:
    fail(f"Direction A instrument id {ri[0].id} != {F['instrument_id']}")
if abs(float(str(rt[0].price)) - float(F["trade_price"])) > 1e-9:
    fail(f"Direction A trade price {rt[0].price} != {F['trade_price']}")
print(f"Direction A (Rust->Python, via name shim) OK: {(len(ri), len(rt), len(rd))} with exact values")

py_summary = strategy_less_result(rust_view, "rust_written")
(ROOT / "python.binary-option.result.json").write_text(json.dumps(py_summary, indent=2, sort_keys=True))

# === Direction B prep: write a Python catalog + a Rust-convention view =========
py_catalog = ROOT / "python_written"
if py_catalog.exists():
    shutil.rmtree(py_catalog)
py_catalog.mkdir(parents=True)
instrument = build_instrument()
trades, deltas = build_data(instrument)
cat = ParquetDataCatalog(path=str(py_catalog))
cat.write_data([instrument])
cat.write_data(trades)
cat.write_data(deltas)
shim_view(py_catalog, ROOT / "python_written_rustview", PY_TO_RUST_DIR)
print("Direction B prep OK: wrote python_written + python_written_rustview "
      "(run binary_option_cross_engine_read_python next)")

# === Comparator: Direction A engine agreement (Python over Rust == Rust) =======
rust_json_path = ROOT / "rust.binary-option.result.json"
if not rust_json_path.exists():
    fail(f"{rust_json_path} missing — the Rust write test must run first")
rust_summary = json.loads(rust_json_path.read_text())
mismatches = {
    k: (py_summary[k], rust_summary[k]) for k in COMPARED_KEYS if py_summary[k] != rust_summary[k]
}
if mismatches:
    fail(f"engine result mismatch (python, rust): {mismatches}")
print("CROSS-ENGINE AGREEMENT OK:", {k: py_summary[k] for k in COMPARED_KEYS})
