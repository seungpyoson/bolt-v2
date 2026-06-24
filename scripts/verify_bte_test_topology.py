#!/usr/bin/env python3
"""Verify the backtester test topology stays compile-efficient."""

from __future__ import annotations

import pathlib
import re
import sys
import tomllib

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
BVS_CRATE = pathlib.Path("crates/backtesting-vertical-slice")
BVS_CARGO_TOML = BVS_CRATE / "Cargo.toml"
BVS_TESTS = BVS_CRATE / "tests"
BVS_HARNESS_NAME = "backtesting_vertical_slice_tests.rs"
BVS_HARNESS_PATH = BVS_TESTS / BVS_HARNESS_NAME
BVS_HARNESS_TEST_NAME = "backtesting_vertical_slice_tests"
BVS_PUBLIC_API_TEST = BVS_TESTS / "backtesting_vertical_slice_public_api.rs"

MODULE_RE = re.compile(
    r'(?m)^#\[path = "(?P<path>[^"]+)"\]\nmod (?P<module>[A-Za-z][A-Za-z0-9_]*);$'
)


def repo_relative(path: pathlib.Path) -> str:
    return path.as_posix()


def module_name(path: pathlib.Path) -> str:
    stem = path.stem
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*", stem):
        raise ValueError(f"{path}: cannot derive Rust module name from file stem {stem!r}")
    return stem


def integration_test_files(root: pathlib.Path) -> list[pathlib.Path]:
    tests_dir = root / BVS_TESTS
    if not tests_dir.exists():
        return []
    return sorted(path for path in tests_dir.glob("*.rs") if path.name != BVS_HARNESS_NAME)


def verify_cargo_manifest(root: pathlib.Path) -> list[str]:
    cargo_toml_path = root / BVS_CARGO_TOML
    if not cargo_toml_path.exists():
        return [f"{repo_relative(BVS_CARGO_TOML)} must exist"]
    try:
        manifest = tomllib.loads(cargo_toml_path.read_text())
    except tomllib.TOMLDecodeError as exc:
        return [f"{repo_relative(BVS_CARGO_TOML)} is invalid TOML: {exc}"]

    errors: list[str] = []
    package = manifest.get("package", {})
    if package.get("autotests") is not False:
        errors.append("backtester Cargo.toml must set package.autotests = false")

    test_entries = manifest.get("test", [])
    if not isinstance(test_entries, list):
        errors.append("backtester Cargo.toml [[test]] entries must be an array")
        return errors
    expected_path = repo_relative(pathlib.Path("tests") / BVS_HARNESS_NAME)
    expected = [
        entry
        for entry in test_entries
        if isinstance(entry, dict)
        and entry.get("name") == BVS_HARNESS_TEST_NAME
        and entry.get("path") == expected_path
    ]
    if len(expected) != 1:
        errors.append("backtester Cargo.toml must define exactly one explicit integration test harness")
    unexpected = [
        entry
        for entry in test_entries
        if not (
            isinstance(entry, dict)
            and entry.get("name") == BVS_HARNESS_TEST_NAME
            and entry.get("path") == expected_path
        )
    ]
    if unexpected:
        errors.append("backtester Cargo.toml must not define extra integration test binaries")
    return errors


def verify_harness(root: pathlib.Path) -> list[str]:
    tests = integration_test_files(root)
    harness_path = root / BVS_HARNESS_PATH
    if not harness_path.exists():
        return [f"{repo_relative(BVS_HARNESS_PATH)} must exist"]

    errors: list[str] = []
    harness_text = harness_path.read_text()
    if '#![recursion_limit = "256"]' not in harness_text:
        errors.append("backtester integration harness must carry the shared recursion_limit")

    entries = [(match.group("path"), match.group("module")) for match in MODULE_RE.finditer(harness_text)]
    expected_entries = [(path.name, module_name(path)) for path in tests]
    missing = sorted(set(expected_entries) - set(entries))
    stale = sorted(set(entries) - set(expected_entries))
    if missing:
        errors.append("backtester integration harness must include every test file: missing " + ", ".join(path for path, _ in missing))
    if stale:
        errors.append("backtester integration harness must not include stale modules: " + ", ".join(path for path, _ in stale))
    if len(entries) != len(set(entries)):
        errors.append("backtester integration harness must not duplicate test modules")

    for path in tests:
        text = path.read_text()
        if re.search(r"(?m)^#!\[", text):
            errors.append(f"{repo_relative(path.relative_to(root))} must not keep crate-level attributes outside the harness")
    return errors


def verify_public_api_probe(root: pathlib.Path) -> list[str]:
    path = root / BVS_PUBLIC_API_TEST
    if not path.exists():
        return [f"{repo_relative(BVS_PUBLIC_API_TEST)} must exist"]
    text = path.read_text()
    if 'env("CARGO_TARGET_DIR", temp.path().join("target"))' in text:
        return ["backtester public API compile-fail probe must not force an isolated temp CARGO_TARGET_DIR"]
    return []


def verify_root(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    errors.extend(verify_cargo_manifest(root))
    errors.extend(verify_harness(root))
    errors.extend(verify_public_api_probe(root))
    return errors


def main() -> int:
    errors = verify_root(REPO_ROOT)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: backtester test topology verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
