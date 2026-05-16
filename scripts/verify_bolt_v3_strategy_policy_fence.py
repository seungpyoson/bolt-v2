#!/usr/bin/env python3
"""Verify binary-oracle strategy policy stays config-owned.

This gate covers Phase 9 hardcode-policy regressions in the production
strategy file. It ignores comments and `#[cfg(test)]` code through the shared
production-text helper.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from verify_bolt_v3_pure_rust_runtime import production_text


REPO_ROOT = Path(__file__).resolve().parent.parent
STRATEGY_PATH = "src/strategies/binary_oracle_edge_taker.rs"


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


FORBIDDEN_RULES: tuple[Rule, ...] = (
    Rule(
        "dead runtime-selection bus path",
        re.compile(
            r"\bruntime_selection_topic\b"
            r"|(?<![A-Za-z0-9_])platform\.runtime\.selection(?![A-Za-z0-9_])"
            r"|\bsubscribe_any\b"
            r"|\btry_get_actor_unchecked\b"
        ),
    ),
    Rule(
        "inline updown NT metadata interpretation",
        re.compile(r'"market_slug"|\"market_id\"|\"Up\"|\"Down\"'),
    ),
    Rule(
        "fixed long-only position contract tuple",
        re.compile(
            r"OrderSide::Buy,\s*"
            r"PositionSide::Long,\s*"
            r"OrderSide::Sell,\s*"
            r"PositionSide::Long,",
            re.MULTILINE,
        ),
    ),
    Rule(
        "buy-only entry VWAP helper",
        re.compile(r"\bmax_buy_execution_within_vwap_slippage_bps\b"),
    ),
    Rule(
        "buy-biased entry price block",
        re.compile(
            r"OutcomeSide::Up\s*=>\s*self\.active\.books\.up\.best_ask,\s*"
            r"OutcomeSide::Down\s*=>\s*self\.active\.books\.down\.best_ask,",
            re.MULTILINE,
        ),
    ),
)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    violations: list[Violation] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(text):
            line_start = text.rfind("\n", 0, match.start()) + 1
            line_end = text.find("\n", match.end())
            if line_end == -1:
                line_end = len(text)
            violations.append(
                Violation(
                    path=path,
                    line=line_number(text, match.start()),
                    label=rule.label,
                    excerpt=text[line_start:line_end].strip(),
                )
            )
    return violations


def collect_violations() -> list[Violation]:
    path = REPO_ROOT / STRATEGY_PATH
    return find_violations_in_text(STRATEGY_PATH, production_text(path))


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 strategy policy hardcode "
                f"{violation.label} at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 strategy policy fence passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
