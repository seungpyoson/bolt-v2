#!/usr/bin/env python3
"""Self-tests for the Research Analytics notebook read-only boundary."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_notebook_read_only_boundary.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_ra_notebook_read_only_boundary", SCRIPT
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
                "research/features.py": (
                    "def load_frame(con):\n"
                    "    return con.execute('SELECT * FROM catalog').fetch_arrow_table()\n"
                ),
                "scripts/leadlag_probe.py": (
                    "task.cancel()\n"
                    "con.execute('CREATE SECRET (TYPE s3, PROVIDER credential_chain)')\n"
                ),
            },
        )

        assert verifier.scan_root(root) == []


def test_live_runtime_imports_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "research/live_path.py": "\n".join(
                    [
                        "import nautilus_trader.live.node",
                        "from nautilus_trader.execution import client",
                    ]
                ),
            },
        )

        findings = verifier.scan_root(root)

    assert len(findings) == 2
    assert any("nautilus_trader.live" in finding for finding in findings)
    assert any("nautilus_trader.execution" in finding for finding in findings)


def test_production_mutation_calls_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "research/mutate.py": "\n".join(
                    [
                        "trader.submit_order(order)",
                        "client.cancel_order(order_id)",
                        "wallet.transfer('funds')",
                        "ssm.put_parameter(Name='x', Value='y')",
                    ]
                ),
            },
        )

        findings = verifier.scan_root(root)

    assert len(findings) == 4
    assert any("submit_order" in finding for finding in findings)
    assert any("cancel_order" in finding for finding in findings)
    assert any("transfer" in finding for finding in findings)
    assert any("put_parameter" in finding for finding in findings)


def test_notebook_code_cells_are_scanned_for_mutation() -> None:
    verifier = load_verifier()
    notebook = {
        "cells": [
            {"cell_type": "markdown", "source": "client.submit_order(order)"},
            {"cell_type": "code", "source": ["portfolio.withdraw(amount)\n"]},
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
    assert "withdraw" in findings[0]


def test_specs_and_docs_are_not_research_code_surfaces() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md": (
                    "Notebook code must not call submit_order.\n"
                ),
                "docs/research/note.md": "Historical note: cancel_order is forbidden.\n",
            },
        )

        assert verifier.scan_root(root) == []


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, {"research/bad.py": "client.submit_order(order)\n"})

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "submit_order" in result.stderr


def main() -> int:
    tests = [
        test_clean_research_surfaces_have_no_findings,
        test_live_runtime_imports_are_findings,
        test_production_mutation_calls_are_findings,
        test_notebook_code_cells_are_scanned_for_mutation,
        test_specs_and_docs_are_not_research_code_surfaces,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA notebook read-only boundary verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
