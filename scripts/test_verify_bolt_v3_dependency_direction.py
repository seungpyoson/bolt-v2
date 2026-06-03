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


def test_inline_type_annotation_path_flagged() -> None:
    # codex finding: a fully-qualified inline path with NO `use` must be caught.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "\n\nfn f() {\n    let p: crate::strategies::registry::FeeProvider = make();\n}\n",
        )
        err = expect_fail(root)
        if "src/bolt_v3_foo.rs:4" not in err or "strategies::registry::FeeProvider" not in err:
            raise AssertionError(f"unexpected stderr: {err!r}")


def test_inline_call_path_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() { crate::strategies::production_strategy_registry(); }\n",
        )
        expect_fail(root)


def test_inline_macro_arg_path_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() { log!(\"{}\", crate::strategies::binary_oracle_edge_taker::KEY); }\n",
        )
        expect_fail(root)


def test_inline_turbofish_path_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() { make::<crate::strategies::registry::StrategyBuilder>(); }\n",
        )
        expect_fail(root)


def test_inline_super_path_from_top_level_flagged() -> None:
    # `super::strategies::X` inline (no `use`) from a top-level bolt_v3_* module.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() -> super::strategies::registry::FeeProvider { todo!() }\n",
        )
        expect_fail(root)


def test_attribute_inside_brace_import_flagged() -> None:
    # gemini finding: `#[cfg(...)]` before a member inside a brace group must not
    # corrupt resolution and let the strategy import slip past.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "use crate::{\n    other::Thing,\n    #[cfg(test)] strategies::registry::FeeProvider,\n};\n",
        )
        expect_fail(root)


def test_glob_import_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "use crate::strategies::registry::*;\n")
        expect_fail(root)


def test_raw_string_containing_path_not_flagged() -> None:
    # grok finding: a raw string literal that merely contains the text
    # `crate::strategies` (incl. an interior quote) must NOT be a false positive.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            'pub const DOC: &str = r#"use crate::strategies::registry::FeeProvider; "x""#;\n',
        )
        expect_pass(root)


def test_string_literal_containing_path_not_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            'pub const D: &str = "see crate::strategies::registry::FeeProvider";\n',
        )
        expect_pass(root)


def test_byte_and_raw_byte_strings_with_path_not_flagged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            'const A: &[u8] = b"crate::strategies::X";\n'
            'const B: &[u8] = br#"crate::strategies::Y"#;\n',
        )
        expect_pass(root)


def test_lifetime_does_not_hide_following_import() -> None:
    # grok finding: char/lifetime mishandling must not desync the lexer and hide a
    # real import that follows.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "struct W<'a>(&'a str);\nfn g(c: char) { let _ = '\\''; let _ = 'z'; }\n"
            "use crate::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "src/bolt_v3_foo.rs:3" not in err:
            raise AssertionError(f"expected import on line 3, got: {err!r}")


def test_nested_block_comment_ignored() -> None:
    # Rust block comments nest: the inner `*/` must not end the comment early and
    # expose the commented-out import.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "/* outer /* inner */ use crate::strategies::registry::FeeProvider; */\n"
            "pub struct Clean;\n",
        )
        expect_pass(root)


def test_inline_path_allowance_suppresses() -> None:
    # An inline (non-use) reference to a bare symbol is keyed and allowlisted
    # identically to a `use` of the same symbol.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() { let _ = crate::strategies::binary_oracle_edge_taker::KEY; }\n",
        )
        expect_pass(
            root,
            allowances=(
                allowance(
                    "src/bolt_v3_foo.rs",
                    "strategies::binary_oracle_edge_taker::KEY",
                ),
            ),
        )


def test_inline_path_keyed_literally_including_trailing_segments() -> None:
    # Design: an inline path is keyed by exactly what is written, so
    # `crate::strategies::X::method()` keys as `strategies::X::method` (not `X`).
    # This is intentional and fail-safe: any inline reference is a NEW violation
    # to fix (the frozen allowlist is entirely `use`-based), so swapping an allowed
    # `use` for a fully-qualified inline call correctly fails rather than passing.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "fn f() { crate::strategies::registry::FeeProvider::new(); }\n",
        )
        err = expect_fail(root)
        if "strategies::registry::FeeProvider::new" not in err:
            raise AssertionError(f"expected literal trailing segment, got: {err!r}")


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
        test_inline_type_annotation_path_flagged,
        test_inline_call_path_flagged,
        test_inline_macro_arg_path_flagged,
        test_inline_turbofish_path_flagged,
        test_inline_super_path_from_top_level_flagged,
        test_attribute_inside_brace_import_flagged,
        test_glob_import_flagged,
        test_raw_string_containing_path_not_flagged,
        test_string_literal_containing_path_not_flagged,
        test_byte_and_raw_byte_strings_with_path_not_flagged,
        test_lifetime_does_not_hide_following_import,
        test_nested_block_comment_ignored,
        test_inline_path_allowance_suppresses,
        test_inline_path_keyed_literally_including_trailing_segments,
        test_real_repo_is_green_with_committed_allowlist,
        test_file_size_limit_exceeded_fails_cleanly,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 dependency-direction verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
