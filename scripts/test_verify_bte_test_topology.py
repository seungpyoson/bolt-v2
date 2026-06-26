#!/usr/bin/env python3
"""Self-tests for the backtester test topology verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import tempfile

SCRIPT_PATH = pathlib.Path(__file__).with_name("verify_bte_test_topology.py")


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_test_topology", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("failed to load verifier")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def write_good_fixture(root: pathlib.Path) -> None:
    write(
        root / "crates/backtesting-vertical-slice/Cargo.toml",
        """[package]
name = "backtesting-vertical-slice"
version = "0.0.0"
edition = "2024"
autotests = false

[[test]]
name = "backtesting_vertical_slice_tests"
path = "tests/backtesting_vertical_slice_tests.rs"
""",
    )
    write(root / "crates/backtesting-vertical-slice/tests/a_contract.rs", "#[test]\nfn a() {}\n")
    write(root / "crates/backtesting-vertical-slice/tests/b_contract.rs", "#[test]\nfn b() {}\n")
    write(
        root / "crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs",
        """#![recursion_limit = "256"]

#[path = "a_contract.rs"]
mod a_contract;
#[path = "b_contract.rs"]
mod b_contract;
""",
    )
    write(
        root / "crates/backtesting-vertical-slice/src/source_proof.rs",
        """pub struct AcceptedDataset {
    pub(crate) source_proof_id: String,
}

pub(crate) fn synthetic_accepted_dataset_for_tests() -> AcceptedDataset {
    AcceptedDataset { source_proof_id: String::new() }
}

impl AcceptedDataset {
    pub(crate) fn result_contract_claim_limits(&self) -> Vec<String> {
        Vec::new()
    }
}

pub fn select_accepted_dataset() -> AcceptedDataset {
    synthetic_accepted_dataset_for_tests()
}
""",
    )


def test_good_fixture_is_clean() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write_good_fixture(root)
        assert verifier.verify_root(root) == []


def test_missing_harness_shape_is_reported() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write(root / "crates/backtesting-vertical-slice/Cargo.toml", "[package]\nname = \"x\"\n")
        errors = verifier.verify_root(root)
        assert "backtester Cargo.toml must set package.autotests = false" in errors
        assert any("explicit integration test harness" in error for error in errors)
        assert any("backtesting_vertical_slice_tests.rs must exist" in error for error in errors)


def test_missing_module_is_reported() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write_good_fixture(root)
        harness = root / "crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs"
        harness.write_text(harness.read_text().replace('#[path = "b_contract.rs"]\nmod b_contract;\n', ""))
        errors = verifier.verify_root(root)
        assert any("missing b_contract.rs" in error for error in errors), errors


def test_inner_attrs_must_move_to_harness() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write_good_fixture(root)
        write(root / "crates/backtesting-vertical-slice/tests/a_contract.rs", "#![recursion_limit = \"256\"]\n")
        errors = verifier.verify_root(root)
        assert any("must not keep crate-level attributes outside the harness" in error for error in errors), errors


def test_accepted_dataset_fields_must_not_be_public() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write_good_fixture(root)
        source_proof = root / "crates/backtesting-vertical-slice/src/source_proof.rs"
        source_proof.write_text(source_proof.read_text().replace("pub(crate) source_proof_id", "pub source_proof_id"))
        errors = verifier.verify_root(root)
        assert any("fields must stay non-public" in error for error in errors), errors


def test_accepted_dataset_impl_must_not_expose_public_constructors() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write_good_fixture(root)
        source_proof = root / "crates/backtesting-vertical-slice/src/source_proof.rs"
        source_proof.write_text(
            source_proof.read_text().replace(
                "pub(crate) fn result_contract_claim_limits",
                "pub fn result_contract_claim_limits",
            )
        )
        errors = verifier.verify_root(root)
        assert any("must not expose public constructors or mutators" in error for error in errors), errors


def main() -> int:
    tests = [
        test_good_fixture_is_clean,
        test_missing_harness_shape_is_reported,
        test_missing_module_is_reported,
        test_inner_attrs_must_move_to_harness,
        test_accepted_dataset_fields_must_not_be_public,
        test_accepted_dataset_impl_must_not_expose_public_constructors,
    ]
    for test in tests:
        test()
    print("OK: backtester test topology verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
