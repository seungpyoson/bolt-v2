#!/usr/bin/env python3
"""Self-tests for the dashboard field-source matrix verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_dashboard_field_source_matrix.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_dashboard_field_source_matrix", SCRIPT)
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


def dashboard_plan_text(*, omit_proof_pin_detail: bool = False) -> str:
    detail = "" if omit_proof_pin_detail else "`proof_pin_reason_detail`, "
    return f"""
## Field Source Matrix Seed

Matrix semantics come from `../reference/contracts.md`; this plan only selects
dashboard fields and source columns.

| Field group | Required source stance | Required source columns |
|---|---|---|
| Trade explanation fields | Strategy/signal/reason evidence refs and source binding from accepted upstream artifacts; never inferred by dashboard. | `source_proof_id`, `run_purpose`, `proof_pin_reason_code`, {detail}`fidelity_class`, `claim_limits`, `warning_fields`, `source_role`, `data_status`, `gap_reason` |
| Data health/freshness | Source timestamp plus configured stale threshold. | `source_role`, `data_status`, `gap_reason` |
"""


def dashboard_tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return f"""
- [x] DASH-001 Define dashboard customer jobs and capability classes.
- [{mark}] DASH-002 Define dashboard field-source matrix, including trade explanation fields, source proof id, run purpose, proof pin reason code/detail when present, fidelity class, claim limits, warning fields, source role, and data status/gap reason.
"""


def write_complete_fixture(root: Path) -> None:
    write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/plan.md", dashboard_plan_text())
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


def test_field_source_matrix_passes_when_complete_and_checked() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_proof_pin_detail_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/3-dashboard/plan.md",
            dashboard_plan_text(omit_proof_pin_detail=True),
        )

        findings = verifier.scan_root(root)

    assert any("proof_pin_reason_detail" in finding for finding in findings)


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
            dashboard_plan_text(omit_proof_pin_detail=True),
        )
        write_file(root, "specs/023-nt-research-analytics-platform/3-dashboard/tasks.md", dashboard_tasks_text())

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "proof_pin_reason_detail" in result.stderr


def main() -> int:
    tests = [
        test_field_source_matrix_passes_when_complete_and_checked,
        test_missing_proof_pin_detail_is_a_finding,
        test_unchecked_task_still_passes,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: dashboard field-source matrix verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
