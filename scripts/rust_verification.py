#!/usr/bin/env python3
"""Repo-local Rust verification owner for bolt-v2."""

from __future__ import annotations

import argparse
import contextlib
import copy
import fcntl
import functools
import json
import os
import pathlib
import re
import shlex
import shutil
import stat
import subprocess
import sys
import time
import uuid
from typing import Any

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv

# Keep the former verifier-local helper families module-scoped so parity tests
# prove the old helper surface now points at the shared path.
from command_understanding import (
    cargo_args_for_target_routing_scan,
    cargo_subcommand,
    cargo_subcommand_with_index,
    nextest_subcommand_with_index,
    python_call_command_argument,
    python_call_name,
    python_command_string,
    python_constant_string,
    python_inline_command_payloads,
)
from ci_test_manifest import build_test_manifest

try:
    import tomllib as _toml
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback.
    try:
        import tomli as _toml  # type: ignore[no-redef]
    except ModuleNotFoundError:  # pragma: no cover - exercised by system Python on macOS.
        _toml = None

_TOML_DECODE_ERROR = _toml.TOMLDecodeError if _toml is not None else ValueError


POLICY_RELATIVE_PATH = pathlib.Path("ci/rust-verification.toml")
CI_RUNNERS_RELATIVE_PATH = pathlib.Path("ci/github-actions-runners.toml")
MAX_POLICY_BYTES = 1024 * 1024
SAFE_IDENTIFIER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
JUST_RECIPE_RE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]*$")
LANE_LABEL_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]*$")
ENV_NAME_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")
FULL_SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
RUST_PROBE_MODES = (
    "check-lib",
    "check-test-target",
    "nextest-no-run-test-target",
    "nextest-test-target",
    "nextest-test-target-name",
)
RUST_PROBE_SUGGEST_COMMAND = "suggest"
RUST_PROBE_COMMANDS = (RUST_PROBE_SUGGEST_COMMAND, *RUST_PROBE_MODES)
GATE_NAME_KEYS = (
    "gate_required",
    "gate_iteration",
    "gate_dispatch_full",
    "backtester_required",
    "backtester_iteration",
    "backtester_dispatch_full",
)
RUST_PROBE_HELP_EPILOG = """\
Examples:
  just rust-probe suggest
  just rust-probe check-lib
  just rust-probe check-test-target <harness_target>
  just rust-probe nextest-no-run-test-target <harness_target>
  just rust-probe nextest-test-target <harness_target>
  just rust-probe nextest-test-target-name <harness_target> <member_stem>::

Rust Probe is targeted remote debugging feedback only. It is not merge proof.
Use just verify-remote for full remote feedback; mark the PR ready before treating it as merge proof.
"""
RUST_PROBE_INPUT_KEYS = (
    "runner_tier",
    "job_timeout_minutes",
    "ref",
    "expected_sha",
    "probe_id",
    "mode",
    "test_target",
    "test_name",
)
SCRUB_ENV_KEYS = (
    "BOLT_ALLOW_LOCAL_RUST",
    "BOLT_MANAGED_JUST",
    "BOLT_RUST_VERIFICATION_ROOT",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "CARGO_INCREMENTAL",
    "CARGO_INSTALL_ROOT",
    "CARGO_TARGET_DIR",
    "CARGO_TARGET_TMPDIR",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUST_VERIFICATION_PRESERVE_ROUTING_ENV",
    "RUST_VERIFICATION_REAL_CARGO",
    "RUST_VERIFICATION_ROOT_BASE",
)
OPAQUE_RUST_LAUNCHERS = {
    "bash",
    "catchsegv",
    "chrt",
    "chroot",
    "command",
    "dash",
    "docker",
    "env",
    "exec",
    "fish",
    "flock",
    "find",
    "ionice",
    "make",
    "nohup",
    "npm",
    "python",
    "python2",
    "python3",
    "podman",
    "rustup",
    "setsid",
    "sh",
    "sg",
    "stdbuf",
    "su",
    "taskset",
    "time",
    "timeout",
    "runuser",
    "xargs",
    "zsh",
}
PROCESS_PARSE_DEPTH_LIMIT = 6
PROCESS_PARSE_DEPTH_EXCEEDED = "__process_parse_depth_exceeded__"
SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "if", "elif", "then", "else", "while", "until", "do", "!", "(", "{", ")"}
SHELL_BOUNDARY_TOKEN_RE = re.compile(r"([;&|(){}!<>]+)")
SHELL_REDIRECTION_OPERATORS = {">", ">>", "<", "<<", "<>", ">|", ">&", "<&", "&>", "&>>", "<<<"}


class PolicyError(RuntimeError):
    pass


require_positive_int = functools.partial(_cv.require_positive_int, error_cls=PolicyError)


class ProcessVisibilityError(RuntimeError):
    pass


def check_policy_size(path: pathlib.Path) -> None:
    size = path.stat().st_size
    if size > MAX_POLICY_BYTES:
        raise PolicyError(f"{POLICY_RELATIVE_PATH} exceeds maximum size of {MAX_POLICY_BYTES} bytes")


def parse_minimal_toml(path: pathlib.Path) -> dict[str, Any]:
    check_policy_size(path)
    data: dict[str, Any] = {}
    current: dict[str, Any] = data
    with path.open("r", encoding="utf-8") as handle:
        lines = enumerate(handle, start=1)
        for lineno, raw_line in lines:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                current = data
                for part in line[1:-1].split("."):
                    if not part or not SAFE_IDENTIFIER_RE.match(part):
                        raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported table name")
                    child = current.setdefault(part, {})
                    if not isinstance(child, dict):
                        raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: table conflicts with scalar")
                    current = child
                continue
            key, sep, value_text = line.partition("=")
            if not sep:
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: expected key = value")
            key = key.strip()
            if key.startswith('"') and key.endswith('"'):
                try:
                    parsed_key = json.loads(key)
                except json.JSONDecodeError as exc:
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: invalid key") from exc
                if not isinstance(parsed_key, str) or not parsed_key:
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: invalid key")
                key = parsed_key
            elif not SAFE_IDENTIFIER_RE.match(key):
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported key")
            value_text = value_text.strip()
            if value_text.startswith('"') and value_text.endswith('"'):
                try:
                    value: Any = json.loads(value_text)
                except json.JSONDecodeError as exc:
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: invalid string") from exc
            elif value_text.startswith("[") and value_text.endswith("]"):
                try:
                    value = json.loads(value_text)
                except json.JSONDecodeError as exc:
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: invalid array") from exc
                if not all(isinstance(item, str) for item in value):
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported array")
            elif value_text in ("true", "false"):
                value = value_text == "true"
            elif value_text.isdigit():
                value = int(value_text)
            else:
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported value")
            current[key] = value
    return data


def load_toml(path: pathlib.Path) -> dict[str, Any]:
    if _toml is None:
        return parse_minimal_toml(path)
    check_policy_size(path)
    try:
        with path.open("rb") as handle:
            return _toml.load(handle)
    except _toml.TOMLDecodeError as exc:
        raise PolicyError(f"{POLICY_RELATIVE_PATH} is invalid TOML: {exc}") from exc


def repo_path(raw: str) -> pathlib.Path:
    return pathlib.Path(raw).expanduser().absolute()


def policy_path(repo: pathlib.Path) -> pathlib.Path:
    return repo / POLICY_RELATIVE_PATH


def load_policy(repo: pathlib.Path) -> dict[str, Any]:
    path = policy_path(repo)
    if not path.exists():
        raise FileNotFoundError(path)
    data = load_toml(path)
    validate_policy_data(data)
    return data


def validate_policy_data(data: dict[str, Any]) -> None:
    if data.get("schema_version") != 2:
        raise PolicyError("schema_version must be 2")
    project_id = data.get("project_id")
    namespace = data.get("target_namespace")
    if not isinstance(project_id, str) or not SAFE_IDENTIFIER_RE.match(project_id):
        raise PolicyError("project_id must be a safe identifier")
    if not isinstance(namespace, str) or not SAFE_IDENTIFIER_RE.match(namespace):
        raise PolicyError("target_namespace must be a safe identifier")
    commands = data.get("commands")
    if not isinstance(commands, dict):
        raise PolicyError("commands table is required")
    for name in ("test", "clippy", "build"):
        command = commands.get(name)
        if not isinstance(command, dict):
            raise PolicyError(f"commands.{name} table is required")
        recipe = command.get("recipe")
        if not isinstance(recipe, str) or not SAFE_IDENTIFIER_RE.match(recipe):
            raise PolicyError(f"commands.{name}.recipe must be a safe identifier")
    build = commands["build"]
    for key in ("target", "profile"):
        value = build.get(key)
        if not isinstance(value, str) or not SAFE_IDENTIFIER_RE.match(value):
            raise PolicyError(f"commands.build.{key} must be a safe identifier")
    if build.get("artifact_layout") != "cargo":
        raise PolicyError("commands.build.artifact_layout must be 'cargo'")
    validate_local_compile_policy(data)
    validate_remote_compile_cache_policy(data)
    validate_local_lane_policy(data)
    if "remote_verification" in data:
        validate_remote_verification_policy(data)
    if "remote_probe" in data:
        validate_remote_probe_policy(data)
    if "cache" in data:
        validate_cache_policy(data)


def string_array_policy_value(table: dict[str, Any], key: str) -> list[str]:
    value = table.get(key)
    if not isinstance(value, list) or not value or not all(isinstance(item, str) for item in value):
        raise PolicyError(f"{key} must be a non-empty string array")
    for item in value:
        if not SAFE_IDENTIFIER_RE.match(item):
            raise PolicyError(f"{key} entries must be safe identifiers")
    return value


def validate_cheap_lane_label(label: object, key: str) -> str:
    if not isinstance(label, str) or not LANE_LABEL_RE.match(label):
        raise PolicyError(f"local_lane_policy.{key} entries must be safe lane labels")
    return label


def validate_cheap_lane_just_recipe(recipe: object) -> str:
    if not isinstance(recipe, str) or not JUST_RECIPE_RE.match(recipe):
        raise PolicyError("local_lane_policy.cheap_lane_just_recipes entries must be safe just recipe names")
    return recipe


def just_recipe_name(line: str) -> str | None:
    if not line or line[0].isspace():
        return None
    stripped = line.strip()
    if not stripped or stripped.startswith(("#", "[")) or ":=" in stripped:
        return None
    match = re.match(r"^([A-Za-z0-9_][A-Za-z0-9_-]*)(?:\s+[^:]*)?:", line)
    return match.group(1) if match else None


def just_recipe_body(justfile_text: str, recipe: str) -> list[str]:
    body: list[str] = []
    in_recipe = False
    found = False
    for line in justfile_text.splitlines():
        name = just_recipe_name(line)
        if name is not None:
            if in_recipe:
                break
            in_recipe = name == recipe
            found = found or in_recipe
            continue
        if in_recipe and (line.startswith(" ") or line.startswith("\t")):
            body.append(line)
    if not found:
        raise PolicyError(f"local_lane_policy.cheap_lane_just_recipes {recipe!r} is missing from justfile")
    return body


def cheap_lane_just_recipe_labels(repo: pathlib.Path, recipe: str) -> list[str]:
    body: list[str] | None = None
    for candidate in (repo, *repo.parents):
        justfile = candidate / "justfile"
        if not justfile.is_file():
            continue
        try:
            body = just_recipe_body(justfile.read_text(encoding="utf-8"), recipe)
        except PolicyError:
            continue
        break
    if body is None:
        raise PolicyError(f"local_lane_policy.cheap_lane_just_recipes {recipe!r} is missing from justfile")
    labels = sorted(
        {
            match.group(1)
            for line in body
            for match in re.finditer(r"(?<![A-Za-z0-9_./-])scripts/([A-Za-z0-9_.-]+\.py)\b", line)
        }
    )
    if not labels:
        raise PolicyError(f"local_lane_policy.cheap_lane_just_recipes {recipe!r} runs no scripts/*.py files")
    return labels


def resolve_cheap_lane_labels(repo: pathlib.Path, lane_policy: dict[str, Any]) -> list[str]:
    labels: list[str] = []
    seen: set[str] = set()

    def append_label(label: str) -> None:
        if label not in seen:
            seen.add(label)
            labels.append(label)

    for label in lane_policy.get("cheap_lane_labels", []):
        append_label(validate_cheap_lane_label(label, "cheap_lane_labels"))

    for recipe in lane_policy.get("cheap_lane_just_recipes", []):
        safe_recipe = validate_cheap_lane_just_recipe(recipe)
        for label in cheap_lane_just_recipe_labels(repo, safe_recipe):
            append_label(validate_cheap_lane_label(label, "cheap_lane_just_recipes"))

    return labels


def validate_local_compile_policy(data: dict[str, Any]) -> None:
    policy = data.get("local_compile_policy")
    if not isinstance(policy, dict):
        raise PolicyError("local_compile_policy table is required")
    if policy.get("enabled") is not True:
        raise PolicyError("local_compile_policy.enabled must be true")
    allowed_ci_env = policy.get("allowed_ci_env")
    break_glass_env = policy.get("break_glass_env")
    if allowed_ci_env != "GITHUB_ACTIONS":
        raise PolicyError("local_compile_policy.allowed_ci_env must be 'GITHUB_ACTIONS'")
    if break_glass_env != "BOLT_ALLOW_LOCAL_RUST":
        raise PolicyError("local_compile_policy.break_glass_env must be 'BOLT_ALLOW_LOCAL_RUST'")
    refused_managed = set(string_array_policy_value(policy, "refused_managed_commands"))
    if refused_managed != {"test", "clippy", "build"}:
        raise PolicyError("local_compile_policy.refused_managed_commands must be test/clippy/build")
    refused_cargo = set(string_array_policy_value(policy, "refused_cargo_subcommands"))
    if refused_cargo != CARGO_DISK_PREFLIGHT_SUBCOMMANDS | CARGO_ALIAS_SUBCOMMANDS:
        raise PolicyError(
            "local_compile_policy.refused_cargo_subcommands must match disk-preflight and alias subcommands"
        )


def validate_remote_compile_cache_policy(data: dict[str, Any]) -> None:
    policy = data.get("remote_compile_cache")
    if policy is None:
        return
    if not isinstance(policy, dict):
        raise PolicyError("remote_compile_cache table must be a table")
    allowed_keys = {"enabled", "enable_env", "ci_env", "wrapper_env", "wrapper_program"}
    for key in policy:
        if key not in allowed_keys:
            raise PolicyError(f"remote_compile_cache.{key} is not supported")
    if policy.get("enabled") is not True:
        raise PolicyError("remote_compile_cache.enabled must be true")
    for key in ("enable_env", "ci_env", "wrapper_env"):
        value = require_non_empty_string(policy, key, "remote_compile_cache")
        if not ENV_NAME_RE.match(value):
            raise PolicyError(f"remote_compile_cache.{key} must be an environment variable name")
    if policy["ci_env"] != "GITHUB_ACTIONS":
        raise PolicyError("remote_compile_cache.ci_env must be 'GITHUB_ACTIONS'")
    wrapper_program = require_non_empty_string(policy, "wrapper_program", "remote_compile_cache")
    if wrapper_program != "sccache":
        raise PolicyError("remote_compile_cache.wrapper_program must be 'sccache'")


def validate_local_lane_policy(data: dict[str, Any]) -> None:
    policy = data.get("local_lane_policy")
    if not isinstance(policy, dict):
        raise PolicyError("local_lane_policy table is required")
    allowed_keys = {
        "enabled",
        "allowed_ci_env",
        "lock_dir",
        "acquire_timeout_seconds",
        "heartbeat_seconds",
        "poll_interval_seconds",
        "cheap_lane_labels",
        "cheap_lane_just_recipes",
        "cheap_lane_max_concurrent",
    }
    for key in policy:
        if key not in allowed_keys:
            raise PolicyError(f"local_lane_policy.{key} is not supported")
    if policy.get("enabled") is not True:
        raise PolicyError("local_lane_policy.enabled must be true")
    if policy.get("allowed_ci_env") != "GITHUB_ACTIONS":
        raise PolicyError("local_lane_policy.allowed_ci_env must be 'GITHUB_ACTIONS'")
    lock_dir = policy.get("lock_dir")
    if not isinstance(lock_dir, str):
        raise PolicyError("local_lane_policy.lock_dir must be an absolute path")
    if "$" in lock_dir or "~" in lock_dir:
        raise PolicyError("local_lane_policy.lock_dir must not contain env or home expansions")
    if not os.path.isabs(lock_dir):
        raise PolicyError("local_lane_policy.lock_dir must be an absolute path")
    values: dict[str, int] = {}
    for key in ("acquire_timeout_seconds", "heartbeat_seconds"):
        value = policy.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            raise PolicyError(f"local_lane_policy.{key} must be a positive integer")
        values[key] = value
    poll = policy.get("poll_interval_seconds")
    if not isinstance(poll, (int, float)) or isinstance(poll, bool) or poll <= 0:
        raise PolicyError("local_lane_policy.poll_interval_seconds must be a positive number")
    if poll > values["heartbeat_seconds"]:
        raise PolicyError(
            "local_lane_policy.poll_interval_seconds must be less than or equal to heartbeat_seconds"
        )
    if values["heartbeat_seconds"] >= values["acquire_timeout_seconds"]:
        raise PolicyError("local_lane_policy.heartbeat_seconds must be less than acquire_timeout_seconds")
    cheap_lane_labels = policy.get("cheap_lane_labels")
    if cheap_lane_labels is not None:
        if not isinstance(cheap_lane_labels, list):
            raise PolicyError("local_lane_policy.cheap_lane_labels must be a list of lane labels")
        seen_labels: set[str] = set()
        for label in cheap_lane_labels:
            safe_label = validate_cheap_lane_label(label, "cheap_lane_labels")
            if safe_label in seen_labels:
                raise PolicyError("local_lane_policy.cheap_lane_labels entries must be unique")
            seen_labels.add(safe_label)
    cheap_lane_just_recipes = policy.get("cheap_lane_just_recipes")
    if cheap_lane_just_recipes is not None:
        if not isinstance(cheap_lane_just_recipes, list):
            raise PolicyError("local_lane_policy.cheap_lane_just_recipes must be a list of just recipe names")
        seen_recipes: set[str] = set()
        for recipe in cheap_lane_just_recipes:
            safe_recipe = validate_cheap_lane_just_recipe(recipe)
            if safe_recipe in seen_recipes:
                raise PolicyError("local_lane_policy.cheap_lane_just_recipes entries must be unique")
            seen_recipes.add(safe_recipe)
    cheap_lane_max_concurrent = policy.get("cheap_lane_max_concurrent", 0)
    if (
        not isinstance(cheap_lane_max_concurrent, int)
        or isinstance(cheap_lane_max_concurrent, bool)
        or cheap_lane_max_concurrent < 0
    ):
        raise PolicyError("local_lane_policy.cheap_lane_max_concurrent must be a non-negative integer")


def validate_remote_verification_policy(data: dict[str, Any]) -> None:
    policy = data.get("remote_verification")
    if not isinstance(policy, dict):
        raise PolicyError("remote_verification table must be a table")
    values: dict[str, int] = {}
    for key in (
        "poll_interval_seconds",
        "checks_appear_timeout_seconds",
        "overall_timeout_seconds",
        "diagnostic_log_max_lines",
        "diagnostic_log_max_bytes",
        "diagnostic_unavailable_notice_interval_polls",
    ):
        value = policy.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            raise PolicyError(f"remote_verification.{key} must be a positive integer")
        values[key] = value
    if values["checks_appear_timeout_seconds"] >= values["overall_timeout_seconds"]:
        raise PolicyError("remote_verification.checks_appear_timeout_seconds must be less than overall_timeout_seconds")


