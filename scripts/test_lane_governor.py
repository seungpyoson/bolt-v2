#!/usr/bin/env python3
"""Self-tests for lane_governor and the local_lane_policy validator (#653)."""

from __future__ import annotations

import errno
import ast
import importlib.util
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def _load(name: str):
    path = Path(__file__).with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


RV = _load("rust_verification")
REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = Path(__file__).resolve().parent

_REPO_ORIGIN = "repo"
_TEMP_ORIGIN = "temp"

_REPO_ROOT_NAMES = frozenset({"REPO_ROOT", "SCRIPTS_DIR"})
_MUTATING_PATH_METHODS = frozenset(
    {
        "hardlink_to",
        "mkdir",
        "replace",
        "rmdir",
        "rmtree",
        "symlink_to",
        "touch",
        "unlink",
        "write_bytes",
        "write_text",
    }
)
_OS_MUTATORS = frozenset(
    {
        "link",
        "makedirs",
        "mkdir",
        "remove",
        "removedirs",
        "rename",
        "replace",
        "rmdir",
        "symlink",
        "unlink",
    }
)
_MANIFEST_PATH = SCRIPTS_DIR / "cheap_lane_discovered_unlabeled.manifest"
_GATE_ALIASES: dict[str, str] = {}
_JUST_DUMP_CACHE: dict | None = None
_DISCOVERY_CACHE: set[Path] | None = None

_INVOCATION_FORMS = {
    "python_interpreters": ("python", "python3", "sys.executable"),
    "subprocess_calls": ("run", "Popen", "call", "check_call", "check_output"),
    "loader_calls": (
        "spec_from_file_location",
        "SourceFileLoader",
        "import_module",
        "run_path",
        "run_module",
    ),
    "dynamic_code_calls": (
        "eval",
        "exec",
        "os.system",
        "os.popen",
        "os.exec*",
        "os.spawn*",
        "subprocess.getoutput",
        "subprocess.getstatusoutput",
        "pty.spawn",
    ),
    "mutating_path_methods": tuple(sorted(_MUTATING_PATH_METHODS)),
}


def _valid_lane_policy() -> dict:
    return {
        "enabled": True,
        "allowed_ci_env": "GITHUB_ACTIONS",
        "lock_dir": "/tmp/rust-verification-lanes",
        "acquire_timeout_seconds": 900,
        "heartbeat_seconds": 15,
        "poll_interval_seconds": 1,
        "cheap_lane_labels": [
            "local-gate:fmt-check",
            "local-gate:source-fence-static",
            "local-gate:ci-lint-workflow",
            "test_lane_governor.py",
            "verify_lane_governance.py",
        ],
        "cheap_lane_max_concurrent": 0,
    }


def _expect_policy_error(data: dict, fragment: str) -> None:
    try:
        RV.validate_local_lane_policy(data)
    except RV.PolicyError as exc:
        assert fragment in str(exc), f"expected {fragment!r} in {exc}"
        return
    raise AssertionError(f"expected PolicyError containing {fragment!r}")


def test_valid_lane_policy_passes() -> None:
    RV.validate_local_lane_policy({"local_lane_policy": _valid_lane_policy()})


def test_missing_lane_policy_rejected() -> None:
    _expect_policy_error({}, "local_lane_policy table is required")


def test_disabled_lane_policy_rejected() -> None:
    policy = _valid_lane_policy()
    policy["enabled"] = False
    _expect_policy_error({"local_lane_policy": policy}, "enabled must be true")


def test_relative_lock_dir_rejected() -> None:
    policy = _valid_lane_policy()
    policy["lock_dir"] = "var/lanes"
    _expect_policy_error({"local_lane_policy": policy}, "absolute path")


def test_env_expansion_lock_dir_rejected() -> None:
    for bad in ("/tmp/$USER/lanes", "~/lanes"):
        policy = _valid_lane_policy()
        policy["lock_dir"] = bad
        _expect_policy_error({"local_lane_policy": policy}, "must not contain")


def test_heartbeat_must_be_below_timeout() -> None:
    policy = _valid_lane_policy()
    policy["heartbeat_seconds"] = 900
    _expect_policy_error({"local_lane_policy": policy}, "less than acquire_timeout_seconds")


def test_poll_interval_must_not_exceed_heartbeat() -> None:
    policy = _valid_lane_policy()
    policy["heartbeat_seconds"] = 5
    policy["poll_interval_seconds"] = 6
    _expect_policy_error(
        {"local_lane_policy": policy},
        "poll_interval_seconds must be less than or equal to heartbeat_seconds",
    )


def test_non_positive_intervals_rejected() -> None:
    for key in ("acquire_timeout_seconds", "heartbeat_seconds", "poll_interval_seconds"):
        policy = _valid_lane_policy()
        policy[key] = 0
        _expect_policy_error({"local_lane_policy": policy}, key)


def test_cheap_lane_labels_must_be_a_string_list() -> None:
    for bad in ("local-gate:fmt-check", [True], [""], ["../escape"]):
        policy = _valid_lane_policy()
        policy["cheap_lane_labels"] = bad
        _expect_policy_error({"local_lane_policy": policy}, "cheap_lane_labels")


def test_cheap_lane_max_concurrent_must_be_a_non_negative_integer() -> None:
    for bad in (True, -1, "2"):
        policy = _valid_lane_policy()
        policy["cheap_lane_max_concurrent"] = bad
        _expect_policy_error({"local_lane_policy": policy}, "cheap_lane_max_concurrent")


def test_unknown_lane_policy_keys_rejected() -> None:
    policy = _valid_lane_policy()
    policy["cheap_lane_label_prefixes"] = ["verify_"]
    _expect_policy_error({"local_lane_policy": policy}, "cheap_lane_label_prefixes")


def test_repo_policy_file_declares_lane_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    assert "local_lane_policy" in data, "ci/rust-verification.toml must declare [local_lane_policy]"


def _expr_label(node: ast.AST) -> str:
    try:
        return ast.unparse(node)
    except Exception:
        return type(node).__name__


class _RepoSharedStateWriteAnalyzer(ast.NodeVisitor):
    def __init__(self, path: Path, *, relative_paths_are_repo: bool = False) -> None:
        self.path = path
        self.findings: list[str] = []
        self.origins: dict[str, str] = {name: _REPO_ORIGIN for name in _REPO_ROOT_NAMES}
        self.relative_paths_are_repo = relative_paths_are_repo
        self.os_modules = {"os"}
        self.shutil_modules = {"shutil"}
        self.tempfile_modules = {"tempfile"}
        self.tempdir_names = {"TemporaryDirectory"}
        self.pathlib_modules = {"pathlib"}
        self.path_names = {"Path"}

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            name = alias.asname or alias.name
            if alias.name == "os":
                self.os_modules.add(name)
            elif alias.name == "shutil":
                self.shutil_modules.add(name)
            elif alias.name == "tempfile":
                self.tempfile_modules.add(name)
            elif alias.name == "pathlib":
                self.pathlib_modules.add(name)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module == "tempfile":
            for alias in node.names:
                if alias.name == "TemporaryDirectory":
                    self.tempdir_names.add(alias.asname or alias.name)
        elif node.module == "pathlib":
            for alias in node.names:
                if alias.name == "Path":
                    self.path_names.add(alias.asname or alias.name)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_isolated_body(node.body)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_isolated_body(node.body)

    def _visit_isolated_body(self, body: list[ast.stmt]) -> None:
        original_origins = dict(self.origins)
        try:
            for statement in body:
                self.visit(statement)
        finally:
            self.origins = original_origins

    def visit_Assign(self, node: ast.Assign) -> None:
        origin = self._origin(node.value)
        for target in node.targets:
            self._assign_origin(target, origin)
        self.visit(node.value)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        origin = self._origin(node.value) if node.value is not None else None
        self._assign_origin(node.target, origin)
        if node.value is not None:
            self.visit(node.value)

    def visit_For(self, node: ast.For) -> None:
        origin = self._origin(node.iter)
        self._assign_origin(node.target, origin)
        self.visit(node.iter)
        for statement in node.body:
            self.visit(statement)
        for statement in node.orelse:
            self.visit(statement)

    def visit_With(self, node: ast.With) -> None:
        original_origins = dict(self.origins)
        try:
            for item in node.items:
                if self._is_temporary_directory_call(item.context_expr):
                    temp_origin = self._temporary_directory_origin(item.context_expr)
                    if item.optional_vars is not None:
                        self._assign_origin(item.optional_vars, temp_origin)
                    if temp_origin == _REPO_ORIGIN:
                        self.findings.append(
                            self._finding(
                                item.context_expr,
                                "tempfile.TemporaryDirectory",
                                "dir=REPO_ROOT",
                            )
                        )
                    continue
                self.visit(item.context_expr)
            for statement in node.body:
                self.visit(statement)
        finally:
            self.origins = original_origins

    def visit_Call(self, node: ast.Call) -> None:
        for operation, target in self._mutating_targets(node):
            if self._origin(target) == _REPO_ORIGIN:
                self.findings.append(
                    self._finding(node, operation, _expr_label(target))
                )
        self.generic_visit(node)

    def _assign_origin(self, target: ast.AST, origin: str | None) -> None:
        if isinstance(target, ast.Name):
            if target.id in _REPO_ROOT_NAMES:
                self.origins[target.id] = _REPO_ORIGIN
            elif origin is None:
                self.origins.pop(target.id, None)
            else:
                self.origins[target.id] = origin
        elif isinstance(target, (ast.Tuple, ast.List)):
            for element in target.elts:
                self._assign_origin(element, origin)

    def _origin(self, node: ast.AST | None) -> str | None:
        if node is None:
            return None
        if (
            self.relative_paths_are_repo
            and isinstance(node, ast.Constant)
            and isinstance(node.value, str)
            and node.value
            and not Path(node.value).is_absolute()
        ):
            return _REPO_ORIGIN
        if self._is_file_anchored_repo_path(node):
            return _REPO_ORIGIN
        if isinstance(node, ast.Name):
            if node.id in _REPO_ROOT_NAMES:
                return _REPO_ORIGIN
            return self.origins.get(node.id)
        if isinstance(node, ast.Attribute):
            if node.attr in _REPO_ROOT_NAMES:
                return _REPO_ORIGIN
            return self._origin(node.value)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
            return self._merge_origin(self._origin(node.left), self._origin(node.right))
        if isinstance(node, ast.JoinedStr):
            return self._merge_origin(*(self._origin(value) for value in node.values))
        if isinstance(node, ast.Call):
            return self._call_origin(node)
        if isinstance(node, ast.Subscript):
            return self._origin(node.value)
        return None

    def _call_origin(self, node: ast.Call) -> str | None:
        func = node.func
        if isinstance(func, ast.Name) and func.id == "repo_path":
            return _REPO_ORIGIN
        if isinstance(func, ast.Name) and func.id == "str" and node.args:
            return self._origin(node.args[0])
        if self._is_path_constructor_call(func) and node.args:
            return self._origin(node.args[0])
        if isinstance(func, ast.Attribute):
            if (
                func.attr == "fspath"
                and isinstance(func.value, ast.Name)
                and func.value.id in self.os_modules
                and node.args
            ):
                return self._origin(node.args[0])
            if func.attr in {
                "absolute",
                "expanduser",
                "glob",
                "iterdir",
                "joinpath",
                "relative_to",
                "resolve",
                "rglob",
                "with_name",
                "with_stem",
                "with_suffix",
            }:
                return self._origin(func.value)
        return None

    def _is_path_constructor_call(self, func: ast.AST) -> bool:
        if isinstance(func, ast.Name) and func.id in self.path_names:
            return True
        return (
            isinstance(func, ast.Attribute)
            and func.attr == "Path"
            and isinstance(func.value, ast.Name)
            and func.value.id in self.pathlib_modules
        )

    def _is_file_anchored_repo_path(self, node: ast.AST) -> bool:
        if self._is_path_file_chain(node):
            return True
        if isinstance(node, ast.Attribute) and node.attr == "parent":
            return self._is_path_file_chain(node.value)
        if (
            isinstance(node, ast.Subscript)
            and isinstance(node.value, ast.Attribute)
            and node.value.attr == "parents"
            and self._is_path_file_chain(node.value.value)
        ):
            return True
        return False

    def _is_path_file_chain(self, node: ast.AST) -> bool:
        if (
            isinstance(node, ast.Call)
            and self._is_path_constructor_call(node.func)
            and len(node.args) == 1
            and isinstance(node.args[0], ast.Name)
            and node.args[0].id == "__file__"
        ):
            return True
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            if node.func.attr in {"resolve", "absolute"}:
                return self._is_path_file_chain(node.func.value)
        if isinstance(node, ast.Attribute) and node.attr == "parent":
            return self._is_path_file_chain(node.value)
        if (
            isinstance(node, ast.Subscript)
            and isinstance(node.value, ast.Attribute)
            and node.value.attr == "parents"
        ):
            return self._is_path_file_chain(node.value.value)
        return False

    def _merge_origin(self, *origins: str | None) -> str | None:
        if _REPO_ORIGIN in origins:
            return _REPO_ORIGIN
        if _TEMP_ORIGIN in origins:
            return _TEMP_ORIGIN
        return None

    def _is_module_call(self, node: ast.Call, modules: set[str]) -> str | None:
        func = node.func
        if (
            isinstance(func, ast.Attribute)
            and isinstance(func.value, ast.Name)
            and func.value.id in modules
        ):
            return func.attr
        return None

    def _is_temporary_directory_call(self, node: ast.AST) -> bool:
        return isinstance(node, ast.Call) and (
            (isinstance(node.func, ast.Name) and node.func.id in self.tempdir_names)
            or self._is_module_call(node, self.tempfile_modules) == "TemporaryDirectory"
        )

    def _temporary_directory_origin(self, node: ast.Call) -> str:
        for keyword in node.keywords:
            if keyword.arg == "dir" and self._origin(keyword.value) == _REPO_ORIGIN:
                return _REPO_ORIGIN
        return _TEMP_ORIGIN

    def _mutating_targets(self, node: ast.Call) -> list[tuple[str, ast.AST]]:
        targets: list[tuple[str, ast.AST]] = []
        func = node.func
        if isinstance(func, ast.Attribute):
            method = func.attr
            if method in _MUTATING_PATH_METHODS:
                targets.append((method, func.value))
            elif method in {"rename", "replace"}:
                targets.append((method, func.value))
                if node.args:
                    targets.append((method, node.args[0]))
            elif method == "open" and self._open_mode_writes(node, 0):
                targets.append((method, func.value))

            shutil_attr = self._is_module_call(node, self.shutil_modules)
            if shutil_attr == "rmtree" and node.args:
                targets.append((shutil_attr, node.args[0]))
            elif shutil_attr == "move" and node.args:
                for arg in node.args:
                    targets.append((shutil_attr, arg))
            elif (shutil_attr or "").startswith("copy") and len(node.args) >= 2:
                targets.append((shutil_attr, node.args[1]))

            os_attr = self._is_module_call(node, self.os_modules)
            if os_attr in _OS_MUTATORS - {"rename", "replace", "link", "symlink"} and node.args:
                targets.append((os_attr, node.args[0]))
            elif os_attr in {"rename", "replace"} and node.args:
                for arg in node.args[:2]:
                    targets.append((os_attr, arg))
            elif os_attr in {"link", "symlink"} and len(node.args) >= 2:
                targets.append((os_attr, node.args[1]))
            elif os_attr == "open" and node.args and self._os_open_flags_write(node):
                targets.append((os_attr, node.args[0]))

        if isinstance(func, ast.Name) and func.id == "open" and node.args:
            if self._open_mode_writes(node, 1):
                targets.append(("open", node.args[0]))
        return targets

    def _open_mode_writes(self, node: ast.Call, positional_index: int) -> bool:
        mode: ast.AST | None = None
        if len(node.args) > positional_index:
            mode = node.args[positional_index]
        for keyword in node.keywords:
            if keyword.arg == "mode":
                mode = keyword.value
        return (
            isinstance(mode, ast.Constant)
            and isinstance(mode.value, str)
            and any(flag in mode.value for flag in ("w", "a", "x", "+"))
        )

    def _os_open_flags_write(self, node: ast.Call) -> bool:
        flags: ast.AST | None = node.args[1] if len(node.args) >= 2 else None
        for keyword in node.keywords:
            if keyword.arg == "flags":
                flags = keyword.value
        return self._os_flags_include_write(flags)

    def _os_flags_include_write(self, node: ast.AST | None) -> bool:
        if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name) and node.value.id in self.os_modules:
            return node.attr in {"O_APPEND", "O_CREAT", "O_RDWR", "O_TRUNC", "O_WRONLY"}
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):
            return self._os_flags_include_write(node.left) or self._os_flags_include_write(node.right)
        return False

    def _finding(self, node: ast.AST, operation: str, target: str) -> str:
        rel = self.path.relative_to(REPO_ROOT)
        return f"{rel}:{node.lineno}: {operation} targets shared repo state: {target}"


