#!/usr/bin/env python3
"""Verify that workspace size has one TOML authority and generated Rust."""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tomllib


SOURCE = pathlib.Path("config/risk-closure-workspaces.toml")
GENERATED = pathlib.Path("src/bolt_v3_risk_closure_workspace/generated.rs")
OWNER = pathlib.Path("src/bolt_v3_risk_closure_workspace.rs")
RUST_INTEGER = (
    r"(?:0[xX][0-9a-fA-F_]+|0[oO][0-7_]+|0[bB][01_]+|[0-9][0-9_]*)"
    r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))?"
)
INTEGER_LITERAL = re.compile(rf"(?<![A-Za-z0-9_]){RUST_INTEGER}(?![A-Za-z0-9_])")
SYMBOLIC_AUTHORITY = re.compile(r"\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\b")
INTEGER_SUFFIX = re.compile(r"(?:usize|isize|u(?:8|16|32|64|128)|i(?:8|16|32|64|128))$")
IGNORED_TOML_PATH_PARTS = frozenset({".git", ".worktrees", "target"})


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


def _repository_toml_sources(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not IGNORED_TOML_PATH_PARTS.intersection(path.relative_to(root).parts)
    )


def _positive_integer(value: object) -> int | None:
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return value
    return None


def _toml_key_paths(
    value: object,
    target: str,
    prefix: tuple[str | int, ...] = (),
) -> list[tuple[str | int, ...]]:
    paths: list[tuple[str | int, ...]] = []
    if isinstance(value, dict):
        for key, child in value.items():
            path = (*prefix, key)
            if key == target:
                paths.append(path)
            paths.extend(_toml_key_paths(child, target, path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            paths.extend(_toml_key_paths(child, target, (*prefix, index)))
    return paths


def _render_toml_key_path(path: tuple[str | int, ...]) -> str:
    rendered = ""
    for component in path:
        if isinstance(component, int):
            rendered += f"[{component}]"
        else:
            rendered += f"[{json.dumps(component)}]"
    return rendered


def authority_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    authorities: list[tuple[pathlib.Path, tuple[str | int, ...]]] = []
    authoritative_arena_bytes: int | None = None
    authoritative_slot_bytes: int | None = None
    for path in _repository_toml_sources(root):
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key_path in _toml_key_paths(document, "risk_closure_workspaces"):
            authorities.append((relative, key_path))
            if relative != SOURCE or key_path != ("risk_closure_workspaces",):
                continue
            workspace = document["risk_closure_workspaces"]
            if not isinstance(workspace, dict):
                errors.append(f"{SOURCE} risk_closure_workspaces must be a table")
                continue
            authoritative_arena_bytes = _positive_integer(workspace.get("arena_bytes"))
            authoritative_slot_bytes = _positive_integer(workspace.get("slot_bytes"))
            if authoritative_arena_bytes is None:
                errors.append(
                    f"{SOURCE} risk_closure_workspaces.arena_bytes must be a positive integer"
                )
            if authoritative_slot_bytes is None:
                errors.append(
                    f"{SOURCE} risk_closure_workspaces.slot_bytes must be a positive integer"
                )
    expected_authority = [(SOURCE, ("risk_closure_workspaces",))]
    if authorities != expected_authority:
        rendered_authorities = [
            f"{path}::{_render_toml_key_path(key_path)}"
            for path, key_path in authorities
        ]
        errors.append(
            "risk_closure_workspaces geometry must have exactly one TOML authority at "
            f"{SOURCE}::risk_closure_workspaces; found {rendered_authorities}"
        )
    if authoritative_arena_bytes is None or authoritative_slot_bytes is None:
        return errors
    authoritative_sizes = {authoritative_arena_bytes, authoritative_slot_bytes}

    try:
        owner_text = (root / OWNER).read_text(encoding="utf-8")
        generated_text = (root / GENERATED).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        errors.append(f"cannot inspect private workspace authority surface: {error}")
        return errors
    if re.search(r"\bpub\s+(?:\([^)]*\)\s+)?struct\s+RiskClosureWorkspaceConfig\b", owner_text):
        errors.append(f"workspace configuration type must remain private to {OWNER}")
    if re.search(r"\bpub\s+(?:\([^)]*\)\s+)?const\s+RISK_CLOSURE_WORKSPACE_CONFIG\b", generated_text):
        errors.append(f"generated workspace configuration must remain private to {OWNER}")
    if "const RISK_CLOSURE_WORKSPACE_CONFIG" not in generated_text:
        errors.append(f"generated workspace configuration is missing from {GENERATED}")

    for path in _production_rust_sources(root):
        relative = path.relative_to(root)
        if relative == GENERATED:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect {relative}: {error}")
            continue
        if relative != OWNER and re.search(
            r"\b(?:RiskClosureWorkspaceConfig|RISK_CLOSURE_WORKSPACE_CONFIG)\b", text
        ):
            errors.append(f"private workspace configuration referenced outside {OWNER}: {relative}")
        if any(
            _integer_value(match.group()) in authoritative_sizes
            for match in INTEGER_LITERAL.finditer(text)
        ):
            errors.append(f"runtime workspace-size literal found outside generated Rust: {relative}")
        symbolic_names = (match.group(1) for match in SYMBOLIC_AUTHORITY.finditer(text))
        if any(
            "CLOSURE" in name
            and ("WORKSPACE" in name or "SLOT" in name or "ARENA" in name)
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
    print("OK: risk-closure workspace geometry has one TOML authority.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
