#!/usr/bin/env python3
"""Fail when a `scripts/test_*.py` builds a `git` argv directly.

Fixture git must be built by `repo_git_command`/`run_repo_git` in
`ci_workflow_hygiene_test_helpers`, which is the single home for the
auto-maintenance suppression. A test that spells `["git", ...]` itself spawns a
detached `git maintenance run --auto --detach` writer into a temporary
directory the test is about to delete, which crashes the suite
nondeterministically and costs two ordered re-runs to recover from.

Expected-argv values are not executions and stay legal: a comparison operand, a
`CompletedProcess`/`CalledProcessError` argument, or an element of an enclosing
literal.
"""

from __future__ import annotations

import argparse
import ast
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST_GLOB = "scripts/test_*.py"
HELPER = "repo_git_command"

# `["git", ...]` here is data, not a command line.
NON_EXECUTING_CALLS = frozenset({"CompletedProcess", "CalledProcessError"})


class GitArgvVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.parents: list[ast.AST] = []
        self.violations: list[int] = []

    def generic_visit(self, node: ast.AST) -> None:
        self.parents.append(node)
        super().generic_visit(node)
        self.parents.pop()

    def visit_List(self, node: ast.List) -> None:
        if _starts_with_git(node) and not self._is_expected_value(node):
            self.violations.append(node.lineno)
        self.generic_visit(node)

    def _is_expected_value(self, node: ast.List) -> bool:
        parent = self.parents[-1] if self.parents else None
        if isinstance(parent, (ast.Compare, ast.Tuple, ast.List)):
            return True
        if isinstance(parent, ast.Call):
            func = parent.func
            name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
            return name in NON_EXECUTING_CALLS
        return False


def _starts_with_git(node: ast.List) -> bool:
    if not node.elts:
        return False
    first = node.elts[0]
    return isinstance(first, ast.Constant) and first.value == "git"


def violations_in(source: str) -> list[int]:
    visitor = GitArgvVisitor()
    visitor.visit(ast.parse(source))
    return sorted(visitor.violations)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=pathlib.Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    failures: list[str] = []
    for path in sorted(args.repo_root.glob(TEST_GLOB)):
        for lineno in violations_in(path.read_text(encoding="utf-8")):
            failures.append(f"{path.relative_to(args.repo_root)}:{lineno}")

    if failures:
        print(
            f"error: {len(failures)} direct `git` argv in test fixtures; "
            f"build them with `{HELPER}` from ci_workflow_hygiene_test_helpers "
            "so auto-maintenance stays suppressed:",
            file=sys.stderr,
        )
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    print("ok: no direct `git` argv in scripts/test_*.py")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
