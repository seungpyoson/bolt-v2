#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 dependency-direction verifier."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_dependency_direction.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_dependency_direction", SCRIPT_PATH
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def write_file(root: Path, rel: str, content: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def run_with(root: Path, allowances: tuple | None = None) -> tuple[int, str, str]:
    original_root = VERIFIER.REPO_ROOT
    original_allow = VERIFIER.FINDING_ALLOWANCES
    stdout = io.StringIO()
    stderr = io.StringIO()
    try:
        VERIFIER.REPO_ROOT = root
        if allowances is not None:
            VERIFIER.FINDING_ALLOWANCES = allowances
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = VERIFIER.main()
    finally:
        VERIFIER.REPO_ROOT = original_root
        VERIFIER.FINDING_ALLOWANCES = original_allow
    return code, stdout.getvalue(), stderr.getvalue()


def allowance(path: str, strategy_path: str):
    return VERIFIER.FindingAllowance(path=path, strategy_path=strategy_path)


def expect_fail(root: Path, allowances=()) -> str:
    code, _out, err = run_with(root, allowances=allowances)
    if code != 1:
        raise AssertionError(f"expected FAIL (1), got {code}: {err}")
    return err


def expect_pass(root: Path, allowances=()) -> None:
    code, _out, err = run_with(root, allowances=allowances)
    if code != 0:
        raise AssertionError(f"expected PASS (0), got {code}: {err}")


def test_clean_fixture_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "pub struct Clean;\n")
        expect_pass(root)


def test_new_back_reference_fails_with_line_number() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "\n\nuse crate::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "src/bolt_v3_foo.rs:3" not in err or "crate::strategies" not in err:
            raise AssertionError(f"unexpected stderr: {err!r}")


def test_allowance_suppresses_pre_existing_reference() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use crate::strategies::registry::FeeProvider;\n",
        )
        expect_pass(
            root,
            allowances=(
                allowance("src/bolt_v3_foo.rs", "strategies::registry::FeeProvider"),
            ),
        )


def test_stale_allowance_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "pub struct Clean;\n")
        err = expect_fail(
            root,
            allowances=(allowance("src/bolt_v3_foo.rs", "strategies::registry::Gone"),),
        )
        if "stale allowance" not in err:
            raise AssertionError(f"expected stale-allowance message, got: {err!r}")


def test_strategy_layer_and_non_bolt_v3_not_scanned() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/strategies/foo.rs", "use crate::strategies::registry::X;\n")
        write_file(root, "src/other_module.rs", "use crate::strategies::registry::X;\n")
        expect_pass(root)


def test_commented_reference_ignored() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            '// use crate::strategies::registry::FeeProvider;\nlet u = "ok";\n',
        )
        expect_pass(root)


def test_block_comment_reference_ignored() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "/* use crate::strategies::registry::FeeProvider; */\npub struct A;\n",
        )
        expect_pass(root)


def test_block_comment_stripping_preserves_following_line_numbers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "/*\nuse crate::strategies::registry::Ignored;\n*/\n"
            "use crate::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "src/bolt_v3_foo.rs:4" not in err:
            raise AssertionError(f"expected following import on line 4, got: {err!r}")


def test_grouped_import_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use crate::{strategies::registry::FeeProvider, other::Thing};\n",
        )
        expect_fail(root)


def test_multiline_grouped_import_flagged_on_opening_line() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_operator_artifacts.rs",
            "use crate::{\n    other::Thing,\n    strategies::registry::FeeProvider,\n};\n",
        )
        err = expect_fail(root)
        if "src/bolt_v3_operator_artifacts.rs:1" not in err:
            raise AssertionError(f"expected opening-line hit, got: {err!r}")


def test_nested_grouped_import_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use crate::strategies::{registry::FeeProvider, binary_oracle_edge_taker::KEY};\n",
        )
        expect_fail(root)


def test_aliased_import_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use crate::strategies::registry::FeeProvider as Fp;\n",
        )
        expect_fail(root)


def test_super_path_from_top_level_module_flagged() -> None:
    # In a top-level src/bolt_v3_foo.rs, `super` is the crate root, so
    # `super::strategies` IS the strategy layer.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use super::strategies::registry::FeeProvider;\n",
        )
        expect_fail(root)


def test_super_path_from_nested_module_not_flagged() -> None:
    # In src/bolt_v3_providers/polymarket/fees.rs, `super::strategies` resolves to
    # crate::bolt_v3_providers::polymarket::strategies — NOT the strategy layer.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_providers/polymarket/fees.rs",
            "use super::strategies::Thing;\n",
        )
        expect_pass(root)


def test_external_strategies_crate_not_flagged() -> None:
    # A bare root (`strategies::...`) is an external crate, not the local layer.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "use strategies::registry::X;\n")
        expect_pass(root)


def test_market_families_is_in_scope() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_market_families/updown.rs",
            "use crate::strategies::registry::X;\n",
        )
        expect_fail(root)


def test_real_repo_is_green_with_committed_allowlist() -> None:
    # Guard: the committed FINDING_ALLOWANCES must match the actual source tree.
    code, _out, err = run_with(VERIFIER.REPO_ROOT)
    if code != 0:
        raise AssertionError(
            f"committed allowlist does not match the real tree (got {code}): {err}"
        )


def test_file_size_limit_exceeded_fails_cleanly() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            " " * (VERIFIER.MAX_SCAN_FILE_BYTES + 1),
        )
        code, _out, err = run_with(root, allowances=())
    if code != 1:
        raise AssertionError(f"expected oversized source file to fail, got {code}")
    if "exceeds 1 MiB limit" not in err:
        raise AssertionError(f"expected size-limit diagnostic, got: {err!r}")
    if "Traceback" in err:
        raise AssertionError(f"expected clean PolicyError handling, got traceback: {err!r}")


def main() -> int:
    tests = [
        test_clean_fixture_passes,
        test_new_back_reference_fails_with_line_number,
        test_allowance_suppresses_pre_existing_reference,
        test_stale_allowance_fails,
        test_strategy_layer_and_non_bolt_v3_not_scanned,
        test_commented_reference_ignored,
        test_block_comment_reference_ignored,
        test_block_comment_stripping_preserves_following_line_numbers,
        test_grouped_import_flagged,
        test_multiline_grouped_import_flagged_on_opening_line,
        test_nested_grouped_import_flagged,
        test_aliased_import_flagged,
        test_super_path_from_top_level_module_flagged,
        test_super_path_from_nested_module_not_flagged,
        test_external_strategies_crate_not_flagged,
        test_market_families_is_in_scope,
        test_real_repo_is_green_with_committed_allowlist,
        test_file_size_limit_exceeded_fails_cleanly,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 dependency-direction verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
