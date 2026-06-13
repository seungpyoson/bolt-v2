#!/usr/bin/env python3
"""Verify binary-oracle strategy policy stays config-owned.

This gate covers Phase 9 hardcode-policy regressions in the production strategy
source. The strategy root may be a single file or a directory of `.rs` files;
it is resolved layout-independently through the shared gated-source-root
registry so the gate follows file moves. Each file's production text — comments
and `#[cfg(test)]` code excluded via the shared production-text helper — is
scanned individually so violations are reported against the file that actually
contains them.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass

from bolt_v3_source_roots import REPO_ROOT, STRATEGY_SOURCE_ROOTS, source_set_files
from verify_bolt_v3_pure_rust_runtime import production_text


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
    Rule(
        "strategy-local kill switch policy",
        re.compile(
            r"(?<![A-Za-z0-9_])KillSwitch[A-Za-z0-9_]*"
            r"|(?<![A-Za-z0-9_])kill_switch(?![A-Za-z0-9_])"
        ),
    ),
    Rule(
        "direct kill-switch action bypass",
        re.compile(
            r"(?<![A-Za-z0-9_])forced_reduction_submit(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])submit_forced_reduction(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])force_flatten(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])cancel_all_orders(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])flatten_all_positions(?![A-Za-z0-9_])"
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
    violations: list[Violation] = []
    for path in source_set_files(STRATEGY_SOURCE_ROOTS):
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(find_violations_in_text(rel, production_text(path)))
    return violations


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
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
