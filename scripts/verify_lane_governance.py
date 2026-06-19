#!/usr/bin/env python3
"""Meta-check: every governed lane entry point acquires the lane lock (#653).

Rule: every scripts/verify_*.py and scripts/test_*.py file must have a
module-level ``if __name__ == "__main__":`` block whose first statement is
``import lane_governor`` and whose next statement acquires the lane lock. The
lock may be a bare ``lane_governor.acquire()`` call, or a captured handle that
is released in the immediately following ``try/finally`` block. This makes
lane-coverage drift a CI failure instead of a convention.

This is an entrypoint-governance check, not a general module side-effect
analyzer. Governed files must keep CPU-heavy work behind ``main()``; the checker
does not attempt to classify import-time setup such as fixture constants or
dynamic test-module loading.
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
    return (
        isinstance(left, ast.Name)
        and left.id == "__name__"
        and isinstance(right, ast.Constant)
        and right.value == "__main__"
    ) or (
        isinstance(right, ast.Name)
        and right.id == "__name__"
        and isinstance(left, ast.Constant)
        and left.value == "__main__"
    )


def _is_lane_governor_call(node: ast.AST, name: str) -> bool:
    return (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == name
        and isinstance(node.func.value, ast.Name)
        and node.func.value.id == "lane_governor"
    )


def _is_lane_governor_import(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.Import)
        and len(node.names) == 1
        and node.names[0].name == "lane_governor"
        and node.names[0].asname is None
    )


def _is_bare_acquire_call(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.Expr)
        and _is_lane_governor_call(node.value, "acquire")
        and not node.value.args
        and not node.value.keywords
    )


def _acquire_handle_name(node: ast.stmt) -> str | None:
    if not (
        isinstance(node, ast.Assign)
        and len(node.targets) == 1
        and isinstance(node.targets[0], ast.Name)
        and _is_lane_governor_call(node.value, "acquire")
        and not node.value.args
        and not node.value.keywords
    ):
        return None
    return node.targets[0].id


def _is_release_call(node: ast.stmt, handle_name: str) -> bool:
    return (
        isinstance(node, ast.Expr)
        and _is_lane_governor_call(node.value, "release")
        and len(node.value.args) == 1
        and isinstance(node.value.args[0], ast.Name)
        and node.value.args[0].id == handle_name
        and not node.value.keywords
    )


def _has_immediate_acquire(guard_body: list[ast.stmt]) -> bool:
    if len(guard_body) < 2:
        return False
    if _is_bare_acquire_call(guard_body[1]):
        return True

    handle_name = _acquire_handle_name(guard_body[1])
    return (
        handle_name is not None
        and len(guard_body) == 3
        and isinstance(guard_body[2], ast.Try)
        and bool(guard_body[2].body)
        and not guard_body[2].handlers
        and not guard_body[2].orelse
        and len(guard_body[2].finalbody) == 1
        and _is_release_call(guard_body[2].finalbody[0], handle_name)
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
            violations.append(
                f"{path.name}: governed files must define a module-level __main__ block"
            )
            continue
        for guard in main_guards:
            if not guard.body or not _is_lane_governor_import(guard.body[0]):
                violations.append(
                    f"{path.name}: first statement in the __main__ block "
                    "must be import lane_governor"
                )
                continue
            if not _has_immediate_acquire(guard.body):
                violations.append(
                    f"{path.name}: second statement in the __main__ block "
                    "must be lane_governor.acquire() or a released acquire handle"
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
