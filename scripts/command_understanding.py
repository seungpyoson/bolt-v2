"""Shared command-understanding helpers for repo verification scripts."""

from __future__ import annotations

import ast
import shlex


CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT = {
    "--color",
    "--config",
    "--jobs",
    "--manifest-path",
    "--profile",
    "--target",
    "--target-dir",
    "-C",
    "-Z",
}
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT = {
    "--help",
    "--list",
    "--frozen",
    "--locked",
    "--offline",
    "--quiet",
    "--verbose",
    "--version",
    "-q",
    "-v",
    "-V",
}
NEXTEST_GLOBAL_OPTIONS_WITH_ARGUMENT = {"--config-file", "--manifest-path", "--profile", "--workspace-remap"}


def python_constant_string(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = python_constant_string(node.left)
        right = python_constant_string(node.right)
        if left is not None and right is not None:
            return left + right
    if isinstance(node, ast.JoinedStr):
        parts: list[str] = []
        for value in node.values:
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                parts.append(value.value)
            else:
                return None
        return "".join(parts)
    return None


def python_command_string(node: ast.AST) -> str | None:
    scalar = python_constant_string(node)
    if scalar is not None:
        return scalar
    if isinstance(node, (ast.List, ast.Tuple)):
        parts: list[str] = []
        for element in node.elts:
            part = python_constant_string(element)
            if part is None:
                return None
            parts.append(part)
        return shlex.join(parts)
    return None


def python_call_name(node: ast.AST) -> str:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        prefix = python_call_name(node.value)
        return f"{prefix}.{node.attr}" if prefix else node.attr
    return ""


def python_call_command_argument(node: ast.Call) -> ast.AST | None:
    if node.args:
        return node.args[0]
    for keyword in node.keywords:
        if keyword.arg in {"args", "command"}:
            return keyword.value
    return None


def python_inline_command_payloads(tokens: list[str]) -> list[str]:
    payloads: list[str] = []
    for index, token in enumerate(tokens):
        if token != "-c" or index + 1 >= len(tokens):
            continue
        try:
            tree = ast.parse(tokens[index + 1])
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            if python_call_name(node.func) not in {
                "os.system",
                "subprocess.call",
                "subprocess.check_call",
                "subprocess.check_output",
                "subprocess.Popen",
                "subprocess.run",
            }:
                continue
            command_argument = python_call_command_argument(node)
            if command_argument is None:
                continue
            payload = python_command_string(command_argument)
            if payload is not None:
                payloads.append(payload)
    return payloads


def cargo_subcommand_with_index(tokens: list[str], start: int = 0) -> tuple[int, str] | None:
    index = start
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("+"):
            index += 1
            continue
        if token == "--":
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return index, token
    return None


def cargo_subcommand(tokens: list[str]) -> str | None:
    subcommand = cargo_subcommand_with_index(tokens)
    if subcommand is None:
        return None
    return subcommand[1]


def nextest_subcommand_with_index(nextest_args: list[str]) -> tuple[int, str] | None:
    index = 0
    while index < len(nextest_args):
        token = nextest_args[index]
        if token == "--":
            return None
        if token in NEXTEST_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in NEXTEST_GLOBAL_OPTIONS_WITH_ARGUMENT):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return index, token
    return None


def cargo_args_for_target_routing_scan(tokens: list[str]) -> list[str]:
    subcommand = cargo_subcommand_with_index(tokens)
    if subcommand is None:
        return tokens
    subcommand_index, subcommand_name = subcommand
    if subcommand_name == "nextest":
        nextest_subcommand = nextest_subcommand_with_index(tokens[subcommand_index + 1 :])
        if nextest_subcommand is None or nextest_subcommand[1] != "run":
            return tokens
        nextest_run_index = subcommand_index + 1 + nextest_subcommand[0]
        for index, token in enumerate(tokens):
            if index > nextest_run_index and token == "--":
                return tokens[:index]
        return tokens
    if subcommand_name not in {"bench", "run", "test"}:
        return tokens
    for index, token in enumerate(tokens):
        if index > subcommand_index and token == "--":
            return tokens[:index]
    return tokens