def _cheap_lane_labels() -> list[str]:
    policy = RV.load_policy(REPO_ROOT)["local_lane_policy"]
    labels = policy.get("cheap_lane_labels", [])
    assert isinstance(labels, list), "cheap_lane_labels must be a list"
    return labels


def _cheap_labeled_python_scripts(labels: list[str] | None = None) -> set[Path]:
    labels = _cheap_lane_labels() if labels is None else labels
    missing = sorted(
        label
        for label in labels
        if isinstance(label, str)
        and not label.startswith("local-gate:")
        and not (SCRIPTS_DIR / label).is_file()
    )
    assert not missing, f"cheap lane script labels must exist: {missing}"
    non_python = sorted(
        label
        for label in labels
        if isinstance(label, str)
        and not label.startswith("local-gate:")
        and (SCRIPTS_DIR / label).is_file()
        and not _is_python_script_path(SCRIPTS_DIR / label)
    )
    assert not non_python, f"cheap lane script labels must be Python scripts: {non_python}"
    return {
        SCRIPTS_DIR / label
        for label in labels
        if isinstance(label, str)
        and not label.startswith("local-gate:")
        and (SCRIPTS_DIR / label).is_file()
        and _is_python_script_path(SCRIPTS_DIR / label)
    }


def _cheap_lane_python_scripts() -> set[Path]:
    return _discover_cheap_lane_scripts()


