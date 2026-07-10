#!/usr/bin/env python3
"""Fence direct fixture-repository git execution in ``scripts/test_*.py``.

Fixture git must be built by the helpers in
``ci_workflow_hygiene_test_helpers``. They are the single home for the
auto-maintenance suppression that keeps detached maintenance writers out of
temporary repositories. This verifier rejects direct git argv spelling,
execution edges that resolve to git, and fixture ``init``/``clone`` calls that
bypass the dedicated constructors.

Expected argv values remain legal in comparisons, exception/process results,
and enclosing literals. An opaque callable in the command position cannot be
resolved statically and is intentionally not flagged; this is the fence's
known limit.
"""

from __future__ import annotations

import argparse
import ast
import collections
import pathlib
import shlex
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST_GLOB = "scripts/test_*.py"

Violation = collections.namedtuple("Violation", "lineno rule message")

NON_EXECUTING_CALLS = frozenset({"CompletedProcess", "CalledProcessError"})
ALLOWED_BUILDERS = frozenset({"repo_git_command", "run_repo_git"})
SHELL_WRAPPERS = frozenset({"sh", "bash", "zsh", "dash", "env"})
EXECUTION_FUNCTIONS = {
    "subprocess": frozenset(
        {
            "run",
            "call",
            "check_call",
            "check_output",
            "Popen",
            "getoutput",
            "getstatusoutput",
        }
    ),
    "os": frozenset(
        {
            "system",
            "popen",
            "execl",
            "execle",
            "execlp",
            "execv",
            "execve",
            "execvp",
            "execvpe",
            "spawnl",
            "spawnle",
            "spawnlp",
            "spawnv",
            "spawnve",
            "spawnvp",
            "posix_spawn",
        }
    ),
}

ARGV_MESSAGE = (
    "direct git argv spelling; build commands with repo_git_command/run_repo_git"
)
EXECUTION_MESSAGE = (
    "direct git execution edge; route commands through repo_git_command/run_repo_git"
)
CONSTRUCTOR_MESSAGE = (
    "fixture repository construction must use init_fixture_repo/clone_fixture_repo "
    "from ci_workflow_hygiene_test_helpers"
)


def _basename(value: str) -> str:
    return pathlib.PurePath(value).name


def _call_name(call: ast.Call) -> str:
    if isinstance(call.func, ast.Attribute):
        return call.func.attr
    if isinstance(call.func, ast.Name):
        return call.func.id
    return ""


def _which_git(node: ast.AST) -> bool:
    if not isinstance(node, ast.Call) or len(node.args) != 1:
        return False
    func = node.func
    is_which = (
        isinstance(func, ast.Name)
        and func.id == "which"
        or isinstance(func, ast.Attribute)
        and isinstance(func.value, ast.Name)
        and func.value.id == "shutil"
        and func.attr == "which"
    )
    arg = node.args[0]
    return (
        is_which
        and isinstance(arg, ast.Constant)
        and isinstance(arg.value, str)
        and arg.value == "git"
    )


def _bound_values(tree: ast.AST) -> dict[str, set[str]]:
    values: dict[str, set[str]] = collections.defaultdict(set)
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Assign, ast.AnnAssign)):
            continue
        value = node.value
        if not isinstance(value, ast.Constant) or not isinstance(value.value, str):
            continue
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        for target in targets:
            if isinstance(target, ast.Name):
                values[target.id].add(value.value)
    return dict(values)


def _imported_execution_names(
    tree: ast.AST,
) -> tuple[dict[str, tuple[str, str]], dict[str, str]]:
    bare: dict[str, tuple[str, str]] = {}
    modules = {"subprocess": "subprocess", "os": "os"}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name in EXECUTION_FUNCTIONS:
                    modules[alias.asname or alias.name] = alias.name
        elif isinstance(node, ast.ImportFrom) and node.module in EXECUTION_FUNCTIONS:
            for alias in node.names:
                if alias.name in EXECUTION_FUNCTIONS[node.module]:
                    bare[alias.asname or alias.name] = (node.module, alias.name)
    return bare, modules


def _execution_command_index(
    call: ast.Call,
    bare: dict[str, tuple[str, str]],
    modules: dict[str, str],
    wrappers: dict[str, int],
) -> int | None:
    func = call.func
    if isinstance(func, ast.Name):
        if func.id in bare:
            return 0
        return wrappers.get(func.id)
    if not isinstance(func, ast.Attribute) or not isinstance(func.value, ast.Name):
        return None
    module = modules.get(func.value.id)
    if module is None or func.attr not in EXECUTION_FUNCTIONS[module]:
        return None
    return 0


def _forwarded_parameter_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Starred):
        node = node.value
    return node.id if isinstance(node, ast.Name) else None


def _local_execution_wrappers(
    tree: ast.Module,
    bare: dict[str, tuple[str, str]],
    modules: dict[str, str],
) -> dict[str, int]:
    wrappers: dict[str, int] = {}
    functions = [node for node in tree.body if isinstance(node, ast.FunctionDef)]

    changed = True
    while changed:
        changed = False
        for function in functions:
            if function.name in wrappers:
                continue
            positional = function.args.posonlyargs + function.args.args
            parameter_indexes = {
                parameter.arg: index for index, parameter in enumerate(positional)
            }
            for node in ast.walk(function):
                if not isinstance(node, ast.Call):
                    continue
                command_index = _execution_command_index(
                    node, bare, modules, wrappers
                )
                if command_index is None or command_index >= len(node.args):
                    continue
                parameter_name = _forwarded_parameter_name(
                    node.args[command_index]
                )
                if parameter_name not in parameter_indexes:
                    continue
                wrappers[function.name] = parameter_indexes[parameter_name]
                changed = True
                break
    return wrappers


