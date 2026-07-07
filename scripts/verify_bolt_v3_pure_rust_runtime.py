#!/usr/bin/env python3
"""Verify the Bolt-v3 runtime stays pure Rust and SSM-SDK backed."""

from __future__ import annotations

import os
import re
import sys
import tomllib
from pathlib import Path

from rust_source_scanner import (
    blank_preserving_newlines,
    char_literal_end,
    quoted_literal_end,
    raw_string_end,
    strip_rust_comments_and_literals,
)
from verify_bolt_v3_provider_leaks import (
    production_text as production_source_text,
)
from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"
MAIN_RS = REPO_ROOT / "src/main.rs"

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

MAIN_RS_ENTRYPOINT_CALLS = (
    "verify_live_config(&context.config_root, &context.profile)?",
    "build_bolt_v3_live_node_with_resolved(&loaded, resolved)?",
    "run_bolt_v3_live_node(&mut node, &loaded).await?",
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
    # Prune ignored directories at walk time instead of REPO_ROOT.rglob("Cargo.toml")
    # + post-filter. rglob is eager: it descends into .worktrees/ (one subtree per
    # active worktree), .git/, and target/ before the filter discards them, paying a
    # full traversal of tens of thousands of files -- while holding the verification
    # lane -- just to keep a couple of manifests. os.walk with in-place dirnames
    # pruning skips those subtrees entirely. The result set is identical: any
    # Cargo.toml under an ignored directory is unreachable either way.
    paths: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(REPO_ROOT):
        dirnames[:] = [d for d in dirnames if d not in IGNORED_MANIFEST_DIRS]
        if "Cargo.toml" in filenames:
            paths.append(Path(dirpath) / "Cargo.toml")
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


def missing_main_rs_entrypoint_calls(text: str) -> list[str]:
    scan_text = strip_rust_comments_and_literals(strip_cfg_test_items(text))
    return [
        f"src/main.rs is missing entrypoint call {call!r}"
        for call in MAIN_RS_ENTRYPOINT_CALLS
        if call not in scan_text
    ]


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

    manifests = cargo_manifest_paths()
    rust_source_paths = sorted((REPO_ROOT / "src").glob("**/*.rs"))
    floor_findings: list[str] = []
    require_nonempty(manifests, "Cargo manifests", floor_findings)
    require_nonempty(rust_source_paths, "Rust source files under src", floor_findings)
    require_nonempty(RUNTIME_SOURCE_PATHS, "Bolt-v3 runtime source paths", floor_findings)
    if floor_findings:
        findings.extend(floor_findings)
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    for manifest in manifests:
        dependency_names = cargo_dependency_names(manifest)
        rel = manifest.relative_to(REPO_ROOT).as_posix()
        for name in sorted(dependency_names & FORBIDDEN_PACKAGE_NAMES):
            findings.append(f"{rel}: Cargo manifest references forbidden Python bridge package {name!r}")

    lock_names = cargo_lock_package_names(REPO_ROOT / "Cargo.lock")
    for name in sorted(lock_names & FORBIDDEN_PACKAGE_NAMES):
        findings.append(f"Cargo.lock references forbidden Python bridge package {name!r}")

    for path in rust_source_paths:
        rel = path.relative_to(REPO_ROOT).as_posix()
        text = path.read_text(encoding="utf-8")
        scan_text = strip_rust_comments_and_literals(strip_cfg_test_items(text))
        for pattern, label in FORBIDDEN_RUST_PATTERNS:
            for match in pattern.finditer(scan_text):
                findings.append(f"{rel}:{line_number(text, match.start())}: {label}")

    for rel in RUNTIME_SOURCE_PATHS:
        path = REPO_ROOT / rel
        if not path.exists():
            findings.append(f"{rel}: runtime source file is missing")
            continue
        text = production_text(path)
        for pattern, label in FORBIDDEN_RUNTIME_SOURCE_PATTERNS:
            for match in pattern.finditer(text):
                findings.append(
                    f"{rel}:{line_number(text, match.start())}: forbidden {label}: {match.group(0)}"
                )

    if MAIN_RS.exists():
        findings.extend(
            missing_main_rs_entrypoint_calls(MAIN_RS.read_text(encoding="utf-8"))
        )

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 pure-Rust runtime verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
