#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_test_targets.py"


def load_module():
    spec = importlib.util.spec_from_file_location("rust_test_targets", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("failed to load rust_test_targets.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def fixture_crate(root: Path) -> Path:
    crate = root / "crates" / "example-crate"
    write(
        crate / "Cargo.toml",
        """
[package]
name = "example-crate"
version = "0.0.0"
edition = "2024"
autotests = false

[[test]]
name = "example_tests"
path = "tests/example_tests.rs"

[[test]]
name = "second_harness"
path = "tests/second_harness.rs"
""",
    )
    write(crate / "src" / "lib.rs", "pub fn lib() {}\n")
    write(
        crate / "src" / "main.rs",
        """
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn main_bin_unit_test_runs() {}
}
""",
    )
    write(
        crate / "src" / "bin" / "source_universe_batch_execution.rs",
        """
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn source_bin_unit_test_runs() {}
}
""",
    )
    write(crate / "src" / "bin" / "untested_operator.rs", "fn main() {}\n")
    write(crate / "src" / "bin" / "nested_runner" / "main.rs", "fn main() {}\n#[test]\nfn nested_runs() {}\n")
    write(
        crate / "tests" / "example_tests.rs",
        """
#[test]
fn uses_sidecars() {
    let _ = std::env::var("CARGO_BIN_EXE_first_proof_selector");
    let _ = std::env::var("CARGO_BIN_EXE_source_proof_admissibility");
}
""",
    )
    write(
        crate / "tests" / "second_harness.rs",
        """
#[test]
fn uses_more_sidecars() {
    let _ = std::env::var("CARGO_BIN_EXE_first_proof_selector");
}
""",
    )
    return crate


def assert_archive_args_are_derived_from_cargo_and_test_bearing_bins() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        crate = fixture_crate(Path(tmp))
        actual = module.archive_args(crate)
    expected = [
        "--lib",
        "--test",
        "example_tests",
        "--test",
        "second_harness",
        "--bin",
        "example-crate",
        "--bin",
        "nested_runner",
        "--bin",
        "source_universe_batch_execution",
    ]
    if actual != expected:
        raise AssertionError(f"archive args drifted:\nexpected={expected!r}\nactual={actual!r}")


def assert_sidecars_are_derived_from_cargo_bin_exe_references() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        crate = fixture_crate(Path(tmp))
        actual = module.sidecars(crate)
    expected = ["first_proof_selector", "source_proof_admissibility"]
    if actual != expected:
        raise AssertionError(f"sidecars drifted: expected={expected!r} actual={actual!r}")


def assert_cli_matches_library_output() -> None:
    with TemporaryDirectory() as tmp:
        crate = fixture_crate(Path(tmp))
        archive = subprocess.check_output(
            [sys.executable, str(SCRIPT), "archive-args", "--crate", str(crate)],
            text=True,
        ).splitlines()
        sidecar_names = subprocess.check_output(
            [sys.executable, str(SCRIPT), "sidecars", "--crate", str(crate)],
            text=True,
        ).splitlines()
    if "--bin" not in archive or "source_universe_batch_execution" not in archive:
        raise AssertionError(f"archive CLI did not include discovered bin target: {archive!r}")
    if sidecar_names != ["first_proof_selector", "source_proof_admissibility"]:
        raise AssertionError(f"sidecar CLI mismatch: {sidecar_names!r}")


def assert_conventional_crates_do_not_need_bvs_layout() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        crate = Path(tmp) / "plain-crate"
        write(
            crate / "Cargo.toml",
            """
[package]
name = "plain-crate"
version = "0.0.0"
edition = "2024"
""",
        )
        write(crate / "src" / "main.rs", "fn main() {}\n#[test]\nfn main_test_runs() {}\n")
        write(crate / "tests" / "cli_contract.rs", "#[test]\nfn cli_contract_runs() {}\n")
        actual = module.archive_args(crate)
    expected = ["--test", "cli_contract", "--bin", "plain-crate"]
    if actual != expected:
        raise AssertionError(f"conventional crate target discovery drifted: expected={expected!r} actual={actual!r}")


def assert_explicit_bins_can_use_cargo_default_paths() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        crate = Path(tmp) / "explicit-bin-crate"
        write(
            crate / "Cargo.toml",
            """
[package]
name = "explicit-bin-crate"
version = "0.0.0"
edition = "2024"
autobins = false

[[bin]]
name = "explicit_runner"
""",
        )
        write(crate / "src" / "bin" / "explicit_runner.rs", "fn main() {}\n#[test]\nfn runner_test_runs() {}\n")
        actual = module.archive_args(crate)
    expected = ["--bin", "explicit_runner"]
    if actual != expected:
        raise AssertionError(f"default explicit-bin path discovery drifted: expected={expected!r} actual={actual!r}")


def assert_autolib_false_disables_conventional_lib_target() -> None:
    module = load_module()
    with TemporaryDirectory() as tmp:
        crate = Path(tmp) / "no-autolib-crate"
        write(
            crate / "Cargo.toml",
            """
[package]
name = "no-autolib-crate"
version = "0.0.0"
edition = "2024"
autolib = false
""",
        )
        write(crate / "src" / "lib.rs", "pub fn lib() {}\n")
        actual = module.archive_args(crate)
    expected: list[str] = []
    if actual != expected:
        raise AssertionError(f"autolib=false target discovery drifted: expected={expected!r} actual={actual!r}")


def main() -> int:
    assert_archive_args_are_derived_from_cargo_and_test_bearing_bins()
    assert_sidecars_are_derived_from_cargo_bin_exe_references()
    assert_cli_matches_library_output()
    assert_conventional_crates_do_not_need_bvs_layout()
    assert_explicit_bins_can_use_cargo_default_paths()
    assert_autolib_false_disables_conventional_lib_target()
    print("OK: Rust test target discovery self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