def _just_dump() -> dict:
    global _JUST_DUMP_CACHE
    if _JUST_DUMP_CACHE is not None:
        return _JUST_DUMP_CACHE
    result = subprocess.run(
        ["just", "--dump", "--dump-format", "json"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, f"just --dump failed: {result.stderr.strip()}"
    try:
        dump = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise AssertionError(f"just --dump emitted invalid JSON: {exc}") from exc
    _validate_just_dump_shape(dump)
    _JUST_DUMP_CACHE = dump
    return dump


def _validate_just_dump_shape(dump: object) -> None:
    assert isinstance(dump, dict), "just dump must be an object"
    for key in ("recipes", "assignments", "settings"):
        assert key in dump, f"just dump missing required key: {key}"
    assert isinstance(dump["recipes"], dict), "just dump recipes must be an object"
    assert isinstance(dump["assignments"], dict), "just dump assignments must be an object"


def _eval_assignment(expr: object, dump: dict, stack: tuple[str, ...] = ()) -> str:
    if isinstance(expr, dict) and "value" in expr:
        return _eval_assignment(expr["value"], dump, stack)
    if isinstance(expr, str):
        return expr
    if not isinstance(expr, list) or not expr:
        raise AssertionError(f"unrecognized just expression: {expr!r}")
    op = expr[0]
    if op == "concatenate":
        return "".join(_eval_assignment(part, dump, stack) for part in expr[1:])
    if op == "variable" and len(expr) == 2 and isinstance(expr[1], str):
        name = expr[1]
        if name in stack:
            raise AssertionError(f"recursive just assignment: {' -> '.join(stack + (name,))}")
        assignments = dump.get("assignments", {})
        assert isinstance(assignments, dict)
        if name not in assignments:
            raise AssertionError(f"unresolved just variable: {name}")
        return _eval_assignment(assignments[name], dump, stack + (name,))
    if op == "call" and len(expr) >= 2 and expr[1] == "justfile_directory":
        return str(REPO_ROOT)
    if op == "call" and len(expr) == 3 and expr[1] == "env_var" and isinstance(expr[2], str):
        name = expr[2]
        if name not in os.environ:
            raise AssertionError(f"just env_var({name}) is unset")
        return os.environ[name]
    raise AssertionError(f"unrecognized just expression: {expr!r}")


def _flatten_just_fragments(fragment: object, dump: dict, *, strict: bool = True) -> str:
    if isinstance(fragment, str):
        return fragment
    if isinstance(fragment, list):
        if _is_just_expression(fragment):
            try:
                return _eval_assignment(fragment, dump)
            except AssertionError:
                if strict:
                    raise
                return "{{" + repr(fragment) + "}}"
        return "".join(_flatten_just_fragments(part, dump, strict=strict) for part in fragment)
    raise AssertionError(f"unrecognized just body fragment: {fragment!r}")


def _is_just_expression(fragment: list[object]) -> bool:
    return bool(fragment) and isinstance(fragment[0], str) and fragment[0] in {"call", "concatenate", "variable"}


def _recipe_command_lines(recipe: dict, dump: dict, *, strict: bool = True) -> list[str]:
    body = recipe.get("body")
    assert isinstance(body, list), f"recipe {recipe.get('name', '<unknown>')} body must be a list"
    lines: list[str] = []
    for raw_line in body:
        line = _flatten_just_fragments(raw_line, dump, strict=strict).strip()
        if not line:
            continue
        lines.append(line)
        lines.extend(_shell_subcommands(line))
    return lines


def _shell_subcommands(line: str) -> list[str]:
    commands: list[str] = []
    for match in re.finditer(r"python3?\s+-c\s+('[^']*'|\"[^\"]*\")", line):
        commands.append(match.group(0))
    for payload in _command_substitution_payloads(line):
        payload = payload.strip()
        if payload and (
            "python" in payload.lower()
            or " just " in f" {payload} "
            or payload.startswith("$")
        ):
            commands.append(payload)
            commands.extend(
                part.strip()
                for part in re.split(r"(?<!\|)\|(?!\|)", payload)
                if part.strip()
                and (
                    "python" in part.lower()
                    or " just " in f" {part} "
                    or part.strip().startswith("$")
                )
            )
    if "|" in line and not re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", line.strip()):
        commands.extend(
            part.strip()
            for part in re.split(r"(?<!\|)\|(?!\|)", line)
            if part.strip() and ("python" in part or " just " in f" {part} ")
        )
    stripped = line.strip()
    if stripped.startswith("if ! "):
        inner = stripped[5:].strip()
        if inner.endswith("; then"):
            inner = inner[:-6].strip()
        commands.append(inner)
    return commands


def _command_substitution_payloads(line: str) -> list[str]:
    payloads: list[str] = []
    index = 0
    while index < len(line) - 1:
        if line[index : index + 2] != "$(":
            index += 1
            continue
        start = index + 2
        cursor = start
        depth = 1
        quote: str | None = None
        while cursor < len(line):
            char = line[cursor]
            if quote == "'":
                if char == "'":
                    quote = None
                cursor += 1
                continue
            if quote == '"':
                if char == "\\":
                    cursor += 2
                    continue
                if char == '"':
                    quote = None
                cursor += 1
                continue
            if char == "\\":
                cursor += 2
                continue
            if char in {"'", '"'}:
                quote = char
                cursor += 1
                continue
            if line[cursor : cursor + 2] == "$(":
                depth += 1
                cursor += 2
                continue
            if char == ")":
                depth -= 1
                if depth == 0:
                    payload = line[start:cursor]
                    payloads.append(payload)
                    payloads.extend(_command_substitution_payloads(payload))
                    index = cursor + 1
                    break
            cursor += 1
        else:
            payloads.append(line[start:])
            break
    return payloads


def _shlex_tokens(line: str) -> list[str]:
    try:
        return shlex.split(line, comments=False, posix=True)
    except ValueError as exc:
        raise AssertionError(f"cannot parse shell command statically: {line!r}: {exc}") from exc


def _classify_command(line: str) -> str:
    stripped = line.strip()
    if not stripped or stripped.startswith("#") or stripped.startswith("#!"):
        return "none"
    if "{{" in stripped or "}}" in stripped:
        return "dynamic-shell"
    if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", stripped):
        return "none"
    if stripped.startswith("if ! "):
        stripped = stripped[5:].strip()
        if stripped.endswith("; then"):
            stripped = stripped[:-6].strip()
    tokens = _shlex_tokens(stripped)
    if not tokens:
        return "none"
    while tokens and _is_shell_assignment(tokens[0]):
        tokens = tokens[1:]
    if not tokens:
        return "none"
    command = tokens[0]
    if command in {"if", "then", "fi", "for", "do", "done", "else", "elif", "set", "shopt"}:
        return "boundary"
    if command == "env":
        env_tokens = _env_wrapped_command_tokens(tokens)
        if not env_tokens:
            return "boundary"
        if env_tokens[0].startswith("$"):
            return "dynamic-shell"
        return "py-exec" if _is_python_interpreter_token(env_tokens[0]) else "boundary"
    if command.startswith("$") or "*" in command or "?" in command:
        return "dynamic-shell"
    if _is_python_interpreter_token(command):
        return "py-exec"
    if command in {"bash", "sh"}:
        return "boundary"
    return "boundary"


def _is_shell_assignment(token: str) -> bool:
    return bool(re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", token))


def _env_wrapped_command_tokens(tokens: list[str]) -> list[str]:
    if not tokens or tokens[0] != "env":
        return tokens
    index = 1
    while index < len(tokens) and _is_shell_assignment(tokens[index]):
        index += 1
    return tokens[index:]


def _cheap_gate_closure(dump: dict, labels: set[str] | None = None) -> tuple[set[str], dict[str, str]]:
    _validate_just_dump_shape(dump)
    labels = set(_cheap_lane_labels()) if labels is None else labels
    recipes = dump["recipes"]
    assert isinstance(recipes, dict)
    gates: dict[str, str] = {}
    for recipe_name, recipe in recipes.items():
        assert isinstance(recipe, dict), f"recipe {recipe_name} must be an object"
        for line in _recipe_command_lines(recipe, dump, strict=False):
            parsed = _coordinator_gate_from_line(line)
            if parsed is None:
                continue
            gate = parsed
            if recipe.get("private") is True:
                raise AssertionError(f"private recipe {recipe_name} invokes local_verification_gate.py")
            gates[gate] = recipe_name

    _validate_gates(gates, labels, recipes)

    closure = set(gates.values())
    queue = list(closure)
    while queue:
        recipe_name = queue.pop(0)
        recipe = recipes.get(recipe_name)
        if not isinstance(recipe, dict):
            raise AssertionError(f"recipe in cheap-gate closure is missing: {recipe_name}")
        for dep in recipe.get("dependencies", []):
            if not isinstance(dep, dict) or not isinstance(dep.get("recipe"), str):
                raise AssertionError(f"unrecognized dependency in {recipe_name}: {dep!r}")
            dep_name = dep["recipe"]
            if dep_name not in closure:
                closure.add(dep_name)
                queue.append(dep_name)
        for line in _recipe_command_lines(recipe, dump):
            target = _routed_just_recipe(line)
            if target is not None and target not in closure:
                if target not in recipes:
                    raise AssertionError(f"cheap gate routes to missing recipe: {target}")
                closure.add(target)
                queue.append(target)
    return closure, gates


def _coordinator_gate_from_line(line: str) -> str | None:
    if "local_verification_gate.py" not in line:
        return None
    tokens = _shlex_tokens(line)
    for index, token in enumerate(tokens):
        if Path(token).name == "local_verification_gate.py":
            if index + 1 >= len(tokens):
                raise AssertionError(f"missing gate name in coordinator call: {line}")
            return tokens[index + 1]
    return None


def _routed_just_recipe(line: str) -> str | None:
    if "-- just " not in line:
        return None
    tokens = _shlex_tokens(line)
    for index, token in enumerate(tokens):
        if token == "--" and tokens[index + 1 : index + 3] and tokens[index + 1] == "just":
            if index + 2 >= len(tokens):
                raise AssertionError(f"local gate route missing just recipe: {line}")
            return tokens[index + 2]
    return None


def _validate_gates(gates: dict[str, str], labels: set[str], recipes: dict) -> None:
    labeled_gates = {label.removeprefix("local-gate:") for label in labels if label.startswith("local-gate:")}
    derived_gates = set(gates)
    missing_recipe = sorted(labeled_gates - derived_gates)
    missing_label = sorted(derived_gates - labeled_gates)
    assert not missing_recipe, f"local-gate labels without coordinator recipe: {missing_recipe}"
    assert not missing_label, f"coordinator recipes without local-gate labels: {missing_label}"
    for gate, recipe_name in gates.items():
        expected_recipe = _GATE_ALIASES.get(gate, gate)
        assert recipe_name == expected_recipe, (
            f"coordinator gate {gate} must be invoked by recipe {expected_recipe}, got {recipe_name}"
        )
        inner = f"{gate}-inner"
        assert inner in recipes, f"cheap gate {gate} missing inner recipe {inner}"
        recipe = recipes.get(recipe_name)
        assert isinstance(recipe, dict) and recipe.get("private") is not True, (
            f"cheap gate recipe {recipe_name} must be public"
        )


def _closure_python_scripts(dump: dict, recipes: set[str]) -> set[Path]:
    scripts: set[Path] = set()
    all_recipes = dump["recipes"]
    for recipe_name in sorted(recipes):
        recipe = all_recipes[recipe_name]
        for line in _recipe_command_lines(recipe, dump):
            classification = _classify_command(line)
            if classification == "dynamic-shell":
                raise AssertionError(f"unsupported dynamic shell command in cheap closure {recipe_name}: {line}")
            if classification != "py-exec":
                continue
            tokens = _shlex_tokens(_normalize_shell_python_line(line))
            target = _python_script_from_tokens(tokens, scan_set=scripts)
            if target is not None:
                scripts.add(target)
    return scripts


def _normalize_shell_python_line(line: str) -> str:
    return " ".join(shlex.quote(token) for token in _normalized_shell_tokens(line))


def _normalized_shell_tokens(line: str) -> list[str]:
    stripped = line.strip()
    if stripped.startswith("if ! "):
        stripped = stripped[5:].strip()
        if stripped.endswith("; then"):
            stripped = stripped[:-6].strip()
    tokens = _shlex_tokens(stripped)
    while tokens and _is_shell_assignment(tokens[0]):
        tokens = tokens[1:]
    tokens = _env_wrapped_command_tokens(tokens)
    return tokens


def _python_script_from_tokens(tokens: list[str], scan_set: set[Path]) -> Path | None:
    if not tokens:
        return None
    if not _is_python_interpreter_token(tokens[0]):
        return None
    operand_index = _python_operand_index(tokens)
    if operand_index >= len(tokens):
        raise AssertionError(f"Python command missing operand: {tokens}")
    if tokens[operand_index] == "-c":
        if len(tokens) > operand_index + 1:
            _validate_inline_python_payload(tokens[operand_index + 1], scan_set)
        return None
    if tokens[operand_index] == "-m":
        if len(tokens) <= operand_index + 1:
            raise AssertionError(f"Python -m command missing module: {tokens}")
        return _script_from_module_name(tokens[operand_index + 1])
    return _resolve_script_operand(tokens[operand_index])


def _python_operand_index(tokens: list[object]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"-c", "-m"}:
            return index
        if isinstance(token, str) and token.startswith("-"):
            index += 1
            continue
        return index
    return index


def _resolve_script_operand(raw: str) -> Path | None:
    path = Path(raw)
    if not path.is_absolute():
        path = REPO_ROOT / path
    path = path.resolve()
    if path.exists() and _is_python_script_path(path):
        return path
    if raw.startswith("scripts/") or raw.endswith(".py"):
        raise AssertionError(f"unresolved Python script target: {raw}")
    return None


def _is_python_interpreter_token(token: object) -> bool:
    if token == "sys.executable":
        return True
    if not isinstance(token, str):
        return False
    name = Path(token).name
    return name in {"python", "python3"} or bool(re.fullmatch(r"python3?\.\d+", name))


def _script_from_module_name(module: str) -> Path | None:
    if module.startswith("scripts."):
        rel = module.removeprefix("scripts.")
    else:
        rel = module
    candidate = SCRIPTS_DIR / (rel.replace(".", "/") + ".py")
    if candidate.is_file():
        return candidate.resolve()
    return None


def _validate_inline_python_payload(payload: object, scan_set: set[Path]) -> None:
    if not isinstance(payload, str):
        return
    try:
        tree = ast.parse(payload, filename="<python -c>")
    except SyntaxError:
        tree = None
    if tree is not None:
        analyzer = _RepoSharedStateWriteAnalyzer(
            SCRIPTS_DIR / "inline_python_payload.py",
            relative_paths_are_repo=True,
        )
        analyzer.visit(tree)
        if analyzer.findings:
            raise AssertionError("inline python payload mutates shared repo state: " + "; ".join(analyzer.findings))
    for match in re.findall(r"scripts/[A-Za-z0-9_./-]+(?:\.py)?", payload):
        script = _resolve_script_operand(match)
        if script is not None and script not in scan_set:
            rel = script.relative_to(REPO_ROOT)
            raise AssertionError(f"inline python payload names unscanned script: {rel}")


def _is_python_script_path(path: Path) -> bool:
    if path.suffix == ".py":
        return True
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return False
    first_line = text.splitlines()[0] if text.splitlines() else ""
    if first_line.startswith("#!") and "python" in first_line:
        return True
    try:
        ast.parse(text, filename=str(path))
    except SyntaxError:
        return False
    return True


_UNRESOLVED = object()
_PARAMETER = object()


class _CodeExecutionEdgeResolver(ast.NodeVisitor):
    def __init__(self, path: Path, tree: ast.AST, *, scan_set: set[Path]) -> None:
        self.path = path
        self.tree = tree
        self.scan_set = scan_set
        self.targets: set[Path] = set()
        self.failures: list[str] = []
        self.scopes: list[dict[str, object]] = [{}]
        self.functions: dict[str, ast.FunctionDef | ast.AsyncFunctionDef] = {}
        self.active_functions: set[str] = set()
        self.subprocess_modules = {"subprocess"}
        self.os_modules = {"os"}
        self.pty_modules = {"pty"}
        self.sys_modules = {"sys"}
        self.pathlib_modules = {"pathlib"}
        self.tempfile_modules = {"tempfile"}
        self.path_names = {"Path"}
        self.temp_path_names = {"TemporaryDirectory", "NamedTemporaryFile", "mkdtemp", "mkstemp", "gettempdir"}
        self.spec_loader_names = {"spec_from_file_location"}
        self.source_loader_names = {"SourceFileLoader"}
        self.importlib_modules = {"importlib"}
        self.runpy_modules = {"runpy"}

    def resolve(self) -> tuple[set[Path], list[str]]:
        self._collect_function_defs(self.tree)
        self.visit(self.tree)
        return self.targets, self.failures

    def _collect_function_defs(self, tree: ast.AST) -> None:
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self.functions[node.name] = node

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            name = alias.asname or alias.name.split(".")[0]
            if alias.name == "subprocess":
                self.subprocess_modules.add(name)
            elif alias.name == "os":
                self.os_modules.add(name)
            elif alias.name == "pty":
                self.pty_modules.add(name)
            elif alias.name == "sys":
                self.sys_modules.add(name)
            elif alias.name == "pathlib":
                self.pathlib_modules.add(name)
            elif alias.name == "tempfile":
                self.tempfile_modules.add(name)
            elif alias.name == "importlib":
                self.importlib_modules.add(name)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module == "tempfile":
            for alias in node.names:
                if alias.name in {"TemporaryDirectory", "NamedTemporaryFile", "mkdtemp", "mkstemp", "gettempdir"}:
                    self.temp_path_names.add(alias.asname or alias.name)
        elif node.module == "pathlib":
            for alias in node.names:
                if alias.name == "Path":
                    self.path_names.add(alias.asname or alias.name)
        elif node.module == "importlib.util":
            for alias in node.names:
                if alias.name == "spec_from_file_location":
                    self.spec_loader_names.add(alias.asname or alias.name)
        elif node.module == "importlib.machinery":
            for alias in node.names:
                if alias.name == "SourceFileLoader":
                    self.source_loader_names.add(alias.asname or alias.name)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.functions[node.name] = node
        self._visit_function_body(node, {})

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.functions[node.name] = node
        self._visit_function_body(node, {})

    def visit_If(self, node: ast.If) -> None:
        for name in self._assigned_names(node.body) | self._assigned_names(node.orelse):
            self.scopes[-1][name] = _UNRESOLVED
        self.visit(node.test)
        for statement in node.body:
            self.visit(statement)
        for statement in node.orelse:
            self.visit(statement)

    def visit_For(self, node: ast.For) -> None:
        for name in self._target_names(node.target):
            self.scopes[-1][name] = _UNRESOLVED
        self.visit(node.iter)
        for statement in node.body:
            self.visit(statement)
        for statement in node.orelse:
            self.visit(statement)

    def visit_With(self, node: ast.With) -> None:
        original_scope = dict(self.scopes[-1])
        try:
            for item in node.items:
                value = _PARAMETER if self._is_temp_path_source(item.context_expr) else _UNRESOLVED
                if item.optional_vars is not None and value is _PARAMETER:
                    for name in self._target_names(item.optional_vars):
                        self.scopes[-1][name] = value
                self.visit(item.context_expr)
            for statement in node.body:
                self.visit(statement)
        finally:
            self.scopes[-1] = original_scope

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        self.visit_With(node)

    def visit_Assign(self, node: ast.Assign) -> None:
        value = self._resolve_value(node.value)
        for target in node.targets:
            self._bind_target(target, value)
        self.visit(node.value)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        value = self._resolve_value(node.value) if node.value is not None else _UNRESOLVED
        self._bind_target(node.target, value)
        if node.value is not None:
            self.visit(node.value)

    def visit_Call(self, node: ast.Call) -> None:
        call_name = self._call_name(node.func)
        if self._is_dynamic_code_call(call_name):
            self._fail(node, f"dynamic code execution is not allowed: {call_name}")
        if self._is_os_exec_spawn_call(call_name):
            self._handle_os_exec_spawn_call(node, call_name)
        if self._is_subprocess_call(call_name):
            self._handle_subprocess_call(node)
        elif self._is_loader_call(call_name):
            self._handle_loader_call(node, call_name)
        self._handle_local_wrapper_call(node)
        self.generic_visit(node)

    def _visit_function_body(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
        bound_args: dict[str, object],
    ) -> None:
        parent = dict(self.scopes[-1])
        for arg in self._function_parameters(node):
            parent[arg.arg] = bound_args.get(arg.arg, _PARAMETER)
        self.scopes.append(parent)
        try:
            for statement in node.body:
                self.visit(statement)
        finally:
            self.scopes.pop()

    def _handle_local_wrapper_call(self, node: ast.Call) -> None:
        if not isinstance(node.func, ast.Name):
            return
        name = node.func.id
        if name not in self.functions or name in self.active_functions:
            return
        function = self.functions[name]
        if not self._function_may_wrap_execution(function):
            return
        bound: dict[str, object] = {}
        bound_any = False
        parameters = self._function_parameters(function)
        parameter_names = {arg.arg for arg in parameters}
        explicit_names: set[str] = set()
        for arg_def, arg_value in zip(parameters, node.args):
            value = self._resolve_value(arg_value)
            bound[arg_def.arg] = value
            explicit_names.add(arg_def.arg)
            bound_any = True
        for keyword in node.keywords:
            if keyword.arg is not None and keyword.arg in parameter_names:
                value = self._resolve_value(keyword.value)
                bound[keyword.arg] = value
                explicit_names.add(keyword.arg)
                bound_any = True
        for name, default in self._function_defaults(function).items():
            if name not in explicit_names and name not in bound:
                value = self._resolve_value(default)
                bound[name] = value
                bound_any = True
        if not bound_any:
            return
        self.active_functions.add(name)
        try:
            self._visit_function_body(function, bound)
        finally:
            self.active_functions.discard(name)

    def _function_parameters(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> list[ast.arg]:
        return [*node.args.posonlyargs, *node.args.args, *node.args.kwonlyargs]

    def _function_defaults(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> dict[str, ast.AST]:
        positional = [*node.args.posonlyargs, *node.args.args]
        padded_defaults: list[ast.AST | None] = [None] * (len(positional) - len(node.args.defaults))
        padded_defaults.extend(node.args.defaults)
        defaults = {
            arg.arg: default
            for arg, default in zip(positional, padded_defaults)
            if default is not None
        }
        for arg, default in zip(node.args.kwonlyargs, node.args.kw_defaults):
            if default is not None:
                defaults[arg.arg] = default
        return defaults

    def _function_may_wrap_execution(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
        seen: set[str] | None = None,
    ) -> bool:
        seen = set() if seen is None else set(seen)
        if node.name in seen:
            return False
        seen.add(node.name)
        for child in ast.walk(node):
            if not isinstance(child, ast.Call):
                continue
            call_name = self._call_name(child.func)
            if (
                self._is_subprocess_call(call_name)
                or self._is_loader_call(call_name)
                or self._is_os_exec_spawn_call(call_name)
            ):
                return True
            if call_name in self.functions and self._function_may_wrap_execution(self.functions[call_name], seen):
                return True
        return False

    def _handle_subprocess_call(self, node: ast.Call) -> None:
        command_node = self._command_argument(node)
        if command_node is None:
            return
        command = self._resolve_value(command_node)
        executable = self._subprocess_executable(node)
        shell = self._subprocess_shell_enabled(node)
        if executable is _UNRESOLVED:
            if self._command_may_require_python_executable(command):
                self._fail(node, "unresolved Python subprocess executable")
            return
        if executable is _PARAMETER and self._command_may_require_python_executable(command):
            return
        if isinstance(command, str):
            if shell:
                try:
                    tokens = _normalized_shell_tokens(command)
                except AssertionError as exc:
                    self._fail(node, str(exc))
                    return
                if tokens and _is_python_interpreter_token(tokens[0]):
                    self._handle_python_tokens(node, tokens)
                elif _classify_command(command) == "dynamic-shell":
                    self._fail(node, f"unsupported dynamic shell=True command: {command}")
                return
            if executable is not None and _is_python_interpreter_token(executable):
                self._handle_python_tokens(node, [executable, command])
                return
        if command is _UNRESOLVED:
            if self._looks_like_python_command_expr(command_node):
                self._fail(node, "unresolved Python subprocess command")
            return
        if command is _PARAMETER:
            return
        if not isinstance(command, list) or not command:
            return
        if executable is not None and _is_python_interpreter_token(executable):
            self._handle_python_tokens(node, [executable, *command])
            return
        executable = command[0]
        if executable is _UNRESOLVED:
            self._fail(node, "unresolved Python command executable")
            return
        if executable is _PARAMETER:
            return
        if not _is_python_interpreter_token(executable):
            return
        if len(command) < 2:
            self._fail(node, "Python subprocess command is missing a target")
            return
        self._handle_python_tokens(node, command)

    def _subprocess_shell_enabled(self, node: ast.Call) -> bool:
        for keyword in node.keywords:
            if keyword.arg == "shell":
                return self._resolve_value(keyword.value) is True
        return False

    def _subprocess_executable(self, node: ast.Call) -> object | None:
        for keyword in node.keywords:
            if keyword.arg == "executable":
                return self._resolve_value(keyword.value)
        return None

    def _command_may_require_python_executable(self, command: object) -> bool:
        if isinstance(command, str):
            try:
                tokens = _normalized_shell_tokens(command)
            except AssertionError:
                tokens = [command]
            return bool(tokens) and (
                _is_python_interpreter_token(tokens[0])
                or self._value_is_script_shaped(tokens[0])
            )
        if isinstance(command, list) and command:
            first = command[0]
            return _is_python_interpreter_token(first) or self._value_is_script_shaped(first)
        return False

    def _value_is_script_shaped(self, value: object) -> bool:
        if value is _UNRESOLVED or value is _PARAMETER:
            return False
        if isinstance(value, Path):
            return value.exists() and _is_python_script_path(value)
        if not isinstance(value, str):
            return False
        return value.startswith("scripts/") or value.endswith(".py")

    def _handle_python_tokens(self, node: ast.AST, tokens: list[object]) -> None:
        operand_index = _python_operand_index(tokens)
        if operand_index >= len(tokens):
            self._fail(node, "Python subprocess command is missing a target")
            return
        operand = tokens[operand_index]
        if operand == "-c":
            if len(tokens) > operand_index + 1 and isinstance(tokens[operand_index + 1], str):
                try:
                    _validate_inline_python_payload(tokens[operand_index + 1], self.scan_set)
                except AssertionError as exc:
                    self._fail(node, str(exc))
            return
        if operand == "-m":
            if len(tokens) <= operand_index + 1:
                self._fail(node, "Python -m command is missing a module")
                return
            module = tokens[operand_index + 1]
            if module is _UNRESOLVED:
                self._fail(node, "unresolved Python -m module")
            elif module is not _PARAMETER and isinstance(module, str):
                target = _script_from_module_name(module)
                if target is not None:
                    self.targets.add(target)
            return
        if operand is _PARAMETER:
            return
        if operand is _UNRESOLVED:
            self._fail(node, "unresolved Python script target")
            return
        target = self._path_from_value(operand)
        if target is None:
            self._fail(node, f"unrecognized Python script target: {operand!r}")
            return
        if target.exists() and _is_python_script_path(target):
            self.targets.add(target)
            return
        self._fail(node, f"unresolved Python script target: {target}")

    def _looks_like_python_command_expr(self, node: ast.AST) -> bool:
        if isinstance(node, (ast.List, ast.Tuple)) and node.elts:
            first = self._resolve_value(node.elts[0])
            return _is_python_interpreter_token(first)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            return self._looks_like_python_command_expr(node.left) or self._looks_like_python_command_expr(node.right)
        if isinstance(node, ast.Name):
            value = self._lookup(node.id)
            if isinstance(value, list) and value:
                return _is_python_interpreter_token(value[0])
        return False

    def _handle_os_exec_spawn_call(self, node: ast.Call, call_name: str) -> None:
        target_index = 1 if any(call_name.startswith(f"{module}.spawn") for module in self.os_modules) else 0
        if len(node.args) <= target_index:
            return
        target = self._resolve_value(node.args[target_index])
        if target is _PARAMETER or target is _UNRESOLVED:
            return
        if _is_python_interpreter_token(str(target)):
            self._fail(node, f"dynamic Python process replacement is not allowed: {call_name}")

    def _handle_loader_call(self, node: ast.Call, call_name: str) -> None:
        if call_name in {"importlib.import_module", "runpy.run_module"} or call_name.endswith(".import_module") or call_name.endswith(".run_module"):
            if not node.args:
                self._fail(node, f"{call_name} missing module name")
                return
            module = self._resolve_value(node.args[0])
            if module is _PARAMETER:
                return
            if module is _UNRESOLVED:
                self._fail(node, f"unresolved module load target in {call_name}")
                return
            if isinstance(module, str):
                target = _script_from_module_name(module)
                if target is not None:
                    self.targets.add(target)
            return

        index = 0 if call_name.endswith("run_path") else 1
        target_node = node.args[index] if len(node.args) > index else None
        if target_node is None:
            target_node = self._loader_keyword_target(node, call_name)
        if target_node is None:
            self._fail(node, f"{call_name} missing path target")
            return
        target_value = self._resolve_value(target_node)
        if target_value is _PARAMETER:
            return
        if target_value is _UNRESOLVED:
            self._fail(node, f"unresolved loader path target in {call_name}")
            return
        target = self._path_from_value(target_value)
        if target is None:
            self._fail(node, f"unrecognized loader target in {call_name}: {target_value!r}")
            return
        if target.exists() and _is_python_script_path(target):
            self.targets.add(target)
            return
        self._fail(node, f"loader target is not a resolvable Python script: {target}")

    def _loader_keyword_target(self, node: ast.Call, call_name: str) -> ast.AST | None:
        if call_name.endswith("spec_from_file_location"):
            keyword_names = {"location"}
        elif call_name.endswith("run_path"):
            keyword_names = {"path_name"}
        else:
            keyword_names = {"path"}
        for keyword in node.keywords:
            if keyword.arg in keyword_names:
                return keyword.value
        return None

    def _command_argument(self, node: ast.Call) -> ast.AST | None:
        if node.args:
            return node.args[0]
        for keyword in node.keywords:
            if keyword.arg in {"args", "command"}:
                return keyword.value
        return None

    def _resolve_value(self, node: ast.AST | None) -> object:
        if node is None:
            return _UNRESOLVED
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, ast.Name):
            if node.id == "__file__":
                return self.path
            return self._lookup(node.id)
        if isinstance(node, ast.Attribute):
            value = self._resolve_value(node.value)
            if isinstance(node.value, ast.Name) and node.value.id in self.sys_modules and node.attr == "executable":
                return "sys.executable"
            if isinstance(value, Path):
                if node.attr == "parent":
                    return value.parent
                if node.attr == "parents":
                    return value.parents
            return _PARAMETER if value is _PARAMETER else _UNRESOLVED
        if isinstance(node, ast.Subscript):
            value = self._resolve_value(node.value)
            index = self._resolve_value(node.slice)
            if isinstance(value, type(Path.cwd().parents)) and isinstance(index, int):
                return value[index]
            return _PARAMETER if value is _PARAMETER else _UNRESOLVED
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
            left = self._resolve_value(node.left)
            right = self._resolve_value(node.right)
            if left is _PARAMETER or right is _PARAMETER:
                return _PARAMETER
            if left is _UNRESOLVED or right is _UNRESOLVED:
                return _UNRESOLVED
            if isinstance(left, Path):
                return left / str(right)
            return _UNRESOLVED
        if isinstance(node, (ast.List, ast.Tuple)):
            return [self._resolve_value(element) for element in node.elts]
        if isinstance(node, ast.JoinedStr):
            parts: list[str] = []
            for value in node.values:
                resolved = self._resolve_value(value)
                if resolved is _PARAMETER:
                    return _PARAMETER
                if resolved is _UNRESOLVED:
                    return _UNRESOLVED
                parts.append(str(resolved))
            return "".join(parts)
        if isinstance(node, ast.FormattedValue):
            return self._resolve_value(node.value)
        if isinstance(node, ast.Call):
            return self._resolve_call_value(node)
        return _UNRESOLVED

    def _resolve_call_value(self, node: ast.Call) -> object:
        call_name = self._call_name(node.func)
        if self._is_temp_path_source(node):
            return _PARAMETER
        if call_name in self.path_names or call_name.endswith(".Path"):
            if not node.args:
                return Path()
            value = self._resolve_value(node.args[0])
            if value is _PARAMETER:
                return _PARAMETER
            if value is _UNRESOLVED:
                return _UNRESOLVED
            if value is None:
                return _UNRESOLVED
            try:
                return Path(value)
            except TypeError:
                return _UNRESOLVED
        if call_name == "str" and node.args:
            value = self._resolve_value(node.args[0])
            if value is _PARAMETER:
                return _PARAMETER
            if value is _UNRESOLVED:
                return _UNRESOLVED
            return str(value)
        if call_name == "list" and node.args:
            value = self._resolve_value(node.args[0])
            if value is _PARAMETER:
                return _PARAMETER
            if isinstance(value, list):
                return value
            return _UNRESOLVED
        if isinstance(node.func, ast.Attribute):
            owner = self._resolve_value(node.func.value)
            if owner is _PARAMETER:
                return _PARAMETER
            if isinstance(owner, Path):
                if node.func.attr in {"resolve", "absolute", "expanduser"}:
                    return owner.resolve() if node.func.attr != "expanduser" else owner.expanduser()
                if node.func.attr == "joinpath":
                    current = owner
                    for arg in node.args:
                        part = self._resolve_value(arg)
                        if part is _PARAMETER:
                            return _PARAMETER
                        if part is _UNRESOLVED:
                            return _UNRESOLVED
                        current = current / str(part)
                    return current
                if node.func.attr == "with_name" and node.args:
                    value = self._resolve_value(node.args[0])
                    if value is _PARAMETER:
                        return _PARAMETER
                    if value is _UNRESOLVED:
                        return _UNRESOLVED
                    return owner.with_name(str(value))
                if node.func.attr == "with_suffix" and node.args:
                    value = self._resolve_value(node.args[0])
                    if isinstance(value, str):
                        return owner.with_suffix(value)
                if node.func.attr == "with_stem" and node.args:
                    value = self._resolve_value(node.args[0])
                    if isinstance(value, str):
                        return owner.with_stem(value)
        return _UNRESOLVED

    def _is_temp_path_source(self, node: ast.AST) -> bool:
        if not isinstance(node, ast.Call):
            return False
        call_name = self._call_name(node.func)
        if call_name in self.temp_path_names:
            return True
        return any(
            call_name == f"{module}.{function}"
            for module in self.tempfile_modules
            for function in {"TemporaryDirectory", "NamedTemporaryFile", "mkdtemp", "mkstemp", "gettempdir"}
        )

    def _path_from_value(self, value: object) -> Path | None:
        if isinstance(value, Path):
            path = value
        elif isinstance(value, str):
            if value == "sys.executable":
                return None
            path = Path(value)
        else:
            return None
        if not path.is_absolute():
            if path.parts and path.parts[0] == "scripts":
                path = REPO_ROOT / path
            else:
                path = self.path.parent / path
        return path.resolve()

    def _lookup(self, name: str) -> object:
        for scope in reversed(self.scopes):
            if name in scope:
                return scope[name]
        return _UNRESOLVED

    def _bind_target(self, target: ast.AST, value: object) -> None:
        for name in self._target_names(target):
            if name in self.scopes[-1]:
                self.scopes[-1][name] = _UNRESOLVED
            else:
                self.scopes[-1][name] = value

    def _target_names(self, target: ast.AST) -> set[str]:
        if isinstance(target, ast.Name):
            return {target.id}
        if isinstance(target, (ast.Tuple, ast.List)):
            names: set[str] = set()
            for element in target.elts:
                names.update(self._target_names(element))
            return names
        return set()

    def _assigned_names(self, body: list[ast.stmt]) -> set[str]:
        names: set[str] = set()
        for statement in body:
            for child in ast.walk(statement):
                if isinstance(child, (ast.Assign, ast.AnnAssign)):
                    targets = child.targets if isinstance(child, ast.Assign) else [child.target]
                    for target in targets:
                        names.update(self._target_names(target))
        return names

    def _call_name(self, node: ast.AST) -> str:
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.Attribute):
            parent = self._call_name(node.value)
            return f"{parent}.{node.attr}" if parent else node.attr
        return ""

    def _is_subprocess_call(self, call_name: str) -> bool:
        return any(
            call_name == f"{module}.{method}"
            for module in self.subprocess_modules
            for method in _INVOCATION_FORMS["subprocess_calls"]
        )

    def _is_loader_call(self, call_name: str) -> bool:
        if call_name in self.spec_loader_names or call_name in self.source_loader_names:
            return True
        suffixes = (
            ".spec_from_file_location",
            ".SourceFileLoader",
            ".import_module",
            ".run_path",
            ".run_module",
        )
        return call_name.endswith(suffixes)

    def _is_dynamic_code_call(self, call_name: str) -> bool:
        if call_name in {"eval", "exec"}:
            return True
        if any(call_name == f"{module}.system" or call_name == f"{module}.popen" for module in self.os_modules):
            return True
        if any(
            call_name in {f"{module}.getoutput", f"{module}.getstatusoutput"}
            for module in self.subprocess_modules
        ):
            return True
        return any(call_name == f"{module}.spawn" for module in self.pty_modules)

    def _is_os_exec_spawn_call(self, call_name: str) -> bool:
        return any(
            call_name.startswith(f"{module}.exec") or call_name.startswith(f"{module}.spawn")
            for module in self.os_modules
        )

    def _fail(self, node: ast.AST, message: str) -> None:
        rel = self.path.relative_to(REPO_ROOT)
        lineno = getattr(node, "lineno", 0)
        self.failures.append(f"{rel}:{lineno}: {message}")


def _discover_cheap_lane_scripts() -> set[Path]:
    global _DISCOVERY_CACHE
    if _DISCOVERY_CACHE is not None:
        return set(_DISCOVERY_CACHE)
    labels = _cheap_lane_labels()
    labeled_scripts = _cheap_labeled_python_scripts(labels)
    dump = _just_dump()
    label_set = {label for label in labels if isinstance(label, str)}
    closure, _gates = _cheap_gate_closure(dump, label_set)
    scripts = set(labeled_scripts) | _closure_python_scripts(dump, closure)
    failures: list[str] = []
    scanned: set[Path] = set()
    queue = list(sorted(scripts))
    while queue:
        script = queue.pop(0).resolve()
        if script in scanned:
            continue
        scanned.add(script)
        try:
            source = script.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(script))
        except (OSError, SyntaxError, UnicodeDecodeError) as exc:
            failures.append(f"{script.relative_to(REPO_ROOT)}: cannot statically parse Python script: {exc}")
            continue
        resolver = _CodeExecutionEdgeResolver(script, tree, scan_set=scripts)
        targets, target_failures = resolver.resolve()
        failures.extend(target_failures)
        for target in targets:
            target = target.resolve()
            if target not in scripts:
                scripts.add(target)
                queue.append(target)
    if failures:
        raise AssertionError("cheap-lane code-execution discovery failed closed:\n  " + "\n  ".join(failures))
    _DISCOVERY_CACHE = set(scripts)
    return scripts


def _cheap_lane_discovered_unlabeled_manifest() -> set[str]:
    assert _MANIFEST_PATH.is_file(), f"missing cheap-lane manifest: {_MANIFEST_PATH.relative_to(REPO_ROOT)}"
    return {
        line.strip()
        for line in _MANIFEST_PATH.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    }


def _live_discovered_unlabeled(scripts: set[Path]) -> set[str]:
    labeled = _cheap_labeled_python_scripts()
    return {
        path.relative_to(REPO_ROOT).as_posix()
        for path in scripts
        if path not in labeled
    }


def _manifest_floor_missing(scripts: set[Path], manifest: set[str]) -> set[str]:
    return manifest - _live_discovered_unlabeled(scripts)


def _synthetic_just_dump(*, labels: list[str], recipes: dict[str, list[list[object]]]) -> dict:
    del labels
    return {
        "recipes": {
            name: {
                "name": name,
                "private": name.startswith("_"),
                "body": body,
                "dependencies": [],
            }
            for name, body in recipes.items()
        },
        "assignments": {
            "repo_root": {"value": ["call", "justfile_directory"]},
            "rust_verification_owner": {
                "value": ["concatenate", ["variable", "repo_root"], "/scripts/rust_verification.py"]
            },
        },
        "settings": {},
    }


def _repo_shared_state_write_findings(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    analyzer = _RepoSharedStateWriteAnalyzer(path)
    analyzer.visit(tree)
    return analyzer.findings


def _repo_shared_state_write_findings_from_source(source: str) -> list[str]:
    path = SCRIPTS_DIR / "synthetic_guard_fixture.py"
    tree = ast.parse(source, filename=str(path))
    analyzer = _RepoSharedStateWriteAnalyzer(path)
    analyzer.visit(tree)
    return analyzer.findings


def test_cheap_lanes_do_not_write_repo_root_shared_state() -> None:
    scripts = _cheap_lane_python_scripts()
    required = {
        SCRIPTS_DIR / "test_verify_bolt_v3_runtime_literals.py",
        SCRIPTS_DIR / "test_verify_bolt_v3_strategy_policy_fence.py",
    }
    missing_required = sorted(str(path.relative_to(REPO_ROOT)) for path in required - scripts)
    assert not missing_required, f"guard did not scan required scripts: {missing_required}"

    findings = [
        finding
        for script in sorted(scripts)
        for finding in _repo_shared_state_write_findings(script)
    ]
    assert not findings, (
        "cheap lane scripts must not write or delete REPO_ROOT-derived shared "
        "state; use process-private tempfile.TemporaryDirectory() instead:\n  "
        + "\n  ".join(findings)
    )


def test_cheap_lane_discovery_manifest_floor_and_required_edges() -> None:
    scripts = _discover_cheap_lane_scripts()
    rels = {path.relative_to(REPO_ROOT).as_posix() for path in scripts}
    manifest = _cheap_lane_discovered_unlabeled_manifest()
    missing_manifest = _manifest_floor_missing(scripts, manifest)
    assert not missing_manifest, f"live discovery dropped committed manifest entries: {sorted(missing_manifest)}"
    required = {
        "scripts/local_verification_gate.py",
        "scripts/rust_verification.py",
        "scripts/test_nextest_fingerprint.py",
        "scripts/nextest_fingerprint.py",
        "scripts/cargo-shim",
        "scripts/clean_merged_artifacts.py",
        "scripts/lane_governor.py",
        "scripts/command_understanding.py",
        "scripts/cancel_obsolete_dispatch_runs.py",
        "scripts/ci_provenance.py",
        "scripts/ubicloud_runner_minutes.py",
        "scripts/developer_tool_storage_hygiene.py",
        "scripts/find_same_sha_main_evidence.py",
        "scripts/require_sp_reviewer.py",
        "scripts/require_resolved_review_threads.py",
    }
    assert required <= rels, f"guard did not discover required fixed-point edges: {sorted(required - rels)}"


def test_manifest_floor_does_not_accept_labeled_seed_only() -> None:
    labeled_only = {SCRIPTS_DIR / "test_lane_governor.py"}
    manifest = {"scripts/test_lane_governor.py"}
    assert _manifest_floor_missing(labeled_only, manifest) == manifest


def test_direct_cheap_labels_resolve_python_by_semantics() -> None:
    scripts = _cheap_labeled_python_scripts(["cargo-shim"])
    assert SCRIPTS_DIR / "cargo-shim" in scripts
    try:
        _cheap_labeled_python_scripts(["non-script-runtime-label"])
    except AssertionError as exc:
        assert "must exist" in str(exc)
    else:
        raise AssertionError("missing direct cheap lane label must fail closed")


def test_repo_origin_detection_is_expression_based() -> None:
    snippets = [
        "from pathlib import Path\nROOT = Path(__file__).resolve().parents[1]\n(ROOT / 'x').write_text('x')\n",
        "from pathlib import Path\nROOT = Path(__file__).resolve().parent\np = ROOT / 'x'\np.write_text('x')\n",
        "from pathlib import Path\nROOT = Path(__file__).resolve().parents[1]\n(ROOT / 'a' / 'b').touch()\n",
        "from pathlib import Path\nROOT = Path(__file__).resolve().parents[1]\nROOT.joinpath('x').mkdir()\n",
        "from pathlib import Path\nSCRIPT = Path(__file__).resolve()\nSCRIPT.parent.joinpath('x').write_text('x')\n",
        "def repo_path(raw):\n    return raw\np = repo_path('x')\np.mkdir()\n",
    ]
    for source in snippets:
        findings = _repo_shared_state_write_findings_from_source(source)
        assert findings, f"expected repo-root write finding for:\n{source}"

    temp_fixture = (
        "from pathlib import Path\n"
        "import tempfile\n"
        "repo = Path(tempfile.mkdtemp())\n"
        "(repo / 'x').write_text('x')\n"
    )
    assert not _repo_shared_state_write_findings_from_source(temp_fixture)


def test_repo_write_analyzer_catches_extended_mutators() -> None:
    snippets = [
        "import os\nos.replace(REPO_ROOT / 'tmp', REPO_ROOT / 'dst')\n",
        "from pathlib import Path\nROOT = Path(__file__).resolve().parents[1]\n(ROOT / 'tmp').replace(ROOT / 'dst')\n",
        "import os\nos.link('/tmp/src', REPO_ROOT / 'dst')\n",
        "import os\nos.symlink('/tmp/src', REPO_ROOT / 'dst')\n",
        "import os\nos.mkdir(REPO_ROOT / 'dst')\n",
        "import os\nos.open(REPO_ROOT / 'dst', os.O_CREAT | os.O_RDWR)\n",
        "open(str(REPO_ROOT / 'dst'), 'w')\n",
        "import os\nos.remove(os.fspath(REPO_ROOT / 'dst'))\n",
        "open(REPO_ROOT / 'dst', 'r+')\n",
    ]
    for source in snippets:
        findings = _repo_shared_state_write_findings_from_source(source)
        assert findings, f"expected repo-root write finding for:\n{source}"


def test_code_execution_edges_are_static_fixed_point() -> None:
    source = """
from pathlib import Path
import importlib.util
import subprocess
import sys

SCRIPTS = Path(__file__).resolve().parent
SCRIPT = SCRIPTS / "nextest_fingerprint.py"
DEFAULT_SCRIPT = SCRIPTS / "find_same_sha_main_evidence.py"

def load(name: str):
    path = SCRIPTS / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    return spec

def load_default(path=DEFAULT_SCRIPT, module_name="find_same_sha_main_evidence"):
    spec = importlib.util.spec_from_file_location(module_name, path)
    return spec

def run(script):
    return subprocess.run([sys.executable, script])

load("lane_governor")
load_default()
importlib.util.spec_from_file_location("nextest_fingerprint_location", location=str(SCRIPT))
subprocess.run(["python3", str(SCRIPT)])
subprocess.run([str(SCRIPT)], executable="python3")
subprocess.run("python3 scripts/test_nextest_fingerprint.py", shell=True)
run(str(SCRIPTS / "clean_merged_artifacts.py"))
"""
    resolver = _CodeExecutionEdgeResolver(
        SCRIPTS_DIR / "synthetic_guard_fixture.py",
        ast.parse(source),
        scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
    )
    targets, failures = resolver.resolve()
    assert not failures
    rels = {target.relative_to(SCRIPTS_DIR).as_posix() for target in targets}
    assert {
        "lane_governor.py",
        "find_same_sha_main_evidence.py",
        "nextest_fingerprint.py",
        "clean_merged_artifacts.py",
    } <= rels


def test_local_wrappers_resolve_forward_and_nested_calls() -> None:
    source = """
import subprocess

forward("scripts/test_nextest_fingerprint.py")

def forward(script):
    return subprocess.run(["python3", script])

def inner(script):
    return subprocess.run(["python3", script])

def outer(script):
    return inner(script)

outer("scripts/clean_merged_artifacts.py")
"""
    resolver = _CodeExecutionEdgeResolver(
        SCRIPTS_DIR / "synthetic_guard_fixture.py",
        ast.parse(source),
        scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
    )
    targets, failures = resolver.resolve()
    assert not failures
    rels = {target.relative_to(SCRIPTS_DIR).as_posix() for target in targets}
    assert {"test_nextest_fingerprint.py", "clean_merged_artifacts.py"} <= rels


def test_shell_true_python_wrappers_resolve() -> None:
    fixtures = [
        "import subprocess\nsubprocess.run('env PYTHONPATH=/tmp python3 scripts/test_nextest_fingerprint.py', shell=True)\n",
        "import subprocess\nsubprocess.run('PYTHONPATH=/tmp python3 scripts/clean_merged_artifacts.py', shell=True)\n",
    ]
    expected = {"test_nextest_fingerprint.py", "clean_merged_artifacts.py"}
    rels: set[str] = set()
    for source in fixtures:
        resolver = _CodeExecutionEdgeResolver(
            SCRIPTS_DIR / "synthetic_guard_fixture.py",
            ast.parse(source),
            scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
        )
        targets, failures = resolver.resolve()
        assert not failures
        rels.update(target.relative_to(SCRIPTS_DIR).as_posix() for target in targets)
    assert expected <= rels


def test_code_execution_tripwires_fail_closed() -> None:
    fixtures = [
        "import subprocess\nsubprocess.run(['python3', script])\n",
        "import subprocess\nscript = 'a.py'\nscript = 'b.py'\nsubprocess.run(['python3', script])\n",
        "import subprocess\nargs = ['scripts/test_nextest_fingerprint.py']\nsubprocess.run(['python3'] + args)\n",
        "import subprocess\nsubprocess.run(['scripts/test_nextest_fingerprint.py'], executable=PYTHON)\n",
        "import subprocess\nsubprocess.run('python3 scripts/missing_guard_fixture.py', shell=True)\n",
        "import os\nos.system('python3 scripts/x.py')\n",
        "import os\nos.execv('python3', ['python3', 'scripts/test_nextest_fingerprint.py'])\n",
        "import os\nos.spawnv(os.P_NOWAIT, 'python3', ['python3', 'scripts/test_nextest_fingerprint.py'])\n",
        "import subprocess\nsubprocess.getoutput('echo x')\n",
        "import subprocess\nsubprocess.getstatusoutput('echo x')\n",
        "import pty\npty.spawn('/bin/echo')\n",
        "import subprocess\nsubprocess.run(['python3', '-c', 'from pathlib import Path; Path(\"inline-write\").write_text(\"x\")'])\n",
        "eval('1 + 1')\n",
        "import importlib.util\nimportlib.util.spec_from_file_location('x', target)\n",
        (
            "import importlib.util\n"
            "def load(path=TARGET):\n"
            "    return importlib.util.spec_from_file_location('x', path)\n"
            "load()\n"
        ),
    ]
    for source in fixtures:
        resolver = _CodeExecutionEdgeResolver(
            SCRIPTS_DIR / "synthetic_guard_fixture.py",
            ast.parse(source),
            scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
        )
        _targets, failures = resolver.resolve()
        assert failures, f"expected fail-closed execution edge for:\n{source}"


def test_unresolved_wrapper_call_fails_without_falling_back_to_default() -> None:
    fixtures = [
        """
from pathlib import Path
import importlib.util

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

load(target)
""",
        """
from pathlib import Path
import importlib.util

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"

if condition:
    target = SCRIPT

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

load(target)
""",
        """
from pathlib import Path
import importlib.util

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"
target = SCRIPT
target = SCRIPT

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

load(target)
""",
    ]
    for source in fixtures:
        resolver = _CodeExecutionEdgeResolver(
            SCRIPTS_DIR / "synthetic_guard_fixture.py",
            ast.parse(source),
            scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
        )
        targets, failures = resolver.resolve()
        assert failures, f"expected unresolved wrapper target to fail:\n{source}"
        assert SCRIPTS_DIR / "nextest_fingerprint.py" not in targets


def test_temp_bound_wrapper_call_is_opaque_without_falling_back_to_default() -> None:
    fixtures = [
        """
from pathlib import Path
from tempfile import TemporaryDirectory
import importlib.util

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

with TemporaryDirectory() as tmp:
    target = Path(tmp) / "scripts" / "verify_ci_workflow_hygiene.py"
    load(target)
""",
        """
from pathlib import Path
import importlib.util
import tempfile

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

target = Path(tempfile.mkdtemp()) / "scripts" / "verify_ci_workflow_hygiene.py"
load(target)
""",
    ]
    for source in fixtures:
        resolver = _CodeExecutionEdgeResolver(
            SCRIPTS_DIR / "synthetic_guard_fixture.py",
            ast.parse(source),
            scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
        )
        targets, failures = resolver.resolve()
        assert not failures, f"expected temp-derived wrapper target to remain opaque:\n{source}"
        assert SCRIPTS_DIR / "nextest_fingerprint.py" not in targets


def test_omitted_wrapper_arg_with_resolvable_default_is_l1() -> None:
    source = """
from pathlib import Path
import importlib.util

SCRIPT = Path(__file__).resolve().parent / "nextest_fingerprint.py"

def load(path=SCRIPT):
    return importlib.util.spec_from_file_location("x", path)

load()
"""
    resolver = _CodeExecutionEdgeResolver(
        SCRIPTS_DIR / "synthetic_guard_fixture.py",
        ast.parse(source),
        scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
    )
    targets, failures = resolver.resolve()
    assert not failures
    assert SCRIPTS_DIR / "nextest_fingerprint.py" in targets


def test_unresolved_direct_and_wrapper_targets_have_l2_parity() -> None:
    fixtures = [
        "import importlib.util\nimportlib.util.spec_from_file_location('x', target)\n",
        (
            "import importlib.util\n"
            "def load(path):\n"
            "    return importlib.util.spec_from_file_location('x', path)\n"
            "load(target)\n"
        ),
    ]
    for source in fixtures:
        resolver = _CodeExecutionEdgeResolver(
            SCRIPTS_DIR / "synthetic_guard_fixture.py",
            ast.parse(source),
            scan_set={SCRIPTS_DIR / "synthetic_guard_fixture.py"},
        )
        targets, failures = resolver.resolve()
        assert failures, f"expected unresolved Python target to fail:\n{source}"
        assert not targets


def test_just_dump_gate_derivation_and_fail_closed_fixtures() -> None:
    dump = _synthetic_just_dump(
        labels=["local-gate:alpha"],
        recipes={
            "alpha": [["python3 scripts/local_verification_gate.py alpha -- just alpha-inner"]],
            "alpha-inner": [["if ! python3 scripts/test_nextest_fingerprint.py; then"]],
        },
    )
    recipes, gates = _cheap_gate_closure(dump, {"local-gate:alpha"})
    assert {"alpha", "alpha-inner"} <= recipes
    assert gates == {"alpha": "alpha"}
    scripts = _closure_python_scripts(dump, recipes)
    assert SCRIPTS_DIR / "local_verification_gate.py" in scripts
    assert SCRIPTS_DIR / "test_nextest_fingerprint.py" in scripts

    bad_dump = _synthetic_just_dump(
        labels=["local-gate:alpha"],
        recipes={"alpha": [["python3 scripts/local_verification_gate.py alpha -- just alpha-inner"]]},
    )
    try:
        _cheap_gate_closure(bad_dump, {"local-gate:alpha"})
    except AssertionError as exc:
        assert "inner" in str(exc)
    else:
        raise AssertionError("missing inner recipe must fail closed")

    call_fragment_dump = _synthetic_just_dump(
        labels=["local-gate:alpha"],
        recipes={
            "alpha": [["python3 scripts/local_verification_gate.py alpha -- just alpha-inner"]],
            "alpha-inner": [
                [
                    "python3 ",
                    [["call", "justfile_directory"]],
                    "/scripts/test_nextest_fingerprint.py",
                ]
            ],
        },
    )
    recipes, _gates = _cheap_gate_closure(call_fragment_dump, {"local-gate:alpha"})
    scripts = _closure_python_scripts(call_fragment_dump, recipes)
    assert SCRIPTS_DIR / "test_nextest_fingerprint.py" in scripts


def test_shell_expanded_python_commands_fail_closed() -> None:
    dump = _synthetic_just_dump(
        labels=["local-gate:alpha"],
        recipes={
            "alpha": [["python3 scripts/local_verification_gate.py alpha -- just alpha-inner"]],
            "alpha-inner": [['tool="$(${PYTHON} scripts/test_nextest_fingerprint.py)"']],
        },
    )
    recipes, _gates = _cheap_gate_closure(dump, {"local-gate:alpha"})
    try:
        _closure_python_scripts(dump, recipes)
    except AssertionError as exc:
        assert "dynamic" in str(exc) or "unsupported" in str(exc)
    else:
        raise AssertionError("shell-expanded Python command must fail closed")


def test_shell_wrappers_and_pipelines_discover_python_commands() -> None:
    dump = _synthetic_just_dump(
        labels=["local-gate:alpha"],
        recipes={
            "alpha": [["python3 scripts/local_verification_gate.py alpha -- just alpha-inner"]],
            "alpha-inner": [
                ["env PYTHONPATH=/tmp python3 scripts/test_nextest_fingerprint.py"],
                ["python3 scripts/test_nextest_fingerprint.py | grep ok || true"],
                ["VALUE=\"$(echo $(python3 scripts/test_nextest_fingerprint.py))\""],
                [
                    "VALUE=\"$(printf '%s\\n' \"$policy_json\" | python3 -c 'import json, sys; print(json.load(sys.stdin)[\"x\"])')\""
                ],
            ],
        },
    )
    recipes, _gates = _cheap_gate_closure(dump, {"local-gate:alpha"})
    scripts = _closure_python_scripts(dump, recipes)
    assert SCRIPTS_DIR / "test_nextest_fingerprint.py" in scripts


def test_subcrate_lane_policy_matches_repo_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    subcrate = RV.load_policy(REPO_ROOT / "crates/backtesting-vertical-slice")
    assert subcrate["local_lane_policy"] == data["local_lane_policy"]


# Subprocess runner: acquire, write a sentinel, hold for --hold seconds, exit.
HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], float(sys.argv[4])
handle = lane_governor.acquire(
    "hold-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

# Subprocess runner: acquire once, print acquisition wall time, exit immediately.
ONCE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, timeout = sys.argv[2], float(sys.argv[3])
handle = lane_governor.acquire(
    "once-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=timeout, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("acquired", time.time())
"""

FAIL_FAST_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir = sys.argv[2]
t0 = time.monotonic()
lane_governor.acquire(
    "fail-fast-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
    fail_fast=True,
)
print("unexpected-acquired", time.monotonic() - t0)
"""

LOCAL_GATE_HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], float(sys.argv[4])
handle = lane_governor.acquire(
    "local-gate:external", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

LABEL_HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
label, lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], sys.argv[4], float(sys.argv[5])
handle = lane_governor.acquire(
    label, lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

LABEL_ONCE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
label, lock_dir, timeout = sys.argv[2], sys.argv[3], float(sys.argv[4])
t0 = time.monotonic()
handle = lane_governor.acquire(
    label, lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=timeout, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("acquired", handle is not None, time.monotonic() - t0)
"""

LOCAL_GATE_RUNNER = """
import sys
sys.path.insert(0, sys.argv[1])
import local_verification_gate
gate, lock_dir = sys.argv[2], sys.argv[3]
rc = local_verification_gate.run_gate(
    gate,
    [sys.executable, "-c", "print('gate-ran')"],
    lock_dir=lock_dir,
    honor_ci_env=False,
)
print("gate-rc", rc)
raise SystemExit(rc)
"""

NAMESPACE_ONCE_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
label, repo_root, lock_dir, timeout = sys.argv[2], sys.argv[3], sys.argv[4], float(sys.argv[5])
lane_governor.REPO_ROOT = Path(repo_root)
t0 = time.monotonic()
handle = lane_governor.acquire(
    label, lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=timeout, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("acquired", handle is not None, time.monotonic() - t0)
"""

FORGED_GATE_ENV_RUNNER = """
import os, sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir = sys.argv[2]
os.environ[lane_governor.LOCAL_VERIFICATION_GATE_ENV] = "1"
t0 = time.monotonic()
lane_governor.acquire(
    "forged-gate-env-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=1, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("unexpected-acquired", time.monotonic() - t0)
"""

# Parent: acquire, then spawn a child runner WITH A SCRUBBED ENV that attempts
# acquire on the same lock dir. The child must pass through (ancestor holds).
PARENT_CHILD_RUNNER = """
import os, subprocess, sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
handle = lane_governor.acquire(
    "parent-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
child_code = (
    "import sys, time; sys.path.insert(0, sys.argv[1]); import lane_governor; "
    "t0 = time.monotonic(); "
    "lane_governor.acquire('child-runner', lock_dir=sys.argv[2], honor_ci_env=False, "
    "acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1); "
    "print('child-done', time.monotonic() - t0)"
)
scrubbed = {"PATH": "/usr/bin:/bin"}
completed = subprocess.run(
    [sys.executable, "-c", child_code, scripts_dir, lock_dir],
    env=scrubbed, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("child-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""

GRANDCHILD_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
lane_governor.acquire(
    "grandchild-runner", lock_dir=sys.argv[2], honor_ci_env=False,
    acquire_timeout_seconds=2, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("grandchild-done", time.monotonic() - t0)
"""

INTERMEDIATE_CHILD_RUNNER = """
import subprocess, sys
scripts_dir, lock_dir, grandchild_code = sys.argv[1], sys.argv[2], sys.argv[3]
completed = subprocess.run(
    [sys.executable, "-c", grandchild_code, scripts_dir, lock_dir],
    env={"PATH": "/usr/bin:/bin"}, text=True,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("grandchild-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""

GRANDPARENT_CHILD_RUNNER = """
import subprocess, sys
sys.path.insert(0, sys.argv[1])
import lane_governor
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
intermediate_code, grandchild_code = sys.argv[3], sys.argv[4]
handle = lane_governor.acquire(
    "grandparent-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
completed = subprocess.run(
    [sys.executable, "-c", intermediate_code, scripts_dir, lock_dir, grandchild_code],
    env={"PATH": "/usr/bin:/bin"}, text=True,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("middle-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""

CI_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("ci-result", result is None, time.monotonic() - t0)
"""

CI_FAIL_FAST_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-fail-fast-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
    fail_fast=True,
)
print("ci-fail-fast-result", result is None, time.monotonic() - t0)
"""

CI_FALSE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-false-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=1, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("ci-false-result", result is None, time.monotonic() - t0)
"""

HELP_RUNNER = """
import sys, time
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
sys.path.insert(0, scripts_dir)
import lane_governor
sys.argv = ["verify_sample.py", "--help"]
t0 = time.monotonic()
result = lane_governor.acquire(
    "help-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("help-result", result is None, time.monotonic() - t0)
"""

HELP_BROKEN_REPO_RUNNER = """
import sys, time
from pathlib import Path
scripts_dir, repo_root = sys.argv[1], sys.argv[2]
sys.path.insert(0, scripts_dir)
import lane_governor
lane_governor.REPO_ROOT = Path(repo_root)
sys.argv = ["verify_sample.py", "--help"]
t0 = time.monotonic()
result = lane_governor.acquire("help-runner", honor_ci_env=False)
print("help-result", result is None, time.monotonic() - t0)
"""


def _spawn(snippet: str, *args: str, env: dict | None = None) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, "-c", snippet, str(SCRIPTS_DIR), *args],
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
    )


def _wait_for(path: Path, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.05)
    raise AssertionError(f"sentinel {path} never appeared")


def test_uncontended_acquire_is_fast() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        start = time.monotonic()
        proc = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = proc.communicate(timeout=20)
        assert proc.returncode == 0, err
        assert "acquired" in out
        assert time.monotonic() - start < 10, "uncontended acquire must not wait"


def test_second_acquire_queues_until_release() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        t0 = time.monotonic()
        waiter = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = waiter.communicate(timeout=30)
        waited = time.monotonic() - t0
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, err
        assert "acquired" in out
        assert waited >= 2.0, f"waiter should queue behind holder, waited only {waited:.2f}s"


def test_distinct_cheap_lanes_acquire_concurrently() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(
            LABEL_HOLD_RUNNER,
            "local-gate:source-fence-static",
            tmp,
            str(sentinel),
            "4",
        )
        _wait_for(sentinel)
        start = time.monotonic()
        waiter = _spawn(LABEL_ONCE_RUNNER, "local-gate:fmt-check", tmp, "1")
        out, err = waiter.communicate(timeout=10)
        elapsed = time.monotonic() - start
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, f"distinct cheap lanes must not contend: {out}\n{err}"
        assert "acquired True" in out
        assert elapsed < 2.0, f"cheap waiter should not queue behind another cheap lane: {elapsed:.2f}s"


def test_same_label_unbounded_cheap_lanes_acquire_concurrently() -> None:
    # Exact scenario raised in PR #900 review: two processes with the SAME cheap
    # label share one lock file. They must run concurrently, and the shared file
    # must carry NO racy holder metadata (unbounded cheap lanes do not
    # _record_holder; concurrent seek/truncate would corrupt it for no reader).
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(LABEL_HOLD_RUNNER, "local-gate:fmt-check", tmp, str(sentinel), "4")
        _wait_for(sentinel)
        start = time.monotonic()
        waiter = _spawn(LABEL_ONCE_RUNNER, "local-gate:fmt-check", tmp, "1")
        out, err = waiter.communicate(timeout=10)
        elapsed = time.monotonic() - start
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, f"same-label cheap lanes must not contend: {out}\n{err}"
        assert "acquired True" in out
        assert elapsed < 2.0, f"same-label cheap waiter should not queue: {elapsed:.2f}s"
        cheap_locks = list(Path(tmp).glob("*.cheap.*.lock"))
        assert cheap_locks, "expected a shared cheap lock file to exist"
        for lock_file in cheap_locks:
            size = lock_file.stat().st_size
            assert size == 0, (
                f"unbounded cheap lock must carry no racy holder metadata: "
                f"{lock_file.name} = {size} bytes"
            )


def test_front_door_cheap_gate_runs_while_cheap_lane_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(
            LABEL_HOLD_RUNNER,
            "local-gate:source-fence-static",
            tmp,
            str(sentinel),
            "4",
        )
        _wait_for(sentinel)
        gate = _spawn(LOCAL_GATE_RUNNER, "fmt-check", tmp)
        out, err = gate.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert gate.returncode == 0, f"front-door cheap gate must run under cheap contention: {out}\n{err}"
        assert "gate-ran" in out
        assert "gate-rc 0" in out


def test_unlisted_label_uses_heavy_single_flight() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(LABEL_HOLD_RUNNER, "unlisted-heavy-holder", tmp, str(sentinel), "10")
        _wait_for(sentinel)
        waiter = _spawn(LABEL_ONCE_RUNNER, "unlisted-heavy-waiter", tmp, "1")
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"unlisted labels must remain heavy single-flight: {out}"
        assert "FAILED to acquire" in err
        assert "unlisted-heavy-holder" in err


def test_bvs_namespace_lane_is_independent_of_bolt_v2_namespace() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(LABEL_HOLD_RUNNER, "namespace-heavy", tmp, str(sentinel), "4")
        _wait_for(sentinel)
        subcrate_root = REPO_ROOT / "crates/backtesting-vertical-slice"
        waiter = _spawn(NAMESPACE_ONCE_RUNNER, "namespace-heavy", str(subcrate_root), tmp, "1")
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, f"BVS namespace must not contend with bolt-v2 namespace: {out}\n{err}"
        assert "acquired True" in out


def test_holder_metadata_written() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        data = RV.load_policy(REPO_ROOT)
        lock_path = Path(tmp) / f"{data['target_namespace']}.lane.lock"
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
        holder.communicate(timeout=10)
        assert payload["pid"] == holder.pid
        assert payload["lane"] == "hold-runner"


def test_timeout_fails_loud_with_holder_info() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "15")
        _wait_for(sentinel)
        waiter = _spawn(ONCE_RUNNER, tmp, "2")
        out, err = waiter.communicate(timeout=30)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"expected exit 1, got {waiter.returncode}"
        assert "FAILED to acquire" in err
        assert "hold-runner" in err, "timeout message must name the holding lane"
        assert str(holder.pid) in err, "timeout message must name the holding pid"


def test_fail_fast_refuses_busy_lane_without_queueing() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        start = time.monotonic()
        waiter = _spawn(FAIL_FAST_RUNNER, tmp)
        out, err = waiter.communicate(timeout=10)
        elapsed = time.monotonic() - start
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"fail-fast waiter must refuse busy lane: {out}"
        assert elapsed < 2.0, f"fail-fast waiter queued for {elapsed:.2f}s"
        assert "already running" in err
        assert "hold-runner" in err
        assert str(holder.pid) in err


def test_release_closes_and_unregisters_held_handle() -> None:
    lane_governor = _load("lane_governor")
    with tempfile.TemporaryDirectory() as tmp:
        baseline = len(lane_governor._HELD_HANDLES)
        handle = lane_governor.acquire("release-runner", lock_dir=tmp, honor_ci_env=False)
        assert handle in lane_governor._HELD_HANDLES
        lane_governor.release(handle)
        assert handle.closed
        assert handle not in lane_governor._HELD_HANDLES

        reacquired = lane_governor.acquire("release-runner-2", lock_dir=tmp, honor_ci_env=False)
        lane_governor.release(reacquired)
        assert len(lane_governor._HELD_HANDLES) == baseline


def test_unrelated_holder_does_not_reenter() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        waiter = _spawn(ONCE_RUNNER, tmp, "1")
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"unrelated holder must not pass through: {out}"
        assert "FAILED to acquire" in err


def test_forged_gate_env_does_not_reenter_unrelated_local_gate_holder() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(LOCAL_GATE_HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        waiter = _spawn(FORGED_GATE_ENV_RUNNER, tmp)
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"unrelated local gate holder must not pass through: {out}"
        assert "FAILED to acquire" in err
        assert "local-gate:external" in err


def test_unexpected_flock_error_fails_immediately() -> None:
    lane_governor = _load("lane_governor")
    with tempfile.TemporaryDirectory() as tmp:
        original_flock = lane_governor.fcntl.flock

        def broken_flock(*_args) -> None:
            raise OSError(errno.EINVAL, "bad file descriptor")

        lane_governor.fcntl.flock = broken_flock
        try:
            try:
                lane_governor.acquire(
                    "broken-flock",
                    lock_dir=tmp,
                    honor_ci_env=False,
                    acquire_timeout_seconds=30,
                    heartbeat_seconds=1,
                    poll_interval_seconds=0.1,
                )
            except OSError as exc:
                assert exc.errno == errno.EINVAL
                return
            raise AssertionError("unexpected flock errors must not be treated as contention")
        finally:
            lane_governor.fcntl.flock = original_flock


def test_scrubbed_env_child_reenters_while_parent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(PARENT_CHILD_RUNNER, tmp)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "child-rc 0" in out, f"child must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("child-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"child must pass through re-entrantly, took {elapsed:.1f}s"


def test_scrubbed_env_grandchild_reenters_while_grandparent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(GRANDPARENT_CHILD_RUNNER, tmp, INTERMEDIATE_CHILD_RUNNER, GRANDCHILD_RUNNER)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "middle-rc 0" in out, f"intermediate child must succeed, got: {out}\n{err}"
        assert "grandchild-rc 0" in out, f"grandchild must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("grandchild-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"grandchild must pass through re-entrantly, took {elapsed:.1f}s"


def test_ci_env_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "true"
        ci = _spawn(CI_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "CI bypass must return None without locking"
        assert elapsed < 5.0, "CI bypass must not wait"


def test_ci_env_bypasses_fail_fast_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "true"
        ci = _spawn(CI_FAIL_FAST_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "CI bypass must return None before fail-fast lock refusal"
        assert elapsed < 5.0, "CI fail-fast bypass must not wait"


def test_ci_false_env_does_not_bypass_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "false"
        ci = _spawn(CI_FALSE_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 1, f"GITHUB_ACTIONS=false must not bypass the lane lock: {out}"
        assert "FAILED to acquire" in err


def test_help_invocation_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        helper = _spawn(HELP_RUNNER, tmp)
        out, err = helper.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert helper.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "--help must not take or wait for the lane lock"
        assert elapsed < 5.0, "--help fast-path must not wait"


def test_help_invocation_bypasses_policy_load() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        broken_repo = Path(tmp) / "missing-policy-repo"
        broken_repo.mkdir()
        helper = _spawn(HELP_BROKEN_REPO_RUNNER, str(broken_repo))
        out, err = helper.communicate(timeout=20)
        assert helper.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "--help must return None without loading policy"
        assert elapsed < 5.0, "--help fast-path must not wait"


def main() -> int:
    tests = [
        test_valid_lane_policy_passes,
        test_missing_lane_policy_rejected,
        test_disabled_lane_policy_rejected,
        test_relative_lock_dir_rejected,
        test_env_expansion_lock_dir_rejected,
        test_heartbeat_must_be_below_timeout,
        test_poll_interval_must_not_exceed_heartbeat,
        test_non_positive_intervals_rejected,
        test_cheap_lane_labels_must_be_a_string_list,
        test_cheap_lane_max_concurrent_must_be_a_non_negative_integer,
        test_unknown_lane_policy_keys_rejected,
        test_repo_policy_file_declares_lane_policy,
        test_cheap_lanes_do_not_write_repo_root_shared_state,
        test_cheap_lane_discovery_manifest_floor_and_required_edges,
        test_manifest_floor_does_not_accept_labeled_seed_only,
        test_direct_cheap_labels_resolve_python_by_semantics,
        test_repo_origin_detection_is_expression_based,
        test_repo_write_analyzer_catches_extended_mutators,
        test_code_execution_edges_are_static_fixed_point,
        test_local_wrappers_resolve_forward_and_nested_calls,
        test_shell_true_python_wrappers_resolve,
        test_code_execution_tripwires_fail_closed,
        test_unresolved_wrapper_call_fails_without_falling_back_to_default,
        test_temp_bound_wrapper_call_is_opaque_without_falling_back_to_default,
        test_omitted_wrapper_arg_with_resolvable_default_is_l1,
        test_unresolved_direct_and_wrapper_targets_have_l2_parity,
        test_just_dump_gate_derivation_and_fail_closed_fixtures,
        test_shell_expanded_python_commands_fail_closed,
        test_shell_wrappers_and_pipelines_discover_python_commands,
        test_subcrate_lane_policy_matches_repo_policy,
        test_uncontended_acquire_is_fast,
        test_second_acquire_queues_until_release,
        test_distinct_cheap_lanes_acquire_concurrently,
        test_same_label_unbounded_cheap_lanes_acquire_concurrently,
        test_front_door_cheap_gate_runs_while_cheap_lane_holds,
        test_unlisted_label_uses_heavy_single_flight,
        test_bvs_namespace_lane_is_independent_of_bolt_v2_namespace,
        test_holder_metadata_written,
        test_timeout_fails_loud_with_holder_info,
        test_fail_fast_refuses_busy_lane_without_queueing,
        test_release_closes_and_unregisters_held_handle,
        test_unrelated_holder_does_not_reenter,
        test_forged_gate_env_does_not_reenter_unrelated_local_gate_holder,
        test_unexpected_flock_error_fails_immediately,
        test_scrubbed_env_child_reenters_while_parent_holds,
        test_scrubbed_env_grandchild_reenters_while_grandparent_holds,
        test_ci_env_bypasses_lock,
        test_ci_env_bypasses_fail_fast_lock,
        test_ci_false_env_does_not_bypass_lock,
        test_help_invocation_bypasses_lock,
        test_help_invocation_bypasses_policy_load,
    ]
    for test in tests:
        test()
    print("OK: lane governor self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
