#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 pure-Rust runtime verifier."""

from __future__ import annotations

import importlib.util
import contextlib
import io
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


def run_main_with_temp_root(
    root: Path,
    main_rs: Path,
    runtime_source_paths: tuple[str, ...] | None = None,
) -> tuple[int, str]:
    original_root = VERIFIER.REPO_ROOT
    original_main_rs = VERIFIER.MAIN_RS
    original_runtime_source_paths = VERIFIER.RUNTIME_SOURCE_PATHS
    stderr = io.StringIO()
    try:
        VERIFIER.REPO_ROOT = root
        VERIFIER.MAIN_RS = main_rs
        if runtime_source_paths is not None:
            VERIFIER.RUNTIME_SOURCE_PATHS = runtime_source_paths
        with contextlib.redirect_stderr(stderr):
            code = VERIFIER.main()
    finally:
        VERIFIER.REPO_ROOT = original_root
        VERIFIER.MAIN_RS = original_main_rs
        VERIFIER.RUNTIME_SOURCE_PATHS = original_runtime_source_paths
    return code, stderr.getvalue()


def entrypoint_text() -> str:
    return "\n".join(f"{call}();" for call in VERIFIER.MAIN_RS_ENTRYPOINT_CALLS)


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


def test_main_fails_closed_when_manifest_discovery_is_empty() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source = root / "src" / "main.rs"
        source.parent.mkdir(parents=True)
        source.write_text(entrypoint_text(), encoding="utf-8")
        (root / "Cargo.lock").write_text("", encoding="utf-8")
        code, stderr = run_main_with_temp_root(root, source)

    expected = "FAIL: Cargo manifests: enforcement set is empty\n"
    if code != 1 or stderr != expected:
        raise AssertionError(f"expected empty manifest floor, got code={code}, stderr={stderr!r}")


def test_main_fails_closed_when_runtime_sources_are_empty() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text("[package]\nname = \"probe\"\n", encoding="utf-8")
        (root / "Cargo.lock").write_text("", encoding="utf-8")
        main_rs = root / "main.rs"
        main_rs.write_text(entrypoint_text(), encoding="utf-8")
        code, stderr = run_main_with_temp_root(root, main_rs, runtime_source_paths=())

    expected = (
        "Rust source files under src: enforcement set is empty",
        "Bolt-v3 runtime source paths: enforcement set is empty",
    )
    if code != 1 or any(text not in stderr for text in expected):
        raise AssertionError(f"expected empty runtime floors, got code={code}, stderr={stderr!r}")


def test_main_reports_source_floor_without_main_rs_crash() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text("[package]\nname = \"probe\"\n", encoding="utf-8")
        (root / "Cargo.lock").write_text("", encoding="utf-8")
        code, stderr = run_main_with_temp_root(
            root,
            root / "src" / "main.rs",
            runtime_source_paths=(),
        )

    expected = (
        "Rust source files under src: enforcement set is empty",
        "Bolt-v3 runtime source paths: enforcement set is empty",
    )
    if code != 1 or any(text not in stderr for text in expected):
        raise AssertionError(f"expected empty runtime floors, got code={code}, stderr={stderr!r}")


def test_main_reports_missing_listed_runtime_source() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text("[package]\nname = \"probe\"\n", encoding="utf-8")
        (root / "Cargo.lock").write_text("", encoding="utf-8")
        src = root / "src"
        src.mkdir(parents=True)
        (src / "clean.rs").write_text("pub struct Clean;\n", encoding="utf-8")
        main_rs = root / "main.rs"
        main_rs.write_text(entrypoint_text(), encoding="utf-8")
        code, stderr = run_main_with_temp_root(
            root,
            main_rs,
            runtime_source_paths=("src/missing.rs",),
        )

    if code != 1 or "src/missing.rs: runtime source file is missing" not in stderr:
        raise AssertionError(f"expected missing runtime source finding, got code={code}, stderr={stderr!r}")


