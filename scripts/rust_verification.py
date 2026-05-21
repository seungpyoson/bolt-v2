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
    "chrt",
    "command",
    "dash",
    "docker",
    "env",
    "exec",
    "fish",
    "flock",
    "ionice",
    "make",
    "nohup",
    "npm",
    "python",
    "python2",
    "python3",
    "rustup",
    "setsid",
    "sh",
    "taskset",
    "time",
    "timeout",
    "xargs",
    "zsh",
}


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
        return rest[0]
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
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in argument_options and index + 1 < len(tokens):
            index += 2
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
            index += 1
            continue
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


def process_names_from_tokens(tokens: list[str], *, depth: int = 0) -> set[str]:
    # Depth cap guards against pathological re-tokenisation loops while
    # leaving headroom for realistic legitimate wrapper stacks. A supported
    # chain like `sudo nice env -i bash -c 'rustup run stable cargo test'`
    # reaches `cargo` at depth 5; cap 6 keeps one slot of safety margin.
    if not tokens or depth > 6:
        return set()
    executable = basename_token(tokens[0])
    names = {executable}
    if executable == "env":
        names.update(process_names_from_tokens(env_wrapped_tokens(tokens), depth=depth + 1))
    elif executable in ("sudo", "doas"):
        names.update(process_names_from_tokens(tokens[sudo_command_index(tokens) :], depth=depth + 1))
    elif executable == "nice":
        names.update(process_names_from_tokens(tokens[nice_command_index(tokens) :], depth=depth + 1))
    elif executable == "flock":
        names.update(process_names_from_tokens(flock_wrapped_tokens(tokens), depth=depth + 1))
    elif executable == "rustup" and len(tokens) >= 4 and tokens[1] == "run":
        names.update(process_names_from_tokens(rustup_run_tokens(tokens), depth=depth + 1))
    elif executable == "timeout":
        names.update(process_names_from_tokens(tokens[timeout_command_index(tokens) :], depth=depth + 1))
    elif executable in ("bash", "dash", "fish", "sh", "zsh"):
        command = shell_command(tokens)
        if command is not None:
            names.update(process_names_from_tokens(command_tokens(command), depth=depth + 1))
    elif executable.startswith("python"):
        script_name = python_script_name(tokens, 1)
        if script_name is not None:
            names.add(script_name)
    return names


def command_process_names(command: str) -> set[str]:
    tokens = command_tokens(command)
    return process_names_from_tokens(tokens)


def executable_is_rust_tool(executable: str) -> bool:
    return (
        executable in {"cargo", "clippy", "nextest", "rustc", "rustdoc", "rustup"}
        or executable.startswith(("cargo-", "clippy-", "rust-"))
    )


def command_may_launch_rust(command: str) -> bool:
    tokens = command_tokens(command)
    if not tokens:
        return False
    executable = basename_token(tokens[0])
    if executable_is_rust_tool(executable):
        return True
    names = process_names_from_tokens(tokens)
    if any(executable_is_rust_tool(name) or name == "nextest" for name in names):
        return True
    cargo_specific_tokens = {"--manifest-path", "--workspace", "--all-targets", "--all-features"}
    if any(token in cargo_specific_tokens for token in tokens):
        if any(token in CARGO_PROCESS_SUBCOMMANDS for token in tokens):
            return True
    rustc_specific_tokens = {"--crate-name", "--emit", "--out-dir"}
    if any(token in rustc_specific_tokens or token.startswith("--emit=") or token.startswith("--out-dir=") for token in tokens):
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


def matching_process_pattern(command: str, patterns: list[str]) -> str | None:
    names = command_process_names(command)
    return next((pattern for pattern in patterns if basename_token(pattern) in names), None)


