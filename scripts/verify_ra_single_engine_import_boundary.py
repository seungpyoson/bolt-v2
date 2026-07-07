#!/usr/bin/env python3
"""Verify Research Analytics never imports NT's Python/Cython backtest engine."""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
FORBIDDEN_MODULES = (
    "nautilus_trader.backtest.engine",
    "nautilus_trader.backtest.node",
)
RA_CODE_DIRS = ("notebooks", "research", "analytics")
RA_SCRIPT_PREFIXES = ("leadlag", "research", "analytics")
PYTHON_SUFFIXES = (".py", ".ipynb")


@dataclass(frozen=True)
class Finding:
    path: Path
    line: int
    module: str

    def message(self, root: Path) -> str:
        rel = self.path.relative_to(root).as_posix()
        return f"{rel}:{self.line}: forbidden RA single-engine import {self.module}"


def research_code_files(root: Path) -> list[Path]:
    paths: set[Path] = set()
    for directory_name in RA_CODE_DIRS:
        directory = root / directory_name
        if directory.exists():
            paths.update(
                path
                for suffix in PYTHON_SUFFIXES
                for path in directory.rglob(f"*{suffix}")
                if path.is_file()
            )

    scripts_dir = root / "scripts"
    if scripts_dir.exists():
        paths.update(
            path
            for suffix in PYTHON_SUFFIXES
            for path in scripts_dir.glob(f"*{suffix}")
            if path.is_file()
            and path.name.startswith(RA_SCRIPT_PREFIXES)
            and not path.name.startswith(("test_", "verify_"))
        )
    return sorted(paths)


def forbidden_module(name: str) -> str | None:
    for module in FORBIDDEN_MODULES:
        if name == module or name.startswith(f"{module}."):
            return module
    return None


def source_findings(path: Path, source: str) -> list[Finding]:
    try:
        tree = ast.parse(source, filename=str(path))
    except SyntaxError:
        return fallback_source_findings(path, source)

    findings: list[Finding] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if module := forbidden_module(alias.name):
                    findings.append(Finding(path, node.lineno, module))
        elif isinstance(node, ast.ImportFrom):
            module_name = node.module or ""
            if module := forbidden_module(module_name):
                findings.append(Finding(path, node.lineno, module))
                continue
            if module_name == "nautilus_trader.backtest":
                for alias in node.names:
                    if alias.name in {"engine", "node"}:
                        findings.append(
                            Finding(path, node.lineno, f"{module_name}.{alias.name}")
                        )
    return findings


IMPORT_RE = re.compile(
    r"^\s*(?:import\s+(?P<import>\S+)|from\s+(?P<from>\S+)\s+import\s+(?P<names>.+))"
)


def fallback_source_findings(path: Path, source: str) -> list[Finding]:
    findings: list[Finding] = []
    for line_number, raw_line in enumerate(source.splitlines(), start=1):
        line = raw_line.split("#", 1)[0]
        match = IMPORT_RE.match(line)
        if match is None:
            continue
        imported = match.group("import")
        if imported and (module := forbidden_module(imported.rstrip(","))):
            findings.append(Finding(path, line_number, module))
            continue
        from_module = match.group("from")
        if from_module and (module := forbidden_module(from_module)):
            findings.append(Finding(path, line_number, module))
            continue
        if from_module == "nautilus_trader.backtest":
            names = {
                token.strip().split(" ", 1)[0]
                for token in match.group("names").split(",")
            }
            for name in sorted(names & {"engine", "node"}):
                findings.append(Finding(path, line_number, f"{from_module}.{name}"))
    return findings


def notebook_code_source(path: Path) -> str:
    data = json.loads(path.read_text(encoding="utf-8"))
    chunks: list[str] = []
    for cell in data.get("cells", []):
        if cell.get("cell_type") != "code":
            continue
        source = cell.get("source", "")
        if isinstance(source, list):
            chunks.append("".join(str(part) for part in source))
        else:
            chunks.append(str(source))
    return "\n".join(chunks)


def file_findings(path: Path) -> list[Finding]:
    if path.suffix == ".ipynb":
        source = notebook_code_source(path)
    else:
        source = path.read_text(encoding="utf-8")
    return source_findings(path, source)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    paths = research_code_files(root)
    findings_text: list[str] = []
    if not require_nonempty(paths, "RA single-engine code files", findings_text):
        return findings_text
    findings: list[Finding] = []
    for path in paths:
        findings.extend(file_findings(path))
    return [finding.message(root) for finding in sorted(findings, key=lambda item: item.message(root))]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA single-engine import boundary violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA single-engine import boundary passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
