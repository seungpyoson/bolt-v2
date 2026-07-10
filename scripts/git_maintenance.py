#!/usr/bin/env python3
"""Shared Git auto-maintenance suppression and Trace2 inspection."""

from __future__ import annotations

import json
import pathlib


GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG = (
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
)


def count_trace2_maintenance_children(trace_path: pathlib.Path) -> int:
    """Count `git maintenance` and `git gc` children in a Trace2 event log."""
    if not trace_path.exists():
        return 0
    total = 0
    for line in trace_path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("event") != "child_start":
            continue
        argv = " ".join(event.get("argv", []))
        if "maintenance" in argv or argv.startswith("git gc"):
            total += 1
    return total
