#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 dependency-direction verifier."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import subprocess
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


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout.strip()


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
            code = VERIFIER.main([])
    finally:
        VERIFIER.REPO_ROOT = original_root
        VERIFIER.FINDING_ALLOWANCES = original_allow
    return code, stdout.getvalue(), stderr.getvalue()


def allowance(path: str, strategy_path: str):
    return VERIFIER.FindingAllowance(path=path, strategy_path=strategy_path)


def baseline_source(pairs) -> str:
    body = ",\n".join(f'    FindingAllowance("{p}", "{s}")' for p, s in pairs)
    return f"FINDING_ALLOWANCES = (\n{body},\n)\n"


def run_shrink_only(allowances, baseline) -> tuple[int, str, str]:
    """Run the shrink-only mode with FINDING_ALLOWANCES and the mainline baseline
    source both injected. `baseline` is a source string, None (introducing PR), or
    an Exception instance to raise from the baseline reader."""

    orig_allow = VERIFIER.FINDING_ALLOWANCES
    orig_read = VERIFIER._read_baseline_source
    stdout = io.StringIO()
    stderr = io.StringIO()

    def fake_read():
        if isinstance(baseline, BaseException):
            raise baseline
        return baseline

    try:
        VERIFIER.FINDING_ALLOWANCES = allowances
        VERIFIER._read_baseline_source = fake_read
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = VERIFIER.main(["--check-shrink-only-vs-main"])
    finally:
        VERIFIER.FINDING_ALLOWANCES = orig_allow
        VERIFIER._read_baseline_source = orig_read
    return code, stdout.getvalue(), stderr.getvalue()


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


def test_empty_scan_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        code, _out, err = run_with(root, allowances=None)

    expected = "FAIL: Bolt-v3 dependency direction source files: enforcement set is empty\n"
    if code != 1 or err != expected:
        raise AssertionError(f"expected exact empty source floor, got code={code}, stderr={err!r}")


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
        write_file(root, "src/bolt_v3_foo.rs", "pub struct Clean;\n")
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


def test_include_source_macro_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", 'include!("strategies/registry.rs");\n')
        err = expect_fail(root)
        if "include!" not in err or "source inclusion" not in err:
            raise AssertionError(f"expected include! source-inclusion diagnostic, got: {err!r}")


def test_raw_include_source_macro_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", 'r#include!("strategies/registry.rs");\n')
        err = expect_fail(root)
        if "include!" not in err or "source inclusion" not in err:
            raise AssertionError(f"expected raw include! source-inclusion diagnostic, got: {err!r}")


def test_path_attribute_source_module_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            '#[path = "strategies/registry.rs"]\nmod registry;\n',
        )
        err = expect_fail(root)
        if "#[path]" not in err or "source inclusion" not in err:
            raise AssertionError(f"expected #[path] source-inclusion diagnostic, got: {err!r}")


def test_raw_path_attribute_source_module_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            '#[r#path = "strategies/registry.rs"]\nmod registry;\n',
        )
        err = expect_fail(root)
        if "#[path]" not in err or "source inclusion" not in err:
            raise AssertionError(f"expected raw #[path] source-inclusion diagnostic, got: {err!r}")


def test_use_crate_root_alias_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "use crate as renamed_crate;\n"
            "use renamed_crate::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "crate-root alias" not in err:
            raise AssertionError(f"expected crate-root alias diagnostic, got: {err!r}")


def test_use_crate_group_self_alias_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "use crate::{self as renamed_crate};\n"
            "use renamed_crate::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "crate-root alias" not in err:
            raise AssertionError(f"expected crate-root alias diagnostic, got: {err!r}")


def test_non_root_group_self_alias_allowed_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/bolt_v3_foo.rs", "use crate::foo::{self as renamed_foo};\n")
        expect_pass(root)


def test_use_super_crate_root_alias_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "use super as renamed_crate;\n"
            "fn f() -> renamed_crate::strategies::registry::FeeProvider { todo!() }\n",
        )
        err = expect_fail(root)
        if "crate-root alias" not in err:
            raise AssertionError(f"expected crate-root alias diagnostic, got: {err!r}")


