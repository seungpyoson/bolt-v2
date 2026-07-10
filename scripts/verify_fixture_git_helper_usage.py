#!/usr/bin/env python3
"""Fence direct fixture-repository Git execution in ``scripts/test_*.py``.

Fixture Git commands must use ``repo_git_command``/``run_repo_git``; fixture
repositories must use ``init_fixture_repo``/``clone_fixture_repo``. The AST
scan recognizes the process APIs and command-prefix grammars declared in
``process_execution_edges``, their import and assignment aliases, variables,
and local wrappers that forward a command parameter. Static shell payloads are
split into commands, and constant ``exec``/``eval`` payloads are scanned
recursively. A recognized execution edge is rejected unless its process image
and wrapped command can be proven non-Git or routed through the shared helper.

The scan also rejects direct Git argv list spelling outside inert expected
values. Comparisons, enclosing literals, ``CompletedProcess`` and
``CalledProcessError`` fixtures, ordinary strings, and provably non-Git
commands remain legal. This is structured AST/command analysis; it does not
search raw source text for words such as ``git``, ``init``, or ``clone``.
"""

from __future__ import annotations

import argparse
import ast
import collections
import pathlib
import shlex
import sys

from process_execution_edges import (
    COMMAND_PREFIX_SPECS,
    COMMAND_PREFIX_WRAPPERS,
    DYNAMIC_BUILTINS,
    PROCESS_API_SPECS,
    SHELL_INTERPRETERS,
)

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST_GLOB = "scripts/test_*.py"

Violation = collections.namedtuple("Violation", "lineno rule message")

NON_EXECUTING_CALLS = frozenset({"CompletedProcess", "CalledProcessError"})
ALLOWED_BUILDERS = frozenset({"repo_git_command"})
EXECUTION_FUNCTIONS = PROCESS_API_SPECS

ExecutionSpec = collections.namedtuple("ExecutionSpec", "index keyword")
CommandResolution = collections.namedtuple("CommandResolution", "kind argv")
GIT = "git"
NON_GIT = "non-git"
UNKNOWN = "unknown"
ROUTED_GIT = "routed-git"

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


