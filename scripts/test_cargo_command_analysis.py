#!/usr/bin/env python3
"""Relocated CI workflow hygiene analyzer tests."""

from __future__ import annotations

import sys

from test_verify_ci_workflow_hygiene import *  # noqa: F403

def assert_v6_red_renamed_path_cargo_source_builds_are_reported() -> None:
    verifier = load_verifier()
    cases = [
        (
            "/tmp/c install cargo-deny --root s3://bolt-v2-active-cache/install-root",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "timeout 30 /tmp/c install cargo-nextest --version 0.9.132 --locked",
            "repo automation must not compile cargo-nextest from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny /tmp/my-deny\ncargo install --path /tmp/my-deny",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny /tmp/my-deny\ncd /tmp/my-deny && cargo install --path .",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny.git\ncd cargo-deny && cargo install --path .",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "cargo install --git https://github.com/EmbarkStudios/Cargo-Deny --locked",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "cargo install --git https://github.com/nextest-rs/cargo-NeXtEsT --package cargo-nextest --locked",
            "repo automation must not compile cargo-nextest from source",
        ),
        (
            "cargo install $(echo ; echo cargo-deny) --locked",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "sudo sudo sudo sudo sudo sudo sudo cargo install cargo-deny --locked",
            "repo automation must not compile cargo-deny from source",
        ),
    ]
    for text, expected in cases:
        errors = verifier.repo_automation_source_build_errors(text)
        if expected not in errors:
            raise AssertionError(f"{text!r}: expected {expected!r}, got {errors!r}")

def assert_v6_red_static_path_classifier_ignores_host_filesystem_resolution() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        cargo_target = tmp_path / "cargo"
        cargo_target.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        cargo_link = tmp_path / "builder"
        cargo_link.symlink_to(cargo_target)
        rustc_target = tmp_path / "rustc"
        rustc_target.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        rustc_link = tmp_path / "compiler"
        rustc_link.symlink_to(rustc_target)
        if verifier.path_executable_looks_like_cargo(str(cargo_link)):
            raise AssertionError("static cargo classifier must not inspect host filesystem symlink targets")
        if verifier.path_executable_looks_like_rustc(str(rustc_link)):
            raise AssertionError("static rustc classifier must not inspect host filesystem symlink targets")
        if not verifier.path_executable_looks_like_cargo(str(tmp_path / "mycargo")):
            raise AssertionError("static cargo classifier must still classify path names that look like cargo")

