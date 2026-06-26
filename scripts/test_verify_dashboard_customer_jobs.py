#!/usr/bin/env python3
"""Self-tests for the dashboard customer-jobs verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_dashboard_customer_jobs.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_dashboard_customer_jobs", SCRIPT)
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


def dashboard_plan_text(*, omit_controlled_action_id: bool = False) -> str:
    ids = [
        "trade_monitor",
        "trade_investigation",
        "annotation_review_notes",
        *([] if omit_controlled_action_id else ["controlled_action_workflow"]),
    ]
    return f"""
<!-- dashboard-customer-job-ids: {", ".join(ids)} -->

The dashboard plan can describe the customer jobs in ordinary prose.
"""


def dashboard_spec_text() -> str:
    return """
<!-- dashboard-capability-boundary-ids: no_trading_runtime_credential_fund_order_mutation -->

The dashboard capability boundary can be worded freely.
"""


def dashboard_tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return f"""
- [{mark}] DASH-001 Define dashboard customer jobs and capability classes before product selection: trade monitor, trade investigation, optional annotation/review notes, and controlled action workflow; keep trading/runtime/credential/fund/order mutation outside this package unless separately approved.
- [ ] DASH-002 Define dashboard field-source matrix.
"""


def write_complete_fixture(root: Path) -> None:
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/plan.md", dashboard_plan_text())
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/spec.md", dashboard_spec_text())
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", dashboard_tasks_text())


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_customer_jobs_pass_when_defined_and_checked() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_controlled_action_marker_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/plan.md",
            dashboard_plan_text(omit_controlled_action_id=True),
        )

        findings = verifier.scan_root(root)

    assert any("controlled_action_workflow" in finding for finding in findings)


def test_unchecked_task_still_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md",
            dashboard_tasks_text(checked=False),
        )

        assert verifier.scan_root(root) == []


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/plan.md",
            dashboard_plan_text(omit_controlled_action_id=True),
        )
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/spec.md", dashboard_spec_text())
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", dashboard_tasks_text())

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "controlled_action_workflow" in result.stderr


def main() -> int:
    tests = [
        test_customer_jobs_pass_when_defined_and_checked,
        test_missing_controlled_action_marker_is_a_finding,
        test_unchecked_task_still_passes,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: dashboard customer-jobs verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