def test_extern_crate_self_alias_rejected_in_shared_module() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "extern crate self as bolt_v2;\n"
            "use ::bolt_v2::strategies::registry::FeeProvider;\n",
        )
        err = expect_fail(root)
        if "extern crate self" not in err:
            raise AssertionError(f"expected extern crate self diagnostic, got: {err!r}")


def test_scanned_source_symlink_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/strategies/registry.rs", "pub struct StrategyLayer;\n")
        symlink = root / "src/bolt_v3_laundered.rs"
        symlink.symlink_to("strategies/registry.rs")
        err = expect_fail(root)
        if "symlink" not in err:
            raise AssertionError(f"expected symlink diagnostic, got: {err!r}")


def test_scanned_source_directory_symlink_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "src/strategies/registry.rs", "pub struct StrategyLayer;\n")
        symlink = root / "src/bolt_v3_laundered"
        symlink.symlink_to("strategies", target_is_directory=True)
        err = expect_fail(root)
        if "symlink" not in err:
            raise AssertionError(f"expected directory symlink diagnostic, got: {err!r}")


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


def test_inline_turbofish_path_key_includes_trailing_segments() -> None:
    # A turbofish in the middle of an inline path must not truncate the key to an
    # allowlisted parent type and suppress a new call-site reference.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_foo.rs",
            "fn f() { crate::strategies::registry::FeeProvider::<Cfg>::new(); }\n",
        )
        err = expect_fail(
            root,
            allowances=(
                allowance("src/bolt_v3_foo.rs", "strategies::registry::FeeProvider"),
            ),
        )
        if "strategies::registry::FeeProvider::new" not in err:
            raise AssertionError(f"expected trailing segment after turbofish, got: {err!r}")


def test_inline_mod_super_super_from_top_level_flagged() -> None:
    # codex finding: inside an inline `mod tests {}` in a TOP-LEVEL bolt_v3_* file,
    # `super::super` reaches the crate root, so `super::super::strategies` is a real
    # back-reference the file-path-only resolver previously missed.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "mod tests {\n"
            "    use super::super::strategies::registry::FeeProvider;\n"
            "    type T = super::super::strategies::registry::StrategyBuilder;\n"
            "}\n",
        )
        err = expect_fail(root)
        if "FeeProvider" not in err or "StrategyBuilder" not in err:
            raise AssertionError(f"expected both super::super refs flagged, got: {err!r}")


def test_inline_mod_single_super_not_flagged() -> None:
    # A single `super` from `mod tests {}` in a top-level file resolves to the file
    # module (`bolt_v3_foo`), NOT the crate root — so it is not the strategy layer.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "mod tests {\n    use super::strategies::Thing;\n}\n",
        )
        expect_pass(root)


def test_inline_mod_super_reaches_crate_in_nested_file() -> None:
    # In a nested file module (depth 3) inside `mod tests {}` (depth 4), four
    # `super`s reach the crate root.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_providers/polymarket/fees.rs",
            "mod tests {\n"
            "    type T = super::super::super::super::strategies::registry::FeeProvider;\n"
            "}\n",
        )
        expect_fail(root)
        # One fewer `super` stays inside the crate's module tree (not strategies).
        write_file(
            root, "src/bolt_v3_providers/polymarket/fees.rs",
            "mod tests {\n"
            "    type T = super::super::super::strategies::registry::FeeProvider;\n"
            "}\n",
        )
        expect_pass(root)


def test_mod_block_scope_restored_after_close() -> None:
    # After an inline `mod {}` closes, resolution reverts to the file's own depth:
    # `super::super` at top-level file scope is unresolvable (not the crate root),
    # proving the module stack was popped rather than leaking.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "mod tests {\n    pub struct A;\n}\n"
            "fn f() -> super::super::strategies::registry::FeeProvider { todo!() }\n",
        )
        expect_pass(root)


def test_inner_attribute_does_not_break_detection() -> None:
    # glm note: `#![...]` inner attributes are now skipped; a real back-reference
    # after one is still flagged.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root, "src/bolt_v3_foo.rs",
            "#![allow(clippy::all)]\nuse crate::strategies::registry::FeeProvider;\n",
        )
        expect_fail(root)