def active_related_processes(repo: pathlib.Path, target: pathlib.Path, policy: dict[str, Any]) -> list[dict[str, Any]]:
    patterns = active_process_patterns(policy)
    if not patterns:
        return []
    result = subprocess.run(
        ["ps", "-ax", "-o", "pid=", "-o", "command="],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise ProcessVisibilityError("unable to inspect active processes")
    current_pid = os.getpid()
    related: list[dict[str, Any]] = []
    repo_scope = repo.resolve()
    target_scope = target.resolve()
    scope_texts = {str(repo), str(target), str(repo_scope), str(target_scope)}
    unscoped_match = False
    for line in result.stdout.splitlines():
        stripped = line.strip()
        pid_text, _, command = stripped.partition(" ")
        if not pid_text.isdigit() or int(pid_text) == current_pid:
            continue
        pid = int(pid_text)
        command_matches_scope = any(scope_text in command for scope_text in scope_texts)
        matched_pattern = matching_process_pattern(command, patterns)
        may_launch_rust = matched_pattern is None and command_may_launch_rust(command)
        may_launch_build = matched_pattern is None and not may_launch_rust and command_may_launch_build(command)
        if matched_pattern is None and not may_launch_rust and not may_launch_build and not command_matches_scope:
            continue
        if command_matches_scope:
            if matched_pattern is None:
                if may_launch_rust:
                    matched_pattern = "unclassified Rust launch command"
                elif may_launch_build:
                    matched_pattern = "unclassified build launch command"
                else:
                    continue
            entry = {
                "command": command,
                "pid": pid,
                "reason": f"matched {matched_pattern} and referenced repo or target",
            }
            related.append(entry)
            continue
        cwd = process_cwd(pid)
        cwd_matches_scope = cwd is not None and (
            path_is_or_inside(cwd, repo_scope) or path_is_or_inside(cwd, target_scope)
        )
        if matched_pattern is None:
            if cwd_matches_scope and may_launch_rust:
                matched_pattern = "unclassified Rust launch command"
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
    status = cache_status_payload(repo)
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


def print_refusal(payload: dict[str, Any]) -> int:
    print(json.dumps(payload, sort_keys=True), file=sys.stderr)
    return 2


CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT = {
    "--config",
    "--color",
    "--jobs",
    "--target-dir",
    "-C",
    "-Z",
}
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT = {
    "--help",
    "--list",
    "--frozen",
    "--locked",
    "--offline",
    "--quiet",
    "--verbose",
    "--version",
    "-q",
    "-v",
    "-V",
}
CARGO_DISK_PREFLIGHT_SUBCOMMANDS = frozenset(
    {"bench", "build", "check", "clippy", "doc", "fetch", "install", "nextest", "run", "rustc", "test"}
)
CARGO_PROCESS_SUBCOMMANDS = CARGO_DISK_PREFLIGHT_SUBCOMMANDS | {"clean", "fmt"}
CARGO_ALIAS_SUBCOMMANDS = {"b", "c", "d", "r", "t"}


def cargo_subcommand_with_index(cargo_args: list[str]) -> tuple[int, str] | None:
    index = 0
    while index < len(cargo_args):
        token = cargo_args[index]
        if token.startswith("+"):
            index += 1
            continue
        if token == "--":
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return index, token
    return None


def cargo_subcommand(cargo_args: list[str]) -> str | None:
    subcommand = cargo_subcommand_with_index(cargo_args)
    if subcommand is None:
        return None
    return subcommand[1]


def cargo_args_need_disk_preflight(cargo_args: list[str]) -> bool:
    return cargo_subcommand(cargo_args) in CARGO_DISK_PREFLIGHT_SUBCOMMANDS


def cargo_alias_subcommand(cargo_args: list[str]) -> str | None:
    subcommand = cargo_subcommand(cargo_args)
    if subcommand in CARGO_ALIAS_SUBCOMMANDS:
        return subcommand
    return None


def cargo_args_for_target_routing_scan(cargo_args: list[str]) -> list[str]:
    subcommand = cargo_subcommand_with_index(cargo_args)
    if subcommand is None:
        return cargo_args
    subcommand_index, subcommand_name = subcommand
    if subcommand_name not in {"bench", "run", "test"}:
        return cargo_args
    for index, token in enumerate(cargo_args):
        if index > subcommand_index and token == "--":
            return cargo_args[:index]
    return cargo_args


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
        if token == "-C" and index + 1 < len(scan_args):
            override = cargo_config_storage_override(scan_args[index + 1])
            if override is not None:
                return f"-C {override}"
        if token.startswith("-C"):
            override = cargo_config_storage_override(token[2:])
            if override is not None:
                return f"-C{override}"
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
    alias = cargo_alias_subcommand(cargo_args)
    if alias is not None:
        return print_refusal(cargo_alias_refusal_payload(repo, policy, alias))
    override = cargo_target_routing_override(cargo_args)
    if override is not None:
        return print_refusal(target_routing_refusal_payload(repo, policy, override))
    if cargo_subcommand(cargo_args) == "clean":
        target = target_dir(repo, policy)
        refusal = active_process_refusal_payload(repo, target, policy)
        if refusal is not None:
            return print_refusal(refusal)
        with cache_lock(policy, exclusive=True):
            refusal = active_process_refusal_payload(repo, target, policy)
            if refusal is not None:
                return print_refusal(refusal)
            return run_process([cargo, *cargo_args], repo=repo, env=managed_env(repo, policy))
    if cargo_args_need_disk_preflight(cargo_args):
        refusal = disk_preflight_refusal_payload(repo, policy)
        if refusal is not None:
            return print_refusal(refusal)
    with cache_lock(policy, exclusive=False):
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
    justfile = repo / "justfile"
    argv = ["just", "-f", str(justfile), "--working-directory", str(repo), "--", command["recipe"], *args.args]
    override = cargo_target_routing_override(args.args)
    if override is not None:
        return print_refusal(target_routing_refusal_payload(repo, policy, override))
    if args.command in {"build", "clippy", "test"}:
        refusal = disk_preflight_refusal_payload(repo, policy)
        if refusal is not None:
            return print_refusal(refusal)
    with cache_lock(policy, exclusive=False):
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

    cleanup = subparsers.add_parser("cleanup")
    cleanup.set_defaults(func=cmd_cleanup)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
