#!/usr/bin/env python3
"""Auto-cleanup of merged branches and worktrees.

Execution lanes split by trust/speed profile:

- Lane S (manual sync, network-bound):
    `git fetch --prune <remote>` and fast-forward local trunk to the refreshed
    remote-tracking trunk. Composed dry-run fetches trunk into a temporary
    preview ref for exact cleanup reporting. Apply refuses dirty/non-FF.
- Lane H (hook, always-on, offline, reflog-safe):
    `git merge-base --is-ancestor` + CAS `git update-ref -d` for
    non-worktree-bound ancestor branches. No gh. Never bare `-D`.
- Lane R (reconcile, network-bound):
    Per-branch `gh pr list --head <B>` matched on `headRefOid == tip` AND
    `baseRefName == trunk` AND same-repo. CAS `update-ref -d`. Skips
    worktree-bound branches (those flow to Lane W).
- Lane W (worktree, explicit):
    Owns worktree-bound branches end-to-end BEFORE the ref is deleted.
    tar + verify + `git worktree remove`, then CAS `update-ref -d` with the
    re-read tip. Fail-closed on ignored content, assume-unchanged/skip-worktree
    bits (BOTH lowercase ls-files -v flags AND uppercase `S`), nested `.git`,
    dirty. Re-validates hidden-bits + ignored at the TOCTOU point too.
- Lane T (target-dir reaper, explicit):
    Removes idle worktree-local raw Cargo `target/` directories from surviving
    linked worktrees. Recency is the latest mtime anywhere inside the subtree.
    Apply refuses on active Cargo/Rust processes or missing process visibility.

See docs/ops/clean-merged-design.md for the full design and accepted risks.
Config lives in config/clean-merged.toml (single source of truth).
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import hashlib
import json
import math
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import time
from typing import Any, Callable

try:
    import tomllib
except ModuleNotFoundError as exc:
    tomllib = None  # type: ignore[assignment]
    _TOMLLIB_IMPORT_ERROR: ModuleNotFoundError | None = exc
    _TOML_DECODE_ERROR: type[Exception] = ValueError
else:
    _TOMLLIB_IMPORT_ERROR = None
    _TOML_DECODE_ERROR = tomllib.TOMLDecodeError

SCRIPT_NAME = "clean-merged"
CLEAN_MERGED_HOOKS = ("post-merge", "post-checkout", "post-rewrite")
HOOK_MANIFEST_NAME = "clean-merged.hooks-manifest.json"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHORT_SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
LOCK_FILE = "clean-merged.lock"
GIT_HOOK_NAMES = frozenset(
    (
        "applypatch-msg",
        "pre-applypatch",
        "post-applypatch",
        "pre-commit",
        "pre-merge-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
        "pre-rebase",
        "post-checkout",
        "post-merge",
        "pre-push",
        "pre-receive",
        "update",
        "proc-receive",
        "post-receive",
        "post-update",
        "reference-transaction",
        "push-to-checkout",
        "pre-auto-gc",
        "post-rewrite",
        "sendemail-validate",
        "fsmonitor-watchman",
        "p4-changelist",
        "p4-prepare-changelist",
        "p4-post-changelist",
        "p4-pre-submit",
        "post-index-change",
    )
)
HOOK_MANIFEST_ACTIVE_SCOPES = frozenset(("default", "local", "worktree", "global", "system"))
HOOK_MANIFEST_SOURCE_DIR_SCOPES = frozenset(("local", "worktree", "global", "system"))
# Internal marker that _lane_w_eligible prefixes onto the reason for a refused
# detached-HEAD worktree; run_lane_w strips it and maps it to the distinct
# 'refused-detached-head' action. Single source of truth shared by the producer
# and the consumer so a one-sided edit cannot silently drop the label or leak
# the marker into the operator-facing reason.
_REFUSED_DETACHED_SENTINEL = "__REFUSED_DETACHED_HEAD__:"


def _load_toml(path: pathlib.Path) -> dict[str, Any]:
    """Load clean-merged TOML through the single stdlib parser path."""
    if _TOMLLIB_IMPORT_ERROR is not None or tomllib is None:
        raise ConfigError(
            "Python 3.11+ with stdlib tomllib is required to read "
            f"{path}; current interpreter is {sys.executable}"
        )
    return tomllib.loads(path.read_text(encoding="utf-8"))


class CleanMergedError(RuntimeError):
    """Base error. Always caught at the top level so hooks never break git."""


class ConfigError(CleanMergedError):
    pass


# ---------------------------------------------------------------------------
# Config


@dataclasses.dataclass(frozen=True)
class LaneRConfig:
    gh_timeout_s: float
    cache_ttl_s: float
    gh_limit: int


@dataclasses.dataclass(frozen=True)
class LaneWConfig:
    quarantine_dir: str
    quarantine_grace_days: int
    discard_ignored: bool
    remove_nested_repos: bool
    discard_hidden_index_bits: bool
    archive_timeout_s: float
    archive_verify_timeout_s: float


@dataclasses.dataclass(frozen=True)
class LaneTConfig:
    target_dir_name: str
    idle_after_days: int
    active_process_patterns: tuple[str, ...]
    process_list_timeout_s: float
    cwd_visibility_timeout_s: float


@dataclasses.dataclass(frozen=True)
class LoggingConfig:
    audit_format: str
    audit_path: str
    max_log_bytes: int
    rotated_log_retention_days: int
    report_error_max_chars: int
    heartbeat_path: str
    heartbeat_stale_days: int
    lane_r_log_path: str


@dataclasses.dataclass(frozen=True)
class BackupsConfig:
    prune_after_days: int


@dataclasses.dataclass(frozen=True)
class Config:
    enabled: bool
    trunk_branch: str
    remote_name: str
    lane_r: LaneRConfig
    lane_w: LaneWConfig
    lane_t: LaneTConfig | None
    logging: LoggingConfig
    backups: BackupsConfig
    origin_owner: str


def _resolve_repo_root(start: pathlib.Path) -> pathlib.Path:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=start, check=True, capture_output=True, text=True,
        )
    except subprocess.CalledProcessError as exc:
        raise CleanMergedError(f"not inside a git repo: {exc.stderr.strip()}") from exc
    return pathlib.Path(out.stdout.strip())


def _main_worktree_root(repo_root: pathlib.Path) -> pathlib.Path:
    """Resolve the MAIN worktree root (where config/docs live), not a feature worktree.

    Do not infer this from `git rev-parse --git-common-dir`. Inside a
    submodule that returns `<super>/.git/modules/<name>`, whose parent is
    inside the superproject git dir rather than a working tree.

    Hardened approach: parse `git worktree list --porcelain` and return the
    FIRST worktree's path. Idempotent and correct for normal repos, linked
    worktrees, and submodules-in-their-own-working-tree.

    Submodule limitation: this returns the submodule's main worktree, but
    `load_config` then looks for `config/clean-merged.toml` there. Most
    submodules do not have one, so ConfigError becomes a safe no-op. The tool
    does not pick up the superproject's config and does not operate on
    superproject refs from inside a submodule.
    """
    try:
        out = subprocess.run(
            ["git", "worktree", "list", "--porcelain"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
        for line in out.stdout.splitlines():
            if line.startswith("worktree "):
                candidate = pathlib.Path(line[len("worktree "):].strip())
                if candidate.is_dir():
                    if (candidate / ".git").exists():
                        return candidate
                    try:
                        worktree_val = subprocess.run(
                            ["git", "-C", str(candidate), "config", "--get", "core.worktree"],
                            check=True, capture_output=True, text=True,
                        ).stdout.strip()
                        if worktree_val:
                            resolved = (candidate / worktree_val).resolve()
                            if (resolved / ".git").exists() or (resolved.is_dir() and (resolved / "config").exists()):
                                return resolved
                    except subprocess.CalledProcessError:
                        pass
    except subprocess.CalledProcessError:
        pass
    # Fallback for normal repos / linked worktrees (not submodules).
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--git-common-dir"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
        common_dir = pathlib.Path(out.stdout.strip())
        if not common_dir.is_absolute():
            common_dir = repo_root / common_dir
        common_dir = common_dir.resolve()
        candidate = common_dir.parent
        # Sanity check: parent should be a working tree, not inside .git/.
        if (candidate / ".git").exists() or (candidate.is_dir() and (candidate / "config").exists()):
            return candidate
    except subprocess.CalledProcessError:
        pass
    return repo_root


CONFIG_KEYS = frozenset({
    "schema_version",
    "clean-merged.enabled",
    "clean-merged.trunk_branch",
    "clean-merged.remote_name",
    "clean-merged.origin_owner",
    "clean-merged.lane_r.gh_timeout_s",
    "clean-merged.lane_r.cache_ttl_s",
    "clean-merged.lane_r.gh_limit",
    "clean-merged.lane_w.quarantine_dir",
    "clean-merged.lane_w.quarantine_grace_days",
    "clean-merged.lane_w.discard_ignored",
    "clean-merged.lane_w.remove_nested_repos",
    "clean-merged.lane_w.discard_hidden_index_bits",
    "clean-merged.lane_w.archive_timeout_s",
    "clean-merged.lane_w.archive_verify_timeout_s",
    "clean-merged.lane_t.target_dir_name",
    "clean-merged.lane_t.idle_after_days",
    "clean-merged.lane_t.active_process_patterns",
    "clean-merged.lane_t.process_list_timeout_s",
    "clean-merged.lane_t.cwd_visibility_timeout_s",
    "clean-merged.daily_maintenance_launch_agent.label",
    "clean-merged.daily_maintenance_launch_agent.program_arguments",
    "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Hour",
    "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Minute",
    "clean-merged.logging.audit_format",
    "clean-merged.logging.audit_path",
    "clean-merged.logging.max_log_bytes",
    "clean-merged.logging.rotated_log_retention_days",
    "clean-merged.logging.report_error_max_chars",
    "clean-merged.logging.heartbeat_path",
    "clean-merged.logging.heartbeat_stale_days",
    "clean-merged.logging.lane_r_log_path",
    "clean-merged.backups.prune_after_days",
})
OPTIONAL_CONFIG_KEY_PREFIXES = (
    "clean-merged.lane_t.",
    "clean-merged.daily_maintenance_launch_agent.",
)
REQUIRED_CONFIG_KEYS = frozenset(
    key for key in CONFIG_KEYS
    if not any(key.startswith(prefix) for prefix in OPTIONAL_CONFIG_KEY_PREFIXES)
)


def _flatten_config(data: dict[str, Any], prefix: str = "") -> dict[str, Any]:
    flat: dict[str, Any] = {}
    for key, value in data.items():
        dotted = f"{prefix}.{key}" if prefix else key
        if isinstance(value, dict):
            flat.update(_flatten_config(value, dotted))
        else:
            flat[dotted] = value
    return flat


def _config_str(flat: dict[str, Any], key: str) -> str:
    value = flat[key]
    if not isinstance(value, str) or value == "":
        raise ConfigError(f"invalid config: {key} must be a non-empty string")
    return value


def _config_bool(flat: dict[str, Any], key: str) -> bool:
    value = flat[key]
    if not isinstance(value, bool):
        raise ConfigError(f"invalid config: {key} must be bool")
    return value


def _config_positive_float(flat: dict[str, Any], key: str) -> float:
    value = flat[key]
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        raise ConfigError(f"invalid config: {key} must be a positive number")
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise ConfigError(f"invalid config: {key} must be a positive number")
    return result


def _config_positive_int(flat: dict[str, Any], key: str) -> int:
    value = flat[key]
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ConfigError(f"invalid config: {key} must be a positive integer")
    return value


def _config_int_range(flat: dict[str, Any], key: str, *, minimum: int, maximum: int) -> int:
    value = flat[key]
    if not isinstance(value, int) or isinstance(value, bool) or not minimum <= value <= maximum:
        raise ConfigError(f"invalid config: {key} must be an integer from {minimum} to {maximum}")
    return value


def _config_string_array(flat: dict[str, Any], key: str) -> tuple[str, ...]:
    value = flat[key]
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise ConfigError(f"invalid config: {key} must be a non-empty string array")
    return tuple(value)


def _config_single_path_name(flat: dict[str, Any], key: str) -> str:
    value = _config_str(flat, key)
    if value in {".", ".."} or "/" in value or "\\" in value:
        raise ConfigError(f"invalid config: {key} must be a single relative path name")
    return value


def load_config(repo_root: pathlib.Path) -> Config:
    """Load config from the MAIN worktree path (not the current worktree,
    which may be on a feature branch predating config/clean-merged.toml).

    Honors TOML nesting ([clean-merged.lane_r].gh_timeout_s). Every runtime
    key is required; code supplies no runtime defaults.
    """
    main_root = _main_worktree_root(repo_root)
    cfg_path = main_root / "config" / "clean-merged.toml"
    if not cfg_path.is_file():
        raise ConfigError(
            f"config not found: {cfg_path}. Run from a checkout that has "
            "config/clean-merged.toml (the main worktree)."
        )
    try:
        data = _load_toml(cfg_path)
    except _TOML_DECODE_ERROR as exc:
        raise ConfigError(f"invalid TOML in {cfg_path}: {exc}") from exc

    flat = _flatten_config(data)
    missing = sorted(REQUIRED_CONFIG_KEYS - set(flat))
    if missing:
        raise ConfigError(f"missing required config: {missing[0]}")
    unknown = sorted(set(flat) - CONFIG_KEYS)
    if unknown:
        raise ConfigError(f"unknown config key: {unknown[0]}")
    if flat["schema_version"] != 1:
        raise ConfigError(f"invalid config: schema_version expected 1, got {flat['schema_version']!r}")

    enabled = _config_bool(flat, "clean-merged.enabled")
    trunk_branch = _config_str(flat, "clean-merged.trunk_branch")
    remote_name = _config_str(flat, "clean-merged.remote_name")
    origin_owner = _config_str(flat, "clean-merged.origin_owner")

    lane_r = LaneRConfig(
        gh_timeout_s=_config_positive_float(flat, "clean-merged.lane_r.gh_timeout_s"),
        cache_ttl_s=_config_positive_float(flat, "clean-merged.lane_r.cache_ttl_s"),
        gh_limit=_config_positive_int(flat, "clean-merged.lane_r.gh_limit"),
    )
    lane_w = LaneWConfig(
        quarantine_dir=_config_str(flat, "clean-merged.lane_w.quarantine_dir"),
        quarantine_grace_days=_config_positive_int(
            flat, "clean-merged.lane_w.quarantine_grace_days"),
        discard_ignored=_config_bool(flat, "clean-merged.lane_w.discard_ignored"),
        remove_nested_repos=_config_bool(flat, "clean-merged.lane_w.remove_nested_repos"),
        discard_hidden_index_bits=_config_bool(
            flat, "clean-merged.lane_w.discard_hidden_index_bits"),
        archive_timeout_s=_config_positive_float(flat, "clean-merged.lane_w.archive_timeout_s"),
        archive_verify_timeout_s=_config_positive_float(
            flat, "clean-merged.lane_w.archive_verify_timeout_s"),
    )
    lane_t_keys = {
        "clean-merged.lane_t.target_dir_name",
        "clean-merged.lane_t.idle_after_days",
        "clean-merged.lane_t.active_process_patterns",
        "clean-merged.lane_t.process_list_timeout_s",
        "clean-merged.lane_t.cwd_visibility_timeout_s",
    }
    present_lane_t_keys = lane_t_keys & set(flat)
    if present_lane_t_keys and present_lane_t_keys != lane_t_keys:
        missing_lane_t = sorted(lane_t_keys - present_lane_t_keys)[0]
        raise ConfigError(f"missing required config: {missing_lane_t}")
    lane_t = None
    if present_lane_t_keys:
        lane_t = LaneTConfig(
            target_dir_name=_config_single_path_name(flat, "clean-merged.lane_t.target_dir_name"),
            idle_after_days=_config_positive_int(flat, "clean-merged.lane_t.idle_after_days"),
            active_process_patterns=_config_string_array(
                flat, "clean-merged.lane_t.active_process_patterns"),
            process_list_timeout_s=_config_positive_float(
                flat, "clean-merged.lane_t.process_list_timeout_s"),
            cwd_visibility_timeout_s=_config_positive_float(
                flat, "clean-merged.lane_t.cwd_visibility_timeout_s"),
        )
    daily_maintenance_keys = {
        "clean-merged.daily_maintenance_launch_agent.label",
        "clean-merged.daily_maintenance_launch_agent.program_arguments",
        "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Hour",
        "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Minute",
    }
    present_daily_maintenance_keys = daily_maintenance_keys & set(flat)
    if present_daily_maintenance_keys and present_daily_maintenance_keys != daily_maintenance_keys:
        missing_daily_maintenance = sorted(
            daily_maintenance_keys - present_daily_maintenance_keys)[0]
        raise ConfigError(f"missing required config: {missing_daily_maintenance}")
    if present_daily_maintenance_keys:
        _config_str(flat, "clean-merged.daily_maintenance_launch_agent.label")
        _config_string_array(flat, "clean-merged.daily_maintenance_launch_agent.program_arguments")
        _config_int_range(
            flat,
            "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Hour",
            minimum=0,
            maximum=23,
        )
        _config_int_range(
            flat,
            "clean-merged.daily_maintenance_launch_agent.start_calendar_interval.Minute",
            minimum=0,
            maximum=59,
        )
    logging_cfg = LoggingConfig(
        audit_format=_config_str(flat, "clean-merged.logging.audit_format"),
        audit_path=_config_str(flat, "clean-merged.logging.audit_path"),
        max_log_bytes=_config_positive_int(flat, "clean-merged.logging.max_log_bytes"),
        rotated_log_retention_days=_config_positive_int(
            flat, "clean-merged.logging.rotated_log_retention_days"),
        report_error_max_chars=_config_positive_int(
            flat, "clean-merged.logging.report_error_max_chars"),
        heartbeat_path=_config_str(flat, "clean-merged.logging.heartbeat_path"),
        heartbeat_stale_days=_config_positive_int(
            flat, "clean-merged.logging.heartbeat_stale_days"),
        lane_r_log_path=_config_str(flat, "clean-merged.logging.lane_r_log_path"),
    )
    backups = BackupsConfig(
        prune_after_days=_config_positive_int(flat, "clean-merged.backups.prune_after_days"),
    )

    return Config(
        enabled=enabled, trunk_branch=trunk_branch, remote_name=remote_name,
        lane_r=lane_r, lane_w=lane_w, lane_t=lane_t,
        logging=logging_cfg, backups=backups, origin_owner=origin_owner,
    )


# ---------------------------------------------------------------------------
# Git helpers


def git_common_dir(repo_root: pathlib.Path) -> pathlib.Path:
    out = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=repo_root, check=True, capture_output=True, text=True,
    )
    common_dir = pathlib.Path(out.stdout.strip())
    if not common_dir.is_absolute():
        common_dir = repo_root / common_dir
    return common_dir.resolve()


def _git(repo_root: pathlib.Path, args: list[str], *,
         check: bool = True, timeout: float | None = None,
         env: dict[str, str] | None = None,
         input: str | None = None) -> subprocess.CompletedProcess[str]:
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    return subprocess.run(
        ["git", *args], cwd=repo_root, check=check, capture_output=True,
        text=True, timeout=timeout, env=full_env, input=input,
    )


def _git_bytes(repo_root: pathlib.Path, args: list[str], *,
               check: bool = True) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", *args], cwd=repo_root, check=check, capture_output=True,
    )


def _resolve_home_dir() -> pathlib.Path | None:
    try:
        return pathlib.Path.home()
    except (KeyError, RuntimeError):
        return None


def _resolve_hooks_path(
    repo_root: pathlib.Path,
    raw: str,
    *,
    home_dir: pathlib.Path | None = None,
) -> pathlib.Path:
    if raw == "~" or raw.startswith("~/"):
        if home_dir is None:
            home_dir = _resolve_home_dir()
        if home_dir is not None:
            suffix = raw[2:] if raw.startswith("~/") else ""
            path = home_dir / suffix if suffix else home_dir
        else:
            path = pathlib.Path(raw)
    else:
        path = pathlib.Path(raw)
    return path if path.is_absolute() else repo_root / path


def _same_path(left: pathlib.Path, right: pathlib.Path) -> bool:
    try:
        return left.resolve() == right.resolve()
    except OSError:
        return left.absolute() == right.absolute()


@dataclasses.dataclass(frozen=True)
class ActiveHookDir:
    path: pathlib.Path
    source_scope: str


@dataclasses.dataclass(frozen=True)
class HookSnapshot:
    source_file: pathlib.Path
    hook_name: str
    content: bytes
    mode: int
    sha256: str


@dataclasses.dataclass(frozen=True)
class PlannedShadowCopy:
    destination: pathlib.Path
    snapshot: HookSnapshot


def _is_git_hook_name(name: str) -> bool:
    return name in GIT_HOOK_NAMES


def _git_config_value(repo_root: pathlib.Path, args: list[str]) -> str | None:
    proc = _git(repo_root, ["config", *args], check=False)
    if proc.returncode != 0 or not proc.stdout.strip():
        return None
    return proc.stdout.strip()


def _worktree_config_enabled(repo_root: pathlib.Path) -> bool:
    return _git_config_value(repo_root, ["--get", "extensions.worktreeConfig"]) == "true"


def _tracked_hook_source_paths(repo_root: pathlib.Path) -> list[str]:
    proc = _git(repo_root, ["ls-files", "-z", "--", ".githooks"], check=False)
    if proc.returncode != 0:
        return []
    return sorted(
        path for path in proc.stdout.split("\0")
        if pathlib.PurePosixPath(path).parent == pathlib.PurePosixPath(".githooks")
        and _is_git_hook_name(pathlib.PurePosixPath(path).name)
    )


def _dirty_tracked_hook_sources(repo_root: pathlib.Path) -> list[str]:
    rel_paths = _tracked_hook_source_paths(repo_root)
    if not rel_paths:
        return []
    dirty: set[str] = set()
    for args in (
        ["diff", "--name-only", "-z", "--", *rel_paths],
        ["diff", "--cached", "--name-only", "-z", "--", *rel_paths],
    ):
        proc = _git(repo_root, args, check=False)
        if proc.returncode == 0:
            dirty.update(path for path in proc.stdout.split("\0") if path in rel_paths)
    return sorted(dirty)


def _raise_if_dirty_tracked_hook_sources(repo_root: pathlib.Path) -> None:
    dirty_sources = _dirty_tracked_hook_sources(repo_root)
    if dirty_sources:
        raise CleanMergedError(
            "tracked hook source(s) have local changes: "
            + ", ".join(dirty_sources)
            + "; restore or commit them before installing hooks"
        )


def _hook_manifest_path(common_dir: pathlib.Path) -> pathlib.Path:
    return common_dir / HOOK_MANIFEST_NAME


def _manifest_invalid(manifest_path: pathlib.Path, message: str) -> CleanMergedError:
    return CleanMergedError(f"hook manifest invalid at {manifest_path}: {message}")


def _manifest_required_string(
    manifest_path: pathlib.Path,
    entry: dict[str, Any],
    field: str,
    label: str,
) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value:
        raise _manifest_invalid(manifest_path, f"{label}.{field} must be non-empty string")
    return value


def _validate_manifest_sha256(
    manifest_path: pathlib.Path,
    entry: dict[str, Any],
    field: str,
    label: str,
) -> None:
    value = entry.get(field)
    if not isinstance(value, str) or SHA256_RE.fullmatch(value) is None:
        raise _manifest_invalid(manifest_path, f"{label}.{field} must be sha256 hex")


def _validate_hook_manifest_hook_entry(
    manifest_path: pathlib.Path,
    hook_name: str,
    entry: Any,
) -> None:
    label = f"hooks.{hook_name}"
    if not _is_git_hook_name(hook_name):
        raise _manifest_invalid(manifest_path, f"{label} must be known Git hook name")
    if not isinstance(entry, dict):
        raise _manifest_invalid(manifest_path, f"{label} must be object")
    source_kind = entry.get("source_kind")
    if source_kind not in {"repo-source", "active-hook"}:
        raise _manifest_invalid(manifest_path, f"{label} source_kind is invalid")
    source_scope = entry.get("source_scope")
    if source_kind == "repo-source":
        if source_scope != "repo":
            raise _manifest_invalid(manifest_path, f"{label} source_scope must be repo")
    elif source_scope not in HOOK_MANIFEST_ACTIVE_SCOPES:
        raise _manifest_invalid(
            manifest_path,
            f"{label} unsupported source_scope {source_scope}",
        )
    _manifest_required_string(manifest_path, entry, "source_path", label)
    _validate_manifest_sha256(manifest_path, entry, "source_sha256", label)
    _validate_manifest_sha256(manifest_path, entry, "runtime_sha256", label)


def _validate_hook_manifest_shadowed_entry(
    manifest_path: pathlib.Path,
    hook_name: str,
    entries: Any,
) -> None:
    label = f"shadowed_hooks.{hook_name}"
    if not _is_git_hook_name(hook_name):
        raise _manifest_invalid(manifest_path, f"{label} must be known Git hook name")
    if not isinstance(entries, list):
        raise _manifest_invalid(manifest_path, f"{label} must be array")
    for index, entry in enumerate(entries):
        entry_label = f"{label}[{index}]"
        if not isinstance(entry, dict):
            raise _manifest_invalid(manifest_path, f"{entry_label} must be object")
        if entry.get("source_kind") != "active-hook":
            raise _manifest_invalid(manifest_path, f"{entry_label} source_kind must be active-hook")
        source_scope = entry.get("source_scope")
        if source_scope not in HOOK_MANIFEST_ACTIVE_SCOPES:
            raise _manifest_invalid(
                manifest_path,
                f"{entry_label} unsupported source_scope {source_scope}",
            )
        _manifest_required_string(manifest_path, entry, "source_path", entry_label)
        _validate_manifest_sha256(manifest_path, entry, "source_sha256", entry_label)
        _manifest_required_string(manifest_path, entry, "shadowed_by", entry_label)


def _validate_hook_manifest_source_dir_entry(
    manifest_path: pathlib.Path,
    index: int,
    entry: Any,
) -> None:
    label = f"source_dirs[{index}]"
    if not isinstance(entry, dict):
        raise _manifest_invalid(manifest_path, f"{label} must be object")
    source_scope = entry.get("source_scope")
    if source_scope not in HOOK_MANIFEST_SOURCE_DIR_SCOPES:
        raise _manifest_invalid(
            manifest_path,
            f"{label} unsupported source_scope {source_scope}",
        )
    _manifest_required_string(manifest_path, entry, "source_path", label)


def _load_hook_manifest(common_dir: pathlib.Path) -> dict[str, Any]:
    manifest_path = _hook_manifest_path(common_dir)
    if not manifest_path.is_file():
        return {"version": 1, "hooks": {}, "source_dirs": []}
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CleanMergedError(f"hook manifest unreadable at {manifest_path}: {exc}") from exc
    if not isinstance(manifest, dict):
        raise CleanMergedError(f"hook manifest invalid at {manifest_path}: root must be object")
    if "hooks" not in manifest:
        manifest["hooks"] = {}
    elif not isinstance(manifest["hooks"], dict):
        raise CleanMergedError(f"hook manifest invalid at {manifest_path}: hooks must be object")
    if "shadowed_hooks" not in manifest:
        manifest["shadowed_hooks"] = {}
    elif not isinstance(manifest["shadowed_hooks"], dict):
        raise CleanMergedError(
            f"hook manifest invalid at {manifest_path}: shadowed_hooks must be object"
        )
    if "source_dirs" not in manifest:
        manifest["source_dirs"] = []
    elif not isinstance(manifest["source_dirs"], list):
        raise CleanMergedError(
            f"hook manifest invalid at {manifest_path}: source_dirs must be array"
        )
    for hook_name, entry in manifest["hooks"].items():
        _validate_hook_manifest_hook_entry(manifest_path, hook_name, entry)
    for hook_name, entries in manifest["shadowed_hooks"].items():
        _validate_hook_manifest_shadowed_entry(manifest_path, hook_name, entries)
    for index, entry in enumerate(manifest["source_dirs"]):
        _validate_hook_manifest_source_dir_entry(manifest_path, index, entry)
    return manifest


def _hook_manifest_hooks(manifest: dict[str, Any]) -> dict[str, Any]:
    hooks = manifest.get("hooks")
    return hooks if isinstance(hooks, dict) else {}


def _hook_manifest_shadowed(manifest: dict[str, Any]) -> dict[str, Any]:
    shadowed = manifest.get("shadowed_hooks")
    return shadowed if isinstance(shadowed, dict) else {}


def _hook_manifest_source_dirs(manifest: dict[str, Any]) -> list[Any]:
    source_dirs = manifest.get("source_dirs")
    return source_dirs if isinstance(source_dirs, list) else []


def _write_hook_manifest(
    common_dir: pathlib.Path,
    *,
    runtime_hooks_dir: pathlib.Path,
    hooks: dict[str, Any],
    shadowed_hooks: dict[str, Any],
    source_dirs: list[dict[str, str]],
) -> None:
    manifest = {
        "version": 1,
        "runtime_hooks_dir": str(runtime_hooks_dir),
        "hooks": hooks,
        "shadowed_hooks": shadowed_hooks,
        "source_dirs": source_dirs,
    }
    _atomic_write_text(
        _hook_manifest_path(common_dir),
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
    )


def _file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _bytes_sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_hook_snapshot(source_file: pathlib.Path) -> HookSnapshot:
    if source_file.is_symlink():
        raise CleanMergedError(f"refusing to install symlink hook source at {source_file}")
    try:
        source_stat = source_file.stat()
        if not stat.S_ISREG(source_stat.st_mode):
            raise CleanMergedError(f"refusing to install non-file hook source at {source_file}")
        source_bytes = source_file.read_bytes()
    except OSError as exc:
        raise CleanMergedError(
            f"hook source changed during install planning at {source_file}; retry setup"
        ) from exc
    return HookSnapshot(
        source_file=source_file,
        hook_name=source_file.name,
        content=source_bytes,
        mode=stat.S_IMODE(source_stat.st_mode),
        sha256=_bytes_sha256(source_bytes),
    )


def _tracked_hook_snapshot(repo_root: pathlib.Path, rel_path: str) -> HookSnapshot:
    source_file = repo_root / rel_path
    tree_entry = _git(repo_root, ["ls-tree", "HEAD", "--", rel_path], check=False)
    if tree_entry.returncode != 0 or not tree_entry.stdout.strip():
        raise CleanMergedError(f"tracked hook source missing from HEAD: {rel_path}")
    mode_text = tree_entry.stdout.split(maxsplit=1)[0]
    if mode_text == "120000":
        raise CleanMergedError(f"refusing to install symlink hook source at {source_file}")
    if mode_text not in {"100644", "100755"}:
        raise CleanMergedError(f"refusing to install non-file hook source at {source_file}")
    blob = _git_bytes(repo_root, ["show", f"HEAD:{rel_path}"], check=False)
    if blob.returncode != 0:
        raise CleanMergedError(f"tracked hook source missing from HEAD: {rel_path}")
    content = blob.stdout
    return HookSnapshot(
        source_file=source_file,
        hook_name=source_file.name,
        content=content,
        mode=0o755 if mode_text == "100755" else 0o644,
        sha256=_bytes_sha256(content),
    )


def _repo_relative_path(repo_root: pathlib.Path, path: pathlib.Path) -> str:
    try:
        return str(path.relative_to(repo_root))
    except ValueError:
        return str(path)


def _is_executable(path: pathlib.Path) -> bool:
    try:
        return bool(path.stat().st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH))
    except OSError:
        return False


def _manifest_authorizes_overwrite(
    entry: Any,
    *,
    destination_sha: str,
    source_kind: str,
    source_scope: str,
    source_path: str,
) -> bool:
    return (
        isinstance(entry, dict)
        and entry.get("source_kind") == source_kind
        and entry.get("source_scope") == source_scope
        and (
            entry.get("source_path") == source_path
            or source_scope in {"global", "local", "worktree", "system"}
        )
        and entry.get("runtime_sha256") == destination_sha
    )


def _record_hook_provenance(
    *,
    source_file: pathlib.Path,
    destination: pathlib.Path,
    source_kind: str,
    source_scope: str,
    source_path: str,
    manifest_hooks: dict[str, Any],
) -> None:
    source_sha = _file_sha256(source_file)
    manifest_hooks[destination.name] = {
        "source_kind": source_kind,
        "source_scope": source_scope,
        "source_path": source_path,
        "source_sha256": source_sha,
        "runtime_sha256": _file_sha256(destination),
    }


def _record_planned_hook_provenance(
    *,
    destination_name: str,
    source_kind: str,
    source_scope: str,
    source_path: str,
    source_sha: str,
    manifest_hooks: dict[str, Any],
) -> None:
    manifest_hooks[destination_name] = {
        "source_kind": source_kind,
        "source_scope": source_scope,
        "source_path": source_path,
        "source_sha256": source_sha,
        "runtime_sha256": source_sha,
    }


def _validate_hook_copy_with_provenance(
    *,
    source_file: pathlib.Path,
    destination: pathlib.Path,
    source_kind: str,
    source_scope: str,
    source_path: str,
    manifest_hooks: dict[str, Any],
    source_sha: str | None = None,
) -> None:
    if source_file.is_symlink():
        raise CleanMergedError(f"refusing to install symlink hook source at {source_file}")
    if destination.is_symlink():
        raise CleanMergedError(f"refusing to overwrite non-file hook at {destination}")
    if _same_path(source_file, destination):
        if not source_file.is_file():
            raise CleanMergedError(f"refusing to install non-file hook source at {source_file}")
        current_sha = _file_sha256(source_file)
        entry = manifest_hooks.get(destination.name)
        if isinstance(entry, dict) and entry.get("runtime_sha256") != current_sha:
            raise CleanMergedError(
                f"refusing to adopt modified runtime hook {destination.name} "
                f"without installer provenance at {destination}"
            )
        return
    source_sha = source_sha or _file_sha256(source_file)
    if destination.exists():
        if not destination.is_file():
            raise CleanMergedError(f"refusing to overwrite non-file hook at {destination}")
        destination_sha = _file_sha256(destination)
        if (
            destination_sha != source_sha
            and not _manifest_authorizes_overwrite(
                manifest_hooks.get(destination.name),
                destination_sha=destination_sha,
                source_kind=source_kind,
                source_scope=source_scope,
                source_path=source_path,
            )
        ):
            raise CleanMergedError(
                f"refusing to overwrite hook {destination.name} without "
                f"installer provenance at {destination}"
            )


def _apply_hook_snapshot(
    *,
    destination: pathlib.Path,
    source_bytes: bytes,
    source_mode: int,
    source_kind: str,
    source_sha: str,
    same_path: bool,
) -> None:
    if same_path:
        if _file_sha256(destination) != source_sha:
            raise CleanMergedError(
                f"hook source changed during install planning at {destination}; retry setup"
            )
        if source_kind == "repo-source":
            destination.chmod(
                destination.stat().st_mode
                | stat.S_IXUSR
                | stat.S_IXGRP
                | stat.S_IXOTH
            )
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    mode = source_mode
    if source_kind == "repo-source":
        mode |= stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    tmp = destination.with_name(f".{destination.name}.tmp.{os.getpid()}")
    try:
        tmp.write_bytes(source_bytes)
        tmp.chmod(mode)
        os.replace(str(tmp), str(destination))
    except BaseException:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def _shadowed_hook_destination(
    common_dir: pathlib.Path,
    hook_file: pathlib.Path,
) -> pathlib.Path:
    source_sha = _read_hook_snapshot(hook_file).sha256
    return _shadowed_hook_destination_for_sha(common_dir, hook_file.name, source_sha)


def _shadowed_hook_destination_for_sha(
    common_dir: pathlib.Path,
    hook_name: str,
    source_sha: str,
) -> pathlib.Path:
    shadow_dir = common_dir / "clean-merged.shadowed-hooks"
    return shadow_dir / f"{hook_name}.{source_sha}"


def _validate_shadowed_hook_copy(
    common_dir: pathlib.Path,
    hook_file: pathlib.Path,
) -> pathlib.Path:
    if hook_file.is_symlink():
        raise CleanMergedError(f"refusing to shadow symlink hook at {hook_file}")
    destination = _shadowed_hook_destination(common_dir, hook_file)
    if destination.is_symlink():
        raise CleanMergedError(f"refusing to shadow into symlink hook at {destination}")
    if destination.exists() and not destination.is_file():
        raise CleanMergedError(f"refusing to shadow into non-file hook at {destination}")
    if destination.exists() and _file_sha256(destination) != _file_sha256(hook_file):
        raise CleanMergedError(f"refusing to shadow into mismatched hook at {destination}")
    return destination


def _apply_shadowed_hook_snapshot(
    *,
    destination: pathlib.Path,
    source_bytes: bytes,
    source_mode: int,
    source_sha: str,
) -> None:
    if destination.exists():
        if _file_sha256(destination) != source_sha:
            raise CleanMergedError(f"shadowed hook copy changed since install at {destination}")
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    tmp = destination.with_name(f".{destination.name}.tmp.{os.getpid()}")
    try:
        tmp.write_bytes(source_bytes)
        tmp.chmod(source_mode)
        os.replace(str(tmp), str(destination))
    except BaseException:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def _is_shadowed_hook_backup_path(common_dir: pathlib.Path, path: pathlib.Path) -> bool:
    return _same_path(path.parent, common_dir / "clean-merged.shadowed-hooks")


def _validate_hook_removal_with_provenance(
    *,
    runtime_hooks_dir: pathlib.Path,
    hook_name: str,
    entry: dict[str, Any],
) -> None:
    destination = runtime_hooks_dir / hook_name
    if not destination.exists() and not destination.is_symlink():
        return
    if destination.is_symlink() or not destination.is_file():
        raise CleanMergedError(f"refusing to remove non-file hook at {destination}")
    if entry.get("runtime_sha256") != _file_sha256(destination):
        raise CleanMergedError(
            f"refusing to remove hook {hook_name} without "
            f"installer provenance at {destination}"
        )


def _validate_active_hook_runtime_unchanged(
    *,
    runtime_hooks_dir: pathlib.Path,
    hook_name: str,
    entry: dict[str, Any],
) -> None:
    destination = runtime_hooks_dir / hook_name
    if (
        destination.exists()
        and destination.is_file()
        and entry.get("runtime_sha256") != _file_sha256(destination)
    ):
        raise CleanMergedError(
            f"refusing to adopt modified runtime hook {hook_name} "
            f"without installer provenance at {destination}"
        )


def _apply_hook_removal(destination: pathlib.Path) -> None:
    if not destination.exists() and not destination.is_symlink():
        return
    destination.unlink()


def _install_plan_path_key(path: pathlib.Path) -> str:
    try:
        return str(path.resolve())
    except OSError:
        return str(path.absolute())


@dataclasses.dataclass
class HookInstallPlan:
    preflight_operations: list[Callable[[], None]] = dataclasses.field(
        default_factory=list
    )
    operations: list[Callable[[], None]] = dataclasses.field(default_factory=list)
    unlinked_paths: set[str] = dataclasses.field(default_factory=set)

    def will_unlink(self, path: pathlib.Path) -> bool:
        return _install_plan_path_key(path) in self.unlinked_paths

    def stage_shadow_copy(
        self,
        *,
        common_dir: pathlib.Path,
        hook_file: pathlib.Path,
        source_snapshot: HookSnapshot | None = None,
    ) -> PlannedShadowCopy:
        destination = _validate_shadowed_hook_copy(common_dir, hook_file)
        snapshot = source_snapshot or _read_hook_snapshot(hook_file)
        if destination != _shadowed_hook_destination_for_sha(
            common_dir,
            hook_file.name,
            snapshot.sha256,
        ):
            raise CleanMergedError(
                f"hook {hook_file.name} changed during install planning at {hook_file}; "
                "retry setup"
            )
        self.preflight_operations.append(
            lambda common_dir=common_dir, hook_file=hook_file, destination=destination:
            _validate_planned_shadow_copy(common_dir, hook_file, destination)
        )
        if not destination.exists():
            self.operations.append(
                lambda destination=destination, snapshot=snapshot:
                _apply_shadowed_hook_snapshot(
                    destination=destination,
                    source_bytes=snapshot.content,
                    source_mode=snapshot.mode,
                    source_sha=snapshot.sha256,
                )
            )
        return PlannedShadowCopy(destination=destination, snapshot=snapshot)

    def stage_unlink(self, path: pathlib.Path) -> None:
        self.unlinked_paths.add(_install_plan_path_key(path))
        self.operations.append(lambda path=path: _apply_hook_removal(path))

    def stage_copy_hook(
        self,
        *,
        source_file: pathlib.Path,
        destination: pathlib.Path,
        source_kind: str,
        source_scope: str,
        source_path: str,
        manifest_hooks: dict[str, Any],
        source_snapshot: HookSnapshot | None = None,
    ) -> None:
        validation_manifest_hooks = dict(manifest_hooks)
        snapshot = source_snapshot or _read_hook_snapshot(source_file)
        destination_will_be_unlinked = (
            self.will_unlink(destination) and not _same_path(source_file, destination)
        )
        if destination_will_be_unlinked:
            if source_file.is_symlink():
                raise CleanMergedError(f"refusing to install symlink hook source at {source_file}")
            if not source_file.is_file():
                raise CleanMergedError(f"refusing to install non-file hook source at {source_file}")
        else:
            _validate_hook_copy_with_provenance(
                source_file=source_file,
                destination=destination,
                source_kind=source_kind,
                source_scope=source_scope,
                source_path=source_path,
                manifest_hooks=validation_manifest_hooks,
                source_sha=snapshot.sha256,
            )
        _record_planned_hook_provenance(
            destination_name=destination.name,
            source_kind=source_kind,
            source_scope=source_scope,
            source_path=source_path,
            source_sha=snapshot.sha256,
            manifest_hooks=manifest_hooks,
        )
        self.preflight_operations.append(
            lambda source_file=source_file, destination=destination, source_kind=source_kind,
            source_scope=source_scope, source_path=source_path,
            manifest_hooks=validation_manifest_hooks,
            destination_will_be_unlinked=destination_will_be_unlinked,
            source_sha=snapshot.sha256:
            _validate_planned_hook_copy(
                source_file=source_file,
                destination=destination,
                source_kind=source_kind,
                source_scope=source_scope,
                source_path=source_path,
                manifest_hooks=manifest_hooks,
                destination_will_be_unlinked=destination_will_be_unlinked,
                source_sha=source_sha,
            )
        )
        self.unlinked_paths.discard(_install_plan_path_key(destination))
        same_path = _same_path(source_file, destination)
        self.operations.append(
            lambda destination=destination, snapshot=snapshot,
            source_kind=source_kind, source_sha=snapshot.sha256,
            same_path=same_path:
            _apply_hook_snapshot(
                destination=destination,
                source_bytes=snapshot.content,
                source_mode=snapshot.mode,
                source_kind=source_kind,
                source_sha=source_sha,
                same_path=same_path,
            )
        )

    def stage_validate_clean_tracked_sources(
        self,
        *,
        repo_root: pathlib.Path,
    ) -> None:
        self.preflight_operations.insert(
            0,
            lambda repo_root=repo_root: _raise_if_dirty_tracked_hook_sources(repo_root)
        )

    def stage_remove_hook(
        self,
        *,
        runtime_hooks_dir: pathlib.Path,
        hook_name: str,
        entry: dict[str, Any],
    ) -> None:
        _validate_hook_removal_with_provenance(
            runtime_hooks_dir=runtime_hooks_dir,
            hook_name=hook_name,
            entry=entry,
        )
        self.preflight_operations.append(
            lambda runtime_hooks_dir=runtime_hooks_dir, hook_name=hook_name, entry=entry:
            _validate_hook_removal_with_provenance(
                runtime_hooks_dir=runtime_hooks_dir,
                hook_name=hook_name,
                entry=entry,
            )
        )
        destination = runtime_hooks_dir / hook_name
        self.unlinked_paths.add(_install_plan_path_key(destination))
        self.operations.append(lambda destination=destination: _apply_hook_removal(destination))

    def stage_set_runtime_hooks_path(
        self,
        *,
        invoke_root: pathlib.Path,
        source_root: pathlib.Path,
        runtime_hooks_dir: pathlib.Path,
        home_dir: pathlib.Path | None,
    ) -> None:
        self.operations.append(
            lambda: _set_runtime_hooks_path(
                invoke_root=invoke_root,
                source_root=source_root,
                runtime_hooks_dir=runtime_hooks_dir,
                home_dir=home_dir,
            )
        )

    def stage_write_hook_manifest(
        self,
        common_dir: pathlib.Path,
        *,
        runtime_hooks_dir: pathlib.Path,
        hooks: dict[str, Any],
        shadowed_hooks: dict[str, Any],
        source_dirs: list[dict[str, str]],
    ) -> None:
        planned_hooks = dict(hooks)
        planned_shadowed_hooks = dict(shadowed_hooks)
        planned_source_dirs = list(source_dirs)
        self.operations.append(
            lambda: _write_hook_manifest(
                common_dir,
                runtime_hooks_dir=runtime_hooks_dir,
                hooks=planned_hooks,
                shadowed_hooks=planned_shadowed_hooks,
                source_dirs=planned_source_dirs,
            )
        )

    def apply(self) -> None:
        for operation in self.preflight_operations:
            operation()
        for operation in self.operations:
            operation()


def _validate_planned_shadow_copy(
    common_dir: pathlib.Path,
    hook_file: pathlib.Path,
    expected_destination: pathlib.Path,
) -> None:
    destination = _validate_shadowed_hook_copy(common_dir, hook_file)
    if not _same_path(destination, expected_destination):
        raise CleanMergedError(
            f"hook {hook_file.name} changed during install planning at {hook_file}; "
            "retry setup"
        )


def _validate_planned_hook_copy(
    *,
    source_file: pathlib.Path,
    destination: pathlib.Path,
    source_kind: str,
    source_scope: str,
    source_path: str,
    manifest_hooks: dict[str, Any],
    destination_will_be_unlinked: bool,
    source_sha: str,
) -> None:
    if destination_will_be_unlinked:
        if source_file.is_symlink():
            raise CleanMergedError(f"refusing to install symlink hook source at {source_file}")
        if not source_file.is_file():
            raise CleanMergedError(f"refusing to install non-file hook source at {source_file}")
    else:
        _validate_hook_copy_with_provenance(
            source_file=source_file,
            destination=destination,
            source_kind=source_kind,
            source_scope=source_scope,
            source_path=source_path,
            manifest_hooks=manifest_hooks,
            source_sha=source_sha,
        )
    current_sha = _read_hook_snapshot(source_file).sha256
    if current_sha != source_sha:
        raise CleanMergedError(
            f"hook source changed during install planning at {source_file}; retry setup"
        )


def _manifest_source_file(
    repo_root: pathlib.Path,
    entry: Any,
    *,
    hook_name: str | None = None,
    invoke_root: pathlib.Path | None = None,
    runtime_hooks_dir: pathlib.Path | None = None,
    home_dir: pathlib.Path | None = None,
) -> pathlib.Path | None:
    if not isinstance(entry, dict):
        return None
    raw = entry.get("source_path")
    if not isinstance(raw, str) or not raw:
        return None
    path = pathlib.Path(raw)
    manifest_path = path if path.is_absolute() else repo_root / path
    hook_scope = str(entry.get("source_scope") or "manifest")
    if hook_name is None:
        return manifest_path
    current_dir = _current_hooks_dir_for_scope(
        hook_scope,
        repo_root=repo_root,
        invoke_root=invoke_root or repo_root,
        home_dir=home_dir,
    )
    if hook_scope in {"global", "system"}:
        return None if current_dir is None else current_dir / hook_name
    if hook_scope in {"local", "worktree"}:
        if current_dir is None:
            return None
        if runtime_hooks_dir is not None and _same_path(current_dir, runtime_hooks_dir):
            return manifest_path
        return current_dir / hook_name
    return manifest_path


def _record_planned_shadowed_hook(
    *,
    hook_file: pathlib.Path,
    shadow_copy: PlannedShadowCopy,
    repo_source_snapshot: HookSnapshot,
    source_scope: str,
    shadowed_hooks: dict[str, Any],
    hook_name: str | None = None,
    source_path: str | None = None,
) -> None:
    _record_shadowed_hook_snapshot(
        hook_file=hook_file,
        source_snapshot=shadow_copy.snapshot,
        repo_source_snapshot=repo_source_snapshot,
        source_scope=source_scope,
        shadowed_hooks=shadowed_hooks,
        hook_name=hook_name,
        source_path=source_path,
    )


def _record_shadowed_hook_snapshot(
    *,
    hook_file: pathlib.Path,
    source_snapshot: HookSnapshot,
    repo_source_snapshot: HookSnapshot,
    source_scope: str,
    shadowed_hooks: dict[str, Any],
    hook_name: str | None = None,
    source_path: str | None = None,
) -> None:
    if source_snapshot.content == repo_source_snapshot.content:
        return
    manifest_hook_name = hook_name or hook_file.name
    _record_shadowed_hook_entry(
        manifest_hook_name=manifest_hook_name,
        source_path=source_path or str(hook_file),
        source_sha=source_snapshot.sha256,
        repo_source_file=repo_source_snapshot.source_file,
        shadowed_hooks=shadowed_hooks,
        source_scope=source_scope,
    )


def _record_shadowed_hook_entry(
    *,
    manifest_hook_name: str,
    source_path: str,
    source_sha: str,
    repo_source_file: pathlib.Path,
    shadowed_hooks: dict[str, Any],
    source_scope: str,
) -> None:
    entries = shadowed_hooks.setdefault(manifest_hook_name, [])
    if not isinstance(entries, list):
        entries = []
        shadowed_hooks[manifest_hook_name] = entries
    replacement = {
        "source_kind": "active-hook",
        "source_scope": source_scope,
        "source_path": source_path,
        "source_sha256": source_sha,
        "shadowed_by": _repo_relative_path(repo_source_file.parent.parent, repo_source_file),
    }
    for index, entry in enumerate(entries):
        if isinstance(entry, dict) and entry.get("source_path") == source_path:
            entries[index] = replacement
            break
    else:
        entries.append(replacement)


def _validate_default_shadowed_hook_backup(
    *,
    hook_name: str,
    entry: dict[str, Any],
    source_file: pathlib.Path,
) -> None:
    if source_file.is_symlink() or not source_file.is_file():
        raise CleanMergedError(
            f"shadowed hook {hook_name} backup missing at {source_file}; "
            "repair or remove hook manifest before running setup"
        )
    if entry.get("source_sha256") != _file_sha256(source_file):
        raise CleanMergedError(
            f"shadowed hook {hook_name} backup changed since install at {source_file}; "
            "repair or remove hook manifest before running setup"
        )


def _preflight_default_shadow_backups(
    *,
    manifest: dict[str, Any],
    source_root: pathlib.Path,
    invoke_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    common_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> None:
    for hook_name, entries in sorted(_hook_manifest_shadowed(manifest).items()):
        if not isinstance(entries, list):
            raise CleanMergedError(f"shadowed hook manifest entry for {hook_name} is invalid")
        for entry in entries:
            if not isinstance(entry, dict):
                raise CleanMergedError(
                    f"shadowed hook manifest entry for {hook_name} is invalid"
                )
            if entry.get("source_scope") != "default":
                continue
            source_file = _manifest_source_file(
                source_root,
                entry,
                hook_name=hook_name,
                invoke_root=invoke_root,
                runtime_hooks_dir=runtime_hooks_dir,
                home_dir=home_dir,
            )
            if source_file is None:
                raise CleanMergedError(
                    f"shadowed hook manifest entry for {hook_name} has invalid source_path"
                )
            _validate_default_shadowed_hook_backup(
                hook_name=hook_name,
                entry=entry,
                source_file=source_file,
            )

    for hook_name, entry in sorted(_hook_manifest_hooks(manifest).items()):
        if (
            not isinstance(entry, dict)
            or entry.get("source_kind") != "active-hook"
            or entry.get("source_scope") != "default"
        ):
            continue
        source_file = _manifest_source_file(
            source_root,
            entry,
            hook_name=hook_name,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        )
        if source_file is None:
            raise CleanMergedError(f"hook manifest entry for {hook_name} has invalid source_path")
        if not _is_shadowed_hook_backup_path(common_dir, source_file):
            continue
        _validate_default_shadowed_hook_backup(
            hook_name=hook_name,
            entry=entry,
            source_file=source_file,
        )


def _preflight_manifest_hook_runtime_state(
    *,
    manifest: dict[str, Any],
    source_root: pathlib.Path,
    invoke_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    common_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
    source_hook_names: set[str],
) -> None:
    manifest_hooks = _hook_manifest_hooks(manifest)
    source_hooks_dir = source_root / ".githooks"
    removable_scopes = {"global", "local", "worktree", "system"}
    for hook_name, entry in sorted(manifest_hooks.items()):
        if not isinstance(entry, dict):
            raise CleanMergedError(f"hook manifest entry for {hook_name} is invalid")
        source_kind = entry.get("source_kind")
        if source_kind == "repo-source":
            if hook_name in source_hook_names:
                source_file = source_hooks_dir / hook_name
                _validate_hook_copy_with_provenance(
                    source_file=source_file,
                    destination=runtime_hooks_dir / hook_name,
                    source_kind="repo-source",
                    source_scope="repo",
                    source_path=_repo_relative_path(source_root, source_file),
                    manifest_hooks=manifest_hooks,
                )
            else:
                _validate_hook_removal_with_provenance(
                    runtime_hooks_dir=runtime_hooks_dir,
                    hook_name=hook_name,
                    entry=entry,
                )
            continue
        if source_kind != "active-hook":
            raise CleanMergedError(f"hook manifest entry for {hook_name} is invalid")

        source_file = _manifest_source_file(
            source_root,
            entry,
            hook_name=hook_name,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        )
        source_scope = str(entry.get("source_scope") or "manifest")
        if source_file is None:
            if entry.get("source_scope") in removable_scopes:
                _validate_active_hook_runtime_unchanged(
                    runtime_hooks_dir=runtime_hooks_dir,
                    hook_name=hook_name,
                    entry=entry,
                )
                _validate_hook_removal_with_provenance(
                    runtime_hooks_dir=runtime_hooks_dir,
                    hook_name=hook_name,
                    entry=entry,
                )
                continue
            raise CleanMergedError(f"hook manifest entry for {hook_name} has invalid source_path")
        if source_scope == "default" and _is_shadowed_hook_backup_path(common_dir, source_file):
            _validate_default_shadowed_hook_backup(
                hook_name=hook_name,
                entry=entry,
                source_file=source_file,
            )
        if not source_file.is_file():
            _validate_active_hook_runtime_unchanged(
                runtime_hooks_dir=runtime_hooks_dir,
                hook_name=hook_name,
                entry=entry,
            )
            _validate_hook_removal_with_provenance(
                runtime_hooks_dir=runtime_hooks_dir,
                hook_name=hook_name,
                entry=entry,
            )
            continue
        if hook_name in source_hook_names:
            if source_file.is_symlink():
                raise CleanMergedError(f"refusing to record symlink hook at {source_file}")
            continue
        destination = runtime_hooks_dir / hook_name
        _validate_active_hook_runtime_unchanged(
            runtime_hooks_dir=runtime_hooks_dir,
            hook_name=hook_name,
            entry=entry,
        )
        _validate_hook_copy_with_provenance(
            source_file=source_file,
            destination=destination,
            source_kind="active-hook",
            source_scope=source_scope,
            source_path=str(source_file),
            manifest_hooks=manifest_hooks,
        )


def _preflight_fresh_shadow_collisions(
    *,
    active_hook_dirs: list[ActiveHookDir],
    source_hook_snapshots_by_name: dict[str, HookSnapshot],
    manifest_hooks: dict[str, Any],
    runtime_hooks_dir: pathlib.Path,
    common_dir: pathlib.Path,
) -> None:
    seen: set[pathlib.Path] = set()
    candidate_dirs = [
        runtime_hooks_dir,
        *(active_hooks.path for active_hooks in active_hook_dirs),
    ]
    source_hook_names = set(source_hook_snapshots_by_name)
    for active_hooks_dir in candidate_dirs:
        if not active_hooks_dir.is_dir():
            continue
        try:
            active_dir_key = active_hooks_dir.resolve()
        except OSError:
            active_dir_key = active_hooks_dir.absolute()
        if active_dir_key in seen:
            continue
        seen.add(active_dir_key)
        for hook_file in sorted(active_hooks_dir.iterdir(), key=lambda path: path.name):
            if (
                not _is_git_hook_name(hook_file.name)
                or hook_file.name not in source_hook_names
            ):
                continue
            if hook_file.is_symlink() or not hook_file.is_file():
                _validate_shadowed_hook_copy(common_dir, hook_file)
                continue
            if hook_file.name in manifest_hooks:
                continue
            snapshot = _read_hook_snapshot(hook_file)
            if snapshot.content == source_hook_snapshots_by_name[hook_file.name].content:
                continue
            _validate_shadowed_hook_copy(common_dir, hook_file)


def _configured_hooks_path(
    *,
    invoke_root: pathlib.Path,
    source_root: pathlib.Path,
) -> tuple[str, str | None]:
    effective_raw = _git_config_value(invoke_root, ["--get", "core.hooksPath"])
    if effective_raw is None:
        return "default", None
    scoped = _git(
        invoke_root,
        ["config", "--show-scope", "--get", "core.hooksPath"],
        check=False,
    )
    if scoped.returncode != 0 or not scoped.stdout.strip():
        raise CleanMergedError(
            "core.hooksPath is configured but Git did not report its config scope; "
            "set it in repo-local, worktree, global, or system config before installing "
            "clean-merged hooks"
        )
    parts = scoped.stdout.splitlines()[0].split(None, 1)
    if len(parts) != 2:
        raise CleanMergedError(
            "core.hooksPath config scope output was not understood; set it in "
            "repo-local, worktree, global, or system config before installing "
            "clean-merged hooks"
        )
    source_scope, raw = parts
    if source_scope in {"worktree", "local", "global", "system"}:
        return source_scope, raw
    raise CleanMergedError(
        f"core.hooksPath comes from unsupported {source_scope} config scope; "
        "set it in repo-local, worktree, global, or system config before installing "
        "clean-merged hooks"
    )


def _current_hooks_dir_for_scope(
    source_scope: str,
    *,
    repo_root: pathlib.Path,
    invoke_root: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> pathlib.Path | None:
    if source_scope == "worktree":
        if not _worktree_config_enabled(invoke_root):
            return None
        raw = _git_config_value(invoke_root, ["--worktree", "--get", "core.hooksPath"])
        base = invoke_root
    elif source_scope == "local":
        raw = _git_config_value(repo_root, ["--local", "--get", "core.hooksPath"])
        base = repo_root
    elif source_scope == "global":
        raw = _git_config_value(invoke_root, ["--global", "--get", "core.hooksPath"])
        base = invoke_root
    elif source_scope == "system":
        raw = _git_config_value(invoke_root, ["--system", "--get", "core.hooksPath"])
        base = invoke_root
    else:
        return None
    if raw is None:
        return None
    return _resolve_hooks_path(base, raw, home_dir=home_dir)


def _hook_manifest_entry_source_dir(
    repo_root: pathlib.Path,
    entry: Any,
    *,
    hook_name: str,
    invoke_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> ActiveHookDir | None:
    if not isinstance(entry, dict) or entry.get("source_kind") != "active-hook":
        return None
    source_scope = str(entry.get("source_scope") or "manifest")
    if source_scope not in HOOK_MANIFEST_SOURCE_DIR_SCOPES:
        return None
    source_file = _manifest_source_file(
        repo_root,
        entry,
        hook_name=hook_name,
        invoke_root=invoke_root,
        runtime_hooks_dir=runtime_hooks_dir,
        home_dir=home_dir,
    )
    if source_file is None:
        return None
    return ActiveHookDir(source_file.parent, source_scope)


def _hook_manifest_source_dir_entry(
    repo_root: pathlib.Path,
    entry: Any,
    *,
    invoke_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> ActiveHookDir | None:
    if not isinstance(entry, dict):
        return None
    source_scope = str(entry.get("source_scope") or "")
    raw = entry.get("source_path")
    if not isinstance(raw, str) or not raw:
        return None
    manifest_dir = pathlib.Path(raw)
    if not manifest_dir.is_absolute():
        manifest_dir = repo_root / manifest_dir
    current_dir = _current_hooks_dir_for_scope(
        source_scope,
        repo_root=repo_root,
        invoke_root=invoke_root,
        home_dir=home_dir,
    )
    if source_scope in {"global", "system"}:
        return None if current_dir is None else ActiveHookDir(current_dir, source_scope)
    if source_scope in {"local", "worktree"}:
        if current_dir is None:
            return None
        if _same_path(current_dir, runtime_hooks_dir):
            return ActiveHookDir(manifest_dir, source_scope)
        return ActiveHookDir(current_dir, source_scope)
    return None


def _manifest_active_hook_dirs(
    manifest: dict[str, Any],
    *,
    repo_root: pathlib.Path,
    invoke_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> list[ActiveHookDir]:
    dirs: list[ActiveHookDir] = []
    for entry in _hook_manifest_source_dirs(manifest):
        candidate = _hook_manifest_source_dir_entry(
            repo_root,
            entry,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        )
        if candidate is not None:
            dirs.append(candidate)
    for hook_name, entry in sorted(_hook_manifest_hooks(manifest).items()):
        candidate = _hook_manifest_entry_source_dir(
            repo_root,
            entry,
            hook_name=hook_name,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        )
        if candidate is not None:
            dirs.append(candidate)
    for hook_name, entries in sorted(_hook_manifest_shadowed(manifest).items()):
        if not isinstance(entries, list):
            continue
        for entry in entries:
            candidate = _hook_manifest_entry_source_dir(
                repo_root,
                entry,
                hook_name=hook_name,
                invoke_root=invoke_root,
                runtime_hooks_dir=runtime_hooks_dir,
                home_dir=home_dir,
            )
            if candidate is not None:
                dirs.append(candidate)
    deduped: list[ActiveHookDir] = []
    for candidate in dirs:
        if not any(_same_path(candidate.path, existing.path) for existing in deduped):
            deduped.append(candidate)
    return deduped


def _record_source_dir(
    source_dirs: list[dict[str, str]],
    hook_dir: ActiveHookDir,
    *,
    runtime_hooks_dir: pathlib.Path,
) -> None:
    if hook_dir.source_scope not in HOOK_MANIFEST_SOURCE_DIR_SCOPES:
        return
    if _same_path(hook_dir.path, runtime_hooks_dir):
        return
    entry = {
        "source_scope": hook_dir.source_scope,
        "source_path": str(hook_dir.path),
    }
    if entry not in source_dirs:
        source_dirs.append(entry)


def _active_hook_dirs(
    *,
    invoke_root: pathlib.Path,
    source_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> list[ActiveHookDir]:
    source_scope, raw = _configured_hooks_path(
        invoke_root=invoke_root,
        source_root=source_root,
    )
    if raw is None:
        return [ActiveHookDir(runtime_hooks_dir, "default")]

    if pathlib.Path(raw).is_absolute() or raw == "~" or raw.startswith("~/"):
        return [
            ActiveHookDir(
                _resolve_hooks_path(source_root, raw, home_dir=home_dir),
                source_scope,
            )
        ]

    bases = (
        [invoke_root]
        if source_scope in {"worktree", "global", "system"}
        else [source_root]
    )
    if source_scope == "local" and not _same_path(invoke_root, source_root):
        bases.append(invoke_root)

    candidates: list[ActiveHookDir] = []
    for base in bases:
        candidate = _resolve_hooks_path(base, raw, home_dir=home_dir)
        if not any(_same_path(candidate, existing.path) for existing in candidates):
            candidates.append(ActiveHookDir(candidate, source_scope))
    return candidates


def _effective_hooks_dir(
    repo_root: pathlib.Path,
    *,
    home_dir: pathlib.Path | None,
) -> pathlib.Path | None:
    raw = _git_config_value(repo_root, ["--get", "core.hooksPath"])
    if raw is None:
        return None
    return _resolve_hooks_path(repo_root, raw, home_dir=home_dir)


def _set_runtime_hooks_path(
    *,
    invoke_root: pathlib.Path,
    source_root: pathlib.Path,
    runtime_hooks_dir: pathlib.Path,
    home_dir: pathlib.Path | None,
) -> None:
    _git(source_root, ["config", "--local", "core.hooksPath", str(runtime_hooks_dir)])
    effective = _effective_hooks_dir(invoke_root, home_dir=home_dir)
    if effective is not None and _same_path(effective, runtime_hooks_dir):
        return

    worktree_set = _git(
        invoke_root,
        ["config", "--worktree", "core.hooksPath", str(runtime_hooks_dir)],
        check=False,
    )
    if worktree_set.returncode != 0:
        raise CleanMergedError(
            "core.hooksPath is still overridden outside repo-local config and "
            f"worktree config could not be updated: {worktree_set.stderr.strip()}"
        )
    effective = _effective_hooks_dir(invoke_root, home_dir=home_dir)
    if effective is None or not _same_path(effective, runtime_hooks_dir):
        raise CleanMergedError(
            f"core.hooksPath did not resolve to runtime hooks directory {runtime_hooks_dir}"
        )


def install_hooks(
    invoke_root: pathlib.Path,
    *,
    source_root: pathlib.Path | None = None,
    home_dir: pathlib.Path | None = None,
) -> pathlib.Path:
    source_root = _main_worktree_root(invoke_root) if source_root is None else source_root
    if not invoke_root.is_dir():
        raise CleanMergedError(f"invoke worktree does not exist: {invoke_root}")
    if not source_root.is_dir():
        raise CleanMergedError(f"source worktree does not exist: {source_root}")
    source_hooks_dir = source_root / ".githooks"
    tracked_hook_paths = set(_tracked_hook_source_paths(source_root))
    missing = [
        name for name in CLEAN_MERGED_HOOKS
        if (
            not (source_hooks_dir / name).is_file()
            or f".githooks/{name}" not in tracked_hook_paths
        )
    ]
    if missing:
        raise CleanMergedError(
            "missing tracked clean-merged hook source(s): " + ", ".join(missing)
        )
    _raise_if_dirty_tracked_hook_sources(source_root)

    common_dir = git_common_dir(source_root)
    runtime_hooks_dir = common_dir / "hooks"
    runtime_hooks_dir.mkdir(parents=True, exist_ok=True)
    manifest = _load_hook_manifest(common_dir)
    manifest_hooks = dict(_hook_manifest_hooks(manifest))
    shadowed_hooks: dict[str, Any] = {}
    plan = HookInstallPlan()

    configured_active_hook_dirs = _active_hook_dirs(
        invoke_root=invoke_root,
        source_root=source_root,
        runtime_hooks_dir=runtime_hooks_dir,
        home_dir=home_dir,
    )
    active_hook_dirs: list[ActiveHookDir] = []
    for candidate in [
        *configured_active_hook_dirs,
        *_manifest_active_hook_dirs(
            manifest,
            repo_root=source_root,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        ),
    ]:
        if not any(_same_path(candidate.path, existing.path) for existing in active_hook_dirs):
            active_hook_dirs.append(candidate)
    source_hook_snapshots = sorted(
        (
            _tracked_hook_snapshot(source_root, rel_path)
            for rel_path in tracked_hook_paths
            if (source_root / rel_path).is_file()
        ),
        key=lambda snapshot: snapshot.hook_name,
    )
    source_hook_snapshots_by_name = {
        snapshot.hook_name: snapshot for snapshot in source_hook_snapshots
    }
    source_hook_names = set(source_hook_snapshots_by_name)
    _preflight_default_shadow_backups(
        manifest=manifest,
        source_root=source_root,
        invoke_root=invoke_root,
        runtime_hooks_dir=runtime_hooks_dir,
        common_dir=common_dir,
        home_dir=home_dir,
    )
    _preflight_manifest_hook_runtime_state(
        manifest=manifest,
        source_root=source_root,
        invoke_root=invoke_root,
        runtime_hooks_dir=runtime_hooks_dir,
        common_dir=common_dir,
        home_dir=home_dir,
        source_hook_names=source_hook_names,
    )
    _preflight_fresh_shadow_collisions(
        active_hook_dirs=active_hook_dirs,
        source_hook_snapshots_by_name=source_hook_snapshots_by_name,
        manifest_hooks=manifest_hooks,
        runtime_hooks_dir=runtime_hooks_dir,
        common_dir=common_dir,
    )
    source_dirs: list[dict[str, str]] = []
    adopted_source_paths: set[str] = set()
    runtime_source_scope = next(
        (
            active_hooks.source_scope
            for active_hooks in active_hook_dirs
            if _same_path(active_hooks.path, runtime_hooks_dir)
        ),
        "default",
    )
    if runtime_hooks_dir.is_dir():
        for hook_file in sorted(runtime_hooks_dir.iterdir(), key=lambda path: path.name):
            if (
                not hook_file.is_file()
                or not _is_git_hook_name(hook_file.name)
                or hook_file.name not in source_hook_names
            ):
                continue
            repo_source_snapshot = source_hook_snapshots_by_name[hook_file.name]
            if hook_file.read_bytes() == repo_source_snapshot.content:
                continue
            if hook_file.name in manifest_hooks:
                continue
            shadow_copy = plan.stage_shadow_copy(
                common_dir=common_dir,
                hook_file=hook_file,
            )
            _record_planned_shadowed_hook(
                hook_file=hook_file,
                shadow_copy=shadow_copy,
                repo_source_snapshot=repo_source_snapshot,
                source_scope=runtime_source_scope,
                shadowed_hooks=shadowed_hooks,
                hook_name=hook_file.name,
                source_path=str(shadow_copy.destination),
            )
            plan.stage_unlink(hook_file)
    for active_hooks in active_hook_dirs:
        _record_source_dir(
            source_dirs,
            active_hooks,
            runtime_hooks_dir=runtime_hooks_dir,
        )
        if not active_hooks.path.is_dir() or not _same_path(active_hooks.path, runtime_hooks_dir):
            continue
        for hook_file in sorted(active_hooks.path.iterdir(), key=lambda path: path.name):
            if plan.will_unlink(hook_file):
                continue
            if not hook_file.is_file() or not _is_git_hook_name(hook_file.name):
                continue
            if hook_file.name in source_hook_names:
                repo_source_snapshot = source_hook_snapshots_by_name[hook_file.name]
                if hook_file.read_bytes() == repo_source_snapshot.content:
                    continue
                if hook_file.name in manifest_hooks:
                    continue
                shadow_copy = plan.stage_shadow_copy(
                    common_dir=common_dir,
                    hook_file=hook_file,
                )
                _record_planned_shadowed_hook(
                    hook_file=hook_file,
                    shadow_copy=shadow_copy,
                    repo_source_snapshot=repo_source_snapshot,
                    source_scope=active_hooks.source_scope,
                    shadowed_hooks=shadowed_hooks,
                    hook_name=hook_file.name,
                    source_path=str(shadow_copy.destination),
                )
                plan.stage_unlink(hook_file)
                continue
            entry = manifest_hooks.get(hook_file.name)
            if isinstance(entry, dict):
                if entry.get("runtime_sha256") != _file_sha256(hook_file):
                    raise CleanMergedError(
                        f"refusing to adopt modified runtime hook {hook_file.name} "
                        f"without installer provenance at {hook_file}"
                    )
                adopted_source_paths.add(str(hook_file))
                continue
            _record_hook_provenance(
                source_file=hook_file,
                destination=hook_file,
                source_kind="active-hook",
                source_scope=active_hooks.source_scope,
                source_path=str(hook_file),
                manifest_hooks=manifest_hooks,
            )
            adopted_source_paths.add(str(hook_file))
    for source_snapshot in source_hook_snapshots:
        hook_file = source_snapshot.source_file
        destination = runtime_hooks_dir / source_snapshot.hook_name
        plan.stage_copy_hook(
            source_file=hook_file,
            destination=destination,
            source_kind="repo-source",
            source_scope="repo",
            source_path=_repo_relative_path(source_root, hook_file),
            manifest_hooks=manifest_hooks,
            source_snapshot=source_snapshot,
        )

    for hook_name, entry in sorted(_hook_manifest_hooks(manifest).items()):
        if (
            not isinstance(entry, dict)
            or entry.get("source_kind") != "repo-source"
            or hook_name in source_hook_names
        ):
            continue
        plan.stage_remove_hook(
            runtime_hooks_dir=runtime_hooks_dir,
            hook_name=hook_name,
            entry=entry,
        )
        manifest_hooks.pop(hook_name, None)

    for hook_name, entries in sorted(_hook_manifest_shadowed(manifest).items()):
        if not isinstance(entries, list):
            raise CleanMergedError(f"shadowed hook manifest entry for {hook_name} is invalid")
        for entry in entries:
            if not isinstance(entry, dict):
                raise CleanMergedError(
                    f"shadowed hook manifest entry for {hook_name} is invalid"
                )
            source_file = _manifest_source_file(
                source_root,
                entry,
                hook_name=hook_name,
                invoke_root=invoke_root,
                runtime_hooks_dir=runtime_hooks_dir,
                home_dir=home_dir,
            )
            if source_file is None:
                if entry.get("source_scope") in {"global", "local", "worktree", "system"}:
                    continue
                raise CleanMergedError(
                    f"shadowed hook manifest entry for {hook_name} has invalid source_path"
                )
            source_scope = str(entry.get("source_scope") or "manifest")
            if source_scope == "default":
                _validate_default_shadowed_hook_backup(
                    hook_name=hook_name,
                    entry=entry,
                    source_file=source_file,
                )
            if not source_file.is_file():
                continue
            adopted_source_paths.add(str(source_file))
            if hook_name in source_hook_names:
                if source_scope == "default":
                    entries = shadowed_hooks.setdefault(hook_name, [])
                    if not isinstance(entries, list):
                        entries = []
                        shadowed_hooks[hook_name] = entries
                    entries.append(dict(entry))
                else:
                    repo_source_snapshot = source_hook_snapshots_by_name[hook_name]
                    source_snapshot = _read_hook_snapshot(source_file)
                    if source_snapshot.content == repo_source_snapshot.content:
                        continue
                    _record_shadowed_hook_snapshot(
                        hook_file=source_file,
                        source_snapshot=source_snapshot,
                        repo_source_snapshot=repo_source_snapshot,
                        source_scope=source_scope,
                        shadowed_hooks=shadowed_hooks,
                        hook_name=hook_name,
                    )
                continue
            destination = runtime_hooks_dir / hook_name
            plan.stage_copy_hook(
                source_file=source_file,
                destination=destination,
                source_kind="active-hook",
                source_scope=source_scope,
                source_path=str(source_file),
                manifest_hooks=manifest_hooks,
            )

    for hook_name, entry in sorted(_hook_manifest_hooks(manifest).items()):
        if not isinstance(entry, dict) or entry.get("source_kind") != "active-hook":
            continue
        source_file = _manifest_source_file(
            source_root,
            entry,
            hook_name=hook_name,
            invoke_root=invoke_root,
            runtime_hooks_dir=runtime_hooks_dir,
            home_dir=home_dir,
        )
        source_scope = str(entry.get("source_scope") or "manifest")
        if source_file is None:
            if entry.get("source_scope") in {"global", "local", "worktree", "system"}:
                plan.stage_remove_hook(
                    runtime_hooks_dir=runtime_hooks_dir,
                    hook_name=hook_name,
                    entry=entry,
                )
                manifest_hooks.pop(hook_name, None)
                continue
            raise CleanMergedError(f"hook manifest entry for {hook_name} has invalid source_path")
        if source_scope == "default" and _is_shadowed_hook_backup_path(common_dir, source_file):
            _validate_default_shadowed_hook_backup(
                hook_name=hook_name,
                entry=entry,
                source_file=source_file,
            )
        if not source_file.is_file():
            plan.stage_remove_hook(
                runtime_hooks_dir=runtime_hooks_dir,
                hook_name=hook_name,
                entry=entry,
            )
            manifest_hooks.pop(hook_name, None)
            continue
        adopted_source_paths.add(str(source_file))
        if hook_name in source_hook_names:
            repo_source_snapshot = source_hook_snapshots_by_name[hook_name]
            source_snapshot = _read_hook_snapshot(source_file)
            if source_snapshot.content == repo_source_snapshot.content:
                continue
            if source_scope == "default" or _same_path(source_file.parent, runtime_hooks_dir):
                shadow_copy = plan.stage_shadow_copy(
                    common_dir=common_dir,
                    hook_file=source_file,
                    source_snapshot=source_snapshot,
                )
                _record_planned_shadowed_hook(
                    hook_file=source_file,
                    shadow_copy=shadow_copy,
                    repo_source_snapshot=repo_source_snapshot,
                    source_scope=source_scope,
                    shadowed_hooks=shadowed_hooks,
                    hook_name=hook_name,
                    source_path=str(shadow_copy.destination),
                )
            else:
                _record_shadowed_hook_snapshot(
                    hook_file=source_file,
                    source_snapshot=source_snapshot,
                    repo_source_snapshot=repo_source_snapshot,
                    source_scope=source_scope,
                    shadowed_hooks=shadowed_hooks,
                    hook_name=hook_name,
                )
            continue
        destination = runtime_hooks_dir / hook_name
        plan.stage_copy_hook(
            source_file=source_file,
            destination=destination,
            source_kind="active-hook",
            source_scope=source_scope,
            source_path=str(source_file),
            manifest_hooks=manifest_hooks,
        )

    for active_hooks in active_hook_dirs:
        active_hooks_dir = active_hooks.path
        if not active_hooks_dir.is_dir() or _same_path(active_hooks_dir, runtime_hooks_dir):
            continue
        for hook_file in sorted(active_hooks_dir.iterdir(), key=lambda path: path.name):
            if (
                not _is_git_hook_name(hook_file.name)
                or str(hook_file) in adopted_source_paths
            ):
                continue
            if hook_file.name in source_hook_names:
                if hook_file.is_symlink() or not hook_file.is_file():
                    _validate_shadowed_hook_copy(common_dir, hook_file)
                    continue
                repo_source_snapshot = source_hook_snapshots_by_name[hook_file.name]
                source_snapshot = _read_hook_snapshot(hook_file)
                if source_snapshot.content == repo_source_snapshot.content:
                    continue
                _record_shadowed_hook_snapshot(
                    hook_file=hook_file,
                    source_snapshot=source_snapshot,
                    repo_source_snapshot=repo_source_snapshot,
                    source_scope=active_hooks.source_scope,
                    shadowed_hooks=shadowed_hooks,
                )
                continue
            if not hook_file.is_file():
                continue
            destination = runtime_hooks_dir / hook_file.name
            plan.stage_copy_hook(
                source_file=hook_file,
                destination=destination,
                source_kind="active-hook",
                source_scope=active_hooks.source_scope,
                source_path=str(hook_file),
                manifest_hooks=manifest_hooks,
            )

    plan.stage_validate_clean_tracked_sources(repo_root=source_root)
    plan.stage_set_runtime_hooks_path(
        invoke_root=invoke_root,
        source_root=source_root,
        runtime_hooks_dir=runtime_hooks_dir,
        home_dir=home_dir,
    )
    plan.stage_write_hook_manifest(
        common_dir,
        runtime_hooks_dir=runtime_hooks_dir,
        hooks=manifest_hooks,
        shadowed_hooks=shadowed_hooks,
        source_dirs=source_dirs,
    )
    plan.apply()
    return runtime_hooks_dir


_REPORT_SECRET_PATTERNS: tuple[tuple[re.Pattern[str], str], ...] = (
    (re.compile(r"([A-Za-z][A-Za-z0-9+.-]*://)[^/@\s'\"]+@"),
     r"\1<redacted>@"),
    (re.compile(r"(?i)\b(authorization\s*:\s*(?:bearer|basic)\s+)"
                r"[A-Za-z0-9._~+/=-]+"),
     r"\1<redacted>"),
    (re.compile(r"(?i)\b((?:token|password|passwd|secret|api[_-]?key|"
                r"access[_-]?token|refresh[_-]?token)\s*[:=]\s*)"
                r"[^,\s;'\"\\]+"),
     r"\1<redacted>"),
    (re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_]{20,}\b"),
     "<redacted>"),
    (re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b"),
     "<redacted>"),
)


def _safe_report_error(raw: str, *, limit: int) -> str:
    """Sanitize subprocess stderr before it reaches reports or audit logs."""
    text = raw.strip()
    for pattern, replacement in _REPORT_SECRET_PATTERNS:
        text = pattern.sub(replacement, text)
    return text[:limit]


@dataclasses.dataclass(frozen=True)
class Branch:
    name: str
    sha: str


@dataclasses.dataclass(frozen=True)
class Worktree:
    path: pathlib.Path
    head: str
    branch: str | None  # None = detached


def list_local_branches(repo_root: pathlib.Path) -> list[Branch]:
    out = _git(repo_root, ["for-each-ref", "--format=%(refname:short)\t%(objectname)",
                            "refs/heads/"])
    branches: list[Branch] = []
    for line in out.stdout.splitlines():
        if not line.strip():
            continue
        name, sha = line.split("\t", 1)
        branches.append(Branch(name=name.strip(), sha=sha.strip()))
    return branches


def current_branch(repo_root: pathlib.Path) -> str | None:
    out = _git(repo_root, ["symbolic-ref", "--short", "HEAD"], check=False)
    if out.returncode != 0:
        return None  # detached HEAD
    return out.stdout.strip() or None


def protected_branch_names(config: Config, current: str | None, keep: set[str]) -> set[str]:
    """Branch identities protected from cleanup by configuration or invocation."""
    return {b for b in (config.trunk_branch, current) if b} | keep


def resolve_trunk_sha(repo_root: pathlib.Path, trunk: str, remote: str) -> str | None:
    """Resolve only the configured trunk SHA, preferring remote-tracking."""
    for ref in (f"refs/remotes/{remote}/{trunk}", f"refs/heads/{trunk}"):
        out = _git(repo_root, ["rev-parse", "--verify", ref], check=False)
        if out.returncode == 0:
            return out.stdout.strip()
    return None


def effective_trunk_sha(
    repo_root: pathlib.Path, config: Config, trunk_sha_override: str | None = None,
) -> str | None:
    if trunk_sha_override is not None:
        return trunk_sha_override if SHA_RE.match(trunk_sha_override) else None
    return resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)


def resolve_ref_sha(repo_root: pathlib.Path, ref: str) -> str | None:
    out = _git(repo_root, ["rev-parse", "--verify", ref], check=False)
    if out.returncode != 0:
        return None
    sha = out.stdout.strip()
    return sha if SHA_RE.match(sha) else None


def worktree_for_branch(repo_root: pathlib.Path, branch: str) -> pathlib.Path | None:
    for wt in list_worktrees(repo_root):
        if wt.branch == branch:
            return wt.path
    return None


def is_ancestor(repo_root: pathlib.Path, tip: str, ancestor_of: str) -> bool:
    out = _git(repo_root, ["merge-base", "--is-ancestor", tip, ancestor_of], check=False)
    return out.returncode == 0


def list_worktrees(repo_root: pathlib.Path) -> list[Worktree]:
    out = _git(repo_root, ["worktree", "list", "--porcelain"])
    worktrees: list[Worktree] = []
    wt_path: pathlib.Path | None = None
    wt_head: str = ""
    wt_branch: str | None = None
    for line in out.stdout.splitlines():
        if not line:
            if wt_path is not None:
                worktrees.append(Worktree(path=wt_path, head=wt_head, branch=wt_branch))
            wt_path, wt_head, wt_branch = None, "", None
            continue
        if line.startswith("worktree "):
            wt_path = pathlib.Path(line[len("worktree "):])
        elif line.startswith("HEAD "):
            wt_head = line[len("HEAD "):].strip()
        elif line.startswith("branch "):
            ref = line[len("branch "):].strip()
            wt_branch = ref[len("refs/heads/"):] if ref.startswith("refs/heads/") else ref
        elif line.strip() == "detached":
            wt_branch = None
    if wt_path is not None:
        worktrees.append(Worktree(path=wt_path, head=wt_head, branch=wt_branch))
    return worktrees


def worktree_bound_branches(repo_root: pathlib.Path) -> set[str]:
    """Names of branches checked out in any worktree (including main)."""
    return {wt.branch for wt in list_worktrees(repo_root) if wt.branch}


# ---------------------------------------------------------------------------
# Backup refs


def backup_ref_name(branch: str, tip: str, now_ts: int) -> str:
    safe_branch = re.sub(r"[^A-Za-z0-9._/-]", "_", branch)
    return f"refs/clean-merged/{safe_branch}-{tip[:12]}-{now_ts}"


def write_backup_ref(repo_root: pathlib.Path, branch: str, tip: str) -> str:
    ref = backup_ref_name(branch, tip, int(time.time()))
    _git(repo_root, ["update-ref", ref, tip])
    return ref


def delete_branch_ref_cas(
    repo_root: pathlib.Path, branch: str, expected_sha: str, *, report_error_max_chars: int,
) -> tuple[bool, str]:
    """CAS delete via `git update-ref -d refs/heads/<branch> <expected_sha>`.

    We do NOT use `git branch -d` because its merged-ness check is against HEAD
    or the branch's upstream — not the trunk we already verified ancestor-against.
    When Lane H runs from a hook while HEAD is on a feature branch (or behind
    trunk), `branch -d` may refuse eligible branches. The
    is_ancestor(<B>, <trunk>) check above already proved merged-ness; CAS
    deletes exactly that tip and refuses on SHA drift.
    """
    out = _git(repo_root, ["update-ref", "-d", f"refs/heads/{branch}", expected_sha],
               check=False)
    return out.returncode == 0, _safe_report_error(out.stderr, limit=report_error_max_chars)


# ---------------------------------------------------------------------------
# Audit log + heartbeat


def _resolve_path(repo_root: pathlib.Path, raw: str) -> pathlib.Path:
    if raw.startswith("<git-common-dir>/"):
        return git_common_dir(repo_root) / raw[len("<git-common-dir>/"):]
    p = pathlib.Path(raw)
    return p if p.is_absolute() else repo_root / p


def _acquire_lock(lock_path: pathlib.Path, exclusive: bool = True) -> int | None:
    """Acquire fcntl.flock on lock_path. Returns fd, or None if it would block.

    Non-blocking in both shared and exclusive modes so callers can preserve
    the fail-open "another instance holds the lock; aborting" behavior.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        flags = (fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH) | fcntl.LOCK_NB
        fcntl.flock(fd, flags)
    except BlockingIOError:
        os.close(fd)
        return None
    return fd


