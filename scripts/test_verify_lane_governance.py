#!/usr/bin/env python3
"""Self-tests for the lane-governance meta-check (#653)."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("verify_lane_governance.py")
SPEC = importlib.util.spec_from_file_location("verify_lane_governance", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)

COMPLIANT = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
'''

MISSING_ACQUIRE = '''
def main():
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
'''

ACQUIRE_TOO_LATE = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    print("starting")
    lane_governor.acquire()
    raise SystemExit(main())
'''

NO_MAIN_BLOCK = '''
def helper():
    return 0
'''


def _violations(named_sources: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for name, source in named_sources.items():
            (root / name).write_text(source, encoding="utf-8")
        return CHECKER.lane_governance_violations(root)


def test_compliant_file_passes() -> None:
    assert _violations({"verify_sample.py": COMPLIANT}) == []


def test_missing_acquire_flagged() -> None:
    violations = _violations({"verify_sample.py": MISSING_ACQUIRE})
    assert len(violations) == 1 and "verify_sample.py" in violations[0]


def test_acquire_after_other_statement_flagged() -> None:
    violations = _violations({"test_sample.py": ACQUIRE_TOO_LATE})
    assert len(violations) == 1 and "first executable statement" in violations[0]


def test_module_without_main_is_exempt() -> None:
    assert _violations({"verify_sample.py": NO_MAIN_BLOCK}) == []


def test_non_matching_names_ignored() -> None:
    assert _violations({"leadlag_tool.py": MISSING_ACQUIRE}) == []


def test_real_scripts_dir_is_clean() -> None:
    assert CHECKER.lane_governance_violations(Path(__file__).resolve().parent) == []


def main() -> int:
    tests = [
        test_compliant_file_passes,
        test_missing_acquire_flagged,
        test_acquire_after_other_statement_flagged,
        test_module_without_main_is_exempt,
        test_non_matching_names_ignored,
        test_real_scripts_dir_is_clean,
    ]
    for test in tests:
        test()
    print("OK: lane-governance meta-check self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