def test_parse_allowances_matches_real_committed_allowlist() -> None:
    text = SCRIPT_PATH.read_text(encoding="utf-8")
    parsed = VERIFIER.parse_allowances_from_source(text)
    expected = {(a.path, a.strategy_path) for a in VERIFIER.FINDING_ALLOWANCES}
    if parsed != expected:
        raise AssertionError(f"AST parse mismatch: {parsed ^ expected}")


def test_parse_allowances_roundtrips_synthetic_source() -> None:
    pairs = {("src/bolt_v3_a.rs", "strategies::x::A"), ("src/bolt_v3_b.rs", "strategies::y::B")}
    if VERIFIER.parse_allowances_from_source(baseline_source(pairs)) != pairs:
        raise AssertionError("synthetic baseline did not round-trip")


def test_parse_allowances_rejects_duplicate_module_assignment() -> None:
    # Two module-level FINDING_ALLOWANCES assignments must fail closed (not union) —
    # at runtime only the last wins, so unioning would inflate the baseline.
    src = (
        baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")})
        + baseline_source({("src/bolt_v3_b.rs", "strategies::y::BACKDOOR")})
    )
    try:
        VERIFIER.parse_allowances_from_source(src)
    except VERIFIER.PolicyError as error:
        if "expected exactly 1" not in str(error):
            raise AssertionError(f"unexpected PolicyError text: {error}")
    else:
        raise AssertionError("expected PolicyError on duplicate FINDING_ALLOWANCES")


def test_parse_allowances_ignores_function_scope_reassignment() -> None:
    # A reassignment nested in a function/class never touches the runtime constant,
    # so it must NOT contribute to the parsed baseline.
    src = (
        baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")})
        + "\ndef _unused():\n"
        + "    FINDING_ALLOWANCES = (FindingAllowance('src/bolt_v3_p.rs', 'strategies::p::PHANTOM'),)\n"
        + "    return FINDING_ALLOWANCES\n"
    )
    parsed = VERIFIER.parse_allowances_from_source(src)
    if parsed != {("src/bolt_v3_a.rs", "strategies::x::A")}:
        raise AssertionError(f"function-scope reassignment leaked into baseline: {parsed}")


def test_parse_allowances_rejects_zero_assignment() -> None:
    try:
        VERIFIER.parse_allowances_from_source("X = 1\n")
    except VERIFIER.PolicyError as error:
        if "no module-level" not in str(error):
            raise AssertionError(f"unexpected PolicyError text: {error}")
    else:
        raise AssertionError("expected PolicyError when FINDING_ALLOWANCES is absent")


def test_shrink_only_duplicate_baseline_fails_closed() -> None:
    current = (allowance("src/bolt_v3_a.rs", "strategies::x::A"),)
    dup_baseline = (
        baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")})
        + baseline_source({("src/bolt_v3_b.rs", "strategies::y::BACKDOOR")})
    )
    code, _out, err = run_shrink_only(current, dup_baseline)
    if code != 1 or "cannot parse mainline baseline" not in err:
        raise AssertionError(f"expected fail-closed on ambiguous baseline, got {code}: {err!r}")


def test_shrink_only_subset_passes() -> None:
    current = (allowance("src/bolt_v3_a.rs", "strategies::x::A"),)
    base = baseline_source(
        {("src/bolt_v3_a.rs", "strategies::x::A"), ("src/bolt_v3_b.rs", "strategies::y::B")}
    )
    code, out, _ = run_shrink_only(current, base)
    if code != 0 or "subset of the mainline baseline" not in out:
        raise AssertionError(f"expected subset PASS, got {code}: {out!r}")


def test_shrink_only_addition_fails() -> None:
    current = (
        allowance("src/bolt_v3_a.rs", "strategies::x::A"),
        allowance("src/bolt_v3_b.rs", "strategies::y::NEW"),
    )
    base = baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")})
    code, _out, err = run_shrink_only(current, base)
    if code != 1 or "may only shrink" not in err or "strategies::y::NEW" not in err:
        raise AssertionError(f"expected addition FAIL naming the new entry, got {code}: {err!r}")


def test_shrink_only_introducing_pr_passes() -> None:
    current = (allowance("src/bolt_v3_a.rs", "strategies::x::A"),)
    code, out, _ = run_shrink_only(current, baseline=None)
    if code != 0 or "introducing" not in out:
        raise AssertionError(f"expected introducing-PR PASS, got {code}: {out!r}")


