#!/usr/bin/env python3
"""Verify the Bolt-v3 runtime has no Python bridge layer.

This script intentionally allows Python verifier tooling under `scripts/`.
It checks production Rust source and Cargo metadata for Python FFI/build
bridges such as PyO3, maturin, or cpython.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

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


def cargo_dependency_names(path: Path) -> set[str]:
    if not path.exists():
        return set()

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    names: set[str] = set()
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        dependencies = data.get(section, {})
        if isinstance(dependencies, dict):
            names.update(str(name).lower() for name in dependencies)
    return names


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


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def main() -> int:
    findings: list[str] = []

    for rel in FORBIDDEN_ROOT_FILES:
        path = REPO_ROOT / rel
        if path.exists():
            findings.append(f"{rel}: Python package/build metadata is not allowed for the Rust runtime")

    dependency_names = cargo_dependency_names(REPO_ROOT / "Cargo.toml")
    lock_names = cargo_lock_package_names(REPO_ROOT / "Cargo.lock")
    for name in sorted((dependency_names | lock_names) & FORBIDDEN_PACKAGE_NAMES):
        findings.append(f"Cargo metadata references forbidden Python bridge package {name!r}")

    for path in sorted((REPO_ROOT / "src").glob("**/*.rs")):
        text = path.read_text(encoding="utf-8")
        rel = path.relative_to(REPO_ROOT).as_posix()
        for pattern, label in FORBIDDEN_RUST_PATTERNS:
            for match in pattern.finditer(text):
                findings.append(f"{rel}:{line_number(text, match.start())}: {label}")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 pure Rust runtime audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
