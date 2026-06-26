#!/usr/bin/env python3
"""Verify active SpecKit context files without running heavy source-fence checks."""

from __future__ import annotations

import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from verify_bolt_v3_schema_current import AGENTS_DOC, FEATURE_JSON, validate_speckit_context  # noqa: E402


def validate_current_speckit_context(
    agents_doc: Path = AGENTS_DOC,
    feature_json: Path = FEATURE_JSON,
) -> list[str]:
    return validate_speckit_context(
        agents_doc.read_text(encoding="utf-8"),
        feature_json.read_text(encoding="utf-8"),
    )


def main() -> int:
    findings = validate_current_speckit_context()
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: SpecKit context files match active feature pointers.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
