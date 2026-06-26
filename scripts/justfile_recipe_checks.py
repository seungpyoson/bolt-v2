#!/usr/bin/env python3
"""Small helpers for checking commands inside just recipe bodies."""

from __future__ import annotations


def recipe_commands(justfile_text: str, recipe_name: str) -> set[str]:
    commands: set[str] = set()
    in_recipe = False
    for raw_line in justfile_text.splitlines():
        stripped = raw_line.strip()
        if not in_recipe:
            if raw_line.startswith(f"{recipe_name}:"):
                in_recipe = True
            continue
        if raw_line and not raw_line.startswith((" ", "\t")):
            break
        if stripped and not stripped.startswith("#"):
            commands.add(stripped)
    return commands


def missing_recipe_commands(
    justfile_text: str,
    required_commands: tuple[str, ...],
    *,
    recipe_name: str = "source-fence-static-inner",
) -> list[str]:
    commands = recipe_commands(justfile_text, recipe_name)
    return [command for command in required_commands if command not in commands]