def _release_lock(fd: int) -> None:
    fcntl.flock(fd, fcntl.LOCK_UN)
    os.close(fd)


def _rotated_log_paths(log_path: pathlib.Path) -> list[pathlib.Path]:
    return sorted(
        path for path in log_path.parent.glob(f"{log_path.name}.1*")
        if path.is_file()
    )


def _prune_expired_rotated_logs(log_path: pathlib.Path, retention_days: int) -> None:
    cutoff = time.time() - retention_days * 86400
    for rotated in _rotated_log_paths(log_path):
        try:
            if rotated.stat().st_mtime < cutoff:
                rotated.unlink()
        except OSError:
            pass


def _rotate_log_if_needed(log_path: pathlib.Path, max_bytes: int) -> None:
    if log_path.exists() and log_path.stat().st_size > max_bytes:
        rotated = _next_rotated_log_path(log_path)
        try:
            log_path.rename(rotated)
        except OSError:
            pass


def _next_rotated_log_path(log_path: pathlib.Path) -> pathlib.Path:
    first = log_path.with_suffix(log_path.suffix + ".1")
    if not first.exists():
        return first
    while True:
        candidate = log_path.with_name(f"{log_path.name}.1.{os.getpid()}.{time.time_ns()}")
        if not candidate.exists():
            return candidate


