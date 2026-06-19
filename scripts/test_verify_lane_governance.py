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

COMPLIANT_SYS_EXIT = '''
import sys

def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
'''

COMPLIANT_UNITTEST = '''
import unittest

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
'''

COMPLIANT_REVERSED_MAIN_GUARD = '''
def main():
    return 0

if "__main__" == __name__:
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
'''

COMPLIANT_RELEASED_HANDLE = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
'''

RELEASED_HANDLE_WITH_WORK_AFTER_RELEASE = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        print("only setup is locked")
    finally:
        lane_governor.release(lock_handle)
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

ACQUIRE_WITH_POLICY_OVERRIDE = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire(lock_dir="/tmp/unshared")
    raise SystemExit(main())
'''

ACQUIRE_WITH_POSITIONAL_ARG = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire("custom-lane")
    raise SystemExit(main())
'''

MISSING_LANE_GOVERNOR_IMPORT = '''
def main():
    return 0

if __name__ == "__main__":
    lane_governor.acquire()
    raise SystemExit(main())
'''

ALIASED_LANE_GOVERNOR_IMPORT = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor as lg

    lg.acquire()
    raise SystemExit(main())
'''

MULTI_NAME_LANE_GOVERNOR_IMPORT = '''
def main():
    return 0

if __name__ == "__main__":
    import lane_governor, os

    lane_governor.acquire()
    raise SystemExit(main())
'''

FROM_IMPORT_ACQUIRE = '''
def main():
    return 0

if __name__ == "__main__":
    from lane_governor import acquire

    acquire()
    raise SystemExit(main())
'''

EXECUTABLE_IMPORT_BEFORE_LANE_GOVERNOR = '''
def main():
    return 0

if __name__ == "__main__":
    import expensive_side_effect
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
'''

LANE_GOVERNOR_IMPORT_TOO_LATE = '''
def main():
    return 0

if __name__ == "__main__":
    print("starting")
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
'''

NO_MAIN_BLOCK = '''
def helper():
    return 0
'''

TOP_LEVEL_WORK_WITHOUT_MAIN = '''
print("running expensive verifier work")
'''

FAKE_CONSTANT_MAIN_GUARD = '''
if "__main__" == "__name__":
    import lane_governor

    lane_governor.acquire()
'''


def _violations(named_sources: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for name, source in named_sources.items():
            (root / name).write_text(source, encoding="utf-8")
        return CHECKER.lane_governance_violations(root)


def test_compliant_file_passes() -> None:
    assert _violations({"verify_sample.py": COMPLIANT}) == []


def test_all_existing_entry_tail_shapes_pass() -> None:
    assert _violations({
        "verify_raise.py": COMPLIANT,
        "test_sys_exit.py": COMPLIANT_SYS_EXIT,
        "test_unittest.py": COMPLIANT_UNITTEST,
    }) == []


def test_reversed_main_guard_passes() -> None:
    assert _violations({"verify_sample.py": COMPLIANT_REVERSED_MAIN_GUARD}) == []


def test_released_acquire_handle_passes() -> None:
    assert _violations({"test_sample.py": COMPLIANT_RELEASED_HANDLE}) == []


def test_released_acquire_handle_with_work_after_release_is_flagged() -> None:
    violations = _violations({"test_sample.py": RELEASED_HANDLE_WITH_WORK_AFTER_RELEASE})
    assert len(violations) == 1 and "released acquire handle" in violations[0]


def test_missing_acquire_flagged() -> None:
    violations = _violations({"verify_sample.py": MISSING_ACQUIRE})
    assert len(violations) == 1 and "verify_sample.py" in violations[0]


def test_missing_lane_governor_import_flagged() -> None:
    violations = _violations({"verify_sample.py": MISSING_LANE_GOVERNOR_IMPORT})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_import_before_lane_governor_is_flagged() -> None:
    violations = _violations({"verify_sample.py": EXECUTABLE_IMPORT_BEFORE_LANE_GOVERNOR})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_lane_governor_import_after_other_statement_flagged() -> None:
    violations = _violations({"verify_sample.py": LANE_GOVERNOR_IMPORT_TOO_LATE})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_acquire_after_other_statement_flagged() -> None:
    violations = _violations({"test_sample.py": ACQUIRE_TOO_LATE})
    assert len(violations) == 1 and "lane_governor.acquire()" in violations[0]


def test_acquire_with_policy_override_flagged() -> None:
    violations = _violations({"verify_sample.py": ACQUIRE_WITH_POLICY_OVERRIDE})
    assert len(violations) == 1 and "lane_governor.acquire()" in violations[0]


def test_acquire_with_positional_arg_flagged() -> None:
    violations = _violations({"verify_sample.py": ACQUIRE_WITH_POSITIONAL_ARG})
    assert len(violations) == 1 and "lane_governor.acquire()" in violations[0]


def test_aliased_lane_governor_import_flagged() -> None:
    violations = _violations({"verify_sample.py": ALIASED_LANE_GOVERNOR_IMPORT})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_multi_name_lane_governor_import_flagged() -> None:
    violations = _violations({"verify_sample.py": MULTI_NAME_LANE_GOVERNOR_IMPORT})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_from_import_acquire_flagged() -> None:
    violations = _violations({"verify_sample.py": FROM_IMPORT_ACQUIRE})
    assert len(violations) == 1 and "import lane_governor" in violations[0]


def test_module_without_main_is_flagged() -> None:
    violations = _violations({"verify_sample.py": NO_MAIN_BLOCK})
    assert len(violations) == 1 and "__main__" in violations[0]


def test_top_level_work_without_main_is_flagged() -> None:
    violations = _violations({"test_sample.py": TOP_LEVEL_WORK_WITHOUT_MAIN})
    assert len(violations) == 1 and "__main__" in violations[0]


def test_fake_constant_main_guard_is_flagged() -> None:
    violations = _violations({"verify_sample.py": FAKE_CONSTANT_MAIN_GUARD})
    assert len(violations) == 1 and "__main__" in violations[0]


def test_non_matching_names_ignored() -> None:
    assert _violations({"leadlag_tool.py": MISSING_ACQUIRE}) == []


def test_real_scripts_dir_is_clean() -> None:
    scripts_dir = Path(__file__).resolve().parent
    governed = sorted(list(scripts_dir.glob("verify_*.py")) + list(scripts_dir.glob("test_*.py")))
    assert governed, "real scripts dir must contain governed verify_*.py/test_*.py files"
    assert CHECKER.lane_governance_violations(scripts_dir) == []


def main() -> int:
    tests = [
        test_compliant_file_passes,
        test_all_existing_entry_tail_shapes_pass,
        test_reversed_main_guard_passes,
        test_released_acquire_handle_passes,
        test_released_acquire_handle_with_work_after_release_is_flagged,
        test_missing_acquire_flagged,
        test_missing_lane_governor_import_flagged,
        test_import_before_lane_governor_is_flagged,
        test_lane_governor_import_after_other_statement_flagged,
        test_acquire_after_other_statement_flagged,
        test_acquire_with_policy_override_flagged,
        test_acquire_with_positional_arg_flagged,
        test_aliased_lane_governor_import_flagged,
        test_multi_name_lane_governor_import_flagged,
        test_from_import_acquire_flagged,
        test_module_without_main_is_flagged,
        test_top_level_work_without_main_is_flagged,
        test_fake_constant_main_guard_is_flagged,
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
