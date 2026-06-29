#!/usr/bin/env python3
"""Characterization tests for shared command-understanding helpers."""

from __future__ import annotations

import ast
import importlib
import importlib.util
import pathlib
import subprocess
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS_DIR = REPO_ROOT / "scripts"
RUNTIME_VERIFIER = REPO_ROOT / "scripts" / "rust_verification.py"
STATIC_VERIFIER = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
SHARED_HELPERS = REPO_ROOT / "scripts" / "command_understanding.py"
_MODULE_CACHE: dict[pathlib.Path, object] = {}


def ensure_test_imports_available() -> None:
    scripts_dir = str(SCRIPTS_DIR)
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)


ensure_test_imports_available()


def load_module(path: pathlib.Path, module_name: str) -> object:
    cached = _MODULE_CACHE.get(path)
    if cached is not None:
        return cached
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _MODULE_CACHE[path] = module
    return module


def load_shared_module() -> object:
    if not SHARED_HELPERS.exists():
        raise AssertionError("missing scripts/command_understanding.py")
    return importlib.import_module("command_understanding")


def top_level_sys_import_aliases(tree: ast.Module) -> tuple[set[str], set[str]]:
    sys_module_aliases = {"sys"}
    sys_path_aliases: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "sys":
                    sys_module_aliases.add(alias.asname or alias.name)
        if isinstance(node, ast.ImportFrom) and node.module == "sys":
            for alias in node.names:
                if alias.name == "path":
                    sys_path_aliases.add(alias.asname or alias.name)
    return sys_module_aliases, sys_path_aliases


def node_references_sys_path(
    node: ast.AST, sys_module_aliases: set[str], sys_path_aliases: set[str]
) -> bool:
    for child in ast.walk(node):
        if (
            isinstance(child, ast.Attribute)
            and child.attr == "path"
            and isinstance(child.value, ast.Name)
            and child.value.id in sys_module_aliases
        ):
            return True
        if isinstance(child, ast.Name) and child.id in sys_path_aliases:
            return True
    return False


def assert_test_import_setup_is_encapsulated() -> None:
    tree = ast.parse(pathlib.Path(__file__).read_text(encoding="utf-8"))
    sys_module_aliases, sys_path_aliases = top_level_sys_import_aliases(tree)
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        if node_references_sys_path(node, sys_module_aliases, sys_path_aliases):
            raise AssertionError("test import sys.path setup must be encapsulated in a helper")


def assert_import_setup_rejects_bare_top_level_sys_path_mutation() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        candidate = pathlib.Path(tmp) / "candidate.py"
        candidate.write_text(
            "import sys\n"
            "sys.path.insert(0, 'scripts')\n"
            "\n"
            "def helper() -> None:\n"
            "    sys.path.insert(0, 'scripts')\n",
            encoding="utf-8",
        )
        original_file = globals()["__file__"]
        globals()["__file__"] = str(candidate)
        try:
            try:
                assert_test_import_setup_is_encapsulated()
            except AssertionError as exc:
                if "test import sys.path setup" not in str(exc):
                    raise
            else:
                raise AssertionError("bare top-level sys.path setup must be rejected")
        finally:
            globals()["__file__"] = original_file


def assert_import_setup_rejects_aliased_top_level_sys_path_mutation() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        candidate = pathlib.Path(tmp) / "candidate.py"
        candidate.write_text(
            "from sys import path\n"
            "path.insert(0, 'scripts')\n"
            "\n"
            "def helper() -> None:\n"
            "    path.insert(0, 'scripts')\n",
            encoding="utf-8",
        )
        original_file = globals()["__file__"]
        globals()["__file__"] = str(candidate)
        try:
            try:
                assert_test_import_setup_is_encapsulated()
            except AssertionError as exc:
                if "test import sys.path setup" not in str(exc):
                    raise
            else:
                raise AssertionError("aliased top-level sys.path setup must be rejected")
        finally:
            globals()["__file__"] = original_file


def expression(source: str) -> ast.AST:
    return ast.parse(source, mode="eval").body


def first_call(source: str) -> ast.Call:
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            return node
    raise AssertionError(f"no call found in {source!r}")


