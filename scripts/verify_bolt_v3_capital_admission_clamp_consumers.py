#!/usr/bin/env python3
"""Verify capital-admission clamp consumers stay decision-safe.

The over-sell runtime feed clamps prediction-market position and allowance
underflows to zero. The only allowed trading decisions on those clamped fields
are the existing fail-closed reject paths. This source fence is intentionally
no-growth: a new production read of the clamped fields must be reviewed here
instead of silently becoming a fail-open consumer.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from verify_bolt_v3_pure_rust_runtime import production_text


REPO_ROOT = Path(__file__).resolve().parent.parent
CAPITAL_ADMISSION = "src/bolt_v3_capital_admission.rs"
RUNTIME_FEED = "src/bolt_v3_capital_admission_runtime_feed.rs"

CLAMPED_FIELD_PATTERN = re.compile(
    r"\b(?:yes_position|no_position|conditional_token_allowance)\b"
)

ALLOWED_FIELD_REFERENCES: tuple[tuple[str, re.Pattern[str], str], ...] = (
    (
        CAPITAL_ADMISSION,
        re.compile(r"pub\s+(?:yes_position|no_position|conditional_token_allowance):\s+Decimal,"),
        "snapshot field definition",
    ),
    (
        CAPITAL_ADMISSION,
        re.compile(r"snapshot\.conditional_token_allowance\s*<\s*request\.quantity"),
        "sell allowance reject path",
    ),
    (
        CAPITAL_ADMISSION,
        re.compile(r"snapshot\.(?:yes_position|no_position)\b"),
        "sell position reject path",
    ),
    (
        CAPITAL_ADMISSION,
        re.compile(r"(?:yes_position|no_position|conditional_token_allowance):\s+Decimal::"),
        "capital-admission fixture construction",
    ),
    (
        "src/bolt_v3_capital_admission_state.rs",
        re.compile(r"(?:yes_position|no_position|conditional_token_allowance):\s+Decimal::"),
        "capital-admission state fixture construction",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"(?:yes_position|no_position):\s+Decimal,"),
        "startup cache seed input",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"\b(?:yes_position|no_position),$"),
        "startup cache seed forwarding",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"snapshot\.(?:yes_position|no_position)\s*="),
        "startup cache seed write",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"&mut\s+snapshot\.(?:yes_position|no_position)\b"),
        "fill delta position update target",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"snapshot\.conditional_token_allowance\s*(?:\+=|=)"),
        "fill delta allowance update",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"let\s+allowance_before\s*=\s+snapshot\.conditional_token_allowance;"),
        "over-sell conservation observability read",
    ),
    (
        RUNTIME_FEED,
        re.compile(r"\.conditional_token_allowance$"),
        "fill delta allowance checked_sub receiver",
    ),
    (
        "src/bolt_v3_live_node.rs",
        re.compile(r"\b(?:yes_position|no_position)\b"),
        "NT startup position cache seed",
    ),
    (
        "src/bolt_v3_live_node.rs",
        re.compile(r"(?:yes_position|no_position|conditional_token_allowance):\s+Decimal::"),
        "live-node configured startup product construction",
    ),
    (
        "src/bolt_v3_submit_admission.rs",
        re.compile(r"product\.(?:yes_position|no_position)\b"),
        "existing fail-closed risk-reducing exit position consumer",
    ),
)


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    message: str


def _runtime_source_paths(root: Path = REPO_ROOT) -> tuple[str, ...]:
    return tuple(
        sorted(
            path.relative_to(root).as_posix()
            for path in (root / "src").rglob("*.rs")
            if path.is_file()
        )
    )


def _allowed(path: str, line: str) -> bool:
    return any(
        allowed_path == path and pattern.search(line)
        for allowed_path, pattern, _reason in ALLOWED_FIELD_REFERENCES
    )


def find_disallowed_field_references(
    source_by_path: dict[str, str],
) -> list[Violation]:
    violations: list[Violation] = []
    for path, text in sorted(source_by_path.items()):
        for index, line in enumerate(text.splitlines(), start=1):
            if not CLAMPED_FIELD_PATTERN.search(line):
                continue
            if _allowed(path, line):
                continue
            violations.append(
                Violation(
                    path,
                    index,
                    f"non-allowlisted clamped-field reference: {line.strip()}",
                )
            )
    return violations


def reject_path_violations(capital_admission_text: str) -> list[Violation]:
    violations: list[Violation] = []
    if "snapshot.conditional_token_allowance < request.quantity" not in capital_admission_text:
        violations.append(
            Violation(
                CAPITAL_ADMISSION,
                0,
                "sell allowance reject path must read conditional_token_allowance before permit",
            )
        )
    if "return Err(LiabilityError::InsufficientAllowance);" not in capital_admission_text:
        violations.append(
            Violation(
                CAPITAL_ADMISSION,
                0,
                "sell allowance reject path must fail closed with InsufficientAllowance",
            )
        )
    if "if outcome_position < request.quantity" not in capital_admission_text:
        violations.append(
            Violation(
                CAPITAL_ADMISSION,
                0,
                "sell position reject path must compare outcome_position before permit",
            )
        )
    if "return Err(LiabilityError::InsufficientInventory);" not in capital_admission_text:
        violations.append(
            Violation(
                CAPITAL_ADMISSION,
                0,
                "sell position reject path must fail closed with InsufficientInventory",
            )
        )
    return violations


def collect_violations(root: Path = REPO_ROOT) -> list[Violation]:
    source_by_path = {
        rel_path: production_text(root / rel_path) for rel_path in _runtime_source_paths(root)
    }
    violations = find_disallowed_field_references(source_by_path)
    violations.extend(reject_path_violations(source_by_path.get(CAPITAL_ADMISSION, "")))
    runtime_feed = source_by_path.get(RUNTIME_FEED, "")
    if "record_capital_admission_oversell_conservation_violation" not in runtime_feed:
        violations.append(
            Violation(
                RUNTIME_FEED,
                0,
                "over-sell underflow site must emit the conservation-violation metric/log helper",
            )
        )
    return violations


def main() -> int:
    findings = collect_violations()
    if findings:
        for finding in findings:
            location = finding.path if finding.line == 0 else f"{finding.path}:{finding.line}"
            print(f"{location}: {finding.message}", file=sys.stderr)
        return 1
    print("OK: capital-admission clamped-field consumers stay decision-safe")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
