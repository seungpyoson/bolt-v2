#!/usr/bin/env python3
"""Self-tests for the no-mistakes Rust routing verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "scripts" / "verify_no_mistakes_rust_routing.py"


def load_module():
    spec = importlib.util.spec_from_file_location("verify_no_mistakes_rust_routing", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load verify_no_mistakes_rust_routing.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_current_no_mistakes_config_does_not_launch_local_cargo():
    module = load_module()

    errors = module.validate_no_mistakes_config(REPO_ROOT / ".no-mistakes.yaml")

    assert errors == []


def test_verifier_rejects_raw_cargo_commands():
    module = load_module()
    with tempfile.TemporaryDirectory() as tmp:
        config = pathlib.Path(tmp) / ".no-mistakes.yaml"
        config.write_text(
            textwrap.dedent(
                """
                commands:
                  test: "cargo test"
                  lint: "cargo clippy --all-targets -- -D warnings"
                  format: "cargo fmt --check"
                """
            ),
            encoding="utf-8",
        )

        errors = module.validate_no_mistakes_config(config)

        assert any("commands.test" in error for error in errors)
        assert any("launches raw Cargo" in error for error in errors)


def main() -> int:
    test_current_no_mistakes_config_does_not_launch_local_cargo()
    test_verifier_rejects_raw_cargo_commands()
    print("OK: no-mistakes Rust routing verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