def _log_lock_path(log_path: pathlib.Path) -> pathlib.Path:
    return log_path.with_suffix(log_path.suffix + ".lock")


def _open_rotating_log(
    log_path: pathlib.Path, max_bytes: int, *, rotated_retention_days: int,
) -> Any:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    fd = _acquire_lock(_log_lock_path(log_path))
    try:
        if fd is not None:
            _prune_expired_rotated_logs(log_path, rotated_retention_days)
            _rotate_log_if_needed(log_path, max_bytes)
        return log_path.open("a", encoding="utf-8", buffering=1)
    finally:
        if fd is not None:
            _release_lock(fd)


def _rotated_log_usage(repo_root: pathlib.Path, config: Config) -> tuple[int, int]:
    paths = {
        _resolve_path(repo_root, config.logging.audit_path),
        _resolve_path(repo_root, config.logging.lane_r_log_path),
    }
    rotated_paths: set[pathlib.Path] = set()
    total_bytes = 0
    for log_path in paths:
        for rotated in _rotated_log_paths(log_path):
            if rotated in rotated_paths:
                continue
            rotated_paths.add(rotated)
            try:
                total_bytes += rotated.stat().st_size
            except OSError:
                pass
    return len(rotated_paths), total_bytes


def write_audit(repo_root: pathlib.Path, config: Config, record: dict[str, Any]) -> None:
    log_path = _resolve_path(repo_root, config.logging.audit_path)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = _log_lock_path(log_path)
    fd = _acquire_lock(lock_path)
    if fd is None:
        # best-effort; never break the op over logging
        return
    try:
        _prune_expired_rotated_logs(log_path, config.logging.rotated_log_retention_days)
        _rotate_log_if_needed(log_path, config.logging.max_log_bytes)
        record_with_ts = {"ts": dt.datetime.now(dt.timezone.utc).isoformat(), **record}
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(record_with_ts, ensure_ascii=False, sort_keys=True) + "\n")
    finally:
        _release_lock(fd)