def assert_v6_red_no_mistakes_raw_cargo_is_reported() -> None:
    raw_fixture = """
aliases:
  - &raw_top "cargo build --target-dir /tmp/raw"
  - &raw_top_block |
      cargo build --target-dir /tmp/raw
commands:
  test: cargo test
  lint: >
    cargo clippy --all-targets -- -D warnings
  format: "cargo fmt --check"
  review: 'cargo test --all-targets'
  ci: |
    cargo clippy --workspace
  envcheck: env CARGO_TARGET_DIR=target cargo test
  envsplit: env -S 'cargo test'
  envsplitunquoted: env -S timeout 30 cargo test
  envinvalidassignment: env 1=2 cargo build
  envblocksignal: env --block-signal cargo test
  timeoutdashdash: timeout -- 30 cargo build
  tasksetdashdash: taskset -- 0 cargo build
  anchored: &raw "cargo build --target-dir /tmp/raw"
  anchoralias: *raw
  topanchoralias: *raw_top
  topanchoraliascomment: *raw_top # inline comment
  topblockanchoralias: *raw_top_block
  shellcheck: bash -lc 'cargo test --all'
  evalraw: eval "cargo test"
  evaldashdash: eval -- cargo test
  evalprefix: A=B eval cargo test
  evalprefixquoted: A=B eval "cargo test"
  evalvar: 'CMD="cargo build --target-dir /tmp/raw"; eval "$CMD"'
  evalexportvar: 'export CMD="cargo build"; eval "$CMD"'
  evalparam: 'export CMD="cargo build"; eval "${CMD:-}"'
  evaldynamicenv: 'VAR=CARGO_TARGET_DIR; $VAR=/tmp/raw cargo check'
  evalcomposedenv: 'VAR=CARGO; ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  evalinlinecomposedenv: 'VAR=CARGO ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  evalinlinecomposedeval: 'VAR=CARGO eval "${VAR}_TARGET_DIR=/tmp/raw cargo check"'
  shellccomposedenv: 'VAR=CARGO; bash -c "${VAR}_TARGET_DIR=/tmp/raw cargo test"'
  aliaspayload: 'alias c='\''V=CARGO; eval "${V}_TARGET_DIR=/tmp/raw cargo"'\''; c build'
  sudocomposedenv: 'VAR=CARGO; sudo ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  timecomposedenv: 'VAR=CARGO; time ${VAR}_TARGET_DIR=/tmp/raw cargo test'
  evalcmdpayload: 'VAR=CARGO; CMD="${VAR}_TARGET_DIR=/tmp/raw cargo check"; eval "$CMD"'
  shellcmdpayload: 'VAR=CARGO; export CMD="${VAR}_TARGET_DIR=/tmp/raw cargo check"; bash -c "$CMD"'
  aliasouterpayload: 'VAR=CARGO; alias c='\''${VAR}_TARGET_DIR=/tmp/raw cargo'\''; c build'
  redirectprefix: '> /dev/null cargo test'
  redirectcompact: 'cargo>out test'
  foldedplain: eval
    cargo test
  foldeddouble: "eval
    cargo test"
  shellprefix: A=B bash -c "cargo test"
  shellevalraw: bash -lc 'eval "cargo test"'
  shellalias: bash -lc 'alias c=cargo; c test'
  shellaliasquoted: bash -lc 'alias c="command cargo"; c test'
  shellaliasclippy: bash -lc 'alias c=clippy; c --all-targets'
  shellaliasnextest: bash -lc 'alias c=nextest; c run'
  shellaliasrustc: bash -lc 'alias c=rustc; c --crate-name bolt_v2'
  shellaliastime: bash -lc 'alias c=cargo; time c test'
  shellaliasnice: bash -lc 'alias c=clippy; nice c --all-targets'
  renamedcargo: /tmp/c build
  timerenamedcargo: time /tmp/c build
  xargsrenamedcargo: xargs /tmp/c build
  wrapped: command cargo fmt --check
  stdbufwrap: stdbuf -oL cargo build
  catchsegvwrap: catchsegv cargo test
  chrtbatchwrap: chrt -b cargo build
  chrtidlewrap: chrt -i cargo build
  nicewrap: nice cargo test
  timeniceadjust: time nice --adjustment 10 cargo test
  timeverbose: A=B time -v cargo test
  timeoutput: A=B time -o /tmp/bolt-time cargo test
  doaswrap: doas cargo test
  flockwrap: flock "$TMPDIR/bolt.lock" cargo test
  flockfilec: flock "$TMPDIR/bolt.lock" -c 'cargo test'
  flockshortc: flock -xc 'cargo test' "$TMPDIR/bolt.lock"
  timeflocktimeout: time flock --timeout 5 "$TMPDIR/bolt.lock" cargo test
  flockclose: flock -o "$TMPDIR/bolt.lock" cargo test
  sudoflock: sudo flock -o "$TMPDIR/bolt.lock" cargo test
  sudousercommand: sudo -u bash cargo build
  sudoshell: sudo bash -lc 'cargo test --all'
  envargv0: env --argv0 cargo cargo build
  envshortargv0: env -a cargo cargo build
  envshell: env -i bash -lc 'cargo test --all'
  hyphenated: cargo-clippy --workspace
  zigbuild: cargo zigbuild --release
  rustup: rustup run stable cargo test
  pyinline: python -c 'import os; os.system("cargo test")'
  timeout: timeout 30 cargo test
  managedjustenv: BOLT_MANAGED_JUST=1 just managed-build
  managedjustdynamic: VAR=BOLT_MANAGED_JUST; export $VAR=1; just managed-build
  declaretarget: E=CARGO_TARGET_DIR; declare -x $E=/tmp/raw; cargo check
  substitutiontarget: cargo check $(echo ; echo --target-dir /tmp/raw)
  dockeruncertainrenamed: docker run --unknown-opt=rust mycargo build
  podmanuncertainrenamedrustc: podman run --unknown-opt=rust myrustc --out-dir /tmp/raw
  chained: python3 scripts/rust_verification.py cargo --repo . -- test && cargo test
  compact_and: python3 scripts/rust_verification.py cargo --repo . -- test&&cargo test
  compact_semicolon: python3 scripts/rust_verification.py cargo --repo . -- test;cargo test
  compact_pipe: python3 scripts/rust_verification.py cargo --repo . -- test|cargo test
  compact_or: python3 scripts/rust_verification.py cargo --repo . -- test||cargo test
  blockmanagedhidden: |
    python3 scripts/rust_verification.py cargo --repo . -- test
    cargo test
  managedtarget: python3 scripts/rust_verification.py cargo --repo . -- test --target-dir /tmp/raw
  managedconfig: python3 scripts/rust_verification.py cargo --repo . -- --config=build.target-dir=/tmp/raw test
  managedencodedrustflags: CARGO_ENCODED_RUSTFLAGS='--out-dir\\x1f/tmp/raw-out' python3 scripts/rust_verification.py cargo --repo . -- check
  managedinstallroot: python3 scripts/rust_verification.py cargo --repo . -- install ripgrep --root /tmp/install-root
  managedrustcwrapper: RUSTC_WRAPPER=/tmp/wrapper python3 scripts/rust_verification.py cargo --repo . -- test
  managedtimeout: timeout 30 python3 scripts/rust_verification.py cargo --repo . -- test
  managedenvci: GITHUB_ACTIONS=true python3 scripts/rust_verification.py cargo --repo . -- test
  managedenvcmdci: env GITHUB_ACTIONS=true python3 scripts/rust_verification.py cargo --repo . -- test
  managedpythonflag: python3 -W ignore scripts/rust_verification.py cargo --repo . -- test
  no-mistakes-clippy-command: no-mistakes run -- clippy
  no-mistakes-nextest-command: no-mistakes run -- nextest run
  s3cache: aws s3 sync target s3://bolt-v2-active-cache/target
  docs: just docs
"""
    allowed_fixture = """
commands:
  test: just source-fence-static
  lint: just fmt-check
  format: python3 scripts/rust_verification.py cargo --repo . -- fmt --check
  exact-head-ci: gh run view --repo seungpyoson/bolt-v2 --commit "$GITHUB_SHA" --json conclusion
  sudouserarg: timeout 30 sudo -u cargo echo hello
"""
    commented_commands_fixture = """
commands: # repo review commands
  test: cargo test
"""
    inline_commands_fixture = """
commands: { test: "cargo test" }
"""
    fixture_expected_raw_keys = [
        "test",
        "lint",
        "format",
        "review",
        "ci",
        "envcheck",
        "envsplit",
        "envsplitunquoted",
        "envinvalidassignment",
        "envblocksignal",
        "timeoutdashdash",
        "tasksetdashdash",
        "anchored",
        "anchoralias",
        "topanchoralias",
        "topanchoraliascomment",
        "topblockanchoralias",
        "shellcheck",
        "evalraw",
        "evaldashdash",
        "evalprefix",
        "evalprefixquoted",
        "evalvar",
        "evalexportvar",
        "evalparam",
        "evaldynamicenv",
        "evalcomposedenv",
        "evalinlinecomposedenv",
        "evalinlinecomposedeval",
        "shellccomposedenv",
        "aliaspayload",
        "sudocomposedenv",
        "timecomposedenv",
        "evalcmdpayload",
        "shellcmdpayload",
        "aliasouterpayload",
        "redirectprefix",
        "redirectcompact",
        "foldedplain",
        "foldeddouble",
        "shellprefix",
        "shellevalraw",
        "shellalias",
        "shellaliasquoted",
        "shellaliasclippy",
        "shellaliasnextest",
        "shellaliasrustc",
        "shellaliastime",
        "shellaliasnice",
        "renamedcargo",
        "timerenamedcargo",
        "xargsrenamedcargo",
        "wrapped",
        "stdbufwrap",
        "catchsegvwrap",
        "chrtbatchwrap",
        "chrtidlewrap",
        "nicewrap",
        "timeniceadjust",
        "timeverbose",
        "timeoutput",
        "doaswrap",
        "flockwrap",
        "flockfilec",
        "flockshortc",
        "timeflocktimeout",
        "flockclose",
        "sudoflock",
        "sudousercommand",
        "sudoshell",
        "envargv0",
        "envshortargv0",
        "envshell",
        "hyphenated",
        "zigbuild",
        "rustup",
        "pyinline",
        "timeout",
        "managedjustenv",
        "managedjustdynamic",
        "declaretarget",
        "substitutiontarget",
        "dockeruncertainrenamed",
        "podmanuncertainrenamedrustc",
        "chained",
        "compact_and",
        "compact_semicolon",
        "compact_pipe",
        "compact_or",
        "blockmanagedhidden",
        "managedtarget",
        "managedconfig",
        "managedencodedrustflags",
        "managedinstallroot",
        "no-mistakes-clippy-command",
        "no-mistakes-nextest-command",
    ]
    expected = [
        f".no-mistakes.yaml commands.{command_name} raw Cargo drift must be classified"
        for command_name in fixture_expected_raw_keys
    ]
    expected_s3 = ".no-mistakes.yaml commands.s3cache S3 active mutable target cache must be rejected"
    expected_storage = [
        ".no-mistakes.yaml commands.managedrustcwrapper RUSTC_WRAPPER raw compiler wrapper must be classified",
        ".no-mistakes.yaml commands.managedenvci GITHUB_ACTIONS local CI spoof must not be checked in",
        ".no-mistakes.yaml commands.managedenvcmdci GITHUB_ACTIONS local CI spoof must not be checked in",
    ]
    expected_wrapper = [
        ".no-mistakes.yaml commands.managedtimeout wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedenvci wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedenvcmdci wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedpythonflag wrapper-routed local compile-heavy Rust must be remote-first",
    ]
    fixture_result, fixture_errors = run_verifier_main_with_no_mistakes(raw_fixture)
    missing_fixture = [fragment for fragment in expected if fragment not in fixture_errors]
    missing_storage = [fragment for fragment in expected_storage if fragment not in fixture_errors]
    missing_wrapper = [fragment for fragment in expected_wrapper if fragment not in fixture_errors]
    false_fixture = ".no-mistakes.yaml commands.docs raw Cargo drift must be classified" in fixture_errors
    allowed_result, allowed_errors = run_verifier_main_with_no_mistakes(allowed_fixture)
    false_allowed = [
        error for error in allowed_errors.splitlines()
        if ".no-mistakes.yaml" in error and "raw Cargo drift" in error
    ]
    commented_result, commented_errors = run_verifier_main_with_no_mistakes(commented_commands_fixture)
    commented_expected = ".no-mistakes.yaml commands.test raw Cargo drift must be classified"
    inline_result, inline_errors = run_verifier_main_with_no_mistakes(inline_commands_fixture)
    inline_expected = ".no-mistakes.yaml commands section must use block mapping"

    if (
        fixture_result == 0
        or missing_fixture
        or missing_storage
        or missing_wrapper
        or expected_s3 not in fixture_errors
        or false_fixture
        or allowed_result != 0
        or false_allowed
        or commented_result == 0
        or commented_expected not in commented_errors
        or inline_result == 0
        or inline_expected not in inline_errors
    ):
        raise AssertionError(
            "no-mistakes raw-Cargo drift must fail through verifier main() while managed-wrapper "
            "and exact-head CI evidence commands stay allowed: "
            f"fixture_result={fixture_result} missing_fixture={missing_fixture!r} "
            f"missing_storage={missing_storage!r} missing_wrapper={missing_wrapper!r} "
            f"expected_s3={expected_s3!r} false_fixture={false_fixture} fixture_errors={fixture_errors!r} "
            f"fixture_expected_raw_keys={fixture_expected_raw_keys!r} "
            f"allowed_result={allowed_result} false_allowed={false_allowed!r} "
            f"allowed_errors={allowed_errors!r} "
            f"commented_result={commented_result} commented_errors={commented_errors!r} "
            f"inline_result={inline_result} inline_errors={inline_errors!r}"
        )