class BindingIndex(ast.NodeVisitor):
    """Lexical bindings used to resolve command variables conservatively."""

    def __init__(self, tree: ast.Module) -> None:
        self.bindings: dict[int, dict[str, list[ast.AST]]] = collections.defaultdict(
            lambda: collections.defaultdict(list)
        )
        self.parents: dict[int, ast.AST | None] = {id(tree): None}
        self.scope_stack: list[ast.AST] = [tree]

    def _bind(self, target: ast.AST, value: ast.AST) -> None:
        if isinstance(target, ast.Name):
            self.bindings[id(self.scope_stack[-1])][target.id].append(value)

    def visit_Assign(self, node: ast.Assign) -> None:
        for target in node.targets:
            self._bind(target, node.value)
        self.visit(node.value)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if node.value is not None:
            self._bind(node.target, node.value)
            self.visit(node.value)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
        self._bind(node.target, node.value)
        self.visit(node.value)

    def _visit_scope(self, node: ast.AST) -> None:
        self.parents[id(node)] = self.scope_stack[-1]
        self.scope_stack.append(node)
        self.generic_visit(node)
        self.scope_stack.pop()

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_scope(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_scope(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._visit_scope(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self._visit_scope(node)

    def values(self, name: str, scopes: list[ast.AST]) -> list[ast.AST]:
        for scope in reversed(scopes):
            values = self.bindings.get(id(scope), {}).get(name)
            if values:
                return values
        return []


def _imported_execution_names(
    tree: ast.AST,
) -> tuple[dict[str, tuple[str, str]], dict[str, str]]:
    bare: dict[str, tuple[str, str]] = {}
    modules = {module: module for module in EXECUTION_FUNCTIONS}
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


def _imported_dynamic_names(tree: ast.AST) -> tuple[dict[str, str], set[str]]:
    names = {name: name for name in DYNAMIC_BUILTINS}
    modules = {"builtins"}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "builtins":
                    modules.add(alias.asname or alias.name)
        elif isinstance(node, ast.ImportFrom) and node.module == "builtins":
            for alias in node.names:
                if alias.name in DYNAMIC_BUILTINS:
                    names[alias.asname or alias.name] = alias.name
    return names, modules


def _direct_execution_spec(
    call: ast.Call,
    bare: dict[str, tuple[str, str]],
    modules: dict[str, str],
) -> ExecutionSpec | None:
    func = call.func
    if isinstance(func, ast.Name):
        imported = bare.get(func.id)
        if imported is None:
            return None
        index, keyword = EXECUTION_FUNCTIONS[imported[0]][imported[1]]
        return ExecutionSpec(index, keyword)
    if not isinstance(func, ast.Attribute):
        return None
    if isinstance(func.value, ast.Name):
        module = modules.get(func.value.id)
    elif (
        isinstance(func.value, ast.Call)
        and _call_name(func.value) == "__import__"
        and func.value.args
        and isinstance(func.value.args[0], ast.Constant)
        and isinstance(func.value.args[0].value, str)
    ):
        module = func.value.args[0].value
    else:
        module = None
    if module not in EXECUTION_FUNCTIONS:
        return None
    raw = EXECUTION_FUNCTIONS[module].get(func.attr)
    return ExecutionSpec(*raw) if raw is not None else None


def _command_operand(call: ast.Call, spec: ExecutionSpec) -> ast.AST | None:
    for keyword in call.keywords:
        if keyword.arg == spec.keyword:
            return keyword.value
    return call.args[spec.index] if spec.index < len(call.args) else None


def _forwarded_parameter_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Starred):
        node = node.value
    return node.id if isinstance(node, ast.Name) else None


def _callable_parts(
    node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
) -> tuple[str | None, list[ast.arg], list[ast.stmt] | ast.expr]:
    if isinstance(node, ast.Lambda):
        return None, node.args.posonlyargs + node.args.args + node.args.kwonlyargs, node.body
    return node.name, node.args.posonlyargs + node.args.args + node.args.kwonlyargs, node.body


def _own_calls(body: list[ast.stmt] | ast.expr) -> list[ast.Call]:
    calls: list[ast.Call] = []

    class CallCollector(ast.NodeVisitor):
        def visit_Call(self, node: ast.Call) -> None:
            calls.append(node)
            self.generic_visit(node)

        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            return

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            return

        def visit_Lambda(self, node: ast.Lambda) -> None:
            return

    collector = CallCollector()
    if isinstance(body, list):
        for statement in body:
            collector.visit(statement)
    else:
        collector.visit(body)
    return calls


def _local_execution_wrappers(
    tree: ast.Module,
    bare: dict[str, tuple[str, str]],
    modules: dict[str, str],
) -> dict[str, set[ExecutionSpec]]:
    wrappers: dict[str, set[ExecutionSpec]] = collections.defaultdict(set)
    callables: list[tuple[str, ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda]] = []
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            callables.append((node.name, node))
        elif isinstance(node, (ast.Assign, ast.AnnAssign)) and isinstance(
            node.value, ast.Lambda
        ):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                if isinstance(target, ast.Name):
                    callables.append((target.id, node.value))

    changed = True
    while changed:
        changed = False
        for wrapper_name, function in callables:
            _unused_name, parameters, body = _callable_parts(function)
            indexes = {parameter.arg: index for index, parameter in enumerate(parameters)}
            for call in _own_calls(body):
                specs: set[ExecutionSpec] = set()
                direct = _direct_execution_spec(call, bare, modules)
                if direct is not None:
                    specs.add(direct)
                else:
                    specs.update(wrappers.get(_call_name(call), ()))
                for called_spec in specs:
                    operand = _command_operand(call, called_spec)
                    parameter_name = (
                        _forwarded_parameter_name(operand)
                        if operand is not None
                        else None
                    )
                    if parameter_name not in indexes:
                        continue
                    wrapper_spec = ExecutionSpec(indexes[parameter_name], parameter_name)
                    if wrapper_spec not in wrappers[wrapper_name]:
                        wrappers[wrapper_name].add(wrapper_spec)
                        changed = True
    return dict(wrappers)


def _literal_value(node: ast.AST) -> object | None:
    try:
        return ast.literal_eval(node)
    except (ValueError, TypeError):
        return None


def _shell_resolution(command: str) -> CommandResolution:
    if any(marker in command for marker in ("$", "`")):
        return CommandResolution(UNKNOWN, ())
    lexer = shlex.shlex(command, posix=True, punctuation_chars=";&|()")
    lexer.whitespace_split = True
    try:
        tokens = list(lexer)
    except ValueError:
        return CommandResolution(UNKNOWN, ())
    segments: list[list[str]] = [[]]
    for token in tokens:
        if token and all(character in ";&|()" for character in token):
            if segments[-1]:
                segments.append([])
            continue
        segments[-1].append(token)
    resolutions = [_tokens_resolution(segment) for segment in segments if segment]
    git_resolution = next(
        (resolution for resolution in resolutions if resolution.kind == GIT), None
    )
    if git_resolution is not None:
        return git_resolution
    if any(resolution.kind == UNKNOWN for resolution in resolutions):
        return CommandResolution(UNKNOWN, ())
    return CommandResolution(NON_GIT, ())


def _unwrap_command_prefix(tokens: list[str], program: str) -> list[str] | None:
    spec = COMMAND_PREFIX_SPECS[program]
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if not token.startswith("-") or token == "-":
            break
        option = token.split("=", 1)[0]
        if option in spec.value_options and "=" in token:
            index += 1
            continue
        if option in spec.value_options:
            index += 2
            continue
        if token in spec.flag_options:
            index += 1
            continue
        if spec.numeric_options and token[1:].isdigit():
            index += 1
            continue
        if any(
            token.startswith(prefix) and token != prefix
            for prefix in spec.attached_value_prefixes
        ):
            index += 1
            continue
        return None
    index += spec.leading_operands
    if index > len(tokens):
        return None
    if index == len(tokens):
        return list(spec.default_command) if spec.default_command is not None else None
    return tokens[index:]


def _tokens_resolution(tokens: list[str]) -> CommandResolution:
    if not tokens:
        return CommandResolution(UNKNOWN, ())
    if any(marker in tokens[0] for marker in ("$", "`", "*", "?", "[", "{")):
        return CommandResolution(UNKNOWN, ())
    program = _basename(tokens[0])
    if program == "git":
        return CommandResolution(GIT, tuple(tokens[1:]))
    if program == "env":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                break
            if token in {"-u", "--unset"}:
                index += 2
                continue
            if token.startswith("--unset=") or token in {
                "-i",
                "--ignore-environment",
                "-0",
                "--null",
            }:
                index += 1
                continue
            if token == "-S" or token == "--split-string":
                if index + 1 >= len(tokens):
                    return CommandResolution(UNKNOWN, ())
                try:
                    split = shlex.split(tokens[index + 1])
                except ValueError:
                    return CommandResolution(UNKNOWN, ())
                return _tokens_resolution(split + tokens[index + 2 :])
            if token.startswith("--split-string="):
                try:
                    split = shlex.split(token.split("=", 1)[1])
                except ValueError:
                    return CommandResolution(UNKNOWN, ())
                return _tokens_resolution(split + tokens[index + 1 :])
            if "=" in token and not token.startswith("="):
                index += 1
                continue
            break
        return _tokens_resolution(tokens[index:])
    if program in COMMAND_PREFIX_WRAPPERS:
        command = _unwrap_command_prefix(tokens, program)
        if command is None:
            return CommandResolution(UNKNOWN, ())
        return _tokens_resolution(command)
    if program in SHELL_INTERPRETERS:
        command_index = next(
            (
                index + 1
                for index, token in enumerate(tokens[1:], start=1)
                if token == "-c"
                or token.startswith("-")
                and not token.startswith("--")
                and "c" in token[1:]
            ),
            None,
        )
        if command_index is None:
            return CommandResolution(NON_GIT, ())
        if command_index >= len(tokens):
            return CommandResolution(UNKNOWN, ())
        try:
            return _shell_resolution(tokens[command_index])
        except ValueError:
            return CommandResolution(UNKNOWN, ())
    return CommandResolution(NON_GIT, ())


class GitFixtureVisitor(ast.NodeVisitor):
    def __init__(self, tree: ast.Module) -> None:
        self.parents: list[ast.AST] = []
        self.scope_stack: list[ast.AST] = [tree]
        self.bindings = BindingIndex(tree)
        self.bindings.visit(tree)
        self.bare_execution, self.module_aliases = _imported_execution_names(tree)
        self.dynamic_names, self.dynamic_modules = _imported_dynamic_names(tree)
        self.local_execution = _local_execution_wrappers(
            tree, self.bare_execution, self.module_aliases
        )
        self.violations: dict[tuple[int, str], Violation] = {}
        self.execution_commands: set[int] = set()

    def _binding_values(self, name: str) -> list[ast.AST]:
        return self.bindings.values(name, self.scope_stack)

    def _literal_string(self, node: ast.AST, seen: frozenset[str] = frozenset()) -> str | None:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            if node.id in seen:
                return None
            values = self._binding_values(node.id)
            resolved = {
                self._literal_string(value, seen | {node.id}) for value in values
            }
            return resolved.pop() if len(resolved) == 1 and None not in resolved else None
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = self._literal_string(node.left, seen)
            right = self._literal_string(node.right, seen)
            return left + right if left is not None and right is not None else None
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod):
            template = self._literal_string(node.left, seen)
            values = _literal_value(node.right)
            if template is None or values is None:
                return None
            try:
                result = template % values
            except (TypeError, ValueError):
                return None
            return result if isinstance(result, str) else None
        if isinstance(node, ast.JoinedStr):
            parts: list[str] = []
            for value in node.values:
                if isinstance(value, ast.FormattedValue):
                    part = self._literal_string(value.value, seen)
                    if part is None:
                        literal = _literal_value(value.value)
                        part = str(literal) if literal is not None else None
                else:
                    part = self._literal_string(value, seen)
                if part is None:
                    return None
                parts.append(part)
            return "".join(parts)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            receiver = self._literal_string(node.func.value, seen)
            if node.func.attr == "join" and receiver is not None and len(node.args) == 1:
                sequence = self._sequence_elements(node.args[0], seen)
                if sequence is None:
                    return None
                parts = [self._literal_string(part, seen) for part in sequence]
                return receiver.join(parts) if all(part is not None for part in parts) else None
            if node.func.attr == "format" and receiver is not None:
                args = [_literal_value(arg) for arg in node.args]
                kwargs = {
                    keyword.arg: _literal_value(keyword.value)
                    for keyword in node.keywords
                    if keyword.arg is not None
                }
                if any(value is None for value in args) or any(
                    value is None for value in kwargs.values()
                ):
                    return None
                try:
                    return receiver.format(*args, **kwargs)
                except (IndexError, KeyError, ValueError):
                    return None
        if isinstance(node, ast.Call) and _call_name(node) == "str" and len(node.args) == 1:
            value = _literal_value(node.args[0])
            return str(value) if value is not None else None
        return None

    def _sequence_elements(
        self, node: ast.AST, seen: frozenset[str] = frozenset()
    ) -> tuple[ast.AST, ...] | None:
        if isinstance(node, (ast.List, ast.Tuple)):
            return tuple(node.elts)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = self._sequence_elements(node.left, seen)
            right = self._sequence_elements(node.right, seen)
            return left + right if left is not None and right is not None else None
        if isinstance(node, ast.Name) and node.id not in seen:
            values = self._binding_values(node.id)
            sequences = [self._sequence_elements(value, seen | {node.id}) for value in values]
            dumps = {
                tuple(ast.dump(element) for element in sequence)
                for sequence in sequences
                if sequence is not None
            }
            if len(sequences) == 1 and len(dumps) == 1:
                return sequences[0]
        return None

    def _program_resolution(
        self, node: ast.AST, seen: frozenset[str] = frozenset()
    ) -> str:
        literal = self._literal_string(node, seen)
        if literal is not None:
            return GIT if _basename(literal) == "git" else NON_GIT
        if _which_git(node):
            return GIT
        basename = self._path_basename(node, seen)
        if basename is not None:
            return GIT if basename == "git" else NON_GIT
        if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
            if node.value.id == "sys" and node.attr == "executable":
                return NON_GIT
        if isinstance(node, ast.Name) and node.id not in seen:
            values = self._binding_values(node.id)
            kinds = {self._program_resolution(value, seen | {node.id}) for value in values}
            if len(kinds) == 1:
                return kinds.pop()
        return UNKNOWN

    def _path_basename(
        self, node: ast.AST, seen: frozenset[str] = frozenset()
    ) -> str | None:
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
            final = self._literal_string(node.right, seen)
            return _basename(final) if final is not None else None
        if isinstance(node, ast.Call):
            name = _call_name(node)
            if name in {"Path", "PurePath"} and node.args:
                value = self._literal_string(node.args[0], seen)
                return _basename(value) if value is not None else None
            if name == "str" and len(node.args) == 1:
                return self._path_basename(node.args[0], seen)
            if name == "which" and len(node.args) == 1:
                value = self._literal_string(node.args[0], seen)
                return _basename(value) if value is not None else None
        if isinstance(node, ast.Name) and node.id not in seen:
            values = self._binding_values(node.id)
            basenames = {
                self._path_basename(value, seen | {node.id}) for value in values
            }
            return basenames.pop() if len(basenames) == 1 and None not in basenames else None
        return None

    def _command_resolution(
        self, node: ast.AST, seen: frozenset[str] = frozenset()
    ) -> CommandResolution:
        if isinstance(node, ast.Name) and node.id not in seen:
            values = self._binding_values(node.id)
            resolutions = [
                self._command_resolution(value, seen | {node.id}) for value in values
            ]
            unique = {(result.kind, result.argv) for result in resolutions}
            if len(unique) == 1:
                kind, argv = unique.pop()
                return CommandResolution(kind, argv)
            return CommandResolution(UNKNOWN, ())
        literal = self._literal_string(node, seen)
        if literal is not None:
            try:
                return _tokens_resolution(shlex.split(literal))
            except ValueError:
                return CommandResolution(UNKNOWN, ())
        elements = self._sequence_elements(node, seen)
        if elements is not None:
            if not elements:
                return CommandResolution(UNKNOWN, ())
            tokens = [self._literal_string(element, seen) for element in elements]
            if all(token is not None for token in tokens):
                return _tokens_resolution([token for token in tokens if token is not None])
            first_kind = self._program_resolution(elements[0], seen)
            if first_kind == GIT:
                return CommandResolution(
                    GIT,
                    tuple(self._literal_string(element, seen) for element in elements[1:]),
                )
            if first_kind == NON_GIT:
                first = self._literal_string(elements[0], seen)
                if first is not None and _basename(first) in (
                    COMMAND_PREFIX_WRAPPERS | SHELL_INTERPRETERS | {"env"}
                ):
                    return CommandResolution(UNKNOWN, ())
                return CommandResolution(NON_GIT, ())
            return CommandResolution(UNKNOWN, ())
        if isinstance(node, ast.Call):
            if _call_name(node) in ALLOWED_BUILDERS:
                return CommandResolution(
                    ROUTED_GIT,
                    tuple(self._literal_string(arg, seen) for arg in node.args),
                )
            if _which_git(node):
                return CommandResolution(GIT, ())
        return CommandResolution(UNKNOWN, ())

    def _execution_specs(self, node: ast.Call) -> set[ExecutionSpec]:
        return self._callable_execution_specs(node.func)

    def _callable_execution_specs(
        self, func: ast.AST, seen: frozenset[str] = frozenset()
    ) -> set[ExecutionSpec]:
        probe = ast.Call(func=func, args=[], keywords=[])
        direct = _direct_execution_spec(
            probe, self.bare_execution, self.module_aliases
        )
        if direct is not None:
            return {direct}
        if isinstance(func, ast.Name):
            specs = set(self.local_execution.get(func.id, ()))
            if func.id in seen:
                return specs
            for value in self._binding_values(func.id):
                specs.update(
                    self._callable_execution_specs(value, seen | {func.id})
                )
            return specs
        if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
            if func.value.id in seen:
                return set(self.local_execution.get(func.attr, ()))
            specs = set(self.local_execution.get(func.attr, ()))
            for value in self._binding_values(func.value.id):
                rebound = ast.Attribute(value=value, attr=func.attr, ctx=ast.Load())
                specs.update(
                    self._callable_execution_specs(
                        rebound, seen | {func.value.id}
                    )
                )
            return specs
        if isinstance(func, ast.Attribute):
            return set(self.local_execution.get(func.attr, ()))
        return set()

    def _dynamic_call_kind(
        self, func: ast.AST, seen: frozenset[str] = frozenset()
    ) -> str | None:
        if isinstance(func, ast.Name):
            if func.id in self.dynamic_names:
                return self.dynamic_names[func.id]
            if func.id in seen:
                return None
            kinds = {
                self._dynamic_call_kind(value, seen | {func.id})
                for value in self._binding_values(func.id)
            }
            kinds.discard(None)
            return kinds.pop() if len(kinds) == 1 else None
        if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
            if (
                func.value.id in self.dynamic_modules
                and func.attr in DYNAMIC_BUILTINS
            ):
                return func.attr
            if func.value.id in seen:
                return None
            kinds = {
                self._dynamic_call_kind(
                    ast.Attribute(value=value, attr=func.attr, ctx=ast.Load()),
                    seen | {func.value.id},
                )
                for value in self._binding_values(func.value.id)
            }
            kinds.discard(None)
            return kinds.pop() if len(kinds) == 1 else None
        return None

    def _dynamic_payload_is_safe(self, node: ast.Call, kind: str) -> bool:
        payload_node = _command_operand(node, ExecutionSpec(0, "source"))
        if payload_node is None:
            return False
        payload = self._literal_string(payload_node)
        if payload is None:
            return False
        try:
            payload_tree = ast.parse(payload, mode=kind)
        except SyntaxError:
            return False
        visitor = GitFixtureVisitor(payload_tree)
        visitor.visit(payload_tree)
        return not visitor.violations

    def _is_forwarded_wrapper_operand(self, node: ast.AST) -> bool:
        if not isinstance(node, (ast.Name, ast.Starred)):
            return False
        parameter = _forwarded_parameter_name(node)
        scope = self.scope_stack[-1]
        if not isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return False
        return any(
            spec.keyword == parameter
            for spec in self.local_execution.get(scope.name, ())
        )

    @staticmethod
    def _is_constructor_argv(argv: tuple[str | None, ...]) -> bool:
        index = 0
        options_with_values = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix", "--config-env"}
        while index < len(argv):
            token = argv[index]
            if token is None:
                return False
            if token == "--":
                index += 1
                break
            if token in options_with_values:
                index += 2
                continue
            if token.startswith(("-C", "-c")) and token not in {"-C", "-c"}:
                index += 1
                continue
            if token.startswith(("--git-dir=", "--work-tree=", "--namespace=", "--exec-path=", "--super-prefix=", "--config-env=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            break
        return index < len(argv) and argv[index] in {"init", "clone"}

    def _constructor_argv(self, node: ast.Call) -> tuple[str | None, ...] | None:
        name = _call_name(node)
        if name in {"init_fixture_repo", "clone_fixture_repo"}:
            return None
        if name == "repo_git_command":
            return tuple(self._literal_string(arg) for arg in node.args)
        if name in {"run_repo_git", "run_git", "git"} and node.args:
            return tuple(self._literal_string(arg) for arg in node.args[1:])
        for spec in self._execution_specs(node):
            operand = _command_operand(node, spec)
            if operand is None:
                continue
            executable = next(
                (
                    keyword.value
                    for keyword in node.keywords
                    if keyword.arg == "executable"
                ),
                None,
            )
            if executable is not None and self._program_resolution(executable) == GIT:
                elements = self._sequence_elements(operand)
                if elements is not None:
                    return tuple(self._literal_string(element) for element in elements)
            resolution = self._command_resolution(operand)
            if resolution.kind in {GIT, ROUTED_GIT}:
                return resolution.argv
        return None

    def generic_visit(self, node: ast.AST) -> None:
        self.parents.append(node)
        super().generic_visit(node)
        self.parents.pop()

    def _visit_scope(self, node: ast.AST) -> None:
        self.scope_stack.append(node)
        self.generic_visit(node)
        self.scope_stack.pop()

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_scope(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_scope(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._visit_scope(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self._visit_scope(node)

    def visit_Call(self, node: ast.Call) -> None:
        dynamic_kind = self._dynamic_call_kind(node.func)
        if dynamic_kind is not None and not self._dynamic_payload_is_safe(
            node, dynamic_kind
        ):
            self._add(node.lineno, "execution-edge", EXECUTION_MESSAGE)

        constructor_argv = self._constructor_argv(node)
        if constructor_argv is not None and self._is_constructor_argv(constructor_argv):
            self._add(node.lineno, "fixture-constructor", CONSTRUCTOR_MESSAGE)

        for spec in self._execution_specs(node):
            command = _command_operand(node, spec)
            if command is None:
                self._add(node.lineno, "execution-edge", EXECUTION_MESSAGE)
                continue
            if self._is_forwarded_wrapper_operand(command):
                continue
            executable = next(
                (
                    keyword.value
                    for keyword in node.keywords
                    if keyword.arg == "executable"
                ),
                None,
            )
            if executable is not None and self._program_resolution(executable) != NON_GIT:
                self._add(node.lineno, "execution-edge", EXECUTION_MESSAGE)
            resolution = self._command_resolution(command)
            if resolution.kind != NON_GIT and resolution.kind != ROUTED_GIT:
                self.execution_commands.add(id(command))
                self._add(node.lineno, "execution-edge", EXECUTION_MESSAGE)

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
        self.violations.setdefault((lineno, rule), Violation(lineno, rule, message))

    def _starts_with_git(self, node: ast.List | ast.Tuple) -> bool:
        return bool(node.elts) and self._program_resolution(node.elts[0]) == GIT

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

    print("ok: fixture Git execution in scripts/test_*.py uses shared helpers")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