def _atomic_write_text(path: pathlib.Path, text: str) -> None:
    """Atomic write via tmp + os.replace.

    Plain pathlib.Path.write_text() truncates-then-writes; an interruption
    leaves the file empty or partial JSON. For manifests whose integrity gates
    purge decisions, that's a data-loss vector. Atomic rename guarantees the
    file is either the previous content or the new content, never partial.

    The try/finally unlinks the tmp file if we crash between write_text and
    os.replace, so orphan .tmp.<pid> files don't accumulate across many
    crashes.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp.{os.getpid()}")
    try:
        tmp.write_text(text, encoding="utf-8")
        os.replace(str(tmp), str(path))
    except BaseException:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def write_heartbeat(repo_root: pathlib.Path, config: Config) -> None:
    path = _resolve_path(repo_root, config.logging.heartbeat_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        path.write_text(dt.datetime.now(dt.timezone.utc).isoformat(), encoding="utf-8")
    except OSError:
        pass


# ---------------------------------------------------------------------------
# Lane R: gh reconcile


_GH_ENV = {"GH_PROMPT_DISABLED": "1", "GIT_TERMINAL_PROMPT": "0", "NO_COLOR": "1"}
_GH_CACHE_NAME = "clean-merged-gh-cache.json"


def _gh_cache_path(repo_root: pathlib.Path) -> pathlib.Path:
    return git_common_dir(repo_root) / _GH_CACHE_NAME


def _load_gh_cache(path: pathlib.Path) -> tuple[dict[str, Any] | None, str | None]:
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}, None
    except OSError as exc:
        return None, f"gh cache unavailable: {exc}"
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        return None, f"gh cache invalid json: {exc}"
    if not isinstance(data, dict):
        return None, f"gh cache invalid root: expected object, got {type(data).__name__}"
    return data, None


def _save_gh_cache(path: pathlib.Path, cache: dict[str, Any], ttl: float) -> None:
    """Atomic write via tmp + os.replace plus TTL-based eviction.

    Concurrent detached Lane R processes RMW the cache; non-atomic
    path.write_text() can interleave/truncate. Atomic rename makes the worst
    case a lost update (one writer wins), never corruption.

    Every save keeps only exact-shape, unexpired entries.
    """
    now = time.time()
    pruned: dict[str, Any] = {}
    for key, entry in cache.items():
        if "@" not in key:
            continue
        try:
            age = _gh_cache_entry_age(entry, now)
            prs = _gh_cache_entry_prs(entry)
        except ValueError:
            continue
        if age < ttl:
            assert isinstance(entry, dict)
            pruned[key] = {"fetched_at": entry["fetched_at"], "prs": prs}
    try:
        tmp = path.with_suffix(path.suffix + f".tmp.{os.getpid()}")
        tmp.write_text(json.dumps(pruned), encoding="utf-8")
        os.replace(str(tmp), str(path))
    except OSError:
        pass


def gh_merged_pr_for_branch(
    repo_root: pathlib.Path, branch: str, timeout: float, limit: int,
    report_error_max_chars: int,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    """Return (prs, error). prs=None means gh trouble (keep the branch)."""
    cmd = [
        "gh", "pr", "list", "--head", branch, "--state", "merged",
        "--json", "number,headRefOid,baseRefName,headRepositoryOwner,isCrossRepository",
        "--limit", str(limit),
    ]
    try:
        out = subprocess.run(
            cmd, cwd=repo_root, capture_output=True, text=True,
            timeout=timeout, env={**os.environ, **_GH_ENV},
        )
    except subprocess.TimeoutExpired:
        return None, "gh timeout"
    except OSError as exc:
        return None, f"gh unavailable: {_safe_report_error(str(exc), limit=report_error_max_chars)}"
    if out.returncode != 0:
        return None, (
            f"gh exit {out.returncode}: "
            f"{_safe_report_error(out.stderr, limit=report_error_max_chars)}"
        )
    try:
        prs = json.loads(out.stdout) if out.stdout.strip() else []
    except json.JSONDecodeError as exc:
        return None, f"gh malformed json: {exc}"
    if not isinstance(prs, list):
        return None, "gh non-list payload"
    if not all(_valid_gh_pr_payload_item(pr) for pr in prs):
        return None, "gh invalid PR payload"
    return prs, None


def _valid_gh_pr_payload_item(pr: Any) -> bool:
    if not isinstance(pr, dict):
        return False
    head_oid = pr.get("headRefOid")
    base_ref = pr.get("baseRefName")
    owner = pr.get("headRepositoryOwner")
    number = pr.get("number")
    return (
        isinstance(number, int)
        and not isinstance(number, bool)
        and number > 0
        and isinstance(head_oid, str)
        and SHA_RE.fullmatch(head_oid) is not None
        and isinstance(base_ref, str)
        and base_ref != ""
        and isinstance(owner, dict)
        and isinstance(owner.get("login"), str)
        and owner.get("login") != ""
        and isinstance(pr.get("isCrossRepository"), bool)
    )


def _gh_cache_entry_age(entry: Any, now: float) -> float:
    if not isinstance(entry, dict) or set(entry) != {"fetched_at", "prs"}:
        raise ValueError("invalid cache entry")
    try:
        fetched_at = float(entry["fetched_at"])
    except (TypeError, ValueError, OverflowError) as exc:
        raise ValueError("invalid cache entry") from exc
    age = now - fetched_at
    if not math.isfinite(age) or age < 0:
        raise ValueError("invalid cache entry")
    return age


def _gh_cache_entry_prs(entry: Any) -> list[dict[str, Any]]:
    if not isinstance(entry, dict):
        raise ValueError("invalid cache entry")
    prs = entry["prs"]
    if not _valid_gh_pr_cache_payload(prs):
        raise ValueError("invalid cache entry")
    return prs


def _valid_gh_pr_cache_payload(prs: Any) -> bool:
    return isinstance(prs, list) and all(_valid_gh_pr_payload_item(pr) for pr in prs)


def _entry_is_live(entry: Any, now: float, ttl: float) -> bool:
    try:
        return _gh_cache_entry_age(entry, now) < ttl
    except ValueError:
        return False


def _gh_cache_health(path: pathlib.Path) -> str | None:
    cache, err = _load_gh_cache(path)
    if err is not None:
        return err
    assert cache is not None
    now = time.time()
    for key, entry in cache.items():
        if "@" not in key:
            return f"gh cache invalid key: {key!r}"
        try:
            _gh_cache_entry_age(entry, now)
            _gh_cache_entry_prs(entry)
        except ValueError as exc:
            return f"gh cache invalid entry for {key}: {exc}"
    return None


def gh_merged_pr_for_branch_cached(
    repo_root: pathlib.Path, config: Config, branch: str, tip: str,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    """Per-branch gh result with TTL cache (avoids re-querying on every hook fire).

    Cache key is (branch, tip-sha[:12]). Keying by branch alone lets a stale
    negative result for tip A suppress cleanup after the branch advances to
    merged squash commit B. Keying by (branch, tip) invalidates the entry
    automatically when the tip moves.

    Stored under <git-common-dir>/clean-merged-gh-cache.json. Missing entries
    and exact-shape stale entries query gh. Corrupt cache state fails closed.
    """
    cache_path = _gh_cache_path(repo_root)
    now = time.time()
    cache, cache_error = _load_gh_cache(cache_path)
    if cache is None:
        return None, cache_error
    cache_key = f"{branch}@{tip[:12]}"
    if cache_key in cache:
        entry = cache[cache_key]
        try:
            age = _gh_cache_entry_age(entry, now)
            prs = _gh_cache_entry_prs(entry)
        except ValueError:
            return None, "invalid cache entry"
        if age < config.lane_r.cache_ttl_s:
            return prs, None
    prs, err = gh_merged_pr_for_branch(
        repo_root, branch, config.lane_r.gh_timeout_s, config.lane_r.gh_limit,
        config.logging.report_error_max_chars)
    safe_err = (_safe_report_error(err, limit=config.logging.report_error_max_chars)
                if isinstance(err, str) else err)
    if prs is not None:
        cache[cache_key] = {"fetched_at": now, "prs": prs}
        _save_gh_cache(cache_path, cache, config.lane_r.cache_ttl_s)
    return prs, safe_err


def find_matching_merged_pr(
    prs: list[dict[str, Any]], tip: str, trunk: str, origin_owner: str,
) -> dict[str, Any] | None:
    """Return the PR whose headRefOid == tip AND baseRefName == trunk AND same-repo.
    """
    for pr in prs:
        if pr["headRefOid"] != tip:
            continue
        if pr["baseRefName"] != trunk:
            continue
        if pr["isCrossRepository"]:
            continue
        owner = pr["headRepositoryOwner"]
        if owner["login"] != origin_owner:
            continue
        return pr
    return None


# ---------------------------------------------------------------------------
# Lane W: filesystem guards


def has_hidden_index_bits(wt_path: pathlib.Path) -> list[str]:
    """Return paths with assume-unchanged/skip-worktree bits.

    `git ls-files -v` uses lowercase letters for assume-unchanged (e.g. `h`
    instead of `H` for cached) and UPPERCASE `S` for skip-worktree. Both bits
    hide modifications from `git status --porcelain`, so both must be detected
    or Lane W will archive+remove worktrees with hidden dirty state.

    Per `git help ls-files`: identified flags are c/m/k/? for various states,
    H for cached; the lowercase variant means assume-unchanged is set; the
    dedicated skip-worktree marker is uppercase S. We flag any line whose
    first char is a lowercase alpha (assume-unchanged) OR an uppercase S.
    """
    out = subprocess.run(
        ["git", "-C", str(wt_path), "ls-files", "-v"],
        capture_output=True, text=True, check=True,
    )
    flagged: list[str] = []
    for line in out.stdout.splitlines():
        if not line:
            continue
        flag = line[0]
        # lowercase alpha = assume-unchanged bit on a tracked file
        # uppercase 'S' = skip-worktree bit on a tracked file
        if (flag.islower() and flag.isalpha()) or flag == "S":
            flagged.append(line[2:].strip())
    return flagged


def has_ignored_content(wt_path: pathlib.Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(wt_path), "-c", "status.showUntrackedFiles=all",
         "ls-files", "--others", "--ignored", "--exclude-standard", "-z"],
        capture_output=True, text=True, check=True,
    )
    return [p for p in out.stdout.split("\0") if p]


def has_nested_git(wt_path: pathlib.Path) -> list[pathlib.Path]:
    """Find nested .git dirs/files (submodules, separate repos). Excludes the worktree's own .git."""
    hits: list[pathlib.Path] = []
    for dirpath, dirnames, filenames in os.walk(wt_path):
        # don't descend into the worktree's own .git
        rel = pathlib.Path(dirpath).relative_to(wt_path)
        if str(rel) == ".git":
            dirnames[:] = []
            continue
        # don't descend into other VCs
        if ".git" in dirnames or ".git" in filenames:
            # but skip the worktree's own top-level .git
            if str(rel) == ".":
                continue
            hits.append(pathlib.Path(dirpath) / ".git")
            # don't descend further into the nested repo
            if ".git" in dirnames:
                dirnames.remove(".git")
    return hits


