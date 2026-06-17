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


def dashboard_plan_text(*, omit_controlled_action: bool = False) -> str:
    controlled_action = (
        ""
        if omit_controlled_action
        else (
            "4. Controlled action workflow: request rerun, request RA review, stage config\n"
            "   review, or future high-risk actions. Trading/runtime/credential/fund/order\n"
            "   mutation remains outside this package unless separately approved.\n"
        )
    )
    return f"""
## Customer Jobs And Capability Classes

Product choice is deferred until these jobs are specified and weighted:

1. Trade monitor: ongoing trades/orders, positions, exposure, current PnL,
   venue/source binding, and freshness.
2. Trade investigation: prior trades/fills, why trade fired,
   strategy/signal/reason refs, source proof/data used, and historical PnL
   context.
3. Annotation/review notes: optional notes, tags, comments, and investigation
   status. This is least necessary and requires explicit owner/schema/audit
   rules before any write path.
{controlled_action}
"""


def dashboard_spec_text() -> str:
    return """
- Future dashboard work must classify customer jobs and write capabilities
  before product selection. Non-trading annotation/review workflow writes may be
  considered only after explicit artifact kind/schema/owner/audit rules exist.
  Trading, runtime config, credential, and funds/order mutations remain outside
  this package unless a separate future scope explicitly approves them.
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


def test_missing_controlled_action_workflow_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/plan.md",
            dashboard_plan_text(omit_controlled_action=True),
        )

        findings = verifier.scan_root(root)

    assert any("Controlled action workflow" in finding for finding in findings)


def test_unchecked_task_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md",
            dashboard_tasks_text(checked=False),
        )

        findings = verifier.scan_root(root)

    assert any("DASH-001 must be checked" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/plan.md",
            dashboard_plan_text(omit_controlled_action=True),
        )
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/spec.md", dashboard_spec_text())
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", dashboard_tasks_text())

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "Controlled action workflow" in result.stderr


def main() -> int:
    tests = [
        test_customer_jobs_pass_when_defined_and_checked,
        test_missing_controlled_action_workflow_is_a_finding,
        test_unchecked_task_is_a_finding,
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
