#!/usr/bin/env python3
"""Verify Bolt does not wire NautilusTrader venue-mutating control paths.

Shadow mode forbids venue mutation under `submit_orders=false`. The NT
`StrategyCommand::ExitMarket` control endpoint can route to `market_exit()`,
which is outside Bolt's submit/cancel chokepoints, so Bolt production source
must not wire a sender for that command or call equivalent market-exit/close
APIs directly. The fence also rejects other NT venue-mutating APIs that would
bypass Bolt's shadow-mode submit/cancel chokepoints.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import (
    strip_cfg_test_items,
    strip_rust_comments_and_literals,
)


@dataclass(frozen=True)
class Rule:
    label: str
    pattern: re.Pattern[str]


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    label: str
    excerpt: str


FORBIDDEN_RULES = (
    Rule(
        "NT ExitMarket command sender",
        re.compile(r"(?<![A-Za-z0-9_])ExitMarket(?![A-Za-z0-9_])"),
    ),
    Rule(
        "NT venue-mutating lifecycle API",
        re.compile(
            r"(?:\.|::)\s*(?:r#)?"
            r"(?:market_exit_strategy|submit_order_list|close_all_positions|cancel_all_orders|close_position|cancel_orders|modify_order|exit_market|market_exit)"
            r"(?![A-Za-z0-9_])"
        ),
    ),
)


@dataclass(frozen=True)
class SanctionedSite:
    """A single, explicitly authorized venue-mutating call site.

    The fence is otherwise total: no Bolt production source may wire an NT
    venue-mutating control path. The loss-governor halt handler is the one
    spec-sanctioned exception (specs/505-nt-loss-governor FR-017: active
    loss-halt exits MUST dispatch through `Trader::market_exit_strategy`).
    To stay narrow, a call is sanctioned only when ALL of: the file path
    matches; the matched API is exactly `api`; AND the marker comment appears
    on the call line or the line immediately above it in the original
    (un-stripped) source. This neither disables the rule globally nor
    allowlists the file wholesale — it pins the exception to one annotated
    call to one specific API.
    """

    path: str
    api: str
    marker: str


SANCTIONED_SITES = (
    SanctionedSite(
        path="src/bolt_v3_live_node.rs",
        api="market_exit_strategy",
        marker="FR-017 sanctioned: sole NT market-exit dispatch",
    ),
)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def is_sanctioned(
    path: str, matched: str, raw_lines: list[str], line_no: int
) -> bool:
    """True when this match is the one spec-sanctioned market-exit dispatch.

    The marker comment is blanked from the scan view, so it is read from the
    raw source by LINE NUMBER (the cfg-test and comment strippers both preserve
    newline counts, so the scan line number aligns with the raw line number,
    even though byte offsets do not). Sanctioning requires the file path, the
    matched API token, and a marker on the matched call line or the line
    immediately above it (the conventional annotation position).
    `line_no` is 1-based.
    """
    idx = line_no - 1
    if idx < 0 or idx >= len(raw_lines):
        return False
    call_line = raw_lines[idx]
    prev_line = raw_lines[idx - 1] if idx - 1 >= 0 else ""
    for site in SANCTIONED_SITES:
        if path != site.path:
            continue
        if site.api not in matched:
            continue
        if site.marker in call_line or site.marker in prev_line:
            return True
    return False


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    # `scan_text` excludes `#[cfg(test)]` items AND blanks comments/literals so
    # the forbidden-API regex matches only real production calls. The sanction
    # marker is a COMMENT, so it is read from the RAW `text` by line number. The
    # cfg-test and comment strippers both preserve newline counts, so scan line
    # numbers align with raw line numbers (byte offsets do not, since blanked
    # content is shortened).
    scan_text = strip_rust_comments_and_literals(strip_cfg_test_items(text))
    raw_lines = text.split("\n")
    violations: list[Violation] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            line_no = line_number(scan_text, match.start())
            if is_sanctioned(path, match.group(0), raw_lines, line_no):
                continue
            excerpt = (
                raw_lines[line_no - 1].strip()
                if 1 <= line_no <= len(raw_lines)
                else ""
            )
            violations.append(
                Violation(
                    path=path,
                    line=line_no,
                    label=rule.label,
                    excerpt=excerpt,
                )
            )
    return violations


def bolt_src_files() -> list[Path]:
    src_root = REPO_ROOT / "src"
    files: list[Path] = []
    for path in src_root.rglob("*.rs"):
        if path.is_symlink():
            raise ValueError(f"src contains a symlink: {path}")
        if path.is_file():
            files.append(path)
    files.sort(key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8"))
    return files


def collect_violations_from_files(files: list[Path]) -> list[Violation]:
    if not files:
        raise RuntimeError("no Rust source files found under src")

    violations: list[Violation] = []
    for path in files:
        try:
            rel = str(path.relative_to(REPO_ROOT))
        except ValueError:
            rel = str(path)
        # Pass RAW source: `find_violations_in_text` owns all stripping so the
        # sanction marker (a comment) survives for lookup before the scan view
        # blanks comments.
        violations.extend(
            find_violations_in_text(rel, path.read_text(encoding="utf-8"))
        )
    return violations


def collect_violations() -> list[Violation]:
    return collect_violations_from_files(bolt_src_files())


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 NT ExitMarket command fence "
                f"{violation.label} at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 NT ExitMarket command fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
