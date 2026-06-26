#!/usr/bin/env python3
"""Self-tests for the stable Repo Rule reference verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_doc_rule_references.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_doc_rule_references", SCRIPT)
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


def agents_text(
    module,
    *,
    omit_anchor: str | None = None,
    heading: str = "## Repo Rules",
    decorated_anchors: bool = False,
) -> str:
    lines = ["# bolt-v2 Agent Rules", "", heading, ""]
    for index, anchor in enumerate(module.REPO_RULE_IDS, start=1):
        if anchor == omit_anchor:
            lines.append(f"{index}. **RULE {index}** — placeholder rule text.")
        elif decorated_anchors:
            anchor_markup = f"<a class='stable-id' id='{anchor}' data-owner='agents'></a>"
            lines.append(f"{index}. {anchor_markup} **RULE {index}** — placeholder rule text.")
        else:
            lines.append(f"{index}. <a id=\"{anchor}\"></a> **RULE {index}** — placeholder rule text.")
    lines.extend(["", "## Evidence-Driven Verification", ""])
    return "\n".join(lines)


def spec_text(*, ordinal: bool = False, unknown_anchor: bool = False) -> str:
    reference = "rule #9" if ordinal else "../../AGENTS.md#repo-rule-strategies-produce-intent-only"
    if unknown_anchor:
        reference = "../../AGENTS.md#repo-rule-made-up"
    return f"""
# Feature Specification

The strategy file owns concerns that, per `{reference}`, must live in shared modules.
"""


def plan_text(*, ordinal: bool = False) -> str:
    reference = "rule #9" if ordinal else "../../AGENTS.md#repo-rule-strategies-produce-intent-only"
    return f"""
# Implementation Plan

src/bolt_v3_book_sizing.rs # shared execution sizing ({reference})
"""


def a1_text(*, anchor: str = "repo-rule-no-dual-paths") -> str:
    return f"""
# Slice A1

Generic primitives move to the shared numeric module, keeping one source of truth
(`../../../AGENTS.md#{anchor}`).
"""


def a2_text() -> str:
    return """
# Slice A2

One coherent strategy-intent scope (`../../../AGENTS.md#repo-rule-strategies-produce-intent-only`).
"""


def justfile_text(*, wired: bool = True, standalone_only: bool = False) -> str:
    commands = (
        "    python3 scripts/test_verify_doc_rule_references.py\n"
        "    python3 scripts/verify_doc_rule_references.py\n"
        if wired
        else ""
    )
    if standalone_only:
        return f"verify-doc-rule-references:\n{commands}\nsource-fence-static-inner:\n"
    return f"source-fence-static-inner:\n{commands}"


def write_complete_fixture(root: Path, module) -> None:
    write_file(root, "AGENTS.md", agents_text(module))
    write_file(root, "specs/522-decompose-strategy-monolith/spec.md", spec_text())
    write_file(root, "specs/522-decompose-strategy-monolith/plan.md", plan_text())
    write_file(root, "specs/522-decompose-strategy-monolith/slices/A1.md", a1_text())
    write_file(root, "specs/522-decompose-strategy-monolith/slices/A2.md", a2_text())
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
        write_complete_fixture(root, verifier)

        assert verifier.scan_root(root) == []


def test_repo_rule_anchor_scan_ignores_heading_text() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "AGENTS.md", agents_text(verifier, heading="## Runtime Rules"))

        findings = verifier.scan_root(root)

    assert findings == []


def test_repo_rule_anchor_scan_accepts_single_quotes_and_attributes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "AGENTS.md", agents_text(verifier, decorated_anchors=True))

        findings = verifier.scan_root(root)

    assert findings == []


def test_owned_rule_ordinal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "specs/522-decompose-strategy-monolith/plan.md", plan_text(ordinal=True))

        findings = verifier.scan_root(root)

    assert any("replace ordinal `rule #9`" in finding for finding in findings), findings


def test_missing_repo_rule_anchor_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "AGENTS.md", agents_text(verifier, omit_anchor="repo-rule-no-dual-paths"))

        findings = verifier.scan_root(root)

    assert any("missing Repo Rule ID `repo-rule-no-dual-paths`" in finding for finding in findings), findings


def test_unknown_repo_rule_link_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "specs/522-decompose-strategy-monolith/spec.md", spec_text(unknown_anchor=True))

        findings = verifier.scan_root(root)

    assert any("repo-rule-made-up" in finding for finding in findings), findings


def test_a1_single_source_truth_must_not_use_ssm_rule() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "specs/522-decompose-strategy-monolith/slices/A1.md", a1_text(anchor="repo-rule-ssm-single-secret-source"))

        findings = verifier.scan_root(root)

    assert any("must reference AGENTS.md#repo-rule-no-dual-paths" in finding for finding in findings), findings
    assert any("must not reference AGENTS.md#repo-rule-ssm-single-secret-source" in finding for finding in findings), findings


def test_missing_source_fence_wiring_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "justfile", justfile_text(wired=False))

        findings = verifier.scan_root(root)

    assert any("source-fence-static must run" in finding for finding in findings), findings


def test_standalone_recipe_does_not_satisfy_source_fence_static_inner_wiring() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "justfile", justfile_text(standalone_only=True))

        findings = verifier.scan_root(root)

    assert any("source-fence-static" in finding for finding in findings), findings


def test_cli_fails_with_actionable_output() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, verifier)
        write_file(root, "specs/522-decompose-strategy-monolith/plan.md", plan_text(ordinal=True))

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "rule #9" in result.stderr


def main() -> int:
    tests = [
        test_complete_fixture_passes,
        test_repo_rule_anchor_scan_ignores_heading_text,
        test_repo_rule_anchor_scan_accepts_single_quotes_and_attributes,
        test_owned_rule_ordinal_is_a_finding,
        test_missing_repo_rule_anchor_is_a_finding,
        test_unknown_repo_rule_link_is_a_finding,
        test_a1_single_source_truth_must_not_use_ssm_rule,
        test_missing_source_fence_wiring_is_a_finding,
        test_standalone_recipe_does_not_satisfy_source_fence_static_inner_wiring,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: doc rule-reference verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