def test_shrink_only_unresolved_baseline_fails_closed() -> None:
    current = (allowance("src/bolt_v3_a.rs", "strategies::x::A"),)
    code, _out, err = run_shrink_only(
        current, baseline=VERIFIER.PolicyError("cannot resolve baseline ref origin/main")
    )
    if code != 1 or "cannot resolve baseline ref" not in err:
        raise AssertionError(f"expected fail-closed on missing baseline, got {code}: {err!r}")


def test_git_check_error_includes_stderr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        try:
            VERIFIER._git(["rev-parse", "--verify", "missing"], cwd=Path(tmp), check=True)
        except VERIFIER.PolicyError as exc:
            message = str(exc)
        else:
            raise AssertionError("expected checked git failure")
        if "fatal:" not in message:
            raise AssertionError(f"expected git stderr in PolicyError, got: {message!r}")


def test_shrink_only_remote_get_url_failure_includes_git_stderr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        work = root / "work"
        git(root, "init", str(work))

        original_root = VERIFIER.REPO_ROOT
        original_allow = VERIFIER.FINDING_ALLOWANCES
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = work
            VERIFIER.FINDING_ALLOWANCES = (
                allowance("src/bolt_v3_a.rs", "strategies::x::A"),
            )
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = VERIFIER.main(["--check-shrink-only-vs-main"])
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.FINDING_ALLOWANCES = original_allow
        message = stderr.getvalue()
        if code != 1 or "cannot resolve baseline remote origin" not in message:
            raise AssertionError(f"expected baseline remote failure, got {code}: {message!r}")
        if "No such remote" not in message:
            raise AssertionError(f"expected git stderr in baseline remote failure, got: {message!r}")


def test_shrink_only_fetch_failure_includes_git_stderr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        work = root / "work"
        git(root, "init", str(work))
        git(work, "remote", "add", "origin", str(root / "missing-origin.git"))

        original_root = VERIFIER.REPO_ROOT
        original_allow = VERIFIER.FINDING_ALLOWANCES
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = work
            VERIFIER.FINDING_ALLOWANCES = (
                allowance("src/bolt_v3_a.rs", "strategies::x::A"),
            )
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = VERIFIER.main(["--check-shrink-only-vs-main"])
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.FINDING_ALLOWANCES = original_allow
        message = stderr.getvalue()
        if code != 1 or "cannot resolve baseline ref origin/main" not in message:
            raise AssertionError(f"expected baseline fetch failure, got {code}: {message!r}")
        if "missing-origin.git" not in message and "does not appear to be a git repository" not in message:
            raise AssertionError(f"expected git stderr in baseline fetch failure, got: {message!r}")


def test_shrink_only_fetches_baseline_without_checkout_tracking_ref() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        remote = root / "origin.git"
        seed = root / "seed"
        work = root / "work"
        git(root, "init", "--bare", str(remote))
        git(root, "init", str(seed))
        git(seed, "config", "user.email", "dependency-direction@example.invalid")
        git(seed, "config", "user.name", "Dependency Direction Test")
        write_file(
            seed,
            "scripts/verify_bolt_v3_dependency_direction.py",
            baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")}),
        )
        git(seed, "add", ".")
        git(seed, "commit", "-m", "baseline")
        git(seed, "branch", "-M", "main")
        git(seed, "remote", "add", "origin", str(remote))
        git(seed, "push", "-u", "origin", "main")
        git(root, "clone", "-b", "main", str(remote), str(work))
        git(work, "update-ref", "-d", "refs/remotes/origin/main")
        git(work, "remote", "set-url", "origin", "../origin.git")

        original_root = VERIFIER.REPO_ROOT
        original_allow = VERIFIER.FINDING_ALLOWANCES
        original_git = VERIFIER._git
        git_commands: list[tuple[str, ...]] = []

        def recording_git(
            args: list[str],
            *,
            cwd: Path,
            check: bool = False,
            env: dict[str, str] | None = None,
        ) -> subprocess.CompletedProcess[str]:
            del env
            git_commands.append(tuple(args))
            return original_git(args, cwd=cwd, check=check)

        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = work
            VERIFIER.FINDING_ALLOWANCES = (
                allowance("src/bolt_v3_a.rs", "strategies::x::A"),
            )
            VERIFIER._git = recording_git
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = VERIFIER.main(["--check-shrink-only-vs-main"])
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.FINDING_ALLOWANCES = original_allow
            VERIFIER._git = original_git
        if code != 0 or "subset of the mainline baseline" not in stdout.getvalue():
            raise AssertionError(
                f"expected isolated baseline fetch to pass, got {code}: "
                f"stdout={stdout.getvalue()!r} stderr={stderr.getvalue()!r}"
            )
        if any(command[:2] == ("remote", "add") for command in git_commands):
            raise AssertionError(f"baseline fetch must not configure temp remotes: {git_commands!r}")
        fetch_commands = [command for command in git_commands if command[:1] == ("fetch",)]
        if not any(str(remote.resolve()) in command for command in fetch_commands):
            raise AssertionError(f"baseline fetch did not use resolved absolute remote URL: {fetch_commands!r}")
        tracking_ref = subprocess.run(
            ["git", "show-ref", "--verify", "--quiet", "refs/remotes/origin/main"],
            cwd=work,
            check=False,
        )
        if tracking_ref.returncode == 0:
            raise AssertionError("baseline fetch must not recreate checkout refs/remotes/origin/main")


