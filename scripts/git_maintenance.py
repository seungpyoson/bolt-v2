#!/usr/bin/env python3
"""Shared Git auto-maintenance suppression and Trace2 inspection."""

from __future__ import annotations

GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG = (
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
)