def require_non_empty_string(table: dict[str, Any], key: str, prefix: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise PolicyError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_non_empty_string_array(table: dict[str, Any], key: str, prefix: str) -> list[str]:
    value = table.get(key)
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise PolicyError(f"{prefix}.{key} must be a non-empty string array")
    return value


def validate_git_ref(value: str, key: str) -> None:
    invalid = any(char.isspace() for char in value) or any(char in value for char in "\\^:?*[]~@{}")
    if (
        invalid
        or value.startswith(("/", "."))
        or value.startswith("-")
        or value.endswith(("/", "."))
        or "//" in value
        or ".." in value
    ):
        raise PolicyError(f"remote_probe.{key} must be a safe git ref")


def validate_workflow_path(value: str, key: str) -> None:
    path = pathlib.PurePosixPath(value)
    if value.startswith("/") or ".." in path.parts or not value.endswith(".yml"):
        raise PolicyError(f"remote_probe.{key} must be a relative .yml workflow path")


def validate_relative_workspace_path(value: str, prefix: str) -> None:
    path = pathlib.PurePosixPath(value)
    if value.startswith("/") or ".." in path.parts or not path.parts:
        raise PolicyError(f"{prefix} must be a relative path")


def validate_remote_probe_policy(data: dict[str, Any]) -> None:
    policy = data.get("remote_probe")
    if not isinstance(policy, dict):
        raise PolicyError("remote_probe table must be a table")
    workflow_name = require_non_empty_string(policy, "workflow_name", "remote_probe")
    if workflow_name != "Rust Probe":
        raise PolicyError("remote_probe.workflow_name must be 'Rust Probe'")
    validate_workflow_path(require_non_empty_string(policy, "workflow_path", "remote_probe"), "workflow_path")
    validate_git_ref(require_non_empty_string(policy, "suggest_base_ref", "remote_probe"), "suggest_base_ref")
    values: dict[str, int] = {}
    for key in (
        "poll_interval_seconds",
        "appearance_timeout_seconds",
        "overall_timeout_seconds",
        "active_run_limit",
        "workflow_runs_per_page",
        "guard_timeout_minutes",
    ):
        values[key] = require_positive_int(policy, key, "remote_probe")
    if values["appearance_timeout_seconds"] >= values["overall_timeout_seconds"]:
        raise PolicyError("remote_probe.appearance_timeout_seconds must be less than overall_timeout_seconds")
    allowed = string_array_policy_value(policy, "allowed_runner_tiers")
    if len(set(allowed)) != len(allowed):
        raise PolicyError("remote_probe.allowed_runner_tiers must not contain duplicates")
    mode_tiers = policy.get("mode_runner_tiers")
    if not isinstance(mode_tiers, dict):
        raise PolicyError("remote_probe.mode_runner_tiers table is required")
    if set(mode_tiers) != set(RUST_PROBE_MODES):
        raise PolicyError("remote_probe.mode_runner_tiers must declare every Rust Probe mode")
    for mode, tier in mode_tiers.items():
        if tier not in allowed:
            raise PolicyError(f"remote_probe.mode_runner_tiers.{mode} must be an allowed runner tier")
    timeouts = policy.get("workflow_timeouts")
    if not isinstance(timeouts, dict):
        raise PolicyError("remote_probe.workflow_timeouts table is required")
    expected_timeout_keys = {f"probe-{tier}" for tier in allowed}
    if set(timeouts) != expected_timeout_keys:
        expected = ", ".join(sorted(expected_timeout_keys))
        raise PolicyError(f"remote_probe.workflow_timeouts must declare {expected}")
    for job in sorted(expected_timeout_keys):
        require_positive_int(timeouts, job, "remote_probe.workflow_timeouts")
    max_workflow_timeout_seconds = max(int(timeouts[job]) for job in expected_timeout_keys) * 60
    if values["overall_timeout_seconds"] <= max_workflow_timeout_seconds:
        raise PolicyError("remote_probe.overall_timeout_seconds must exceed remote_probe.workflow_timeouts")
    separate_workspaces = policy.get("separate_workspaces")
    if not isinstance(separate_workspaces, dict) or not separate_workspaces:
        raise PolicyError("remote_probe.separate_workspaces table is required")
    paths: set[str] = set()
    for name, workspace in separate_workspaces.items():
        prefix = f"remote_probe.separate_workspaces.{name}"
        if not SAFE_IDENTIFIER_RE.match(str(name)):
            raise PolicyError("remote_probe.separate_workspaces keys must be safe identifiers")
        if not isinstance(workspace, dict):
            raise PolicyError(f"{prefix} must be a table")
        path = require_non_empty_string(workspace, "path", prefix)
        validate_relative_workspace_path(path, f"{prefix}.path")
        if path in paths:
            raise PolicyError("remote_probe.separate_workspaces paths must not contain duplicates")
        paths.add(path)
        message = require_non_empty_string(workspace, "message", prefix)
        if message.strip() != message or "\n" in message:
            raise PolicyError(f"{prefix}.message must be a single trimmed line")
        commands = require_non_empty_string_array(workspace, "commands", prefix)
        for command in commands:
            if command.strip() != command or "\n" in command:
                raise PolicyError(f"{prefix}.commands entries must be single trimmed lines")


def status_for_repo(repo: pathlib.Path) -> str:
    if not policy_path(repo).exists():
        return "unmanaged"
    try:
        load_policy(repo)
    except (OSError, PolicyError):
        return "invalid-policy"
    return "managed"


def root_base() -> pathlib.Path:
    raw = os.environ.get("RUST_VERIFICATION_ROOT_BASE")
    if raw:
        return pathlib.Path(raw).expanduser()
    return pathlib.Path.home() / ".cache" / "rust-verification"


def target_dir(repo: pathlib.Path, policy: dict[str, Any] | None = None) -> pathlib.Path:
    data = policy if policy is not None else load_policy(repo)
    namespace = data["target_namespace"]
    return root_base() / namespace / "target"


def global_cargo_home_path() -> pathlib.Path:
    env_cargo_home = os.environ.get("CARGO_HOME")
    if env_cargo_home:
        return pathlib.Path(env_cargo_home).expanduser()
    return pathlib.Path.home() / ".cargo"


def global_cargo_config_path() -> pathlib.Path:
    cargo_home = global_cargo_home_path()
    legacy_path = cargo_home / "config"
    if legacy_path.exists():
        return legacy_path
    return cargo_home / "config.toml"


def cargo_config_data(content: str, path: pathlib.Path) -> dict[str, Any]:
    if not content.strip():
        return {}
    if _toml is None:
        raise PolicyError("Python 3.11+ tomllib or tomli is required to parse Cargo config")
    try:
        data = _toml.loads(content)
    except _TOML_DECODE_ERROR as exc:
        raise PolicyError(f"invalid TOML in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise PolicyError(f"invalid TOML in {path}: root must be a table")
    build = data.get("build", {})
    if build is None:
        return data
    if not isinstance(build, dict):
        raise PolicyError(f"invalid TOML in {path}: build must be a table")
    return data


def cargo_config_target_dir_value(content: str, path: pathlib.Path) -> str | None:
    data = cargo_config_data(content, path)
    build = data.get("build", {})
    if not isinstance(build, dict):
        raise PolicyError(f"invalid TOML in {path}: build must be a table")
    value = build.get("target-dir")
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise PolicyError(f"invalid TOML in {path}: build.target-dir must be a non-empty string")
    return value


def insert_cargo_target_dir(content: str, target_dir_value: str) -> str:
    line_ending = "\r\n" if "\r\n" in content else "\n"
    target_line = f"target-dir = {json.dumps(target_dir_value)}{line_ending}"
    dotted_target_line = f"build.target-dir = {json.dumps(target_dir_value)}{line_ending}"
    build_header = re.compile(r"^\s*\[\s*(?:build|['\"]build['\"])\s*\]\s*(?:#.*)?$")
    table_header = re.compile(r"^\s*\[")
    build_dotted_key = re.compile(r"^\s*(?:build|['\"]build['\"])\s*\.")
    lines = content.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if build_header.match(line.rstrip("\r\n")):
            lines.insert(index + 1, target_line)
            return "".join(lines)
    in_root = True
    last_root_build_dotted_index: int | None = None
    for index, line in enumerate(lines):
        stripped = line.rstrip("\r\n")
        if table_header.match(stripped):
            in_root = False
        if in_root and build_dotted_key.match(stripped):
            last_root_build_dotted_index = index
    if last_root_build_dotted_index is not None:
        lines.insert(last_root_build_dotted_index + 1, dotted_target_line)
        return "".join(lines)
    parsed = cargo_config_data(content, pathlib.Path("<cargo-config>"))
    if "build" in parsed:
        raise PolicyError(
            "global Cargo config has a build table that cannot be safely edited; "
            "convert it to a [build] table or build.* dotted keys"
        )
    prefix = content
    if prefix and not prefix.endswith(("\n", "\r")):
        prefix += line_ending
    if prefix and not prefix.endswith(f"{line_ending}{line_ending}"):
        prefix += line_ending
    return f"{prefix}[build]{line_ending}{target_line}"


def cargo_config_with_target_dir(content: str, path: pathlib.Path, target_dir_value: str) -> str:
    before = cargo_config_data(content, path)
    next_content = insert_cargo_target_dir(content, target_dir_value)
    after = cargo_config_data(next_content, path)
    expected = copy.deepcopy(before)
    build = expected.setdefault("build", {})
    if not isinstance(build, dict):
        raise PolicyError(f"invalid TOML in {path}: build must be a table")
    build["target-dir"] = target_dir_value
    if after != expected:
        raise PolicyError(f"refusing to rewrite {path}: existing Cargo config values would not be preserved")
    return next_content


def write_cargo_config_atomic(path: pathlib.Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    write_path = path
    if path.is_symlink():
        try:
            write_path = path.resolve(strict=True)
        except OSError as exc:
            raise PolicyError(f"refusing to rewrite dangling symlink {path}") from exc
    tmp_path = write_path.with_name(f".{write_path.name}.{os.getpid()}.tmp")
    tmp_path.write_text(content, encoding="utf-8")
    os.replace(tmp_path, write_path)


def resolved_cargo_target_dir_value(value: str) -> pathlib.Path | None:
    try:
        return pathlib.Path(value).expanduser().resolve(strict=False)
    except (OSError, RuntimeError, ValueError):
        return None


def assert_global_cargo_target_dir(repo: pathlib.Path) -> dict[str, str]:
    expected_path = target_dir(repo).expanduser().resolve(strict=False)
    expected = str(expected_path)
    config_path = global_cargo_config_path()
    try:
        content = config_path.read_text(encoding="utf-8")
    except FileNotFoundError:
        content = ""
    existing = cargo_config_target_dir_value(content, config_path)
    if existing is not None:
        if resolved_cargo_target_dir_value(existing) == expected_path:
            return {"config_path": str(config_path), "status": "already-configured", "target_dir": expected}
        raise PolicyError(
            "global Cargo build.target-dir already set to "
            f"{existing!r}; expected {expected!r}; refusing to rewrite"
        )
    next_content = cargo_config_with_target_dir(content, config_path, expected)
    write_cargo_config_atomic(config_path, next_content)
    status = "created" if not content else "updated"
    return {"config_path": str(config_path), "status": status, "target_dir": expected}


def cache_lock_path(policy: dict[str, Any]) -> pathlib.Path:
    return root_base() / policy["target_namespace"] / "cache.lock"


@contextlib.contextmanager
def cache_lock(policy: dict[str, Any], *, exclusive: bool) -> Any:
    path = cache_lock_path(policy)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        mode = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
        fcntl.flock(handle.fileno(), mode)
        try:
            yield
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def managed_env(repo: pathlib.Path, policy: dict[str, Any] | None = None) -> dict[str, str]:
    data = policy if policy is not None else load_policy(repo)
    env = os.environ.copy()
    for key in SCRUB_ENV_KEYS:
        env.pop(key, None)
    env["CARGO_TARGET_DIR"] = str(target_dir(repo, data))
    env["RUST_VERIFICATION_PRESERVE_ROUTING_ENV"] = "1"
    env.update(managed_remote_compile_cache_env(data))
    return env


def managed_remote_compile_cache_env(policy: dict[str, Any]) -> dict[str, str]:
    cache_policy = policy.get("remote_compile_cache")
    if not isinstance(cache_policy, dict) or cache_policy.get("enabled") is not True:
        return {}
    if os.environ.get(str(cache_policy["enable_env"])) != "1":
        return {}
    if os.environ.get(str(cache_policy["ci_env"])) != "true":
        return {}

    # Fail-open: the opt-in only resolves to "1" after the CI precondition ladder
    # has installed sccache and exported its path, so a missing or malformed
    # wrapper here means the environment is not what we expect. Degrade to no
    # wrapper (a normal local compile) instead of raising -- the remote compile
    # cache must never be able to fail the build. A structurally invalid
    # *committed* policy still fails loudly in validate_remote_compile_cache_policy;
    # this guards only the runtime environment value.
    wrapper_program = str(cache_policy["wrapper_program"])
    wrapper = os.environ.get(str(cache_policy["wrapper_env"]), "")
    if not wrapper:
        return {}
    if wrapper.strip() != wrapper or any(char.isspace() for char in wrapper):
        return {}
    if pathlib.Path(wrapper).name != wrapper_program:
        return {}
    return {"RUSTC_WRAPPER": wrapper}


def scrubbed_local_env() -> dict[str, str]:
    env = os.environ.copy()
    for key in SCRUB_ENV_KEYS:
        env.pop(key, None)
    return env


def classify_cache_subtree(relative_path: str) -> str:
    if relative_path in ("debug", "release", "tmp"):
        return relative_path
    parts = relative_path.split("-")
    if len(parts) >= 3 and all(parts):
        return "cross-target"
    return "other"


def existing_disk_path(path: pathlib.Path) -> pathlib.Path:
    current = path
    while not current.exists() and current.parent != current:
        current = current.parent
    return current


def scan_cache_tree(path: pathlib.Path) -> tuple[int, float, int]:
    try:
        root_info = path.lstat()
    except FileNotFoundError:
        return 0, 0.0, 0
    except OSError:
        return 0, 0.0, 1
    total_bytes = 0
    skipped = 0
    latest_mtime = float(root_info.st_mtime)
    stack = [(path, root_info)]
    seen_allocated_entries: set[tuple[int, int]] = set()
    while stack:
        current_path, info = stack.pop()
        latest_mtime = max(latest_mtime, float(info.st_mtime))
        mode = info.st_mode
        if stat.S_ISREG(mode) or stat.S_ISLNK(mode):
            inode_key = (int(info.st_dev), int(info.st_ino))
            if inode_key in seen_allocated_entries:
                continue
            seen_allocated_entries.add(inode_key)
            blocks = getattr(info, "st_blocks", None)
            total_bytes += int(blocks) * 512 if blocks is not None else int(info.st_size)
            continue
        if not stat.S_ISDIR(mode):
            skipped += 1
            continue
        try:
            with os.scandir(current_path) as entries:
                for entry in entries:
                    child_path = current_path / entry.name
                    try:
                        child_info = child_path.lstat()
                    except FileNotFoundError:
                        continue
                    except OSError:
                        skipped += 1
                        continue
                    stack.append((child_path, child_info))
        except OSError:
            skipped += 1
    return total_bytes, latest_mtime, skipped


def cache_status_payload(repo: pathlib.Path) -> dict[str, Any]:
    policy = load_policy(repo)
    target = target_dir(repo, policy)
    filesystem = shutil.disk_usage(existing_disk_path(target))
    subtrees: list[dict[str, Any]] = []
    total_bytes = 0
    skipped_special_entries = 0
    if target.exists():
        for child in sorted(target.iterdir(), key=lambda item: item.name):
            child_bytes, latest_mtime, skipped = scan_cache_tree(child)
            total_bytes += child_bytes
            skipped_special_entries += skipped
            subtrees.append(
                {
                    "bytes": child_bytes,
                    "class": classify_cache_subtree(child.name),
                    "latest_mtime": latest_mtime,
                    "path": str(child),
                    "relative_path": child.name,
                    "skipped_special_entries": skipped,
                }
            )
    pressure = cache_pressure(total_bytes=total_bytes, filesystem_free=filesystem.free, policy=policy)
    return {
        "filesystem": {
            "free_bytes": filesystem.free,
            "total_bytes": filesystem.total,
            "used_bytes": filesystem.used,
        },
        "policy": str(policy_path(repo)),
        "skipped_special_entries": skipped_special_entries,
        "status": "ok",
        "subtrees": subtrees,
        "target_dir": str(target),
        **pressure,
        "total_bytes": total_bytes,
    }


def cache_config(policy: dict[str, Any], *, required: bool = False) -> dict[str, Any]:
    config = policy.get("cache")
    if config is None:
        if required:
            raise PolicyError("cache table is required")
        return {}
    if not isinstance(config, dict):
        raise PolicyError("cache table must be a table")
    return config


def cache_thresholds(policy: dict[str, Any]) -> dict[str, int]:
    config = cache_config(policy, required=True)
    thresholds: dict[str, int] = {}
    for key in ("min_free_bytes", "soft_limit_bytes"):
        value = config.get(key)
        if not is_non_negative_int(value):
            raise PolicyError(f"cache.{key} must be a non-negative integer")
        thresholds[key] = value
    return thresholds


def is_non_negative_int(value: Any) -> bool:
    return type(value) is int and value >= 0


def cache_pressure(*, total_bytes: int, filesystem_free: int, policy: dict[str, Any]) -> dict[str, Any]:
    if "cache" not in policy:
        return {"pressure": False, "pressure_reasons": [], "thresholds": None}
    thresholds = cache_thresholds(policy)
    reasons: list[str] = []
    if total_bytes > thresholds["soft_limit_bytes"]:
        reasons.append("cache exceeds soft_limit_bytes")
    if filesystem_free < thresholds["min_free_bytes"]:
        reasons.append("filesystem free below min_free_bytes")
    return {
        "pressure": bool(reasons),
        "pressure_reasons": reasons,
        "thresholds": thresholds,
    }


def retention_config(policy: dict[str, Any], class_name: str) -> dict[str, Any]:
    retention = cache_config(policy, required=True).get("retention", {})
    if not isinstance(retention, dict):
        raise PolicyError("cache.retention table must be a table")
    config = retention.get(class_name, {})
    if not isinstance(config, dict):
        raise PolicyError(f"cache.retention.{class_name} must be a table")
    return config


def active_process_patterns(policy: dict[str, Any]) -> list[str]:
    patterns = cache_config(policy, required=True).get("active_process_patterns", [])
    if not isinstance(patterns, list) or not all(isinstance(pattern, str) and pattern for pattern in patterns):
        raise PolicyError("cache.active_process_patterns must be a string array")
    return patterns


def validate_cache_policy(policy: dict[str, Any]) -> None:
    config = cache_config(policy, required=True)
    cache_thresholds(policy)
    active_process_patterns(policy)
    retention = config.get("retention")
    if not isinstance(retention, dict):
        raise PolicyError("cache.retention table must be a table")
    for class_name in ("debug", "release", "cross-target", "tmp", "other"):
        class_config = retention_config(policy, class_name)
        prunable = class_config.get("prunable")
        if not isinstance(prunable, bool):
            raise PolicyError(f"cache.retention.{class_name}.prunable must be a boolean")
        prune_after_days = class_config.get("prune_after_days")
        if prunable and not is_non_negative_int(prune_after_days):
            raise PolicyError(f"cache.retention.{class_name}.prune_after_days must be a non-negative integer")


def is_prune_candidate(
    subtree: dict[str, Any],
    policy: dict[str, Any],
    *,
    now: float,
    pressure: bool,
) -> tuple[bool, str]:
    if not pressure:
        return False, "cache below pressure thresholds"
    config = retention_config(policy, subtree["class"])
    if config.get("prunable") is not True:
        return False, "class is not prunable"
    prune_after_days = config.get("prune_after_days")
    if not is_non_negative_int(prune_after_days):
        return False, "class has no prune age"
    if subtree.get("skipped_special_entries", 0):
        return False, "subtree scan incomplete"
    cutoff = now - (prune_after_days * 24 * 60 * 60)
    if subtree["latest_mtime"] > cutoff:
        return False, "subtree is newer than prune age"
    return True, f"older than {prune_after_days} days"


def process_cwd_from_proc(pid: int) -> pathlib.Path | None:
    base = pathlib.Path(os.environ.get("RUST_VERIFICATION_PROCESS_CWD_BASE", "/proc"))
    try:
        return (base / str(pid) / "cwd").resolve(strict=True)
    except (OSError, RuntimeError):
        return None


def process_cwd_from_lsof(pid: int) -> pathlib.Path | None:
    try:
        result = subprocess.run(
            ["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=2,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return None
    if result.returncode != 0:
        return None
    for line in result.stdout.splitlines():
        if not line.startswith("n"):
            continue
        try:
            return pathlib.Path(line[1:]).resolve(strict=True)
        except (OSError, RuntimeError):
            return None
    return None


def process_cwd(pid: int) -> pathlib.Path | None:
    return process_cwd_from_proc(pid) or process_cwd_from_lsof(pid)


def path_is_or_inside(path: pathlib.Path, parent: pathlib.Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def command_tokens(command: str) -> list[str]:
    try:
        return shlex.split(command)
    except ValueError:
        return command.split()


def basename_token(token: str) -> str:
    return pathlib.Path(token).name


def shell_normalized_tokens(tokens: list[str]) -> list[str]:
    normalized: list[str] = []
    for raw_token in tokens:
        if re.search(r"\s", raw_token):
            normalized.append(raw_token)
            continue
        normalized.extend(part for part in SHELL_BOUNDARY_TOKEN_RE.split(raw_token) if part)
    return normalized


def strip_shell_redirections(tokens: list[str]) -> list[str]:
    stripped: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        operator_index = index
        if (
            token.isdigit()
            and index + 1 < len(tokens)
            and tokens[index + 1] in SHELL_REDIRECTION_OPERATORS
        ):
            operator_index = index + 1
        if tokens[operator_index] in SHELL_REDIRECTION_OPERATORS:
            index = operator_index + 1
            if index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
                index += 1
            continue
        stripped.append(token)
        index += 1
    return stripped


def backtick_command_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        start = token.find("`")
        if start < 0:
            index += 1
            continue
        payload_parts: list[str] = []
        remainder = token[start + 1 :]
        end = remainder.find("`")
        if end >= 0:
            payload = remainder[:end].strip()
            if payload:
                payloads.append(command_tokens(payload))
            index += 1
            continue
        if remainder:
            payload_parts.append(remainder)
        cursor = index + 1
        while cursor < len(tokens):
            part = tokens[cursor]
            end = part.find("`")
            if end >= 0:
                if end:
                    payload_parts.append(part[:end])
                break
            payload_parts.append(part)
            cursor += 1
        if cursor < len(tokens):
            payload = " ".join(payload_parts).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor + 1
            continue
        index += 1
    return payloads


def inline_command_substitution_payloads(token: str) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index + 1 < len(token):
        if token[index : index + 2] not in {"$(", "<("}:
            index += 1
            continue
        cursor = index + 2
        depth = 1
        payload_chars: list[str] = []
        while cursor < len(token) and depth:
            char = token[cursor]
            if char == "(":
                depth += 1
                payload_chars.append(char)
            elif char == ")":
                depth -= 1
                if depth:
                    payload_chars.append(char)
            else:
                payload_chars.append(char)
            cursor += 1
        if depth == 0:
            payload = "".join(payload_chars).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor
            continue
        index += 1
    return payloads


def shell_command_substitution_payloads(tokens: list[str]) -> list[list[str]]:
    normalized = shell_normalized_tokens(tokens)
    payloads = backtick_command_payloads(normalized)
    for token in normalized:
        payloads.extend(inline_command_substitution_payloads(token))
    index = 0
    while index + 1 < len(normalized):
        token = normalized[index]
        if (token == "$" or token.endswith("$") or token == "<") and normalized[index + 1] == "(":
            cursor = index + 2
            depth = 1
            payload: list[str] = []
            while cursor < len(normalized) and depth:
                current = normalized[cursor]
                if current == "(":
                    depth += 1
                    payload.append(current)
                elif current == ")":
                    depth -= 1
                    if depth:
                        payload.append(current)
                else:
                    payload.append(current)
                cursor += 1
            if depth == 0:
                if payload:
                    payloads.append(payload)
                index = cursor
                continue
        index += 1
    return payloads


def shell_command_substitution_at(tokens: list[str], index: int) -> tuple[list[str], int] | None:
    normalized = shell_normalized_tokens(tokens)
    if index + 1 >= len(normalized) or normalized[index] != "$" or normalized[index + 1] != "(":
        return None
    cursor = index + 2
    depth = 1
    payload: list[str] = []
    while cursor < len(normalized) and depth:
        token = normalized[cursor]
        if token == "(":
            depth += 1
            payload.append(token)
        elif token == ")":
            depth -= 1
            if depth:
                payload.append(token)
        else:
            payload.append(token)
        cursor += 1
    return (payload, cursor) if depth == 0 else None


def shell_command_segments(tokens: list[str]) -> list[list[str]]:
    segments: list[list[str]] = []
    segment: list[str] = []
    normalized = shell_normalized_tokens(tokens)
    index = 0
    substitution_depth = 0
    while index < len(normalized):
        token = normalized[index]
        if token == "$" and index + 1 < len(normalized) and normalized[index + 1] == "(":
            segment.extend([token, normalized[index + 1]])
            substitution_depth += 1
            index += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            if segment:
                segments.append(segment)
                segment = []
            index += 1
            continue
        segment.append(token)
        index += 1
    if segment:
        segments.append(segment)
    return segments if len(segments) > 1 else []


def python_script_name(tokens: list[str], start: int) -> str | None:
    for token in tokens[start:]:
        if token in ("-c", "-m"):
            return None
        name = basename_token(token)
        if name.endswith(".py"):
            return name
    return None


def shell_command(tokens: list[str]) -> str | None:
    # POSIX shells accept `-c` either alone (bash -c CMD) or combined with
    # other short flags (bash -lc CMD, bash -ic CMD, sh -ec CMD, zsh -fc CMD).
    # A cluster is any token with a single leading "-" (not "--") that
    # contains the letter "c"; the command string is always the next token.
    # Long-form (--command=) is not POSIX and is not supported here.
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if index + 1 < len(tokens):
            if token == "-c":
                return tokens[index + 1]
            if (
                token.startswith("-")
                and not token.startswith("--")
                and len(token) > 1
                and "c" in token[1:]
            ):
                return tokens[index + 1]
        index += 1
    return None


def env_command_index(tokens: list[str]) -> int:
    signal_options = ("--block-signal", "--default-signal", "--ignore-signal")
    index = 1
    while index < len(tokens):
        token = tokens[index]
        redirection_index = shell_redirection_next_index(tokens, index)
        if redirection_index is not None:
            index = redirection_index
            continue
        if token == "--":
            return index + 1
        if token in ("-i", "--ignore-environment", "-0", "--null", "-v", "--debug"):
            index += 1
            continue
        if token == "--split-string" and index + 1 < len(tokens):
            return index
        if token.startswith("--split-string="):
            return index
        if token in signal_options:
            index += 1
            continue
        if token.startswith(tuple(f"{option}=" for option in signal_options)):
            index += 1
            continue
        if token in ("-u", "--unset", "-C", "--chdir") and index + 1 < len(tokens):
            index += 2
            continue
        if token == "-S" and index + 1 < len(tokens):
            return index
        if token.startswith("--unset=") or token.startswith("--chdir="):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            if "S" in cluster and (cluster.split("S", 1)[1] or index + 1 < len(tokens)):
                return index
            parsed_index = env_short_cluster_next_index(tokens, index, cluster)
            if parsed_index is not None:
                index = parsed_index
                continue
        if "=" in token and not token.startswith("-"):
            index += 1
            continue
        return index
    return index


def shell_redirection_next_index(tokens: list[str], index: int) -> int | None:
    token = tokens[index]
    if token in {">", ">>", "<", "<<", "<>", ">|"}:
        return min(index + 2, len(tokens))
    if re.match(r"^\d?(?:>>?|<<?|<>|>\|).+", token):
        return index + 1
    return None


def env_short_cluster_next_index(tokens: list[str], index: int, cluster: str) -> int | None:
    offset = 0
    while offset < len(cluster):
        option = cluster[offset]
        if option in "i0v":
            offset += 1
            continue
        if option in "uC":
            if offset + 1 < len(cluster):
                return index + 1
            if index + 1 < len(tokens):
                return index + 2
            return index + 1
        return None
    return index + 1


def env_short_split_command(token: str, rest: list[str]) -> str | None:
    if not token.startswith("-") or token.startswith("--"):
        return None
    cluster = token[1:]
    if "S" not in cluster:
        return None
    suffix = cluster.split("S", 1)[1]
    if suffix:
        return " ".join([suffix, *rest]).strip()
    if rest:
        return " ".join(rest).strip()
    return None


def env_wrapped_tokens(tokens: list[str]) -> list[str]:
    index = env_command_index(tokens)
    if index < len(tokens):
        token = tokens[index]
        split_command: str | None = None
        if (token == "-S" or token == "--split-string") and index + 1 < len(tokens):
            split_command = " ".join(tokens[index + 1 :])
        elif token.startswith("--split-string="):
            split_command = " ".join([token.split("=", 1)[1], *tokens[index + 1 :]]).strip()
        elif token.startswith("-") and not token.startswith("--"):
            split_command = env_short_split_command(token, tokens[index + 1 :])
        if split_command is not None:
            split_tokens = command_tokens(split_command)
            split_index = env_command_index(["env", *split_tokens]) - 1
            return split_tokens[max(split_index, 0) :]
    return tokens[index:]


def shell_assignment_word(token: str) -> bool:
    return re.match(r"^[A-Za-z_][A-Za-z0-9_]*=.*$", token) is not None


def consume_assignment_words(tokens: list[str], index: int) -> int:
    while index < len(tokens) and shell_assignment_word(tokens[index]):
        index += 1
    return index


def sudo_command_index(tokens: list[str]) -> int:
    argument_options = {
        "-u",
        "--user",
        "-g",
        "--group",
        "-h",
        "--host",
        "-p",
        "--prompt",
        "-C",
        "--close-from",
        "-T",
        "--command-timeout",
        "-r",
        "--role",
        "-t",
        "--type",
        "-U",
        "--other-user",
        "-D",
        "--chdir",
        "-R",
        "--chroot",
        "-a",
        "--auth-type",
        "-c",
        "--login-class",
    }
    no_argument_options = {
        "-A",
        "-b",
        "-E",
        "-e",
        "-H",
        "-i",
        "-K",
        "-k",
        "-l",
        "-n",
        "-P",
        "-S",
        "-s",
        "-V",
        "-v",
        "--askpass",
        "--background",
        "--bell",
        "--edit",
        "--help",
        "--ignore-ticket",
        "--list",
        "--login",
        "--non-interactive",
        "--remove-timestamp",
        "--reset-timestamp",
        "--stdin",
        "--validate",
        "--version",
    }
    optional_argument_options = {"--preserve-env"}
    short_argument_options = {option for option in argument_options if re.fullmatch(r"-[A-Za-z0-9]", option)}
    short_no_argument_options = {option for option in no_argument_options if re.fullmatch(r"-[A-Za-z0-9]", option)}
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            continue
        if token in argument_options and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in argument_options if option.startswith("--")):
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in optional_argument_options):
            index += 1
            continue
        if token in optional_argument_options:
            index += 1
            continue
        if token in no_argument_options:
            index += 1
            continue
        if len(token) > 2 and token.startswith("-") and not token.startswith("--"):
            offset = 1
            while offset < len(token):
                option = f"-{token[offset]}"
                if option in short_no_argument_options:
                    offset += 1
                    continue
                if option in short_argument_options:
                    if offset + 1 < len(token):
                        index += 1
                    elif index + 1 < len(tokens):
                        index += 2
                    else:
                        index += 1
                    break
                index += 1
                break
            else:
                index += 1
            continue
        if "=" in token and not token.startswith("-"):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return index
    return index


def nice_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token == "-n" and index + 1 < len(tokens):
            index += 2
            continue
        if token == "--adjustment" and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("--adjustment="):
            index += 1
            continue
        if re.fullmatch(r"-n-?\d+", token):
            index += 1
            continue
        if re.fullmatch(r"-?\d+", token):
            index += 1
            continue
        return index
    return index


def flock_wrapped_tokens(tokens: list[str]) -> list[str]:
    command_option_tokens = flock_command_option_tokens(tokens)
    if command_option_tokens is not None:
        return command_option_tokens
    index = 1
    separator_seen = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            separator_seen = True
            break
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token in ("-E", "--conflict-exit-code", "-w", "--wait", "--timeout") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return tokens[index + 1 :]
    if separator_seen and index < len(tokens):
        return tokens[index + 1 :]
    return tokens[index:]


def flock_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return None
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:] and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        index += 1
    return None


def rustup_run_tokens(tokens: list[str]) -> list[str]:
    index = 2
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return []
    index += 1
    while index < len(tokens) and tokens[index] == "--":
        index += 1
    return tokens[index:]


def timeout_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            if index < len(tokens):
                index += 1
            break
        if token in ("-k", "--kill-after", "-s", "--signal") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--kill-after=", "--signal=")):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        index += 1
        break
    return index


def stdbuf_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in ("-i", "-o", "-e", "--input", "--output", "--error") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--input=", "--output=", "--error=")):
            index += 1
            continue
        if re.fullmatch(r"-[ioe].+", token):
            index += 1
            continue
        return index
    return index


def command_builtin_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token == "-p":
            index += 1
            continue
        if token in ("-v", "-V"):
            return len(tokens)
        return index
    return index


def exec_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token == "-a" and index + 1 < len(tokens):
            index += 2
            continue
        if token in ("-c", "-l"):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            if set(cluster) <= {"c", "l"}:
                index += 1
                continue
            if cluster.endswith("a") and set(cluster[:-1]) <= {"c", "l"} and index + 1 < len(tokens):
                index += 2
                continue
        return index
    return index


def container_rust_payload_from_tokens(tokens: list[str], start: int) -> list[str] | None:
    for index in range(start, len(tokens)):
        token = tokens[index]
        executable = basename_token(token)
        if (
            executable_is_rust_tool(executable)
            or path_executable_looks_like_cargo(token)
            or path_executable_looks_like_rustc(token)
            or path_name_looks_like_renamed_cargo(executable)
            or path_name_looks_like_renamed_rustc(executable)
        ):
            return tokens[index:]
    return None


def container_wrapped_tokens(tokens: list[str]) -> list[str] | None:
    if len(tokens) < 3:
        return None
    executable = basename_token(tokens[0])
    if executable not in {"docker", "podman"}:
        return None
    command = tokens[1]
    options_with_argument = {
        "--add-host",
        "--cpus",
        "--entrypoint",
        "--env",
        "--env-file",
        "--hostname",
        "--mount",
        "--name",
        "--network",
        "--platform",
        "--user",
        "--volume",
        "--workdir",
        "-e",
        "-h",
        "-m",
        "-u",
        "-v",
        "-w",
    }
    index = 2
    entrypoint: str | None = None
    uncertain_options = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token in options_with_argument and index + 1 < len(tokens):
            if token == "--entrypoint":
                entrypoint = tokens[index + 1]
            index += 2
            continue
        if token.startswith("--entrypoint="):
            entrypoint = token.split("=", 1)[1]
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-"):
            uncertain_options = True
            index += 1
            continue
        break
    if command in {"run", "exec"}:
        if index >= len(tokens):
            return []
        tail = tokens[index + 1 :]
        if entrypoint is not None:
            return [entrypoint, *tail]
        if uncertain_options:
            fallback = container_rust_payload_from_tokens(tokens, 2)
            if fallback is not None:
                return fallback
        return tail
    return None


def chroot_wrapped_tokens(tokens: list[str]) -> list[str]:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token.startswith("--userspec=") or token.startswith("--groups="):
            index += 1
            continue
        if token in {"--userspec", "--groups"} and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return tokens[index + 1 :] if index < len(tokens) else []


def setsid_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in ("-c", "--ctty", "-f", "--fork", "-w", "--wait"):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--") and set(token[1:]) <= {"c", "f", "w"}:
            index += 1
            continue
        return index
    return index


def time_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in ("-f", "--format", "-o", "--output") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--format=", "--output=")) or re.fullmatch(r"-[fo].+", token):
            index += 1
            continue
        if token in ("-a", "--append", "-p", "--portability", "-v", "--verbose"):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--") and set(token[1:]) <= {"a", "p", "v"}:
            index += 1
            continue
        return index
    return index


def taskset_command_index(tokens: list[str]) -> int:
    index = 1
    cpu_list_mode = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            continue
        if token in ("-c", "--cpu-list") and index + 1 < len(tokens):
            index += 2
            cpu_list_mode = True
            continue
        if token.startswith("--cpu-list=") or re.fullmatch(r"-c.+", token):
            index += 1
            cpu_list_mode = True
            continue
        if token in ("-a", "--all-tasks"):
            index += 1
            continue
        if token in ("-p", "--pid"):
            return len(tokens)
        if token.startswith("-"):
            index += 1
            continue
        if not cpu_list_mode:
            index += 1
        return index
    return index


def ionice_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in ("-c", "--class", "-n", "--classdata") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--class=", "--classdata=")) or re.fullmatch(r"-[cn].+", token):
            index += 1
            continue
        if token in ("-p", "--pid"):
            return len(tokens)
        if token in ("-t", "--ignore"):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            if cluster and (set(cluster) <= {"t"} or re.fullmatch(r"t*[cn].+", cluster)):
                index += 1
                continue
        return index
    return index


def chrt_command_index(tokens: list[str]) -> int:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token in ("-p", "--pid"):
            return len(tokens)
        if token in ("-T", "--sched-runtime", "-P", "--sched-period", "-D", "--sched-deadline") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--sched-runtime=", "--sched-period=", "--sched-deadline=")):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    if index < len(tokens):
        index += 1
    return index


def xargs_command_index(tokens: list[str]) -> int:
    options_with_argument = {"-a", "--arg-file", "-d", "--delimiter", "-E", "-I", "-L", "-n", "--max-args", "-P", "--max-procs", "-s", "--max-chars"}
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in options_with_argument and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if re.fullmatch(r"-(?:a|d|E|I|L|n|P|s).+", token):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return index
    return index


def su_wrapped_tokens(tokens: list[str]) -> list[str] | None:
    executable = basename_token(tokens[0]) if tokens else ""
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"-c", "--command"} and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if executable == "runuser":
            if token == "--":
                return tokens[index + 1 :]
            if token in {"-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell"} and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--user=", "--group=", "--supp-group=", "--shell=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index:]
        index += 1
    return None


def find_exec_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 1
    while index < len(tokens):
        if tokens[index] not in {"-exec", "-execdir"}:
            index += 1
            continue
        index += 1
        payload: list[str] = []
        while index < len(tokens) and tokens[index] not in {";", "+"}:
            payload.append(tokens[index])
            index += 1
        if payload:
            payloads.append(payload)
    return payloads


def no_mistakes_wrapped_tokens(tokens: list[str]) -> list[str] | None:
    for index, token in enumerate(tokens):
        if token == "--":
            return tokens[index + 1 :]
    return None


def process_wrapper_tokens(tokens: list[str]) -> list[str] | None:
    if not tokens:
        return None
    executable = basename_token(tokens[0])
    if executable == "env":
        return env_wrapped_tokens(tokens)
    if executable in ("sudo", "doas"):
        return tokens[sudo_command_index(tokens) :]
    if executable == "nice":
        return tokens[nice_command_index(tokens) :]
    if executable == "flock":
        return flock_wrapped_tokens(tokens)
    if executable == "rustup" and len(tokens) >= 4 and tokens[1] == "run":
        return rustup_run_tokens(tokens)
    if executable == "timeout":
        return tokens[timeout_command_index(tokens) :]
    if executable == "stdbuf":
        return tokens[stdbuf_command_index(tokens) :]
    if executable == "catchsegv":
        return tokens[1:]
    if executable in {"docker", "podman"}:
        return container_wrapped_tokens(tokens)
    if executable == "chroot":
        return chroot_wrapped_tokens(tokens)
    if executable == "command":
        return tokens[command_builtin_index(tokens) :]
    if executable == "exec":
        return tokens[exec_command_index(tokens) :]
    if executable == "nohup":
        return tokens[1:]
    if executable == "setsid":
        return tokens[setsid_command_index(tokens) :]
    if executable == "time":
        return tokens[time_command_index(tokens) :]
    if executable == "taskset":
        return tokens[taskset_command_index(tokens) :]
    if executable == "ionice":
        return tokens[ionice_command_index(tokens) :]
    if executable == "chrt":
        return tokens[chrt_command_index(tokens) :]
    if executable == "xargs":
        return tokens[xargs_command_index(tokens) :]
    if executable in {"runuser", "sg", "su"}:
        return su_wrapped_tokens(tokens)
    if executable == "no-mistakes":
        return no_mistakes_wrapped_tokens(tokens)
    return None


def process_names_from_tokens(tokens: list[str], *, depth: int = 0) -> set[str]:
    # Depth cap guards against pathological re-tokenisation loops while
    # leaving headroom for realistic legitimate wrapper stacks. A supported
    # chain like `sudo nice env -i bash -c 'rustup run stable cargo test'`
    # reaches `cargo` at depth 5; the cap keeps one slot of safety margin.
    if not tokens:
        return set()
    if depth > PROCESS_PARSE_DEPTH_LIMIT:
        return {PROCESS_PARSE_DEPTH_EXCEEDED}
    substitution_names: set[str] = set()
    for payload in shell_command_substitution_payloads(tokens):
        substitution_names.update(process_names_from_tokens(payload, depth=depth + 1))
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index >= len(tokens):
        return substitution_names
    if assignment_index:
        tokens = tokens[assignment_index:]
    segments = shell_command_segments(tokens)
    if segments:
        names = set(substitution_names)
        for segment in segments:
            names.update(process_names_from_tokens(segment, depth=depth + 1))
        return names
    tokens = strip_shell_redirections(shell_normalized_tokens(tokens))
    if not tokens:
        return substitution_names
    executable = basename_token(tokens[0])
    names = {executable, *substitution_names}
    wrapped_tokens = process_wrapper_tokens(tokens)
    if wrapped_tokens is not None:
        names.update(process_names_from_tokens(wrapped_tokens, depth=depth + 1))
    elif executable == "eval":
        eval_index = 1
        if eval_index < len(tokens) and tokens[eval_index] == "--":
            eval_index += 1
        names.update(process_names_from_tokens(command_tokens(" ".join(tokens[eval_index:])), depth=depth + 1))
    elif executable in ("bash", "dash", "fish", "sh", "zsh"):
        command = shell_command(tokens)
        if command is not None:
            names.update(process_names_from_tokens(command_tokens(command), depth=depth + 1))
    elif executable.startswith("python"):
        for payload in python_inline_command_payloads(tokens):
            names.update(process_names_from_tokens(command_tokens(payload), depth=depth + 1))
        script_name = python_script_name(tokens, 1)
        if script_name is not None:
            names.add(script_name)
    elif executable == "find":
        for payload in find_exec_payloads(tokens):
            names.update(process_names_from_tokens(payload, depth=depth + 1))
    return names


def command_process_names(command: str) -> set[str]:
    tokens = command_tokens(command)
    return process_names_from_tokens(tokens)


def rust_tool_name_has_script_extension(name: str) -> bool:
    return pathlib.Path(name).suffix.lower() in {".bash", ".fish", ".ksh", ".ps1", ".py", ".rb", ".sh", ".zsh"}


def executable_is_rust_tool(executable: str) -> bool:
    if rust_tool_name_has_script_extension(executable):
        return False
    return (
        executable in {"cargo", "clippy", "nextest", "rustc", "rustdoc", "rustup"}
        or executable.startswith(("cargo-", "clippy-", "rust-"))
    )


def path_name_looks_like_renamed_cargo(executable: str) -> bool:
    return executable == "c" or executable_is_rust_tool(executable) or (
        executable.endswith("cargo") and "_" not in executable
    )


def path_executable_looks_like_cargo(token: str) -> bool:
    if "/" not in token:
        return False
    executable = basename_token(token)
    if path_name_looks_like_renamed_cargo(executable):
        return True
    try:
        resolved = pathlib.Path(token).expanduser().resolve(strict=True)
    except (OSError, RuntimeError):
        return False
    return path_name_looks_like_renamed_cargo(resolved.name)


def path_name_looks_like_renamed_rustc(executable: str) -> bool:
    return executable == "r" or executable == "rustc" or (executable.endswith("rustc") and "_" not in executable)


def path_executable_looks_like_rustc(token: str) -> bool:
    if "/" not in token:
        return False
    executable = basename_token(token)
    if path_name_looks_like_renamed_rustc(executable):
        return True
    try:
        resolved = pathlib.Path(token).expanduser().resolve(strict=True)
    except (OSError, RuntimeError):
        return False
    return path_name_looks_like_renamed_rustc(resolved.name)


RUSTC_SPECIFIC_TOKENS = {"--crate-name", "--emit", "--out-dir"}


def tokens_have_rustc_specific_flags(tokens: list[str]) -> bool:
    return any(
        token in RUSTC_SPECIFIC_TOKENS or token.startswith("--emit=") or token.startswith("--out-dir=")
        for token in tokens
    )


def tokens_may_be_renamed_rustc(tokens: list[str], *, depth: int = 0) -> bool:
    if not tokens:
        return False
    if depth > PROCESS_PARSE_DEPTH_LIMIT:
        return True
    tokens = strip_shell_redirections(shell_normalized_tokens(tokens))
    if not tokens:
        return False
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index >= len(tokens):
        return False
    if assignment_index:
        return tokens_may_be_renamed_rustc(tokens[assignment_index:], depth=depth + 1)
    segments = shell_command_segments(tokens)
    if segments:
        return any(tokens_may_be_renamed_rustc(segment, depth=depth + 1) for segment in segments)
    if (
        (path_executable_looks_like_rustc(tokens[0]) or path_name_looks_like_renamed_rustc(basename_token(tokens[0])))
        and tokens_have_rustc_specific_flags(tokens[1:])
    ):
        return True
    wrapped_tokens = process_wrapper_tokens(tokens)
    if wrapped_tokens is None:
        executable = basename_token(tokens[0])
        if executable == "eval":
            eval_index = 1
            if eval_index < len(tokens) and tokens[eval_index] == "--":
                eval_index += 1
            wrapped_tokens = command_tokens(" ".join(tokens[eval_index:]))
        elif executable in ("bash", "dash", "fish", "sh", "zsh"):
            command = shell_command(tokens)
            wrapped_tokens = command_tokens(command) if command is not None else None
        elif executable.startswith("python"):
            return any(
                tokens_may_be_renamed_rustc(command_tokens(payload), depth=depth + 1)
                for payload in python_inline_command_payloads(tokens)
            )
    return wrapped_tokens is not None and tokens_may_be_renamed_rustc(wrapped_tokens, depth=depth + 1)


def command_may_launch_rust(command: str) -> bool:
    tokens = command_tokens(command)
    if not tokens:
        return False
    if command_may_be_renamed_cargo(command):
        return True
    if tokens_may_be_renamed_rustc(tokens):
        return True
    tokens = strip_shell_redirections(shell_normalized_tokens(tokens))
    if not tokens:
        return False
    executable = basename_token(tokens[0])
    if executable_is_rust_tool(executable):
        return True
    names = process_names_from_tokens(tokens)
    if PROCESS_PARSE_DEPTH_EXCEEDED in names:
        return True
    if any(executable_is_rust_tool(name) or name == "nextest" for name in names):
        return True
    cargo_specific_tokens = {"--manifest-path", "--workspace", "--all-targets", "--all-features"}
    if any(token in cargo_specific_tokens for token in tokens):
        if any(token in CARGO_PROCESS_SUBCOMMANDS for token in tokens):
            return True
    if executable not in OPAQUE_RUST_LAUNCHERS:
        return False
    return any(
        re.search(r"(^|[^A-Za-z0-9_-])(?:cargo|rustc|rustdoc|rustup|clippy|nextest)(?:[^A-Za-z0-9_-]|$)", token)
        for token in tokens
    )


def command_may_launch_build(command: str) -> bool:
    tokens = command_tokens(command)
    if not tokens:
        return False
    executable = basename_token(tokens[0])
    return executable in OPAQUE_RUST_LAUNCHERS and "build" in command.lower()


def command_may_be_renamed_cargo(command: str) -> bool:
    tokens = command_tokens(command)
    return tokens_may_be_renamed_cargo(tokens)


def tokens_may_be_renamed_cargo(tokens: list[str], *, depth: int = 0) -> bool:
    if not tokens:
        return False
    if depth > PROCESS_PARSE_DEPTH_LIMIT:
        return True
    tokens = strip_shell_redirections(shell_normalized_tokens(tokens))
    if not tokens:
        return False
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index >= len(tokens):
        return False
    if assignment_index:
        return tokens_may_be_renamed_cargo(tokens[assignment_index:], depth=depth + 1)
    segments = shell_command_segments(tokens)
    if segments:
        return any(tokens_may_be_renamed_cargo(segment, depth=depth + 1) for segment in segments)
    if (
        (path_executable_looks_like_cargo(tokens[0]) or path_name_looks_like_renamed_cargo(basename_token(tokens[0])))
        and cargo_subcommand(tokens[1:]) in CARGO_PROCESS_SUBCOMMANDS
    ):
        return True
    wrapped_tokens = process_wrapper_tokens(tokens)
    if wrapped_tokens is None:
        executable = basename_token(tokens[0])
        if executable == "eval":
            eval_index = 1
            if eval_index < len(tokens) and tokens[eval_index] == "--":
                eval_index += 1
            wrapped_tokens = command_tokens(" ".join(tokens[eval_index:]))
        elif executable in ("bash", "dash", "fish", "sh", "zsh"):
            command = shell_command(tokens)
            wrapped_tokens = command_tokens(command) if command is not None else None
        elif executable.startswith("python"):
            return any(
                tokens_may_be_renamed_cargo(command_tokens(payload), depth=depth + 1)
                for payload in python_inline_command_payloads(tokens)
            )
    return wrapped_tokens is not None and tokens_may_be_renamed_cargo(wrapped_tokens, depth=depth + 1)


def matching_process_pattern(command: str, patterns: list[str]) -> str | None:
    names = command_process_names(command)
    return next((pattern for pattern in patterns if basename_token(pattern) in names), None)


def cargo_config_scope_path_values(config: str) -> list[str]:
    stripped = decode_toml_unicode_escapes(config).strip().strip("\"'")
    if not stripped:
        return []
    if cargo_config_looks_like_path(stripped):
        return [stripped]
    values: list[str] = []
    for pattern in (
        r"(?:^|[\s,{])build\.target-dir\s*=\s*[\"']?([^\"'\s,}\]]+)",
        r"(?:^|[\s,{])target-dir\s*=\s*[\"']?([^\"'\s,}\]]+)",
    ):
        for match in re.finditer(pattern, stripped):
            values.append(match.group(1))
    return values


def command_scope_path_values(tokens: list[str], *, depth: int = 0) -> list[str]:
    if not tokens:
        return []
    if depth > PROCESS_PARSE_DEPTH_LIMIT:
        return []
    tokens = strip_shell_redirections(shell_normalized_tokens(tokens))
    if not tokens:
        return []
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index >= len(tokens):
        return []
    if assignment_index:
        return command_scope_path_values(tokens[assignment_index:], depth=depth + 1)
    segments = shell_command_segments(tokens)
    if segments:
        values: list[str] = []
        for segment in segments:
            values.extend(command_scope_path_values(segment, depth=depth + 1))
        return values
    values: list[str] = []
    for index, token in enumerate(tokens):
        if token in {"--manifest-path", "--target-dir"} and index + 1 < len(tokens):
            substitution = shell_command_substitution_at(tokens, index + 1)
            if substitution is not None:
                values.extend(command_substitution_path_values(substitution[0]))
            else:
                values.append(tokens[index + 1])
        elif token.startswith("--manifest-path=") or token.startswith("--target-dir="):
            values.append(token.split("=", 1)[1])
        elif token == "--config" and index + 1 < len(tokens):
            values.extend(cargo_config_scope_path_values(tokens[index + 1]))
        elif token.startswith("--config="):
            values.extend(cargo_config_scope_path_values(token.split("=", 1)[1]))
    executable = basename_token(tokens[0])
    wrapped_tokens = process_wrapper_tokens(tokens)
    if wrapped_tokens is not None:
        values.extend(command_scope_path_values(wrapped_tokens, depth=depth + 1))
    elif executable == "eval":
        eval_index = 1
        if eval_index < len(tokens) and tokens[eval_index] == "--":
            eval_index += 1
        values.extend(command_scope_path_values(command_tokens(" ".join(tokens[eval_index:])), depth=depth + 1))
    elif executable in ("bash", "dash", "fish", "sh", "zsh"):
        command = shell_command(tokens)
        if command is not None:
            values.extend(command_scope_path_values(command_tokens(command), depth=depth + 1))
    elif executable.startswith("python"):
        for payload in python_inline_command_payloads(tokens):
            values.extend(command_scope_path_values(command_tokens(payload), depth=depth + 1))
    return values


def command_substitution_path_values(tokens: list[str]) -> list[str]:
    values: list[str] = []
    for token in tokens:
        if token in SHELL_COMMAND_BOUNDARIES or token in {"echo", "printf"}:
            continue
        if "/" in token or token.endswith((".toml", ".json")):
            values.append(token)
    return values


def command_references_scope_path(command: str, cwd: pathlib.Path | None, scopes: tuple[pathlib.Path, ...]) -> bool:
    if cwd is None:
        return False
    for value in command_scope_path_values(command_tokens(command)):
        path = pathlib.Path(value)
        if not path.is_absolute():
            path = cwd / path
        try:
            resolved = path.resolve()
        except (OSError, RuntimeError):
            resolved = path
        if any(path_is_or_inside(resolved, scope) for scope in scopes):
            return True
    return False


def ps_process_entry(line: str) -> tuple[int, int | None, str] | None:
    stripped = line.strip()
    pid_text, _, rest = stripped.partition(" ")
    if not pid_text.isdigit():
        return None
    rest = rest.lstrip()
    ppid_text, _, command = rest.partition(" ")
    if ppid_text.isdigit() and command:
        return int(pid_text), int(ppid_text), command
    return int(pid_text), None, rest


def current_process_family_pids(entries: list[tuple[int, int | None, str]], current_pid: int) -> set[int]:
    ignored = {current_pid}
    parent = os.getppid()
    if parent > 0:
        ignored.add(parent)
    parent_by_pid = {pid: ppid for pid, ppid, _command in entries if ppid is not None}
    cursor = parent
    for _ in range(64):
        if cursor <= 0:
            break
        next_parent = parent_by_pid.get(cursor)
        if next_parent is None or next_parent in ignored:
            break
        ignored.add(next_parent)
        cursor = next_parent
    return ignored


def active_related_processes(repo: pathlib.Path, target: pathlib.Path, policy: dict[str, Any]) -> list[dict[str, Any]]:
    patterns = active_process_patterns(policy)
    if not patterns:
        return []
    result = subprocess.run(
        ["ps", "-ww", "-ax", "-o", "pid=", "-o", "ppid=", "-o", "command="],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise ProcessVisibilityError("unable to inspect active processes")
    current_pid = os.getpid()
    entries = [entry for line in result.stdout.splitlines() if (entry := ps_process_entry(line)) is not None]
    ignored_pids = current_process_family_pids(entries, current_pid)
    related: list[dict[str, Any]] = []
    repo_scope = repo.resolve()
    target_scope = target.resolve()
    path_scopes = (repo_scope, target_scope)
    scope_texts = {str(repo), str(target), str(repo_scope), str(target_scope)}
    unscoped_match = False
    for pid, _ppid, command in entries:
        if pid in ignored_pids:
            continue
        command_matches_scope = any(scope_text in command for scope_text in scope_texts)
        matched_pattern = matching_process_pattern(command, patterns)
        may_launch_renamed_cargo = matched_pattern is None and command_may_be_renamed_cargo(command)
        may_launch_rust = matched_pattern is None and (command_may_launch_rust(command) or may_launch_renamed_cargo)
        may_launch_build = matched_pattern is None and not may_launch_rust and command_may_launch_build(command)
        cwd: pathlib.Path | None = None
        cwd_sampled = False
        if not command_matches_scope and (matched_pattern is not None or may_launch_rust or may_launch_build):
            cwd = process_cwd(pid)
            cwd_sampled = True
            command_matches_scope = command_references_scope_path(command, cwd, path_scopes)
        if matched_pattern is None and not may_launch_rust and not may_launch_build and not command_matches_scope:
            continue
        if command_matches_scope:
            if matched_pattern is None:
                if may_launch_rust:
                    matched_pattern = (
                        "renamed Cargo launch command"
                        if may_launch_renamed_cargo
                        else "unclassified Rust launch command"
                    )
                elif may_launch_build:
                    matched_pattern = "unclassified build launch command"
                else:
                    continue
            entry = {
                "command": command,
                "pid": pid,
                "reason": f"matched {matched_pattern} and referenced repo or target",
            }
            if cwd is not None:
                entry["cwd"] = str(cwd)
            related.append(entry)
            continue
        if not cwd_sampled:
            cwd = process_cwd(pid)
            cwd_sampled = True
        cwd_matches_scope = cwd is not None and (
            path_is_or_inside(cwd, repo_scope) or path_is_or_inside(cwd, target_scope)
        )
        if matched_pattern is None:
            if cwd_matches_scope and may_launch_rust:
                matched_pattern = (
                    "renamed Cargo launch command"
                    if may_launch_renamed_cargo
                    else "unclassified Rust launch command"
                )
            elif cwd_matches_scope and may_launch_build:
                matched_pattern = "unclassified build launch command"
            elif may_launch_rust and cwd is None:
                unscoped_match = True
                continue
            else:
                continue
        if not command_matches_scope and not cwd_matches_scope:
            if cwd is not None:
                continue
            unscoped_match = True
            continue
        entry = {
            "command": command,
            "pid": pid,
            "reason": f"matched {matched_pattern} and referenced repo or target",
        }
        if cwd is not None:
            entry["cwd"] = str(cwd)
        related.append(entry)
    if unscoped_match:
        raise ProcessVisibilityError("matching process found without repo or target evidence")
    return related


def is_direct_child(path: pathlib.Path, parent: pathlib.Path) -> bool:
    try:
        relative = path.relative_to(parent)
    except ValueError:
        return False
    return len(relative.parts) == 1


def remove_cache_candidate(entry: dict[str, Any], target: pathlib.Path) -> None:
    path = pathlib.Path(entry["path"])
    if path == target or not is_direct_child(path, target):
        raise PolicyError("refusing to remove non-child cache path")
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        try:
            path.unlink()
        except FileNotFoundError:
            pass


def active_process_refusal_payload(repo: pathlib.Path, target: pathlib.Path, policy: dict[str, Any]) -> dict[str, Any] | None:
    try:
        active = active_related_processes(repo, target, policy)
    except ProcessVisibilityError as exc:
        return {
            "candidates": [],
            "dry_run": False,
            "reclaimable_bytes": 0,
            "refusal_code": "insufficient_process_visibility",
            "refusal_reason": str(exc),
            "refused": True,
            "target_dir": str(target),
        }
    if active:
        return {
            "active_processes": active,
            "candidates": [],
            "dry_run": False,
            "reclaimable_bytes": 0,
            "refusal_code": "active_process",
            "refusal_reason": "active related Rust verification process detected",
            "refused": True,
            "target_dir": str(target),
        }
    return None


def cache_prune_payload(repo: pathlib.Path, *, dry_run: bool) -> dict[str, Any]:
    policy = load_policy(repo)
    validate_cache_policy(policy)
    lock_context = cache_lock(policy, exclusive=True) if not dry_run else contextlib.nullcontext()
    with lock_context:
        if not dry_run:
            target = target_dir(repo, policy)
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return refusal
        status = cache_status_payload(repo)
        if not dry_run:
            refusal = active_process_refusal_payload(repo, pathlib.Path(status["target_dir"]), policy)
            if refusal is not None:
                return refusal
        candidates: list[dict[str, Any]] = []
        reclaimable_bytes = 0
        now = time.time()
        for subtree in status["subtrees"]:
            candidate, reason = is_prune_candidate(subtree, policy, now=now, pressure=bool(status["pressure"]))
            if not candidate:
                continue
            entry = dict(subtree)
            entry["reason"] = reason
            candidates.append(entry)
            reclaimable_bytes += int(entry["bytes"])
        removed: list[dict[str, Any]] = []
        if not dry_run:
            target = pathlib.Path(status["target_dir"])
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return refusal
            for entry in candidates:
                remove_cache_candidate(entry, target)
                removed.append(entry)
        return {
            "candidates": candidates,
            "dry_run": dry_run,
            "pressure": status["pressure"],
            "pressure_reasons": status["pressure_reasons"],
            "reclaimable_bytes": reclaimable_bytes,
            "removed": removed,
            "refused": False,
            "target_dir": status["target_dir"],
        }


def refusal_payload(*, code: str, reason: str, dry_run: bool, target: str | None = None) -> dict[str, Any]:
    return {
        "candidates": [],
        "dry_run": dry_run,
        "reclaimable_bytes": 0,
        "refusal_code": code,
        "refusal_reason": reason,
        "refused": True,
        "target_dir": target,
    }


def disk_preflight_refusal_payload(repo: pathlib.Path, policy: dict[str, Any]) -> dict[str, Any] | None:
    try:
        status = cache_status_payload(repo)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        try:
            target = str(target_dir(repo, policy))
        except (KeyError, OSError, PolicyError):
            target = None
        return refusal_payload(
            code="preflight_error",
            reason=f"unable to inspect disk preflight: {exc}",
            dry_run=False,
            target=target,
        )
    if not status.get("pressure"):
        return None
    target = str(status["target_dir"])
    return {
        "candidates": [],
        "dry_run": False,
        "filesystem": status["filesystem"],
        "legacy_target_dir": str(repo / "target"),
        "managed_target_dir": target,
        "pressure_reasons": status["pressure_reasons"],
        "reclaimable_bytes": 0,
        "refusal_code": "disk_pressure",
        "refusal_reason": "managed Rust command refused before execution because free disk/cache pressure failed preflight",
        "refused": True,
        "target_dir": target,
        "thresholds": status["thresholds"],
    }


def clean_preflight_refusal_payload(
    repo: pathlib.Path,
    policy: dict[str, Any],
    target: pathlib.Path | None = None,
) -> tuple[pathlib.Path | None, dict[str, Any] | None]:
    try:
        inspected_target = target if target is not None else target_dir(repo, policy)
        refusal = active_process_refusal_payload(repo, inspected_target, policy)
    except (KeyError, OSError, PolicyError, FileNotFoundError) as exc:
        return target, refusal_payload(
            code="preflight_error",
            reason=f"unable to inspect managed cargo clean preflight: {exc}",
            dry_run=False,
            target=str(target) if target is not None else None,
        )
    return inspected_target, refusal


def print_refusal(payload: dict[str, Any]) -> int:
    print(json.dumps(payload, sort_keys=True), file=sys.stderr)
    return 2


CARGO_DISK_PREFLIGHT_SUBCOMMANDS = frozenset(
    {"bench", "build", "check", "clippy", "doc", "fetch", "install", "nextest", "run", "rustc", "test", "zigbuild"}
)
CARGO_PROCESS_SUBCOMMANDS = CARGO_DISK_PREFLIGHT_SUBCOMMANDS | {"clean", "fmt"}
CARGO_ALIAS_SUBCOMMANDS = {"b", "c", "d", "r", "t"}
CARGO_CONFIG_RELATIVE_PATHS = (pathlib.Path(".cargo/config.toml"), pathlib.Path(".cargo/config"))
CARGO_HOME_CONFIG_NAMES = ("config.toml", "config")


def cargo_args_need_disk_preflight(cargo_args: list[str]) -> bool:
    return cargo_subcommand(cargo_args) in CARGO_DISK_PREFLIGHT_SUBCOMMANDS


def local_compile_allowed(policy: dict[str, Any]) -> bool:
    local_policy = policy["local_compile_policy"]
    allowed_ci_env = local_policy["allowed_ci_env"]
    break_glass_env = local_policy["break_glass_env"]
    return os.environ.get(allowed_ci_env) == "true" or os.environ.get(break_glass_env) == "1"


def local_compile_refusal_payload(
    repo: pathlib.Path,
    policy: dict[str, Any],
    *,
    command_kind: str,
    command_name: str,
) -> dict[str, Any] | None:
    if local_compile_allowed(policy):
        return None
    return {
        "break_glass": "BOLT_ALLOW_LOCAL_RUST=1 is operator-only and still runs disk preflight",
        "candidates": [],
        "command_kind": command_kind,
        "command_name": command_name,
        "dry_run": False,
        "next_steps": [
            "for targeted Rust debugging after cheap local checks: run: just rust-probe suggest",
            "then commit and push the branch before running the smallest suggested just rust-probe command",
            "for full remote feedback on a draft PR: run: just verify-remote",
            "for merge proof: commit local changes",
            "for merge proof: push the branch",
            (
                "for merge proof: mark the PR ready, then run: just verify-remote "
                "to wait for the required PR gate, or use the merge-queue gate"
            ),
        ],
        "reclaimable_bytes": 0,
        "refusal_code": "local_compile_disabled",
        "refusal_reason": (
            "local compile-heavy Rust verification is disabled by default because concurrent "
            "agent sessions share the local managed Cargo target/cache"
        ),
        "refused": True,
        "target_dir": str(target_dir(repo, policy)),
    }


def local_compile_refusal_for_cargo_args(
    repo: pathlib.Path,
    policy: dict[str, Any],
    cargo_args: list[str],
) -> dict[str, Any] | None:
    subcommand = cargo_subcommand(cargo_args)
    if subcommand is None:
        return None
    refused = set(policy["local_compile_policy"]["refused_cargo_subcommands"])
    if subcommand not in refused:
        return None
    return local_compile_refusal_payload(repo, policy, command_kind="cargo", command_name=subcommand)


def local_compile_refusal_for_managed_command(
    repo: pathlib.Path,
    policy: dict[str, Any],
    command: str,
) -> dict[str, Any] | None:
    refused = set(policy["local_compile_policy"]["refused_managed_commands"])
    if command not in refused:
        return None
    return local_compile_refusal_payload(repo, policy, command_kind="managed", command_name=command)


def cargo_args_need_exclusive_cache_lock(cargo_args: list[str]) -> bool:
    subcommand = cargo_subcommand_with_index(cargo_args)
    if subcommand is None:
        return False
    index, command = subcommand
    if command != "nextest":
        return False
    nextest_subcommand = nextest_subcommand_with_index(cargo_args[index + 1 :])
    if nextest_subcommand is None or nextest_subcommand[1] != "run":
        return False
    nextest_index = index + 1 + nextest_subcommand[0]
    tail = cargo_args[nextest_index + 1 :]
    if "--" in tail:
        tail = tail[: tail.index("--")]
    has_archive_file = any(token == "--archive-file" or token.startswith("--archive-file=") for token in tail)
    has_extract = any(
        token in {"--extract-to", "--extract-overwrite"} or token.startswith("--extract-to=")
        for token in tail
    )
    return has_archive_file and has_extract


def run_args_need_exclusive_cache_lock(command: str, command_args: list[str], *, test_separator: bool) -> bool:
    if command != "test" or test_separator:
        return False
    return cargo_args_need_exclusive_cache_lock(["nextest", "run", "--locked", *command_args])


def repo_cargo_aliases(repo: pathlib.Path) -> set[str]:
    aliases: set[str] = set()
    config_paths = [(repo / relative_path, relative_path) for relative_path in CARGO_CONFIG_RELATIVE_PATHS]
    cargo_home = os.environ.get("CARGO_HOME")
    cargo_home_path = pathlib.Path(cargo_home).expanduser() if cargo_home else pathlib.Path.home() / ".cargo"
    config_paths.extend((cargo_home_path / name, pathlib.Path(f"$CARGO_HOME/{name}")) for name in CARGO_HOME_CONFIG_NAMES)
    for path, display_path in config_paths:
        if not path.exists():
            continue
        try:
            if _toml is None:
                config = parse_minimal_toml(path)
            else:
                with path.open("rb") as handle:
                    config = _toml.load(handle)
        except (OSError, PolicyError, _toml.TOMLDecodeError if _toml is not None else ValueError) as exc:
            raise PolicyError(f"unable to inspect Cargo alias config {display_path}: {exc}") from exc
        alias_table = config.get("alias")
        if isinstance(alias_table, dict):
            aliases.update(str(name) for name in alias_table)
    return aliases


def cargo_alias_subcommand(cargo_args: list[str], repo: pathlib.Path | None = None) -> str | None:
    subcommand = cargo_subcommand(cargo_args)
    if subcommand in CARGO_ALIAS_SUBCOMMANDS:
        return subcommand
    if repo is not None and subcommand is not None and subcommand in repo_cargo_aliases(repo):
        return subcommand
    return None


def cargo_target_routing_override(cargo_args: list[str]) -> str | None:
    value_options = {"--artifact-dir", "--out-dir", "--root", "--target-dir"}
    scan_args = cargo_args_for_target_routing_scan(cargo_args)
    for index, token in enumerate(scan_args):
        if token in value_options:
            return token
        for option in value_options:
            if token.startswith(f"{option}="):
                return option
        if token == "--config" and index + 1 < len(scan_args):
            override = cargo_config_storage_override(scan_args[index + 1])
            if override is not None:
                return f"--config {override}"
        if token.startswith("--config="):
            override = cargo_config_storage_override(token.split("=", 1)[1])
            if override is not None:
                return f"--config={override}"
    return None


def decode_toml_unicode_escapes(value: str) -> str:
    def replace(match: re.Match[str]) -> str:
        digits = match.group(1) or match.group(2)
        return chr(int(digits, 16))

    return re.sub(r"\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})", lambda match: replace(match), value)


def cargo_config_looks_like_path(config: str) -> bool:
    stripped = config.strip()
    if not stripped:
        return False
    if stripped.startswith(("[", "{")):
        return False
    if "=" not in stripped:
        return True
    key_prefix = stripped.split("=", 1)[0]
    return "/" in key_prefix or "\\" in key_prefix or key_prefix.endswith(".toml")


def cargo_config_storage_override(config: str) -> str | None:
    if cargo_config_looks_like_path(config):
        return "config-file"
    scan_config = decode_toml_unicode_escapes(config)
    if "target-dir" in scan_config and ("build" in scan_config or "[build]" in scan_config):
        return "build.target-dir"
    if "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config):
        return "build.rustflags"
    return None


def target_routing_refusal_payload(repo: pathlib.Path, policy: dict[str, Any], option: str) -> dict[str, Any]:
    target = str(target_dir(repo, policy))
    return {
        "candidates": [],
        "dry_run": False,
        "managed_target_dir": target,
        "reclaimable_bytes": 0,
        "refusal_code": "target_routing_override",
        "refusal_reason": f"managed Cargo refused target/output routing override: {option}",
        "refused": True,
        "target_dir": target,
    }


def cargo_alias_refusal_payload(repo: pathlib.Path, policy: dict[str, Any], alias: str) -> dict[str, Any]:
    target = str(target_dir(repo, policy))
    return {
        "candidates": [],
        "dry_run": False,
        "managed_target_dir": target,
        "reclaimable_bytes": 0,
        "refusal_code": "cargo_alias_subcommand",
        "refusal_reason": f"managed Cargo refused alias subcommand: {alias}",
        "refused": True,
        "target_dir": target,
    }


def command_args(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
    return subprocess.run(argv, cwd=repo, env=env, check=False).returncode


def run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def command_error(argv: list[str], result: subprocess.CompletedProcess[str]) -> str:
    stderr = result.stderr.strip()
    if stderr:
        return stderr
    stdout = result.stdout.strip()
    if stdout:
        return stdout
    return f"{pathlib.Path(argv[0]).name} exited {result.returncode}"


def load_json_command(argv: list[str], *, repo: pathlib.Path) -> tuple[Any | None, str | None]:
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, f"{pathlib.Path(argv[0]).name} is required for remote verification"
    except OSError as exc:
        return None, f"{pathlib.Path(argv[0]).name} could not run: {exc}"
    if result.returncode != 0:
        return None, command_error(argv, result)
    try:
        return json.loads(result.stdout), None
    except json.JSONDecodeError as exc:
        return None, f"{pathlib.Path(argv[0]).name} returned invalid JSON: {exc}"


def remote_verification_policy(policy: dict[str, Any]) -> dict[str, int]:
    raw = policy.get("remote_verification")
    if not isinstance(raw, dict):
        raise PolicyError("remote_verification table is required for verify-remote")
    validate_remote_verification_policy(policy)
    return {
        "poll_interval_seconds": int(raw["poll_interval_seconds"]),
        "checks_appear_timeout_seconds": int(raw["checks_appear_timeout_seconds"]),
        "overall_timeout_seconds": int(raw["overall_timeout_seconds"]),
        "diagnostic_log_max_lines": int(raw["diagnostic_log_max_lines"]),
        "diagnostic_log_max_bytes": int(raw["diagnostic_log_max_bytes"]),
        "diagnostic_unavailable_notice_interval_polls": int(
            raw["diagnostic_unavailable_notice_interval_polls"]
        ),
    }


def remote_probe_policy(policy: dict[str, Any]) -> dict[str, Any]:
    raw = policy.get("remote_probe")
    if not isinstance(raw, dict):
        raise PolicyError("remote_probe table is required for rust-probe")
    validate_remote_probe_policy(policy)
    separate_workspaces = {
        str(workspace["path"]): {
            "message": str(workspace["message"]),
            "commands": list(workspace["commands"]),
        }
        for workspace in raw["separate_workspaces"].values()
    }
    return {
        "workflow_name": str(raw["workflow_name"]),
        "workflow_path": str(raw["workflow_path"]),
        "poll_interval_seconds": int(raw["poll_interval_seconds"]),
        "appearance_timeout_seconds": int(raw["appearance_timeout_seconds"]),
        "overall_timeout_seconds": int(raw["overall_timeout_seconds"]),
        "active_run_limit": int(raw["active_run_limit"]),
        "workflow_runs_per_page": int(raw["workflow_runs_per_page"]),
        "guard_timeout_minutes": int(raw["guard_timeout_minutes"]),
        "allowed_runner_tiers": list(raw["allowed_runner_tiers"]),
        "mode_runner_tiers": dict(raw["mode_runner_tiers"]),
        "workflow_timeouts": dict(raw["workflow_timeouts"]),
        "suggest_base_ref": str(raw["suggest_base_ref"]),
        "separate_workspaces": separate_workspaces,
    }


def verify_remote_fail(message: str) -> int:
    print(f"ERROR: {message}", file=sys.stderr)
    return 2


def git_output(repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
    argv = ["git", *args]
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, "git is required for remote verification"
    if result.returncode != 0:
        return None, command_error(argv, result)
    return result.stdout.strip(), None


def current_branch(repo: pathlib.Path) -> tuple[str | None, str | None]:
    return git_output(repo, "branch", "--show-current")


def live_upstream_head(repo: pathlib.Path, branch: str, *, command_name: str = "verify-remote") -> tuple[str | None, str | None]:
    remote, error = git_output(repo, "config", f"branch.{branch}.remote")
    if error is not None or not remote:
        return None, None
    merge_ref, error = git_output(repo, "config", f"branch.{branch}.merge")
    if error is not None or not merge_ref:
        return None, None
    if not merge_ref.startswith("refs/heads/"):
        return None, f"{command_name} requires upstream to be a branch, got {merge_ref}"
    upstream_branch = merge_ref.removeprefix("refs/heads/")
    refs, error = git_output(repo, "ls-remote", "--heads", remote, upstream_branch)
    if error is not None:
        return None, error
    for line in refs.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1] == f"refs/heads/{upstream_branch}":
            return fields[0], None
    return None, None


def upstream_branch_name(repo: pathlib.Path, branch: str, *, command_name: str) -> tuple[str | None, str | None]:
    merge_ref, error = git_output(repo, "config", f"branch.{branch}.merge")
    if error is not None or not merge_ref:
        return None, f"{command_name} requires pushed HEAD with an upstream"
    if not merge_ref.startswith("refs/heads/"):
        return None, f"{command_name} requires upstream to be a branch, got {merge_ref}"
    return merge_ref.removeprefix("refs/heads/"), None


def ensure_clean_pushed_head_preconditions(
    repo: pathlib.Path,
    *,
    command_name: str,
) -> tuple[str | None, str | None, str | None]:
    status, error = git_output(repo, "status", "--porcelain", "--untracked-files=normal")
    if error is not None:
        return None, None, error
    if status:
        return None, None, f"{command_name} requires a clean worktree, including untracked files"
    head, error = git_output(repo, "rev-parse", "HEAD")
    if error is not None:
        return None, None, error
    branch, error = current_branch(repo)
    if error is not None:
        return None, None, error
    if not branch:
        return None, None, f"{command_name} requires a named branch"
    upstream, error = live_upstream_head(repo, branch, command_name=command_name)
    if error is not None:
        return None, None, error
    if upstream is None:
        hint = "git push -u origin HEAD"
        return None, None, f"{command_name} requires pushed HEAD with an upstream; run: {hint}"
    if upstream != head:
        return None, None, f"{command_name} requires HEAD to be pushed to the upstream branch"
    return head, branch, None


def ensure_verify_remote_preconditions(repo: pathlib.Path) -> tuple[str | None, str | None, str | None]:
    return ensure_clean_pushed_head_preconditions(repo, command_name="verify-remote")


def ensure_rust_probe_preconditions(repo: pathlib.Path) -> tuple[str | None, str | None, str | None]:
    head, branch, error = ensure_clean_pushed_head_preconditions(repo, command_name="rust-probe")
    if error is not None or head is None or branch is None:
        return head, branch, error
    upstream_branch, error = upstream_branch_name(repo, branch, command_name="rust-probe")
    if error is not None or upstream_branch is None:
        return None, None, error or "rust-probe requires pushed HEAD with an upstream"
    return head, upstream_branch, None


def pr_create_hint(branch: str) -> str:
    return f"gh pr create --draft --fill --head {shlex.quote(branch)}"


def pr_for_current_branch(repo: pathlib.Path, branch: str) -> tuple[dict[str, Any] | None, str | None]:
    payload, error = load_json_command(
        [
            "gh",
            "pr",
            "view",
            "--json",
            "number,url,headRefOid,headRefName,state,isDraft,headRepositoryOwner,headRepository",
        ],
        repo=repo,
    )
    if error is not None:
        lowered = error.lower()
        if "no pull requests found" in lowered or "no pull request found" in lowered:
            return None, f"verify-remote requires an open or draft PR for this branch; run: {pr_create_hint(branch)}"
        return None, f"verify-remote could not inspect pull request state: {error}"
    if not isinstance(payload, dict):
        return None, "gh pr view returned an unexpected payload"
    if payload.get("state") != "OPEN":
        return None, f"PR for this branch is {payload.get('state') or 'not open'}; start from main instead of stale branch"
    return payload, None


def pr_for_exact_head(
    repo: pathlib.Path,
    branch: str,
    head: str,
    *,
    during_watch: bool,
) -> tuple[dict[str, Any] | None, str | None]:
    pr, error = pr_for_current_branch(repo, branch)
    if error is not None or pr is None:
        return None, error or "unable to inspect pull request"
    if pr.get("headRefOid") != head:
        if during_watch:
            return (
                None,
                f"PR branch advanced during watch: headRefOid {pr.get('headRefOid')} no longer matches "
                f"local HEAD {head}; fetch the branch and rerun verify-remote",
            )
        return None, f"PR headRefOid {pr.get('headRefOid')} does not match local HEAD {head}; push the current branch"
    return pr, None


def pr_checks(repo: pathlib.Path) -> tuple[list[dict[str, Any]] | None, str | None]:
    argv = ["gh", "pr", "checks", "--json", "name,bucket,state,link,workflow"]
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, "gh is required for remote verification"
    if result.returncode not in {0, 8}:
        return None, command_error(argv, result)
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        return None, f"gh returned invalid PR checks JSON: {exc}"
    if not isinstance(payload, list) or not all(isinstance(item, dict) for item in payload):
        return None, "gh pr checks returned an unexpected payload"
    return payload, None


def check_summary(check: dict[str, Any]) -> str:
    name = str(check.get("name") or "<unnamed>")
    bucket = str(check.get("bucket") or check.get("state") or "unknown")
    link = str(check.get("link") or "")
    return f"{name} [{bucket}]" + (f" {link}" if link else "")


def github_actions_output_safe_check_name(value: str) -> bool:
    return (
        value == value.strip()
        and "${{" not in value
        and "}}" not in value
        and all(char not in "\r\n" and 32 <= ord(char) < 127 for char in value)
    )


def gate_name_collision_errors(gate_names: dict[str, str]) -> list[str]:
    errors: list[str] = []
    keys = (
        "gate_required",
        "backtester_required",
        "gate_iteration",
        "backtester_iteration",
        "gate_dispatch_full",
        "backtester_dispatch_full",
    )
    seen: dict[str, str] = {}
    for key in keys:
        value = gate_names.get(key)
        if value is None:
            continue
        previous = seen.get(value)
        if previous is not None:
            errors.append(f"ci_provenance.gate_names.{key} must not equal {previous}")
        else:
            seen[value] = key
    return errors


def verify_remote_head_current_or_fail(repo: pathlib.Path, branch: str, head: str) -> int | None:
    _pr, error = pr_for_exact_head(repo, branch, head, during_watch=True)
    if error is not None:
        return verify_remote_fail(error)
    return None


def ci_provenance_dispatch_config(repo: pathlib.Path) -> tuple[dict[str, Any] | None, str | None]:
    path = repo / CI_RUNNERS_RELATIVE_PATH
    if not path.exists():
        return None, f"{CI_RUNNERS_RELATIVE_PATH} is required for verify-remote dispatch"
    try:
        data = load_toml(path)
    except (OSError, PolicyError) as exc:
        return None, str(exc)
    provenance = data.get("ci_provenance")
    if not isinstance(provenance, dict):
        return None, "ci_provenance table is required for verify-remote dispatch"
    dispatch = provenance.get("dispatch")
    if not isinstance(dispatch, dict):
        return None, "ci_provenance.dispatch table is required for verify-remote dispatch"
    workflow_name = provenance.get("workflow_name")
    workflow_path = provenance.get("workflow_path")
    workflow_input = dispatch.get("workflow_input")
    run_name_full = dispatch.get("run_name_full")
    run_name_iteration = dispatch.get("run_name_iteration")
    proof_gate_job = dispatch.get("proof_gate_job")
    gate_names = provenance.get("gate_names")
    if not isinstance(workflow_name, str) or not workflow_name:
        return None, "ci_provenance.workflow_name must be a non-empty string"
    if not isinstance(workflow_path, str) or not workflow_path:
        return None, "ci_provenance.workflow_path must be a non-empty string"
    if not isinstance(workflow_input, str) or not SAFE_IDENTIFIER_RE.match(workflow_input):
        return None, "ci_provenance.dispatch.workflow_input must be a safe identifier"
    if not isinstance(run_name_full, str) or not run_name_full:
        return None, "ci_provenance.dispatch.run_name_full must be a non-empty string"
    if not isinstance(run_name_iteration, str) or not run_name_iteration:
        return None, "ci_provenance.dispatch.run_name_iteration must be a non-empty string"
    if run_name_full == run_name_iteration:
        return None, "ci_provenance.dispatch run_name_full and run_name_iteration must differ"
    if not isinstance(proof_gate_job, str) or not proof_gate_job:
        return None, "ci_provenance.dispatch.proof_gate_job must be a non-empty string"
    if not github_actions_output_safe_check_name(proof_gate_job):
        return None, "ci_provenance.dispatch.proof_gate_job must be a GitHub Actions output-safe check name"
    if not isinstance(gate_names, dict):
        return None, "ci_provenance.gate_names table is required for verify-remote dispatch"
    configured_gate_names: dict[str, str] = {}
    for key in GATE_NAME_KEYS:
        if key not in gate_names:
            continue
        value = gate_names.get(key)
        if not isinstance(value, str) or not value:
            return None, f"ci_provenance.gate_names.{key} must be a non-empty string"
        if not github_actions_output_safe_check_name(value):
            return None, f"ci_provenance.gate_names.{key} must be a GitHub Actions output-safe check name"
        configured_gate_names[key] = value
    gate_required = configured_gate_names.get("gate_required")
    if gate_required is not None and proof_gate_job != gate_required:
        return None, "ci_provenance.dispatch.proof_gate_job must match required gate name"
    gate_name_errors = gate_name_collision_errors(configured_gate_names)
    if gate_name_errors:
        return None, "; ".join(gate_name_errors)
    dispatch_full_gate_job = gate_names.get("gate_dispatch_full")
    if not isinstance(dispatch_full_gate_job, str) or not dispatch_full_gate_job:
        return None, "ci_provenance.gate_names.gate_dispatch_full must be a non-empty string"
    api_limits = provenance.get("api_limits")
    run_limit = None
    if isinstance(api_limits, dict):
        raw_limit = api_limits.get("workflow_runs_per_page")
        if raw_limit is not None:
            if not isinstance(raw_limit, int) or isinstance(raw_limit, bool) or raw_limit <= 0:
                return None, "ci_provenance.api_limits.workflow_runs_per_page must be a positive integer"
            run_limit = raw_limit
    return {
        "workflow_name": workflow_name,
        "workflow_path": workflow_path,
        "workflow_input": workflow_input,
        "run_name_full": run_name_full,
        "run_name_iteration": run_name_iteration,
        "proof_gate_job": proof_gate_job,
        "dispatch_full_gate_job": dispatch_full_gate_job,
        "workflow_runs_per_page": run_limit,
    }, None


def repository_identity(repo: pathlib.Path) -> tuple[tuple[str, str] | None, str | None]:
    payload, error = load_json_command(["gh", "repo", "view", "--json", "name,owner"], repo=repo)
    if error is not None:
        return None, f"verify-remote could not inspect repository identity: {error}"
    if not isinstance(payload, dict):
        return None, "gh repo view returned an unexpected payload"
    name = payload.get("name")
    owner = payload.get("owner")
    owner_login = owner.get("login") if isinstance(owner, dict) else owner
    if not isinstance(name, str) or not isinstance(owner_login, str):
        return None, "gh repo view returned incomplete repository identity"
    return (owner_login, name), None


def repository_name(value: Any) -> str | None:
    if isinstance(value, dict):
        name = value.get("name")
        if isinstance(name, str):
            return name
        name_with_owner = value.get("nameWithOwner")
        if isinstance(name_with_owner, str) and "/" in name_with_owner:
            return name_with_owner.rsplit("/", 1)[1]
    if isinstance(value, str):
        return value.rsplit("/", 1)[-1]
    return None


def repository_owner(value: Any) -> str | None:
    if isinstance(value, dict):
        login = value.get("login")
        if isinstance(login, str):
            return login
        name_with_owner = value.get("nameWithOwner")
        if isinstance(name_with_owner, str) and "/" in name_with_owner:
            return name_with_owner.split("/", 1)[0]
    if isinstance(value, str):
        return value.split("/", 1)[0] if "/" in value else None
    return None


def draft_pr_is_fork(repo: pathlib.Path, pr: dict[str, Any]) -> tuple[bool | None, str | None]:
    current, error = repository_identity(repo)
    if error is not None or current is None:
        return None, error or "unable to inspect repository identity"
    current_owner, current_name = current
    head_owner = repository_owner(pr.get("headRepositoryOwner")) or repository_owner(pr.get("headRepository"))
    head_repo = repository_name(pr.get("headRepository"))
    if head_owner is None or head_repo is None:
        return None, "verify-remote could not determine whether the draft PR branch is in the upstream repository"
    return (head_owner, head_repo) != (current_owner, current_name), None


WORKFLOW_RUN_FIELDS = "attempt,databaseId,event,headSha,status,conclusion,createdAt,url,displayTitle"
ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
SECRET_ASSIGNMENT_RE = re.compile(
    r"(?i)\b("
    r"[A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY|SESSION_TOKEN|PRIVATE_KEY|MNEMONIC|SEED_PHRASE|WALLET_KEY|SIGNING_KEY|PASSPHRASE|CREDENTIAL)[A-Z0-9_]*"
    r")\b(\s*[:=]\s*)\S.*"
)
BEARER_RE = re.compile(r"(?i)\bAuthorization\s*:\s*Bearer\s+\S+")
FULL_CI_READY_EVENTS = {"pull_request"}
FULL_CI_DRAFT_EVENTS = {"workflow_dispatch"}


class RemoteFailureDiagnosticsState:
    def __init__(self) -> None:
        self.reported_job_ids: set[int] = set()
        self.unavailable_notice_polls: dict[int, int] = {}
        self.jobs_unavailable_notice_polls: dict[int, int] = {}


def workflow_run_list(
    repo: pathlib.Path,
    dispatch_config: dict[str, Any],
    branch: str,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    argv = [
        "gh",
        "run",
        "list",
        "--workflow",
        str(dispatch_config["workflow_name"]),
        "--branch",
        branch,
        "--json",
        WORKFLOW_RUN_FIELDS,
    ]
    run_limit = dispatch_config.get("workflow_runs_per_page")
    if isinstance(run_limit, int):
        argv.extend(["--limit", str(run_limit)])
    payload, error = load_json_command(argv, repo=repo)
    if error is not None:
        return None, f"verify-remote could not inspect workflow runs: {error}"
    if not isinstance(payload, list) or not all(isinstance(item, dict) for item in payload):
        return None, "gh run list returned an unexpected payload"
    return payload, None


def workflow_run_view(
    repo: pathlib.Path,
    run_id: int,
    *,
    command_name: str = "verify-remote",
) -> tuple[dict[str, Any] | None, str | None]:
    payload, error = load_json_command(
        ["gh", "run", "view", str(run_id), "--json", WORKFLOW_RUN_FIELDS],
        repo=repo,
    )
    if error is not None:
        return None, f"{command_name} could not inspect workflow run {run_id}: {error}"
    if not isinstance(payload, dict):
        return None, "gh run view returned an unexpected payload"
    return payload, None


def workflow_run_jobs(
    repo: pathlib.Path,
    run_id: int,
    attempt: int | None,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    argv = ["gh", "run", "view", str(run_id), "--json", "jobs"]
    if attempt is not None:
        argv.extend(["--attempt", str(attempt)])
    payload, error = load_json_command(argv, repo=repo)
    if error is not None:
        return None, f"verify-remote could not inspect workflow run {run_id} jobs: {error}"
    if not isinstance(payload, dict):
        return None, "gh run view returned an unexpected jobs payload"
    jobs = payload.get("jobs")
    if not isinstance(jobs, list) or not all(isinstance(item, dict) for item in jobs):
        return None, "gh run view returned an unexpected jobs payload"
    return jobs, None


def job_log_failed(repo: pathlib.Path, job_id: int) -> tuple[str | None, str | None]:
    argv = ["gh", "run", "view", "--job", str(job_id), "--log-failed"]
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, "gh is required for remote verification"
    if result.returncode != 0:
        return None, command_error(argv, result)
    if not ANSI_ESCAPE_RE.sub("", result.stdout).strip():
        return None, "failed job log is not available yet"
    return result.stdout, None


def run_created_at(run: dict[str, Any]) -> str:
    created_at = run.get("createdAt")
    return created_at if isinstance(created_at, str) else ""


def run_database_id(run: dict[str, Any]) -> int | None:
    database_id = run.get("databaseId")
    if isinstance(database_id, int) and not isinstance(database_id, bool):
        return database_id
    if isinstance(database_id, str) and database_id.isdigit():
        return int(database_id)
    return None


def run_attempt(run: dict[str, Any]) -> int | None:
    attempt = run.get("attempt")
    if isinstance(attempt, int) and not isinstance(attempt, bool) and attempt > 0:
        return attempt
    if isinstance(attempt, str) and attempt.isdecimal():
        value = int(attempt)
        if value > 0:
            return value
    return None


def run_display_title(run: dict[str, Any]) -> str:
    value = run.get("displayTitle")
    if isinstance(value, str) and value:
        return value
    value = run.get("display_title")
    return value if isinstance(value, str) else ""


def workflow_dispatch_run_is_full(run: dict[str, Any], dispatch_config: dict[str, Any]) -> bool:
    return run_display_title(run) == dispatch_config.get("run_name_full")


def job_database_id(job: dict[str, Any]) -> int | None:
    database_id = job.get("databaseId")
    if database_id is None:
        database_id = job.get("id")
    if isinstance(database_id, int) and not isinstance(database_id, bool):
        return database_id
    if isinstance(database_id, str) and database_id.isdecimal():
        return int(database_id)
    return None


def job_text(job: dict[str, Any], key: str) -> str | None:
    value = job.get(key)
    if isinstance(value, str) and value:
        return value
    return None


def failed_job_summary(job: dict[str, Any], job_id: int) -> str:
    name = job_text(job, "name") or f"job {job_id}"
    status = job_text(job, "status") or "unknown"
    conclusion = job_text(job, "conclusion") or "unknown"
    url = job_text(job, "url")
    summary = f"{name} [{status}/{conclusion}]"
    return summary + (f" {url}" if url else "")


def mask_obvious_secrets(line: str) -> str:
    line = SECRET_ASSIGNMENT_RE.sub(lambda match: f"{match.group(1)}{match.group(2)}<redacted>", line)
    return BEARER_RE.sub("Authorization: Bearer <redacted>", line)


def diagnostic_log_excerpt(text: str, *, max_lines: int, max_bytes: int) -> str:
    cleaned = ANSI_ESCAPE_RE.sub("", text)
    lines = [mask_obvious_secrets(line) for line in cleaned.splitlines()]
    excerpt = "\n".join(lines[-max_lines:])
    encoded = excerpt.encode("utf-8")
    if len(encoded) > max_bytes:
        excerpt = encoded[-max_bytes:].decode("utf-8", errors="replace")
    return excerpt.strip()


def emit_failed_job_diagnostics(
    *,
    repo: pathlib.Path,
    run: dict[str, Any],
    state: RemoteFailureDiagnosticsState,
    remote_policy: dict[str, int],
) -> bool:
    run_id = run_database_id(run)
    if run_id is None:
        print("CI failed-job diagnostics unavailable: workflow run databaseId missing", file=sys.stderr)
        return False
    attempt = run_attempt(run)
    if attempt is None:
        print("CI failed-job diagnostics unavailable: workflow run attempt missing", file=sys.stderr)
        return False
    jobs, error = workflow_run_jobs(repo, run_id, attempt)
    if error is not None or jobs is None:
        notice_interval = remote_policy["diagnostic_unavailable_notice_interval_polls"]
        poll_count = state.jobs_unavailable_notice_polls.get(run_id, 0) + 1
        state.jobs_unavailable_notice_polls[run_id] = poll_count
        if poll_count == 1 or poll_count % notice_interval == 0:
            print(f"CI failed-job diagnostics unavailable: {error or 'unable to inspect workflow run jobs'}", file=sys.stderr)
        return False
    state.jobs_unavailable_notice_polls.pop(run_id, None)
    notice_interval = remote_policy["diagnostic_unavailable_notice_interval_polls"]
    for job in jobs:
        if job_text(job, "status") != "completed" or job_text(job, "conclusion") != "failure":
            continue
        job_id = job_database_id(job)
        if job_id is None or job_id in state.reported_job_ids:
            continue
        log_text, log_error = job_log_failed(repo, job_id)
        excerpt = (
            diagnostic_log_excerpt(
                log_text,
                max_lines=remote_policy["diagnostic_log_max_lines"],
                max_bytes=remote_policy["diagnostic_log_max_bytes"],
            )
            if log_text is not None
            else ""
        )
        if not excerpt:
            poll_count = state.unavailable_notice_polls.get(job_id, 0) + 1
            state.unavailable_notice_polls[job_id] = poll_count
            if poll_count == 1 or poll_count % notice_interval == 0:
                print(f"CI failed job: {failed_job_summary(job, job_id)}", file=sys.stderr)
                print(f"  job_log=unavailable yet: {log_error or 'not available'}", file=sys.stderr)
            continue
        state.reported_job_ids.add(job_id)
        state.unavailable_notice_polls.pop(job_id, None)
        print(f"CI failed job: {failed_job_summary(job, job_id)}", file=sys.stderr)
        print(excerpt, file=sys.stderr)
    return True


def matching_full_ci_runs(
    runs: list[dict[str, Any]],
    *,
    head: str,
    events: set[str],
    dispatch_config: dict[str, Any] | None = None,
    created_at_floor: str | None = None,
    ignored_run_ids: set[int] | None = None,
) -> list[dict[str, Any]]:
    matching = []
    for run in runs:
        event = run.get("event")
        if run.get("headSha") != head or event not in events:
            continue
        if event == "workflow_dispatch":
            if dispatch_config is None or not workflow_dispatch_run_is_full(run, dispatch_config):
                continue
        run_id = run_database_id(run)
        if ignored_run_ids is not None and run_id in ignored_run_ids:
            continue
        if created_at_floor is not None and run_created_at(run) < created_at_floor:
            continue
        matching.append(run)
    return sorted(matching, key=run_created_at, reverse=True)


def workflow_run_state(run: dict[str, Any]) -> str:
    status = str(run.get("status") or "")
    if status != "completed":
        return "pending"
    conclusion = str(run.get("conclusion") or "")
    if conclusion == "success":
        return "pass"
    return "fail"


def workflow_run_summary(run: dict[str, Any]) -> str:
    run_id = run.get("databaseId") or "<unknown>"
    status = str(run.get("status") or "unknown")
    conclusion = str(run.get("conclusion") or "none")
    url = str(run.get("url") or "")
    return f"workflow run {run_id} [{status}/{conclusion}]" + (f" {url}" if url else "")


def run_ids(runs: list[dict[str, Any]]) -> set[int]:
    return {run_id for run in runs if (run_id := run_database_id(run)) is not None}


def dispatch_full_ci(
    repo: pathlib.Path,
    dispatch_config: dict[str, Any],
    branch: str,
) -> tuple[None, str | None]:
    field = f"{dispatch_config['workflow_input']}=true"
    argv = [
        "gh",
        "workflow",
        "run",
        str(dispatch_config["workflow_path"]),
        "--ref",
        branch,
        "-f",
        field,
    ]
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return None, "gh is required for remote verification"
    if result.returncode != 0:
        return None, f"verify-remote could not dispatch full CI: {command_error(argv, result)}"
    return None, None


def rust_probe_run_list(
    repo: pathlib.Path,
    remote_policy: dict[str, Any],
    *,
    branch: str | None = None,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    argv = [
        "gh",
        "run",
        "list",
        "--workflow",
        str(remote_policy["workflow_name"]),
        "--json",
        WORKFLOW_RUN_FIELDS,
        "--limit",
        str(remote_policy["workflow_runs_per_page"]),
    ]
    if branch is not None:
        argv.extend(["--branch", branch])
    payload, error = load_json_command(argv, repo=repo)
    if error is not None:
        return None, f"rust-probe could not inspect workflow runs: {error}"
    if not isinstance(payload, list) or not all(isinstance(item, dict) for item in payload):
        return None, "gh run list returned an unexpected payload"
    return payload, None


def rust_probe_active_run_count(repo: pathlib.Path, remote_policy: dict[str, Any]) -> tuple[int | None, str | None]:
    runs, error = rust_probe_run_list(repo, remote_policy)
    if error is not None or runs is None:
        return None, error or "unable to inspect Rust Probe workflow runs"
    return sum(1 for run in runs if str(run.get("status") or "") != "completed"), None


def dispatch_rust_probe(
    repo: pathlib.Path,
    remote_policy: dict[str, Any],
    *,
    branch: str,
    head: str,
    mode: str,
    test_target: str,
    test_name: str,
    runner_tier: str,
    job_timeout_minutes: int,
    probe_id: str,
) -> str | None:
    fields = {
        "runner_tier": runner_tier,
        "job_timeout_minutes": str(job_timeout_minutes),
        "ref": head,
        "expected_sha": head,
        "probe_id": probe_id,
        "mode": mode,
        "test_target": test_target,
        "test_name": test_name,
    }
    argv = [
        "gh",
        "workflow",
        "run",
        str(remote_policy["workflow_path"]),
        "--ref",
        branch,
    ]
    for key in RUST_PROBE_INPUT_KEYS:
        argv.extend(["-f", f"{key}={fields[key]}"])
    try:
        result = run_capture(argv, repo=repo)
    except FileNotFoundError:
        return "gh is required for rust-probe"
    if result.returncode != 0:
        return f"rust-probe could not dispatch workflow: {command_error(argv, result)}"
    return None


def new_probe_id() -> str:
    return f"rust-probe-{uuid.uuid4().hex}"


def rust_probe_validation_hint(message: str) -> str:
    return f"{message}\n\n{RUST_PROBE_HELP_EPILOG}"


def validate_rust_probe_selection(mode: str, test_target: str, test_name: str) -> str | None:
    target_regex = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.-]*$")
    name_regex = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_:.@/-]*$")
    if mode == RUST_PROBE_SUGGEST_COMMAND:
        if test_target or test_name:
            return rust_probe_validation_hint("suggest does not accept test_target or test_name")
        return None
    if mode == "check-lib":
        if test_target:
            return rust_probe_validation_hint("test_target is forbidden for mode check-lib")
        if test_name:
            return rust_probe_validation_hint("test_name is forbidden for mode check-lib")
        return None
    if mode in {"check-test-target", "nextest-no-run-test-target", "nextest-test-target"}:
        if not test_target:
            return rust_probe_validation_hint(f"test_target is required for mode {mode}")
        if test_name:
            return rust_probe_validation_hint(f"test_name is forbidden for mode {mode}")
    elif mode == "nextest-test-target-name":
        if not test_target:
            return rust_probe_validation_hint("test_target is required for mode nextest-test-target-name")
        if not test_name:
            return rust_probe_validation_hint("test_name is required for mode nextest-test-target-name")
    else:
        return rust_probe_validation_hint(f"unsupported mode: {mode}")
    if not target_regex.match(test_target):
        return rust_probe_validation_hint("test_target must be a safe Rust test target name")
    if test_name and not name_regex.match(test_name):
        return rust_probe_validation_hint("test_name must be a safe nextest test name")
    return None


def rust_probe_test_stem_for_path(path: str) -> str | None:
    normalized = path.strip().replace("\\", "/")
    if not normalized:
        return None
    parsed = pathlib.PurePosixPath(normalized)
    parts = parsed.parts
    if len(parts) != 2 or parts[0] != "tests" or parsed.suffix != ".rs":
        return None
    return parsed.stem


def rust_probe_test_target_for_path(
    path: str,
    *,
    manifest_path: pathlib.Path | None = None,
    tests_root: pathlib.Path | None = None,
) -> str | None:
    stem = rust_probe_test_stem_for_path(path)
    if stem is None:
        return None
    manifest = build_test_manifest(
        manifest_path or SCRIPT_DIR.parent / "Cargo.toml",
        tests_root or SCRIPT_DIR.parent / "tests",
    )
    return manifest.member_to_harness.get(stem)


def rust_probe_separate_workspace_for_path(path: str, separate_workspaces: dict[str, Any]) -> tuple[str, ...] | None:
    normalized = path.strip().replace("\\", "/")
    for prefix, suggestion in separate_workspaces.items():
        if normalized == prefix or normalized.startswith(f"{prefix}/"):
            return (str(suggestion["message"]), *(str(command) for command in suggestion["commands"]))
    return None


def rust_probe_suggestions(
    changed_files: list[str],
    separate_workspaces: dict[str, Any],
    *,
    manifest_path: pathlib.Path | None = None,
    tests_root: pathlib.Path | None = None,
) -> list[str]:
    normalized = sorted({path.strip().replace("\\", "/") for path in changed_files if path.strip()})
    test_stems = sorted(
        {
            stem
            for path in normalized
            if (stem := rust_probe_test_stem_for_path(path)) is not None
        }
    )
    target_to_stems: dict[str, list[str]] = {}
    if test_stems:
        manifest = build_test_manifest(
            manifest_path or SCRIPT_DIR.parent / "Cargo.toml",
            tests_root or SCRIPT_DIR.parent / "tests",
        )
        for stem in test_stems:
            target = manifest.member_to_harness.get(stem)
            if target is not None:
                target_to_stems.setdefault(target, []).append(stem)
    suggestions: list[str] = []
    lib_or_workspace_changed = any(
        path == "Cargo.toml"
        or path == "Cargo.lock"
        or path == "build.rs"
        or path.startswith("src/")
        for path in normalized
    )
    if lib_or_workspace_changed:
        suggestions.append("just rust-probe check-lib")
    for suggestion_lines in sorted(
        {
            suggestion
            for path in normalized
            if (suggestion := rust_probe_separate_workspace_for_path(path, separate_workspaces)) is not None
        }
    ):
        suggestions.extend(suggestion_lines)
    for target, stems in sorted(target_to_stems.items()):
        suggestions.extend(
            [
                f"just rust-probe check-test-target {target}",
                f"just rust-probe nextest-no-run-test-target {target}",
                f"just rust-probe nextest-test-target {target}",
            ]
        )
        for stem in sorted(stems):
            if stem == target:
                # Harness root file has no direct #[test]s; `nextest-test-target {target}`
                # above already covers it. A `{target}::` filter would match zero tests.
                continue
            suggestions.append(f"just rust-probe nextest-test-target-name {target} {stem}::")
    if suggestions:
        return suggestions
    if any(path.startswith("crates/") for path in normalized):
        return [
            "No root Rust Probe suggestion was inferred for changed crates/ paths.",
            "Configure remote_probe.separate_workspaces for separate workspaces that need non-root guidance.",
        ]
    return [
        "No Rust source or top-level integration-test target was inferred from changed files.",
        "No targeted Rust Probe command was inferred.",
    ]


def rust_probe_changed_files(repo: pathlib.Path, suggest_base_ref: str) -> tuple[list[str] | None, str | None, list[str]]:
    changed: set[str] = set()
    notes: list[str] = []
    working_tree, error = git_output(repo, "diff", "--name-only", "HEAD", "--")
    if error is not None:
        return None, error, notes
    changed.update(line for line in working_tree.splitlines() if line)
    untracked, error = git_output(repo, "ls-files", "--others", "--exclude-standard")
    if error is not None:
        return None, error, notes
    changed.update(line for line in untracked.splitlines() if line)
    merge_base, error = git_output(repo, "merge-base", suggest_base_ref, "HEAD")
    if error is not None or not merge_base:
        notes.append(
            f"merge-base for configured base ref {suggest_base_ref!r} was unavailable; "
            "using direct base-to-HEAD tree diff"
        )
        branch_diff, error = git_output(repo, "diff", "--name-only", suggest_base_ref, "HEAD", "--")
        if error is not None:
            return None, f"rust-probe suggest could not resolve configured base ref {suggest_base_ref!r}: {error}", notes
    else:
        branch_diff, error = git_output(repo, "diff", "--name-only", merge_base, "HEAD", "--")
    if error is not None:
        return None, error, notes
    changed.update(line for line in branch_diff.splitlines() if line)
    return sorted(changed), None, notes


def cmd_rust_probe_suggest(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    if args.runner_tier is not None:
        return verify_remote_fail("suggest does not accept --runner-tier because it does not dispatch a probe")
    try:
        probe_policy = remote_probe_policy(load_policy(repo))
    except (OSError, PolicyError, FileNotFoundError) as exc:
        return verify_remote_fail(str(exc))
    changed_files, error, notes = rust_probe_changed_files(repo, probe_policy["suggest_base_ref"])
    if error is not None or changed_files is None:
        return verify_remote_fail(error or "unable to inspect changed files")
    print("Rust Probe suggestions for targeted remote debugging:")
    print(f"base ref: {probe_policy['suggest_base_ref']} (ensure this ref is fetched and current)")
    for note in notes:
        print(f"note: {note}")
    if changed_files:
        print("changed files considered:")
        for path in changed_files[:20]:
            print(f"- {path}")
        if len(changed_files) > 20:
            print(f"- ... {len(changed_files) - 20} more")
    else:
        print("changed files considered: <none>")
    print("commands:")
    for suggestion in rust_probe_suggestions(changed_files, probe_policy["separate_workspaces"]):
        print(f"- {suggestion}")
    print("Rust Probe is not merge proof. Draft verify-remote is full feedback only; mark the PR ready for merge proof.")
    return 0


def run_display_title(run: dict[str, Any]) -> str:
    title = run.get("displayTitle")
    return title if isinstance(title, str) else ""


def matching_rust_probe_runs(runs: list[dict[str, Any]], *, head: str, probe_id: str) -> list[dict[str, Any]]:
    expected_prefix = f"Rust Probe {probe_id} "
    matching = [run for run in runs if run.get("headSha") == head and run_display_title(run).startswith(expected_prefix)]
    return sorted(matching, key=run_created_at, reverse=True)


def evaluate_rust_probe_run(
    run: dict[str, Any],
    *,
    head: str,
    probe_id: str,
) -> int | None:
    summary = workflow_run_summary(run)
    run_head = str(run.get("headSha") or "")
    if run_head != head:
        print(
            f"Rust Probe {probe_id} ended without exact-head evidence for {head}: "
            f"matched {run_head or '<missing>'}; {summary}",
            file=sys.stderr,
        )
        return 2
    state = workflow_run_state(run)
    if state == "pending":
        return None
    if state == "pass":
        print(f"OK: Rust Probe {probe_id} passed for {head}: {summary}")
        print("NOT MERGE PROOF -- draft verify-remote is feedback only; mark the PR ready for merge proof")
        return 0
    conclusion = str(run.get("conclusion") or "")
    if conclusion == "cancelled":
        print(f"Rust Probe {probe_id} was superseded or cancelled, not reported as a code failure: {summary}", file=sys.stderr)
        return 2
    if conclusion in {"timed_out", "action_required", "startup_failure", "stale", "skipped"}:
        print(f"Rust Probe {probe_id} ended without code-failure evidence: {summary}", file=sys.stderr)
        return 2
    print(f"Rust Probe {probe_id} failed for {head}; this is debugging feedback only:", file=sys.stderr)
    print(f"- {summary}", file=sys.stderr)
    print("NOT MERGE PROOF -- draft verify-remote is feedback only; mark the PR ready for merge proof", file=sys.stderr)
    return 1


def wait_for_rust_probe_run(
    *,
    repo: pathlib.Path,
    remote_policy: dict[str, Any],
    branch: str,
    head: str,
    probe_id: str,
) -> int:
    appear_deadline = time.monotonic() + remote_policy["appearance_timeout_seconds"]
    overall_deadline = time.monotonic() + remote_policy["overall_timeout_seconds"]
    interval = remote_policy["poll_interval_seconds"]
    tracked_run_id: int | None = None
    while True:
        now = time.monotonic()
        if now >= overall_deadline:
            return verify_remote_fail(f"timed out waiting for Rust Probe {probe_id} on {branch}")
        run: dict[str, Any] | None = None
        if tracked_run_id is not None:
            run, error = workflow_run_view(repo, tracked_run_id, command_name="rust-probe")
            if error is not None or run is None:
                return verify_remote_fail(error or f"unable to inspect Rust Probe run {tracked_run_id}")
        else:
            runs, error = rust_probe_run_list(repo, remote_policy, branch=branch)
            if error is not None or runs is None:
                return verify_remote_fail(error or "unable to inspect Rust Probe workflow runs")
            matching = matching_rust_probe_runs(runs, head=head, probe_id=probe_id)
            if matching:
                run = matching[0]
                tracked_run_id = run_database_id(run)
        if run is None:
            if now >= appear_deadline:
                return verify_remote_fail(f"no matching Rust Probe workflow run appeared for probe_id {probe_id}")
            time.sleep(interval)
            continue
        result = evaluate_rust_probe_run(run, head=head, probe_id=probe_id)
        if result is not None:
            return result
        time.sleep(interval)


def evaluate_full_ci_run(
    repo: pathlib.Path,
    run: dict[str, Any],
    *,
    dispatch_config: dict[str, Any],
    head: str,
    pr_url: str,
) -> int | None:
    state = workflow_run_state(run)
    if state == "pending":
        return None
    if state == "pass":
        required_gate_job = None
        missing_gate_message = ""
        if run.get("event") == "workflow_dispatch":
            if not workflow_dispatch_run_is_full(run, dispatch_config):
                print(f"Remote full CI failed for {head} on {pr_url}: workflow_dispatch run is not marked full", file=sys.stderr)
                print(f"- {workflow_run_summary(run)}", file=sys.stderr)
                return 1
            required_gate_job = dispatch_config["dispatch_full_gate_job"]
            missing_gate_message = "workflow_dispatch run lacks successful dispatch full gate job"
        elif run.get("event") == "pull_request":
            required_gate_job = dispatch_config["proof_gate_job"]
            missing_gate_message = "pull_request run lacks successful required gate job"
        else:
            print(
                f"Remote full CI failed for {head} on {pr_url}: unsupported workflow event {run.get('event')!r}",
                file=sys.stderr,
            )
            print(f"- {workflow_run_summary(run)}", file=sys.stderr)
            return 1
        if required_gate_job is not None:
            run_id = run_database_id(run)
            if run_id is None:
                print(f"Remote full CI failed for {head} on {pr_url}: workflow run databaseId missing", file=sys.stderr)
                return 1
            jobs, error = workflow_run_jobs(repo, run_id, run_attempt(run))
            if error is not None or jobs is None:
                print(f"Remote full CI failed for {head} on {pr_url}: {error or 'unable to inspect gate job'}", file=sys.stderr)
                return 1
            if not any(
                job_text(job, "name") == required_gate_job
                and job_text(job, "conclusion") == "success"
                for job in jobs
            ):
                print(
                    f"Remote full CI failed for {head} on {pr_url}: {missing_gate_message}",
                    file=sys.stderr,
                )
                print(f"- {workflow_run_summary(run)}", file=sys.stderr)
                return 1
        print(f"OK: remote full CI passed for {head} on {pr_url}: {workflow_run_summary(run)}")
        return 0
    print(f"Remote full CI failed for {head} on {pr_url}:", file=sys.stderr)
    print(f"- {workflow_run_summary(run)}", file=sys.stderr)
    return 1


def wait_for_full_ci_run(
    *,
    repo: pathlib.Path,
    dispatch_config: dict[str, Any],
    branch: str,
    head: str,
    pr_url: str,
    remote_policy: dict[str, int],
    events: set[str],
    created_at_floor: str | None = None,
    ignored_run_ids: set[int] | None = None,
    initial_tracked_run_id: int | None = None,
    sleep_before_initial_tracked_poll: bool = False,
    track_run_once_found: bool = False,
) -> int:
    appear_deadline = time.monotonic() + remote_policy["checks_appear_timeout_seconds"]
    overall_deadline = time.monotonic() + remote_policy["overall_timeout_seconds"]
    interval = remote_policy["poll_interval_seconds"]
    tracked_run_id: int | None = initial_tracked_run_id
    diagnostics_state = RemoteFailureDiagnosticsState()
    if tracked_run_id is not None and sleep_before_initial_tracked_poll:
        time.sleep(interval)
    while True:
        now = time.monotonic()
        if now >= overall_deadline:
            head_result = verify_remote_head_current_or_fail(repo, branch, head)
            if head_result is not None:
                return head_result
            return verify_remote_fail(f"timed out waiting for full-CI workflow run on {pr_url}")
        head_result = verify_remote_head_current_or_fail(repo, branch, head)
        if head_result is not None:
            return head_result
        run: dict[str, Any] | None = None
        if tracked_run_id is not None:
            run, error = workflow_run_view(repo, tracked_run_id)
            if error is not None or run is None:
                return verify_remote_fail(error or "unable to inspect tracked workflow run")
        else:
            runs, error = workflow_run_list(repo, dispatch_config, branch)
            if error is not None or runs is None:
                return verify_remote_fail(error or "unable to inspect workflow runs")
            matching = matching_full_ci_runs(
                runs,
                head=head,
                events=events,
                dispatch_config=dispatch_config,
                created_at_floor=created_at_floor,
                ignored_run_ids=ignored_run_ids,
            )
            if matching:
                run = matching[0]
                if track_run_once_found:
                    tracked_run_id = run_database_id(run)
        if run is None:
            if now >= appear_deadline:
                return verify_remote_fail(f"no matching full-CI workflow run appeared for {head} on {pr_url}")
            time.sleep(interval)
            continue
        head_result = verify_remote_head_current_or_fail(repo, branch, head)
        if head_result is not None:
            return head_result
        emit_failed_job_diagnostics(
            repo=repo,
            run=run,
            state=diagnostics_state,
            remote_policy=remote_policy,
        )
        if workflow_run_state(run) != "pending":
            head_result = verify_remote_head_current_or_fail(repo, branch, head)
            if head_result is not None:
                return head_result
        result = evaluate_full_ci_run(
            repo,
            run,
            dispatch_config=dispatch_config,
            head=head,
            pr_url=pr_url,
        )
        if result is not None:
            head_result = verify_remote_head_current_or_fail(repo, branch, head)
            if head_result is not None:
                return head_result
            return result
        time.sleep(interval)


def cmd_rust_probe(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    test_target = args.test_target or ""
    test_name = args.test_name or ""
    selection_error = validate_rust_probe_selection(args.mode, test_target, test_name)
    if selection_error is not None:
        return verify_remote_fail(selection_error)
    if args.mode == RUST_PROBE_SUGGEST_COMMAND:
        return cmd_rust_probe_suggest(args)
    try:
        policy = load_policy(repo)
        probe_policy = remote_probe_policy(policy)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        return verify_remote_fail(str(exc))
    head, branch, error = ensure_rust_probe_preconditions(repo)
    if error is not None or head is None or branch is None:
        return verify_remote_fail(error or "unable to inspect git state")
    active_count, error = rust_probe_active_run_count(repo, probe_policy)
    if error is not None or active_count is None:
        return verify_remote_fail(error or "unable to inspect active Rust Probe runs")
    if active_count >= probe_policy["active_run_limit"]:
        return verify_remote_fail(
            f"rust-probe active-run cap reached: {active_count} active runs, "
            f"limit {probe_policy['active_run_limit']}"
        )
    runner_tier = args.runner_tier or probe_policy["mode_runner_tiers"][args.mode]
    if runner_tier not in probe_policy["allowed_runner_tiers"]:
        return verify_remote_fail(f"rust-probe runner tier {runner_tier!r} is not allowed by policy")
    timeout_key = f"probe-{runner_tier}"
    job_timeout_minutes = probe_policy["workflow_timeouts"].get(timeout_key)
    if not isinstance(job_timeout_minutes, int):
        return verify_remote_fail(f"rust-probe runner tier {runner_tier!r} has no workflow timeout in policy")
    probe_id = new_probe_id()
    error = dispatch_rust_probe(
        repo,
        probe_policy,
        branch=branch,
        head=head,
        mode=args.mode,
        test_target=test_target,
        test_name=test_name,
        runner_tier=runner_tier,
        job_timeout_minutes=job_timeout_minutes,
        probe_id=probe_id,
    )
    if error is not None:
        return verify_remote_fail(error)
    scope = args.mode
    if test_target:
        scope += f" {test_target}"
    if test_name:
        scope += f" {test_name}"
    print(f"Dispatched Rust Probe {probe_id}")
    print(f"branch: {branch}")
    print(f"sha: {head}")
    print(f"scope: {scope}")
    print(f"runner_tier: {runner_tier}")
    print("NOT MERGE PROOF -- draft verify-remote is feedback only; mark the PR ready for merge proof")
    return wait_for_rust_probe_run(
        repo=repo,
        remote_policy=probe_policy,
        branch=branch,
        head=head,
        probe_id=probe_id,
    )


def cmd_ci_logs(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        remote_policy = remote_verification_policy(policy)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        return verify_remote_fail(str(exc))
    head, branch, error = ensure_verify_remote_preconditions(repo)
    if error is not None or head is None or branch is None:
        return verify_remote_fail(error or "unable to inspect git state")
    pr, error = pr_for_exact_head(repo, branch, head, during_watch=False)
    if error is not None or pr is None:
        return verify_remote_fail(error or "unable to inspect pull request")
    pr_url = pr.get("url") or f"PR #{pr.get('number')}"
    dispatch_config, error = ci_provenance_dispatch_config(repo)
    if error is not None or dispatch_config is None:
        return verify_remote_fail(error or "unable to inspect CI dispatch config")
    runs, error = workflow_run_list(repo, dispatch_config, branch)
    if error is not None or runs is None:
        return verify_remote_fail(error or "unable to inspect workflow runs")
    events = FULL_CI_DRAFT_EVENTS if bool(pr.get("isDraft")) else FULL_CI_READY_EVENTS
    matching = matching_full_ci_runs(
        runs,
        head=head,
        events=events,
        dispatch_config=dispatch_config,
    )
    if not matching:
        return verify_remote_fail(f"no matching full-CI workflow run found for {head} on {pr_url}")
    diagnostics_available = emit_failed_job_diagnostics(
        repo=repo,
        run=matching[0],
        state=RemoteFailureDiagnosticsState(),
        remote_policy=remote_policy,
    )
    return 2 if diagnostics_available is False else 0


def cmd_verify_remote(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        remote_policy = remote_verification_policy(policy)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        return verify_remote_fail(str(exc))
    head, branch, error = ensure_verify_remote_preconditions(repo)
    if error is not None or head is None or branch is None:
        return verify_remote_fail(error or "unable to inspect git state")
    pr, error = pr_for_exact_head(repo, branch, head, during_watch=False)
    if error is not None or pr is None:
        return verify_remote_fail(error or "unable to inspect pull request")
    pr_url = pr.get("url") or f"PR #{pr.get('number')}"
    dispatch_config, error = ci_provenance_dispatch_config(repo)
    if error is not None or dispatch_config is None:
        return verify_remote_fail(error or "unable to inspect CI dispatch config")

    print(
        "verify-remote full CI feedback: use just rust-probe suggest for targeted Rust debugging "
        "before spending full CI."
    )

    if bool(pr.get("isDraft")):
        is_fork, error = draft_pr_is_fork(repo, pr)
        if error is not None:
            return verify_remote_fail(error)
        if is_fork:
            return verify_remote_fail(
                "draft fork PRs cannot dispatch upstream full CI; mark the PR ready for review "
                "or have a maintainer move the branch into the upstream repository"
            )
        runs, error = workflow_run_list(repo, dispatch_config, branch)
        if error is not None or runs is None:
            return verify_remote_fail(error or "unable to inspect workflow runs")
        existing = matching_full_ci_runs(
            runs,
            head=head,
            events=FULL_CI_DRAFT_EVENTS,
            dispatch_config=dispatch_config,
        )
        _unused, error = dispatch_full_ci(repo, dispatch_config, branch)
        if error is not None:
            return verify_remote_fail(error)
        print(f"Dispatched full CI for {head} on {pr_url}")
        return wait_for_full_ci_run(
            repo=repo,
            dispatch_config=dispatch_config,
            branch=branch,
            head=head,
            pr_url=str(pr_url),
            remote_policy=remote_policy,
            events=FULL_CI_DRAFT_EVENTS,
            ignored_run_ids=run_ids(existing),
            track_run_once_found=True,
        )

    runs, error = workflow_run_list(repo, dispatch_config, branch)
    if error is not None or runs is None:
        return verify_remote_fail(error or "unable to inspect workflow runs")
    existing = matching_full_ci_runs(
        runs,
        head=head,
        events=FULL_CI_READY_EVENTS,
        dispatch_config=dispatch_config,
    )
    if existing:
        run = existing[0]
        state = workflow_run_state(run)
        if state == "pass":
            result = evaluate_full_ci_run(
                repo,
                run,
                dispatch_config=dispatch_config,
                head=head,
                pr_url=str(pr_url),
            )
            head_result = verify_remote_head_current_or_fail(repo, branch, head)
            if head_result is not None:
                return head_result
            return result
        if state == "pending":
            run_id = run_database_id(run)
            if run_id is not None:
                return wait_for_full_ci_run(
                    repo=repo,
                    dispatch_config=dispatch_config,
                    branch=branch,
                    head=head,
                    pr_url=str(pr_url),
                    remote_policy=remote_policy,
                    events=FULL_CI_READY_EVENTS,
                    initial_tracked_run_id=run_id,
                    sleep_before_initial_tracked_poll=True,
                    track_run_once_found=True,
                )

    return wait_for_full_ci_run(
        repo=repo,
        dispatch_config=dispatch_config,
        branch=branch,
        head=head,
        pr_url=str(pr_url),
        remote_policy=remote_policy,
        events=FULL_CI_READY_EVENTS,
        ignored_run_ids=run_ids(existing),
        track_run_once_found=True,
    )


def cmd_repo_status(args: argparse.Namespace) -> int:
    print(status_for_repo(repo_path(args.repo)))
    return 0


def cmd_is_managed(args: argparse.Namespace) -> int:
    status = status_for_repo(repo_path(args.repo))
    if status == "managed":
        return 0
    if status == "invalid-policy":
        return 2
    return 1


def cmd_validate_policy(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        resolve_cheap_lane_labels(repo, policy["local_lane_policy"])
    except FileNotFoundError:
        print(f"missing {POLICY_RELATIVE_PATH}", file=sys.stderr)
        return 2
    except (OSError, PolicyError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    build = policy["commands"]["build"]
    print(
        json.dumps(
            {
                "build_profile": build["profile"],
                "build_target": build["target"],
                "policy": str(policy_path(repo)),
                "project_id": policy["project_id"],
                "status": "ok",
            },
            sort_keys=True,
        )
    )
    return 0


def cmd_target_dir(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        path = target_dir(repo)
        path.mkdir(parents=True, exist_ok=True)
        print(path)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return 0


def cmd_binary_path(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    build = policy["commands"]["build"]
    binary = target_dir(repo, policy) / build["target"] / build["profile"] / args.bin
    print(binary)
    return 0


def cmd_cargo(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    cargo = "cargo"
    cargo_args = command_args(args.args)
    try:
        alias = cargo_alias_subcommand(cargo_args, repo)
    except PolicyError as exc:
        return print_refusal(refusal_payload(code="cargo_alias_config", reason=str(exc), dry_run=False))
    if alias is not None:
        return print_refusal(cargo_alias_refusal_payload(repo, policy, alias))
    override = cargo_target_routing_override(cargo_args)
    if override is not None:
        return print_refusal(target_routing_refusal_payload(repo, policy, override))
    local_refusal = local_compile_refusal_for_cargo_args(repo, policy, cargo_args)
    if local_refusal is not None:
        return print_refusal(local_refusal)
    if cargo_subcommand(cargo_args) == "clean":
        target, refusal = clean_preflight_refusal_payload(repo, policy)
        if refusal is not None:
            return print_refusal(refusal)
        if target is None:
            return print_refusal(
                refusal_payload(
                    code="preflight_error",
                    reason="unable to inspect managed cargo clean preflight: target dir unavailable",
                    dry_run=False,
                )
            )
        with cache_lock(policy, exclusive=True):
            target, refusal = clean_preflight_refusal_payload(repo, policy, target)
            if refusal is not None:
                return print_refusal(refusal)
            return run_process([cargo, *cargo_args], repo=repo, env=managed_env(repo, policy))
    if cargo_subcommand(cargo_args) == "fmt":
        return run_process([cargo, *cargo_args], repo=repo, env=scrubbed_local_env())
    if cargo_args_need_disk_preflight(cargo_args):
        refusal = disk_preflight_refusal_payload(repo, policy)
        if refusal is not None:
            return print_refusal(refusal)
    with cache_lock(policy, exclusive=cargo_args_need_exclusive_cache_lock(cargo_args)):
        return run_process([cargo, *cargo_args], repo=repo, env=managed_env(repo, policy))


def cmd_run(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        command = policy["commands"][args.command]
    except KeyError:
        print(f"unknown managed command: {args.command}", file=sys.stderr)
        return 2
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    test_separator = args.command == "test" and bool(getattr(args, "args_separator", False))
    command_tail = ["--", *args.args] if test_separator else args.args
    justfile = repo / "justfile"
    argv = ["just", "-f", str(justfile), "--working-directory", str(repo), "--", command["recipe"], *command_tail]
    override_args = [args.command, *command_tail] if args.command == "test" else args.args
    override = cargo_target_routing_override(override_args)
    if override is not None:
        return print_refusal(target_routing_refusal_payload(repo, policy, override))
    local_refusal = local_compile_refusal_for_managed_command(repo, policy, args.command)
    if local_refusal is not None:
        return print_refusal(local_refusal)
    if args.command in {"build", "clippy", "test"}:
        refusal = disk_preflight_refusal_payload(repo, policy)
        if refusal is not None:
            return print_refusal(refusal)
    run_exclusive = run_args_need_exclusive_cache_lock(
        args.command,
        args.args,
        test_separator=test_separator,
    )
    with cache_lock(policy, exclusive=run_exclusive):
        env = managed_env(repo, policy)
        env["BOLT_MANAGED_JUST"] = "1"
        return run_process(argv, repo=repo, env=env)


def cmd_scrub_env_keys(_args: argparse.Namespace) -> int:
    for key in SCRUB_ENV_KEYS:
        print(key)
    return 0


def cmd_describe(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    status = status_for_repo(repo)
    payload: dict[str, Any] = {"status": status, "policy": str(policy_path(repo))}
    if status == "managed":
        policy = load_policy(repo)
        payload["target_dir"] = str(target_dir(repo, policy))
        payload["project_id"] = policy["project_id"]
    print(json.dumps(payload, sort_keys=True))
    return 0


def cmd_cache_status(args: argparse.Namespace) -> int:
    if not args.json:
        print("--json is required for cache-status", file=sys.stderr)
        return 2
    repo = repo_path(args.repo)
    try:
        print(json.dumps(cache_status_payload(repo), sort_keys=True))
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return 0


def cmd_cache_prune(args: argparse.Namespace) -> int:
    if not args.json:
        print("--json is required for cache-prune", file=sys.stderr)
        return 2
    repo = repo_path(args.repo)
    dry_run = not args.apply
    try:
        payload = cache_prune_payload(repo, dry_run=dry_run)
        print(json.dumps(payload, sort_keys=True))
    except FileNotFoundError as exc:
        expected_policy = policy_path(repo)
        missing = pathlib.Path(getattr(exc, "filename", "") or exc.args[0])
        code = "missing_policy" if missing == expected_policy else "operation_failed"
        payload = refusal_payload(code=code, reason=str(exc), dry_run=dry_run)
        print(json.dumps(payload, sort_keys=True))
        return 2
    except PolicyError as exc:
        payload = refusal_payload(code="invalid_policy", reason=str(exc), dry_run=dry_run)
        print(json.dumps(payload, sort_keys=True))
        return 2
    except OSError as exc:
        payload = refusal_payload(code="operation_failed", reason=str(exc), dry_run=dry_run)
        print(json.dumps(payload, sort_keys=True))
        return 2
    if payload.get("refused"):
        return 2
    return 0


def cmd_assert_global_cargo_target_dir(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        payload = assert_global_cargo_target_dir(repo)
    except (OSError, PolicyError, RuntimeError, UnicodeDecodeError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    print(
        "global Cargo build.target-dir "
        f"{payload['status']}: {payload['config_path']} -> {payload['target_dir']}"
    )
    return 0


def cmd_cleanup(_args: argparse.Namespace) -> int:
    print(json.dumps({"status": "ok", "removed": []}, sort_keys=True))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command_name", required=True)

    repo_status = subparsers.add_parser("repo-status")
    repo_status.add_argument("--repo", required=True)
    repo_status.set_defaults(func=cmd_repo_status)

    is_managed = subparsers.add_parser("is-managed-rust-repo")
    is_managed.add_argument("--repo", required=True)
    is_managed.set_defaults(func=cmd_is_managed)

    validate = subparsers.add_parser("validate-policy")
    validate.add_argument("--repo", required=True)
    validate.set_defaults(func=cmd_validate_policy)

    target = subparsers.add_parser("target-dir")
    target.add_argument("--repo", required=True)
    target.set_defaults(func=cmd_target_dir)

    binary = subparsers.add_parser("binary-path")
    binary.add_argument("--repo", required=True)
    binary.add_argument("--bin", required=True)
    binary.set_defaults(func=cmd_binary_path)

    cargo = subparsers.add_parser("cargo")
    cargo.add_argument("--repo", required=True)
    cargo.add_argument("args", nargs=argparse.REMAINDER)
    cargo.set_defaults(func=cmd_cargo)

    run = subparsers.add_parser("run")
    run.add_argument("--repo", required=True)
    run.add_argument("command", choices=("test", "clippy", "build"))
    run.add_argument("args", nargs=argparse.REMAINDER)
    run.set_defaults(func=cmd_run)

    verify_remote = subparsers.add_parser("verify-remote")
    verify_remote.add_argument("--repo", required=True)
    verify_remote.set_defaults(func=cmd_verify_remote)

    rust_probe = subparsers.add_parser(
        "rust-probe",
        description="Dispatch a bounded remote Rust Probe for debugging feedback; not merge proof.",
        epilog=RUST_PROBE_HELP_EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    rust_probe.add_argument("--repo", required=True)
    rust_probe.add_argument("--runner-tier")
    rust_probe.add_argument("mode", choices=RUST_PROBE_COMMANDS)
    rust_probe.add_argument("test_target", nargs="?")
    rust_probe.add_argument("test_name", nargs="?")
    rust_probe.set_defaults(func=cmd_rust_probe)

    ci_logs = subparsers.add_parser(
        "ci-logs",
        description="Print failed-job diagnostics for the matching exact-head full-CI run; not a CI pass/fail gate.",
    )
    ci_logs.add_argument("--repo", required=True)
    ci_logs.set_defaults(func=cmd_ci_logs)

    scrub = subparsers.add_parser("scrub-env-keys")
    scrub.set_defaults(func=cmd_scrub_env_keys)

    describe = subparsers.add_parser("describe")
    describe.add_argument("--repo", required=True)
    describe.set_defaults(func=cmd_describe)

    cache_status = subparsers.add_parser("cache-status")
    cache_status.add_argument("--repo", required=True)
    cache_status.add_argument("--json", action="store_true", required=True, help="required; emit JSON output")
    cache_status.set_defaults(func=cmd_cache_status)

    cache_prune = subparsers.add_parser("cache-prune")
    cache_prune.add_argument("--repo", required=True)
    cache_prune_mode = cache_prune.add_mutually_exclusive_group()
    cache_prune_mode.add_argument("--dry-run", action="store_true")
    cache_prune_mode.add_argument("--apply", action="store_true")
    cache_prune.add_argument("--json", action="store_true", required=True, help="required; emit JSON output")
    cache_prune.set_defaults(func=cmd_cache_prune)

    global_cargo_target = subparsers.add_parser("assert-global-cargo-target-dir")
    global_cargo_target.add_argument("--repo", required=True)
    global_cargo_target.set_defaults(func=cmd_assert_global_cargo_target_dir)

    cleanup = subparsers.add_parser("cleanup")
    cleanup.set_defaults(func=cmd_cleanup)

    return parser


def run_args_had_separator(argv: list[str], args: argparse.Namespace) -> bool:
    if getattr(args, "command_name", None) != "run":
        return False
    for index, token in enumerate(argv):
        if token != args.command:
            continue
        tail = argv[index + 1 :]
        had_separator = bool(tail) and tail[0] == "--"
        normalized_tail = tail[1:] if had_separator else tail
        if normalized_tail == args.args:
            return had_separator
    return False


def main(argv: list[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    args = build_parser().parse_args(raw_argv)
    if getattr(args, "command_name", None) == "run":
        args.args_separator = run_args_had_separator(raw_argv, args)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
