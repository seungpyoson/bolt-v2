#!/usr/bin/env python3
"""Verify Bolt-v3 strategy policy stays config-owned.

This gate covers strategy-local hardcode-policy regressions in the registered
strategy source roots and known direct NT venue-mutation bypass forms across
production `src/**/*.rs` files. It is a CI guardrail for reviewed source, not a
complete firewall over every public NautilusTrader transport API. Each file's
production text — comments and `#[cfg(test)]` code excluded via the shared
production-text helper — is scanned individually so violations are reported
against the file that actually contains them. Code-construct rules are matched
against a code-only view with
comments and string literals blanked, so naming a banned token inside an error
message or doc string is not a violation; only rules that deliberately target
string-literal content opt into scanning the original text.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass

from bolt_v3_source_roots import (
    ALL_GATED_SOURCE_ROOTS,
    MAKER_SOURCE_ROOT,
    REPO_ROOT,
    STRATEGY_SOURCE_ROOTS,
    source_set_files,
)
from verify_bolt_v3_pure_rust_runtime import (
    production_text,
    strip_rust_comments_and_literals,
)
from verifier_io import require_nonempty


@dataclass(frozen=True)
class Rule:
    label: str
    pattern: re.Pattern[str]
    # When False (default) the rule bans a *code* construct and is matched against
    # code only — comments and string literals are blanked first, so naming a banned
    # token inside an error message or doc string is not a violation. When True the
    # rule deliberately targets string-literal *content* (e.g. hardcoded NT metadata)
    # and is matched against the original text.
    scan_literals: bool = False


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    label: str
    excerpt: str


NT_VENUE_MUTATION_METHOD_NAMES: tuple[str, ...] = (
    # Raw StrategyCore / OrderManager command transport is the deepest NT
    # mutation path underneath the Strategy helper methods.
    "core_mut",
    "order_manager",
    "send_risk_command",
    "send_exec_command",
    "send_emulator_command",
    "send_algo_command",
    # Common raw msgbus command transport helpers underneath OrderManager.
    "send_trading_command",
    "send_any",
    "send_any_value",
    "risk_engine_queue_execute",
    "exec_engine_queue_execute",
    "emulator_queue_execute",
    "algo_engine_queue_execute",
    # Current pinned NautilusTrader Strategy venue-mutation methods.
    "submit_order",
    "submit_order_list",
    "modify_order",
    "cancel_order",
    "cancel_orders",
    "cancel_all_orders",
    "close_position",
    "close_all_positions",
    # Private Bolt wrapper names stay fenced everywhere outside the policy module.
    "submit_order_via_nt",
    "cancel_order_via_nt",
    "cancel_all_orders_via_nt",
    # Near-neighbor variants are fenced before a future NT bump can use them.
    "submit_order_with_params",
    "submit_order_list_with_params",
    "modify_order_with_params",
    "cancel_order_with_params",
    "cancel_orders_with_params",
    "cancel_all_orders_with_params",
    "modify_order_in_place",
    # NT-managed lifecycle helpers that can transitively submit/cancel through
    # StrategyCore or OrderManager.
    "expire_gtd_order",
    "reactivate_gtd_timers",
    "set_gtd_expiry",
    "cancel_gtd_expiry",
    "finalize_market_exit",
    "cancel_market_exit",
    "deny_order",
    "deny_order_list",
)

NT_VENUE_MUTATION_BARE_NAMES: tuple[str, ...] = (
    "send_trading_command",
    "send_any",
    "send_any_value",
    "risk_engine_queue_execute",
    "exec_engine_queue_execute",
    "emulator_queue_execute",
    "algo_engine_queue_execute",
)

NT_TRADING_COMMAND_SURFACE_NAMES: tuple[str, ...] = (
    "TradingCommand",
    "SubmitOrder",
    "SubmitOrderList",
    "ModifyOrder",
    "CancelOrder",
    "CancelOrders",
    "CancelAllOrders",
    "ClosePosition",
    "CloseAllPositions",
    "DenyOrder",
    "DenyOrderList",
)

NT_VENUE_MUTATION_METHOD_PATTERN = "|".join(
    re.escape(name)
    for name in sorted(NT_VENUE_MUTATION_METHOD_NAMES, key=len, reverse=True)
)
NT_VENUE_MUTATION_BARE_PATTERN = "|".join(
    re.escape(name)
    for name in sorted(NT_VENUE_MUTATION_BARE_NAMES, key=len, reverse=True)
)
NT_TRADING_COMMAND_SURFACE_PATTERN = "|".join(
    re.escape(name)
    for name in sorted(NT_TRADING_COMMAND_SURFACE_NAMES, key=len, reverse=True)
)

DIRECT_NT_VENUE_MUTATION_RULE = Rule(
    "direct NT venue mutation call",
    re.compile(
        r"(?:\.\s*|(?<![A-Za-z0-9_])"
        r"(?:Self|[A-Za-z_][A-Za-z0-9_]*"
        r"(?:::[A-Za-z_][A-Za-z0-9_]*)*)\s*::\s*|<[^>\n]+>\s*::\s*)"
        rf"(?:{NT_VENUE_MUTATION_METHOD_PATTERN})"
        r"(?=\s*(?:::<|\(|;|,|\)|$))"
    ),
)

RAW_MSGBUS_NT_VENUE_MUTATION_RULE = Rule(
    "direct NT venue mutation call",
    re.compile(
        r"(?<![A-Za-z0-9_:])"
        rf"(?:{NT_VENUE_MUTATION_BARE_PATTERN})"
        r"(?=\s*(?:::<|\(|;|,|\)|$))"
        r"|(?<![A-Za-z0-9_:])"
        rf"(?:{NT_TRADING_COMMAND_SURFACE_PATTERN})"
        r"(?=\s*(?:::|<|\(|;|,|:|\)|$))"
    ),
)

STRATEGY_POLICY_RULES: tuple[Rule, ...] = (
    Rule(
        "dead runtime-selection bus path",
        re.compile(
            r"\bruntime_selection_topic\b"
            r"|(?<![A-Za-z0-9_])platform\.runtime\.selection(?![A-Za-z0-9_])"
            r"|\bsubscribe_any\b"
            r"|\btry_get_actor_unchecked\b"
        ),
        # The dead bus path may appear as a hardcoded topic *string*
        # (e.g. "platform.runtime.selection"); scan the original so a string
        # topic is still caught, not only the code symbols.
        scan_literals=True,
    ),
    Rule(
        "inline updown NT metadata interpretation",
        re.compile(r'"market_slug"|\"market_id\"|\"Up\"|\"Down\"'),
        scan_literals=True,
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
            r"|(?<![A-Za-z0-9_])cancel_orders(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])cancel_all_orders(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])close_position(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])close_all_positions(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])flatten_all_positions(?![A-Za-z0-9_])"
        ),
    ),
    Rule(
        "registered strategy outside strategy module tree",
        re.compile(
            r"\bregistry\.register\s*::\s*<\s*crate::(?!strategies::)"
            r"|\bregistry\.register\s*<\s*crate::(?!strategies::)"
            r"|\bregistry\.register\s*::\s*<\s*(?:super::)+"
            r"|\bregistry\.register\s*<\s*(?:super::)+"
        ),
    ),
    Rule(
        "maker dependency on taker pricing internals",
        re.compile(
            r"\bcrate\s*::\s*bolt_v3_taker_pricing\b"
            r"|\b(?:super\s*::\s*)+bolt_v3_taker_pricing\b"
            r"|\bbolt_v3_taker_pricing\s*::"
            r"|\bTakerPricing[A-Za-z0-9_]*\b"
        ),
    ),
    Rule(
        "global kill-switch cancel supervisor policy",
        re.compile(
            r"(?<![A-Za-z0-9_])bolt_v3_kill_switch_cancel(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])BoltV3KillSwitchCancel[A-Za-z0-9_]*"
            r"|(?<![A-Za-z0-9_])cancel_supervisor(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])plan_cancel(?![A-Za-z0-9_])"
        ),
    ),
    Rule(
        "global kill-switch flatten supervisor policy",
        re.compile(
            r"(?<![A-Za-z0-9_])bolt_v3_kill_switch_flatten(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])BoltV3KillSwitchFlatten[A-Za-z0-9_]*"
            r"|(?<![A-Za-z0-9_])flatten_supervisor(?![A-Za-z0-9_])"
            r"|(?<![A-Za-z0-9_])plan_flatten(?![A-Za-z0-9_])"
        ),
    ),
)

EXECUTION_POLICY_RULES: tuple[Rule, ...] = (
    Rule(
        "strategy-local execution policy construction",
        re.compile(
            r"\bBoltV3OrderExecutionPolicy\s*::\s*(?:live|shadow|from_mode)\s*\("
        ),
    ),
    Rule(
        "strategy-local execution policy type reference",
        re.compile(r"\bBoltV3OrderExecution(?:Policy|Mode)\b"),
    ),
    Rule(
        "strategy-local execution policy override",
        re.compile(r"\.with_order_execution_policy\s*\("),
    ),
)

FORBIDDEN_RULES: tuple[Rule, ...] = (
    *STRATEGY_POLICY_RULES,
    *EXECUTION_POLICY_RULES,
    DIRECT_NT_VENUE_MUTATION_RULE,
    RAW_MSGBUS_NT_VENUE_MUTATION_RULE,
)

ALLOWED_DIRECT_NT_MUTATION_PATHS = frozenset(
    {
        "src/bolt_v3_order_execution.rs",
    }
)

ALLOWED_KILL_SWITCH_ACTION_BYPASS_PATHS = frozenset(
    {
        "src/bolt_v3_order_execution.rs",
    }
)

ALLOWED_KILL_SWITCH_POLICY_PATHS = frozenset(
    {
        "src/bolt_v3_order_execution.rs",
    }
)

ALLOWED_KILL_SWITCH_FLATTEN_SUPERVISOR_PATHS = frozenset(
    {
        "src/bolt_v3_order_execution.rs",
    }
)

ALLOWED_EXECUTION_POLICY_TYPE_REFERENCE_PATHS = frozenset(
    {
        "src/bolt_v3_config.rs",
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_live_node/risk_admission_loss.rs",
        "src/bolt_v3_order_execution.rs",
        "src/bolt_v3_strategy_registration.rs",
        "src/bolt_v3_validate/strategy_envelope.rs",
        "src/strategies/registry.rs",
    }
)

ALLOWED_EXECUTION_POLICY_CONSTRUCTION_PATHS = frozenset(
    {
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_live_node/risk_admission_loss.rs",
        "src/bolt_v3_order_execution.rs",
    }
)

ALLOWED_EXECUTION_POLICY_OVERRIDE_PATHS = frozenset(
    {
        "src/strategies/registry.rs",
    }
)

STRATEGY_ROOT_POLICY_EXEMPT_PATHS = frozenset(
    {
        "src/strategies/mod.rs",
        "src/strategies/registry.rs",
    }
)

ALLOWED_EXACT_VIOLATIONS = frozenset(
    {
        (
            "src/bolt_v3_live_node/risk_admission_loss.rs",
            "direct NT venue mutation call",
            "messages::execution::{SubmitOrder, TradingCommand},",
        ),
        (
            "src/bolt_v3_live_node/risk_admission_loss.rs",
            "direct NT venue mutation call",
            "let command = SubmitOrder::new(",
        ),
        (
            "src/bolt_v3_live_node/risk_admission_loss.rs",
            "direct NT venue mutation call",
            ".execute(TradingCommand::SubmitOrder(command));",
        ),
    }
)

def is_maker_strategy_source_path(path: str) -> bool:
    return path == MAKER_SOURCE_ROOT or path.startswith(f"{MAKER_SOURCE_ROOT}/")


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def is_test_source_file(path) -> bool:
    relative = path.relative_to(REPO_ROOT)
    return "tests" in relative.parts


def production_rust_files_under(relative_root: str) -> list:
    root = REPO_ROOT / relative_root
    files = []
    for path in root.rglob("*.rs"):
        if path.is_symlink():
            raise ValueError(f"source root contains a symlink: {path}")
        if path.is_file() and not is_test_source_file(path):
            files.append(path)
    files.sort(key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8"))
    return files


def configured_strategy_policy_source_files() -> list:
    try:
        source_set = source_set_files(STRATEGY_SOURCE_ROOTS, repo_root=REPO_ROOT)
    except FileNotFoundError:
        return []
    return sorted(
        path
        for path in source_set
        if not is_test_source_file(path)
    )


def source_files_for_strategy_policy_fence() -> list:
    configured_files = configured_strategy_policy_source_files()
    if not configured_files:
        return []
    files = set(configured_files)
    files.update(production_rust_files_under("src/strategies"))
    return sorted(
        files, key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8")
    )


def source_files_for_mutation_fence() -> list:
    return production_rust_files_under("src")


def gated_strategy_source_root_names() -> set[str]:
    # A strategy directory under `src/strategies/` is gated if it belongs to ANY
    # gated source set (taker, maker, or a future seal), not only the taker's
    # `STRATEGY_SOURCE_ROOTS`. Deriving from the union keeps each newly sealed
    # strategy recognized as gated without re-listing it here.
    return {
        relative_root
        for relative_root in ALL_GATED_SOURCE_ROOTS
        if relative_root.startswith("src/strategies/")
    }


def production_strategy_source_root_for(path) -> str | None:
    relative = path.relative_to(REPO_ROOT).as_posix()
    if relative in STRATEGY_ROOT_POLICY_EXEMPT_PATHS:
        return None
    parts = relative.split("/")
    if len(parts) < 3 or parts[0] != "src" or parts[1] != "strategies":
        return None
    if len(parts) == 3:
        return relative
    return "/".join(parts[:3])


def ungated_production_strategy_source_roots() -> list[str]:
    gated_roots = gated_strategy_source_root_names()
    roots = {
        root
        for path in production_rust_files_under("src/strategies")
        if (root := production_strategy_source_root_for(path)) is not None
    }
    return sorted(root for root in roots if root not in gated_roots)


def collect_strategy_source_root_violations() -> list[Violation]:
    return [
        Violation(
            path=root,
            line=1,
            label="ungated production strategy source root",
            excerpt=f"{root} is not listed in STRATEGY_SOURCE_ROOTS",
        )
        for root in ungated_production_strategy_source_roots()
    ]


def find_violations_in_text(
    path: str, text: str, rules: tuple[Rule, ...] = FORBIDDEN_RULES
) -> list[Violation]:
    # Blank comments and string literals to equal-length whitespace, preserving
    # every newline so match offsets still map 1:1 onto `text` for accurate line
    # and excerpt reporting. Code-construct rules scan this code-only view so a
    # banned token named inside an error message or doc string is not a false
    # positive; literal-targeting rules (scan_literals=True) scan the original.
    code_only = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for rule in rules:
        if (
            rule.label == "direct NT venue mutation call"
            and path in ALLOWED_DIRECT_NT_MUTATION_PATHS
        ):
            continue
        if (
            rule.label == "direct kill-switch action bypass"
            and path in ALLOWED_KILL_SWITCH_ACTION_BYPASS_PATHS
        ):
            continue
        if (
            rule.label == "strategy-local kill switch policy"
            and path in ALLOWED_KILL_SWITCH_POLICY_PATHS
        ):
            continue
        if (
            rule.label == "global kill-switch flatten supervisor policy"
            and path in ALLOWED_KILL_SWITCH_FLATTEN_SUPERVISOR_PATHS
        ):
            continue
        if (
            rule.label == "strategy-local execution policy construction"
            and path in ALLOWED_EXECUTION_POLICY_CONSTRUCTION_PATHS
        ):
            continue
        if (
            rule.label == "strategy-local execution policy type reference"
            and path in ALLOWED_EXECUTION_POLICY_TYPE_REFERENCE_PATHS
        ):
            continue
        if (
            rule.label == "strategy-local execution policy override"
            and path in ALLOWED_EXECUTION_POLICY_OVERRIDE_PATHS
        ):
            continue
        if (
            rule.label == "maker dependency on taker pricing internals"
            and not is_maker_strategy_source_path(path)
        ):
            continue
        scan_text = text if rule.scan_literals else code_only
        for match in rule.pattern.finditer(scan_text):
            line_start = text.rfind("\n", 0, match.start()) + 1
            line_end = text.find("\n", match.end())
            if line_end == -1:
                line_end = len(text)
            violation = Violation(
                path=path,
                line=line_number(text, match.start()),
                label=rule.label,
                excerpt=text[line_start:line_end].strip(),
            )
            if (violation.path, violation.label, violation.excerpt) in ALLOWED_EXACT_VIOLATIONS:
                continue
            violations.append(violation)
    return violations


def collect_violations() -> list[Violation]:
    violations: list[Violation] = []
    floor_errors: list[str] = []
    strategy_files = source_files_for_strategy_policy_fence()
    mutation_files = source_files_for_mutation_fence()
    require_nonempty(strategy_files, "strategy policy source files", floor_errors)
    require_nonempty(mutation_files, "mutation policy source files", floor_errors)
    violations.extend(
        Violation(path=".", line=0, label=error, excerpt="")
        for error in floor_errors
    )
    if floor_errors:
        return violations
    violations.extend(collect_strategy_source_root_violations())
    for path in strategy_files:
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(
            find_violations_in_text(rel, production_text(path), STRATEGY_POLICY_RULES)
        )
    for path in mutation_files:
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(
            find_violations_in_text(rel, production_text(path), EXECUTION_POLICY_RULES)
        )
    for path in mutation_files:
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(
            find_violations_in_text(
                rel,
                production_text(path),
                (DIRECT_NT_VENUE_MUTATION_RULE, RAW_MSGBUS_NT_VENUE_MUTATION_RULE),
            )
        )
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
