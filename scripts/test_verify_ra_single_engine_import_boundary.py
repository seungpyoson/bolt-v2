#!/usr/bin/env python3
"""Self-tests for the Research Analytics single-engine import boundary."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_single_engine_import_boundary.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_ra_single_engine_import_boundary", SCRIPT
    )
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_clean_research_surfaces_have_no_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "notebooks/read_catalog.py": "from nautilus_trader.persistence.catalog import ParquetDataCatalog\n",
                "research/features.py": "def feature_row(value):\n    return value\n",
                "scripts/leadlag_probe.py": "print('research helper')\n",
            },
        )

        assert verifier.scan_root(root) == []


def test_empty_research_code_set_fails_closed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        findings = verifier.scan_root(Path(tmp))

    assert any("enforcement set is empty" in finding for finding in findings), findings


def test_python_import_forms_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "research/bad_engine.py": "\n".join(
                    [
                        "import nautilus_trader.backtest.engine",
                        "import nautilus_trader.backtest.node as nt_node",
                        "from nautilus_trader.backtest import engine",
                        "from nautilus_trader.backtest.node import BacktestNode",
                    ]
                ),
            },
        )

        findings = verifier.scan_root(root)

    assert len(findings) == 4
    assert all("research/bad_engine.py" in finding for finding in findings)
    assert any("nautilus_trader.backtest.engine" in finding for finding in findings)
    assert any("nautilus_trader.backtest.node" in finding for finding in findings)


def test_notebook_code_cells_are_scanned() -> None:
    verifier = load_verifier()
    notebook = {
        "cells": [
            {
                "cell_type": "markdown",
                "source": "nautilus_trader.backtest.engine is mentioned in prose",
            },
            {
                "cell_type": "code",
                "source": ["from nautilus_trader.backtest import node\n"],
            },
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5,
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        path = root / "notebooks" / "probe.ipynb"
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps(notebook), encoding="utf-8")

        findings = verifier.scan_root(root)

    assert len(findings) == 1
    assert "notebooks/probe.ipynb" in findings[0]
    assert "nautilus_trader.backtest.node" in findings[0]


def test_specs_and_docs_are_not_research_code_surfaces() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md": (
                    "The rule bans nautilus_trader.backtest.engine in notebooks.\n"
                ),
                "docs/research/note.md": (
                    "Historical note: nautilus_trader.backtest.node is forbidden.\n"
                ),
                "research/clean.py": "def feature_row(value):\n    return value\n",
            },
        )

        assert verifier.scan_root(root) == []


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "research/bad.py": "from nautilus_trader.backtest import engine\n",
            },
        )

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "nautilus_trader.backtest.engine" in result.stderr


def main() -> int:
    tests = [
        test_clean_research_surfaces_have_no_findings,
        test_empty_research_code_set_fails_closed,
        test_python_import_forms_are_findings,
        test_notebook_code_cells_are_scanned,
        test_specs_and_docs_are_not_research_code_surfaces,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA single-engine import-boundary verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
