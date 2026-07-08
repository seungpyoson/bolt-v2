#!/usr/bin/env python3
"""Verify Research Analytics notebook code stays read-only."""

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
RA_SCRIPT_PREFIXES = ("leadlag", "research", "analytics")
PYTHON_SUFFIXES = (".py", ".ipynb")
RA_NOTEBOOKS_CODE_FILES_LABEL = "RA notebook read-only notebooks code files"
RA_RESEARCH_CODE_FILES_LABEL = "RA notebook read-only research code files"
RA_ANALYTICS_CODE_FILES_LABEL = "RA notebook read-only analytics code files"
RA_SCRIPTS_CODE_FILES_LABEL = "RA notebook read-only scripts code files"

FORBIDDEN_IMPORT_PREFIXES = (
    "nautilus_trader.live",
    "nautilus_trader.execution",
)
FORBIDDEN_CALL_NAMES = {
    "cancel_all_orders",
    "cancel_order",
    "delete_parameter",
    "deposit",
    "modify_order",
    "put_parameter",
    "submit_order",
    "submit_order_list",
    "transfer",
    "withdraw",
}


@dataclass(frozen=True)
class Finding:
    path: Path
    line: int
    label: str

    def message(self, root: Path) -> str:
        rel = self.path.relative_to(root).as_posix()
        return f"{rel}:{self.line}: forbidden RA notebook mutation path {self.label}"


def tree_python_files(directory: Path) -> list[Path] | None:
    if not directory.is_dir():
        return None
    return sorted(
        path
        for suffix in PYTHON_SUFFIXES
        for path in directory.rglob(f"*{suffix}")
        if path.is_file()
    )


def script_python_files(directory: Path) -> list[Path] | None:
    if not directory.is_dir():
        return None
    return sorted(
        path
        for suffix in PYTHON_SUFFIXES
        for path in directory.glob(f"*{suffix}")
        if path.is_file()
        and path.name.startswith(RA_SCRIPT_PREFIXES)
        and not path.name.startswith(("test_", "verify_"))
    )


def research_code_files(root: Path, findings: list[str] | None = None) -> list[Path]:
    paths: set[Path] = set()

    notebooks = tree_python_files(root / "notebooks")
    if findings is None:
        if notebooks is not None:
            paths.update(notebooks)
    elif notebooks is not None and require_nonempty(notebooks, RA_NOTEBOOKS_CODE_FILES_LABEL, findings):
        paths.update(notebooks)

    research = tree_python_files(root / "research")
    if findings is None:
        if research is not None:
            paths.update(research)
    elif research is not None and require_nonempty(research, RA_RESEARCH_CODE_FILES_LABEL, findings):
        paths.update(research)

    analytics = tree_python_files(root / "analytics")
    if findings is None:
        if analytics is not None:
            paths.update(analytics)
    elif analytics is not None and require_nonempty(analytics, RA_ANALYTICS_CODE_FILES_LABEL, findings):
        paths.update(analytics)

    scripts = script_python_files(root / "scripts")
    if findings is None:
        if scripts is not None:
            paths.update(scripts)
    elif scripts is None:
        findings.append(f"{RA_SCRIPTS_CODE_FILES_LABEL}: configured source path scripts is not present")
    elif require_nonempty(scripts, RA_SCRIPTS_CODE_FILES_LABEL, findings):
        paths.update(scripts)
    return sorted(paths)


def forbidden_import(name: str) -> str | None:
    for prefix in FORBIDDEN_IMPORT_PREFIXES:
        if name == prefix or name.startswith(f"{prefix}."):
            return prefix
    return None


def call_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
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
                if module := forbidden_import(alias.name):
                    findings.append(Finding(path, node.lineno, module))
        elif isinstance(node, ast.ImportFrom):
            if module := forbidden_import(node.module or ""):
                findings.append(Finding(path, node.lineno, module))
        elif isinstance(node, ast.Call):
            name = call_name(node.func)
            if name in FORBIDDEN_CALL_NAMES:
                findings.append(Finding(path, node.lineno, name))
    return findings


IMPORT_RE = re.compile(
    r"^\s*(?:import\s+(?P<import>\S+)|from\s+(?P<from>\S+)\s+import\s+)"
)
CALL_RE = re.compile(
    r"(?:^|[^\w.])(?P<name>"
    + "|".join(re.escape(name) for name in sorted(FORBIDDEN_CALL_NAMES))
    + r")\s*\("
)


def fallback_source_findings(path: Path, source: str) -> list[Finding]:
    findings: list[Finding] = []
    for line_number, raw_line in enumerate(source.splitlines(), start=1):
        line = raw_line.split("#", 1)[0]
        match = IMPORT_RE.match(line)
        if match is not None:
            imported = match.group("import")
            from_module = match.group("from")
            module_name = (imported or from_module or "").rstrip(",")
            if module := forbidden_import(module_name):
                findings.append(Finding(path, line_number, module))
                continue
        for call in CALL_RE.finditer(line):
            findings.append(Finding(path, line_number, call.group("name")))
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
    findings_text: list[str] = []
    paths = research_code_files(root, findings_text)
    if findings_text:
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
        print("FAIL: RA notebook read-only boundary violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA notebook read-only boundary passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
