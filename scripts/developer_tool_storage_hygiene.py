#!/usr/bin/env python3
"""Developer-tool storage hygiene inventory and cleanup safety checks."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat as stat_module
import sys
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SURFACE_SECTIONS = (
    ("codex.log", ("codex", "log")),
    ("codex.sessions", ("codex", "sessions")),
    ("codex.sqlite", ("codex", "sqlite")),
    ("codex.archived_sessions", ("codex", "archived_sessions")),
    ("native_guidance.codex_history", ("native_guidance", "codex_history")),
    ("factory.log", ("factory", "log")),
    ("rustup.toolchains", ("rustup", "toolchains")),
)

PREFLIGHT_SECTION = ("preflight",)
SUPPORTED_POLICY_SCHEMA_VERSION = 1

REQUIRED_SURFACE_KEYS = (
    "path_family",
    "category",
    "growth_shape",
    "owner",
    "native_policy",
    "cleanup_mode",
)

REQUIRED_PREFLIGHT_KEYS = (
    "free_disk_warning_bytes",
    "free_disk_error_bytes",
    "owned_storage_warning_bytes",
    "owned_storage_error_bytes",
)

ROTATING_SURFACE_IDS = frozenset(("codex.log", "factory.log"))
SESSION_SURFACE_ID = "codex.sessions"
RUSTUP_SURFACE_ID = "rustup.toolchains"
OWNED_OWNER = "owned"
REPORT_ONLY_OWNER = "report_only"
OUT_OF_SCOPE_OWNER = "out_of_scope"
MUTATING_ACTIONS = frozenset(("rotate", "delete", "remove_tree"))
ALLOWED_OWNERS = frozenset((OWNED_OWNER, REPORT_ONLY_OWNER, OUT_OF_SCOPE_OWNER))
ALLOWED_CLEANUP_MODES = frozenset(("rotate", "ttl_prune", "toolchain_retention", "none"))
MUTATING_CLEANUP_MODES = frozenset(("rotate", "ttl_prune", "toolchain_retention"))
SURFACE_CLEANUP_MODES = {
    "codex.log": frozenset(("rotate", "none")),
    "codex.sessions": frozenset(("ttl_prune", "none")),
    "codex.sqlite": frozenset(("none",)),
    "codex.archived_sessions": frozenset(("none",)),
    "native_guidance.codex_history": frozenset(("none",)),
    "factory.log": frozenset(("rotate", "none")),
    "rustup.toolchains": frozenset(("toolchain_retention", "none")),
}


class PolicyError(ValueError):
    """Raised when the TOML policy is missing required safety data."""


@dataclass(frozen=True)
class PolicySurface:
    id: str
    path_family: str
    category: str
    growth_shape: str
    owner: str
    native_policy: str
    cleanup_mode: str
    active_writer_processes: tuple[str, ...]
    extra: dict[str, Any]

    def to_status_entry(self) -> dict[str, Any]:
        entry: dict[str, Any] = {
            "id": self.id,
            "path_family": self.path_family,
            "category": self.category,
            "growth_shape": self.growth_shape,
            "owner": self.owner,
            "native_policy": self.native_policy,
            "cleanup_mode": self.cleanup_mode,
            "active_writer_processes": list(self.active_writer_processes),
        }
        entry.update(self.extra)
        return entry


@dataclass(frozen=True)
class Policy:
    path: Path
    digest: str
    schema_version: int
    surfaces: tuple[PolicySurface, ...]
    adjacent_surfaces: tuple[PolicySurface, ...]
    preflight: dict[str, int]


def _section(data: dict[str, Any], path: tuple[str, ...]) -> dict[str, Any]:
    current: Any = data
    for key in path:
        if not isinstance(current, dict) or key not in current:
            raise PolicyError(f"missing policy section: {'.'.join(path)}")
        current = current[key]
    if not isinstance(current, dict):
        raise PolicyError(f"policy section is not a table: {'.'.join(path)}")
    return current


def _require_keys(section_id: str, section: dict[str, Any], keys: tuple[str, ...]) -> None:
    missing = [key for key in keys if key not in section]
    if missing:
        raise PolicyError(f"{section_id} missing required keys: {', '.join(missing)}")


def _read_string(section_id: str, section: dict[str, Any], key: str) -> str:
    value = section[key]
    if not isinstance(value, str) or not value:
        raise PolicyError(f"{section_id}.{key} must be a non-empty string")
    return value


def _read_string_list(section_id: str, section: dict[str, Any], key: str) -> tuple[str, ...]:
    value = section.get(key, [])
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise PolicyError(f"{section_id}.{key} must be a list of non-empty strings")
    return tuple(value)


def _string_tuple_from_extra(surface: PolicySurface, key: str) -> tuple[str, ...]:
    value = surface.extra.get(key, [])
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise PolicyError(f"{surface.id}.{key} must be a list of non-empty strings")
    return tuple(value)


def _read_positive_int(section_id: str, value: Any, key: str) -> int:
    if type(value) is not int or value <= 0:
        raise PolicyError(f"{section_id}.{key} must be a positive integer")
    return value


def _validate_owner_cleanup_mode(section_id: str, owner: str, cleanup_mode: str) -> None:
    if owner not in ALLOWED_OWNERS:
        raise PolicyError(f"{section_id}.owner must be one of: {', '.join(sorted(ALLOWED_OWNERS))}")
    if cleanup_mode not in ALLOWED_CLEANUP_MODES:
        raise PolicyError(
            f"{section_id}.cleanup_mode must be one of: {', '.join(sorted(ALLOWED_CLEANUP_MODES))}"
        )
    allowed_modes = SURFACE_CLEANUP_MODES.get(section_id)
    if allowed_modes is not None and cleanup_mode not in allowed_modes:
        raise PolicyError(
            f"{section_id}.cleanup_mode is not valid for this surface: "
            f"{', '.join(sorted(allowed_modes))}"
        )
    if owner != OWNED_OWNER and cleanup_mode != "none":
        raise PolicyError(f"{section_id}.owner/cleanup_mode combination is not cleanup-owned")
    if cleanup_mode in MUTATING_CLEANUP_MODES and owner != OWNED_OWNER:
        raise PolicyError(f"{section_id}.owner/cleanup_mode combination is not cleanup-owned")


def _validate_mode_fields(section_id: str, section: dict[str, Any], cleanup_mode: str) -> None:
    if cleanup_mode == "rotate":
        _read_positive_int(section_id, section.get("max_bytes"), "max_bytes")
        _read_positive_int(section_id, section.get("retained_rotations"), "retained_rotations")
    elif cleanup_mode == "ttl_prune":
        _read_positive_int(section_id, section.get("ttl_days"), "ttl_days")
    elif cleanup_mode == "toolchain_retention":
        if "retain_exact_names" not in section:
            raise PolicyError(f"{section_id}.retain_exact_names must be a list of non-empty strings")
        if "remove_exact_names" not in section:
            raise PolicyError(f"{section_id}.remove_exact_names must be a list of non-empty strings")
        _read_string_list(section_id, section, "retain_exact_names")
        _read_string_list(section_id, section, "remove_exact_names")


def _surface_from_section(section_id: str, section: dict[str, Any]) -> PolicySurface:
    _require_keys(section_id, section, REQUIRED_SURFACE_KEYS)
    owner = _read_string(section_id, section, "owner")
    cleanup_mode = _read_string(section_id, section, "cleanup_mode")
    _validate_owner_cleanup_mode(section_id, owner, cleanup_mode)
    if cleanup_mode in {"rotate", "ttl_prune"}:
        if "active_writer_processes" not in section:
            raise PolicyError(f"{section_id}.active_writer_processes is required")
        if not _read_string_list(section_id, section, "active_writer_processes"):
            raise PolicyError(f"{section_id}.active_writer_processes must not be empty")
    _validate_mode_fields(section_id, section, cleanup_mode)
    known = set(REQUIRED_SURFACE_KEYS) | {
        "active_writer_processes",
        "id",
        "max_bytes",
        "retained_rotations",
        "ttl_days",
        "persistence",
        "retain_exact_names",
        "remove_exact_names",
    }
    extra = {key: value for key, value in section.items() if key not in known}
    for key in (
        "max_bytes",
        "retained_rotations",
        "ttl_days",
        "persistence",
        "retain_exact_names",
        "remove_exact_names",
    ):
        if key in section:
            extra[key] = section[key]
    return PolicySurface(
        id=str(section.get("id", section_id)),
        path_family=_read_string(section_id, section, "path_family"),
        category=_read_string(section_id, section, "category"),
        growth_shape=_read_string(section_id, section, "growth_shape"),
        owner=owner,
        native_policy=_read_string(section_id, section, "native_policy"),
        cleanup_mode=cleanup_mode,
        active_writer_processes=_read_string_list(section_id, section, "active_writer_processes"),
        extra=extra,
    )


def _configured_path(home_root: Path, path_family: str) -> Path:
    if not path_family.startswith("~/"):
        raise PolicyError(f"path_family must start with ~/: {path_family}")
    return home_root / path_family[2:]


def _path_family_root_and_pattern(home_root: Path, path_family: str) -> tuple[Path, str]:
    if not path_family.startswith("~/"):
        raise PolicyError(f"path_family must start with ~/: {path_family}")
    parts = Path(path_family[2:]).parts
    root_parts: list[str] = []
    pattern_parts: list[str] = []
    pattern_started = False
    for part in parts:
        if any(marker in part for marker in ("*", "?", "[")):
            pattern_started = True
        if pattern_started:
            pattern_parts.append(part)
        else:
            root_parts.append(part)
    if not pattern_parts:
        raise PolicyError(f"path_family must include a glob pattern: {path_family}")
    return home_root.joinpath(*root_parts), "/".join(pattern_parts)


def _path_family_has_glob(path_family: str) -> bool:
    return any(marker in path_family for marker in ("*", "?", "["))


def _paths_for_surface(home_root: Path, path_family: str) -> list[Path]:
    if _path_family_has_glob(path_family):
        base, pattern = _path_family_root_and_pattern(home_root, path_family)
        if not _inside_root(base, home_root) or not base.exists():
            return []
        if pattern == "**":
            return [base]
        return sorted(base.glob(pattern))
    candidate = _configured_path(home_root, path_family)
    if not _inside_root(candidate, home_root) or not candidate.exists():
        return []
    return [candidate]


def _inside_root(candidate: Path, root: Path) -> bool:
    try:
        candidate.resolve(strict=False).relative_to(root.resolve(strict=False))
    except ValueError:
        return False
    return True


def _scan_refusal(surface_id: str, path: Path) -> dict[str, Any]:
    return {
        "surface_id": surface_id,
        "path": str(path),
        "action": "refuse",
        "reason": "path_disappeared_during_scan",
        "estimated_reclaim_bytes": 0,
    }


def _root_refusal(
    surface_id: str,
    path: Path,
    home_root: Path,
    *,
    include_action: bool,
) -> dict[str, Any] | None:
    try:
        stat = path.lstat()
    except FileNotFoundError:
        if _inside_root(path, home_root):
            return None
        entry: dict[str, Any] = {
            "surface_id": surface_id,
            "path": str(path),
            "reason": "outside_configured_root",
            "estimated_reclaim_bytes": 0,
        }
        if include_action:
            entry["action"] = "refuse"
        return entry
    except OSError:
        entry = _scan_refusal(surface_id, path)
        if not include_action:
            entry.pop("action")
        return entry
    reason = "symlink_not_followed" if stat_module.S_ISLNK(stat.st_mode) else ""
    if not reason and not _inside_root(path, home_root):
        reason = "outside_configured_root"
    if not reason:
        return None
    entry = {
        "surface_id": surface_id,
        "path": str(path),
        "reason": reason,
        "bytes": stat.st_size,
        "estimated_reclaim_bytes": 0,
    }
    if include_action:
        entry["action"] = "refuse"
    return entry


def _path_family_refusal(
    surface_id: str,
    path: Path,
    home_root: Path,
    *,
    include_action: bool,
) -> dict[str, Any] | None:
    try:
        relative = path.relative_to(home_root)
    except ValueError:
        return _root_refusal(surface_id, path, home_root, include_action=include_action)
    current = home_root
    for part in relative.parts:
        current = current / part
        refusal = _root_refusal(surface_id, current, home_root, include_action=include_action)
        if refusal is not None:
            return refusal
    return None


def _stale_rotation_sidecar_candidates(
    surface_id: str,
    path: Path,
    retained_rotations: int,
    sidecar_entries: list[tuple[int, Path]] | None = None,
) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    sidecars = sidecar_entries if sidecar_entries is not None else _rotation_sidecar_entries(path)
    for index, sidecar_path in sidecars:
        if index <= retained_rotations:
            continue
        try:
            stat = sidecar_path.lstat()
        except FileNotFoundError:
            continue
        if stat_module.S_ISLNK(stat.st_mode):
            return [
                {
                    "surface_id": surface_id,
                    "path": str(sidecar_path),
                    "action": "refuse",
                    "reason": "symlink_not_followed",
                    "bytes": stat.st_size,
                    "estimated_reclaim_bytes": 0,
                }
            ]
        candidates.append(
            {
                "surface_id": surface_id,
                "path": str(sidecar_path),
                "action": "delete",
                "reason": "rotation_retention_exceeded",
                "bytes": stat.st_size,
                "retained_rotations": retained_rotations,
                "estimated_reclaim_bytes": stat.st_size,
                "state_token": _paths_state_token([sidecar_path]),
            }
        )
    return candidates


def _candidates_for_rotating_log(surface: PolicySurface, home_root: Path) -> list[dict[str, Any]]:
    max_bytes = _read_positive_int(surface.id, surface.extra.get("max_bytes"), "max_bytes")
    retained_rotations = _read_positive_int(
        surface.id,
        surface.extra.get("retained_rotations"),
        "retained_rotations",
    )
    candidate_path = _configured_path(home_root, surface.path_family)
    path_refusal = _path_family_refusal(
        surface.id,
        candidate_path,
        home_root,
        include_action=True,
    )
    if path_refusal is not None:
        return [path_refusal]
    if not _inside_root(candidate_path, home_root):
        return [
            {
                "surface_id": surface.id,
                "path": str(candidate_path),
                "action": "refuse",
                "reason": "outside_configured_root",
                "estimated_reclaim_bytes": 0,
            }
        ]
    try:
        sidecar_entries = _rotation_sidecar_entries(candidate_path)
    except OSError:
        return [_scan_refusal(surface.id, candidate_path.parent)]
    symlink_refusal = _rotation_symlink_refusal(
        surface.id,
        candidate_path,
        retained_rotations,
        sidecar_entries,
    )
    if symlink_refusal is not None:
        return [symlink_refusal]
    try:
        stat = candidate_path.lstat()
    except FileNotFoundError:
        return _stale_rotation_sidecar_candidates(
            surface.id,
            candidate_path,
            retained_rotations,
            sidecar_entries,
        )
    if not candidate_path.is_file():
        return _stale_rotation_sidecar_candidates(
            surface.id,
            candidate_path,
            retained_rotations,
            sidecar_entries,
        )
    if stat.st_size <= max_bytes:
        return _stale_rotation_sidecar_candidates(
            surface.id,
            candidate_path,
            retained_rotations,
            sidecar_entries,
        )
    return [
        {
            "surface_id": surface.id,
            "path": str(candidate_path),
            "action": "rotate",
            "reason": "size_exceeds_max_bytes",
            "bytes": stat.st_size,
            "max_bytes": max_bytes,
            "retained_rotations": retained_rotations,
            "estimated_reclaim_bytes": _rotation_reclaim_bytes(
                candidate_path,
                retained_rotations,
                sidecar_entries,
            ),
            "state_token": _paths_state_token(
                _rotation_paths(candidate_path, retained_rotations, sidecar_entries)
            ),
        }
    ]


def _candidates_for_sessions(surface: PolicySurface, home_root: Path) -> list[dict[str, Any]]:
    ttl_days = _read_positive_int(surface.id, surface.extra.get("ttl_days"), "ttl_days")
    base, pattern = _path_family_root_and_pattern(home_root, surface.path_family)
    root_refusal = _root_refusal(surface.id, base, home_root, include_action=True)
    if root_refusal is not None:
        return [root_refusal]
    if not base.exists():
        return []
    cutoff = time.time() - (ttl_days * 24 * 60 * 60)
    candidates: list[dict[str, Any]] = []
    for candidate_path in sorted(base.glob(pattern)):
        try:
            stat = candidate_path.lstat()
        except OSError:
            candidates.append(_scan_refusal(surface.id, candidate_path))
            continue
        if stat_module.S_ISLNK(stat.st_mode):
            candidates.append(
                {
                    "surface_id": surface.id,
                    "path": str(candidate_path),
                    "action": "refuse",
                    "reason": "symlink_not_followed",
                    "bytes": stat.st_size,
                    "estimated_reclaim_bytes": 0,
                }
            )
            continue
        if not _inside_root(candidate_path, base):
            candidates.append(
                {
                    "surface_id": surface.id,
                    "path": str(candidate_path),
                    "action": "refuse",
                    "reason": "outside_configured_root",
                    "bytes": stat.st_size,
                    "estimated_reclaim_bytes": 0,
                }
            )
            continue
        if not stat_module.S_ISREG(stat.st_mode) or stat.st_mtime > cutoff:
            continue
        try:
            state_token = _state_token(candidate_path)
        except OSError:
            candidates.append(_scan_refusal(surface.id, candidate_path))
            continue
        candidates.append(
            {
                "surface_id": surface.id,
                "path": str(candidate_path),
                "action": "delete",
                "reason": "older_than_ttl_days",
                "bytes": stat.st_size,
                "ttl_days": ttl_days,
                "estimated_reclaim_bytes": stat.st_size,
                "state_token": state_token,
            }
        )
    return candidates


def _measured_bytes(path: Path) -> int:
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    if not path.is_dir():
        return path.lstat().st_size
    total = path.lstat().st_size
    for child in path.rglob("*"):
        total += child.lstat().st_size
    return total


def _measured_bytes_or_error(path: Path) -> tuple[int, dict[str, Any] | None]:
    try:
        return _measured_bytes(path), None
    except OSError as exc:
        return 0, {
            "path": str(path),
            "reason": "measurement_failed",
            "error": type(exc).__name__,
        }


def _paths_measurement(paths: list[Path]) -> tuple[int, list[dict[str, Any]]]:
    total = 0
    errors: list[dict[str, Any]] = []
    for path in paths:
        measured_bytes, error = _measured_bytes_or_error(path)
        total += measured_bytes
        if error is not None:
            errors.append(error)
    return total, errors


def _state_token(path: Path) -> str:
    digest = hashlib.sha256()

    def add_stat(label: str, stat_result: os.stat_result) -> None:
        digest.update(label.encode("utf-8", errors="surrogateescape"))
        digest.update(b"\0")
        digest.update(
            f"{stat_result.st_dev}:{stat_result.st_ino}:{stat_result.st_mode}:"
            f"{stat_result.st_size}:{stat_result.st_mtime_ns}".encode("ascii")
        )
        digest.update(b"\0")

    root_stat = path.lstat()
    add_stat(".", root_stat)
    if stat_module.S_ISDIR(root_stat.st_mode) and not stat_module.S_ISLNK(root_stat.st_mode):
        for child in sorted(path.rglob("*")):
            add_stat(str(child.relative_to(path)), child.lstat())
    return digest.hexdigest()


def _paths_state_token(paths: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in paths:
        digest.update(str(path).encode("utf-8", errors="surrogateescape"))
        digest.update(b"\0")
        try:
            stat_result = path.lstat()
        except FileNotFoundError:
            digest.update(b"missing")
        else:
            digest.update(
                f"{stat_result.st_dev}:{stat_result.st_ino}:{stat_result.st_mode}:"
                f"{stat_result.st_size}:{stat_result.st_mtime_ns}".encode("ascii")
            )
        digest.update(b"\0")
    return digest.hexdigest()


def _rotation_slot_path(path: Path, index: int) -> Path:
    return path.with_name(f"{path.name}.{index}")


def _rotation_sidecar_entries(path: Path) -> list[tuple[int, Path]]:
    prefix = f"{path.name}."
    try:
        candidates = path.parent.iterdir()
    except FileNotFoundError:
        return []
    entries: list[tuple[int, Path]] = []
    for candidate in candidates:
        if not candidate.name.startswith(prefix):
            continue
        suffix = candidate.name[len(prefix):]
        if not suffix.isdecimal():
            continue
        index = int(suffix)
        if index <= 0 or suffix != str(index):
            continue
        entries.append((index, candidate))
    return sorted(entries, key=lambda entry: entry[0])


def _rotation_paths(
    path: Path,
    retained_rotations: int,
    sidecar_entries: list[tuple[int, Path]] | None = None,
) -> list[Path]:
    configured_paths = [path] + [
        _rotation_slot_path(path, index) for index in range(1, retained_rotations + 1)
    ]
    configured = set(configured_paths)
    sidecars = sidecar_entries if sidecar_entries is not None else _rotation_sidecar_entries(path)
    extra_paths = [
        sidecar_path
        for index, sidecar_path in sidecars
        if index > retained_rotations and sidecar_path not in configured
    ]
    return configured_paths + extra_paths


def _rotation_symlink_refusal(
    surface_id: str,
    path: Path,
    retained_rotations: int,
    sidecar_entries: list[tuple[int, Path]] | None = None,
) -> dict[str, Any] | None:
    try:
        rotation_paths = _rotation_paths(path, retained_rotations, sidecar_entries)
    except OSError:
        return _scan_refusal(surface_id, path.parent)
    for candidate_path in rotation_paths:
        try:
            stat = candidate_path.lstat()
        except FileNotFoundError:
            continue
        if stat_module.S_ISLNK(stat.st_mode):
            return {
                "surface_id": surface_id,
                "path": str(candidate_path),
                "action": "refuse",
                "reason": "symlink_not_followed",
                "bytes": stat.st_size,
                "estimated_reclaim_bytes": 0,
            }
    return None


def _validate_rotation_paths(path: Path, retained_rotations: int) -> None:
    try:
        rotation_paths = _rotation_paths(path, retained_rotations)
    except OSError as exc:
        raise PolicyError(f"unable to inspect rotation sidecars: {path.parent}") from exc
    for candidate_path in rotation_paths:
        try:
            stat = candidate_path.lstat()
        except FileNotFoundError:
            continue
        if stat_module.S_ISLNK(stat.st_mode):
            raise PolicyError(f"refusing to rotate symlink: {candidate_path}")


def _lstat_if_present(path: Path) -> os.stat_result | None:
    try:
        return path.lstat()
    except FileNotFoundError:
        return None


def _path_exists_no_follow(path: Path) -> bool:
    return _lstat_if_present(path) is not None


def _create_empty_file_no_follow(path: Path, mode: int) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, mode)
    try:
        os.fchmod(fd, mode)
    finally:
        os.close(fd)


def _rotation_reclaim_bytes(
    path: Path,
    retained_rotations: int,
    sidecar_entries: list[tuple[int, Path]] | None = None,
) -> int:
    total = 0
    sidecars = sidecar_entries if sidecar_entries is not None else _rotation_sidecar_entries(path)
    for index, sidecar_path in sidecars:
        if index < retained_rotations:
            continue
        try:
            total += sidecar_path.lstat().st_size
        except FileNotFoundError:
            continue
    return total


def _rotation_measurement_paths(path: Path) -> list[Path]:
    paths: list[Path] = []
    if _path_exists_no_follow(path):
        paths.append(path)
    try:
        sidecars = _rotation_sidecar_entries(path)
    except OSError:
        sidecars = []
    paths.extend(sidecar_path for _index, sidecar_path in sidecars)
    return paths


def _project_pinned_channels(repo_root: Path, *, required: bool = False) -> tuple[str, ...]:
    toolchain_toml = repo_root / "rust-toolchain.toml"
    if not toolchain_toml.exists():
        if required:
            raise PolicyError(f"repository rust-toolchain.toml is required: {toolchain_toml}")
        return ()
    try:
        with toolchain_toml.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise PolicyError(f"invalid repository rust-toolchain.toml: {toolchain_toml}") from exc
    toolchain = data.get("toolchain")
    if not isinstance(toolchain, dict):
        if required:
            raise PolicyError(f"repository rust-toolchain.toml missing [toolchain]: {toolchain_toml}")
        return ()
    channel = toolchain.get("channel")
    if not isinstance(channel, str) or not channel:
        if required:
            raise PolicyError(f"repository rust-toolchain.toml missing toolchain.channel: {toolchain_toml}")
        return ()
    return (channel,)


def _is_project_pinned_toolchain(name: str, channels: tuple[str, ...]) -> bool:
    return any(name == channel or name.startswith(f"{channel}-") for channel in channels)


def _rustup_entries(
    surface: PolicySurface,
    home_root: Path,
    repo_root: Path,
    active_toolchains: tuple[str, ...],
    default_toolchains: tuple[str, ...],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    base, pattern = _path_family_root_and_pattern(home_root, surface.path_family)
    if pattern != "*":
        raise PolicyError(f"{surface.id}.path_family must end with one exact-name wildcard")
    root_refusal = _root_refusal(surface.id, base, home_root, include_action=True)
    if root_refusal is not None:
        return [root_refusal], []
    if not base.exists():
        return [], []

    retain_exact_names = set(_string_tuple_from_extra(surface, "retain_exact_names"))
    remove_exact_names = set(_string_tuple_from_extra(surface, "remove_exact_names"))
    if remove_exact_names and (not active_toolchains or not default_toolchains):
        raise PolicyError(
            f"{surface.id} active/default rustup snapshots are required when "
            "remove_exact_names is non-empty"
        )
    active_names = set(active_toolchains)
    default_names = set(default_toolchains)
    pinned_channels = _project_pinned_channels(repo_root, required=bool(remove_exact_names))
    candidates: list[dict[str, Any]] = []
    protected: list[dict[str, Any]] = []

    try:
        paths = sorted(base.iterdir())
    except OSError:
        return [_scan_refusal(surface.id, base)], []

    for path in paths:
        name = path.name
        try:
            stat = path.lstat()
        except OSError:
            candidates.append(_scan_refusal(surface.id, path))
            continue
        if stat_module.S_ISLNK(stat.st_mode):
            protected.append(
                {
                    "surface_id": surface.id,
                    "path": str(path),
                    "reason": "symlink_not_followed",
                    "bytes": stat.st_size,
                }
            )
            continue
        if not stat_module.S_ISDIR(stat.st_mode):
            continue

        reason = ""
        if name in active_names:
            reason = "active_toolchain"
        elif name in default_names:
            reason = "default_toolchain"
        elif _is_project_pinned_toolchain(name, pinned_channels):
            reason = "project_pinned_toolchain"
        elif name in retain_exact_names:
            reason = "exact_name_retain_policy"
        elif name not in remove_exact_names:
            reason = "not_in_remove_exact_names"

        if reason:
            measured_bytes, error = _measured_bytes_or_error(path)
            entry: dict[str, Any] = {
                "surface_id": surface.id,
                "path": str(path),
                "reason": reason,
                "bytes": measured_bytes,
            }
            if error is not None:
                entry["measurement_error"] = error
            protected.append(
                entry
            )
            continue

        try:
            measured_bytes = _measured_bytes(path)
            state_token = _state_token(path)
        except OSError:
            candidates.append(_scan_refusal(surface.id, path))
            continue
        candidates.append(
            {
                "surface_id": surface.id,
                "path": str(path),
                "action": "remove_tree",
                "reason": "exact_name_remove_policy",
                "bytes": measured_bytes,
                "estimated_reclaim_bytes": measured_bytes,
                "state_token": state_token,
            }
        )

    return candidates, protected


def _report_only_entries(surface: PolicySurface, home_root: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    if _path_family_has_glob(surface.path_family):
        base, _pattern = _path_family_root_and_pattern(home_root, surface.path_family)
        root_refusal = _root_refusal(surface.id, base, home_root, include_action=False)
        if root_refusal is not None:
            return [root_refusal]
    else:
        path_refusal = _path_family_refusal(
            surface.id,
            _configured_path(home_root, surface.path_family),
            home_root,
            include_action=False,
        )
        if path_refusal is not None:
            return [path_refusal]
    for path in _paths_for_surface(home_root, surface.path_family):
        try:
            is_symlink = path.is_symlink()
            measured_bytes = _measured_bytes(path)
        except OSError:
            entries.append(
                {
                    "surface_id": surface.id,
                    "path": str(path),
                    "reason": "path_disappeared_during_scan",
                    "bytes": 0,
                    "estimated_reclaim_bytes": 0,
                }
            )
            continue
        reason = "symlink_not_followed" if is_symlink else "report_only_policy"
        entry = {
            "surface_id": surface.id,
            "path": str(path),
            "reason": reason,
            "bytes": measured_bytes,
            "estimated_reclaim_bytes": 0,
        }
        if surface.id.startswith("native_guidance."):
            entry["reason"] = "native_guidance_report_only"
            entry["native_config"] = {
                key: surface.extra[key]
                for key in ("max_bytes", "persistence")
                if key in surface.extra
            }
        entries.append(entry)
    return entries


def _surface_measurement(surface: PolicySurface, home_root: Path) -> dict[str, Any]:
    if surface.cleanup_mode == "rotate" and surface.id in ROTATING_SURFACE_IDS:
        path = _configured_path(home_root, surface.path_family)
        paths = _rotation_measurement_paths(path) if _inside_root(path, home_root) else []
    else:
        paths = _paths_for_surface(home_root, surface.path_family)
    measured_bytes, errors = _paths_measurement(paths)
    entry = {
        "surface_id": surface.id,
        "path_family": surface.path_family,
        "owner": surface.owner,
        "cleanup_mode": surface.cleanup_mode,
        "cleanup_eligible": surface.owner == OWNED_OWNER and surface.cleanup_mode != "none",
        "bytes": measured_bytes,
        "path_count": len(paths),
    }
    if errors:
        entry["measurement_errors"] = errors
    return entry


def _owned_root_refusal_errors(
    policy: Policy,
    home_root: Path,
    candidates: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    owned_refusal_paths: dict[str, set[Path]] = {}
    for surface in policy.surfaces:
        if surface.owner != OWNED_OWNER:
            continue
        if _path_family_has_glob(surface.path_family):
            paths = {_path_family_root_and_pattern(home_root, surface.path_family)[0]}
        else:
            configured_path = _configured_path(home_root, surface.path_family)
            paths = {configured_path}
            if surface.cleanup_mode == "rotate" and surface.id in ROTATING_SURFACE_IDS:
                paths.add(configured_path.parent)
        owned_refusal_paths[surface.id] = paths
    errors: list[dict[str, Any]] = []
    for candidate in candidates:
        surface_paths = owned_refusal_paths.get(str(candidate.get("surface_id", "")), set())
        path = candidate.get("path")
        if (
            not surface_paths
            or candidate.get("action") != "refuse"
            or not isinstance(path, str)
            or Path(path) not in surface_paths
        ):
            continue
        errors.append(
            {
                "surface_id": candidate["surface_id"],
                "path": path,
                "reason": candidate.get("reason", "measurement_failed"),
                "error": "root_refusal",
            }
        )
    return errors


def _adjacent_context(policy: Policy, home_root: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for surface in policy.adjacent_surfaces:
        paths = _paths_for_surface(home_root, surface.path_family)
        if not paths:
            continue
        measured_bytes, errors = _paths_measurement(paths)
        entry = {
            "surface_id": surface.id,
            "path_family": surface.path_family,
            "owner": surface.owner,
            "cleanup_mode": surface.cleanup_mode,
            "bytes": measured_bytes,
            "path_count": len(paths),
        }
        if errors:
            entry["measurement_errors"] = errors
        entries.append(entry)
    return entries


def load_policy(policy_path: Path) -> Policy:
    try:
        raw_policy = policy_path.read_bytes()
        data = tomllib.loads(raw_policy.decode("utf-8"))
    except OSError as exc:
        raise PolicyError(f"cannot read policy: {policy_path}") from exc
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        raise PolicyError(f"invalid TOML policy: {exc}") from exc
    policy_digest = hashlib.sha256(raw_policy).hexdigest()

    schema_version = data.get("schema_version")
    if type(schema_version) is not int:
        raise PolicyError("schema_version must be an integer")
    if schema_version != SUPPORTED_POLICY_SCHEMA_VERSION:
        raise PolicyError(f"unsupported schema_version: {schema_version}")

    surfaces = tuple(
        _surface_from_section(section_id, _section(data, path))
        for section_id, path in SURFACE_SECTIONS
    )
    preflight = _section(data, PREFLIGHT_SECTION)
    _require_keys("preflight", preflight, REQUIRED_PREFLIGHT_KEYS)
    preflight_values: dict[str, int] = {}
    for key in REQUIRED_PREFLIGHT_KEYS:
        value = preflight[key]
        if type(value) is not int or value < 0:
            raise PolicyError(f"preflight.{key} must be a non-negative integer")
        preflight_values[key] = value
    if preflight_values["free_disk_error_bytes"] > preflight_values["free_disk_warning_bytes"]:
        raise PolicyError(
            "preflight.free_disk_error_bytes must be less than or equal to "
            "preflight.free_disk_warning_bytes"
        )
    if preflight_values["owned_storage_error_bytes"] < preflight_values["owned_storage_warning_bytes"]:
        raise PolicyError(
            "preflight.owned_storage_error_bytes must be greater than or equal to "
            "preflight.owned_storage_warning_bytes"
        )

    adjacent_raw = data.get("adjacent", {})
    if not isinstance(adjacent_raw, dict):
        raise PolicyError("adjacent must be a table")
    adjacent = tuple(
        _surface_from_section(str(section.get("id", f"adjacent.{name}")), section)
        for name, section in sorted(adjacent_raw.items())
        if isinstance(section, dict)
    )
    if len(adjacent) != len(adjacent_raw):
        raise PolicyError("adjacent entries must be tables")
    for surface in adjacent:
        if surface.owner == OWNED_OWNER:
            raise PolicyError(f"{surface.id}.owner/cleanup_mode combination is not valid for adjacent context")

    return Policy(
        path=policy_path,
        digest=policy_digest,
        schema_version=schema_version,
        surfaces=surfaces,
        adjacent_surfaces=adjacent,
        preflight=preflight_values,
    )


def build_status(policy: Policy, home_root: Path, repo_root: Path) -> dict[str, Any]:
    return {
        "status": "ok",
        "policy_path": str(policy.path),
        "policy_digest": policy.digest,
        "schema_version": policy.schema_version,
        "home_root": str(home_root),
        "evaluated_root": str(home_root),
        "repo_root": str(repo_root),
        "surfaces": [surface.to_status_entry() for surface in policy.surfaces],
        "adjacent_surfaces": [surface.to_status_entry() for surface in policy.adjacent_surfaces],
        "preflight": policy.preflight,
    }


def build_dry_run(
    policy: Policy,
    home_root: Path,
    repo_root: Path,
    *,
    active_rustup_toolchains: tuple[str, ...] = (),
    default_rustup_toolchains: tuple[str, ...] = (),
) -> dict[str, Any]:
    payload = build_status(policy, home_root, repo_root)
    payload["mode"] = "dry_run"
    candidates: list[dict[str, Any]] = []
    report_only: list[dict[str, Any]] = []
    protected: list[dict[str, Any]] = []
    for surface in policy.surfaces:
        if (
            surface.owner == "owned"
            and surface.cleanup_mode == "rotate"
            and surface.id in ROTATING_SURFACE_IDS
        ):
            candidates.extend(_candidates_for_rotating_log(surface, home_root))
        elif (
            surface.owner == "owned"
            and surface.cleanup_mode == "ttl_prune"
            and surface.id == SESSION_SURFACE_ID
        ):
            candidates.extend(_candidates_for_sessions(surface, home_root))
        elif (
            surface.owner == "owned"
            and surface.cleanup_mode == "toolchain_retention"
            and surface.id == RUSTUP_SURFACE_ID
        ):
            rustup_candidates, rustup_protected = _rustup_entries(
                surface,
                home_root,
                repo_root,
                active_rustup_toolchains,
                default_rustup_toolchains,
            )
            candidates.extend(rustup_candidates)
            protected.extend(rustup_protected)
        if surface.owner == REPORT_ONLY_OWNER:
            report_only.extend(_report_only_entries(surface, home_root))
    payload["candidates"] = candidates
    payload["report_only"] = report_only
    payload["protected"] = protected
    payload["surface_measurements"] = [
        _surface_measurement(surface, home_root) for surface in policy.surfaces
    ]
    payload["adjacent_context"] = _adjacent_context(policy, home_root)
    return payload


def build_preflight(
    policy: Policy,
    home_root: Path,
    repo_root: Path,
    *,
    available_disk_bytes: int | None = None,
    active_rustup_toolchains: tuple[str, ...] = (),
    default_rustup_toolchains: tuple[str, ...] = (),
) -> dict[str, Any]:
    payload = build_dry_run(
        policy,
        home_root,
        repo_root,
        active_rustup_toolchains=active_rustup_toolchains,
        default_rustup_toolchains=default_rustup_toolchains,
    )
    payload["mode"] = "preflight"
    payload["read_only"] = True
    free_bytes = (
        available_disk_bytes
        if available_disk_bytes is not None
        else shutil.disk_usage(home_root).free
    )
    owned_bytes = sum(
        entry["bytes"]
        for entry in payload["surface_measurements"]
        if entry["owner"] == OWNED_OWNER
    )
    owned_measurement_errors = [
        error
        for entry in payload["surface_measurements"]
        if entry["owner"] == OWNED_OWNER
        for error in entry.get("measurement_errors", [])
    ]
    owned_measurement_errors.extend(
        _owned_root_refusal_errors(policy, home_root, payload["candidates"])
    )
    warnings: list[str] = []
    errors: list[str] = []
    thresholds = policy.preflight
    if free_bytes < thresholds["free_disk_error_bytes"]:
        errors.append("free_disk_below_error")
    elif free_bytes < thresholds["free_disk_warning_bytes"]:
        warnings.append("free_disk_below_warning")
    if owned_bytes > thresholds["owned_storage_error_bytes"]:
        errors.append("owned_storage_above_error")
    elif owned_bytes > thresholds["owned_storage_warning_bytes"]:
        warnings.append("owned_storage_above_warning")
    if owned_measurement_errors:
        errors.append("owned_storage_measurement_failed")
    payload["available_disk_bytes"] = free_bytes
    payload["owned_storage_bytes"] = owned_bytes
    payload["owned_storage_measurement_errors"] = owned_measurement_errors
    payload["follow_up_classes"] = sorted(
        entry["surface_id"] for entry in payload["adjacent_context"] if entry["bytes"] > 0
    )
    payload["warnings"] = warnings
    payload["errors"] = errors
    payload["status"] = "error" if errors else "warning" if warnings else "ok"
    return payload


def _load_dry_run_report(path: Path) -> dict[str, Any]:
    try:
        with path.open("r", encoding="utf-8") as handle:
            payload = json.load(handle)
    except (OSError, json.JSONDecodeError) as exc:
        raise PolicyError(f"invalid dry-run report: {path}") from exc
    if payload.get("mode") != "dry_run":
        raise PolicyError("dry-run report mode must be dry_run")
    return payload


def _candidate_signature(candidate: dict[str, Any]) -> tuple[Any, ...]:
    return (
        candidate.get("surface_id"),
        candidate.get("path"),
        candidate.get("action"),
        candidate.get("reason"),
        candidate.get("bytes"),
        candidate.get("estimated_reclaim_bytes"),
        candidate.get("state_token"),
    )


def _mutating_candidates(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        candidate
        for candidate in payload.get("candidates", [])
        if candidate.get("action") in MUTATING_ACTIONS
    ]


def _refusal_candidates(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        candidate
        for candidate in payload.get("candidates", [])
        if candidate.get("action") == "refuse"
    ]


def _rotate_log(path: Path, retained_rotations: int) -> None:
    if retained_rotations <= 0:
        raise PolicyError("retained_rotations must be positive for rotation")
    _validate_rotation_paths(path, retained_rotations)
    original_stat = path.lstat()
    if stat_module.S_ISLNK(original_stat.st_mode):
        raise PolicyError(f"refusing to rotate symlink: {path}")
    original_mode = original_stat.st_mode & 0o7777
    rotation_sources = [(0, path)] + _rotation_sidecar_entries(path)
    staged: list[tuple[int, Path, Path, Path]] = []
    created_current = False
    created_current_stat: tuple[int, int, int, int] | None = None

    try:
        for index, source in rotation_sources:
            source_stat = _lstat_if_present(source)
            if source_stat is None:
                continue
            if stat_module.S_ISLNK(source_stat.st_mode):
                raise PolicyError(f"refusing to rotate symlink: {source}")
            temp = source.with_name(f".{source.name}.rotate-{os.getpid()}-{time.time_ns()}-{index}.tmp")
            source.rename(temp)
            staged.append((index, source, temp, temp))

        for index, source, temp, location in list(reversed(staged)):
            if index < retained_rotations:
                destination = _rotation_slot_path(path, index + 1)
                location.rename(destination)
                staged[staged.index((index, source, temp, location))] = (
                    index,
                    source,
                    temp,
                    destination,
                )

        _create_empty_file_no_follow(path, original_mode)
        created_current = True
        current_stat = path.lstat()
        created_current_stat = (
            current_stat.st_dev,
            current_stat.st_ino,
            current_stat.st_size,
            current_stat.st_mtime_ns,
        )

        for index, _source, _temp, location in staged:
            if index >= retained_rotations and _path_exists_no_follow(location):
                location.unlink()
    except (OSError, PolicyError):
        for _index, _source, temp, location in list(staged):
            if location != temp and _path_exists_no_follow(location):
                location.rename(temp)
        if created_current and _path_exists_no_follow(path):
            current_stat = path.lstat()
            current_token = (
                current_stat.st_dev,
                current_stat.st_ino,
                current_stat.st_size,
                current_stat.st_mtime_ns,
            )
            if current_token == created_current_stat:
                path.unlink()
        for _index, source, temp, _location in staged:
            if not _path_exists_no_follow(source) and _path_exists_no_follow(temp):
                temp.rename(source)
        raise


def _active_writer_refusals(
    policy: Policy,
    candidates: list[dict[str, Any]],
    process_names: tuple[str, ...],
) -> list[dict[str, Any]]:
    observed = set(process_names)
    if not observed:
        return []
    surfaces = {surface.id: surface for surface in policy.surfaces}
    refusals: list[dict[str, Any]] = []
    for candidate in candidates:
        surface = surfaces[candidate["surface_id"]]
        matched = sorted(observed.intersection(surface.active_writer_processes))
        if matched:
            refusals.append(
                {
                    "surface_id": surface.id,
                    "path": candidate["path"],
                    "reason": "active_writer_detected",
                    "process_names": matched,
                }
            )
    return refusals


def _process_snapshot_required(policy: Policy, candidates: list[dict[str, Any]]) -> bool:
    surfaces = {surface.id: surface for surface in policy.surfaces}
    return any(surfaces[candidate["surface_id"]].active_writer_processes for candidate in candidates)


def _fresh_candidate_match(
    policy: Policy,
    home_root: Path,
    repo_root: Path,
    candidate: dict[str, Any],
    *,
    active_rustup_toolchains: tuple[str, ...],
    default_rustup_toolchains: tuple[str, ...],
) -> tuple[bool, dict[str, Any]]:
    fresh = build_dry_run(
        policy,
        home_root,
        repo_root,
        active_rustup_toolchains=active_rustup_toolchains,
        default_rustup_toolchains=default_rustup_toolchains,
    )
    signature = _candidate_signature(candidate)
    fresh_signatures = {_candidate_signature(fresh_candidate) for fresh_candidate in _mutating_candidates(fresh)}
    return signature in fresh_signatures, fresh


def build_apply(
    policy: Policy,
    home_root: Path,
    repo_root: Path,
    *,
    dry_run_report: Path,
    active_rustup_toolchains: tuple[str, ...] = (),
    default_rustup_toolchains: tuple[str, ...] = (),
    process_names: tuple[str, ...] = (),
    process_snapshot_supplied: bool = False,
) -> dict[str, Any]:
    previous = _load_dry_run_report(dry_run_report)
    current = build_dry_run(
        policy,
        home_root,
        repo_root,
        active_rustup_toolchains=active_rustup_toolchains,
        default_rustup_toolchains=default_rustup_toolchains,
    )
    if previous.get("policy_digest") != current.get("policy_digest"):
        return {
            "mode": "apply",
            "status": "aborted",
            "reason": "policy_changed_after_dry_run",
            "actions_taken": [],
            "refusal_reasons": _refusal_candidates(current),
            "skipped_report_only": current.get("report_only", []),
            "skipped_protected": current.get("protected", []),
            "bytes_reclaimed": 0,
        }
    previous_signatures = [_candidate_signature(candidate) for candidate in _mutating_candidates(previous)]
    current_candidates = _mutating_candidates(current)
    current_signatures = [_candidate_signature(candidate) for candidate in current_candidates]
    if previous_signatures != current_signatures:
        return {
            "mode": "apply",
            "status": "aborted",
            "reason": "candidate_state_changed",
            "actions_taken": [],
            "refusal_reasons": _refusal_candidates(current),
            "skipped_report_only": current.get("report_only", []),
            "skipped_protected": current.get("protected", []),
            "bytes_reclaimed": 0,
        }

    if _process_snapshot_required(policy, current_candidates) and not process_snapshot_supplied:
        return {
            "mode": "apply",
            "status": "refused",
            "reason": "process_snapshot_required",
            "actions_taken": [],
            "refusal_reasons": _refusal_candidates(current),
            "skipped_report_only": current.get("report_only", []),
            "skipped_protected": current.get("protected", []),
            "bytes_reclaimed": 0,
        }

    active_writer_refusals = _active_writer_refusals(policy, current_candidates, process_names)
    if active_writer_refusals:
        return {
            "mode": "apply",
            "status": "refused",
            "reason": "active_writer_detected",
            "actions_taken": [],
            "active_writer_refusals": active_writer_refusals,
            "refusal_reasons": _refusal_candidates(current),
            "skipped_report_only": current.get("report_only", []),
            "skipped_protected": current.get("protected", []),
            "bytes_reclaimed": 0,
        }

    surfaces = {surface.id: surface for surface in policy.surfaces}
    actions_taken: list[dict[str, Any]] = []
    bytes_reclaimed = 0
    for candidate in current_candidates:
        action = candidate["action"]
        path = Path(candidate["path"])
        candidate_matches, fresh = _fresh_candidate_match(
            policy,
            home_root,
            repo_root,
            candidate,
            active_rustup_toolchains=active_rustup_toolchains,
            default_rustup_toolchains=default_rustup_toolchains,
        )
        if not candidate_matches:
            return {
                "mode": "apply",
                "status": "aborted",
                "reason": "candidate_state_changed",
                "actions_taken": actions_taken,
                "refusal_reasons": _refusal_candidates(fresh),
                "skipped_report_only": fresh.get("report_only", []),
                "skipped_protected": fresh.get("protected", []),
                "bytes_reclaimed": bytes_reclaimed,
            }
        try:
            if action == "rotate":
                surface = surfaces[candidate["surface_id"]]
                retained_rotations = _read_positive_int(
                    surface.id,
                    surface.extra.get("retained_rotations"),
                    "retained_rotations",
                )
                _rotate_log(path, retained_rotations)
            elif action == "delete":
                if path.is_symlink():
                    raise PolicyError(f"refusing to delete symlink: {path}")
                path.unlink()
            elif action == "remove_tree":
                if path.is_symlink():
                    raise PolicyError(f"refusing to remove symlink: {path}")
                shutil.rmtree(path)
            else:
                raise PolicyError(f"unsupported apply action: {action}")
        except (OSError, PolicyError) as exc:
            failed_action = dict(candidate)
            failed_action["error"] = str(exc)
            return {
                "mode": "apply",
                "status": "failed",
                "reason": "mutation_failed",
                "actions_taken": actions_taken,
                "failed_action": failed_action,
                "refusal_reasons": _refusal_candidates(current),
                "skipped_report_only": current.get("report_only", []),
                "skipped_protected": current.get("protected", []),
                "bytes_reclaimed": bytes_reclaimed,
            }
        actions_taken.append(candidate)
        bytes_reclaimed += int(candidate.get("estimated_reclaim_bytes", 0))

    return {
        "mode": "apply",
        "status": "applied",
        "actions_taken": actions_taken,
        "refusal_reasons": _refusal_candidates(current),
        "skipped_report_only": current.get("report_only", []),
        "skipped_protected": current.get("protected", []),
        "bytes_reclaimed": bytes_reclaimed,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("status", "dry-run", "preflight", "apply"))
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--home-root", required=True, type=Path)
    parser.add_argument("--repo-root", required=True, type=Path)
    parser.add_argument("--json", action="store_true", required=True)
    parser.add_argument("--active-rustup-toolchain", action="append", default=[])
    parser.add_argument("--default-rustup-toolchain", action="append", default=[])
    parser.add_argument("--available-disk-bytes", type=int)
    parser.add_argument("--dry-run-report", type=Path)
    parser.add_argument("--process-name", action="append", default=None)
    parser.add_argument("--process-snapshot-empty", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        policy = load_policy(args.policy)
        if args.command == "status":
            payload = build_status(policy, args.home_root, args.repo_root)
        elif args.command == "dry-run":
                payload = build_dry_run(
                    policy,
                    args.home_root,
                    args.repo_root,
                    active_rustup_toolchains=tuple(args.active_rustup_toolchain),
                    default_rustup_toolchains=tuple(args.default_rustup_toolchain),
                )
        else:
            if args.command == "preflight":
                payload = build_preflight(
                    policy,
                    args.home_root,
                    args.repo_root,
                    available_disk_bytes=args.available_disk_bytes,
                    active_rustup_toolchains=tuple(args.active_rustup_toolchain),
                    default_rustup_toolchains=tuple(args.default_rustup_toolchain),
                )
            else:
                if args.dry_run_report is None:
                    raise PolicyError("apply requires --dry-run-report")
                payload = build_apply(
                    policy,
                    args.home_root,
                    args.repo_root,
                    dry_run_report=args.dry_run_report,
                    active_rustup_toolchains=tuple(args.active_rustup_toolchain),
                    default_rustup_toolchains=tuple(args.default_rustup_toolchain),
                    process_names=tuple(args.process_name or []),
                    process_snapshot_supplied=bool(args.process_snapshot_empty or args.process_name is not None),
                )
    except PolicyError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    print(json.dumps(payload, sort_keys=True))
    if payload.get("mode") == "preflight" and payload.get("status") == "error":
        return 1
    if payload.get("mode") == "apply" and payload.get("status") != "applied":
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
