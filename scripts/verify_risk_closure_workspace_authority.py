#!/usr/bin/env python3
"""Verify that workspace size has one TOML authority and generated Rust."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import tomllib


SOURCE = pathlib.Path("config/risk-closure-workspaces.toml")
GENERATED = pathlib.Path("src/bolt_v3_risk_closure_workspace_generated.rs")
PRODUCTION_SLOT_LITERAL = re.compile(r"(?<![0-9_])(?:16_777_216|16777216)(?![0-9_])")
SYMBOLIC_SIZE_AUTHORITY = re.compile(
    r"\b(?:const|static)\s+[A-Z0-9_]*(?:RISK_CLOSURE|CLOSURE_RISK)[A-Z0-9_]*"
    r"(?:WORKSPACE|SLOT)[A-Z0-9_]*(?:BYTES|SIZE)\b"
)


def authority_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    authorities: list[pathlib.Path] = []
    for path in sorted((root / "config").rglob("*.toml")):
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect {path.relative_to(root)}: {error}")
            continue
        workspace = document.get("risk_closure_workspaces")
        if isinstance(workspace, dict) and "slot_bytes" in workspace:
            authorities.append(path.relative_to(root))
    if authorities != [SOURCE]:
        errors.append(
            "risk_closure_workspaces.slot_bytes must have exactly one TOML authority at "
            f"{SOURCE}; found {authorities}"
        )

    for path in sorted((root / "src").rglob("*.rs")):
        relative = path.relative_to(root)
        if relative == GENERATED:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect {relative}: {error}")
            continue
        if PRODUCTION_SLOT_LITERAL.search(text):
            errors.append(f"runtime workspace-size literal found outside generated Rust: {relative}")
        if SYMBOLIC_SIZE_AUTHORITY.search(text):
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
