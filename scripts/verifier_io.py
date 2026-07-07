#!/usr/bin/env python3
"""Shared file/snippet checks for repository verifier scripts."""

from __future__ import annotations

from collections.abc import Sized
from pathlib import Path


def require_text_file(root: Path, rel_path: Path, findings: list[str]) -> str | None:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return None
    return path.read_text(encoding="utf-8")


def require_nonempty(items: Sized, what: str, findings: list[str]) -> bool:
    if len(items) == 0:
        findings.append(f"{what}: enforcement set is empty")
        return False
    return True


def require_snippets(
    rel_path: Path,
    text: str | None,
    snippets: tuple[str, ...],
    findings: list[str],
) -> None:
    if text is None:
        return
    for snippet in snippets:
        if snippet not in text:
            findings.append(f"{rel_path}: missing `{snippet}`")