def _resolves_to_git(node: ast.AST, bindings: dict[str, set[str]]) -> bool:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return _basename(node.value) == "git"
    if isinstance(node, ast.Name):
        return any(_basename(value) == "git" for value in bindings.get(node.id, ()))
    return _which_git(node)


def _literal_program(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return _basename(node.value)
    return None


def _tokens_resolve_to_git(tokens: list[str]) -> bool:
    if not tokens:
        return False
    program = _basename(tokens[0])
    if program == "git":
        return True
    if program not in SHELL_WRAPPERS or len(tokens) < 2:
        return False
    if program == "env":
        return _basename(tokens[1]) == "git"
    if "-c" in tokens:
        index = tokens.index("-c") + 1
        if index >= len(tokens):
            return False
        try:
            return _tokens_resolve_to_git(shlex.split(tokens[index]))
        except ValueError:
            return False
    return _basename(tokens[1]) == "git"


def _string_resolves_to_git(value: str) -> bool:
    try:
        return _tokens_resolve_to_git(shlex.split(value))
    except ValueError:
        return False


def _sequence_resolves_to_git(
    node: ast.List | ast.Tuple, bindings: dict[str, set[str]]
) -> bool:
    if not node.elts:
        return False
    first = node.elts[0]
    if _resolves_to_git(first, bindings):
        return True
    wrapper = _literal_program(first)
    if wrapper not in SHELL_WRAPPERS:
        return False
    remaining = node.elts[1:]
    if wrapper == "env":
        return bool(remaining) and _resolves_to_git(remaining[0], bindings)
    for index, element in enumerate(remaining):
        if isinstance(element, ast.Constant) and element.value == "-c":
            if index + 1 >= len(remaining):
                return False
            payload = remaining[index + 1]
            return (
                isinstance(payload, ast.Constant)
                and isinstance(payload.value, str)
                and _string_resolves_to_git(payload.value)
            )
    return any(_resolves_to_git(element, bindings) for element in remaining)


def _command_resolves_to_git(
    node: ast.AST, bindings: dict[str, set[str]]
) -> bool:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return _string_resolves_to_git(node.value)
    if isinstance(node, ast.JoinedStr):
        if not node.values or not isinstance(node.values[0], ast.Constant):
            return False
        leading = node.values[0].value
        return isinstance(leading, str) and _string_resolves_to_git(leading)
    if isinstance(node, (ast.List, ast.Tuple)):
        return _sequence_resolves_to_git(node, bindings)
    if isinstance(node, ast.Call):
        if _call_name(node) in ALLOWED_BUILDERS:
            return False
        return _which_git(node)
    return _resolves_to_git(node, bindings)


class GitFixtureVisitor(ast.NodeVisitor):
    def __init__(self, tree: ast.Module) -> None:
        self.parents: list[ast.AST] = []
        self.bindings = _bound_values(tree)
        self.bare_execution, self.module_aliases = _imported_execution_names(tree)
        self.local_execution = _local_execution_wrappers(
            tree, self.bare_execution, self.module_aliases
        )
        self.violations: dict[int, Violation] = {}
        self.execution_commands: set[int] = set()

    def generic_visit(self, node: ast.AST) -> None:
        self.parents.append(node)
        super().generic_visit(node)
        self.parents.pop()

    def visit_Call(self, node: ast.Call) -> None:
        command_index = _execution_command_index(
            node,
            self.bare_execution,
            self.module_aliases,
            self.local_execution,
        )
        command = (
            node.args[command_index]
            if command_index is not None and command_index < len(node.args)
            else None
        )
        if command is not None:
            if _command_resolves_to_git(command, self.bindings):
                self.execution_commands.add(id(command))
                self._add(node.lineno, "execution-edge", EXECUTION_MESSAGE)

        first_string = next(
            (
                arg.value
                for arg in node.args
                if isinstance(arg, ast.Constant) and isinstance(arg.value, str)
            ),
            None,
        )
        if first_string in {"init", "clone"}:
            self._add(node.lineno, "fixture-constructor", CONSTRUCTOR_MESSAGE)

        self.generic_visit(node)

    def visit_List(self, node: ast.List) -> None:
        if (
            id(node) not in self.execution_commands
            and self._starts_with_git(node)
            and not self._list_is_expected_value()
        ):
            self._add(node.lineno, "argv-spelling", ARGV_MESSAGE)
        self.generic_visit(node)

    def _add(self, lineno: int, rule: str, message: str) -> None:
        self.violations.setdefault(lineno, Violation(lineno, rule, message))

    def _starts_with_git(self, node: ast.List | ast.Tuple) -> bool:
        return bool(node.elts) and _resolves_to_git(node.elts[0], self.bindings)

    def _list_is_expected_value(self) -> bool:
        parent = self.parents[-1] if self.parents else None
        if isinstance(parent, (ast.Compare, ast.Tuple, ast.List)):
            return True
        return isinstance(parent, ast.Call) and _call_name(parent) in NON_EXECUTING_CALLS

def violations_in(source: str) -> list[Violation]:
    tree = ast.parse(source)
    visitor = GitFixtureVisitor(tree)
    visitor.visit(tree)
    return sorted(visitor.violations.values())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=pathlib.Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    failures: list[str] = []
    for path in sorted(args.repo_root.glob(TEST_GLOB)):
        for violation in violations_in(path.read_text(encoding="utf-8")):
            failures.append(
                f"{path.relative_to(args.repo_root)}:{violation.lineno}: "
                f"{violation.message}"
            )

    if failures:
        print(
            f"error: {len(failures)} fixture git helper violation(s):",
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
