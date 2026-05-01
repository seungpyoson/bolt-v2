#!/usr/bin/env python3
"""Audit Bolt-v3 provider-specific leakage in core-adjacent boundaries.

This verifier is intentionally not wired into `just fmt-check` yet. The current
Bolt-v3 baseline is expected to fail strict mode because adapter mapping,
resolved secrets, and client registration still encode concrete providers.
Use `--audit-current` to print those findings without failing while the
provider-boundary refactor is still pending.
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


RULES = [
    Rule(
        "src/bolt_v3_adapters.rs",
        re.compile(
            r"\benum\s+BoltV3VenueAdapterConfig\s*\{(?P<body>[^}]*)\b(?:Polymarket|Binance)\b",
            re.DOTALL,
        ),
        "closed provider adapter config enum",
    ),
    Rule(
        "src/bolt_v3_adapters.rs",
        re.compile(
            r"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{(?P<body>[^}]*)\b(?:polymarket|binance)::KEY\b",
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
            r"\benum\s+ResolvedBoltV3VenueSecrets\s*\{(?P<body>[^}]*)\b(?:Polymarket|Binance)\b",
            re.DOTALL,
        ),
        "closed resolved venue secret enum",
    ),
    Rule(
        "src/bolt_v3_secrets.rs",
        re.compile(
            r"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{(?P<body>[^}]*)\b(?:polymarket|binance)::KEY\b",
            re.DOTALL,
        ),
        "secret resolution dispatches on concrete provider key",
    ),
    Rule(
        "src/bolt_v3_client_registration.rs",
        re.compile(r"\buse\s+nautilus_(?:polymarket|binance)::factories\b"),
        "concrete NT provider factory import",
    ),
    Rule(
        "src/bolt_v3_client_registration.rs",
        re.compile(
            r"\benum\s+BoltV3RegisteredVenue\s*\{(?P<body>[^}]*)\b(?:Polymarket|Binance)\b",
            re.DOTALL,
        ),
        "closed registered venue summary enum",
    ),
    Rule(
        "src/bolt_v3_client_registration.rs",
        re.compile(r"\bBoltV3VenueAdapterConfig::(?:Polymarket|Binance)\b"),
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


def production_text(text: str) -> str:
    """Return a scan view that excludes comments and inline test modules.

    The verifier targets production architecture leakage. Comment prose and
    `#[cfg(test)] mod tests` fixtures are useful context, but they are not the
    core surface that decides whether adding a provider requires runtime edits.
    """

    lines = text.splitlines()
    output: list[str] = []
    in_cfg_test_tail = False
    pending_cfg_test = False

    for line in lines:
        stripped = line.lstrip()
        if in_cfg_test_tail:
            output.append("")
            continue

        if stripped.startswith("#[cfg(test)]"):
            pending_cfg_test = True
            output.append("")
            continue

        if pending_cfg_test and re.match(r"(?:pub\s+)?mod\s+\w+\s*\{", stripped):
            in_cfg_test_tail = True
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
    for rule in RULES:
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
    parser.add_argument(
        "--audit-current",
        action="store_true",
        help="print findings but exit 0; use while current baseline is expected to violate",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    findings = scan_root(args.root)

    if args.audit_current:
        if findings:
            for finding in findings:
                print(finding.render("AUDIT"))
        else:
            print("OK: no Bolt-v3 provider leaks found.")
        return 0

    if findings:
        for finding in findings:
            print(finding.render("FAIL"), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 provider-leak verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
