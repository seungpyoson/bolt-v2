#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "ci_input_sets.py"


def load_module():
    spec = importlib.util.spec_from_file_location("ci_input_sets", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("failed to load ci_input_sets.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def run_git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True)


def fixture_repo(root: Path) -> Path:
    repo = root / "repo"
    repo.mkdir()
    run_git(repo, "init")
    run_git(repo, "config", "user.email", "ci-input-sets@example.invalid")
    run_git(repo, "config", "user.name", "CI Input Sets Test")
    write(
        repo / "ci" / "rust-ci-inputs.toml",
        """
[sets.cache]
paths = [
  "Cargo.toml",
  "src/**",
]

[sets.detect]
include_sets = ["cache"]
paths = [
  "scripts/helper.py",
]
""",
    )
    write(repo / "Cargo.toml", "[package]\nname = \"fixture\"\n")
    write(repo / "src" / "lib.rs", "pub fn value() -> u8 { 1 }\n")
    write(repo / "scripts" / "helper.py", "print('helper')\n")
    run_git(repo, "add", ".")
    run_git(repo, "commit", "--no-verify", "-m", "initial")
    return repo


def assert_set_expansion_is_recursive_and_stable() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        repo = fixture_repo(Path(tmp))
        config = module.load_config(repo / "ci" / "rust-ci-inputs.toml")
        actual = module.resolve_set(config, "detect")
    expected = ["Cargo.toml", "src/**", "scripts/helper.py"]
    if actual != expected:
        raise AssertionError(f"input set expansion drifted: expected={expected!r} actual={actual!r}")


def assert_hash_changes_when_tracked_inputs_change() -> None:
    with TemporaryDirectory() as tmp:
        repo = fixture_repo(Path(tmp))
        before = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "hash", "cache"],
            text=True,
        ).strip()
        write(repo / "src" / "lib.rs", "pub fn value() -> u8 { 2 }\n")
        after = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "hash", "cache"],
            text=True,
        ).strip()
    if before == after:
        raise AssertionError("input hash must change when a tracked input changes")


def assert_hash_changes_when_exact_input_is_deleted() -> None:
    with TemporaryDirectory() as tmp:
        repo = fixture_repo(Path(tmp))
        before = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "hash", "cache"],
            text=True,
        ).strip()
        run_git(repo, "rm", "Cargo.toml")
        run_git(repo, "commit", "--no-verify", "-m", "delete exact input")
        after = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "hash", "cache"],
            text=True,
        ).strip()
    if before == after:
        raise AssertionError("input hash must change when an exact input is deleted")


def assert_changed_reports_deleted_exact_inputs() -> None:
    with TemporaryDirectory() as tmp:
        repo = fixture_repo(Path(tmp))
        base = run_git(repo, "rev-parse", "HEAD").strip()
        run_git(repo, "rm", "Cargo.toml")
        run_git(repo, "commit", "--no-verify", "-m", "delete exact input")
        changed = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "changed", "cache", "--base", base, "--head", "HEAD"],
            text=True,
        ).splitlines()
    if changed != ["Cargo.toml"]:
        raise AssertionError(f"deleted exact inputs must be reported as changed, got {changed!r}")


def assert_changed_uses_named_input_set() -> None:
    with TemporaryDirectory() as tmp:
        repo = fixture_repo(Path(tmp))
        base = run_git(repo, "rev-parse", "HEAD").strip()
        write(repo / "scripts" / "helper.py", "print('changed helper')\n")
        run_git(repo, "add", ".")
        run_git(repo, "commit", "--no-verify", "-m", "change helper")
        changed = subprocess.check_output(
            [sys.executable, str(SCRIPT), "--repo", str(repo), "changed", "detect", "--base", base, "--head", "HEAD"],
            text=True,
        ).splitlines()
    if changed != ["scripts/helper.py"]:
        raise AssertionError(f"changed paths must come from the named input set, got {changed!r}")


def assert_backtester_sets_cover_cache_and_detector_inputs() -> None:
    module = load_module()
    config = module.load_config(REPO_ROOT / "ci" / "rust-ci-inputs.toml")
    cache = set(module.resolve_set(config, "backtester_cache"))
    detect = set(module.resolve_set(config, "backtester_detect"))
    for required in {
        "Cargo.lock",
        "Cargo.toml",
        ".gitignore",
        "build.rs",
        "gated_source_roots.manifest",
        "src/**",
        "tests/**",
        "specs/023-nt-research-analytics-platform/reference/**",
        "crates/backtesting-vertical-slice/Cargo.lock",
        "crates/backtesting-vertical-slice/Cargo.toml",
        "crates/backtesting-vertical-slice/src/**",
        "crates/backtesting-vertical-slice/tests/**",
        "scripts/rust_test_targets.py",
    }:
        if required not in cache:
            raise AssertionError(f"backtester_cache missing {required}")
    for required in {
        "scripts/ci_provenance.py",
        "ci/github-actions-runners.toml",
        ".github/workflows/backtester-ci.yml",
        "scripts/rust_test_targets.py",
    }:
        if required not in detect:
            raise AssertionError(f"backtester_detect missing {required}")


def main() -> int:
    assert_set_expansion_is_recursive_and_stable()
    assert_hash_changes_when_tracked_inputs_change()
    assert_hash_changes_when_exact_input_is_deleted()
    assert_changed_reports_deleted_exact_inputs()
    assert_changed_uses_named_input_set()
    assert_backtester_sets_cover_cache_and_detector_inputs()
    print("OK: CI input set self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
