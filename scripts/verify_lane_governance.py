#!/usr/bin/env python3
"""Meta-check: every governed lane entry point acquires the lane lock (#653).

Rule: in every scripts/verify_*.py and scripts/test_*.py that has a module-level
``if __name__ == "__main__":`` block, the first two statements of that block
must be ``import lane_governor`` and a bare ``lane_governor.acquire(...)`` call.
Files without a ``__main__`` block cannot run as lanes and are exempt. This
makes lane-coverage drift a CI failure instead of a convention.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent


def _is_main_guard(node: ast.stmt) -> bool:
    if not isinstance(node, ast.If):
        return False
    test = node.test
    if not isinstance(test, ast.Compare) or len(test.ops) != 1:
        return False
    if not isinstance(test.ops[0], ast.Eq):
        return False
    left, right = test.left, test.comparators[0]
    names = set()
    for side in (left, right):
        if isinstance(side, ast.Name):
            names.add(side.id)
        elif isinstance(side, ast.Constant):
            names.add(side.value)
    return "__name__" in names and "__main__" in names


def _is_acquire_call(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.Expr)
        and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Attribute)
        and node.value.func.attr == "acquire"
        and isinstance(node.value.func.value, ast.Name)
        and node.value.func.value.id == "lane_governor"
    )


def _is_lane_governor_import(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.Import)
        and len(node.names) == 1
        and node.names[0].name == "lane_governor"
        and node.names[0].asname is None
    )


def lane_governance_violations(scripts_dir: Path) -> list[str]:
    violations: list[str] = []
    governed = sorted(
        list(scripts_dir.glob("verify_*.py")) + list(scripts_dir.glob("test_*.py"))
    )
    for path in governed:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        main_guards = [node for node in tree.body if _is_main_guard(node)]
        if not main_guards:
            continue
        for guard in main_guards:
            if not guard.body or not _is_lane_governor_import(guard.body[0]):
                violations.append(
                    f"{path.name}: first statement in the __main__ block "
                    "must be import lane_governor"
                )
                continue
            if len(guard.body) < 2 or not _is_acquire_call(guard.body[1]):
                violations.append(
                    f"{path.name}: second statement in the __main__ block "
                    "must be lane_governor.acquire()"
                )
    return violations


def main() -> int:
    violations = lane_governance_violations(SCRIPTS_DIR)
    if violations:
        print("Lane-governance violations:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1
    print("OK: all governed lane entry points acquire the lane lock.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
