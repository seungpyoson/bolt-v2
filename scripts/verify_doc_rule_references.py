#!/usr/bin/env python3
"""Verify owned docs cite AGENTS.md Repo Rules by stable IDs, not ordinals."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
AGENTS_PATH = Path("AGENTS.md")
SPEC_522_ROOT = Path("specs/522-decompose-strategy-monolith")
JUSTFILE_PATH = Path("justfile")

REPO_RULE_IDS = (
    "repo-rule-no-hardcodes",
    "repo-rule-no-dual-paths",
    "repo-rule-no-debts",
    "repo-rule-no-credential-display",
    "repo-rule-pure-rust-binary",
    "repo-rule-ssm-single-secret-source",
    "repo-rule-group-by-change",
    "repo-rule-do-not-reference-bolt-v1",
    "repo-rule-strategies-produce-intent-only",
    "repo-rule-chainlink-data-streams-testnet-is-production",
)
REQUIRED_522_REFERENCES = {
    Path("spec.md"): ("repo-rule-strategies-produce-intent-only",),
    Path("plan.md"): ("repo-rule-strategies-produce-intent-only",),
    Path("slices/A1.md"): ("repo-rule-no-dual-paths",),
    Path("slices/A2.md"): ("repo-rule-strategies-produce-intent-only",),
}
FORBIDDEN_522_REFERENCES = {
    Path("slices/A1.md"): ("repo-rule-ssm-single-secret-source",),
}
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_doc_rule_references.py",
    "python3 scripts/verify_doc_rule_references.py",
)

SECTION_RE = re.compile(r"^## Repo Rules\n(?P<body>.*?)(?=^## |\Z)", re.MULTILINE | re.DOTALL)
RULE_LINE_RE = re.compile(r"^\d+\.\s*<a id=\"(?P<anchor>repo-rule-[a-z0-9-]+)\"></a>\s+\*\*", re.MULTILINE)
ORDINAL_RE = re.compile(r"\brule #[0-9]+\b", re.IGNORECASE)
AGENTS_ANCHOR_RE = re.compile(r"AGENTS\.md#(?P<anchor>repo-rule-[a-z0-9-]+)")


def read_text(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def repo_rules_section(agents_text: str, findings: list[str]) -> str:
    match = SECTION_RE.search(agents_text)
    if not match:
        findings.append(f"{AGENTS_PATH}: missing `## Repo Rules` section")
        return ""
    return match.group("body")


def repo_rule_anchors(agents_text: str, findings: list[str]) -> set[str]:
    section = repo_rules_section(agents_text, findings)
    anchors = [match.group("anchor") for match in RULE_LINE_RE.finditer(section)]
    seen: set[str] = set()
    for anchor in anchors:
        if anchor in seen:
            findings.append(f"{AGENTS_PATH}: duplicate Repo Rule ID `{anchor}`")
        seen.add(anchor)
    expected = set(REPO_RULE_IDS)
    for anchor in sorted(expected - seen):
        findings.append(f"{AGENTS_PATH}: missing Repo Rule ID `{anchor}`")
    for anchor in sorted(seen - expected):
        findings.append(f"{AGENTS_PATH}: unexpected Repo Rule ID `{anchor}`")
    return seen


def spec_522_docs(root: Path, findings: list[str]) -> list[Path]:
    spec_root = root / SPEC_522_ROOT
    if not spec_root.exists():
        findings.append(f"{SPEC_522_ROOT}: directory is missing")
        return []
    return sorted(path.relative_to(spec_root) for path in spec_root.rglob("*.md"))


def check_spec_522_doc(root: Path, rel_doc: Path, valid_rule_ids: set[str], findings: list[str]) -> None:
    rel_path = SPEC_522_ROOT / rel_doc
    text = read_text(root, rel_path, findings)
    if not text:
        return

    for match in ORDINAL_RE.finditer(text):
        findings.append(f"{rel_path}: replace ordinal `{match.group(0)}` with a stable AGENTS.md#repo-rule-* ID")

    referenced = set(match.group("anchor") for match in AGENTS_ANCHOR_RE.finditer(text))
    for anchor in sorted(referenced - valid_rule_ids):
        findings.append(f"{rel_path}: references unknown AGENTS.md Repo Rule ID `{anchor}`")

    for anchor in REQUIRED_522_REFERENCES.get(rel_doc, ()):
        if anchor not in referenced:
            findings.append(f"{rel_path}: must reference AGENTS.md#{anchor}")
    for anchor in FORBIDDEN_522_REFERENCES.get(rel_doc, ()):
        if anchor in referenced:
            findings.append(f"{rel_path}: must not reference AGENTS.md#{anchor} for the numeric single-source citation")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    agents_text = read_text(root, AGENTS_PATH, findings)
    rule_ids = repo_rule_anchors(agents_text, findings) if agents_text else set()
    for rel_doc in spec_522_docs(root, findings):
        check_spec_522_doc(root, rel_doc, rule_ids, findings)

    justfile_text = read_text(root, JUSTFILE_PATH, findings)
    for command in JUSTFILE_COMMANDS:
        if justfile_text and command not in justfile_text:
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: doc rule-reference violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: doc rule references use stable Repo Rule IDs.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