def is_worktree_clean(wt_path: pathlib.Path) -> tuple[bool, str]:
    out = subprocess.run(
        ["git", "-C", str(wt_path), "-c", "status.showUntrackedFiles=all",
         "status", "--porcelain", "-z"],
        capture_output=True, text=True, check=True,
    )
    if out.stdout:
        # first entry only for the reason
        first = out.stdout.split("\0")[0]
        return False, first
    return True, ""


# ---------------------------------------------------------------------------
# Lane S


def _lane_s_record(
    config: Config, action: str, reason: str, **fields: Any,
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "lane": "S",
        "branch": config.trunk_branch,
        "action": action,
        "reason": reason,
    }
    record.update({key: value for key, value in fields.items() if value is not None})
    return record


def run_lane_s(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, quiet: bool, defer_cleanup: bool = False,
) -> tuple[list[dict[str, Any]], bool]:
    """Fetch/prune and fast-forward local trunk, refusing every non-FF case."""
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records, True

    records.append(_sync_fetch_record(repo_root, config, apply=apply))
    if not apply:
        records.append(_lane_s_record(
            config, "would-evaluate-after-fetch",
            "fast-forward safety can only be evaluated after fetch",
        ))
        if defer_cleanup:
            preview_record, ok = _sync_preview_trunk_record(repo_root, config)
            records.append(preview_record)
            return _finish_lane_s(records, apply=apply, quiet=quiet, ok=ok)
        return _finish_lane_s(records, apply=apply, quiet=quiet, ok=True)
    if records[-1]["action"] == "fetch-prune-failed":
        return _finish_lane_s(records, apply=apply, quiet=quiet, ok=False)

    local_ref = f"refs/heads/{config.trunk_branch}"
    remote_ref = f"refs/remotes/{config.remote_name}/{config.trunk_branch}"
    local_sha = resolve_ref_sha(repo_root, local_ref)
    remote_sha = resolve_ref_sha(repo_root, remote_ref)

    refusal = _sync_ref_refusal(config, local_ref, remote_ref, local_sha, remote_sha)
    if refusal is not None:
        records.append(refusal)
        return _finish_lane_s(records, apply=apply, quiet=quiet,
                              ok=refusal["action"] == "up-to-date")

    assert local_sha is not None
    assert remote_sha is not None
    if not is_ancestor(repo_root, local_sha, remote_sha):
        records.append(_lane_s_record(
            config, "refused-non-fast-forward",
            f"{local_ref} is not an ancestor of {remote_ref}",
            tip_sha=local_sha,
        ))
        return _finish_lane_s(records, apply=apply, quiet=quiet, ok=False)

    ff_record, ok = _fast_forward_trunk_ref(
        repo_root, config, local_ref=local_ref, remote_ref=remote_ref,
        local_sha=local_sha, remote_sha=remote_sha)
    if not ok:
        if ff_record is not None:
            records.append(ff_record)
        return _finish_lane_s(records, apply=apply, quiet=quiet, ok=False)

    fresh_sha = resolve_ref_sha(repo_root, local_ref) or remote_sha
    records.append(_lane_s_record(
        config, "fast-forwarded",
        f"{local_sha[:12]} -> {remote_sha[:12]}",
        tip_sha=fresh_sha,
    ))
    return _finish_lane_s(records, apply=apply, quiet=quiet, ok=True)


def _sync_fetch_record(repo_root: pathlib.Path, config: Config, *, apply: bool) -> dict[str, Any]:
    if not apply:
        return _lane_s_record(config, "would-fetch-prune", f"remote {config.remote_name}")
    fetch = _git(repo_root, ["fetch", "--prune", config.remote_name], check=False)
    if fetch.returncode != 0:
        return _lane_s_record(
            config, "fetch-prune-failed",
            _safe_report_error(
                fetch.stderr, limit=config.logging.report_error_max_chars) or "git fetch failed",
        )
    return _lane_s_record(config, "fetched-pruned", f"remote {config.remote_name}")


def _preview_ref_name(config: Config) -> str:
    safe_branch = re.sub(r"[^A-Za-z0-9._-]", "_", config.trunk_branch)
    return f"refs/clean-merged-preview/{os.getpid()}/{time.time_ns()}/{safe_branch}"


@dataclasses.dataclass(frozen=True)
class PreviewTrunkContext:
    local_ref: str
    local_sha: str
    remote_sha: str


def _fetch_preview_trunk(
    repo_root: pathlib.Path, config: Config, temp_ref: str,
) -> tuple[str | None, dict[str, Any] | None]:
    refspec = f"+refs/heads/{config.trunk_branch}:{temp_ref}"
    fetch = _git(
        repo_root,
        ["fetch", "--no-tags", "--no-write-fetch-head", "--refmap=",
         config.remote_name, refspec],
        check=False,
    )
    if fetch.returncode != 0:
        return None, _lane_s_record(
            config, "preview-fetch-failed",
            _safe_report_error(
                fetch.stderr, limit=config.logging.report_error_max_chars) or "git fetch failed",
        )
    return resolve_ref_sha(repo_root, temp_ref), None


def _delete_preview_ref(
    repo_root: pathlib.Path, config: Config, temp_ref: str, expected_sha: str,
) -> dict[str, Any] | None:
    delete = _git(repo_root, ["update-ref", "-d", temp_ref, expected_sha], check=False)
    if delete.returncode != 0:
        reason = _safe_report_error(delete.stderr, limit=config.logging.report_error_max_chars)
        return _lane_s_record(
            config, "preview-ref-cleanup-failed",
            reason or f"could not delete {temp_ref}",
        )
    if resolve_ref_sha(repo_root, temp_ref) is None:
        return None
    return _lane_s_record(
        config, "preview-ref-cleanup-failed",
        f"{temp_ref} still resolves after delete",
    )


def _preview_trunk_context(
    repo_root: pathlib.Path, config: Config, *, temp_ref: str, remote_sha: str | None,
) -> tuple[PreviewTrunkContext | None, dict[str, Any] | None]:
    if remote_sha is None:
        return None, _lane_s_record(
            config, "preview-fetch-failed",
            f"{temp_ref} did not resolve after fetch",
        )
    local_ref = f"refs/heads/{config.trunk_branch}"
    local_sha = resolve_ref_sha(repo_root, local_ref)
    if local_sha is None:
        return None, _lane_s_record(
            config, "refused-missing-local-trunk",
            f"{local_ref} does not resolve",
        )
    return PreviewTrunkContext(
        local_ref=local_ref, local_sha=local_sha, remote_sha=remote_sha,
    ), None


def _preview_non_fast_forward_refusal(
    repo_root: pathlib.Path, config: Config, context: PreviewTrunkContext,
) -> dict[str, Any] | None:
    if (context.local_sha == context.remote_sha
            or is_ancestor(repo_root, context.local_sha, context.remote_sha)):
        return None
    return _lane_s_record(
        config, "refused-non-fast-forward",
        f"{context.local_ref} is not an ancestor of preview remote trunk",
        tip_sha=context.local_sha,
    )


def _preview_dirty_trunk_refusal(
    repo_root: pathlib.Path, config: Config, context: PreviewTrunkContext,
) -> dict[str, Any] | None:
    if context.local_sha == context.remote_sha:
        return None
    trunk_worktree = worktree_for_branch(repo_root, config.trunk_branch)
    if trunk_worktree is None:
        return None
    clean, clean_reason = is_worktree_clean(trunk_worktree)
    if clean:
        return None
    return _lane_s_record(
        config, "refused-dirty-trunk-worktree",
        f"uncommitted changes: {clean_reason}",
        tip_sha=context.local_sha,
        worktree=str(trunk_worktree),
    )


