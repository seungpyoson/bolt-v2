#!/usr/bin/env python3
"""Standalone host and service health sampling for visibility-only telemetry.

This script emits one JSONL record containing host, systemd service, process,
memory, and disk observations. It is intentionally standalone and stdlib-only
so operators can run it directly on a Linux systemd EC2 host, while local macOS
development runs still emit a valid degraded record instead of going silent.

Every collector returns ``(value, error_or_none)`` and collection failures are
reported in the row's ``errors`` array. Collector errors never prevent the
sampler from writing a JSON record.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
from typing import Any, Callable


SCHEMA_VERSION = 2
DEFAULT_SERVICE = "bolt-v2"
# This is the bot's catalog/data dir whose source of truth is config/root.toml
# key ``catalog_directory``; it is overridable for tests and ad hoc checks.
DEFAULT_DISK_PATH = "/srv/bolt-v2/var/bolt-v3-live/catalog"
SYSTEMCTL_TIMEOUT_SECONDS = 5
KNOWN_NON_OOM_RESULTS = {
    "success",
    "exit-code",
    "signal",
    "core-dump",
    "timeout",
    "watchdog",
    "start-limit-hit",
    "resources",
    "protocol",
    "assert",
}


CollectorResult = tuple[Any, str | None]


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


def derive_oom_killed(result: str | None) -> bool | None:
    """Derive OOM solely from systemd's authoritative Result field."""
    if result == "oom-kill":
        return True
    if result in KNOWN_NON_OOM_RESULTS:
        return False
    return None


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
    try:
        result = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            timeout=SYSTEMCTL_TIMEOUT_SECONDS,
        )
    except FileNotFoundError:
        return None, "systemctl unavailable"
    except subprocess.TimeoutExpired:
        return None, f"systemctl show timed out after {SYSTEMCTL_TIMEOUT_SECONDS}s"
    except Exception as exc:  # pragma: no cover - subprocess edge cases
        return None, f"systemctl show failed: {exc}"

    if result.returncode != 0:
        reason = result.stderr.strip().splitlines()[0] if result.stderr.strip() else "no stderr"
        return None, f"systemctl show exited {result.returncode}: {reason}"

    parsed: dict[str, str] = {}
    for raw_line in result.stdout.splitlines():
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


def collect_cgroup_oom_kills(unit: str) -> CollectorResult:
    paths = [
        Path("/sys/fs/cgroup/system.slice") / candidate / "memory.events"
        for candidate in cgroup_unit_candidates(unit)
    ]
    missing: list[str] = []
    for path in paths:
        try:
            with path.open("r", encoding="utf-8") as handle:
                for line in handle:
                    fields = line.split()
                    if len(fields) == 2 and fields[0] == "oom_kill":
                        return int(fields[1]), None
                return None, f"{path} did not contain oom_kill"
        except FileNotFoundError:
            missing.append(str(path))
        except PermissionError as exc:
            return None, str(exc)
        except ValueError as exc:
            return None, f"{path} parse error: {exc}"
        except OSError as exc:
            return None, str(exc)
    return None, "memory.events unavailable at " + ", ".join(missing)


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
        raw_lines = path.read_text(encoding="utf-8").splitlines()
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

    missing_original = not candidate.exists()
    current = candidate
    while not current.exists():
        parent = current.parent
        if parent == current:
            return None, f"disk path {requested_path!r} and ancestors are missing"
        current = parent

    if missing_original:
        return current, (
            f"requested disk path {requested_path!r} missing; "
            f"measured nearest existing ancestor {current}"
        )
    return current, None


def disk_metrics_from_statvfs(path: str, statvfs_result: os.statvfs_result, st_dev: int) -> dict[str, Any]:
    fragment_size = statvfs_result.f_frsize
    total_bytes = statvfs_result.f_blocks * fragment_size
    free_bytes = statvfs_result.f_bavail * fragment_size
    used_bytes = (statvfs_result.f_blocks - statvfs_result.f_bfree) * fragment_size
    available_bytes = statvfs_result.f_bavail * fragment_size
    denominator = used_bytes + available_bytes
    used_pct = round(used_bytes / denominator * 100, 2) if denominator > 0 else None
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
    return disk, path_error


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
    return Path(f"/proc/{pid}/cgroup").read_text(encoding="utf-8")


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
        raw_lines = path.read_text(encoding="utf-8").splitlines()
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
        lines = path.read_text(encoding="utf-8").splitlines()
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

    return process, "; ".join(errors) if errors else None


def run_collector(source: str, function: Callable[..., CollectorResult], *args: Any) -> CollectorResult:
    try:
        value, error = function(*args)
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
    return {
        "schema_version": SCHEMA_VERSION,
        "sampled_at": datetime.now(timezone.utc).isoformat(),
        "host": host,
        "platform": sys.platform,
        "service": service,
        "process": process,
        "memory": memory,
        "disk": disk,
        "oom_killed": derive_oom_killed(service_result),
        "errors": errors,
    }


def write_jsonl_line(record: dict[str, Any], out_path: str | None) -> None:
    line = json.dumps(record, separators=(",", ":")) + "\n"
    if out_path is None:
        sys.stdout.write(line)
        sys.stdout.flush()
        return

    path = Path(out_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        os.write(fd, line.encode("utf-8"))
    finally:
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_UN)
            except OSError:
                pass
        finally:
            os.close(fd)


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
        type=float,
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


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    stop_flag = StopFlag()
    signal.signal(signal.SIGINT, stop_flag.request_stop)
    signal.signal(signal.SIGTERM, stop_flag.request_stop)

    try:
        while not stop_flag.stop:
            record = sample(service_unit=args.service, disk_path=args.disk_path)
            write_jsonl_line(record, args.out)
            if args.interval <= 0:
                break
            sleep_until_next_sample(args.interval, stop_flag)
    except Exception as exc:  # noqa: BLE001 - non-zero only if no record/write is possible
        print(f"host_health_sampler: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
