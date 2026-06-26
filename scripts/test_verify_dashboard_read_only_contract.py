#!/usr/bin/env python3
"""Self-tests for the dashboard read-only contract verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_dashboard_read_only_contract.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_dashboard_read_only_contract", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def code_text(*, omit: str | None = None) -> str:
    snippets = list(load_verifier().CODE_SNIPPETS)
    if omit is not None:
        snippets.remove(omit)
    return "\n".join(snippets)


def test_text() -> str:
    return "\n".join(load_verifier().TEST_SNIPPETS)


def plan_text() -> str:
    return """
<!-- dashboard-read-only-contract-ids: product_gate_metabase, source_binding_key, no_mutation_controls, artifact_root_boundary -->

The read-only dashboard plan prose may be reworded.
"""


def tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return "\n".join(f"- [{mark}] {task_id} task" for task_id in load_verifier().CHECKED_TASKS)


def justfile_text(*, wired: bool = True) -> str:
    commands = (
        "    python3 scripts/test_verify_dashboard_read_only_contract.py\n"
        "    python3 scripts/verify_dashboard_read_only_contract.py\n"
        if wired
        else ""
    )
    return f"source-fence-static:\n{commands}"


def write_complete_fixture(root: Path) -> None:
    write_file(root, "crates/backtesting-vertical-slice/src/dashboard_contract.rs", code_text())
    write_file(root, "crates/backtesting-vertical-slice/tests/dashboard_contract.rs", test_text())
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/plan.md", plan_text())
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", tasks_text())
    write_file(root, "justfile", justfile_text())


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_complete_fixture_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_contract_code_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "crates/backtesting-vertical-slice/src/dashboard_contract.rs", code_text(omit="PortfolioSnapshot"))

        findings = verifier.scan_root(root)

    assert any("PortfolioSnapshot" in finding for finding in findings)


def test_missing_plan_marker_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/plan.md", "")

        findings = verifier.scan_root(root)

    assert any("dashboard-read-only-contract-ids" in finding for finding in findings)


def test_unchecked_tasks_still_pass() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", tasks_text(checked=False))

        assert verifier.scan_root(root) == []


def test_missing_source_fence_wiring_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "justfile", justfile_text(wired=False))

        findings = verifier.scan_root(root)

    assert any("source-fence-static must run" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "crates/backtesting-vertical-slice/tests/dashboard_contract.rs", "")

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "dashboard_read_model_accepts" in result.stderr


def main() -> int:
    tests = [
        test_complete_fixture_passes,
        test_missing_contract_code_is_a_finding,
        test_missing_plan_marker_is_a_finding,
        test_unchecked_tasks_still_pass,
        test_missing_source_fence_wiring_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: dashboard read-only contract verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
