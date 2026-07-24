#!/usr/bin/env python3
"""Behavioral tests for the governed sccache statistics gate."""

from __future__ import annotations

import copy
import json
import pathlib
import subprocess
import tempfile
from typing import Any


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
GATE = REPO_ROOT / "scripts" / "sccache_stats_gate.sh"


def base_stats() -> dict[str, Any]:
    return {
        "version": "0.16.0",
        "stats": {
            "compile_requests": 1,
            "requests_unsupported_compiler": 0,
            "requests_not_compile": 0,
            "requests_not_cacheable": 0,
            "requests_executed": 1,
            "cache_errors": {"counts": {}, "adv_counts": {}},
            "cache_misses": {"counts": {"Rust": 1}, "adv_counts": {"rustc": 1}},
            "cache_timeouts": 0,
            "cache_read_errors": 0,
            "cache_write_errors": 1,
            "cache_writes": 0,
            "dist_errors": 0,
        },
    }


def run_gate(
    stats: dict[str, Any],
    *,
    cache_mode: str = "read_only",
    require_compiler_requests: bool = True,
) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as tmp:
        stats_path = pathlib.Path(tmp) / "stats.json"
        stats_path.write_text(json.dumps(stats), encoding="utf-8")
        return subprocess.run(
            [
                GATE,
                stats_path,
                cache_mode,
                "v0.16.0",
                str(require_compiler_requests).lower(),
            ],
            check=False,
            capture_output=True,
            text=True,
        )


def expect_status(
    label: str,
    stats: dict[str, Any],
    expected_status: int,
    *,
    cache_mode: str = "read_only",
    require_compiler_requests: bool = True,
) -> None:
    result = run_gate(
        stats,
        cache_mode=cache_mode,
        require_compiler_requests=require_compiler_requests,
    )
    if result.returncode != expected_status:
        raise AssertionError(
            f"{label}: expected status {expected_status}, got {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )


def main() -> int:
    if not GATE.is_file():
        raise AssertionError(f"missing executable statistics gate: {GATE}")

    expect_status("ordinary read-only miss", base_stats(), 0)

    missing_refusal = base_stats()
    missing_refusal["stats"]["cache_write_errors"] = 0
    expect_status("read-only miss without write refusal", missing_refusal, 1)

    successful_read_only_write = base_stats()
    successful_read_only_write["stats"]["cache_write_errors"] = 0
    successful_read_only_write["stats"]["cache_writes"] = 1
    expect_status("successful read-only write", successful_read_only_write, 1)

    read_write = base_stats()
    read_write["stats"]["cache_write_errors"] = 0
    read_write["stats"]["cache_writes"] = 1
    expect_status("ordinary read-write miss", read_write, 0, cache_mode="read_write")

    read_write_error = copy.deepcopy(read_write)
    read_write_error["stats"]["cache_write_errors"] = 1
    expect_status("read-write error", read_write_error, 1, cache_mode="read_write")

    runtime_error = copy.deepcopy(read_write)
    runtime_error["stats"]["cache_timeouts"] = 1
    expect_status("runtime error", runtime_error, 1, cache_mode="read_write")

    incomplete_request = copy.deepcopy(read_write)
    incomplete_request["stats"]["requests_executed"] = 0
    expect_status("incomplete request accounting", incomplete_request, 1, cache_mode="read_write")

    wrong_version = copy.deepcopy(read_write)
    wrong_version["version"] = "0.15.0"
    expect_status("wrong version", wrong_version, 1, cache_mode="read_write")

    malformed = copy.deepcopy(read_write)
    del malformed["stats"]["cache_writes"]
    expect_status("missing official field", malformed, 1, cache_mode="read_write")

    no_requests = base_stats()
    no_requests["stats"]["compile_requests"] = 0
    no_requests["stats"]["requests_executed"] = 0
    no_requests["stats"]["cache_misses"] = {"counts": {}, "adv_counts": {}}
    no_requests["stats"]["cache_write_errors"] = 0
    expect_status("required compiler request", no_requests, 1)
    expect_status(
        "optional compiler request",
        no_requests,
        0,
        require_compiler_requests=False,
    )

    expect_status("invalid cache mode", read_write, 1, cache_mode="unexpected")

    print("OK: sccache statistics gate behavioral tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
