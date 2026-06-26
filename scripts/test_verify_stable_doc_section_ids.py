#!/usr/bin/env python3
"""Self-tests for the stable Markdown section-id verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_stable_doc_section_ids.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_stable_doc_section_ids", SCRIPT)
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


def justfile_text(*, wired: bool = True, standalone_only: bool = False) -> str:
    commands = (
        "    python3 scripts/test_verify_stable_doc_section_ids.py\n"
        "    python3 scripts/verify_stable_doc_section_ids.py\n"
        if wired
        else ""
    )
    if standalone_only:
        return f"verify-stable-doc-section-ids:\n{commands}\nsource-fence-static-inner:\n"
    return f"source-fence-static-inner:\n{commands}"


def write_complete_fixture(
    root: Path,
    *,
    agents_link_fragment: str = "ci-operator-policy",
    ci_link_fragment: str = "agent-rust-probe-policy",
    include_agent_anchor: bool = True,
    include_ci_anchor: bool = True,
    include_registry_anchor: bool = True,
    duplicate_ci_anchor: bool = False,
    spaced_cased_anchors: bool = False,
    wired: bool = True,
) -> None:
    agent_anchor = '<A ID = "agent-rust-probe-policy" ></A>\n' if spaced_cased_anchors else '<a id="agent-rust-probe-policy"></a>\n'
    ci_anchor = "<a ID = 'ci-operator-policy' ></a>\n" if spaced_cased_anchors else '<a id="ci-operator-policy"></a>\n'
    registry_anchor = '<a Id = "023-status-legend-registry"></a>\n' if spaced_cased_anchors else '<a id="023-status-legend-registry"></a>\n'
    agent_anchor = agent_anchor if include_agent_anchor else ""
    ci_anchor = ci_anchor if include_ci_anchor else ""
    registry_anchor = registry_anchor if include_registry_anchor else ""
    if duplicate_ci_anchor:
        ci_anchor = ci_anchor + '<a id="ci-operator-policy"></a>\n'
    write_file(
        root,
        "AGENTS.md",
        f"""
## Remote Rust Verification

See [Operator Policy](docs/ci/ubicloud-cost-governance.md#{agents_link_fragment}).

{agent_anchor}## Probe Policy, Reworded Freely
""",
    )
    write_file(
        root,
        "docs/ci/ubicloud-cost-governance.md",
        f"""
{ci_anchor}## Human Session Limits

Use [Rust Probe Policy](../../AGENTS.md#{ci_link_fragment}) for probe limits.
""",
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/contracts.md",
        f"""
{registry_anchor}## Registry Heading Can Change
""",
    )
    write_file(root, "justfile", justfile_text(wired=wired))


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_complete_stable_ids_pass() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_heading_reword_does_not_break_stable_ids() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        contracts = '<a id="023-status-legend-registry"></a>\n## Completely Different Registry Title\n'
        write_file(root, "specs/023-nt-research-analytics-platform/reference/contracts.md", contracts)

        assert verifier.scan_root(root) == []


def test_anchor_spacing_and_attribute_casing_pass() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, spaced_cased_anchors=True)

        assert verifier.scan_root(root) == []


def test_duplicate_stable_section_id_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, duplicate_ci_anchor=True)

        findings = verifier.scan_root(root)

    assert any("duplicate stable section id `ci-operator-policy`" in finding for finding in findings), findings


def test_legacy_heading_slug_link_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, agents_link_fragment="operator-policy")

        findings = verifier.scan_root(root)

    assert any("operator-policy" in finding for finding in findings)


def test_missing_target_anchor_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, include_ci_anchor=False)

        findings = verifier.scan_root(root)

    assert any("ci-operator-policy" in finding for finding in findings)


def test_missing_registry_anchor_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, include_registry_anchor=False)

        findings = verifier.scan_root(root)

    assert any("023-status-legend-registry" in finding for finding in findings)


def test_missing_source_fence_wiring_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, wired=False)

        findings = verifier.scan_root(root)

    assert any("source-fence-static must run" in finding for finding in findings)


def test_standalone_recipe_does_not_satisfy_source_fence_static_inner_wiring() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(root, "justfile", justfile_text(standalone_only=True))

        findings = verifier.scan_root(root)

    assert any("source-fence-static" in finding for finding in findings), findings


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, include_agent_anchor=False)

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "agent-rust-probe-policy" in result.stderr


def main() -> int:
    tests = [
        test_complete_stable_ids_pass,
        test_heading_reword_does_not_break_stable_ids,
        test_anchor_spacing_and_attribute_casing_pass,
        test_duplicate_stable_section_id_is_a_finding,
        test_legacy_heading_slug_link_is_a_finding,
        test_missing_target_anchor_is_a_finding,
        test_missing_registry_anchor_is_a_finding,
        test_missing_source_fence_wiring_is_a_finding,
        test_standalone_recipe_does_not_satisfy_source_fence_static_inner_wiring,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: stable doc section-id verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
