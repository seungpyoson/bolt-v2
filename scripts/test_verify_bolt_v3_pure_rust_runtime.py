#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 pure Rust runtime verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_pure_rust_runtime.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_pure_rust_runtime", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def test_collect_dependency_names_covers_workspace_and_target_tables() -> None:
    names = VERIFIER.collect_dependency_names(
        {
            "dependencies": {"serde": "1"},
            "workspace": {
                "dependencies": {"pyo3": "0.22"},
                "dev-dependencies": {"cpython": "0.7"},
            },
            "target": {
                "cfg(unix)": {
                    "build-dependencies": {"maturin": "1"},
                },
            },
        }
    )

    expected = {"serde", "pyo3", "cpython", "maturin"}
    missing = expected - names
    if missing:
        raise AssertionError(f"dependency scanner missed {sorted(missing)} from {sorted(names)}")


def test_cargo_manifest_paths_scan_nested_manifests_and_skip_managed_dirs() -> None:
    original_root = VERIFIER.REPO_ROOT
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text("[package]\nname = \"root\"\n", encoding="utf-8")
        nested = root / "crates" / "probe"
        nested.mkdir(parents=True)
        (nested / "Cargo.toml").write_text("[package]\nname = \"probe\"\n", encoding="utf-8")
        ignored = root / "target" / "probe"
        ignored.mkdir(parents=True)
        (ignored / "Cargo.toml").write_text("[package]\nname = \"ignored\"\n", encoding="utf-8")

        try:
            VERIFIER.REPO_ROOT = root
            paths = {path.relative_to(root).as_posix() for path in VERIFIER.cargo_manifest_paths()}
        finally:
            VERIFIER.REPO_ROOT = original_root

    expected = {"Cargo.toml", "crates/probe/Cargo.toml"}
    if paths != expected:
        raise AssertionError(f"unexpected manifest paths: expected {sorted(expected)}, got {sorted(paths)}")


def test_forbidden_rust_patterns_detect_python_bridge_shapes() -> None:
    source = """
    #[pyclass]
    struct Bridge;

    fn bridge() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|_| {});
        let _: PyResult<()> = Ok(());
    }
    """

    labels = {
        label
        for pattern, label in VERIFIER.FORBIDDEN_RUST_PATTERNS
        if pattern.search(source)
    }
    expected = {
        "PyO3 Rust API usage",
        "Python export attribute",
        "Python GIL runtime usage",
        "Python object/result type",
    }
    missing = expected - labels
    if missing:
        raise AssertionError(f"forbidden Rust scanner missed {sorted(missing)} from {sorted(labels)}")


def test_forbidden_rust_scan_ignores_comments_and_literals() -> None:
    source = r'''
    // pyo3::prepare_freethreaded_python();
    /* #[pyclass] struct NotCode; */
    const TEXT: &str = "PyResult and Python::with_gil are docs";
    const RAW: &[u8] = br#"cpython::Object"#;
    const CHAR: char = 'P';
    let lifetime: &'static str = "also ignored";

    fn bridge() {
        pyo3::prepare_freethreaded_python();
    }
    '''

    scan_text = VERIFIER.strip_rust_comments_and_literals(source)
    labels = [
        label
        for pattern, label in VERIFIER.FORBIDDEN_RUST_PATTERNS
        for _ in pattern.finditer(scan_text)
    ]
    if labels != ["PyO3 Rust API usage"]:
        raise AssertionError(f"unexpected labels after stripping comments/literals: {labels!r}")


def main() -> int:
    tests = [
        test_collect_dependency_names_covers_workspace_and_target_tables,
        test_cargo_manifest_paths_scan_nested_manifests_and_skip_managed_dirs,
        test_forbidden_rust_patterns_detect_python_bridge_shapes,
        test_forbidden_rust_scan_ignores_comments_and_literals,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 pure Rust verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