def _first_refusal(
    checks: list[Callable[[], dict[str, Any] | None]],
) -> dict[str, Any] | None:
    for check in checks:
        refusal = check()
        if refusal is not None:
            return refusal
    return None


def _preview_trunk_refusal(
    repo_root: pathlib.Path, config: Config, context: PreviewTrunkContext,
) -> dict[str, Any] | None:
    return _first_refusal([
        lambda: _preview_non_fast_forward_refusal(repo_root, config, context),
        lambda: _preview_dirty_trunk_refusal(repo_root, config, context),
    ])


def _sync_preview_trunk_record(repo_root: pathlib.Path, config: Config) -> tuple[dict[str, Any], bool]:
    temp_ref = _preview_ref_name(config)
    remote_sha, fetch_refusal = _fetch_preview_trunk(repo_root, config, temp_ref)
    if fetch_refusal is not None:
        return fetch_refusal, False
    if remote_sha is None:
        return _lane_s_record(
            config, "preview-fetch-failed",
            f"{temp_ref} did not resolve after fetch",
        ), False
    cleanup_refusal = _delete_preview_ref(repo_root, config, temp_ref, remote_sha)
    if cleanup_refusal is not None:
        return cleanup_refusal, False
    context, context_refusal = _preview_trunk_context(
        repo_root, config, temp_ref=temp_ref, remote_sha=remote_sha,
    )
    if context_refusal is not None:
        return context_refusal, False
    assert context is not None
    refusal = _preview_trunk_refusal(repo_root, config, context)
    if refusal is not None:
        return refusal, False
    return _lane_s_record(
        config, "preview-fetched-trunk",
        f"remote {config.remote_name}/{config.trunk_branch}",
        tip_sha=context.remote_sha,
    ), True


def _sync_ref_refusal(
    config: Config, local_ref: str, remote_ref: str,
    local_sha: str | None, remote_sha: str | None,
) -> dict[str, Any] | None:
    if local_sha is None:
        return _lane_s_record(
            config, "refused-missing-local-trunk",
            f"{local_ref} does not resolve",
        )
    if remote_sha is None:
        return _lane_s_record(
            config, "refused-missing-remote-trunk",
            f"{remote_ref} does not resolve",
            tip_sha=local_sha,
        )
    if local_sha == remote_sha:
        return _lane_s_record(
            config, "up-to-date",
            f"{local_ref} matches {remote_ref}",
            tip_sha=local_sha,
        )
    return None


def _fast_forward_trunk_ref(
    repo_root: pathlib.Path, config: Config, *,
    local_ref: str, remote_ref: str, local_sha: str, remote_sha: str,
) -> tuple[dict[str, Any] | None, bool]:
    trunk_worktree = worktree_for_branch(repo_root, config.trunk_branch)
    if trunk_worktree is None:
        ff = _git(repo_root, ["update-ref", local_ref, remote_sha, local_sha], check=False)
        return (
            _lane_s_record(
                config, "fast-forward-cas-refused",
                _safe_report_error(
                    ff.stderr, limit=config.logging.report_error_max_chars)
                or "update-ref CAS failed",
                tip_sha=local_sha,
            ),
            False,
        ) if ff.returncode != 0 else (None, True)

    clean, clean_reason = is_worktree_clean(trunk_worktree)
    if not clean:
        return (
            _lane_s_record(
                config, "refused-dirty-trunk-worktree",
                f"uncommitted changes: {clean_reason}",
                tip_sha=local_sha,
                worktree=str(trunk_worktree),
            ),
            False,
        )

    ff = _git(trunk_worktree, ["merge", "--ff-only", remote_ref], check=False)
    return (
        _lane_s_record(
            config, "fast-forward-failed",
            _safe_report_error(
                ff.stderr, limit=config.logging.report_error_max_chars)
            or "git merge --ff-only failed",
            tip_sha=local_sha,
            worktree=str(trunk_worktree),
        ),
        False,
    ) if ff.returncode != 0 else (None, True)


def _finish_lane_s(
    records: list[dict[str, Any]], *, apply: bool, quiet: bool, ok: bool,
) -> tuple[list[dict[str, Any]], bool]:
    if not quiet:
        _print_lane_summary("S", records, apply)
    return records, ok


# ---------------------------------------------------------------------------
# Lane H


def run_lane_h(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool, trunk_sha_override: str | None = None,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = effective_trunk_sha(repo_root, config, trunk_sha_override)
    if not trunk_sha:
        return records
    cur = current_branch(repo_root)
    branches = list_local_branches(repo_root)
    bound = worktree_bound_branches(repo_root)

    skip_names = protected_branch_names(config, cur, keep)

    for br in branches:
        if br.name in skip_names:
            continue
        if not SHA_RE.match(br.sha):
            continue
        if br.name in bound:
            records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-worktree-bound",
                            "reason": "Lane W candidate"})
            continue
        if not is_ancestor(repo_root, br.sha, trunk_sha):
            continue
        if apply:
            # Re-read tip immediately before delete in case the branch moved
            # between list_local_branches() above and now. CAS refuses on drift.
            fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
            if not fresh_tip or not SHA_RE.match(fresh_tip):
                continue
            if fresh_tip != br.sha:
                records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-tip-moved",
                                "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                continue
            # Re-verify worktree binding: a worktree may have been bound to
            # this branch between function entry and the CAS delete.
            fresh_bound = worktree_bound_branches(repo_root)
            if br.name in fresh_bound:
                records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-worktree-bound-toctou",
                                "reason": "branch became worktree-bound after eligibility check"})
                continue
            backup = write_backup_ref(repo_root, br.name, fresh_tip)
            ok, err = delete_branch_ref_cas(
                repo_root, br.name, fresh_tip,
                report_error_max_chars=config.logging.report_error_max_chars)
            action = "deleted" if ok else "delete-cas-refused"
            reason = "" if ok else err
            records.append({"lane": "H", "branch": br.name, "tip_sha": fresh_tip,
                            "action": action, "reason": reason, "backup_ref": backup,
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
        else:
            records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                            "action": "would-delete", "reason": "ancestor of trunk"})
    if not quiet:
        _print_lane_summary("H", records, apply)
    return records


# ---------------------------------------------------------------------------
# Lane R


def run_lane_r(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool, trunk_sha_override: str | None = None,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = effective_trunk_sha(repo_root, config, trunk_sha_override)
    if not trunk_sha:
        return records
    cur = current_branch(repo_root)
    branches = list_local_branches(repo_root)
    bound = worktree_bound_branches(repo_root)
    skip_names = protected_branch_names(config, cur, keep)

    gh_unavailable = False
    for br in branches:
        if br.name in skip_names:
            continue
        if not SHA_RE.match(br.sha):
            continue
        if br.name in bound:
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-worktree-bound",
                            "reason": "Lane W candidate"})
            continue
        # ancestor path (Lane H may have skipped due to no hook fired yet)
        if is_ancestor(repo_root, br.sha, trunk_sha):
            if apply:
                fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
                if not fresh_tip or not SHA_RE.match(fresh_tip):
                    continue
                if fresh_tip != br.sha:
                    records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                    "action": "skipped-tip-moved",
                                    "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                    continue
                fresh_bound = worktree_bound_branches(repo_root)
                if br.name in fresh_bound:
                    records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                    "action": "skipped-worktree-bound-toctou",
                                    "reason": "branch became worktree-bound after eligibility"})
                    continue
                backup = write_backup_ref(repo_root, br.name, fresh_tip)
                ok, err = delete_branch_ref_cas(
                    repo_root, br.name, fresh_tip,
                    report_error_max_chars=config.logging.report_error_max_chars)
                records.append({"lane": "R", "branch": br.name, "tip_sha": fresh_tip,
                                "action": "deleted" if ok else "delete-cas-refused",
                                "reason": "" if ok else err, "backup_ref": backup,
                                "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
            else:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "would-delete", "reason": "ancestor of trunk"})
            continue
        # gh path
        prs, err = gh_merged_pr_for_branch_cached(repo_root, config, br.name, br.sha)
        if prs is None:
            gh_unavailable = True
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-gh-unavailable", "reason": err or "gh error"})
            continue
        match = find_matching_merged_pr(prs, br.sha, config.trunk_branch, config.origin_owner)
        if match is None:
            continue
        if apply:
            fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
            if not fresh_tip or not SHA_RE.match(fresh_tip):
                continue
            if fresh_tip != br.sha:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-tip-moved",
                                "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                continue
            fresh_bound = worktree_bound_branches(repo_root)
            if br.name in fresh_bound:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-worktree-bound-toctou",
                                "reason": "branch became worktree-bound after gh match"})
                continue
            backup = write_backup_ref(repo_root, br.name, fresh_tip)
            ok, err = delete_branch_ref_cas(
                repo_root, br.name, fresh_tip,
                report_error_max_chars=config.logging.report_error_max_chars)
            action = "deleted" if ok else "delete-cas-refused"
            records.append({"lane": "R", "branch": br.name, "tip_sha": fresh_tip,
                            "action": action, "reason": "" if ok else err,
                            "backup_ref": backup, "pr_number": match.get("number"),
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
        else:
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "would-delete",
                            "reason": f"gh PR #{match.get('number')} merged headRefOid==tip",
                            "pr_number": match.get("number")})

    if gh_unavailable and not quiet:
        print(f"[{SCRIPT_NAME}] lane R: gh unavailable for some branches; kept them", file=sys.stderr)
    if not quiet:
        _print_lane_summary("R", records, apply)
    return records


# ---------------------------------------------------------------------------
# Lane W


def _quarantine_target(config: Config, repo_root: pathlib.Path, name: str, sha: str,
                        wt_path: pathlib.Path) -> pathlib.Path:
    base = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    ts = int(time.time())
    pid = os.getpid()
    # Hash the absolute worktree path to disambiguate multiple worktrees bound
    # to the same branch (rare but possible). Without it, two same-second
    # archives of the same branch would collide on the same quarantine dir.
    wt_hash = hashlib.sha1(str(wt_path).encode("utf-8")).hexdigest()[:8]
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", name)
    return base / f"{safe}-{sha[:12]}-{ts}-{pid}-{wt_hash}"


def _archive_worktree(
    wt_path: pathlib.Path, archive_path: pathlib.Path, config: Config,
) -> tuple[bool, str]:
    """Tar the worktree dir to archive_path. Returns (ok, error)."""
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    # tar from the parent so the archive contains the worktree basename
    parent = wt_path.parent
    name = wt_path.name
    cmd = ["tar", "-czf", str(archive_path), "-C", str(parent), name]
    try:
        out = subprocess.run(
            cmd, capture_output=True, text=True, timeout=config.lane_w.archive_timeout_s)
    except subprocess.TimeoutExpired:
        return False, "tar timeout"
    if out.returncode != 0:
        return False, _safe_report_error(
            out.stderr, limit=config.logging.report_error_max_chars)
    # Integrity check: a malformed or huge archive could hang tar. Keep that
    # failure scoped to this archive instead of killing the rest of the sweep.
    try:
        verify = subprocess.run(["tar", "-tzf", str(archive_path)],
                                capture_output=True, text=True,
                                timeout=config.lane_w.archive_verify_timeout_s)
        if verify.returncode != 0:
            try:
                archive_path.unlink(missing_ok=True)
            except OSError:
                pass
            return False, (
                "archive integrity check failed: "
                f"{_safe_report_error(verify.stderr, limit=config.logging.report_error_max_chars)}"
            )
    except subprocess.TimeoutExpired:
        try:
            archive_path.unlink(missing_ok=True)
        except OSError:
            pass
        return False, "archive integrity check timed out (possible malformed archive)"
    return True, ""


def _lane_w_eligible(
    repo_root: pathlib.Path, config: Config, *,
    branch: str | None, head: str, trunk_sha: str,
    allow_detached_removal: bool = False,
) -> tuple[bool, str, dict[str, Any] | None]:
    """Verify Lane H/R eligibility from inside Lane W (ref still exists).

    Detached-HEAD worktrees are refused by default. If an operator makes an
    exploratory commit in a detached worktree, resets back to trunk, then runs
    Lane W, the worktree's HEAD is now eligible but the reset-away commit lives
    only in the worktree's reflog. Lane W's archive captures the post-reset
    working tree, not the orphaned commit; `git worktree remove` deletes the
    worktree's reflog with the admin entry, and the commit becomes unreachable
    after gc. Require explicit --allow-detached-removal to override after
    accepting that reflog-only commits in detached worktrees will not be
    preserved by the archive.
    """
    if branch is None:
        if not allow_detached_removal:
            # Distinct reason prefix so Lane W can label this as
            # 'refused-detached-head' in the audit log instead of burying it
            # under the generic 'skipped-not-eligible' action.
            return (False, _REFUSED_DETACHED_SENTINEL
                    + " detached-HEAD worktree refused "
                    "(use --allow-detached-removal to override; "
                    "reflog-only commits are not preserved by the archive)", None)
        # detached: HEAD must be ancestor of trunk
        if is_ancestor(repo_root, head, trunk_sha):
            return True, "detached HEAD ancestor of trunk (--allow-detached-removal)", None
        return False, "detached HEAD not ancestor of trunk", None
    if is_ancestor(repo_root, head, trunk_sha):
        return True, "ancestor of trunk", None
    # gh path (Lane W is operator-invoked; network is acceptable here)
    prs, err = gh_merged_pr_for_branch_cached(repo_root, config, branch, head)
    if prs is None:
        return False, f"not ancestor and gh unavailable ({err})", None
    match = find_matching_merged_pr(prs, head, config.trunk_branch, config.origin_owner)
    if match is None:
        return False, "no matching merged PR for tip SHA", None
    return True, f"gh PR #{match.get('number')} merged headRefOid==tip", match


def run_lane_w(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool,
    discard_ignored: bool, remove_nested: bool, discard_hidden: bool,
    invoke_root: pathlib.Path | None = None,
    allow_detached_removal: bool = False,
    trunk_sha_override: str | None = None,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = effective_trunk_sha(repo_root, config, trunk_sha_override)
    if not trunk_sha:
        return records
    # `cur` must reflect the invoker's branch, not repo_root's. If the operator
    # runs Lane W from inside a feature worktree while repo_root resolves to the
    # main worktree, the active-worktree skip would not protect the invoker's
    # worktree.
    invoke_root = invoke_root or repo_root
    cur = current_branch(invoke_root)
    skip_branches = protected_branch_names(config, cur, keep)

    worktrees = list_worktrees(repo_root)
    main_common = git_common_dir(repo_root)
    main_root = main_common.parent

    lock_path = main_common / LOCK_FILE
    fd = _acquire_lock(lock_path)
    if fd is None:
        if not quiet:
            print(f"[{SCRIPT_NAME}] lane W: another instance holds the lock; aborting",
                  file=sys.stderr)
        return records

    try:
        main_root_resolved = main_root.resolve()
        invoke_root_resolved = invoke_root.resolve()
        for wt in worktrees:
            wt_path_resolved = wt.path.resolve()
            try:
                # skip main checkout and currently active worktree
                # explicitly, in case branch resolution missed it (for example
                # detached HEAD in the invoker). Resolve paths consistently so
                # macOS /tmp ↔ /private/tmp symlinks cannot defeat the check.
                # Keep the `wt.branch is not None` guard so a detached invoker
                # does not make every detached worktree match via None == None;
                # the detached invoker itself is protected by the path check.
                if (wt_path_resolved == main_root_resolved
                        or (wt.branch is not None and wt.branch == cur)
                        or wt_path_resolved == invoke_root_resolved):
                    continue
                if wt.branch in skip_branches:
                    continue
                label = wt.branch or f"detached-{wt.head[:8]}"
                # If the worktree dir was manually deleted from disk, git
                # commands on the missing path would raise CalledProcessError.
                # Check existence first so the operator gets a clean diagnostic
                # and a hint to run `git worktree prune`.
                if not wt.path.is_dir():
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "skipped-missing-dir",
                                    "reason": "worktree directory does not exist on disk; "
                                              "run `git worktree prune` to clean up the stale admin entry"})
                    continue
                eligible, reason, match = _lane_w_eligible(
                    repo_root, config, branch=wt.branch, head=wt.head, trunk_sha=trunk_sha,
                    allow_detached_removal=allow_detached_removal,
                )
                if not eligible:
                    # Map the detached-refusal sentinel to a distinct action
                    # label so doctor / audit-log queries can find these
                    # specifically rather than digging through the generic
                    # skipped-not-eligible action.
                    if reason.startswith(_REFUSED_DETACHED_SENTINEL):
                        action = "refused-detached-head"
                        reason = reason[len(_REFUSED_DETACHED_SENTINEL):].strip()
                    else:
                        action = "skipped-not-eligible"
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": action,
                                    "reason": reason})
                    continue

                # Filesystem guards (refuse-by-default)
                hidden = has_hidden_index_bits(wt.path)
                if hidden and not discard_hidden:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "refused-hidden-index-bits",
                                    "reason": f"{len(hidden)} file(s) with assume-unchanged/skip-worktree"})
                    continue
                ignored = has_ignored_content(wt.path)
                if ignored and not discard_ignored:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "refused-ignored-content",
                                    "reason": f"{len(ignored)} ignored path(s); use --discard-ignored to override"})
                    continue
                nested = has_nested_git(wt.path)
                if nested and not remove_nested:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "refused-nested-git",
                                    "reason": f"{len(nested)} nested .git path(s); use --remove-nested-repos"})
                    continue
                clean, clean_reason = is_worktree_clean(wt.path)
                if not clean:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "refused-dirty",
                                    "reason": f"uncommitted changes: {clean_reason}"})
                    continue

                quarantine = _quarantine_target(config, repo_root, label, wt.head, wt.path)
                if apply:
                    # TOCTOU revalidate right before archive (still under our lock).
                    # Re-run hidden-bits + ignored too: another process could have
                    # set assume-unchanged/skip-worktree OR dropped an ignored file
                    # in the window since the upfront guards.
                    clean2, clean_reason2 = is_worktree_clean(wt.path)
                    if not clean2:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path), "action": "refused-dirty-toctou",
                                        "reason": f"TOCTOU: {clean_reason2}"})
                        continue
                    hidden2 = has_hidden_index_bits(wt.path)
                    if hidden2 and not discard_hidden:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "refused-hidden-index-bits-toctou",
                                        "reason": f"TOCTOU: {len(hidden2)} hidden-bit file(s)"})
                        continue
                    ignored2 = has_ignored_content(wt.path)
                    if ignored2 and not discard_ignored:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "refused-ignored-content-toctou",
                                        "reason": f"TOCTOU: {len(ignored2)} ignored path(s)"})
                        continue
                    # prepare quarantine dir; archive + manifest live INSIDE it.
                    # Write a minimal manifest IMMEDIATELY so a crash between here
                    # and the final manifest update still leaves a recoverable
                    # trail.
                    quarantine.mkdir(parents=True, exist_ok=True)
                    archive_path = quarantine / "worktree.tar.gz"
                    minimal_manifest = {
                        "branch": wt.branch, "tip_sha_at_archive_time": wt.head,
                        "moved_from": str(wt.path), "archive": str(archive_path),
                        "archived_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                        "eligibility": reason,
                        "worktree_remove_ok": False,        # set True after remove
                        "branch_delete_ok": False,          # set after branch delete
                        "backup_ref": None,                 # set after backup write
                        "final_tip_sha": None,              # set after re-read
                    }
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(minimal_manifest, indent=2, sort_keys=True))
                    # tar the worktree to quarantine + verify integrity
                    ok, err = _archive_worktree(wt.path, archive_path, config)
                    if not ok:
                        rec = {"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "archive-failed",
                                "reason": err, "quarantine_path": str(quarantine)}
                        records.append(rec)
                        write_audit(repo_root, config, rec)
                        continue
                    # remove the worktree (plain; refuses if dirty — final TOCTOU safety net)
                    # Audit the recovery_hint before the remove call. If a crash
                    # happens between remove and manifest flip, the audit log
                    # still points at the quarantine.
                    write_audit(repo_root, config, {
                        "lane": "W", "branch": label, "tip_sha": wt.head,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "archive_path": str(archive_path),
                        "action": "worktree-removal-attempted",
                        "reason": "worktree archive written; removal attempt imminent",
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          "archive": str(archive_path)},
                    })
                    rm = _git(repo_root, ["worktree", "remove", str(wt.path)], check=False)
                    if rm.returncode != 0:
                        rec = {"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "remove-failed-after-archive",
                                "reason": _safe_report_error(
                                    rm.stderr, limit=config.logging.report_error_max_chars),
                                "quarantine_path": str(quarantine)}
                        records.append(rec)
                        write_audit(repo_root, config, rec)
                        continue
                    # Worktree successfully removed. Flip the manifest's
                    # worktree_remove_ok RIGHT NOW (before branch-delete / final
                    # manifest write) so a crash between here and the end of the
                    # iteration leaves the quarantine entry purge-eligible.
                    minimal_manifest["worktree_remove_ok"] = True
                    minimal_manifest["worktree_removed_at"] = (
                        dt.datetime.now(dt.timezone.utc).isoformat())
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(minimal_manifest, indent=2, sort_keys=True))
                    # Worktree gone. Re-read the branch tip now; a commit may
                    # have landed in the worktree between list_worktrees() and
                    # now. If it moved, we must not delete with the stale SHA.
                    fresh_tip = wt.head
                    if wt.branch:
                        fresh_tip_out = _git(repo_root, ["rev-parse", wt.branch], check=False)
                        if fresh_tip_out.returncode == 0:
                            fresh_tip = fresh_tip_out.stdout.strip()
                    # Audit IMMEDIATELY so a crash between here and branch-delete
                    # leaves a forensic trail with the recovery hint.
                    rec_worktree_removed = {
                        "lane": "W", "branch": label, "tip_sha": fresh_tip,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "action": "worktree-removed-branch-pending",
                        "reason": "worktree archived and removed; branch delete pending",
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          "archive": str(archive_path)},
                    }
                    write_audit(repo_root, config, rec_worktree_removed)
                    backup_ref = None
                    branch_action: str
                    err = ""
                    if wt.branch:
                        if not SHA_RE.match(fresh_tip):
                            branch_action = "branch-delete-refused-missing-ref"
                            err = "branch ref did not resolve after worktree removal"
                        else:
                            fresh_ok, fresh_reason, _ = _lane_w_eligible(
                                repo_root, config, branch=wt.branch, head=fresh_tip,
                                trunk_sha=trunk_sha,
                                allow_detached_removal=allow_detached_removal,
                            )
                            if not fresh_ok:
                                branch_action = "branch-delete-refused-tip-not-eligible"
                                err = f"fresh tip not eligible after worktree removal: {fresh_reason}"
                            else:
                                backup_ref = write_backup_ref(repo_root, wt.branch, fresh_tip)
                                # CAS delete with the FRESH tip (not the stale wt.head).
                                ok_del, err_del = delete_branch_ref_cas(
                                    repo_root, wt.branch, fresh_tip,
                                    report_error_max_chars=config.logging.report_error_max_chars)
                                branch_action = "branch-deleted" if ok_del else "branch-delete-failed"
                                err = err_del if not ok_del else ""
                    else:
                        branch_action = "no-bound-branch"
                    # finalize manifest
                    final_manifest = {
                        **minimal_manifest,
                        "worktree_remove_ok": True,
                        "branch_delete_ok": branch_action == "branch-deleted",
                        "backup_ref": backup_ref,
                        "final_tip_sha": fresh_tip,
                        "tip_drifted": fresh_tip != wt.head,
                    }
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(final_manifest, indent=2, sort_keys=True))
                    rec = {
                        "lane": "W", "branch": label, "tip_sha": fresh_tip,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "action": branch_action, "reason": err, "backup_ref": backup_ref,
                        "tip_drifted": fresh_tip != wt.head,
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          **({"ref": backup_ref, "sha": fresh_tip} if backup_ref else {})},
                    }
                    records.append(rec)
                    write_audit(repo_root, config, rec)
                else:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "would-archive-and-remove",
                                        "reason": reason,
                                        "quarantine_path": str(quarantine),
                                        "ignored_count": len(ignored),
                                        "hidden_index_count": len(hidden),
                                        "nested_git_count": len(nested)})
            except Exception as exc:  # noqa: BLE001 — never let one worktree kill the sweep
                # Log + continue; never break the lane over a single failure.
                rec = {"lane": "W", "branch": getattr(wt, "branch", None) or "<unknown>",
                        "tip_sha": getattr(wt, "head", ""), "action": "iteration-exception",
                        "reason": f"{type(exc).__name__}: {exc}"}
                records.append(rec)
                try:
                    write_audit(repo_root, config, rec)
                except (OSError, TypeError, ValueError):
                    pass
    finally:
        _release_lock(fd)

    if not quiet:
        _print_lane_summary("W", records, apply)
    return records


