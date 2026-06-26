#!/usr/bin/env python3
"""Self-tests for the 023 status/legend registry verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_023_status_legend_registry.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_023_status_legend_registry", SCRIPT)
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


def registry_text(
    *,
    omit_value: str | None = None,
    include_anchor: bool = True,
    heading: str = "## Cross-Project Status And Legend Registry",
) -> str:
    rows = [
        ("L2_REPLAY", "fidelity_class", "L2 replay", "Historical L2/L3 order-book replay supports proven execution-quality claims.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("TRADE_BAR_REPLAY", "fidelity_class", "Trade/bar replay", "Trades, fills, candles, or bars support price or alpha research with limits.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("SIGNAL_ONLY", "fidelity_class", "Signal only", "Data can inform features, provenance, or dashboards but not execution-quality backtests.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("FORWARD_CAPTURE_PENDING", "fidelity_class", "Forward capture pending", "No sufficient history exists; future replay waits for accumulated capture.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("SOURCE_PROVEN", "proof_status", "Source proven", "Evidence row is accepted as source-proven.", "reference/data-model.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("USER_ASSUMPTION", "proof_status", "User assumption", "User supplied assumption, not implementation proof.", "reference/data-model.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("GAP", "proof_status", "Gap", "Known missing proof or implementation surface.", "reference/data-model.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("DECISION_NEEDED", "proof_status", "Decision needed", "Owner decision is required before implementation can claim completion.", "reference/data-model.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("authoritative", "source_role", "Authoritative", "NT reports, events, snapshots, or accepted owner source.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("derived", "source_role", "Derived", "Read model computed from an accepted authoritative source.", "reference/contracts.md", "Research Analytics", "Dashboard"),
        ("exploratory", "source_role", "Exploratory", "Non-trading-truth research or outlook field.", "reference/contracts.md", "Research Analytics", "Dashboard"),
        ("current", "data_status", "Current", "Source is within the configured freshness threshold.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("stale", "data_status", "Stale", "Source exists but exceeds the configured freshness threshold.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("partial", "data_status", "Partial", "Source exists but has incomplete coverage.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("unavailable", "data_status", "Unavailable", "Required source is missing or blocked.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("excluded", "data_status", "Excluded", "Field is intentionally outside the accepted scope.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("missing_source", "gap_reason", "Missing source", "Required upstream source is absent.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("upstream_blocked", "gap_reason", "Upstream blocked", "Upstream issue or proof gate blocks the field.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("scope_excluded", "gap_reason", "Scope excluded", "Owner intentionally excluded the field or claim.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("active", "lifecycle_state", "Active", "Artifact remains in the hot/queryable lifecycle profile.", "reference/contracts.md", "Artifact producer", "Research Analytics, Dashboard"),
        ("inactive", "lifecycle_state", "Inactive", "Artifact is retained but no longer in the active profile.", "reference/contracts.md", "Artifact producer", "Research Analytics, Dashboard"),
        ("normal", "run_purpose", "Normal", "Latest accepted proof is required for normal runs.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("reproduction", "run_purpose", "Reproduction", "Historical rerun may pin older proof with an allowed reason.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("audit", "run_purpose", "Audit", "Investigation run may pin older proof with detail.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("regression", "run_purpose", "Regression", "Mechanical regression run may pin older proof.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("migration", "run_purpose", "Migration", "Migration comparison run may pin older proof.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("raw", "artifact_kind", "Raw", "Canonical raw evidence payload kind.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("nt-catalog", "artifact_kind", "NT catalog", "Canonical NT ParquetDataCatalog projection kind.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("source-proofs", "artifact_kind", "Source proof", "SourceProofReport artifact kind.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("backtests", "artifact_kind", "Backtest", "Backtest output artifact kind.", "reference/contracts.md", "Backtesting Engine", "Research Analytics, Dashboard"),
        ("artifact-index", "artifact_kind", "Artifact index", "Artifact Index event, snapshot, or pointer kind.", "reference/contracts.md", "Artifact producer", "Research Analytics, Dashboard"),
        ("research-analytics", "artifact_kind", "Research analytics", "Research Analytics-owned derived artifact kind.", "reference/contracts.md", "Research Analytics", "Dashboard"),
        ("accepted", "proof_status", "Accepted", "Proof or review artifact has owner acceptance.", "reference/contracts.md", "Owning vertical", "Research Analytics, Dashboard"),
        ("superseded", "proof_status", "Superseded", "A newer proof version replaces this record without mutation.", "reference/contracts.md", "Owning vertical", "Research Analytics, Dashboard"),
        ("blocked", "proof_status", "Blocked", "Required proof or upstream dependency blocks the claim.", "reference/contracts.md", "Owning vertical", "Research Analytics, Dashboard"),
        ("mechanical_blocker", "warning_label", "Mechanical blocker", "Mechanical condition blocks execution-quality interpretation.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("claim_limit", "warning_label", "Claim limit", "Explicit limit on how results may be interpreted.", "reference/contracts.md", "Backtesting Engine, Research Analytics", "Dashboard"),
        ("selected_existing_product", "product_gate_outcome", "Existing product selected", "Existing product passed product-fit gate.", "reference/contracts.md", "Dashboard", "Dashboard"),
        ("custom_ui_requires_exception", "product_gate_outcome", "Custom UI exception", "Custom UI is allowed only after all product candidates are rejected with evidence.", "reference/contracts.md", "Dashboard", "Dashboard"),
    ]
    body = "\n".join(
        f"| `{key}` | {concept} | {label} | {legend} | {owner} | {setters} | {displayers} |"
        for key, concept, label, legend, owner, setters, displayers in rows
        if key != omit_value
    )
    anchor = '<a id="023-status-legend-registry"></a>\n\n' if include_anchor else ""
    return f"""
{anchor}{heading}

