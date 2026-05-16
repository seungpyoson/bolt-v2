#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 pure-Rust runtime verifier."""

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


def assert_contains(text: str, needle: str) -> None:
    if needle not in text:
        raise AssertionError(f"missing expected text: {needle!r}\n{text}")


def assert_not_contains(text: str, needle: str) -> None:
    if needle in text:
        raise AssertionError(f"unexpected text: {needle!r}\n{text}")


def assert_forbidden_runtime_source_detected(text: str, label: str) -> None:
    labels = [
        pattern_label
        for pattern, pattern_label in VERIFIER.FORBIDDEN_RUNTIME_SOURCE_PATTERNS
        if pattern.search(text)
    ]
    if label not in labels:
        raise AssertionError(f"missing forbidden runtime-source label {label!r}; got {labels!r}")


def assert_production_source_detected(source: str, label: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "source.rs"
        path.write_text(source, encoding="utf-8")
        assert_forbidden_runtime_source_detected(VERIFIER.production_text(path), label)


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


def test_cfg_test_items_are_ignored_but_production_items_remain() -> None:
    stripped = VERIFIER.strip_cfg_test_items(
        """
#[cfg(test)]
impl SecretError {
    fn for_test() {
        std::process::Command::new("aws");
    }
}

fn production_resolver() {
    std::process::Command::new("python3");
}

#[cfg(test)]
mod tests {
    fn helper() {
        std::process::Command::new("aws");
    }
}

#[cfg(all(test, feature = "fixture"))]
fn complex_test_helper() {
    std::process::Command::new("aws");
}

#[cfg(any(test))]
fn any_test_helper() {
    std::process::Command::new("aws");
}

#[cfg(any(test, unix))]
fn production_cfg_helper() {
    std::process::Command::new("python3");
}

fn production_tail() {
    std::process::Command::new("aws");
}
""".lstrip()
    )

    assert_not_contains(stripped, "fn for_test()")
    assert_not_contains(stripped, "mod tests")
    assert_not_contains(stripped, "fn complex_test_helper()")
    assert_not_contains(stripped, "fn any_test_helper()")
    assert_contains(stripped, "fn production_cfg_helper()")
    assert_contains(stripped, 'std::process::Command::new("python3")')
    assert_contains(stripped, 'std::process::Command::new("aws")')


def test_runtime_subprocess_detection_survives_comments_literals_and_cfg_fixtures() -> None:
    assert_forbidden_runtime_source_detected(
        """
fn production_subprocess(binary: &str) {
    std::process::Command::new(binary);
}
""",
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
fn production_subprocess_after_url() {
    let _endpoint = "http://example.invalid"; std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
/*
#[cfg(test)]
*/
fn production_subprocess_after_block_comment() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
fn fixture_text() -> &'static str {
    "
#[cfg(test)]
    "
}

fn production_subprocess_after_string_literal() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
struct FixtureFields {
    live_field: i32,
    #[cfg(test)]
    fixture_field: i32,
}

fn production_subprocess_after_cfg_field() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
struct FixtureFields {
    live_field: i32,
    #[cfg(test)]
    fixture_field: i32
}

fn production_subprocess_after_final_cfg_field() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
enum FixtureVariants {
    LiveVariant,
    #[cfg(test)]
    FixtureVariant,
}

fn production_subprocess_after_cfg_variant() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
enum FixtureVariants {
    LiveVariant,
    #[cfg(test)]
    FixtureVariant
}

fn production_subprocess_after_final_cfg_variant() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )
    assert_production_source_detected(
        r'''
#[cfg(test)]
const FIXTURE_BRACE: &str = "{";

fn production_subprocess_after_cfg_string_brace() {
    std::process::Command::new("python3");
}
''',
        "runtime subprocess",
    )


def main() -> int:
    tests = [
        test_collect_dependency_names_covers_workspace_and_target_tables,
        test_cargo_manifest_paths_scan_nested_manifests_and_skip_managed_dirs,
        test_forbidden_rust_patterns_detect_python_bridge_shapes,
        test_forbidden_rust_scan_ignores_comments_and_literals,
        test_cfg_test_items_are_ignored_but_production_items_remain,
        test_runtime_subprocess_detection_survives_comments_literals_and_cfg_fixtures,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 pure-Rust verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