def test_cargo_manifest_paths_matches_rglob_reference_with_pruned_subtrees() -> None:
    original_root = VERIFIER.REPO_ROOT
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text("[package]\nname = \"root\"\n", encoding="utf-8")
        probe = root / "crates" / "probe"
        probe.mkdir(parents=True)
        (probe / "Cargo.toml").write_text("[package]\nname = \"probe\"\n", encoding="utf-8")
        nested = root / "crates" / "probe" / "nested" / "real"
        nested.mkdir(parents=True)
        (nested / "Cargo.toml").write_text("[package]\nname = \"real\"\n", encoding="utf-8")

        ignored_root_target = root / "target" / "probe"
        ignored_root_target.mkdir(parents=True)
        (ignored_root_target / "Cargo.toml").write_text(
            "[package]\nname = \"ignored-root-target\"\n",
            encoding="utf-8",
        )
        ignored_nested_target = root / "crates" / "probe" / "target"
        ignored_nested_target.mkdir(parents=True)
        (ignored_nested_target / "Cargo.toml").write_text(
            "[package]\nname = \"ignored-nested-target\"\n",
            encoding="utf-8",
        )
        ignored_git = root / ".git"
        ignored_git.mkdir()
        (ignored_git / "Cargo.toml").write_text("[package]\nname = \"ignored-git\"\n", encoding="utf-8")
        ignored_worktree = root / ".worktrees" / "wt1"
        ignored_worktree.mkdir(parents=True)
        (ignored_worktree / "Cargo.toml").write_text(
            "[package]\nname = \"ignored-worktree\"\n",
            encoding="utf-8",
        )
        directory_named_manifest = root / "crates" / "weird" / "Cargo.toml"
        directory_named_manifest.mkdir(parents=True)

        reference = sorted(
            p
            for p in root.rglob("Cargo.toml")
            if p.is_file() and not (set(p.relative_to(root).parts) & VERIFIER.IGNORED_MANIFEST_DIRS)
        )

        try:
            VERIFIER.REPO_ROOT = root
            paths = VERIFIER.cargo_manifest_paths()
        finally:
            VERIFIER.REPO_ROOT = original_root

    if paths != reference:
        raise AssertionError(
            f"unexpected manifest paths: expected {[p.as_posix() for p in reference]}, "
            f"got {[p.as_posix() for p in paths]}"
        )
    if directory_named_manifest in paths:
        raise AssertionError(f"directory named Cargo.toml was returned: {directory_named_manifest}")


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


def test_main_rs_entrypoint_calls_pass_when_present_and_flag_missing() -> None:
    # Passing fixture: a src/main.rs body that contains every required
    # bolt-v3 entrypoint call-site.
    passing_main_rs = "\n".join(
        f"        {call}" for call in VERIFIER.MAIN_RS_ENTRYPOINT_CALLS
    )
    if VERIFIER.missing_main_rs_entrypoint_calls(passing_main_rs):
        raise AssertionError(
            "expected no findings when every entrypoint call is present; got "
            f"{VERIFIER.missing_main_rs_entrypoint_calls(passing_main_rs)!r}"
        )

    # Failing fixture: identical, but with one entrypoint call-site removed.
    dropped_call = VERIFIER.MAIN_RS_ENTRYPOINT_CALLS[1]
    broken_main_rs = passing_main_rs.replace(dropped_call, "")
    findings = VERIFIER.missing_main_rs_entrypoint_calls(broken_main_rs)
    if len(findings) != 1:
        raise AssertionError(f"expected exactly one missing-call finding; got {findings!r}")
    assert_contains(
        "\n".join(findings),
        f"src/main.rs is missing entrypoint call {dropped_call!r}",
    )


def test_main_rs_entrypoint_calls_ignore_comments_and_literals() -> None:
    commented_and_literal_calls = "\n".join(
        [
            f"// {VERIFIER.MAIN_RS_ENTRYPOINT_CALLS[0]}",
            f'const DECOY: &str = "{VERIFIER.MAIN_RS_ENTRYPOINT_CALLS[1]}";',
            f"/* {VERIFIER.MAIN_RS_ENTRYPOINT_CALLS[2]} */",
        ]
    )
    findings = VERIFIER.missing_main_rs_entrypoint_calls(commented_and_literal_calls)
    if len(findings) != len(VERIFIER.MAIN_RS_ENTRYPOINT_CALLS):
        raise AssertionError(
            "commented/string-literal entrypoint calls must not satisfy src/main.rs "
            f"checks; got {findings!r}"
        )


def main() -> int:
    tests = [
        test_collect_dependency_names_covers_workspace_and_target_tables,
        test_main_fails_closed_when_manifest_discovery_is_empty,
        test_main_fails_closed_when_runtime_sources_are_empty,
        test_main_reports_source_floor_without_main_rs_crash,
        test_main_reports_missing_listed_runtime_source,
        test_cargo_manifest_paths_matches_rglob_reference_with_pruned_subtrees,
        test_cargo_manifest_paths_scan_nested_manifests_and_skip_managed_dirs,
        test_forbidden_rust_patterns_detect_python_bridge_shapes,
        test_forbidden_rust_scan_ignores_comments_and_literals,
        test_cfg_test_items_are_ignored_but_production_items_remain,
        test_runtime_subprocess_detection_survives_comments_literals_and_cfg_fixtures,
        test_main_rs_entrypoint_calls_pass_when_present_and_flag_missing,
        test_main_rs_entrypoint_calls_ignore_comments_and_literals,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 pure-Rust verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
