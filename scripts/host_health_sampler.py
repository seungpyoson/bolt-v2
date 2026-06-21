#!/usr/bin/env python3
"""Standalone host and service health sampling for visibility-only telemetry.

This script emits one JSONL record containing host, systemd service, process,
memory, and disk observations. It is intentionally standalone and stdlib-only
so operators can run it directly on a Linux systemd EC2 host, while local macOS
development runs still emit a valid degraded record instead of going silent.

Every collector returns ``(value, error_or_none)`` and collection failures are
reported in the row's ``errors`` array. Collector errors never prevent the
sampler from writing a JSON record.

Prime directive (scope): ONCE SAMPLING HAS BEGUN the sampler must never crash,
hang, exit non-zero, or emit non-JSON on the degraded host states it observes;
the only permitted non-zero exit is when a built record reaches NO sink at all
(see :class:`RecordUnemittable`). This guarantee starts after argv/config
validation: argparse and the ``--interval`` type-checker
(:func:`nonnegative_finite_float`) fail FAST with exit code 2 on a bad invocation
*before* any sampling, which is intentional and outside the never-non-zero
guarantee.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import json
import math
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import threading
import time
from typing import Any, Callable, Optional, Tuple


SCHEMA_VERSION = 2
DEFAULT_SERVICE = "bolt-v2"
# Single source of truth for the bot's catalog/data dir is
# ``config/root.toml`` key ``catalog_directory`` (under ``[persistence]``).
# ``discover_catalog_directory()`` reads it from the repo at runtime so this
# script never drifts from the bot config. ``DISK_PATH_FALLBACK`` is the
# standalone-box fallback used ONLY when that file cannot be found/parsed (e.g. a
# bare EC2 host that has the binary but not the repo checkout). The literal here
# must mirror config/root.toml:catalog_directory.
DISK_PATH_FALLBACK = "/srv/bolt-v2/var/bolt-v3-live/catalog"
# Matches ``catalog_directory = "<value>"`` (single OR double quoted), tolerant of
# surrounding whitespace. Anchored on ``catalog_directory`` as a whole word so the
# neighbouring ``required_catalog_prefix`` key cannot match. Deliberately a
# line-scan regex, not ``tomllib`` (added in 3.11) — must parse on Python 3.8+.
_CATALOG_DIRECTORY_RE = re.compile(
    r'^\s*catalog_directory\s*=\s*["\'](?P<value>[^"\']+)["\']\s*$'
)


def extract_catalog_directory(toml_text: str) -> str | None:
    """Return the ``catalog_directory`` value from a root.toml body, or None.

    Pure line-scan so it works on Python 3.8+ (no ``tomllib``). Returns the first
    matching key's value; ``required_catalog_prefix`` and other keys are ignored.
    """
    for line in toml_text.splitlines():
        match = _CATALOG_DIRECTORY_RE.match(line)
        if match:
            return match.group("value")
    return None


def discover_catalog_directory(start: Path | None = None) -> str:
    """Resolve the bot catalog dir from ``config/root.toml``, walking upward.

    Searches ``config/root.toml`` from each origin directory and its ancestors.
    When ``start`` is given it is the sole origin (used by tests); otherwise the
    origins are this script's directory and the current working directory. The
    first file whose ``catalog_directory`` parses wins. Any failure (file absent,
    unreadable, no key) silently yields :data:`DISK_PATH_FALLBACK` so a standalone
    box without the repo still runs.
    """
    try:
        if start is not None:
            origins = [Path(start).resolve()]
        else:
            origins = [Path(__file__).resolve().parent, Path.cwd().resolve()]
        roots: list[Path] = []
        seen: set[Path] = set()
        for origin in origins:
            for directory in (origin, *origin.parents):
                if directory not in seen:
                    seen.add(directory)
                    roots.append(directory)
        for directory in roots:
            candidate = directory / "config" / "root.toml"
            try:
                text = candidate.read_text(encoding="utf-8")
            except OSError:
                continue
            value = extract_catalog_directory(text)
            if value:
                return value
    except Exception:  # noqa: BLE001 - discovery must never break sampling
        return DISK_PATH_FALLBACK
    return DISK_PATH_FALLBACK


# Discovered once at import: the live value when the repo is present, else the
# standalone fallback. Overridable per-run via ``--disk-path``.
DEFAULT_DISK_PATH = discover_catalog_directory()
SYSTEMCTL_TIMEOUT_SECONDS = 5
# Outer per-collector wall-clock backstop. Every collector runs through
# ``run_collector`` -> ``run_with_deadline`` in a daemon thread; if it does not
# finish within this many seconds the daemon thread is ABANDONED (never joined)
# and the collector degrades to a null+timeout error. This bounds the otherwise
# unbounded blocking syscalls a degraded host can wedge on: a D-state subprocess
# that survives SIGKILL (EBS/disk I/O stall), or os.stat/os.statvfs/realpath/
# Path.exists against a hung filesystem. It is strictly GREATER than
# SYSTEMCTL_TIMEOUT_SECONDS so a collector's own inner subprocess timeout fires
# first with its clean message; this outer deadline is the last-resort backstop
# for the truly-wedged case where even SIGKILL cannot reap the child.
COLLECTOR_TIMEOUT_SECONDS = 10
# Bounded non-blocking flock acquisition: a contended file lock must never wedge
# the sampler. We retry LOCK_NB every FLOCK_RETRY_SECONDS until FLOCK_TIMEOUT_SECONDS
# elapses, then treat the file sink as failed and fall back to stdout.
FLOCK_TIMEOUT_SECONDS = 5.0
FLOCK_RETRY_SECONDS = 0.05
# Outer wall-clock backstop for the FILE SINK, mirroring COLLECTOR_TIMEOUT_SECONDS
# on the write side. write_to_file is bounded by run_with_deadline so an EBS/disk
# stall DURING os.open/os.write (which the bounded LOCK_NB flock cannot catch —
# the flock is acquired only after the open returns) can never hang the writer.
# Strictly GREATER than FLOCK_TIMEOUT_SECONDS so the existing bounded LOCK_NB
# acquire fires first with its clean TimeoutError; this is the last-resort guard
# for a stall in the syscalls the flock loop does not cover.
SINK_TIMEOUT_SECONDS = 8.0
KNOWN_NON_OOM_RESULTS = {
    "success",
    "exit-code",
    "timeout",
    "watchdog",
    "start-limit-hit",
    "resources",
    "protocol",
    "assert",
}
# A SIGKILL delivered by the kernel OOM killer is indistinguishable from any
# other signal at the systemd ``Result`` level on systemd <243 / non-oom-kill
# configs: both surface as Result="signal" (or "core-dump"). These results are
# therefore ambiguous — they can only be confirmed or denied with corroborating
# evidence (the cgroup ``memory.events`` oom_kill counter), never asserted False.
OOM_AMBIGUOUS_RESULTS = {"signal", "core-dump"}


# This alias is a RUNTIME value (its RHS is evaluated at import), so it must use
# typing.Tuple/Optional rather than the PEP 604 ``str | None`` / builtin-generic
# ``tuple[...]`` forms, which raise TypeError at import on Python < 3.10. Function
# *signature* annotations stay lazy via ``from __future__ import annotations``;
# only this top-level assignment is eager. Minimum supported runtime: Python 3.8.
CollectorResult = Tuple[Any, Optional[str]]


def service_template(unit: str) -> dict[str, Any]:
    return {
        "unit": unit,
        "load_state": None,
        "active_state": None,
        "sub_state": None,
        "result": None,
        # NRestarts is auto-restarts this invocation, not lifetime. It resets on
        # reset-failed/manual-start/reboot and counts only systemd auto-restarts.
        "n_restarts": None,
        "main_pid": None,
        "exec_main_pid": None,
        "exec_main_code": None,
        "exec_main_status": None,
        "exec_main_start": None,
        "invocation_id": None,
        "cgroup_oom_kills": None,
    }


def derive_oom_killed(result: str | None, cgroup_oom_kills: int | None) -> bool | None:
    """Derive OOM from systemd's Result, corroborated by the cgroup oom counter.

    A kernel OOM kill is delivered as SIGKILL and may surface as systemd
    ``Result="signal"`` (or ``"core-dump"``) rather than ``"oom-kill"`` on systemd
    <243 or non-oom-kill configs, so an ambiguous signal result must NEVER be
    reported as a confident ``False``. Semantics:

    - ``result == "oom-kill"`` -> ``True`` (systemd authoritative).
    - ``result`` in :data:`OOM_AMBIGUOUS_RESULTS` -> ``True`` if
      ``cgroup_oom_kills`` is not None and > 0, else ``None`` (cannot confirm or
      deny without the cgroup counter).
    - ``result`` in :data:`KNOWN_NON_OOM_RESULTS` -> ``False`` (authoritatively
      not an OOM). The cgroup counter deliberately does NOT override a clean
      systemd result: a child process being OOM-killed must not false-positive a
      clean main-process exit.
    - otherwise (unknown/missing result) -> ``True`` if ``cgroup_oom_kills`` is
      not None and > 0, else ``None``.
    """
    if result == "oom-kill":
        return True
    if result in KNOWN_NON_OOM_RESULTS:
        return False
    corroborated = cgroup_oom_kills is not None and cgroup_oom_kills > 0
    if result in OOM_AMBIGUOUS_RESULTS:
        return True if corroborated else None
    return True if corroborated else None


def parse_int(value: str | None, *, zero_is_null: bool = False) -> int | None:
    if value is None or value == "":
        return None
    try:
        parsed = int(value)
    except ValueError:
        return None
    if zero_is_null and parsed == 0:
        return None
    return parsed


def parse_timestamp(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip()
    if not stripped or stripped.lower() == "n/a":
        return None
    return stripped


def collect_host() -> CollectorResult:
    try:
        return socket.gethostname(), None
    except Exception as exc:  # pragma: no cover - defensive around libc/socket
        return None, str(exc)


def collect_service(unit: str) -> CollectorResult:
    properties = (
        "LoadState",
        "ActiveState",
        "SubState",
        "Result",
        "NRestarts",
        "MainPID",
        "ExecMainPID",
        "ExecMainCode",
        "ExecMainStatus",
        "ExecMainStartTimestamp",
        "InvocationID",
    )
    command = [
        "systemctl",
        "show",
        unit,
        *(f"--property={property_name}" for property_name in properties),
    ]
    # subprocess.run(timeout=...) would, on TimeoutExpired, internally kill the
    # child and then issue an UNBOUNDED wait() to reap it. A systemctl wedged in
    # uninterruptible D-state (realistic during an EBS/disk I/O stall) survives
    # SIGKILL until the kernel unblocks it, so that wait() can hang forever. We
    # use Popen + a single bounded communicate(); on timeout we kill and ABANDON
    # the child WITHOUT a second blocking wait. On a one-shot run the process
    # exits and the OS reaps the orphan; the outer run_with_deadline backstop
    # bounds even the rare case where this call itself does not return promptly.
    try:
        proc = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return None, "systemctl unavailable"
    except Exception as exc:  # pragma: no cover - subprocess spawn edge cases
        return None, f"systemctl show failed: {exc}"

    try:
        stdout_text, stderr_text = proc.communicate(timeout=SYSTEMCTL_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        # Kill and abandon: no second blocking communicate()/wait() that could
        # hang on a SIGKILL-immune D-state child. A single non-blocking poll()
        # lets the OS reap it if it has already died, but never blocks.
        try:
            proc.kill()
        except Exception:  # pragma: no cover - child may already be gone
            pass
        try:
            proc.poll()
        except Exception:  # pragma: no cover - poll is non-blocking, defensive
            pass
        return None, f"systemctl show timed out after {SYSTEMCTL_TIMEOUT_SECONDS}s"
    except Exception as exc:  # pragma: no cover - subprocess edge cases
        return None, f"systemctl show failed: {exc}"

    returncode = proc.returncode
    if returncode != 0:
        stderr_stripped = stderr_text.strip() if stderr_text else ""
        reason = stderr_stripped.splitlines()[0] if stderr_stripped else "no stderr"
        return None, f"systemctl show exited {returncode}: {reason}"

    parsed: dict[str, str] = {}
    for raw_line in stdout_text.splitlines():
        if "=" not in raw_line:
            continue
        key, value = raw_line.split("=", 1)
        parsed[key] = value

    service = service_template(unit)
    service["load_state"] = parsed.get("LoadState") or None
    if service["load_state"] == "not-found":
        return service, f"unit {unit!r} not found"

    service["active_state"] = parsed.get("ActiveState") or None
    service["sub_state"] = parsed.get("SubState") or None
    service["result"] = parsed.get("Result") or None
    service["n_restarts"] = parse_int(parsed.get("NRestarts"))
    service["main_pid"] = parse_int(parsed.get("MainPID"), zero_is_null=True)
    service["exec_main_pid"] = parse_int(parsed.get("ExecMainPID"), zero_is_null=True)
    service["exec_main_code"] = parse_int(parsed.get("ExecMainCode"))
    service["exec_main_status"] = parse_int(parsed.get("ExecMainStatus"))
    service["exec_main_start"] = parse_timestamp(parsed.get("ExecMainStartTimestamp"))
    service["invocation_id"] = parsed.get("InvocationID") or None
    return service, None


def cgroup_unit_candidates(unit: str) -> list[str]:
    candidates = [unit]
    if "." not in unit:
        candidates.append(f"{unit}.service")
    return candidates


def cgroup_text_matches_unit(cgroup_text: str, unit: str) -> bool:
    """True iff a cgroup path segment exactly equals a unit candidate.

    Handles cgroup v2 lines ("0::/system.slice/bolt-v2.service") and v1
    lines ("4:memory:/system.slice/bolt-v2.service"). Exact segment match
    avoids substring false-positives (bolt-v2-replica.service must NOT match
    unit bolt-v2).
    """
    candidates = set(cgroup_unit_candidates(unit))
    for line in cgroup_text.splitlines():
        # max 3 parts so a path containing ':' is not over-split; the path is last
        parts = line.split(":", 2)
        path = parts[-1]
        segments = path.strip("/").split("/")
        if candidates.intersection(segments):
            return True
    return False


def parse_memory_events_oom_kills(text: str) -> int | None:
    """Return the ``oom_kill`` counter from a cgroup ``memory.events`` body.

    The file is whitespace-separated ``key value`` lines. Returns None if no
    ``oom_kill`` line is present. Raises ``ValueError`` if the counter is not an
    integer, matching the caller's error handling.
    """
    for line in text.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[0] == "oom_kill":
            return int(fields[1])
    return None


# The two cgroup versions expose the per-unit oom_kill counter in DIFFERENT
# files. cgroup-v2 (the real deploy, unified hierarchy) puts it in
# ``memory.events`` as an ``oom_kill <N>`` line. cgroup-v1 does NOT expose
# oom_kill in ``memory.events`` at all; it lives in ``memory.oom_control`` under
# the dedicated ``memory`` controller mount, also as an ``oom_kill <N>`` line.
# ``parse_memory_events_oom_kills`` parses that same ``oom_kill`` line from either
# file. The v1 path is a best-effort portability fallback only.
CGROUP_V2_SYSTEM_SLICE = Path("/sys/fs/cgroup/system.slice")
CGROUP_V1_MEMORY_SYSTEM_SLICE = Path("/sys/fs/cgroup/memory/system.slice")
CGROUP_V2_OOM_KILL_FILE = "memory.events"
CGROUP_V1_OOM_KILL_FILE = "memory.oom_control"


def cgroup_oom_kill_paths(unit: str) -> list[Path]:
    """Ordered candidate oom-kill-counter paths: cgroup-v2 first, then v1.

    The two versions use different files: cgroup-v2 reads ``memory.events`` and
    cgroup-v1 reads ``memory.oom_control`` (cgroup-v1 does NOT carry the oom_kill
    counter in ``memory.events``). Both contain an ``oom_kill <N>`` line that
    ``parse_memory_events_oom_kills`` parses identically.

    Scope: only units living DIRECTLY under ``system.slice`` are resolved. A unit
    placed in a custom ``Slice=`` or a nested scope lives at a different cgroup
    path, so these candidates miss it and ``collect_cgroup_oom_kills`` returns
    FileNotFound -> null. This is fail-safe by design: a missing path yields a
    null oom counter, never a false-positive count. Full custom-slice resolution
    (reading the unit's actual ControlGroup) is tracked as follow-up.
    """
    candidates = cgroup_unit_candidates(unit)
    v2 = [CGROUP_V2_SYSTEM_SLICE / candidate / CGROUP_V2_OOM_KILL_FILE for candidate in candidates]
    v1 = [CGROUP_V1_MEMORY_SYSTEM_SLICE / candidate / CGROUP_V1_OOM_KILL_FILE for candidate in candidates]
    return v2 + v1


def collect_cgroup_oom_kills(unit: str) -> CollectorResult:
    missing: list[str] = []
    for path in cgroup_oom_kill_paths(unit):
        try:
            text = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            missing.append(str(path))
            continue
        except PermissionError as exc:
            return None, str(exc)
        except OSError as exc:
            return None, str(exc)
        except (UnicodeDecodeError, ValueError) as exc:
            # A malformed/non-UTF-8 counter file raises UnicodeDecodeError, which
            # is a ValueError (NOT an OSError) and would otherwise escape this try
            # to be caught only by the outer run_collector guard. Catch it here so
            # it degrades to a clean null+error and the sample still emits.
            return None, f"{path} decode error: {exc}"
        try:
            oom_kills = parse_memory_events_oom_kills(text)
        except ValueError as exc:
            return None, f"{path} parse error: {exc}"
        if oom_kills is not None:
            return oom_kills, None
        missing.append(f"{path} (no oom_kill line)")
    return None, "oom_kill counter unavailable at " + ", ".join(missing)


def parse_kb_line(value: str) -> int | None:
    parts = value.split()
    if not parts:
        return None
    try:
        return int(parts[0]) * 1024
    except ValueError:
        return None


def collect_memory() -> CollectorResult:
    if sys.platform != "linux":
        return None, f"/proc/meminfo unavailable on {sys.platform}"

    path = Path("/proc/meminfo")
    try:
        raw_lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except FileNotFoundError:
        return None, "/proc/meminfo missing"
    except PermissionError as exc:
        return None, str(exc)
    except OSError as exc:
        return None, str(exc)

    values: dict[str, int] = {}
    for line in raw_lines:
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        parsed = parse_kb_line(value)
        if parsed is not None:
            values[key] = parsed

    mem_available_estimated = "MemAvailable" not in values
    mem_available = values.get("MemAvailable")
    if mem_available is None:
        mem_available = values.get("MemFree")

    memory = {
        "mem_total_bytes": values.get("MemTotal"),
        "mem_available_bytes": mem_available,
        "mem_available_estimated": mem_available_estimated,
        "swap_total_bytes": values.get("SwapTotal"),
        "swap_free_bytes": values.get("SwapFree"),
    }
    return memory, None


def resolve_measured_disk_path(requested_path: str) -> tuple[Path | None, str | None]:
    candidate = Path(requested_path).expanduser()
    if not candidate.is_absolute():
        candidate = Path.cwd() / candidate

    # FIX E: the requested path itself is the only valid measurement target. A
    # missing path is a hard degraded signal (the catalog dir vanished), so we do
    # NOT walk to a parent and measure that — reporting an ancestor's fullness for
    # a path that no longer exists reads as a misleadingly-healthy disk. Return
    # null + an accurate error; collect_disk emits disk: null. (This previously
    # measured the nearest existing ancestor, but that measurement is no longer
    # used, so the old "measured nearest existing ancestor" wording was false.)
    if not candidate.exists():
        return None, f"requested disk path {requested_path!r} missing; disk metrics unavailable"
    return candidate, None


def disk_metrics_from_statvfs(path: str, statvfs_result: os.statvfs_result, st_dev: int) -> dict[str, Any]:
    fragment_size = statvfs_result.f_frsize
    total_bytes = statvfs_result.f_blocks * fragment_size
    # free_bytes deliberately uses f_bavail (space available to unprivileged
    # users == df "Available"), NOT f_bfree, which includes the root-reserved
    # blocks an operator's process cannot actually use. This matches the
    # df-convention denominator (used + available) used for used_pct below, so
    # the reported free space and the used% stay internally consistent.
    free_bytes = statvfs_result.f_bavail * fragment_size
    used_bytes = (statvfs_result.f_blocks - statvfs_result.f_bfree) * fragment_size
    available_bytes = statvfs_result.f_bavail * fragment_size
    denominator = used_bytes + available_bytes
    # Metric-sanity guard (FIX E5): a corrupted statvfs must never yield a
    # used_pct the viewer would render as misleadingly healthy. f_bfree > f_blocks
    # is non-physical (more free blocks than exist) and produces a negative
    # used_bytes -> a negative used_pct; null it out. Otherwise clamp the computed
    # value into the inclusive [0, 100] range so neither a negative nor a >100
    # used_pct can ever reach the viewer (which reads low used_pct as green).
    if statvfs_result.f_bfree > statvfs_result.f_blocks:
        used_pct = None
    elif denominator > 0:
        used_pct = round(used_bytes / denominator * 100, 2)
        used_pct = max(0.0, min(100.0, used_pct))
    else:
        used_pct = None
    return {
        "path": os.path.realpath(path),
        "device": st_dev,
        "total_bytes": total_bytes,
        "free_bytes": free_bytes,
        "used_pct": used_pct,
        "inodes_total": statvfs_result.f_files,
        "inodes_free": statvfs_result.f_favail,
    }


def collect_disk(requested_path: str) -> CollectorResult:
    measured_path, path_error = resolve_measured_disk_path(requested_path)
    if measured_path is None:
        # FIX E: the requested catalog path is missing. Emit disk: null plus the
        # error rather than measuring a parent directory, so a vanished catalog
        # dir reads as a hard signal, not as a misleadingly-healthy ancestor disk.
        return None, path_error
    try:
        stat_result = os.stat(measured_path)
        statvfs_result = os.statvfs(measured_path)
    except FileNotFoundError:
        return None, f"{measured_path} disappeared before statvfs"
    except PermissionError as exc:
        return None, str(exc)
    except OSError as exc:
        return None, str(exc)

    disk = disk_metrics_from_statvfs(str(measured_path), statvfs_result, stat_result.st_dev)
    return disk, None


def process_status_template(pid: int, identity_ok: bool) -> dict[str, Any]:
    return {
        "pid": pid,
        "identity_ok": identity_ok,
        "rss_bytes": None,
        "vsz_bytes": None,
        "swap_bytes": None,
        "threads": None,
        "fd_count": None,
        "fd_limit_soft": None,
    }


def read_process_cgroup(pid: int) -> str:
    return Path(f"/proc/{pid}/cgroup").read_text(encoding="utf-8", errors="replace")


def process_identity_ok(pid: int, unit: str) -> tuple[bool, str | None]:
    try:
        cgroup_text = read_process_cgroup(pid)
    except FileNotFoundError:
        return False, f"pid {pid} vanished"
    except PermissionError as exc:
        return False, str(exc)
    except OSError as exc:
        return False, str(exc)

    if cgroup_text_matches_unit(cgroup_text, unit):
        return True, None
    return False, f"pid recycled: /proc/{pid}/cgroup did not contain unit {unit!r}"


def parse_proc_status(pid: int) -> tuple[dict[str, int], str | None]:
    path = Path(f"/proc/{pid}/status")
    try:
        raw_lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except FileNotFoundError:
        return {}, f"pid {pid} vanished before status read"
    except PermissionError as exc:
        return {}, str(exc)
    except OSError as exc:
        return {}, str(exc)

    parsed: dict[str, int] = {}
    for line in raw_lines:
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        if key in {"VmRSS", "VmSize", "VmSwap"}:
            kb_value = parse_kb_line(value)
            if kb_value is not None:
                parsed[key] = kb_value
        elif key == "Threads":
            try:
                parsed[key] = int(value.strip())
            except ValueError:
                pass
    return parsed, None


def read_fd_count(pid: int) -> tuple[int | None, str | None]:
    try:
        with os.scandir(f"/proc/{pid}/fd") as it:
            return sum(1 for _ in it), None
    except FileNotFoundError:
        return None, f"pid {pid} vanished before fd read"
    except PermissionError as exc:
        return None, str(exc)
    except OSError as exc:
        return None, str(exc)


def read_fd_limit_soft(pid: int) -> tuple[int | None, str | None]:
    path = Path(f"/proc/{pid}/limits")
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except FileNotFoundError:
        return None, f"pid {pid} vanished before limits read"
    except PermissionError as exc:
        return None, str(exc)
    except OSError as exc:
        return None, str(exc)

    for line in lines:
        fields = line.split()
        if len(fields) >= 5 and fields[0:3] == ["Max", "open", "files"]:
            if fields[3] == "unlimited":
                return None, None
            try:
                return int(fields[3]), None
            except ValueError as exc:
                return None, f"Max open files parse error: {exc}"
    return None, "Max open files limit not found"


def collect_process(unit: str, main_pid: int | None, exec_main_pid: int | None) -> CollectorResult:
    if sys.platform != "linux":
        return None, f"/proc unavailable on {sys.platform}"

    pid = main_pid if main_pid and main_pid > 0 else exec_main_pid
    if pid is None or pid <= 0:
        return None, None

    identity_ok, identity_error = process_identity_ok(pid, unit)
    if not identity_ok:
        return None, identity_error or "pid identity mismatch"

    process = process_status_template(pid, True)
    errors: list[str] = []

    status, status_error = parse_proc_status(pid)
    if status_error:
        errors.append(status_error)
    process["rss_bytes"] = status.get("VmRSS")
    process["vsz_bytes"] = status.get("VmSize")
    process["swap_bytes"] = status.get("VmSwap")
    process["threads"] = status.get("Threads")

    process["fd_count"], fd_error = read_fd_count(pid)
    if fd_error:
        errors.append(fd_error)

    process["fd_limit_soft"], limit_error = read_fd_limit_soft(pid)
    if limit_error:
        errors.append(limit_error)

    # Re-check identity AFTER reading the metrics (TOCTOU guard): the pid may have
    # exited and been recycled to an unrelated process between the first
    # process_identity_ok() and these reads, in which case the metrics above
    # belong to the wrong process. If identity no longer holds, discard them.
    recheck_ok, recheck_error = process_identity_ok(pid, unit)
    if not recheck_ok:
        return None, recheck_error or "pid recycled during read"

    return process, "; ".join(errors) if errors else None


def run_with_deadline(function: Callable[..., Any], timeout_seconds: float, *args: Any) -> Any:
    """Run ``function(*args)`` with a hard wall-clock deadline.

    The function runs in a ``daemon`` thread which is joined for at most
    ``timeout_seconds``. If it finishes in time, its return value is returned and
    any exception it raised is re-raised unchanged in the caller. If it does NOT
    finish, ``TimeoutError`` is raised and the daemon thread is ABANDONED — never
    joined again — so a syscall wedged in uninterruptible D-state (e.g. a hung
    filesystem or a SIGKILL-immune child) can never block the sampler's exit. The
    abandoned thread dies with the interpreter.

    This is the single backstop that bounds every otherwise-unbounded blocking
    syscall a collector can issue (subprocess wait, os.stat/os.statvfs/realpath,
    Path.exists). It deliberately does not attempt to cancel the work — Python has
    no safe thread-cancel — it only stops *waiting* for it.
    """
    box: dict[str, Any] = {}

    def worker() -> None:
        try:
            box["value"] = function(*args)
        except BaseException as exc:  # noqa: BLE001 - propagated to the caller below
            box["error"] = exc

    thread = threading.Thread(target=worker, daemon=True)
    thread.start()
    thread.join(timeout_seconds)
    if thread.is_alive():
        raise TimeoutError(f"timed out after {timeout_seconds}s")
    if "error" in box:
        raise box["error"]
    return box.get("value")


def run_collector(source: str, function: Callable[..., CollectorResult], *args: Any) -> CollectorResult:
    # Single chokepoint for EVERY collector: bound it with run_with_deadline so a
    # wedged blocking syscall (D-state subprocess, hung-filesystem stat/statvfs)
    # degrades to a timeout error instead of hanging the whole sampler. The outer
    # deadline is strictly greater than the in-collector subprocess timeout so the
    # collector's own clean message normally wins; this is the last-resort guard.
    try:
        value, error = run_with_deadline(function, COLLECTOR_TIMEOUT_SECONDS, *args)
    except TimeoutError:
        return None, f"{source}: timed out after {COLLECTOR_TIMEOUT_SECONDS}s"
    except Exception as exc:  # noqa: BLE001 - final guard so sample always emits
        return None, f"{source}: {exc}"
    if error:
        return value, f"{source}: {error}"
    return value, None


def append_error(errors: list[str], error: str | None) -> None:
    if error:
        errors.append(error)


def sample(service_unit: str = DEFAULT_SERVICE, disk_path: str = DEFAULT_DISK_PATH) -> dict[str, Any]:
    errors: list[str] = []

    host, error = run_collector("host", collect_host)
    append_error(errors, error)

    service, error = run_collector("service", collect_service, service_unit)
    append_error(errors, error)
    if service is not None:
        cgroup_oom_kills, cgroup_error = run_collector(
            "cgroup_oom_kills", collect_cgroup_oom_kills, service_unit
        )
        append_error(errors, cgroup_error)
        service["cgroup_oom_kills"] = cgroup_oom_kills

    main_pid = service.get("main_pid") if isinstance(service, dict) else None
    exec_main_pid = service.get("exec_main_pid") if isinstance(service, dict) else None
    process, error = run_collector("process", collect_process, service_unit, main_pid, exec_main_pid)
    append_error(errors, error)

    memory, error = run_collector("memory", collect_memory)
    append_error(errors, error)

    disk, error = run_collector("disk", collect_disk, disk_path)
    append_error(errors, error)

    service_result = service.get("result") if isinstance(service, dict) else None
    service_oom_kills = service.get("cgroup_oom_kills") if isinstance(service, dict) else None
    return {
        "schema_version": SCHEMA_VERSION,
        "sampled_at": datetime.now(timezone.utc).isoformat(),
        "host": host,
        "platform": sys.platform,
        "service": service,
        "process": process,
        "memory": memory,
        "disk": disk,
        "oom_killed": derive_oom_killed(service_result, service_oom_kills),
        "errors": errors,
    }


class RecordUnemittable(Exception):
    """Raised only when a built record cannot reach ANY sink (file AND stdout).

    This is the sole condition under which the sampler is permitted a non-zero
    exit: the degraded state it exists to observe could not be surfaced anywhere.
    """


def emit_to_stdout(line: str) -> None:
    """Write one record line to stdout, flushing so a piped consumer sees it.

    Raises whatever the underlying write/flush raises; the caller classifies it.
    On the PRIMARY streaming path ``BrokenPipeError`` is a clean stream
    termination (a gone consumer) and propagates (B4); a closed/broken stdout
    raises ``OSError`` or ``ValueError`` and becomes ``RecordUnemittable``. On the
    FALLBACK path (after the file sink already failed) ANY failure here — broken
    pipe included — means the record is lost, so it becomes ``RecordUnemittable``.
    """
    sys.stdout.write(line)
    sys.stdout.flush()


def acquire_flock_bounded(fd: int, timeout: float, retry: float) -> bool:
    """Acquire an exclusive lock without blocking forever.

    Returns True once LOCK_NB succeeds; returns False if ``timeout`` seconds
    elapse while the lock stays contended. A contended lock must never wedge the
    sampler, so we poll LOCK_NB instead of issuing a blocking LOCK_EX.
    """
    deadline = time.monotonic() + timeout
    while True:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            if time.monotonic() >= deadline:
                return False
            time.sleep(retry)


def write_to_file(record_line: str, out_path: str, lock_timeout: float) -> None:
    """Append one record line to ``out_path`` under a bounded exclusive lock.

    Raises ``OSError`` (or ``TimeoutError`` on lock contention) on any file-sink
    failure so the caller can fall back to stdout. Never blocks indefinitely.

    Process guarantee: on any write failure the original ``OSError`` propagates
    so the caller falls back to stdout and the record reaches a sink.

    Data guarantee: after a partial write, the file is rolled back
    (``ftruncate``) to the last clean record boundary so a fragment is normally
    not left behind. If the rollback itself fails, the fragment may survive on
    disk; the viewer tolerates a corrupt trailing line.
    """
    path = Path(out_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    # O_NONBLOCK on the sink open: if ``out_path`` resolves to a FIFO with no
    # reader, a blocking os.open(O_WRONLY) hangs forever; O_NONBLOCK makes it
    # raise ENXIO (an OSError) immediately, which write_jsonl_line's existing
    # ``except OSError`` turns into a stdout fallback. For a REGULAR file
    # O_NONBLOCK is a no-op: the open returns immediately and regular-file writes
    # never return EAGAIN, so the normal path and the write-all loop below are
    # unchanged.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NONBLOCK, 0o644)
    try:
        if not acquire_flock_bounded(fd, lock_timeout, FLOCK_RETRY_SECONDS):
            raise TimeoutError(
                f"could not acquire file lock on {out_path!r} within {lock_timeout}s"
            )
        # Write-all loop: a single os.write may write only a PREFIX (notably on
        # ENOSPC mid-write to a regular file) and return the short count without
        # raising. Ignoring it would truncate the JSONL line, and the next
        # O_APPEND would glue onto the fragment with no exception fired (no stdout
        # fallback). Loop until every byte lands; a 0-byte return means no
        # progress is possible, so raise OSError to trigger the stdout fallback.
        # On any failure roll the file back to clean_boundary so no fragment
        # survives. We hold the exclusive flock throughout, so no other writer
        # can append between the size snapshot and the rollback.
        payload = record_line.encode("utf-8")
        clean_boundary = os.fstat(fd).st_size
        offset = 0
        try:
            while offset < len(payload):
                written = os.write(fd, payload[offset:])
                if written <= 0:
                    raise OSError(
                        f"os.write made no progress writing to {out_path!r} "
                        f"({offset}/{len(payload)} bytes written)"
                    )
                offset += written
        except BaseException:
            try:
                os.ftruncate(fd, clean_boundary)
            except OSError:
                pass
            raise
    finally:
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_UN)
            except OSError:
                pass
        finally:
            try:
                os.close(fd)
            except OSError:
                pass


def sanitize_non_finite(obj: Any) -> Any:
    """Recursively replace non-finite floats (NaN/Infinity) with ``None``.

    Walks dicts and lists/tuples so a non-finite value nested anywhere in the
    record is neutralised before serialisation. This guarantees ``json.dumps(...,
    allow_nan=False)`` can never raise on a stray non-finite float and that the
    emitted line is always strict JSON. Non-float values pass through unchanged.
    """
    if isinstance(obj, float):
        return obj if math.isfinite(obj) else None
    if isinstance(obj, dict):
        return {key: sanitize_non_finite(value) for key, value in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [sanitize_non_finite(value) for value in obj]
    return obj


def write_jsonl_line(
    record: dict[str, Any],
    out_path: str | None,
    *,
    lock_timeout: float = FLOCK_TIMEOUT_SECONDS,
) -> str | None:
    """Emit ``record`` to its sink, degrading rather than dropping it.

    Happy path with ``--out``: append under a bounded flock; returns None.
    On ANY file-sink failure (mkdir, open, lock timeout, write), the SAME line is
    written to stdout instead and a human-readable warning string is returned so
    the caller can surface it on stderr while exit stays 0 (B3).

    ``out_path is None`` writes straight to stdout (returns None).

    Raises ``RecordUnemittable`` when the record reaches NO sink:
    - primary stdout (``out_path is None``) failing with a closed/broken stream
      (``OSError`` or ``ValueError``); a primary ``BrokenPipeError`` instead
      propagates as a clean stream termination (B4).
    - fallback stdout (after the file sink failed) failing with ANY error,
      ``BrokenPipeError`` included, since there is no other sink left.
    """
    # Sanitize first, then dump with allow_nan=False so the line is ALWAYS strict
    # JSON: a stray non-finite float (NaN/Infinity) would otherwise serialise to
    # the bare tokens NaN/Infinity, which the viewer's JSON.parse rejects (we must
    # never emit non-JSON). Non-finite values become null.
    line = json.dumps(sanitize_non_finite(record), separators=(",", ":"), allow_nan=False) + "\n"
    if out_path is None:
        # PRIMARY streaming stdout. A BrokenPipeError here means a streaming
        # consumer (e.g. ``| head -1``) went away — a CLEAN stream termination —
        # so it propagates and main() exits 0. ANY other stdout failure means
        # stdout itself is unusable and the record is lost: a CLOSED stdout raises
        # ValueError("I/O operation on closed file") which is NOT an OSError, so
        # we catch (OSError, ValueError) together — otherwise the ValueError would
        # escape RecordUnemittable and reach main()'s generic handler (wrong path).
        try:
            emit_to_stdout(line)
        except BrokenPipeError:
            raise
        except (OSError, ValueError) as exc:
            raise RecordUnemittable(f"stdout sink failed: {exc}") from exc
        return None

    try:
        # Bound the file sink with the same daemon-thread deadline used for
        # collectors: an EBS/disk stall DURING os.open/os.write is not covered by
        # the bounded LOCK_NB flock (the flock is acquired only after open
        # returns), so without this an open/write that wedges would hang the
        # writer forever. run_with_deadline raises TimeoutError on overrun, and
        # TimeoutError is a subclass of OSError, so the existing
        # ``except OSError as file_exc`` below catches it and falls back to
        # stdout — no new except branch is needed. On a genuine stall the
        # abandoned worker thread may still hold the flock/fd; that is acceptable:
        # one-shot exits immediately, and a continuous run's next write hits the
        # bounded LOCK_NB acquire, which times out and falls back to stdout
        # rather than deadlocking.
        run_with_deadline(write_to_file, SINK_TIMEOUT_SECONDS, line, out_path, lock_timeout)
        return None
    except OSError as file_exc:
        # FALLBACK stdout: this is a last-resort sink AFTER the file sink already
        # failed. Any stdout failure here — INCLUDING BrokenPipeError — means the
        # record reached NO sink and is lost, so it is RecordUnemittable. Unlike
        # the primary path, a broken pipe is NOT a clean termination here: there
        # is no longer any other place the record could have landed.
        try:
            emit_to_stdout(line)
        except (OSError, ValueError) as stdout_exc:
            raise RecordUnemittable(
                f"file sink failed ({file_exc}) and stdout fallback failed ({stdout_exc})"
            ) from stdout_exc
        return f"file sink {out_path!r} failed ({file_exc}); fell back to stdout"


def nonnegative_finite_float(value: str) -> float:
    """argparse ``type=`` for --interval: reject nan/inf/negative/non-numeric.

    A bad interval is a config error that must be caught BEFORE any sampling:
    ``nan`` crashes ``time.sleep`` and ``inf`` makes the sleep loop spin forever.
    Raising ``ArgumentTypeError`` makes argparse exit 2 with a clean message and
    no traceback.

    This is argv validation, so exit code 2 here is intentional and out of scope
    for the prime directive: the never-non-zero guarantee applies only AFTER
    sampling has begun (see the module docstring). A malformed invocation must
    still fail fast rather than silently sampling on a poisoned interval.
    """
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        raise argparse.ArgumentTypeError(f"invalid float value: {value!r}")
    if not math.isfinite(parsed):
        raise argparse.ArgumentTypeError(f"--interval must be finite, got {value!r}")
    if parsed < 0:
        raise argparse.ArgumentTypeError(f"--interval must be >= 0, got {value!r}")
    return parsed


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Emit JSONL host-health telemetry for a systemd service."
    )
    parser.add_argument("--service", default=DEFAULT_SERVICE, help="systemd unit to sample")
    parser.add_argument(
        "--disk-path",
        default=DEFAULT_DISK_PATH,
        help="catalog/data filesystem path to measure",
    )
    parser.add_argument("--out", help="append JSONL output to this path")
    parser.add_argument(
        "--interval",
        type=nonnegative_finite_float,
        default=0,
        help="seconds between samples; 0 emits one record",
    )
    return parser.parse_args(argv)


class StopFlag:
    def __init__(self) -> None:
        self.stop = False

    def request_stop(self, _signum: int, _frame: object) -> None:
        self.stop = True


def sleep_until_next_sample(seconds: float, stop_flag: StopFlag) -> None:
    deadline = time.monotonic() + seconds
    while not stop_flag.stop:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        time.sleep(min(remaining, 0.5))


def log_stderr(msg: str) -> None:
    """Best-effort stderr write that NEVER raises.

    A failing stderr (closed/full pipe) must never change the sampler's exit
    code: if a record already reached a sink, a logging failure cannot demote
    that success to a non-zero exit. Any exception here is swallowed.
    """
    try:
        print(msg, file=sys.stderr)
    except Exception:  # noqa: BLE001 - logging must never affect exit status
        pass


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    stop_flag = StopFlag()
    signal.signal(signal.SIGINT, stop_flag.request_stop)
    signal.signal(signal.SIGTERM, stop_flag.request_stop)
    one_shot = args.interval <= 0

    try:
        while not stop_flag.stop:
            # Per-iteration isolation: a single sample/emit failure must never
            # kill a long-running daemon (B5). Only one-shot mode propagates the
            # failure as a non-zero exit; interval mode degrades and continues.
            try:
                record = sample(service_unit=args.service, disk_path=args.disk_path)
                warning = write_jsonl_line(record, args.out)
                if warning is not None:
                    # Route through log_stderr: a record already reached a sink, so
                    # a stderr write failure here must NOT demote exit 0 to 1.
                    log_stderr(f"host_health_sampler: {warning}")
            except BrokenPipeError:
                # Gone consumer: break out to the clean-termination handler (B4).
                raise
            except RecordUnemittable as exc:
                log_stderr(f"host_health_sampler: {exc}")
                if one_shot:
                    return 1
            except Exception as exc:  # noqa: BLE001 - sample()/emit unexpected failure
                log_stderr(f"host_health_sampler: {exc}")
                if one_shot:
                    return 1
            if one_shot:
                break
            sleep_until_next_sample(args.interval, stop_flag)
    except BrokenPipeError:
        # The consumer closed the pipe (e.g. ``... | head -1``). A gone consumer
        # is a normal stream termination, not a failure (B4).
        pass
    finally:
        # Suppress interpreter-shutdown flush noise without changing the outcome:
        # any flush failure mode (gone pipe, EIO, a test double) must not escape
        # because a record may already have reached the --out sink. An exception
        # here would wrongly demote a clean exit to non-zero. Best-effort redirect
        # fd 1 to /dev/null so the interpreter's final flush cannot re-raise.
        try:
            sys.stdout.flush()
        except Exception:
            redirect_stdout_to_devnull()
    return 0


def redirect_stdout_to_devnull() -> None:
    """Point fd 1 at /dev/null so the interpreter's final flush cannot error.

    Best-effort and total: every OS call is guarded so this fail-safe cleanup can
    never itself raise (it runs after a BrokenPipe when the record already
    reached the file sink — an escaping OSError would flip a clean exit to
    non-zero). A stdout that exposes no real OS file descriptor is left as-is.
    """
    try:
        stdout_fd = sys.stdout.fileno()
    except (AttributeError, OSError, ValueError):
        return
    try:
        devnull = os.open(os.devnull, os.O_WRONLY)
    except OSError:
        return
    try:
        os.dup2(devnull, stdout_fd)
    except OSError:
        pass
    finally:
        try:
            os.close(devnull)
        except OSError:
            pass


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
