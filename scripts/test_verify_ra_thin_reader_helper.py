#!/usr/bin/env python3
"""Self-tests for the Research Analytics thin reader helper verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_thin_reader_helper.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_thin_reader_helper", SCRIPT)
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


def helper_source(*, duplicate_engine: bool = False, omit_session: bool = False) -> str:
    duplicate = "let _node = BacktestNode::new;\n" if duplicate_engine else ""
    session = (
        ""
        if omit_session
        else (
            "let mut session = DataBackendSession::new(spec.chunk_size);\n"
            "    let _ = session.collect_query_batches(&spec.table_name, &spec.file_path, spec.sql.as_deref());\n"
        )
    )
    return f"""
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_persistence::backend::session::DataBackendSession;
use ahash::AHashMap;

pub struct CatalogQuerySpec {{
    pub catalog_uri: String,
    pub storage_options: Option<AHashMap<String, String>>,
}}
pub struct SqlBatchQuerySpec {{ pub chunk_size: usize }}

pub fn query_catalog_typed<T>(spec: CatalogQuerySpec) {{
    let mut catalog = ParquetDataCatalog::from_uri(&spec.catalog_uri, spec.storage_options.clone(), None, None, None)?;
    let _ = catalog.query_typed_data::<T>(None, None, None, None, None, true);
    {duplicate}
}}

pub fn query_sql_arrow_batches(spec: SqlBatchQuerySpec) {{
    {session}
}}
"""


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_delegating_helper_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "pub mod research_reader;\n")
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/research_reader.rs",
            helper_source(),
        )

        assert verifier.scan_root(root) == []


def test_missing_helper_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "")

        findings = verifier.scan_root(root)

    assert any("research_reader.rs is missing" in finding for finding in findings)


def test_helper_must_use_data_backend_session_for_arrow_sql() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "pub mod research_reader;\n")
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/research_reader.rs",
            helper_source(omit_session=True),
        )

        findings = verifier.scan_root(root)

    assert any("DataBackendSession::new" in finding for finding in findings)
    assert any("collect_query_batches" in finding for finding in findings)


def test_helper_must_not_import_backtest_runtime() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "pub mod research_reader;\n")
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/research_reader.rs",
            helper_source(duplicate_engine=True),
        )

        findings = verifier.scan_root(root)

    assert any("must not reference BacktestNode" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "crates/backtesting-vertical-slice/src/lib.rs", "")

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "research_reader.rs is missing" in result.stderr


def main() -> int:
    tests = [
        test_delegating_helper_passes,
        test_missing_helper_is_a_finding,
        test_helper_must_use_data_backend_session_for_arrow_sql,
        test_helper_must_not_import_backtest_runtime,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA thin reader helper verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
