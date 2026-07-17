#!/usr/bin/env python3
"""Verify the structured risk-closure workspace configuration authority."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tomllib


SOURCE = pathlib.Path("config/risk-closure-workspaces.toml")
GENERATED = pathlib.Path(
    "src/bolt_v3_application_resource_ledger/risk_closure_workspace/generated.rs"
)
IGNORED_TOML_PATH_PARTS = frozenset({".git", ".worktrees", "target"})


def _repository_toml_sources(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not IGNORED_TOML_PATH_PARTS.intersection(path.relative_to(root).parts)
    )


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
    """Return structured TOML authority errors without interpreting Rust source."""

    errors: list[str] = []
    authorities: list[tuple[pathlib.Path, tuple[str | int, ...]]] = []

    for path in _repository_toml_sources(root):
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect {path.relative_to(root)}: {error}")
            continue

        relative = path.relative_to(root)
        for key_path in _toml_key_paths(document, "risk_closure_workspaces"):
            authorities.append((relative, key_path))

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
    print("OK: risk-closure workspace TOML authority and generated Rust agree.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
