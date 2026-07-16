#!/usr/bin/env python3
"""Verify that workspace size has one TOML authority and generated Rust."""

from __future__ import annotations

import ast
import pathlib
import re
import subprocess
import sys
import tomllib


SOURCE = pathlib.Path("config/risk-closure-workspaces.toml")
GENERATED = pathlib.Path("src/bolt_v3_risk_closure_workspace_generated.rs")
RUST_INTEGER = (
    r"(?:0[xX][0-9a-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|[0-9][0-9_]*)"
    r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))?"
)
INTEGER_LITERAL = re.compile(rf"(?<![A-Za-z0-9_]){RUST_INTEGER}(?![A-Za-z0-9_])")
SIZE_EXPRESSION = re.compile(
    rf"(?<![A-Za-z0-9_])({RUST_INTEGER}(?:\s*(?:\*|<<)\s*{RUST_INTEGER})+)(?![A-Za-z0-9_])"
)
SYMBOLIC_AUTHORITY = re.compile(r"\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\b")
INTEGER_SUFFIX = re.compile(r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))$")


def _integer_value(literal: str) -> int:
    normalized = INTEGER_SUFFIX.sub("", literal).replace("_", "")
    if normalized.lower().startswith("0x"):
        base = 16
    elif normalized.lower().startswith("0o"):
        base = 8
    elif normalized.lower().startswith("0b"):
        base = 2
    else:
        base = 10
    digits = normalized[2:] if base != 10 else normalized
    return int(digits, base)


def _expression_value(expression: str) -> int | None:
    normalized = INTEGER_SUFFIX.sub("", expression)
    normalized = re.sub(
        r"(?<=[0-9a-fA-F_])(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))\b",
        "",
        normalized,
    ).replace("_", "")
    try:
        parsed = ast.parse(normalized, mode="eval")
    except SyntaxError:
        return None

    def evaluate(node: ast.AST) -> int:
        if isinstance(node, ast.Expression):
            return evaluate(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Mult, ast.LShift)):
            left = evaluate(node.left)
            right = evaluate(node.right)
            return left * right if isinstance(node.op, ast.Mult) else left << right
        raise ValueError

    try:
        return evaluate(parsed)
    except ValueError:
        return None


def _production_rust_sources(root: pathlib.Path) -> list[pathlib.Path]:
    sources = set((root / "src").rglob("*.rs"))
    build_script = root / "build.rs"
    if build_script.is_file():
        sources.add(build_script)
    crates = root / "crates"
    if crates.is_dir():
        for path in crates.rglob("*.rs"):
            relative_parts = path.relative_to(crates).parts
            if "src" in relative_parts or path.name == "build.rs":
                sources.add(path)
    return sorted(sources)


def authority_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    authorities: list[pathlib.Path] = []
    authoritative_slot_bytes: int | None = None
    for path in sorted((root / "config").rglob("*.toml")):
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect {path.relative_to(root)}: {error}")
            continue
        workspace = document.get("risk_closure_workspaces")
        if isinstance(workspace, dict) and "slot_bytes" in workspace:
            relative = path.relative_to(root)
            authorities.append(relative)
            if relative == SOURCE:
                candidate = workspace["slot_bytes"]
                if (
                    isinstance(candidate, int)
                    and not isinstance(candidate, bool)
                    and candidate > 0
                ):
                    authoritative_slot_bytes = candidate
                else:
                    errors.append(
                        f"{SOURCE} risk_closure_workspaces.slot_bytes must be a positive integer"
                    )
    if authorities != [SOURCE]:
        errors.append(
            "risk_closure_workspaces.slot_bytes must have exactly one TOML authority at "
            f"{SOURCE}; found {authorities}"
        )
    if authoritative_slot_bytes is None:
        return errors

    for path in _production_rust_sources(root):
        relative = path.relative_to(root)
        if relative == GENERATED:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect {relative}: {error}")
            continue
        if any(
            _integer_value(match.group()) == authoritative_slot_bytes
            for match in INTEGER_LITERAL.finditer(text)
        ):
            errors.append(f"runtime workspace-size literal found outside generated Rust: {relative}")
        if any(
            _expression_value(match.group(1)) == authoritative_slot_bytes
            for match in SIZE_EXPRESSION.finditer(text)
        ):
            errors.append(f"runtime workspace-size expression found outside generated Rust: {relative}")
        symbolic_names = (match.group(1) for match in SYMBOLIC_AUTHORITY.finditer(text))
        if any(
            "CLOSURE" in name
            and ("WORKSPACE" in name or "SLOT" in name)
            and ("BYTES" in name or "SIZE" in name)
            for name in symbolic_names
        ):
            errors.append(f"symbolic workspace-size authority found outside generated Rust: {relative}")
    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = authority_errors(root)
    generation = subprocess.run(
        [
            sys.executable,
            str(root / "scripts" / "generate_risk_closure_workspace_config.py"),
            "--source",
            str(root / SOURCE),
            "--output",
            str(root / GENERATED),
            "--check",
        ],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if generation.returncode != 0:
        errors.append(generation.stderr.strip() or "generated Rust configuration is stale")
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: risk-closure workspace slot size has one TOML authority.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
