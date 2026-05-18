#!/usr/bin/env python3
"""Verify no-mistakes cannot launch local Cargo for bolt-v2 gates."""

from __future__ import annotations

import argparse
import ast
import re
import sys
from pathlib import Path


EXPECTED_COMMANDS = {
    "test": "python3 scripts/no_mistakes_ci_gate.py test",
    "lint": "python3 scripts/no_mistakes_ci_gate.py lint",
    "format": "python3 scripts/no_mistakes_ci_gate.py format",
}
RAW_CARGO_RE = re.compile(r"(^|[^A-Za-z0-9_./-])cargo\s+(test|clippy|fmt|build|check|nextest)\b")


def _decode_scalar(raw: str) -> str:
    raw = raw.strip()
    if raw.startswith(("'", '"')):
        try:
            value = ast.literal_eval(raw)
        except (SyntaxError, ValueError) as exc:
            raise ValueError(f"invalid quoted command value: {raw}") from exc
        if not isinstance(value, str):
            raise ValueError(f"command value must be a string: {raw}")
        return value
    return raw


def parse_no_mistakes_commands(path: Path) -> dict[str, str]:
    commands: dict[str, str] = {}
    in_commands = False
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if line.startswith("commands:"):
            in_commands = True
            continue
        if in_commands and line[:1] not in (" ", "\t"):
            break
        if not in_commands:
            continue
        match = re.match(r"^\s{2}([A-Za-z0-9_-]+):\s*(.+?)\s*$", line)
        if match:
            commands[match.group(1)] = _decode_scalar(match.group(2))
    return commands


def validate_no_mistakes_config(path: Path) -> list[str]:
    errors: list[str] = []
    commands = parse_no_mistakes_commands(path)
    for name, expected in EXPECTED_COMMANDS.items():
        actual = commands.get(name)
        if actual != expected:
            errors.append(f"commands.{name} must be {expected!r}, got {actual!r}")
    for name, command in sorted(commands.items()):
        if RAW_CARGO_RE.search(command):
            errors.append(f"commands.{name} launches raw Cargo: {command!r}")
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default=".no-mistakes.yaml")
    args = parser.parse_args(argv)

    config = Path(args.config)
    errors = validate_no_mistakes_config(config)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: no-mistakes Rust gates use exact-head GitHub CI instead of local Cargo")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
