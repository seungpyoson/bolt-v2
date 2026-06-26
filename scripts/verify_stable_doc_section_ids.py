#!/usr/bin/env python3
"""Verify owned Markdown cross-links use explicit stable section IDs."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from justfile_recipe_checks import missing_recipe_commands


REPO_ROOT = Path(__file__).resolve().parent.parent
JUSTFILE_PATH = Path("justfile")
MARKDOWN_FRAGMENT_LINK = re.compile(r"\[[^\]]+\]\((?P<target>[^)\s]+\.md)#(?P<section_id>[^)\s]+)\)")
EXPLICIT_ANCHOR = re.compile(
    r"<a\b[^>]*\bid\s*=\s*(?P<quote>['\"])(?P<section_id>[^'\"]+)(?P=quote)[^>]*>\s*</a>",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class StableAnchor:
    path: Path
    section_id: str


@dataclass(frozen=True)
class StableLink:
    source_path: Path
    target_path: str
    section_id: str

    @property
    def markdown_target(self) -> str:
        return f"{self.target_path}#{self.section_id}"


REQUIRED_ANCHORS = (
    StableAnchor(Path("docs/ci/ubicloud-cost-governance.md"), "ci-operator-policy"),
    StableAnchor(Path("AGENTS.md"), "agent-rust-probe-policy"),
    StableAnchor(
        Path("specs/023-nt-research-analytics-platform/reference/contracts.md"),
        "023-status-legend-registry",
    ),
)
REQUIRED_LINKS = (
    StableLink(Path("AGENTS.md"), "docs/ci/ubicloud-cost-governance.md", "ci-operator-policy"),
    StableLink(
        Path("docs/ci/ubicloud-cost-governance.md"),
        "../../AGENTS.md",
        "agent-rust-probe-policy",
    ),
)
LINK_SOURCE_PATHS = tuple({link.source_path for link in REQUIRED_LINKS})
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_stable_doc_section_ids.py",
    "python3 scripts/verify_stable_doc_section_ids.py",
)


def read_required(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def has_explicit_anchor(text: str, section_id: str) -> bool:
    return section_id in explicit_anchor_ids(text)


def explicit_anchor_ids(text: str) -> list[str]:
    return [match.group("section_id") for match in EXPLICIT_ANCHOR.finditer(text)]


def check_duplicate_anchors(rel_path: Path, text: str, findings: list[str]) -> None:
    seen: set[str] = set()
    for section_id in explicit_anchor_ids(text):
        if section_id in seen:
            findings.append(f"{rel_path}: duplicate stable section id `{section_id}`")
        seen.add(section_id)


def resolve_markdown_target(root: Path, source_path: Path, target_path: str) -> Path:
    return ((root / source_path).parent / target_path).resolve()


def rel_to_root(root: Path, path: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []
    text_by_path: dict[Path, str] = {}

    paths_to_read = {JUSTFILE_PATH, *LINK_SOURCE_PATHS, *(anchor.path for anchor in REQUIRED_ANCHORS)}
    for rel_path in sorted(paths_to_read):
        text_by_path[rel_path] = read_required(root, rel_path, findings)

    for rel_path, text in text_by_path.items():
        if text:
            check_duplicate_anchors(rel_path, text, findings)

    for anchor in REQUIRED_ANCHORS:
        if text_by_path.get(anchor.path) and not has_explicit_anchor(text_by_path[anchor.path], anchor.section_id):
            findings.append(f"{anchor.path}: missing stable section id `{anchor.section_id}`")

    for link in REQUIRED_LINKS:
        source_text = text_by_path.get(link.source_path, "")
        if source_text and f"]({link.markdown_target})" not in source_text:
            findings.append(f"{link.source_path}: missing stable link target `{link.markdown_target}`")

    for source_path in LINK_SOURCE_PATHS:
        source_text = text_by_path.get(source_path, "")
        for match in MARKDOWN_FRAGMENT_LINK.finditer(source_text):
            target_path = match.group("target")
            section_id = match.group("section_id")
            target_abs = resolve_markdown_target(root, source_path, target_path)
            try:
                target_text = target_abs.read_text(encoding="utf-8")
            except FileNotFoundError:
                findings.append(f"{source_path}: link target `{target_path}` is missing")
                continue
            if not has_explicit_anchor(target_text, section_id):
                findings.append(
                    f"{source_path}: `{target_path}#{section_id}` must point to an explicit stable section id in "
                    f"{rel_to_root(root, target_abs)}"
                )

    justfile_text = text_by_path.get(JUSTFILE_PATH, "")
    if justfile_text:
        for command in missing_recipe_commands(justfile_text, JUSTFILE_COMMANDS):
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: stable doc section-id violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: stable doc section IDs passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