def scan_subtree_latest_mtime(path: pathlib.Path) -> tuple[float, int]:
    try:
        root_info = path.lstat()
    except OSError:
        return 0.0, 1
    latest_mtime = float(root_info.st_mtime)
    skipped = 0
    stack = [(path, root_info)]
    while stack:
        current_path, info = stack.pop()
        latest_mtime = max(latest_mtime, float(info.st_mtime))
        if not stat.S_ISDIR(info.st_mode):
            continue
        try:
            with os.scandir(current_path) as entries:
                for entry in entries:
                    child_path = current_path / entry.name
                    try:
                        child_info = child_path.lstat()
                    except FileNotFoundError:
                        skipped += 1
                        continue
                    except OSError:
                        skipped += 1
                        continue
                    stack.append((child_path, child_info))
        except OSError:
            skipped += 1
    return latest_mtime, skipped


def clean_merged_process_cwd_from_proc(pid: int) -> pathlib.Path | None:
    base = pathlib.Path(os.environ.get("CLEAN_MERGED_PROCESS_CWD_BASE", "/proc"))
    try:
        return (base / str(pid) / "cwd").resolve(strict=True)
    except (OSError, RuntimeError):
        return None


def clean_merged_process_cwd_from_lsof(pid: int, *, timeout_s: float) -> tuple[pathlib.Path | None, str | None]:
    if shutil.which("lsof") is None:
        return None, f"process cwd visibility unavailable for pid {pid}: lsof not found"
    try:
        result = subprocess.run(
            ["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout_s,
        )
    except (subprocess.TimeoutExpired, OSError) as exc:
        return None, f"process cwd visibility unavailable for pid {pid}: {exc}"
    if result.returncode != 0:
        reason = result.stderr.strip() or result.stdout.strip() or str(result.returncode)
        return None, f"process cwd visibility unavailable for pid {pid}: lsof failed: {reason}"
    for line in result.stdout.splitlines():
        if line.startswith("n") and line[1:]:
            try:
                return pathlib.Path(line[1:]).resolve(strict=False), None
            except (OSError, RuntimeError) as exc:
                return None, f"process cwd visibility unavailable for pid {pid}: {exc}"
    return None, f"process cwd visibility unavailable for pid {pid}: lsof did not report cwd"


def clean_merged_process_cwd(pid: int, *, timeout_s: float) -> tuple[pathlib.Path | None, str | None]:
    proc_cwd = clean_merged_process_cwd_from_proc(pid)
    if proc_cwd is not None:
        return proc_cwd, None
    return clean_merged_process_cwd_from_lsof(pid, timeout_s=timeout_s)


def path_is_or_inside(path: pathlib.Path, parent: pathlib.Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def command_matches_patterns(command: str, patterns: tuple[str, ...]) -> bool:
    try:
        from rust_verification import matching_process_pattern
    except Exception:
        tokens = command.split()
        basenames = {pathlib.Path(token).name for token in tokens}
        return any(pattern in basenames for pattern in patterns)
    return matching_process_pattern(command, list(patterns)) is not None


def command_may_reference_rust_target(command: str, patterns: tuple[str, ...]) -> bool:
    if command_matches_patterns(command, patterns):
        return True
    try:
        from rust_verification import (
            command_may_be_renamed_cargo,
            command_may_launch_build,
            command_may_launch_rust,
        )
    except Exception:
        return True
    return (
        command_may_launch_rust(command)
        or command_may_be_renamed_cargo(command)
        or command_may_launch_build(command)
    )


def active_target_dir_processes(
    worktree: pathlib.Path, target: pathlib.Path, config: LaneTConfig,
) -> tuple[list[dict[str, Any]], str | None]:
    try:
        ps = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=config.process_list_timeout_s,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError) as exc:
        return [], f"process visibility unavailable: {exc}"
    if ps.returncode != 0:
        return [], f"process visibility unavailable: {ps.stderr.strip() or ps.returncode}"
    active: list[dict[str, Any]] = []
    worktree_resolved = worktree.resolve()
    target_resolved = target.resolve()
    matching_processes: list[tuple[int, str]] = []
    for line in ps.stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        pid_text, _, command = stripped.partition(" ")
        try:
            pid = int(pid_text)
        except ValueError:
            continue
        command_mentions_target = str(target_resolved) in command
        command_mentions_worktree = str(worktree_resolved) in command
        if (
            not command_mentions_target
            and not command_mentions_worktree
            and not command_may_reference_rust_target(command, config.active_process_patterns)
        ):
            continue
        matching_processes.append((pid, command))
    for pid, command in matching_processes:
        cwd, cwd_error = clean_merged_process_cwd(pid, timeout_s=config.cwd_visibility_timeout_s)
        command_mentions_target = str(target_resolved) in command
        cwd_related = cwd is not None and (
            path_is_or_inside(cwd, worktree_resolved) or path_is_or_inside(cwd, target_resolved)
        )
        if cwd_related or command_mentions_target:
            active.append({
                "pid": pid,
                "command": command,
                **({"cwd": str(cwd)} if cwd is not None else {}),
            })
        elif cwd_error is not None:
            return [], cwd_error
    return active, None


def run_lane_t(
    repo_root: pathlib.Path, config: Config, *, apply: bool, keep: set[str], quiet: bool,
    invoke_root: pathlib.Path | None = None,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    lane_config = config.lane_t
    if lane_config is None:
        return records
    invoke_root = invoke_root or repo_root
    repo_root_resolved = repo_root.resolve()
    invoke_root_resolved = invoke_root.resolve()
    now = time.time()
    cutoff = now - (lane_config.idle_after_days * 24 * 60 * 60)
    for wt in list_worktrees(repo_root):
        wt_path_resolved = wt.path.resolve()
        if wt_path_resolved in {repo_root_resolved, invoke_root_resolved}:
            continue
        label = wt.branch or "<detached>"
        if wt.branch in keep:
            continue
        target = wt.path / lane_config.target_dir_name
        if not target.exists():
            continue
        if not target.is_dir() or target.is_symlink():
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-not-directory",
                "reason": "configured target-dir path is not a real directory",
            })
            continue
        latest_mtime, skipped = scan_subtree_latest_mtime(target)
        if skipped:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-scan-incomplete",
                "reason": "target-dir scan skipped entries",
                "skipped_entries": skipped,
            })
            continue
        if latest_mtime > cutoff:
            continue
        if not apply:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-reap-candidate",
                "reason": f"target dir idle for at least {lane_config.idle_after_days} days",
                "latest_mtime": latest_mtime,
            })
            continue
        active, visibility_error = active_target_dir_processes(wt.path, target, lane_config)
        if visibility_error:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-process-visibility",
                "reason": visibility_error,
            })
            continue
        if active:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-active-process",
                "reason": "active Cargo/Rust process references target dir",
                "active_processes": active,
            })
            continue
        latest_mtime_before_delete, skipped_before_delete = scan_subtree_latest_mtime(target)
        if skipped_before_delete:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-scan-incomplete",
                "reason": "target-dir scan skipped entries before deletion",
                "skipped_entries": skipped_before_delete,
            })
            continue
        if latest_mtime_before_delete != latest_mtime or latest_mtime_before_delete > cutoff:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-refused-changed-before-delete",
                "reason": "target dir changed after eligibility scan",
                "latest_mtime": latest_mtime_before_delete,
            })
            continue
        try:
            shutil.rmtree(target)
        except OSError as exc:
            records.append({
                "lane": "T", "branch": label, "tip_sha": wt.head,
                "worktree": str(wt.path), "target_dir": str(target),
                "action": "target-dir-reap-failed",
                "reason": _safe_report_error(str(exc), limit=config.logging.report_error_max_chars),
            })
            continue
        records.append({
            "lane": "T", "branch": label, "tip_sha": wt.head,
            "worktree": str(wt.path), "target_dir": str(target),
            "action": "target-dir-reaped",
            "reason": f"target dir idle for at least {lane_config.idle_after_days} days",
            "latest_mtime": latest_mtime,
        })
    if not quiet:
        _print_lane_summary("T", records, apply)
    return records


# ---------------------------------------------------------------------------
# Purge / prune / doctor


def cmd_purge_quarantine(
    repo_root: pathlib.Path, config: Config, *, grace_days: int | None, quiet: bool,
) -> int:
    base = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    if not base.is_dir():
        if not quiet:
            print(f"[{SCRIPT_NAME}] quarantine absent: {base}")
        return 0
    grace = grace_days if grace_days is not None else config.lane_w.quarantine_grace_days
    cutoff = time.time() - grace * 86400
    # Acquire the same lock Lane W holds so a concurrent sweep can't race us
    # mid-manifest-write.
    main_common = git_common_dir(repo_root)
    lock_path = main_common / LOCK_FILE
    fd = _acquire_lock(lock_path)
    if fd is None:
        if not quiet:
            print(f"[{SCRIPT_NAME}] purge: another instance holds the lock; aborting",
                  file=sys.stderr)
        return 0
    purged = 0
    skipped = 0
    total_bytes_freed = 0
    try:
        for child in base.iterdir():
            if not child.is_dir():
                continue
            manifest_file = child / "clean-merged.manifest.json"
            if not manifest_file.is_file():
                skipped += 1
                continue
            try:
                manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                skipped += 1
                continue
            if not manifest.get("worktree_remove_ok"):
                # A dir with manifest but worktree_remove_ok=False is a stuck
                # half-state. Hold for grace, then purge as cruft unless an
                # intact archive is present; that archive may be the only
                # recovery surface for an already-removed worktree.
                try:
                    mtime = child.stat().st_mtime
                except OSError:
                    skipped += 1
                    continue
                if mtime > cutoff:
                    continue
                archive_file = child / "worktree.tar.gz"
                # If a prior run already verified this archive and recorded
                # verified_archive_at, skip the tar call entirely. Also require
                # the archive file to exist so the flag alone cannot pin an
                # empty dir forever.
                already_verified = (bool(manifest.get("verified_archive_at"))
                                    and archive_file.is_file())
                if already_verified:
                    write_audit(repo_root, config, {
                        "lane": "W", "branch": manifest.get("branch"),
                        "action": "quarantine-cruft-skipped-verified-archive",
                        "quarantine_path": str(child),
                        "reason": "previously-verified archive present; skipping re-verify",
                    })
                    skipped += 1
                    continue
                if archive_file.is_file():
                    try:
                        verify = subprocess.run(["tar", "-tzf", str(archive_file)],
                                                capture_output=True, text=True,
                                                timeout=config.lane_w.archive_verify_timeout_s)
                        if verify.returncode == 0:
                            # Verified archive present — refuse to delete; surface.
                            # Touch mtime and persist verified_archive_at so
                            # future purge runs can skip re-verifying an intact
                            # pinned archive.
                            try:
                                os.utime(child, None)
                            except OSError:
                                pass
                            try:
                                manifest["verified_archive_at"] = (
                                    dt.datetime.now(dt.timezone.utc).isoformat())
                                _atomic_write_text(manifest_file, json.dumps(
                                    manifest, indent=2, sort_keys=True))
                            except (OSError, ValueError):
                                pass
                            write_audit(repo_root, config, {
                                "lane": "W", "branch": manifest.get("branch"),
                                "action": "quarantine-cruft-skipped-verified-archive",
                                "quarantine_path": str(child),
                                "reason": "worktree_remove_ok=False but verified archive present; "
                                          "operator must remove explicitly if unwanted",
                            })
                            skipped += 1
                            continue
                    except Exception as exc:
                        write_audit(repo_root, config, {
                            "lane": "W", "branch": manifest.get("branch"),
                            "action": "quarantine-cruft-skipped-archive-verify-error",
                            "quarantine_path": str(child),
                            "reason": f"archive verification failed with exception: {exc}",
                        })
                        skipped += 1
                        continue
                # No archive (archive-failed) or corrupt archive — safe to purge.
                try:
                    size = sum(f.stat().st_size for f in child.rglob("*") if f.is_file())
                except OSError:
                    size = 0
                shutil.rmtree(child, ignore_errors=True)
                total_bytes_freed += size
                write_audit(repo_root, config, {
                    "lane": "W", "branch": manifest.get("branch"),
                    "action": "quarantine-purged-incomplete",
                    "quarantine_path": str(child), "bytes_freed": size,
                    "reason": "worktree_remove_ok was False, no usable archive; "
                              "purged as cruft after grace",
                })
                purged += 1
                continue
            try:
                mtime = child.stat().st_mtime
            except OSError:
                continue
            if mtime > cutoff:
                continue
            try:
                size = sum(f.stat().st_size for f in child.rglob("*") if f.is_file())
            except OSError:
                size = 0
            shutil.rmtree(child, ignore_errors=True)
            total_bytes_freed += size
            write_audit(repo_root, config, {
                "lane": "W", "branch": manifest.get("branch"),
                "tip_sha": manifest.get("tip_sha"), "action": "quarantine-purged",
                "quarantine_path": str(child), "bytes_freed": size,
            })
            purged += 1
    finally:
        _release_lock(fd)
    if not quiet:
        print(f"[{SCRIPT_NAME}] purge: {purged} purged, {skipped} skipped "
              f"(no/failed manifest), {total_bytes_freed} bytes freed")
    return purged


def cmd_prune_backups(
    repo_root: pathlib.Path, config: Config, *, days: int, quiet: bool,
) -> int:
    """Prune backup refs older than `days`.

    Backup ref names embed the creation timestamp: refs/clean-merged/<branch>-<sha>-<unix_ts>.
    Parse that embedded creation timestamp rather than the target commit date,
    so a backup created today for an old commit still gets the full recovery
    window.
    """
    out = _git(repo_root, ["for-each-ref", "--format=%(refname)", "refs/clean-merged/"])
    cutoff = time.time() - days * 86400
    pruned = 0
    skipped_unparseable = 0
    for line in out.stdout.splitlines():
        ref = line.strip()
        if not ref:
            continue
        # Ref name ends with <safe-branch>-<short-sha>-<unix-ts>. Parse from the right.
        m = re.match(r"^(.+)-([0-9a-f]{7,40})-(\d+)$", ref[len("refs/clean-merged/"):])
        if not m:
            skipped_unparseable += 1
            continue
        try:
            created_ts = int(m.group(3))
        except ValueError:
            skipped_unparseable += 1
            continue
        if created_ts > cutoff:
            continue
        _git(repo_root, ["update-ref", "-d", ref], check=False)
        pruned += 1
    if not quiet:
        print(f"[{SCRIPT_NAME}] prune-backups: {pruned} removed (>{days}d old), "
              f"{skipped_unparseable} unparseable skipped")
    return pruned


def _is_disabled(env_value: str | None) -> bool:
    """Shared kill-switch truthiness so bash hooks and Python agree.

    Shared rule: empty/0/false/no/off (case-insensitive) = enabled;
    anything else = disabled.

    The documented contract is ASCII whitespace only. Python's `.strip()`
    would also strip Unicode whitespace while bash's `[[:space:]]` is
    locale-dependent and usually ASCII-only. Operators should set
    CLEAN_MERGED_DISABLED to a bare ASCII value.
    """
    if env_value is None:
        return False
    v = env_value.strip().lower()
    return v not in ("", "0", "false", "no", "off")


def _python_runtime_status() -> str:
    toml_status = "tomllib=yes" if _TOMLLIB_IMPORT_ERROR is None else "tomllib=no"
    return f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro} ({toml_status})"


def _redirect_output_to(path: pathlib.Path, max_bytes: int, rotated_retention_days: int) -> Any:
    handle = _open_rotating_log(
        path, max_bytes, rotated_retention_days=rotated_retention_days)
    # dup2 gives stdout/stderr independent descriptors; the returned handle
    # only keeps the target open until the dup has completed.
    os.dup2(handle.fileno(), sys.stdout.fileno())
    os.dup2(handle.fileno(), sys.stderr.fileno())
    return handle


def _hook_runtime_outside_allowed_problem(
    hook_name: str,
    hook_file: pathlib.Path,
) -> str:
    return (
        f"hook {hook_name} runtime is outside allowed state; "
        f"remove {hook_file} and run `just setup`"
    )


