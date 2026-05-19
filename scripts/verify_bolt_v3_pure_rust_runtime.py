#!/usr/bin/env python3
"""Verify the Bolt-v3 runtime stays pure Rust and SSM-SDK backed."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

from verify_bolt_v3_provider_leaks import (
    production_text as production_source_text,
)


REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"

FORBIDDEN_ROOT_FILES = (
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
)

FORBIDDEN_PACKAGE_NAMES = {
    "cpython",
    "maturin",
    "pyo3",
    "pyo3-asyncio",
    "pyo3-build-config",
    "pyo3-ffi",
    "pythonize",
    "rust-cpython",
}

FORBIDDEN_RUST_PATTERNS = (
    (re.compile(r"\bpyo3::"), "PyO3 Rust API usage"),
    (re.compile(r"\bcpython::"), "cpython Rust API usage"),
    (re.compile(r"#\s*\[\s*py(?:class|function|method|module|methods)"), "Python export attribute"),
    (re.compile(r"\bPython::with_gil\b"), "Python GIL runtime usage"),
    (re.compile(r"\bPy(?:Any|Err|Module|Object|Result)\b"), "Python object/result type"),
)

RUNTIME_SOURCE_PATHS = tuple(
    sorted(
        {
            "src/main.rs",
            "src/nt_runtime_capture.rs",
            "src/secrets.rs",
            *(
                path.relative_to(REPO_ROOT).as_posix()
                for path in (REPO_ROOT / "src").glob("bolt_v3*.rs")
            ),
            *(
                path.relative_to(REPO_ROOT).as_posix()
                for directory in (REPO_ROOT / "src").glob("bolt_v3_*")
                if directory.is_dir()
                for path in directory.rglob("*.rs")
            ),
            *(
                path.relative_to(REPO_ROOT).as_posix()
                for path in (REPO_ROOT / "src" / "strategies").rglob("*.rs")
            ),
        }
    )
)

FORBIDDEN_RUNTIME_SOURCE_PATTERNS = (
    (re.compile(r"\bpyo3\b", re.IGNORECASE), "PyO3 runtime binding"),
    (re.compile(r"\bmaturin\b", re.IGNORECASE), "maturin Python extension build"),
    (
        re.compile(r"(?:std::process::)?Command::new\s*\("),
        "runtime subprocess",
    ),
)

REQUIRED_STATUS_MAP_PHRASES = (
    "| 3 | No Python runtime layer | Implemented as current source-scan gate |",
    "`scripts/verify_bolt_v3_pure_rust_runtime.py`",
)

DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
IGNORED_MANIFEST_DIRS = {".git", ".worktrees", "target"}


def collect_dependency_names(data: dict[str, object]) -> set[str]:
    names: set[str] = set()

    def add_dependency_table(table: object) -> None:
        if isinstance(table, dict):
            names.update(str(name).lower() for name in table)

    for section in DEPENDENCY_SECTIONS:
        add_dependency_table(data.get(section))

    workspace = data.get("workspace", {})
    if isinstance(workspace, dict):
        for section in DEPENDENCY_SECTIONS:
            add_dependency_table(workspace.get(section))

    target_sections = data.get("target", {})
    if isinstance(target_sections, dict):
        for target_config in target_sections.values():
            if isinstance(target_config, dict):
                for section in DEPENDENCY_SECTIONS:
                    add_dependency_table(target_config.get(section))

    return names


def cargo_dependency_names(path: Path) -> set[str]:
    if not path.exists():
        return set()

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return collect_dependency_names(data)


def cargo_manifest_paths() -> list[Path]:
    paths: list[Path] = []
    for path in REPO_ROOT.rglob("Cargo.toml"):
        rel_parts = path.relative_to(REPO_ROOT).parts
        if any(part in IGNORED_MANIFEST_DIRS for part in rel_parts):
            continue
        paths.append(path)
    return sorted(paths)


def cargo_lock_package_names(path: Path) -> set[str]:
    if not path.exists():
        return set()

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    packages = data.get("package", [])
    names: set[str] = set()
    for package in packages:
        if isinstance(package, dict) and package.get("name"):
            names.add(str(package["name"]).lower())
    return names


def production_text(path: Path) -> str:
    return strip_cfg_test_items(path.read_text(encoding="utf-8"))


def strip_cfg_test_items(text: str) -> str:
    return production_source_text(text)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def blank_preserving_newlines(text: str) -> str:
    return "".join("\n" if char == "\n" else " " for char in text)


def raw_string_end(text: str, start: int) -> int | None:
    i = start
    if i < len(text) and text[i] in {"b", "c"}:
        i += 1
    if i >= len(text) or text[i] != "r":
        return None

    i += 1
    hash_start = i
    while i < len(text) and text[i] == "#":
        i += 1
    if i >= len(text) or text[i] != '"':
        return None

    delimiter = '"' + text[hash_start:i]
    end = text.find(delimiter, i + 1)
    if end == -1:
        return len(text)
    return end + len(delimiter)


def quoted_literal_end(text: str, start: int, quote: str) -> int:
    i = start + 1
    while i < len(text):
        char = text[i]
        if char == "\\":
            i += 2
            continue
        if char == quote:
            return i + 1
        if char == "\n" and quote == "'":
            return start + 1
        i += 1
    return len(text)


def char_literal_end(text: str, start: int) -> int | None:
    i = start + 1
    if i >= len(text) or text[i] in {"'", "\n", "\r"}:
        return None

    if text[i] == "\\":
        i += 1
        if i >= len(text):
            return None
        if text.startswith("u{", i):
            end = text.find("}", i + 2)
            if end == -1:
                return None
            i = end + 1
        elif text[i] == "x" and i + 2 < len(text):
            i += 3
        else:
            i += 1
    else:
        i += 1

    if i < len(text) and text[i] == "'":
        return i + 1
    return None


def strip_rust_comments_and_literals(text: str) -> str:
    output: list[str] = []
    i = 0
    while i < len(text):
        raw_end = raw_string_end(text, i)
        if raw_end is not None:
            output.append(blank_preserving_newlines(text[i:raw_end]))
            i = raw_end
            continue

        if text.startswith("//", i):
            end = text.find("\n", i)
            if end == -1:
                end = len(text)
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text.startswith("/*", i):
            depth = 1
            j = i + 2
            while j < len(text) and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            output.append(blank_preserving_newlines(text[i:j]))
            i = j
            continue

        if text[i] in {"b", "c"} and i + 1 < len(text) and text[i + 1] == '"':
            end = quoted_literal_end(text, i + 1, '"')
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text[i] == '"':
            end = quoted_literal_end(text, i, '"')
            output.append(blank_preserving_newlines(text[i:end]))
            i = end
            continue

        if text[i] == "'":
            end = char_literal_end(text, i)
            if end is not None:
                output.append(blank_preserving_newlines(text[i:end]))
                i = end
                continue

        output.append(text[i])
        i += 1

    return "".join(output)


def main() -> int:
    findings: list[str] = []

    cargo_dependencies = cargo_dependency_names(CARGO_TOML)
    if "aws-sdk-ssm" not in cargo_dependencies:
        findings.append("Cargo.toml does not include aws-sdk-ssm")
    if "aws-config" not in cargo_dependencies:
        findings.append("Cargo.toml does not include aws-config")

    for rel in FORBIDDEN_ROOT_FILES:
        path = REPO_ROOT / rel
        if path.exists():
            findings.append(f"{rel}: Python package/build metadata is not allowed for the Rust runtime")

    for manifest in cargo_manifest_paths():
        dependency_names = cargo_dependency_names(manifest)
        rel = manifest.relative_to(REPO_ROOT).as_posix()
        for name in sorted(dependency_names & FORBIDDEN_PACKAGE_NAMES):
            findings.append(f"{rel}: Cargo manifest references forbidden Python bridge package {name!r}")

    lock_names = cargo_lock_package_names(REPO_ROOT / "Cargo.lock")
    for name in sorted(lock_names & FORBIDDEN_PACKAGE_NAMES):
        findings.append(f"Cargo.lock references forbidden Python bridge package {name!r}")

    for path in sorted((REPO_ROOT / "src").glob("**/*.rs")):
        rel = path.relative_to(REPO_ROOT).as_posix()
        text = path.read_text(encoding="utf-8")
        scan_text = strip_rust_comments_and_literals(strip_cfg_test_items(text))
        for pattern, label in FORBIDDEN_RUST_PATTERNS:
            for match in pattern.finditer(scan_text):
                findings.append(f"{rel}:{line_number(text, match.start())}: {label}")

    for rel in RUNTIME_SOURCE_PATHS:
        path = REPO_ROOT / rel
        if not path.exists():
            continue
        text = production_text(path)
        for pattern, label in FORBIDDEN_RUNTIME_SOURCE_PATTERNS:
            for match in pattern.finditer(text):
                findings.append(
                    f"{rel}:{line_number(text, match.start())}: forbidden {label}: {match.group(0)}"
                )

    status_map = STATUS_MAP.read_text(encoding="utf-8")
    if "| 3 | No Python runtime layer | Missing verifier |" in status_map:
        findings.append("status map still marks row 3 as missing a verifier")
    for phrase in REQUIRED_STATUS_MAP_PHRASES:
        if phrase not in status_map:
            findings.append(f"status map missing current pure-Rust evidence phrase: {phrase}")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 pure-Rust runtime verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