def assert_shell_logical_lines_handles_crlf_continuations() -> None:
    verifier = load_verifier()
    logical_lines = verifier.shell_logical_lines("cargo check \\\r\n  --target-dir /tmp/raw\r\n")
    if logical_lines != ["cargo check    --target-dir /tmp/raw"]:
        raise AssertionError(f"CRLF shell continuation was not folded: {logical_lines!r}")

def assert_cargo_named_just_recipe_headers_are_not_raw_cargo_commands() -> None:
    verifier = load_verifier()
    errors = verifier.verify_repo_automation_texts(
        {
            "justfile": (
                "cargo-shim-tests:\n"
                "    python3 -m pytest scripts/test_cargo_shim.py -q\n"
            )
        }
    )
    expected = "repo automation raw Cargo must use managed rust_verification wrapper"
    if any(expected in error for error in errors):
        raise AssertionError(f"cargo-named just recipe header was treated as raw cargo: {errors!r}")


def main() -> int:
    assert_v6_red_renamed_path_cargo_source_builds_are_reported()
    assert_v6_red_static_path_classifier_ignores_host_filesystem_resolution()
    assert_v6_red_no_mistakes_raw_cargo_is_reported()
    assert_shell_logical_lines_handles_crlf_continuations()
    assert_cargo_named_just_recipe_headers_are_not_raw_cargo_commands()
    print("OK: cargo command analysis tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
