#!/usr/bin/env python3
"""Verify Bolt-v3 one-way dependency direction.

The decomposition architecture contract
(`specs/522-decompose-strategy-monolith/architecture-contract.md` §2) requires a
one-way dependency: the strategy may use shared (`bolt_v3_*`) and family
(`bolt_v3_market_families/*`) modules, but those shared/family modules must NEVER
import the strategy layer (`crate::strategies`). The three existing fences
(naming, provider-leaks, core-boundary) do not catch a shared module doing
`use crate::strategies::...`; this fence does.

Current code already contains pre-existing back-references (e.g.
`crate::strategies::registry::FeeProvider` used by the providers, tracked under
#446). They are captured in `FINDING_ALLOWANCES` so the fence is GREEN on today's
code while FAILING on every NEW back-reference. The allowlist may only SHRINK:

- adding a new allowance is forbidden — a new back-reference is a bug to fix, not
  to allow;
- a stale allowance (one that no longer matches any source line) FAILS, forcing
  its removal once the underlying reference is relocated to a shared module.

The fence checks the import path, which is the mechanism a shared module would use
to reach the strategy. Like the sibling fences, it does not attempt to resolve
aliased re-exports.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

# Shared/family layer = everything under `src/bolt_v3_*` (top-level files and
# their subdirectories, including `bolt_v3_market_families/`). The strategy layer
# (`src/strategies/**`) is intentionally NOT scanned — it MAY use shared/family.
SCAN_PREFIX = "src/bolt_v3_"

# `crate::strategies` with optional rustfmt-compatible spacing around `::`.
STRATEGY_IMPORT = re.compile(r"crate\s*::\s*strategies\b")

MESSAGE = (
    "shared/family module imports the strategy layer (crate::strategies); "
    "violates one-way dependency (contract §2)"
)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    excerpt: str

    def render(self, prefix: str) -> str:
        return f"{prefix}: {self.path}:{self.line}: {MESSAGE}: {self.excerpt}"


@dataclass(frozen=True)
class FindingAllowance:
    """A FROZEN pre-existing back-reference.

    `exact_excerpt` must match the comment-stripped, stripped source line produced
    by `excerpt_for`. The allowlist may only shrink — never add entries.
    """

    path: str
    exact_excerpt: str


# Pre-existing strategy back-references in the shared/family layer, frozen at the
# start of the #522 decomposition (verified against origin/main: exactly one).
# Each entry is removed when its underlying reference is relocated to a shared
# module. DO NOT ADD ENTRIES — a new back-reference is a bug to fix, not to allow.
FINDING_ALLOWANCES: tuple[FindingAllowance, ...] = (
    # The shared FeeProvider trait lives in `strategies::registry`; the Polymarket
    # fee provider implements it. Relocating that trait to a shared module is
    # tracked under #446 — when done, delete this allowance.
    FindingAllowance(
        path="src/bolt_v3_providers/polymarket/fees.rs",
        exact_excerpt="use crate::strategies::registry::FeeProvider;",
    ),
)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def strip_line_comment(line: str) -> str:
    """Drop a trailing `//` line comment so commented-out references don't trip the
    fence. A `://` sequence (e.g. a URL) is preserved."""

    idx = 0
    while True:
        idx = line.find("//", idx)
        if idx == -1:
            return line
        if idx > 0 and line[idx - 1] == ":":
            idx += 2
            continue
        return line[:idx]


def excerpt_for(line: str) -> str:
    return strip_line_comment(line).strip()


def scan_files(root: Path) -> tuple[Path, ...]:
    src = root / "src"
    if not src.exists():
        return ()
    return tuple(
        sorted(
            path
            for path in src.rglob("*.rs")
            if path.is_file()
            and path.relative_to(root).as_posix().startswith(SCAN_PREFIX)
        )
    )


def find_violations(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in scan_files(root):
        rel = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8")
        for lineno, raw_line in enumerate(text.splitlines(), start=1):
            code = strip_line_comment(raw_line)
            if STRATEGY_IMPORT.search(code):
                findings.append(Finding(path=rel, line=lineno, excerpt=code.strip()))
    return findings


def is_allowed(finding: Finding) -> bool:
    return any(
        allowance.path == finding.path
        and allowance.exact_excerpt == finding.excerpt
        for allowance in FINDING_ALLOWANCES
    )


def main() -> int:
    findings = find_violations(REPO_ROOT)

    matched: set[tuple[str, str]] = set()
    real: list[Finding] = []
    for finding in findings:
        if is_allowed(finding):
            matched.add((finding.path, finding.excerpt))
        else:
            real.append(finding)

    stale = [
        allowance
        for allowance in FINDING_ALLOWANCES
        if (allowance.path, allowance.exact_excerpt) not in matched
    ]

    failed = False
    for finding in real:
        print(finding.render("FAIL"), file=sys.stderr)
        failed = True
    for allowance in stale:
        print(
            f"FAIL: {allowance.path}: stale allowance no longer matches any source "
            f"line; remove it (allowlist may only shrink): {allowance.exact_excerpt}",
            file=sys.stderr,
        )
        failed = True

    if failed:
        return 1

    print(
        "OK: Bolt-v3 dependency-direction verifier passed "
        f"({len(FINDING_ALLOWANCES)} frozen pre-existing back-reference(s))."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
