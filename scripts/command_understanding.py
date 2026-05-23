"""Shared command-understanding helpers for repo verification scripts."""

from __future__ import annotations

import ast
import shlex


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