def test_shrink_only_fetch_uses_actions_token_for_matching_github_repo() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_allow = VERIFIER.FINDING_ALLOWANCES
    original_git = VERIFIER._git
    old_env = {
        key: os.environ.get(key)
        for key in ("GITHUB_TOKEN", "GITHUB_REPOSITORY", "GIT_CONFIG_COUNT")
    }
    git_commands: list[tuple[str, ...]] = []
    fetch_envs: list[dict[str, str]] = []

    def completed(
        args: list[str],
        returncode: int = 0,
        stdout: str = "",
        stderr: str = "",
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(args, returncode, stdout=stdout, stderr=stderr)

    def fake_git(
        args: list[str],
        *,
        cwd: Path,
        check: bool = False,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, check
        git_commands.append(tuple(args))
        if any("fake-ci-token" in part for part in args):
            raise AssertionError(f"token leaked into git argv: {args!r}")
        if args == ["remote", "get-url", "origin"]:
            return completed(args, stdout="https://github.com/Owner/Repo.git\n")
        if args[:2] == ["init", "--bare"]:
            Path(args[2]).mkdir(parents=True, exist_ok=True)
            return completed(args)
        if args[:1] == ["fetch"]:
            fetch_envs.append(dict(env or {}))
            return completed(args)
        if args[:1] == ["rev-parse"]:
            return completed(args, stdout="baseline-sha\n")
        if args[:1] == ["show"]:
            return completed(
                args,
                stdout=baseline_source({("src/bolt_v3_a.rs", "strategies::x::A")}),
            )
        raise AssertionError(f"unexpected git command: {args!r}")

    stdout = io.StringIO()
    stderr = io.StringIO()
    try:
        VERIFIER.REPO_ROOT = Path("/workspace/repo")
        VERIFIER.FINDING_ALLOWANCES = (
            allowance("src/bolt_v3_a.rs", "strategies::x::A"),
        )
        VERIFIER._git = fake_git
        os.environ["GITHUB_TOKEN"] = "fake-ci-token"
        os.environ["GITHUB_REPOSITORY"] = "owner/repo"
        os.environ.pop("GIT_CONFIG_COUNT", None)
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = VERIFIER.main(["--check-shrink-only-vs-main"])
    finally:
        VERIFIER.REPO_ROOT = original_root
        VERIFIER.FINDING_ALLOWANCES = original_allow
        VERIFIER._git = original_git
        for key, value in old_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    if code != 0 or "subset of the mainline baseline" not in stdout.getvalue():
        raise AssertionError(
            f"expected authenticated baseline fetch to pass, got {code}: "
            f"stdout={stdout.getvalue()!r} stderr={stderr.getvalue()!r}"
        )
    if not fetch_envs:
        raise AssertionError(f"expected fetch env to be captured, commands={git_commands!r}")
    fetch_env = fetch_envs[0]
    if fetch_env.get("GIT_CONFIG_COUNT") != "1":
        raise AssertionError(f"expected one injected git config, got {fetch_env!r}")
    if fetch_env.get("GIT_CONFIG_KEY_0") != "http.https://github.com/.extraheader":
        raise AssertionError(f"expected GitHub extraheader key, got {fetch_env!r}")
    if not fetch_env.get("GIT_CONFIG_VALUE_0", "").startswith("AUTHORIZATION: basic "):
        raise AssertionError(f"expected basic auth extraheader value, got {fetch_env!r}")


def test_dependency_shrink_only_ci_invocation_carries_github_identity() -> None:
    justfile = SCRIPT_PATH.parent.parent / "justfile"
    ci_workflow = SCRIPT_PATH.parent.parent / ".github" / "workflows" / "ci.yml"
    just_text = justfile.read_text(encoding="utf-8")
    ci_text = ci_workflow.read_text(encoding="utf-8")

    if "python3 scripts/verify_bolt_v3_dependency_direction.py --check-shrink-only-vs-main" not in just_text:
        raise AssertionError("source-fence must invoke dependency shrink-only verification")
    source_fence_step = ci_text.split("      - name: source-fence", 1)[1].split("      - name:", 1)[0]
    for required in (
        "GITHUB_TOKEN: ${{ github.token }}",
        "GITHUB_REPOSITORY: ${{ github.repository }}",
        "just source-fence",
    ):
        if required not in source_fence_step:
            raise AssertionError(f"source-fence CI step must carry {required}")


def test_justfile_dependency_baseline_fetch_is_not_checkout_mutation() -> None:
    justfile = SCRIPT_PATH.parent.parent / "justfile"
    text = justfile.read_text(encoding="utf-8")
    bad_fetch = "git fetch -q origin main"
    if bad_fetch in text:
        raise AssertionError("dependency-direction baseline fetch must not mutate checkout .git")


def test_remote_url_normalization_uses_shared_helper() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    if "REMOTE_URL_SCHEME_RE =" in source or "\ndef fetchable_remote_url(" in source:
        raise AssertionError("remote URL normalization must live in one shared helper")


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
        test_empty_scan_fails_closed,
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
        test_include_source_macro_rejected_in_shared_module,
        test_raw_include_source_macro_rejected_in_shared_module,
        test_path_attribute_source_module_rejected_in_shared_module,
        test_raw_path_attribute_source_module_rejected_in_shared_module,
        test_use_crate_root_alias_rejected_in_shared_module,
        test_use_crate_group_self_alias_rejected_in_shared_module,
        test_non_root_group_self_alias_allowed_in_shared_module,
        test_use_super_crate_root_alias_rejected_in_shared_module,
        test_extern_crate_self_alias_rejected_in_shared_module,
        test_scanned_source_symlink_rejected,
        test_scanned_source_directory_symlink_rejected,
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
        test_inline_turbofish_path_key_includes_trailing_segments,
        test_inline_mod_super_super_from_top_level_flagged,
        test_inline_mod_single_super_not_flagged,
        test_inline_mod_super_reaches_crate_in_nested_file,
        test_mod_block_scope_restored_after_close,
        test_inner_attribute_does_not_break_detection,
        test_parse_allowances_matches_real_committed_allowlist,
        test_parse_allowances_roundtrips_synthetic_source,
        test_parse_allowances_rejects_duplicate_module_assignment,
        test_parse_allowances_ignores_function_scope_reassignment,
        test_parse_allowances_rejects_zero_assignment,
        test_shrink_only_duplicate_baseline_fails_closed,
        test_shrink_only_subset_passes,
        test_shrink_only_addition_fails,
        test_shrink_only_introducing_pr_passes,
        test_shrink_only_unresolved_baseline_fails_closed,
        test_git_check_error_includes_stderr,
        test_shrink_only_remote_get_url_failure_includes_git_stderr,
        test_shrink_only_fetch_failure_includes_git_stderr,
        test_shrink_only_fetches_baseline_without_checkout_tracking_ref,
        test_shrink_only_fetch_uses_actions_token_for_matching_github_repo,
        test_dependency_shrink_only_ci_invocation_carries_github_identity,
        test_justfile_dependency_baseline_fetch_is_not_checkout_mutation,
        test_remote_url_normalization_uses_shared_helper,
        test_real_repo_is_green_with_committed_allowlist,
        test_file_size_limit_exceeded_fails_cleanly,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 dependency-direction verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
