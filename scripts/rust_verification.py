#!/usr/bin/env python3
"""Repo-local Rust verification owner for bolt-v2."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
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
from typing import Any

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

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

try:
    import tomllib as _toml
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback.
    try:
        import tomli as _toml  # type: ignore[no-redef]
    except ModuleNotFoundError:  # pragma: no cover - exercised by system Python on macOS.
        _toml = None


POLICY_RELATIVE_PATH = pathlib.Path("ci/rust-verification.toml")
MAX_POLICY_BYTES = 1024 * 1024
SAFE_IDENTIFIER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SCRUB_ENV_KEYS = (
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
            if not SAFE_IDENTIFIER_RE.match(key):
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
    if data.get("schema_version") != 1:
        raise PolicyError("schema_version must be 1")
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
    if "cache" in data:
        validate_cache_policy(data)


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


def validate_managed_target_path(target: pathlib.Path, policy: dict[str, Any]) -> None:
    if target.is_symlink():
        raise PolicyError("managed target directory is a symlink")
    namespace_root = root_base() / policy["target_namespace"]
    resolved_namespace = namespace_root.resolve(strict=False)
    resolved_target = target.resolve(strict=False)
    if target.name != "target" or not is_direct_child(resolved_target, resolved_namespace):
        raise PolicyError("managed target directory is outside the cache namespace")


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
    env = os.environ.copy()
    for key in SCRUB_ENV_KEYS:
        env.pop(key, None)
    env["CARGO_TARGET_DIR"] = str(target_dir(repo, policy))
    env["RUST_VERIFICATION_PRESERVE_ROUTING_ENV"] = "1"
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
    validate_managed_target_path(target, policy)
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


def active_related_processes(
    repo: pathlib.Path,
    target: pathlib.Path,
    policy: dict[str, Any],
    *,
    extra_scopes: tuple[pathlib.Path, ...] = (),
) -> list[dict[str, Any]]:
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
    extra_path_scopes = tuple(scope.resolve() for scope in extra_scopes)
    path_scopes = (repo_scope, target_scope, *extra_path_scopes)
    scope_texts = {str(repo), str(target), str(repo_scope), str(target_scope)}
    for scope, resolved_scope in zip(extra_scopes, extra_path_scopes):
        scope_texts.add(str(scope))
        scope_texts.add(str(resolved_scope))
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
        cwd_matches_scope = cwd is not None and any(path_is_or_inside(cwd, scope) for scope in path_scopes)
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


def active_process_refusal_payload(
    repo: pathlib.Path,
    target: pathlib.Path,
    policy: dict[str, Any],
    *,
    extra_scopes: tuple[pathlib.Path, ...] = (),
) -> dict[str, Any] | None:
    try:
        active = active_related_processes(repo, target, policy, extra_scopes=extra_scopes)
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


def refusal_with_removed(refusal: dict[str, Any], removed: list[dict[str, Any]]) -> dict[str, Any]:
    if not removed:
        return refusal
    updated = dict(refusal)
    updated["removed"] = list(removed)
    return updated


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


def cache_reset_candidates(target: pathlib.Path) -> tuple[list[dict[str, Any]], int]:
    candidates: list[dict[str, Any]] = []
    reclaimable_bytes = 0
    if not target.exists():
        return candidates, reclaimable_bytes
    for child in sorted(target.iterdir(), key=lambda item: item.name):
        child_bytes, latest_mtime, skipped = scan_cache_tree(child)
        entry = {
            "bytes": child_bytes,
            "class": "cache_reset",
            "latest_mtime": latest_mtime,
            "path": str(child),
            "reason": "explicit managed cache reset",
            "skipped_special_entries": skipped,
        }
        candidates.append(entry)
        reclaimable_bytes += child_bytes
    return candidates, reclaimable_bytes


def cache_reset_payload(repo: pathlib.Path, *, dry_run: bool) -> dict[str, Any]:
    policy = load_policy(repo)
    validate_cache_policy(policy)
    lock_context = cache_lock(policy, exclusive=True) if not dry_run else contextlib.nullcontext()
    with lock_context:
        target = target_dir(repo, policy)
        validate_managed_target_path(target, policy)
        if not dry_run:
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return refusal
        candidates, reclaimable_bytes = cache_reset_candidates(target)
        removed: list[dict[str, Any]] = []
        if not dry_run:
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return refusal
            for entry in candidates:
                refusal = active_process_refusal_payload(repo, target, policy)
                if refusal is not None:
                    return refusal_with_removed(refusal, removed)
                remove_cache_candidate(entry, target)
                removed.append(entry)
        return {
            "candidates": candidates,
            "dry_run": dry_run,
            "rebuild_required": bool(candidates),
            "reclaimable_bytes": reclaimable_bytes,
            "refused": False,
            "removed": removed,
            "target_dir": str(target),
        }


def cleanup_config(policy: dict[str, Any]) -> dict[str, Any]:
    cleanup = policy.get("cleanup", {})
    if not isinstance(cleanup, dict):
        raise PolicyError("cleanup table must be a table")
    return cleanup


def cleanup_tmp_bundle_config(policy: dict[str, Any]) -> dict[str, Any] | None:
    cleanup = cleanup_config(policy)
    tmp_bundles = cleanup.get("tmp_bundles")
    if tmp_bundles is None:
        return None
    if not isinstance(tmp_bundles, dict):
        raise PolicyError("cleanup.tmp_bundles must be a table")
    parent = tmp_bundles.get("parent")
    prefix = tmp_bundles.get("prefix")
    prune_after_days = tmp_bundles.get("prune_after_days")
    if not isinstance(parent, str) or not parent:
        raise PolicyError("cleanup.tmp_bundles.parent must be a non-empty string")
    parent_path = pathlib.Path(parent).expanduser()
    if not parent_path.is_absolute():
        raise PolicyError("cleanup.tmp_bundles.parent must be an absolute path")
    if parent_path.exists() and not parent_path.is_dir():
        raise PolicyError("cleanup.tmp_bundles.parent must be a directory")
    if not isinstance(prefix, str) or not prefix:
        raise PolicyError("cleanup.tmp_bundles.prefix must be a non-empty string")
    if not is_non_negative_int(prune_after_days):
        raise PolicyError("cleanup.tmp_bundles.prune_after_days must be a non-negative integer")
    return tmp_bundles


def cleanup_worktree_target_config(policy: dict[str, Any]) -> dict[str, Any] | None:
    cleanup = cleanup_config(policy)
    worktree_targets = cleanup.get("worktree_targets")
    if worktree_targets is None:
        return None
    if not isinstance(worktree_targets, dict):
        raise PolicyError("cleanup.worktree_targets must be a table")
    dirname = worktree_targets.get("dirname")
    if not isinstance(dirname, str) or not dirname or "/" in dirname:
        raise PolicyError("cleanup.worktree_targets.dirname must be a non-empty path name")
    return worktree_targets


def registered_worktree_paths(repo: pathlib.Path) -> set[pathlib.Path]:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), "worktree", "list", "--porcelain"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as exc:
        raise PolicyError(f"unable to inspect registered worktrees: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or f"exit {result.returncode}"
        raise PolicyError(f"unable to inspect registered worktrees: {detail}")
    paths: set[pathlib.Path] = set()
    for line in result.stdout.splitlines():
        if not line.startswith("worktree "):
            continue
        raw_path = line.removeprefix("worktree ")
        try:
            paths.add(pathlib.Path(raw_path).resolve(strict=True))
        except (OSError, RuntimeError) as exc:
            raise PolicyError(f"unable to resolve registered worktree path {raw_path}: {exc}") from exc
    return paths


def cleanup_worktree_target_candidates(
    repo: pathlib.Path,
    policy: dict[str, Any],
    *,
    registered: set[pathlib.Path],
) -> list[dict[str, Any]]:
    config = cleanup_worktree_target_config(policy)
    if config is None:
        return []
    dirname = str(config["dirname"])
    managed_target = target_dir(repo, policy).resolve()
    candidates: list[dict[str, Any]] = []
    for worktree in sorted(registered, key=lambda item: str(item)):
        candidate = worktree / dirname
        try:
            candidate.lstat()
        except FileNotFoundError:
            continue
        if not candidate.is_dir() or candidate.is_symlink():
            continue
        try:
            resolved = candidate.resolve()
        except (OSError, RuntimeError):
            resolved = candidate
        if resolved == managed_target:
            continue
        child_bytes, latest_mtime, skipped = scan_cache_tree(candidate)
        candidates.append(
            {
                "bytes": child_bytes,
                "class": "worktree_target",
                "dirname": dirname,
                "latest_mtime": latest_mtime,
                "path": str(candidate),
                "reason": f"registered worktree contains cleanup.worktree_targets.dirname ({dirname})",
                "skipped_special_entries": skipped,
                "worktree": str(worktree),
            }
        )
    return candidates


def cleanup_tmp_bundle_candidates(
    repo: pathlib.Path,
    policy: dict[str, Any],
    *,
    now: float,
    registered: set[pathlib.Path],
) -> list[dict[str, Any]]:
    config = cleanup_tmp_bundle_config(policy)
    if config is None:
        return []
    parent = pathlib.Path(str(config["parent"])).expanduser()
    if not parent.exists():
        return []
    if not parent.is_dir():
        raise PolicyError("cleanup.tmp_bundles.parent must be a directory")
    prefix = str(config["prefix"])
    prune_after_seconds = int(config["prune_after_days"]) * 24 * 60 * 60
    candidates: list[dict[str, Any]] = []
    for child in sorted(parent.iterdir(), key=lambda item: item.name):
        if not child.name.startswith(prefix):
            continue
        try:
            child.lstat()
        except FileNotFoundError:
            continue
        if not child.is_dir() or child.is_symlink():
            continue
        try:
            resolved = child.resolve()
        except (OSError, RuntimeError):
            resolved = child
        if resolved in registered:
            continue
        child_bytes, latest_mtime, skipped = scan_cache_tree(child)
        age_seconds = now - latest_mtime
        if skipped or age_seconds < prune_after_seconds:
            continue
        candidates.append(
            {
                "bytes": child_bytes,
                "class": "tmp_bundle",
                "latest_mtime": latest_mtime,
                "parent": str(parent),
                "path": str(child),
                "reason": f"tmp bundle older than cleanup.tmp_bundles.prune_after_days ({config['prune_after_days']})",
                "skipped_special_entries": skipped,
            }
        )
    return candidates


def remove_cleanup_candidate(entry: dict[str, Any]) -> None:
    path = pathlib.Path(entry["path"])
    try:
        resolved_path = path.resolve(strict=True)
    except (OSError, RuntimeError) as exc:
        raise PolicyError(f"unable to resolve cleanup candidate {path}: {exc}") from exc
    class_name = entry.get("class")
    if class_name == "tmp_bundle":
        parent = pathlib.Path(str(entry.get("parent", ""))).expanduser()
        if not is_direct_child(resolved_path, parent.resolve(strict=False)):
            raise PolicyError("refusing to remove tmp bundle outside configured parent")
    elif class_name == "worktree_target":
        worktree = pathlib.Path(str(entry.get("worktree", ""))).expanduser()
        dirname = str(entry.get("dirname", ""))
        if not dirname or path.name != dirname or not is_direct_child(resolved_path, worktree.resolve(strict=False)):
            raise PolicyError("refusing to remove worktree target outside registered worktree")
    else:
        raise PolicyError("refusing to remove unknown cleanup candidate class")
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        raise PolicyError("refusing to remove non-directory cleanup candidate")


def cleanup_candidate_refusal_payload(
    repo: pathlib.Path,
    entry: dict[str, Any],
    policy: dict[str, Any],
) -> dict[str, Any] | None:
    path = pathlib.Path(str(entry["path"]))
    extra_scopes: tuple[pathlib.Path, ...] = ()
    if entry.get("class") == "worktree_target":
        extra_scopes = (pathlib.Path(str(entry.get("worktree", path.parent))),)
    return active_process_refusal_payload(repo, path, policy, extra_scopes=extra_scopes)


def cleanup_payload(repo: pathlib.Path, *, dry_run: bool) -> dict[str, Any]:
    policy = load_policy(repo)
    validate_cache_policy(policy)
    lock_context = cache_lock(policy, exclusive=True) if not dry_run else contextlib.nullcontext()
    with lock_context:
        target = target_dir(repo, policy)
        validate_managed_target_path(target, policy)
        registered = registered_worktree_paths(repo)
        candidates = [
            *cleanup_tmp_bundle_candidates(repo, policy, now=time.time(), registered=registered),
            *cleanup_worktree_target_candidates(repo, policy, registered=registered),
        ]
        reclaimable_bytes = sum(int(entry["bytes"]) for entry in candidates)
        removed: list[dict[str, Any]] = []
        if not dry_run:
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return refusal
            for entry in candidates:
                refusal = cleanup_candidate_refusal_payload(repo, entry, policy)
                if refusal is not None:
                    return refusal
            for entry in candidates:
                refusal = cleanup_candidate_refusal_payload(repo, entry, policy)
                if refusal is not None:
                    return refusal_with_removed(refusal, removed)
                remove_cleanup_candidate(entry)
                removed.append(entry)
        return {
            "candidates": candidates,
            "dry_run": dry_run,
            "reclaimable_bytes": reclaimable_bytes,
            "refused": False,
            "removed": removed,
            "target_dir": str(target),
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
        validate_managed_target_path(inspected_target, policy)
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
    {"bench", "build", "check", "clippy", "doc", "fetch", "install", "nextest", "run", "rustc", "test"}
)
CARGO_PROCESS_SUBCOMMANDS = CARGO_DISK_PREFLIGHT_SUBCOMMANDS | {"clean", "fmt"}
CARGO_ALIAS_SUBCOMMANDS = {"b", "c", "d", "r", "t"}
CARGO_CONFIG_RELATIVE_PATHS = (pathlib.Path(".cargo/config.toml"), pathlib.Path(".cargo/config"))
CARGO_HOME_CONFIG_NAMES = ("config.toml", "config")


def cargo_args_need_disk_preflight(cargo_args: list[str]) -> bool:
    return cargo_subcommand(cargo_args) in CARGO_DISK_PREFLIGHT_SUBCOMMANDS


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


def cmd_cache_reset(args: argparse.Namespace) -> int:
    if not args.json:
        print("--json is required for cache-reset", file=sys.stderr)
        return 2
    repo = repo_path(args.repo)
    dry_run = not args.apply
    try:
        payload = cache_reset_payload(repo, dry_run=dry_run)
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


def cmd_cleanup(args: argparse.Namespace) -> int:
    if not args.json:
        print("--json is required for cleanup", file=sys.stderr)
        return 2
    repo = repo_path(args.repo)
    dry_run = not args.apply
    try:
        payload = cleanup_payload(repo, dry_run=dry_run)
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

    cache_reset = subparsers.add_parser("cache-reset")
    cache_reset.add_argument("--repo", required=True)
    cache_reset_mode = cache_reset.add_mutually_exclusive_group()
    cache_reset_mode.add_argument("--dry-run", action="store_true")
    cache_reset_mode.add_argument("--apply", action="store_true")
    cache_reset.add_argument("--json", action="store_true", required=True, help="required; emit JSON output")
    cache_reset.set_defaults(func=cmd_cache_reset)

    cleanup = subparsers.add_parser("cleanup")
    cleanup.add_argument("--repo", required=True)
    cleanup_mode = cleanup.add_mutually_exclusive_group()
    cleanup_mode.add_argument("--dry-run", action="store_true")
    cleanup_mode.add_argument("--apply", action="store_true")
    cleanup.add_argument("--json", action="store_true", required=True, help="required; emit JSON output")
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