def _diagnose_hook_install_state(
    repo_root: pathlib.Path,
    invoke_root: pathlib.Path,
    common: pathlib.Path,
    problems: list[str],
) -> None:
    expected_hooks_dir = common / "hooks"
    committed_source_snapshots: dict[str, HookSnapshot] = {}
    for rel_path in _tracked_hook_source_paths(repo_root):
        try:
            snapshot = _tracked_hook_snapshot(repo_root, rel_path)
        except CleanMergedError:
            continue
        committed_source_snapshots[snapshot.hook_name] = snapshot
    home_dir = _resolve_home_dir()
    try:
        hook_manifest = _load_hook_manifest(common)
        manifest_hooks = _hook_manifest_hooks(hook_manifest)
        manifest_shadowed = _hook_manifest_shadowed(hook_manifest)
        manifest_loaded = True
    except CleanMergedError as exc:
        manifest_hooks = {}
        manifest_shadowed = {}
        manifest_loaded = False
        problems.append(str(exc))
        problems.append(
            f"repair or remove hook manifest {_hook_manifest_path(common)} "
            "and run `just setup`"
        )

    unsupported_hooks_path = False
    try:
        _source_scope, active_hooks_dir_raw = _configured_hooks_path(
            invoke_root=invoke_root,
            source_root=repo_root,
        )
    except CleanMergedError as exc:
        unsupported_hooks_path = True
        active_hooks_dir_raw = _git_config_value(invoke_root, ["--get", "core.hooksPath"])
        problems.append(str(exc))
        problems.append("remove or convert unsupported core.hooksPath config, then run `just setup`")

    if active_hooks_dir_raw:
        active_hooks_dir = _resolve_hooks_path(
            invoke_root,
            active_hooks_dir_raw,
            home_dir=home_dir,
        )
        print(f"  core.hooksPath           = {active_hooks_dir}")
        if not unsupported_hooks_path:
            if not _same_path(active_hooks_dir, expected_hooks_dir):
                problems.append(
                    "core.hooksPath is not git-common hooks directory (run `just setup`)"
                )
            for h in CLEAN_MERGED_HOOKS:
                hook_file = active_hooks_dir / h
                present = hook_file.exists() or hook_file.is_symlink()
                regular_file = hook_file.is_file() and not hook_file.is_symlink()
                executable = regular_file and _is_executable(hook_file)
                source_snapshot = committed_source_snapshots.get(h)
                source_match = (
                    regular_file
                    and source_snapshot is not None
                    and hook_file.read_bytes() == source_snapshot.content
                )
                entry = manifest_hooks.get(h)
                manifest_match = (
                    regular_file
                    and isinstance(entry, dict)
                    and entry.get("runtime_sha256") == _file_sha256(hook_file)
                )
                print(
                    f"  hook {h:14s} exists={present} source_match={source_match} "
                    f"manifest_match={manifest_match} executable={executable}"
                )
                if not present:
                    problems.append(f"hook {h} missing (run `just setup`)")
                elif not regular_file:
                    problems.append(_hook_runtime_outside_allowed_problem(h, hook_file))
                elif (
                    manifest_loaded
                    and _same_path(active_hooks_dir, expected_hooks_dir)
                    and not manifest_match
                ):
                    problems.append(_hook_runtime_outside_allowed_problem(h, hook_file))
                elif _same_path(active_hooks_dir, expected_hooks_dir) and not source_match:
                    problems.append(
                        f"hook {h} runtime does not match tracked source (run `just setup`)"
                    )
                if _same_path(active_hooks_dir, expected_hooks_dir) and regular_file and not executable:
                    problems.append(f"hook {h} is not executable (run `just setup`)")
    else:
        print("  core.hooksPath           = (unset; hooks would live in .git/hooks)")
        problems.append("core.hooksPath unset; clean-merged hooks not active")

    dirty_sources = _dirty_tracked_hook_sources(repo_root)
    if dirty_sources:
        problems.append(
            "tracked hook source(s) have local changes: "
            + ", ".join(dirty_sources)
        )

    for hook_name, entry in sorted(manifest_hooks.items()):
        if not isinstance(entry, dict):
            problems.append(f"hook {hook_name} manifest entry is invalid")
            continue
        hook_file = expected_hooks_dir / hook_name
        runtime_present = hook_file.exists() or hook_file.is_symlink()
        runtime_regular_file = hook_file.is_file() and not hook_file.is_symlink()
        runtime_match = (
            runtime_regular_file
            and entry.get("runtime_sha256") == _file_sha256(hook_file)
        )
        source_file = _manifest_source_file(
            repo_root,
            entry,
            hook_name=hook_name,
            invoke_root=invoke_root,
            runtime_hooks_dir=expected_hooks_dir,
            home_dir=home_dir,
        )
        source_problem: str | None = None
        if (
            source_file is not None
            and entry.get("source_kind") == "active-hook"
            and entry.get("source_scope") == "default"
            and _is_shadowed_hook_backup_path(common, source_file)
        ):
            try:
                _validate_default_shadowed_hook_backup(
                    hook_name=hook_name,
                    entry=entry,
                    source_file=source_file,
                )
            except CleanMergedError as exc:
                source_problem = str(exc)
        if entry.get("source_kind") == "repo-source":
            source_snapshot = committed_source_snapshots.get(hook_name)
            source_match = (
                source_problem is None
                and source_snapshot is not None
                and entry.get("source_sha256") == source_snapshot.sha256
            )
        else:
            source_match = (
                source_problem is None
                and source_file is not None
                and source_file.is_file()
                and not source_file.is_symlink()
                and entry.get("source_sha256") == _file_sha256(source_file)
            )
        print(
            f"  manifest hook {hook_name:14s} runtime_match={runtime_match} "
            f"source_match={source_match}"
        )
        if hook_name not in CLEAN_MERGED_HOOKS and not runtime_match:
            if runtime_present:
                problems.append(_hook_runtime_outside_allowed_problem(hook_name, hook_file))
            else:
                problems.append(f"hook {hook_name} missing (run `just setup`)")
        if (
            entry.get("source_kind") == "repo-source"
            and hook_name not in CLEAN_MERGED_HOOKS
            and runtime_regular_file
            and not _is_executable(hook_file)
        ):
            problems.append(f"hook {hook_name} is not executable (run `just setup`)")
        if source_problem is not None:
            problems.append(source_problem)
        elif not source_match:
            problems.append(f"hook {hook_name} source changed since install")

    for hook_name, entries in sorted(manifest_shadowed.items()):
        if not isinstance(entries, list):
            problems.append(f"shadowed hook {hook_name} manifest entry is invalid")
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                problems.append(f"shadowed hook {hook_name} manifest entry is invalid")
                continue
            source_file = _manifest_source_file(
                repo_root,
                entry,
                hook_name=hook_name,
                invoke_root=invoke_root,
                runtime_hooks_dir=expected_hooks_dir,
                home_dir=home_dir,
            )
            source_problem: str | None = None
            if (
                source_file is not None
                and entry.get("source_scope") == "default"
                and _is_shadowed_hook_backup_path(common, source_file)
            ):
                try:
                    _validate_default_shadowed_hook_backup(
                        hook_name=hook_name,
                        entry=entry,
                        source_file=source_file,
                    )
                except CleanMergedError as exc:
                    source_problem = str(exc)
            source_match = (
                source_problem is None
                and source_file is not None
                and source_file.is_file()
                and not source_file.is_symlink()
                and entry.get("source_sha256") == _file_sha256(source_file)
            )
            print(
                f"  shadowed hook {hook_name:14s} source_match={source_match}"
            )
            if source_problem is not None:
                problems.append(source_problem)
            elif not source_match:
                problems.append(f"shadowed hook {hook_name} source changed since install")


def cmd_doctor_on_error(
    repo_root: pathlib.Path,
    invoke_root: pathlib.Path,
    exc: Exception,
) -> int:
    """Doctor path that runs even when config parse failed.

    main() catches ConfigError before the doctor dispatch and returned 0 — so
    the one failure doctor most needs to report was unsurfaced. This helper
    reports the config error and the install state, returns 1.
    """
    print(f"[{SCRIPT_NAME}] doctor")
    print(f"  CONFIG ERROR             = {exc}")
    print(f"  python                   = {_python_runtime_status()}")
    problems: list[str] = []
    try:
        common = git_common_dir(repo_root)
        print(f"  git-common-dir           = {common}")
        _diagnose_hook_install_state(repo_root, invoke_root, common, problems)
        # heartbeat freshness even on config error
        import datetime as _dt
        hb_paths = [
            common / "clean-merged.heartbeat",
            repo_root / ".git" / "clean-merged.heartbeat",
        ]
        for hb in hb_paths:
            if hb.is_file():
                try:
                    hb_ts = _dt.datetime.fromisoformat(hb.read_text(encoding="utf-8").strip())
                    age = _dt.datetime.now(_dt.timezone.utc) - hb_ts
                    print(f"  heartbeat age            = {age} (at {hb})")
                except (ValueError, OSError):
                    pass
                break
    except Exception as inner:  # noqa: BLE001
        print(f"  (additional error during doctor: {inner})")
    print()
    if problems:
        print(f"[{SCRIPT_NAME}] {len(problems)} hook/config problem(s):")
        for problem in problems:
            print(f"  - {problem}")
        print()
    print(f"[{SCRIPT_NAME}] config parse failed; lanes are silently halted. "
          "Fix the config to resume automatic cleanup.")
    return 1


def cmd_doctor(repo_root: pathlib.Path, invoke_root: pathlib.Path, config: Config) -> int:
    problems: list[str] = []
    print(f"[{SCRIPT_NAME}] doctor")

    # Python / tomllib availability
    print(f"  python                   = {_python_runtime_status()}")

    # config
    print(f"  config.enabled           = {config.enabled}")
    print(f"  config.trunk_branch      = {config.trunk_branch}")
    print(f"  config.remote_name       = {config.remote_name}")
    print(f"  config.origin_owner      = {config.origin_owner}")

    # trunk resolution
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    print(f"  trunk_sha                = {trunk_sha}")
    if not trunk_sha:
        problems.append("trunk ref does not resolve")

    # git-common-dir
    common = git_common_dir(repo_root)
    print(f"  git-common-dir           = {common}")
    _diagnose_hook_install_state(repo_root, invoke_root, common, problems)

    # remote.<remote>.prune must follow the configured remote name.
    prune_key = f"remote.{config.remote_name}.prune"
    prune = _git(repo_root, ["config", "--get", prune_key], check=False)
    print(f"  {prune_key:25s} = {prune.stdout.strip() or '(unset)'}")
    if prune.stdout.strip() != "true":
        problems.append(f"{prune_key} != true (run `just setup`)")

    # gh
    try:
        gh_check = subprocess.run(["gh", "--version"], capture_output=True, text=True)
        gh_ok = gh_check.returncode == 0
    except OSError:
        gh_ok = False
    print(f"  gh available             = {gh_ok}")
    if not gh_ok:
        problems.append("gh CLI not available; Lane R cannot run")

    cache_path = _gh_cache_path(repo_root)
    cache_problem = _gh_cache_health(cache_path)
    if cache_problem:
        print(f"  gh cache                 = invalid ({cache_path})")
        problems.append(f"gh cache invalid at {cache_path}: {cache_problem}")
    elif cache_path.is_file():
        print(f"  gh cache                 = ok ({cache_path})")
    else:
        print(f"  gh cache                 = (absent; will be created on Lane R)")

    # heartbeat freshness
    hb = _resolve_path(repo_root, config.logging.heartbeat_path)
    if hb.is_file():
        try:
            hb_ts = dt.datetime.fromisoformat(hb.read_text(encoding="utf-8").strip())
            age = dt.datetime.now(dt.timezone.utc) - hb_ts
            print(f"  heartbeat age            = {age}")
            if age > dt.timedelta(days=config.logging.heartbeat_stale_days):
                problems.append(f"heartbeat stale ({age}); hook may be silently broken")
        except (ValueError, OSError):
            problems.append("heartbeat present but unreadable")
    else:
        print("  heartbeat                = (none yet)")

    # Quarantine disk usage and pinned verified-archive cruft visibility.
    # Pinned state is intentional safety behavior, not a malfunction, so report
    # it as info rather than a problem. Predicate on archive presence instead
    # of an internal manifest flag so old pinned dirs and externally deleted
    # archives are reported accurately. Doctor does not re-run tar -tzf; here
    # "pinned" means "stuck dir with an archive file present," not "archive
    # verified intact."
    q = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    if q.is_dir():
        entries = [c for c in q.iterdir() if c.is_dir()]
        total_bytes = 0
        pinned = 0
        pinned_bytes = 0
        for c in entries:
            try:
                size = sum(f.stat().st_size for f in c.rglob("*") if f.is_file())
            except OSError:
                size = 0
            total_bytes += size
            mf = c / "clean-merged.manifest.json"
            if mf.is_file():
                try:
                    m = json.loads(mf.read_text(encoding="utf-8"))
                    archive_file = c / "worktree.tar.gz"
                    if not m.get("worktree_remove_ok") and archive_file.is_file():
                        pinned += 1
                        pinned_bytes += size
                except (OSError, json.JSONDecodeError):
                    pass
        print(f"  quarantine               = {len(entries)} entries, "
              f"{total_bytes / (1024 * 1024):.1f} MiB at {q}")
        if pinned:
            # INFO only — pinned state is intentional safety behavior, not a
            # problem. Don't make doctor return non-zero just because the
            # operator is keeping recovery archives.
            print(f"  quarantine pinned (info) = {pinned} archive-present dirs "
                  f"({pinned_bytes / (1024 * 1024):.1f} MiB) — kept intentionally; "
                  "remove manually when no longer needed")
    else:
        print(f"  quarantine               = (absent)")

    rotated_count, rotated_bytes = _rotated_log_usage(repo_root, config)
    print(f"  rotated logs             = {rotated_count} files "
          f"({rotated_bytes / (1024 * 1024):.1f} MiB)")

    # backup refs
    backups_out = _git(repo_root, ["for-each-ref", "refs/clean-merged/"], check=False)
    n_backups = len([l for l in backups_out.stdout.splitlines() if l.strip()])
    print(f"  backup refs              = {n_backups}")

    if problems:
        print()
        print(f"[{SCRIPT_NAME}] {len(problems)} problem(s):")
        for p in problems:
            print(f"  - {p}")
        return 1
    print()
    print(f"[{SCRIPT_NAME}] all green")
    return 0


# ---------------------------------------------------------------------------
# Output


def _print_lane_summary(lane: str, records: list[dict[str, Any]], apply: bool) -> None:
    if not records:
        return
    verb = "applied" if apply else "dry-run"
    print(f"[{SCRIPT_NAME}] lane {lane} {verb}:")
    for r in records:
        action = r.get("action", "?")
        branch = r.get("branch", "?")
        sha = (r.get("tip_sha") or "")[:12]
        extra = ""
        if r.get("worktree"):
            extra += f" wt={r['worktree']}"
        if r.get("quarantine_path"):
            extra += f" -> {r['quarantine_path']}"
        if r.get("reason"):
            extra += f" :: {r['reason']}"
        print(f"  {action:30s} {branch:40s} {sha}{extra}")


# ---------------------------------------------------------------------------
# CLI


LaneRunner = Callable[[], tuple[list[dict[str, Any]], bool]]


@dataclasses.dataclass(frozen=True)
class LaneStep:
    name: str
    run: LaneRunner
    stop_on_failure: bool = False


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog=SCRIPT_NAME, description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--apply", action="store_true",
                   help="execute mutations (default: dry-run)")
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--keep", action="append", default=[], metavar="BRANCH",
                   help="never delete this branch (repeatable)")
    p.add_argument("--lane", choices=("s", "h", "r", "w", "t"), help="run a single lane")
    p.add_argument("--sync-main", action="store_true",
                   help="run Lane S before cleanup (dry-run reports; --apply fetches/prunes and ff-only syncs trunk)")
    p.add_argument("--reconcile", action="store_true", help="run Lane R (gh)")
    p.add_argument("--include-worktrees", action="store_true", help="run Lane W")
    p.add_argument("--include-target-dirs", action="store_true", help="run Lane T")
    p.add_argument("--discard-ignored", action="store_true",
                   help="Lane W: override ignored-content refusal")
    p.add_argument("--remove-nested-repos", action="store_true",
                   help="Lane W: override nested-.git refusal")
    p.add_argument("--discard-hidden-index-bits", action="store_true",
                   help="Lane W: override assume-unchanged/skip-worktree refusal")
    p.add_argument("--allow-detached-removal", action="store_true",
                   help="Lane W: override detached-HEAD worktree refusal. "
                        "DANGEROUS: reflog-only commits in the detached worktree "
                        "are NOT preserved by the archive and become unreachable "
                        "after git gc. Only use when you have verified the detached "
                        "HEAD has no exploratory commits you want to keep.")
    p.add_argument("--purge-quarantine", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="purge quarantined worktrees older than DAYS")
    p.add_argument("--prune-backups", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="prune backup refs older than DAYS")
    p.add_argument("--doctor", action="store_true", help="run diagnostic")
    p.add_argument("--install-hooks", action="store_true",
                   help="install generated runtime hooks for just setup")
    p.add_argument("--only-if-current-trunk", action="store_true",
                   help=argparse.SUPPRESS)
    p.add_argument("--redirect-output-to-lane-r-log", action="store_true",
                   help=argparse.SUPPRESS)
    p.add_argument("--print-remote-name", action="store_true",
                   help=argparse.SUPPRESS)
    return p


def _lane_steps(
    args: argparse.Namespace, *,
    repo_root: pathlib.Path, config: Config, keep: set[str],
    invoke_root: pathlib.Path, apply: bool,
) -> list[LaneStep]:
    defer_cleanup = args.sync_main and args.lane != "s"
    trunk_authority: dict[str, str] = {}

    def sync() -> tuple[list[dict[str, Any]], bool]:
        records, ok = run_lane_s(repo_root, config, apply=apply, quiet=args.quiet,
                                 defer_cleanup=defer_cleanup)
        if ok:
            for record in records:
                if record.get("action") == "preview-fetched-trunk" and record.get("tip_sha"):
                    trunk_authority["sha"] = str(record["tip_sha"])
        return records, ok

    def h() -> tuple[list[dict[str, Any]], bool]:
        return run_lane_h(
            repo_root, config, apply=apply, keep=keep, quiet=args.quiet,
            trunk_sha_override=trunk_authority.get("sha"),
        ), True

    def r() -> tuple[list[dict[str, Any]], bool]:
        return run_lane_r(
            repo_root, config, apply=apply, keep=keep, quiet=args.quiet,
            trunk_sha_override=trunk_authority.get("sha"),
        ), True

    def w() -> tuple[list[dict[str, Any]], bool]:
        return run_lane_w(
            repo_root, config, apply=apply, keep=keep, quiet=args.quiet,
            discard_ignored=args.discard_ignored,
            remove_nested=args.remove_nested_repos,
            discard_hidden=args.discard_hidden_index_bits,
            invoke_root=invoke_root,
            allow_detached_removal=args.allow_detached_removal,
            trunk_sha_override=trunk_authority.get("sha"),
        ), True

    def t() -> tuple[list[dict[str, Any]], bool]:
        return run_lane_t(
            repo_root, config, apply=apply, keep=keep, quiet=args.quiet, invoke_root=invoke_root,
        ), True

    steps = {
        "s": LaneStep("S", sync, stop_on_failure=True),
        "h": LaneStep("H", h),
        "r": LaneStep("R", r),
        "w": LaneStep("W", w),
        "t": LaneStep("T", t),
    }
    if args.lane:
        plan = [steps[args.lane]]
    else:
        plan = [steps["h"]]
        if args.reconcile:
            plan.append(steps["r"])
        if args.include_worktrees:
            plan.append(steps["w"])
        if args.include_target_dirs:
            plan.append(steps["t"])
    if args.sync_main:
        plan = [step for step in plan if step.name != "S"]
        plan.insert(0, steps["s"])
    return plan


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    # Resolve to the main worktree root, not the current worktree's root. If
    # the operator runs Lane W from inside a worktree that Lane W removes, a
    # cwd-based repo_root becomes invalid mid-sweep. Keep the invoker root too
    # so Lane W can protect "the worktree I'm standing in" even when repo_root
    # is the main worktree.
    #
    # _resolve_repo_root and load_config can raise CleanMergedError; catch the
    # parent type so non-git dirs and bad common-dir resolution fail open.
    repo_root = pathlib.Path.cwd()
    invoke_root = repo_root
    try:
        invoke_root = _resolve_repo_root(pathlib.Path.cwd())
        repo_root = _main_worktree_root(invoke_root)
        if args.install_hooks:
            hooks_dir = install_hooks(
                invoke_root,
                source_root=repo_root,
                home_dir=_resolve_home_dir(),
            )
            if not args.quiet:
                print(f"[{SCRIPT_NAME}] installed hooks in {hooks_dir}")
            return 0
        config = load_config(repo_root)
    except CleanMergedError as exc:
        # Don't crash the hook chain on config or git-resolution errors.
        # If the operator explicitly asked for diagnostics, surface the
        # problem and exit non-zero.
        if not args.quiet:
            print(f"[{SCRIPT_NAME}] error: {exc}", file=sys.stderr)
        if args.install_hooks:
            return 1
        if args.print_remote_name:
            return 1
        if args.doctor:
            return cmd_doctor_on_error(repo_root=repo_root, invoke_root=invoke_root, exc=exc)
        return 0

    if args.print_remote_name:
        print(config.remote_name)
        return 0

    if args.redirect_output_to_lane_r_log:
        try:
            _redirect_output_to(
                _resolve_path(repo_root, config.logging.lane_r_log_path),
                config.logging.max_log_bytes,
                config.logging.rotated_log_retention_days,
            )
        except OSError as exc:
            if not args.quiet:
                print(f"[{SCRIPT_NAME}] lane R log redirect failed: {exc}", file=sys.stderr)

    if _is_disabled(os.environ.get("CLEAN_MERGED_DISABLED")) or not config.enabled:
        if not args.quiet:
            print(f"[{SCRIPT_NAME}] disabled (kill switch)", file=sys.stderr)
        return 0
    if args.only_if_current_trunk and current_branch(invoke_root) != config.trunk_branch:
        return 0

    keep = set(args.keep)
    apply = args.apply

    # Single-shot subcommands
    if args.doctor:
        return cmd_doctor(repo_root, invoke_root, config)
    if args.purge_quarantine is not None:
        grace = None if args.purge_quarantine == -1 else args.purge_quarantine
        cmd_purge_quarantine(repo_root, config, grace_days=grace, quiet=args.quiet)
        return 0
    if args.prune_backups is not None:
        days = config.backups.prune_after_days if args.prune_backups == -1 else args.prune_backups
        cmd_prune_backups(repo_root, config, days=days, quiet=args.quiet)
        return 0

    write_heartbeat(repo_root, config)

    all_records: list[dict[str, Any]] = []
    for step in _lane_steps(args, repo_root=repo_root, config=config, keep=keep,
                            invoke_root=invoke_root, apply=apply):
        records, ok = step.run()
        all_records += records
        if step.stop_on_failure and not ok:
            if not args.quiet:
                print(f"[{SCRIPT_NAME}] cleanup skipped because lane {step.name} "
                      "did not produce usable cleanup authority", file=sys.stderr)
            break

    # audit successful mutations.
    # Lane W records are audited inline (per-mutation) because Lane W is a
    # multi-step flow (archive -> remove -> branch-delete) where a crash between
    # steps must leave a forensic trail. Lane H/R are atomic single-line mutations
    # and are audited here at the end.
    for r in all_records:
        if r.get("lane") == "W":
            continue
        if r.get("lane") == "T" and apply:
            write_audit(repo_root, config, r)
            continue
        if r.get("lane") == "S" and apply:
            write_audit(repo_root, config, r)
            continue
        if r.get("action") in ("deleted", "branch-deleted"):
            write_audit(repo_root, config, r)
        elif r.get("action") in ("delete-refused", "delete-cas-refused",
                                  "move-failed", "branch-delete-failed"):
            write_audit(repo_root, config, r)

    return 0


if __name__ == "__main__":
    sys.exit(main())
