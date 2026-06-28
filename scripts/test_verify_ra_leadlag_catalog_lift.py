#!/usr/bin/env python3
"""Self-tests for the RA-007 lead-lag catalog-lift verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_leadlag_catalog_lift.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_leadlag_catalog_lift", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def rust_reader_source(*, comment_only: bool = False, omit_trades: bool = False) -> str:
    if comment_only:
        return '''
const FAKE: &str = "LeadLagCatalogReadConfig query_catalog_typed::<OrderBookDelta> \
query_catalog_typed::<TradeTick> OrderBook::deltas_to_quotes CatalogQuerySpec";
// pub struct LeadLagCatalogReadConfig
// query_catalog_typed::<OrderBookDelta>
// query_catalog_typed::<TradeTick>
// OrderBook::deltas_to_quotes
fn unrelated() {}
'''
    trades = "" if omit_trades else "let trades = query_catalog_typed::<TradeTick>(&trade_spec)?;\n"
    return f"""
use backtesting_vertical_slice::research_reader::{{CatalogQuerySpec, query_catalog_typed}};
use nautilus_model::data::{{OrderBookDelta, TradeTick}};
use nautilus_model::orderbook::OrderBook;

pub struct LeadLagCatalogReadConfig {{
    pub catalog_uri: String,
    pub storage_options: Option<AHashMap<String, String>>,
    pub instrument_ids: Vec<String>,
    pub start: Option<UnixNanos>,
    pub end: Option<UnixNanos>,
    pub where_clause: Option<String>,
    pub files: Option<Vec<String>>,
    pub optimize_file_loading: bool,
    pub book_type: String,
    pub clock: String,
    pub instrument_aliases: Vec<LeadLagInstrumentAlias>,
}}

pub struct LeadLagInstrumentAlias {{
    pub instrument_id: String,
    pub asset_id: String,
}}

pub fn read_leadlag_top_of_book_from_catalog(config: &LeadLagCatalogReadConfig) -> Result<Vec<LeadLagTopOfBookRow>> {{
    let spec = CatalogQuerySpec {{
        catalog_uri: config.catalog_uri.clone(),
        storage_options: config.storage_options.clone(),
        instrument_ids: Some(config.instrument_ids.clone()),
        start: config.start,
        end: config.end,
        where_clause: config.where_clause.clone(),
        files: config.files.clone(),
        optimize_file_loading: config.optimize_file_loading,
    }};
    let deltas = query_catalog_typed::<OrderBookDelta>(&spec)?;
    let quotes = OrderBook::deltas_to_quotes(book_type, &deltas);
    Ok(Vec::new())
}}

pub fn read_leadlag_trades_from_catalog(config: &LeadLagCatalogReadConfig) -> Result<Vec<LeadLagTradeRow>> {{
    let trade_spec = CatalogQuerySpec {{
        catalog_uri: config.catalog_uri.clone(),
        storage_options: config.storage_options.clone(),
        instrument_ids: Some(config.instrument_ids.clone()),
        start: config.start,
        end: config.end,
        where_clause: config.where_clause.clone(),
        files: config.files.clone(),
        optimize_file_loading: config.optimize_file_loading,
    }};
    {trades}
    Ok(Vec::new())
}}
"""


def python_session_source(*, raw_only: bool = False) -> str:
    if raw_only:
        return """
import argparse
import subprocess

def cmd_extract_pm(args):
    subprocess.run(["aws", "s3", "cp", "raw", "out"])
"""
    return '''
"""Lead-lag lane.

Strategy-fidelity Polymarket reads use the NT catalog. Raw fallback is only for
receive-offset latency work until #677 writes `ts_init = capture_time`.
"""
import argparse
import subprocess
import tomllib

def load_pm_catalog_extract_config(path):
    with open(path, "rb") as handle:
        return tomllib.load(handle)

def run_leadlag_catalog_extract(config, kind, output):
    return subprocess.run([config["reader_bin"], "--kind", kind, "--output", str(output)], check=True)

def write_catalog_extract_frames(payload):
    return payload

def cmd_extract_pm_catalog(args):
    config = load_pm_catalog_extract_config(args.catalog_config)
    run_leadlag_catalog_extract(config, "tob", args.workdir)
    run_leadlag_catalog_extract(config, "trades", args.workdir)

def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers()
    p_pm_catalog = sub.add_parser("extract-pm-catalog")
    p_pm_catalog.add_argument("--catalog-config", required=True)
    p_pm_catalog.set_defaults(func=cmd_extract_pm_catalog)
'''


def write_common(root: Path, *, rust: str | None = None, session: str | None = None) -> None:
    write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "pub mod leadlag_catalog_reader;\n")
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/leadlag_catalog_reader.rs",
        rust if rust is not None else rust_reader_source(),
    )
    write_file(
        root,
        "scripts/leadlag_session4.py",
        session if session is not None else python_session_source(),
    )
    write_file(
        root,
        "scripts/leadlag_clock_alignment.py",
        '"""Raw receive-offset fallback until #677 writes `ts_init = capture_time`."""\n',
    )
    write_file(
        root,
        "scripts/leadlag_subsecond.py",
        '"""Raw receive-offset fallback until #677 writes `ts_init = capture_time`."""\n',
    )
    write_file(
        root,
        "justfile",
        """source-fence-static:
    python3 scripts/test_verify_ra_leadlag_catalog_lift.py
    python3 scripts/verify_ra_leadlag_catalog_lift.py
""",
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
    )


def test_compliant_catalog_lift_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root)

        assert verifier.scan_root(root) == []


def test_rust_comments_and_strings_do_not_satisfy_reader_shape() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, rust=rust_reader_source(comment_only=True))

        findings = verifier.scan_root(root)

    assert any("LeadLagCatalogReadConfig" in finding for finding in findings)
    assert any("OrderBookDelta" in finding for finding in findings)


def test_catalog_reader_must_query_trades_and_books() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, rust=rust_reader_source(omit_trades=True))

        findings = verifier.scan_root(root)

    assert any("TradeTick" in finding for finding in findings)


def test_raw_only_python_script_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, session=python_session_source(raw_only=True))

        findings = verifier.scan_root(root)

    assert any("extract-pm-catalog" in finding for finding in findings)
    assert any("raw fallback sunset" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "leadlag_catalog_reader.rs is missing" in result.stderr


def main() -> int:
    tests = [
        test_compliant_catalog_lift_passes,
        test_rust_comments_and_strings_do_not_satisfy_reader_shape,
        test_catalog_reader_must_query_trades_and_books,
        test_raw_only_python_script_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA lead-lag catalog-lift verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
