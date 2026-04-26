#!/usr/bin/env python3
"""Verify forbidden stale Bolt-v3 field names do not return to canonical docs."""

from __future__ import annotations

import sys
import re
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
CANONICAL_DOCS = [
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md",
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md",
]

FORBIDDEN = {
    "trader_identifier": "use trader_id",
    "event_timestamp_milliseconds": "use ts_event",
    "nautilus_up_instrument_id": "use up_instrument_id",
    "nautilus_down_instrument_id": "use down_instrument_id",
    "post_only": "use is_post_only",
    "reduce_only": "use is_reduce_only",
    "quote_quantity": "use is_quote_quantity",
    "strategy_instance_identifier": "use strategy_instance_id",
    "release_identifier": "use release_id",
    "instrument_identifier": "use instrument_id",
    "venue_order_identifier": "use venue_order_id",
}

REQUIRED = [
    "trader_id",
    "ts_event",
    "up_instrument_id",
    "down_instrument_id",
    "is_post_only",
    "is_reduce_only",
    "is_quote_quantity",
    "strategy_instance_id",
    "release_id",
]


def main() -> int:
    findings: list[str] = []
    combined = ""
    for path in CANONICAL_DOCS:
        text = path.read_text(encoding="utf-8")
        combined += text
        for forbidden, replacement in FORBIDDEN.items():
            if re.search(rf"(?<![A-Za-z0-9_]){re.escape(forbidden)}(?![A-Za-z0-9_])", text):
                findings.append(f"{path}: forbidden {forbidden!r}; {replacement}")

    for required in REQUIRED:
        if required not in combined:
            findings.append(f"required canonical name {required!r} is absent")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 canonical naming audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
