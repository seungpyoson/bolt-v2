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


def allowance(path: str, excerpt: str):
    return VERIFIER.FindingAllowance(path=path, exact_excerpt=excerpt)


def test_clean_fixture_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "pub struct Clean;\n")
        code, _out, err = run_with(root, allowances=())
    if code != 0:
        raise AssertionError(f"expected clean fixture to pass, got {code}: {err}")


def test_new_back_reference_fails_with_line_number() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "\n\nuse crate::strategies::registry::FeeProvider;\n",
        )
        code, _out, err = run_with(root, allowances=())
    if code != 1:
        raise AssertionError(f"expected new back-reference to fail, got {code}")
    if "src/bolt_v3_foo.rs:3" not in err or "crate::strategies" not in err:
        raise AssertionError(f"unexpected stderr: {err!r}")


def test_allowance_suppresses_pre_existing_reference() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "use crate::strategies::registry::FeeProvider;\n",
        )
        code, _out, err = run_with(
            root,
            allowances=(
                allowance(
                    "src/bolt_v3_foo.rs",
                    "use crate::strategies::registry::FeeProvider;",
                ),
            ),
        )
    if code != 0:
        raise AssertionError(f"expected allowance to suppress, got {code}: {err}")


def test_stale_allowance_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "pub struct Clean;\n")
        code, _out, err = run_with(
            root,
            allowances=(
                allowance("src/bolt_v3_foo.rs", "use crate::strategies::registry::Gone;"),
            ),
        )
    if code != 1:
        raise AssertionError(f"expected stale allowance to fail, got {code}")
    if "stale allowance" not in err:
        raise AssertionError(f"expected stale-allowance message, got: {err!r}")


def test_strategy_layer_and_non_bolt_v3_not_scanned() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        # strategy layer may reference itself; a non-bolt_v3 module is out of scope.
        write_file(root, "src/strategies/foo.rs", "use crate::strategies::registry::X;\n")
        write_file(root, "src/other_module.rs", "use crate::strategies::registry::X;\n")
        code, _out, err = run_with(root, allowances=())
    if code != 0:
        raise AssertionError(f"expected out-of-scope files to be ignored, got {code}: {err}")


def test_commented_reference_ignored() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            '// use crate::strategies::registry::FeeProvider;\nlet u = "ok";\n',
        )
        code, _out, err = run_with(root, allowances=())
    if code != 0:
        raise AssertionError(f"expected commented reference to be ignored, got {code}: {err}")


def test_multiline_use_block_flagged_on_opening_line() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_operator_artifacts.rs",
            "use crate::strategies::binary_oracle_edge_taker::{\n    Foo,\n    Bar,\n};\n",
        )
        code, _out, err = run_with(root, allowances=())
    if code != 1:
        raise AssertionError(f"expected multi-line use block to fail, got {code}")
    if "src/bolt_v3_operator_artifacts.rs:1" not in err:
        raise AssertionError(f"expected opening-line hit, got: {err!r}")


def test_market_families_is_in_scope() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_market_families/updown.rs",
            "use crate::strategies::registry::X;\n",
        )
        code, _out, _err = run_with(root, allowances=())
    if code != 1:
        raise AssertionError(f"expected market_families to be scanned, got {code}")


def test_real_repo_is_green_with_committed_allowlist() -> None:
    # Guard: the committed FINDING_ALLOWANCES must match the actual source tree.
    code, _out, err = run_with(VERIFIER.REPO_ROOT)
    if code != 0:
        raise AssertionError(
            f"committed allowlist does not match the real tree (got {code}): {err}"
        )


def main() -> int:
    tests = [
        test_clean_fixture_passes,
        test_new_back_reference_fails_with_line_number,
        test_allowance_suppresses_pre_existing_reference,
        test_stale_allowance_fails,
        test_strategy_layer_and_non_bolt_v3_not_scanned,
        test_commented_reference_ignored,
        test_multiline_use_block_flagged_on_opening_line,
        test_market_families_is_in_scope,
        test_real_repo_is_green_with_committed_allowlist,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 dependency-direction verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
