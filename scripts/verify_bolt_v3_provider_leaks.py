#!/usr/bin/env python3
"""Audit Bolt-v3 provider-specific leakage in core-adjacent boundaries.

Provider-specific NT types, provider-key literals, and concrete provider
dispatch belong in `src/bolt_v3_providers/<provider>.rs`, not in the core
Bolt-v3 assembly files. This verifier is intentionally strict over production
code and ignores comments plus `#[cfg(test)] mod tests` fixtures.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    message: str
    excerpt: str

    def render(self, prefix: str) -> str:
        return f"{prefix}: {self.path}:{self.line}: {self.message}: {self.excerpt}"


@dataclass(frozen=True)
class Rule:
    path: str
    pattern: re.Pattern[str]
    message: str


def rules_for(
    paths: tuple[str, ...],
    pattern: re.Pattern[str],
    message: str,
) -> list[Rule]:
    return [Rule(path, pattern, message) for path in paths]


def discovered_core_files(root: Path) -> tuple[str, ...]:
    """Return core Bolt-v3 files; binding modules are intentionally excluded."""

    src = root / "src"
    paths = set(src.glob("bolt_v3_*.rs"))
    paths.update(
        path
        for path in (
            src / "bolt_v3_archetypes" / "mod.rs",
            src / "bolt_v3_market_families" / "mod.rs",
            src / "bolt_v3_providers" / "mod.rs",
        )
        if path.exists()
    )
    return tuple(sorted(path.relative_to(root).as_posix() for path in paths))


def discovered_binding_names(root: Path, directory: str) -> tuple[str, ...]:
    binding_dir = root / "src" / directory
    if not binding_dir.exists():
        return ()

    names = {path.stem for path in binding_dir.glob("*.rs") if path.name != "mod.rs"}
    names.update(path.name for path in binding_dir.iterdir() if path.is_dir())
    return tuple(sorted(names))


def snake_to_pascal(name: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in name.split("_") if part)


def alternation(names: tuple[str, ...]) -> str:
    if not names:
        return r"(?!)"
    return "|".join(re.escape(name) for name in names)


def rules_for_root(root: Path) -> list[Rule]:
    core_files = discovered_core_files(root)
    provider_names = discovered_binding_names(root, "bolt_v3_providers")
    family_names = discovered_binding_names(root, "bolt_v3_market_families")
    provider_alt = alternation(provider_names)
    provider_type_alt = alternation(tuple(snake_to_pascal(name) for name in provider_names))
    family_alt = alternation(family_names)

    return [
        *rules_for(
            core_files,
            re.compile(rf"\bnautilus_(?:{provider_alt})::"),
            "concrete NT provider crate in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\b[A-Za-z0-9_]*(?:{provider_type_alt})[A-Za-z0-9_]*\b"),
            "concrete provider type name in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(
                rf"\b(?:pub\s+use|use)\s+crate::bolt_v3_providers::[^;]*"
                rf"\b(?:{provider_alt})\b",
                re.DOTALL,
            ),
            "core imports or re-exports concrete provider module",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\bbolt_v3_providers::(?:{provider_alt})::"),
            "core accesses concrete provider module path",
        ),
        *rules_for(
            core_files,
            re.compile(rf'"(?:{provider_alt})"'),
            "provider-key string literal in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(rf'"(?:{family_alt})"'),
            "market-family key string literal in core production code",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(
                rf"\benum\s+BoltV3VenueAdapterConfig\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed provider adapter config enum",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(
                rf"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_alt})::KEY\b",
                re.DOTALL,
            ),
            "adapter mapping dispatches on concrete provider key",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(r"\bMarketSlugFilter\b"),
            "provider-specific NT filter in adapter mapper",
        ),
        Rule(
            "src/bolt_v3_secrets.rs",
            re.compile(
                rf"\benum\s+ResolvedBoltV3VenueSecrets\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed resolved venue secret enum",
        ),
        Rule(
            "src/bolt_v3_secrets.rs",
            re.compile(
                rf"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_alt})::KEY\b",
                re.DOTALL,
            ),
            "secret resolution dispatches on concrete provider key",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(rf"\buse\s+nautilus_(?:{provider_alt})::factories\b"),
            "concrete NT provider factory import",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(
                rf"\benum\s+BoltV3RegisteredVenue\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed registered venue summary enum",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(rf"\bBoltV3VenueAdapterConfig::(?:{provider_type_alt})\b"),
            "client registration dispatches on concrete adapter variant",
        ),
    ]


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def excerpt_for(text: str, pos: int) -> str:
    line_start = text.rfind("\n", 0, pos) + 1
    line_end = text.find("\n", pos)
    if line_end == -1:
        line_end = len(text)
    return " ".join(text[line_start:line_end].strip().split())


def is_cfg_test_attr(stripped: str) -> bool:
    return stripped.startswith("#[") and "cfg" in stripped and "test" in stripped


def brace_delta(line: str) -> int:
    return line.count("{") - line.count("}")


def production_text(text: str) -> str:
    """Return a scan view that excludes comments and inline test modules.

    The verifier targets production architecture leakage. Comment prose and
    `#[cfg(test)]` fixtures are useful context, but they are not the
    core surface that decides whether adding a provider requires runtime edits.
    """

    lines = text.splitlines()
    output: list[str] = []
    cfg_test_depth: int | None = None
    pending_cfg_test = False

    for line in lines:
        stripped = line.lstrip()
        if cfg_test_depth is not None:
            output.append("")
            cfg_test_depth += brace_delta(line)
            if cfg_test_depth <= 0:
                cfg_test_depth = None
            continue

        if is_cfg_test_attr(stripped):
            pending_cfg_test = True
            output.append("")
            continue

        if pending_cfg_test and (not stripped or stripped.startswith("//")):
            output.append("")
            continue

        if pending_cfg_test and "{" in stripped:
            cfg_test_depth = brace_delta(line)
            if cfg_test_depth <= 0:
                cfg_test_depth = None
            output.append("")
            continue

        pending_cfg_test = False
        if stripped.startswith("//"):
            output.append("")
        else:
            output.append(line)

    trailing_newline = "\n" if text.endswith("\n") else ""
    return "\n".join(output) + trailing_newline


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for rule in rules_for_root(root):
        path = root / rule.path
        if not path.exists():
            continue
        text = production_text(path.read_text(encoding="utf-8"))
        for match in rule.pattern.finditer(text):
            findings.append(
                Finding(
                    path=rule.path,
                    line=line_number(text, match.start()),
                    message=rule.message,
                    excerpt=excerpt_for(text, match.start()),
                )
            )
    return findings


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to scan; defaults to this checkout",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    findings = scan_root(args.root)

    if findings:
        for finding in findings:
            print(finding.render("FAIL"), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 provider-leak verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