def assert_verifier_modules_import_from_repo_root() -> None:
    command = (
        "import importlib.util; "
        "paths = ['scripts/rust_verification.py', 'scripts/verify_ci_workflow_hygiene.py']; "
        "\nfor index, path in enumerate(paths):\n"
        "    spec = importlib.util.spec_from_file_location(f'verifier_{index}', path)\n"
        "    module = importlib.util.module_from_spec(spec)\n"
        "    spec.loader.exec_module(module)\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", command],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise AssertionError(
            "verifier modules must import from repo root without relying on scripts/ sys.path; "
            f"stderr={result.stderr.strip()!r}"
        )

    result = subprocess.run(
        [sys.executable, "-m", "scripts.rust_verification", "--help"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise AssertionError(f"rust_verification module execution failed: {result.stderr.strip()!r}")


def assert_load_module_caches_verifier_modules() -> None:
    runtime_first = load_module(RUNTIME_VERIFIER, "rust_verification_cache_first")
    runtime_second = load_module(RUNTIME_VERIFIER, "rust_verification_cache_second")
    static_first = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_cache_first")
    static_second = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_cache_second")
    if runtime_first is not runtime_second:
        raise AssertionError("runtime verifier was re-executed instead of loaded from cache")
    if static_first is not static_second:
        raise AssertionError("static verifier was re-executed instead of loaded from cache")


def assert_python_ast_helpers_match_current_verifiers() -> None:
    runtime = load_module(RUNTIME_VERIFIER, "rust_verification_under_test")
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_under_test")
    shared = load_shared_module()

    constant_cases = [
        ("'cargo build'", "cargo build"),
        ("'car' + 'go build'", "cargo build"),
        ("f'cargo build'", "cargo build"),
        ("f'cargo {target}'", None),
    ]
    for source, expected in constant_cases:
        node = expression(source)
        values = [
            runtime.python_constant_string(node),
            static.python_constant_string(node),
            shared.python_constant_string(node),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"python_constant_string({source!r}) returned {values!r}")

    command_cases = [
        ("'cargo build'", "cargo build"),
        ("['cargo', 'test', '--target-dir', '/tmp/raw']", "cargo test --target-dir /tmp/raw"),
        ("('cargo', 'build with space')", "cargo 'build with space'"),
        ("['cargo', dynamic]", None),
    ]
    for source, expected in command_cases:
        node = expression(source)
        values = [
            runtime.python_command_string(node),
            static.python_command_string(node),
            shared.python_command_string(node),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"python_command_string({source!r}) returned {values!r}")

    call_name_cases = [
        ("os.system('cargo build')", "os.system"),
        ("subprocess.run(['cargo', 'test'])", "subprocess.run"),
        ("run(['cargo'])", "run"),
    ]
    for source, expected in call_name_cases:
        call = first_call(source)
        values = [
            runtime.python_call_name(call.func),
            static.python_call_name(call.func),
            shared.python_call_name(call.func),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"python_call_name({source!r}) returned {values!r}")

    argument_cases = [
        ("subprocess.run(['cargo', 'test'])", "cargo test"),
        ("subprocess.run(args=['cargo', 'build'])", "cargo build"),
        ("subprocess.run(command=['cargo', 'check'])", "cargo check"),
        ("subprocess.run(timeout=30)", None),
    ]
    for source, expected in argument_cases:
        calls = [first_call(source)] * 3
        argument_nodes = [
            runtime.python_call_command_argument(calls[0]),
            static.python_call_command_argument(calls[1]),
            shared.python_call_command_argument(calls[2]),
        ]
        values = [
            None if argument_nodes[0] is None else runtime.python_command_string(argument_nodes[0]),
            None if argument_nodes[1] is None else static.python_command_string(argument_nodes[1]),
            None if argument_nodes[2] is None else shared.python_command_string(argument_nodes[2]),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"python_call_command_argument({source!r}) returned {values!r}")


def assert_python_inline_payloads_match_current_verifiers() -> None:
    runtime = load_module(RUNTIME_VERIFIER, "rust_verification_inline_under_test")
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_inline_under_test")
    shared = load_shared_module()

    cases = [
        (
            ["python", "-c", "import os; os.system('car' + 'go build')"],
            ["cargo build"],
        ),
        (
            ["python", "-c", "import subprocess; subprocess.run(['cargo', 'test', '--target-dir', '/tmp/raw'])"],
            ["cargo test --target-dir /tmp/raw"],
        ),
        (
            ["python", "-c", "import subprocess; subprocess.run(args=['cargo', 'check'])"],
            ["cargo check"],
        ),
        (
            ["python", "-c", "import subprocess; subprocess.call(command=['cargo', 'clippy'])"],
            ["cargo clippy"],
        ),
        (
            ["python", "-c", "import os; os.system('cargo ' + target)"],
            [],
        ),
        (
            ["python", "-c", "not valid python"],
            [],
        ),
    ]
    for tokens, expected in cases:
        values = [
            runtime.python_inline_command_payloads(tokens),
            static.python_inline_command_payloads(tokens),
            shared.python_inline_command_payloads(tokens),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"python_inline_command_payloads({tokens!r}) returned {values!r}")


def assert_static_parity_exports_are_explicit() -> None:
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_client_under_test")
    shared = load_shared_module()
    exported_names = (
        "cargo_subcommand_with_index",
        "nextest_subcommand_with_index",
        "python_call_command_argument",
        "python_call_name",
        "python_command_string",
        "python_constant_string",
    )
    expected = tuple(getattr(shared, name) for name in exported_names)
    actual = getattr(static, "COMMAND_UNDERSTANDING_PARITY_EXPORTS", None)
    if actual != expected:
        raise AssertionError("static command-understanding parity exports drifted")
    for name, expected_helper in zip(exported_names, expected, strict=True):
        if getattr(static, name) is not expected_helper:
            raise AssertionError(f"verify_ci_workflow_hygiene.{name} no longer re-exports the shared helper")


def assert_verifier_clients_use_shared_python_helpers() -> None:
    runtime = load_module(RUNTIME_VERIFIER, "rust_verification_client_under_test")
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_client_under_test")
    shared = load_shared_module()

    helper_names = [
        "python_constant_string",
        "python_command_string",
        "python_call_name",
        "python_call_command_argument",
        "python_inline_command_payloads",
    ]
    failures: list[str] = []
    for helper_name in helper_names:
        shared_helper = getattr(shared, helper_name)
        if getattr(runtime, helper_name) is not shared_helper:
            failures.append(f"rust_verification.{helper_name}")
        if getattr(static, helper_name) is not shared_helper:
            failures.append(f"verify_ci_workflow_hygiene.{helper_name}")
    if failures:
        raise AssertionError("verifier clients must import shared helpers: " + ", ".join(failures))


def assert_shared_cargo_scanner_helpers_match_current_verifiers() -> None:
    runtime = load_module(RUNTIME_VERIFIER, "rust_verification_cargo_shared_under_test")
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_cargo_shared_under_test")
    shared = load_shared_module()

    helper_names = [
        "cargo_subcommand_with_index",
        "cargo_subcommand",
        "nextest_subcommand_with_index",
        "cargo_args_for_target_routing_scan",
    ]
    missing = [helper_name for helper_name in helper_names if not hasattr(shared, helper_name)]
    if missing:
        raise AssertionError("shared cargo scanner helpers missing: " + ", ".join(missing))

    identity_failures: list[str] = []
    for helper_name in helper_names:
        shared_helper = getattr(shared, helper_name)
        if getattr(runtime, helper_name) is not shared_helper:
            identity_failures.append(f"rust_verification.{helper_name}")
        if getattr(static, helper_name) is not shared_helper:
            identity_failures.append(f"verify_ci_workflow_hygiene.{helper_name}")
    if identity_failures:
        raise AssertionError("verifier clients must import shared cargo scanner helpers: " + ", ".join(identity_failures))

    if static.CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT is not shared.CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
        raise AssertionError(
            "static CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT must use the shared cargo scanner constant"
        )

    cargo_subcommand_cases = [
        (["--manifest-path", "Cargo.toml", "test", "--", "--target-dir", "/tmp/raw"], (2, "test")),
        (["--manifest-path=Cargo.toml", "test"], (1, "test")),
        (["-j", "4", "test"], (2, "test")),
        (["--locked", "nextest", "run"], (1, "nextest")),
        (["--unknown-cargo-flag", "build"], (1, "build")),
        (["--version", "build"], (1, "build")),
        (["+nightly", "--offline", "check"], (2, "check")),
        (["--locked", "--frozen"], None),
    ]
    for cargo_args, expected in cargo_subcommand_cases:
        values = [
            runtime.cargo_subcommand_with_index(cargo_args),
            static.cargo_subcommand_with_index(cargo_args),
            shared.cargo_subcommand_with_index(cargo_args),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"cargo_subcommand_with_index({cargo_args!r}) returned {values!r}")

    start_tokens = ["cargo", "--manifest-path", "Cargo.toml", "test"]
    start_values = [
        static.cargo_subcommand_with_index(start_tokens, start=1),
        shared.cargo_subcommand_with_index(start_tokens, start=1),
    ]
    if start_values != [(3, "test"), (3, "test")]:
        raise AssertionError(f"cargo_subcommand_with_index start offset returned {start_values!r}")

    cargo_command_values = [
        runtime.cargo_subcommand(["--locked", "nextest", "run"]),
        static.cargo_subcommand(["--locked", "nextest", "run"]),
        shared.cargo_subcommand(["--locked", "nextest", "run"]),
    ]
    if cargo_command_values != ["nextest", "nextest", "nextest"]:
        raise AssertionError(f"cargo_subcommand returned {cargo_command_values!r}")

    nextest_values = [
        runtime.nextest_subcommand_with_index(["--profile", "ci", "run", "--archive-file", "archive"]),
        static.nextest_subcommand_with_index(["--profile", "ci", "run", "--archive-file", "archive"]),
        shared.nextest_subcommand_with_index(["--profile", "ci", "run", "--archive-file", "archive"]),
    ]
    if nextest_values != [(2, "run"), (2, "run"), (2, "run")]:
        raise AssertionError(f"nextest_subcommand_with_index returned {nextest_values!r}")

    target_scan_cases = [
        (["test", "--", "--target-dir", "/tmp/raw"], ["test"]),
        (["bench", "--", "--target-dir", "/tmp/raw"], ["bench"]),
        (["run", "--", "--target-dir", "/tmp/raw"], ["run"]),
        (
            ["nextest", "run", "--archive-file", "archive", "--", "--target-dir", "/tmp/raw"],
            ["nextest", "run", "--archive-file", "archive"],
        ),
        (["build", "--", "--target-dir", "/tmp/raw"], ["build", "--", "--target-dir", "/tmp/raw"]),
    ]
    for cargo_args, expected in target_scan_cases:
        values = [
            runtime.cargo_args_for_target_routing_scan(cargo_args),
            static.cargo_args_for_target_routing_scan(cargo_args),
            shared.cargo_args_for_target_routing_scan(cargo_args),
        ]
        if values != [expected, expected, expected]:
            raise AssertionError(f"cargo_args_for_target_routing_scan({cargo_args!r}) returned {values!r}")

    nextest_separator_values = [
        runtime.nextest_subcommand_with_index(["--"]),
        static.nextest_subcommand_with_index(["--"]),
        shared.nextest_subcommand_with_index(["--"]),
    ]
    if nextest_separator_values != [None, None, None]:
        raise AssertionError(f"nextest_subcommand_with_index separator returned {nextest_separator_values!r}")


def assert_non_exported_candidate_helpers_are_characterized() -> None:
    runtime = load_module(RUNTIME_VERIFIER, "rust_verification_candidates_under_test")
    static = load_module(STATIC_VERIFIER, "verify_ci_workflow_hygiene_candidates_under_test")
    shared = load_shared_module()

    non_exports = [
        "command_tokens",
        "shell_command_substitution_payloads",
        "shell_command_substitution_at",
        "path_name_looks_like_renamed_cargo",
        "path_executable_looks_like_cargo",
        "path_name_looks_like_renamed_rustc",
        "path_executable_looks_like_rustc",
        "cargo_target_routing_override",
        "tokens_have_target_routing_override",
        "process_wrapper_tokens",
        "wrapper_inner_tokens",
    ]
    leaked = [helper_name for helper_name in non_exports if hasattr(shared, helper_name)]
    if leaked:
        raise AssertionError("unproven helpers must not be shared exports: " + ", ".join(leaked))

    command = "cargo build&&cargo test"
    runtime_tokens = runtime.command_tokens(command)
    static_tokens = static.command_tokens(command)
    if runtime_tokens != ["cargo", "build&&cargo", "test"]:
        raise AssertionError(f"runtime command_tokens boundary changed: {runtime_tokens!r}")
    if static_tokens != ["cargo", "build", "&&", "cargo", "test"]:
        raise AssertionError(f"static command_tokens boundary changed: {static_tokens!r}")

    substitution_tokens = ["echo", "$(", "cargo", ")"]
    runtime_payloads = runtime.shell_command_substitution_payloads(substitution_tokens)
    static_payloads = static.shell_command_substitution_payloads(substitution_tokens)
    if runtime_payloads != [["cargo"]] or static_payloads != []:
        raise AssertionError(
            "shell_command_substitution_payloads divergence changed: "
            f"runtime={runtime_payloads!r} static={static_payloads!r}"
        )

    prefix_tokens = ["prefix$", "(", "cargo", ")"]
    runtime_substitution = runtime.shell_command_substitution_at(prefix_tokens, 0)
    static_substitution = static.shell_command_substitution_at(prefix_tokens, 0)
    if runtime_substitution is not None or static_substitution != (["cargo"], 4):
        raise AssertionError(
            "shell_command_substitution_at prefix-$ divergence changed: "
            f"runtime={runtime_substitution!r} static={static_substitution!r}"
        )

    if runtime.path_name_looks_like_renamed_cargo("rustup") is not True:
        raise AssertionError("runtime rustup-as-cargo path-name classification changed")
    if static.path_name_looks_like_renamed_cargo("rustup") is not False:
        raise AssertionError("static rustup-as-cargo path-name classification changed")

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        cargo_target = tmp_path / "cargo"
        cargo_target.write_text("", encoding="utf-8")
        cargo_link = tmp_path / "tool"
        cargo_link.symlink_to(cargo_target)
        runtime_cargo = runtime.path_executable_looks_like_cargo(str(cargo_link))
        static_cargo = static.path_executable_looks_like_cargo(str(cargo_link))
        if runtime_cargo is not True or static_cargo is not False:
            raise AssertionError(
                "cargo symlink executable classification changed: "
                f"runtime={runtime_cargo!r} static={static_cargo!r}"
            )

        rustc_target = tmp_path / "rustc"
        rustc_target.write_text("", encoding="utf-8")
        rustc_link = tmp_path / "compiler"
        rustc_link.symlink_to(rustc_target)
        runtime_rustc = runtime.path_executable_looks_like_rustc(str(rustc_link))
        static_rustc = static.path_executable_looks_like_rustc(str(rustc_link))
        if runtime_rustc is not True or static_rustc is not False:
            raise AssertionError(
                "rustc symlink executable classification changed: "
                f"runtime={runtime_rustc!r} static={static_rustc!r}"
            )

    cargo_args = ["--manifest-path", "Cargo.toml", "test", "--", "--target-dir", "/tmp/raw"]
    if runtime.cargo_subcommand_with_index(cargo_args) != (2, "test"):
        raise AssertionError("runtime cargo_subcommand_with_index changed")
    if static.cargo_subcommand_with_index(cargo_args) != (2, "test"):
        raise AssertionError("static cargo_subcommand_with_index default changed")
    if static.cargo_subcommand_with_index(["cargo", *cargo_args], start=1) != (3, "test"):
        raise AssertionError("static cargo_subcommand_with_index start-offset behavior changed")

    if runtime.process_wrapper_tokens(["command", "--", "cargo", "build"]) != ["cargo", "build"]:
        raise AssertionError("runtime process_wrapper_tokens representative behavior changed")
    if static.wrapper_inner_tokens(["command", "--", "cargo", "build"]) != ["cargo", "build"]:
        raise AssertionError("static wrapper_inner_tokens representative behavior changed")

    if runtime.cargo_target_routing_override(["test", "--target-dir", "/tmp/raw"]) != "--target-dir":
        raise AssertionError("runtime target-routing override detection changed")
    if static.tokens_have_target_routing_override(["cargo", "test", "--target-dir", "/tmp/raw"]) is not True:
        raise AssertionError("static target-routing override detection changed")
    if runtime.cargo_target_routing_override(["test", "--", "--target-dir", "/tmp/raw"]) is not None:
        raise AssertionError("runtime post-separator target-routing handling changed")
    if static.tokens_have_target_routing_override(["cargo", "test", "--", "--target-dir", "/tmp/raw"]) is not False:
        raise AssertionError("static post-separator target-routing handling changed")


def main() -> int:
    assert_test_import_setup_is_encapsulated()
    assert_import_setup_rejects_bare_top_level_sys_path_mutation()
    assert_import_setup_rejects_aliased_top_level_sys_path_mutation()
    assert_verifier_modules_import_from_repo_root()
    assert_load_module_caches_verifier_modules()
    assert_python_ast_helpers_match_current_verifiers()
    assert_python_inline_payloads_match_current_verifiers()
    assert_static_parity_exports_are_explicit()
    assert_verifier_clients_use_shared_python_helpers()
    assert_shared_cargo_scanner_helpers_match_current_verifiers()
    assert_non_exported_candidate_helpers_are_characterized()
    print("OK: command understanding self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