| Registry key | Concept | Display label | Legend meaning | Owner/source of truth | May set | May display |
|---|---|---|---|---|---|---|
{body}
"""


def tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return f"""
- [{mark}] ROOT-009 Create a cross-project status/label/legend registry before
  implementation or UI work.
"""


def justfile_text(*, wired: bool = True) -> str:
    commands = (
        "    python3 scripts/test_verify_023_status_legend_registry.py\n"
        "    python3 scripts/verify_023_status_legend_registry.py\n"
        if wired
        else ""
    )
    return f"source-fence-static:\n{commands}"


def write_complete_fixture(root: Path) -> None:
    write_file(root, "specs/023-nt-research-analytics-platform/reference/contracts.md", registry_text())
    write_file(root, "specs/023-nt-research-analytics-platform/tasks.md", tasks_text())
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


def test_complete_registry_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_required_value_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "specs/023-nt-research-analytics-platform/reference/contracts.md", registry_text(omit_value="stale"))

        findings = verifier.scan_root(root)

    assert any("stale" in finding for finding in findings)


def test_registry_heading_reword_still_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/contracts.md",
            registry_text(heading="## Registry Title Can Change"),
        )

        assert verifier.scan_root(root) == []


def test_missing_registry_stable_section_id_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/contracts.md",
            registry_text(include_anchor=False),
        )

        findings = verifier.scan_root(root)

    assert any("023-status-legend-registry" in finding for finding in findings)


def test_required_value_mentioned_only_in_prose_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        contracts = (
            registry_text(omit_value="stale")
            + "\nThe stale key is mentioned here only as prose, not as a registry row.\n"
            + "| `extra_placeholder` | proof_status | Extra | Extra placeholder row keeps the row count high. | reference/contracts.md | Backtesting Engine | Dashboard |\n"
        )
        write_file(root, "specs/023-nt-research-analytics-platform/reference/contracts.md", contracts)

        findings = verifier.scan_root(root)

    assert any("stale" in finding for finding in findings)


def test_unchecked_root_task_still_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "specs/023-nt-research-analytics-platform/tasks.md", tasks_text(checked=False))

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
        write_file(root, "specs/023-nt-research-analytics-platform/reference/contracts.md", registry_text(omit_value="GAP"))
        write_file(root, "specs/023-nt-research-analytics-platform/tasks.md", tasks_text())
        write_file(root, "justfile", justfile_text())

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "GAP" in result.stderr


def main() -> int:
    tests = [
        test_complete_registry_passes,
        test_missing_required_value_is_a_finding,
        test_registry_heading_reword_still_passes,
        test_missing_registry_stable_section_id_is_a_finding,
        test_required_value_mentioned_only_in_prose_is_a_finding,
        test_unchecked_root_task_still_passes,
        test_missing_source_fence_wiring_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: 023 status/legend registry verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
