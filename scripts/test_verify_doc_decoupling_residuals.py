#!/usr/bin/env python3
"""Self-tests for verify_doc_decoupling_residuals.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import textwrap
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_doc_decoupling_residuals.py")
SPEC = importlib.util.spec_from_file_location("verify_doc_decoupling_residuals", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


LEDGER_TEXT = """
[doc_decoupling_residuals]
version = 1

[[doc_decoupling_residuals.allowed_markdown_references]]
path = "scripts/verify_ai_review_governance.py"
kind = "workflow_snippet_false_positive"
tracking_issue = "#1027"
read_purpose = "none"
snippets = ['"AGENTS.md",']

[[doc_decoupling_residuals.allowed_markdown_references]]
path = "scripts/verify_bolt_v3_naming.py"
kind = "deliberate_guard"
owner_issue = "#711"
tracking_issue = "#1027"
read_purpose = "rename_guard"
snippets = ['"docs/bolt-v3/*.md",', '"docs/**/*",', '".md",']

[[doc_decoupling_residuals.allowed_markdown_references]]
path = "scripts/verify_bolt_v3_schema_current.py"
kind = "doc_sync_exception"
owner_issue = "#559"
tracking_issue = "#1027"
read_purpose = "doc_sync_exception"
snippets = ['SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/schema.md"']
"""


def write_fixture(
    root: Path,
    *,
    ledger_text: str = LEDGER_TEXT,
    extra_script: str = "",
    naming_script_suffix: str = "",
    schema_script: str = 'SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/schema.md"\n',
) -> None:
    (root / "ci").mkdir(parents=True)
    (root / "ci" / "doc-decoupling-residuals.toml").write_text(
        textwrap.dedent(ledger_text).lstrip(),
        encoding="utf-8",
    )
    scripts = root / "scripts"
    scripts.mkdir()
    (scripts / "verify_ai_review_governance.py").write_text(
        textwrap.dedent(
            """
            KIMI = (
                "AGENTS.md",
            )
            """
        ).lstrip(),
        encoding="utf-8",
    )
    (scripts / "verify_bolt_v3_naming.py").write_text(
        textwrap.dedent(
            f"""
            SCAN_GLOBS = [
                "docs/bolt-v3/*.md",
            ]
            MISNOMER_SCAN_GLOBS = [
                "docs/**/*",
            ]
            MISNOMER_TEXT_SUFFIXES = {{
                ".md",
            }}
            {naming_script_suffix}
            """
        ).lstrip(),
        encoding="utf-8",
    )
    (scripts / "verify_bolt_v3_schema_current.py").write_text(
        schema_script,
        encoding="utf-8",
    )
    if extra_script:
        (scripts / "verify_new_doc_reader.py").write_text(extra_script, encoding="utf-8")


def collect(
    *,
    ledger_text: str = LEDGER_TEXT,
    extra_script: str = "",
    naming_script_suffix: str = "",
    schema_script: str = 'SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/schema.md"\n',
) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            ledger_text=ledger_text,
            extra_script=extra_script,
            naming_script_suffix=naming_script_suffix,
            schema_script=schema_script,
        )
        return VERIFIER.collect_findings(root)


def assert_finding(findings: list[str], expected: str) -> None:
    if not any(expected in finding for finding in findings):
        raise AssertionError(f"expected {expected!r} in {findings!r}")


def test_known_residuals_pass() -> None:
    findings = collect()
    if findings:
        raise AssertionError(findings)


def test_unledgered_markdown_reference_fails() -> None:
    findings = collect(extra_script='DOC = "docs/new-truth.md"\n')
    assert_finding(findings, "unclassified prose reference")


def test_unledgered_docs_wide_glob_fails() -> None:
    findings = collect(extra_script='SCAN = ["docs/**/*"]\n')
    assert_finding(findings, "unclassified prose reference")


def test_same_line_extra_markdown_reference_fails() -> None:
    findings = collect(
        schema_script=(
            'SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/schema.md"; '
            'NEW_DOC = REPO_ROOT / "docs/prose-authority.md"\n'
        )
    )
    assert_finding(findings, "unclassified prose reference")


def test_exact_line_matching_does_not_allow_other_md_suffix_line() -> None:
    findings = collect(naming_script_suffix='EXT = ".md"\n')
    assert_finding(findings, "unclassified prose reference")


def test_stale_ledger_snippet_fails() -> None:
    findings = collect(ledger_text=LEDGER_TEXT.replace('"AGENTS.md"', '"MISSING.md"'))
    assert_finding(findings, "stale residual ledger snippet")


def test_doc_sync_exception_requires_owner_issue() -> None:
    findings = collect(ledger_text=LEDGER_TEXT.replace('owner_issue = "#559"\n', "", 1))
    assert_finding(findings, "doc_sync_exception must declare owner_issue #559")


def test_deliberate_guard_requires_owner_issue() -> None:
    findings = collect(ledger_text=LEDGER_TEXT.replace('owner_issue = "#711"\n', "", 1))
    assert_finding(findings, "deliberate_guard must declare owner_issue #711")


def test_deliberate_guard_requires_rename_guard_purpose() -> None:
    findings = collect(ledger_text=LEDGER_TEXT.replace('read_purpose = "rename_guard"', 'read_purpose = "none"', 1))
    assert_finding(findings, "deliberate_guard must declare read_purpose rename_guard")


def test_doc_sync_exception_requires_doc_sync_purpose() -> None:
    findings = collect(
        ledger_text=LEDGER_TEXT.replace(
            'read_purpose = "doc_sync_exception"',
            'read_purpose = "rename_guard"',
            1,
        )
    )
    assert_finding(findings, "doc_sync_exception must declare read_purpose doc_sync_exception")


def main() -> int:
    tests = [
        test_known_residuals_pass,
        test_unledgered_markdown_reference_fails,
        test_unledgered_docs_wide_glob_fails,
        test_same_line_extra_markdown_reference_fails,
        test_exact_line_matching_does_not_allow_other_md_suffix_line,
        test_stale_ledger_snippet_fails,
        test_doc_sync_exception_requires_owner_issue,
        test_deliberate_guard_requires_owner_issue,
        test_deliberate_guard_requires_rename_guard_purpose,
        test_doc_sync_exception_requires_doc_sync_purpose,
    ]
    for test in tests:
        test()
    print("OK: doc-decoupling residual verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
