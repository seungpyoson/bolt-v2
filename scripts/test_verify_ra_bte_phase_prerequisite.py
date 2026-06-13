#!/usr/bin/env python3
"""Self-tests for the RA BTE phase prerequisite verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_bte_phase_prerequisite.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_bte_phase_prerequisite", SCRIPT)
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


def documented_plan_text(*, omit_binary_oracle: bool = False) -> str:
    binary_oracle = "" if omit_binary_oracle else "`binary_oracle_edge_taker` strategy and "
    return f"""
## Backtest Phase Prerequisite

The BTE runner today wires only an NT example strategy
(`HurstVpinDirectional`) over a single venue (`bybit-spot`). Bolt's
{binary_oracle}venue normalization must be wired into
the BTE engine before any Phase-3 sweep is real. Surface this prerequisite
explicitly in the backtest phase; do not hide it. NT's pyo3
`add_native_strategy` is `#[cfg(feature = "examples")]` and can only run NT
example strategies, not bolt's, so this wiring is a hard precondition, not an
optional optimization.
"""


def documented_spec_text() -> str:
    return """
Known prerequisite (do not hide): the BTE runner today registers only an NT
example strategy (`HurstVpinDirectional`) over one venue (`bybit-spot`); bolt's
`binary_oracle_edge_taker` + venue normalization must be wired into the BTE
before Phase-3 sweeps are real.
"""


def documented_tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return f"""
- [{mark}] RA-016 Document the known prerequisite: the BTE runner today registers only an NT example strategy (HurstVpinDirectional) over one venue (bybit-spot); bolt's binary_oracle_edge_taker + venue normalization must be wired into the BTE before Phase-3 sweeps produce valid results.
"""


def write_complete_fixture(root: Path) -> None:
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
        documented_plan_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md",
        documented_spec_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
        documented_tasks_text(),
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_documented_prerequisite_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_bolt_strategy_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
            documented_plan_text(omit_binary_oracle=True),
        )

        findings = verifier.scan_root(root)

    assert any("binary_oracle_edge_taker" in finding for finding in findings)


def test_unchecked_task_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
            documented_tasks_text(checked=False),
        )

        findings = verifier.scan_root(root)

    assert any("RA-016 must be checked" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
            documented_plan_text(omit_binary_oracle=True),
        )
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md",
            documented_spec_text(),
        )
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
            documented_tasks_text(),
        )

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "binary_oracle_edge_taker" in result.stderr


def main() -> int:
    tests = [
        test_documented_prerequisite_passes,
        test_missing_bolt_strategy_is_a_finding,
        test_unchecked_task_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA BTE phase prerequisite verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
