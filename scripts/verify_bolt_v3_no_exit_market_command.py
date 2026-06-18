#!/usr/bin/env python3
"""Verify Bolt does not wire NautilusTrader venue-mutating control paths.

Shadow mode forbids venue mutation under `runtime.order_execution_mode=shadow`. The NT
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
    production_text,
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
            r"(?P<api>market_exit_strategy|submit_order_list|close_all_positions|cancel_all_orders|close_position|cancel_orders|modify_order|exit_market|market_exit)"
            r"(?![A-Za-z0-9_])"
        ),
    ),
)

# The shared execution-policy module IS Bolt's venue-mutation chokepoint: every
# call here is routed through the shadow-mode `BoltV3OrderExecutionPolicy` gate
# (Live -> NT, Shadow -> suppressed), so a venue-mutating lifecycle API used here
# is enforced BY shadow mode, not a bypass of it. Only the specific APIs Bolt
# routes through that gate are exempt, and only in this one file; every other
# forbidden API (close_position, market_exit, ...) still fails even here, and any
# of these APIs anywhere else still fails. Keyed by the routed-API name so adding
# a new routed mutation (e.g. modify_order alongside cancel_all_orders) is an
# explicit one-line allowlist entry, never a blanket file exemption.
CHOKEPOINT_POLICY_PATH = "src/bolt_v3_order_execution.rs"
ALLOWED_CHOKEPOINT_APIS = frozenset(
    {
        "cancel_all_orders",
        "modify_order",
    }
)


def is_routed_chokepoint_api(api: str) -> bool:
    """Return True only for an EXACT routed-chokepoint API name.

    The decision is exact set membership, never a substring test. A name such as
    `force_modify_order` or `cancel_all_orders_bypass` embeds an allowed name as a
    substring but is a DIFFERENT, unrouted API; this returns False for it so the
    chokepoint exemption stays per-API.

    Scope of what this guards: this exact-match form is FORWARD-PROOFING of the
    chokepoint-exemption contract, not a fix for a currently-reachable bypass. In
    `find_violations_in_text`, `match.group("api")` is supplied by the forbidden
    lifecycle regex, whose `(?:\\.|::)` prefix and `(?![A-Za-z0-9_])` suffix make the
    capture ALWAYS exactly one of the listed API tokens — an impostor name like
    `force_modify_order` can never reach this function through the real pipeline
    (the regex boundary already rejects it; that path is covered by the
    substring/comment boundary test). So a prior substring form was not exploitable
    via the pipeline. What this function adds is a directly unit-tested,
    self-contained exemption contract: `is_routed_chokepoint_api` is asserted on its
    own (the impostor cases below document and forward-proof the contract), so the
    chokepoint allowlist can't silently widen to a near-miss name if the regex ever
    changes.
    """
    return api in ALLOWED_CHOKEPOINT_APIS


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            if (
                rule.label == "NT venue-mutating lifecycle API"
                and path == CHOKEPOINT_POLICY_PATH
                and is_routed_chokepoint_api(match.group("api"))
            ):
                continue
            line_start = text.rfind("\n", 0, match.start()) + 1
            line_end = text.find("\n", match.end())
            if line_end == -1:
                line_end = len(text)
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    label=rule.label,
                    excerpt=text[line_start:line_end].strip(),
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
        violations.extend(find_violations_in_text(rel, production_text(path)))
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
