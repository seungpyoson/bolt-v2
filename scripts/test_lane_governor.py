#!/usr/bin/env python3
"""Self-tests for lane_governor and the local_lane_policy validator (#653)."""

from __future__ import annotations

import errno
import ast
import importlib.util
import json
import os
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
    {"write_text", "write_bytes", "mkdir", "rmdir", "rmtree", "unlink", "touch"}
)
_OS_MUTATORS = frozenset({"makedirs", "remove", "rename", "rmdir"})


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
    def __init__(self, path: Path) -> None:
        self.path = path
        self.findings: list[str] = []
        self.origins: dict[str, str] = {name: _REPO_ORIGIN for name in _REPO_ROOT_NAMES}
        self.os_modules = {"os"}
        self.shutil_modules = {"shutil"}
        self.tempfile_modules = {"tempfile"}
        self.tempdir_names = {"TemporaryDirectory"}

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            name = alias.asname or alias.name
            if alias.name == "os":
                self.os_modules.add(name)
            elif alias.name == "shutil":
                self.shutil_modules.add(name)
            elif alias.name == "tempfile":
                self.tempfile_modules.add(name)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module == "tempfile":
            for alias in node.names:
                if alias.name == "TemporaryDirectory":
                    self.tempdir_names.add(alias.asname or alias.name)

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
        if isinstance(func, ast.Name) and func.id == "Path" and node.args:
            return self._origin(node.args[0])
        if isinstance(func, ast.Attribute):
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
            elif method == "rename":
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
            if os_attr in _OS_MUTATORS - {"rename"} and node.args:
                targets.append((os_attr, node.args[0]))
            elif os_attr == "rename" and node.args:
                for arg in node.args[:2]:
                    targets.append((os_attr, arg))

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
            and any(flag in mode.value for flag in ("w", "a", "x"))
        )

    def _finding(self, node: ast.AST, operation: str, target: str) -> str:
        rel = self.path.relative_to(REPO_ROOT)
        return f"{rel}:{node.lineno}: {operation} targets shared repo state: {target}"


def _justfile_recipe_python_scripts(recipe_name: str) -> set[Path]:
    scripts: set[Path] = set()
    in_recipe = False
    for raw_line in (REPO_ROOT / "justfile").read_text(encoding="utf-8").splitlines():
        if not in_recipe:
            in_recipe = raw_line.startswith(f"{recipe_name}:")
            continue
        if raw_line and not raw_line[0].isspace():
            break
        stripped = raw_line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parts = stripped.split()
        if len(parts) >= 2 and parts[0] == "python3" and parts[1].startswith("scripts/"):
            script = REPO_ROOT / parts[1]
            if script.suffix == ".py":
                scripts.add(script)
    return scripts


def _cheap_lane_python_scripts() -> set[Path]:
    policy = RV.load_policy(REPO_ROOT)["local_lane_policy"]
    labels = policy.get("cheap_lane_labels", [])
    missing = sorted(
        label
        for label in labels
        if isinstance(label, str)
        and label.endswith(".py")
        and not (SCRIPTS_DIR / label).is_file()
    )
    assert not missing, f"cheap lane script labels must exist: {missing}"

    scripts = {
        SCRIPTS_DIR / label
        for label in labels
        if isinstance(label, str) and label.endswith(".py")
    }
    scripts.update(_justfile_recipe_python_scripts("source-fence-static-inner"))
    return scripts


def _repo_shared_state_write_findings(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
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
