#!/usr/bin/env python3
"""Verify the Bolt-v3 Probability typed-value pilot surface."""

from __future__ import annotations

import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

import rust_source_scanner
from verify_bolt_v3_provider_leaks import production_text


REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class PatternCheck:
    path: str
    pattern: str
    description: str


@dataclass(frozen=True)
class MacroDefaultArm:
    argument_count: int
    target_indexes: tuple[int, ...]


FINANCIAL_VALUE_OWNER_MODULE = "src/bolt_v3_numeric.rs"
REGISTERED_FINANCIAL_VALUE_DEFAULT_CHECK_RE = re.compile(
    r"<\s*(?P<type>(?:::)?(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Za-z_][A-Za-z0-9_]*)"
    r"\s+as\s+AmbiguousIfDefault\s*<\s*_\s*>\s*>\s*::\s*_check"
)
TYPE_ALIAS_RE = re.compile(
    r"\btype\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*"
    r"(?P<target>(?:::)?(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Za-z_][A-Za-z0-9_]*)\s*;"
)
RUST_TYPE_PATH_TOKEN_RE = re.compile(
    r"(?:::)?(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Za-z_][A-Za-z0-9_]*"
)
MACRO_RULES_START_RE = re.compile(r"\bmacro_rules!\s*(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\{")
MACRO_MATCHER_VAR_RE = re.compile(r"\$(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*:")
DEFAULT_IMPL_TARGET_VAR_RE = re.compile(
    r"\bimpl\b(?s:.*?)\b(?:(?:::)?(?:std|core)\s*::\s*default\s*::\s*)?"
    r"Default\s+for\s+\$(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b"
)
REGISTERED_FINANCIAL_VALUE_DEFAULT_IMPL_RE = re.compile(
    r"\bimpl\b[^{};]*?\b(?:(?:::)?(?:std|core)\s*::\s*default\s*::\s*)?"
    r"Default\s+for\s+"
    r"(?P<type>(?:::)?(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Za-z_][A-Za-z0-9_]*)\b",
    re.DOTALL,
)
TYPE_WITH_ATTRS_RE = re.compile(
    r"(?P<attrs>(?:\s*#\s*\[[^\]]*\]\s*)*)"
    r"(?:pub(?:\s*\([^)]*\))?\s+)?(?:struct|enum)\s+"
    r"(?P<type>[A-Za-z_][A-Za-z0-9_]*)\b",
    re.DOTALL,
)
DEFAULT_DERIVE_ATTR_RE = re.compile(r"\bderive\s*\([^)]*\bDefault\b", re.DOTALL)
FINANCIAL_VALUE_MARKER_TOKEN_PATTERN = re.compile(
    r"\b(?:FinancialValue|financial_value_private|Sealed)\b"
)
# Drift snapshots, not Rust parsers. Broad tokens intentionally fail closed so
# new owner-module risk surface must be reviewed and allowlisted explicitly.
FINANCIAL_VALUE_OWNER_RISK_TOKENS = (
    "#",
    "!",
    "use",
    "FinancialValue",
    "financial_value_private",
    "Sealed",
    "Default",
    "AmbiguousIfDefault",
    "macro_rules",
)
FINANCIAL_VALUE_OWNER_PRODUCTION_RISK_LINE_ALLOWLIST = (
    ".all(|byte| byte.is_ascii_digit() || matches!(byte, b ..=b ))",
    "mod financial_value_private {",
    "pub trait Sealed {}",
    "#[allow(private_bounds)]",
    "pub trait FinancialValue: financial_value_private::Sealed {}",
    "#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]",
    "impl financial_value_private::Sealed for Probability {}",
    "impl FinancialValue for Probability {}",
    "impl financial_value_private::Sealed for crate::bolt_v3_maker_mu_estimator::UsableMu {}",
    "impl FinancialValue for crate::bolt_v3_maker_mu_estimator::UsableMu {}",
    "impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ValidRealizedVol {}",
    "impl FinancialValue for crate::bolt_v3_realized_volatility::ValidRealizedVol {}",
    "impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}",
    "impl FinancialValue for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}",
    "if !value.is_finite() || !eps.is_finite() {",
    "if !(eps > ZERO_F64 && eps < HALF_F64) {",
    "#[allow(dead_code)]",
    "trait AmbiguousIfDefault<A> {",
    "impl<T: ?Sized> AmbiguousIfDefault<()> for T {}",
    "impl<T: Default> AmbiguousIfDefault<Invalid> for T {}",
    "let _ = <Probability as AmbiguousIfDefault<_>>::_check;",
    "let _ = <crate::bolt_v3_maker_mu_estimator::UsableMu as AmbiguousIfDefault<_>>::_check;",
    "let _ = <crate::bolt_v3_realized_volatility::ValidRealizedVol as AmbiguousIfDefault<_>>::_check;",
    "let _ = <crate::bolt_v3_realized_volatility::ReadyRealizedVol as AmbiguousIfDefault<_>>::_check;",
)
FINANCIAL_VALUE_MARKER_ALLOWLIST = (
    ("src/bolt_v3_numeric.rs", "mod financial_value_private {"),
    ("src/bolt_v3_numeric.rs", "pub trait Sealed {}"),
    ("src/bolt_v3_numeric.rs", "pub trait FinancialValue: financial_value_private::Sealed {}"),
    ("src/bolt_v3_numeric.rs", "impl financial_value_private::Sealed for Probability {}"),
    ("src/bolt_v3_numeric.rs", "impl FinancialValue for Probability {}"),
    (
        "src/bolt_v3_numeric.rs",
        "impl financial_value_private::Sealed for crate::bolt_v3_maker_mu_estimator::UsableMu {}",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "impl FinancialValue for crate::bolt_v3_maker_mu_estimator::UsableMu {}",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ValidRealizedVol {}",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "impl FinancialValue for crate::bolt_v3_realized_volatility::ValidRealizedVol {}",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "impl FinancialValue for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}",
    ),
    ("src/bolt_v3_numeric.rs", "fn assert_financial_value<T: FinancialValue>() {}"),
)


REQUIRED_PATTERNS = [
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"(?:^|\n)\s*mod\s+financial_value_private\s*\{[^}]*pub\s+trait\s+Sealed\s*\{\s*\}",
        "FinancialValue sealing boundary module is private to bolt_v3_numeric",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"pub\s+trait\s+FinancialValue\s*:\s*financial_value_private::Sealed\s*\{\s*\}",
        "FinancialValue sealing boundary requires the private Sealed supertrait",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"#\[derive\(\s*Debug\s*,\s*Clone\s*,\s*Copy\s*,\s*PartialEq\s*,\s*PartialOrd\s*\)\]\s*pub struct Probability",
        "Probability derives only debug/copy/partial comparison traits",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"pub struct Probability\s*\{\s*value:\s*f64,?\s*\}",
        "Probability has a private named f64 field",
    ),
    PatternCheck(
        "src/bolt_v3_maker_mu_estimator.rs",
        r"pub struct UsableMu\s*\(\s*f64\s*\)\s*;",
        "UsableMu has a private f64 field",
    ),
    PatternCheck(
        "src/bolt_v3_realized_volatility.rs",
        r"pub struct ValidRealizedVol\s*\(\s*f64\s*\)\s*;",
        "ValidRealizedVol has a private f64 field",
    ),
    PatternCheck(
        "src/bolt_v3_realized_volatility.rs",
        r"pub struct ReadyRealizedVol\s*\(\s*ValidRealizedVol\s*\)\s*;",
        "ReadyRealizedVol has a private ValidRealizedVol field",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"pub fn new\(value:\s*f64\)\s*->\s*Option<Self>\s*\{[^}]*sanitize_probability",
        "Probability::new delegates to sanitize_probability",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"pub fn clamped\(value:\s*f64\)\s*->\s*Option<Self>",
        "Probability::clamped is fallible",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"fn\s+financial_values_do_not_implement_default\(\)[\s\S]*#\[cfg\(test\)]\s*mod\s+tests",
        "FinancialValue registered types have an always-compiled !Default guard",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"impl<T:\s*Default>\s+AmbiguousIfDefault<Invalid>\s+for\s+T\s*\{\}",
        "FinancialValue !Default guard fails if any registered type implements Default",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"<(?:super::)?Probability\s+as\s+AmbiguousIfDefault\s*<[^>]*>>::_check",
        "Probability is covered by the !Default guard",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"<crate::bolt_v3_maker_mu_estimator::UsableMu\s+as\s+AmbiguousIfDefault\s*<[^>]*>>::_check",
        "UsableMu is covered by the !Default guard",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"<crate::bolt_v3_realized_volatility::ValidRealizedVol\s+as\s+AmbiguousIfDefault\s*<[^>]*>>::_check",
        "ValidRealizedVol is covered by the !Default guard",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"<crate::bolt_v3_realized_volatility::ReadyRealizedVol\s+as\s+AmbiguousIfDefault\s*<[^>]*>>::_check",
        "ReadyRealizedVol is covered by the !Default guard",
    ),
    PatternCheck(
        "src/bolt_v3_market_families/mod.rs",
        r"pub fair_probability_up:\s*fn\(&FairProbabilityInputs\)\s*->\s*Option<Probability>",
        "market-family fair-probability binding is typed",
    ),
    PatternCheck(
        "src/bolt_v3_market_families/mod.rs",
        r"pub fn fair_probability_up_for_family\([^)]*\)\s*->\s*Option<Probability>",
        "market-family fair-probability dispatch is typed",
    ),
    PatternCheck(
        "src/bolt_v3_taker_updown_signal.rs",
        r"pub\(crate\) fn price_agreement_corr\([^)]*\)\s*->\s*Option<Probability>",
        "price agreement crossover returns Probability",
    ),
    PatternCheck(
        "src/bolt_v3_taker_updown_signal.rs",
        r"pub\(crate\) fn price_gap_probability\([^)]*\)\s*->\s*Option<Probability>",
        "price gap crossover returns Probability",
    ),
    PatternCheck(
        "src/bolt_v3_taker_updown_signal.rs",
        r"lead_gap_probability:\s*Probability",
        "uncertainty-band lead gap is typed",
    ),
    PatternCheck(
        "src/bolt_v3_taker_updown_signal.rs",
        r"fair_probability:\s*Option<Probability>",
        "worst-case EV fair probability is typed",
    ),
    PatternCheck(
        "src/bolt_v3_binary_outcome_edge.rs",
        r"fair_probability_up:\s*Option<Probability>",
        "binary edge fair probability is typed",
    ),
    PatternCheck(
        "src/bolt_v3_binary_outcome_edge.rs",
        r"adjusted_probability_up:\s*Option<Probability>",
        "binary edge adjusted probability is typed",
    ),
    PatternCheck(
        "src/bolt_v3_taker_pricing.rs",
        r"last_lead_gap_probability:\s*Option<Probability>",
        "taker lead-gap state is typed",
    ),
    PatternCheck(
        "src/bolt_v3_taker_pricing.rs",
        r"last_jitter_penalty_probability:\s*Option<Probability>",
        "taker jitter-penalty state is typed",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/entry_decision.rs",
        r"fair_probability_up:\s*Option<Probability>",
        "entry evaluation fair probability is typed",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/mod.rs",
        r"fn current_fair_probability_up_at\([^)]*\)\s*->\s*Option<Probability>",
        "strategy current fair probability accessor is typed",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/mod.rs",
        r"fn current_uncertainty_band_probability_at\([^)]*\)\s*->\s*Option<Probability>",
        "strategy uncertainty-band accessor is typed",
    ),
    PatternCheck(
        "src/bolt_v3_decision_evidence.rs",
        r"pub\(crate\) fn probability_evidence\(probability:\s*Probability\)\s*->\s*String",
        "probability evidence uses a typed wrapper",
    ),
]


BOUNDARY_PATTERNS = [
    PatternCheck(
        "src/bolt_v3_fair_value_pricing.rs",
        r"pub fair_probability_up:\s*f64,\s*pub fair_probability_down:\s*f64,",
        "FairValuePricingResult remains an f64 boundary",
    ),
    PatternCheck(
        "src/bolt_v3_taker_pricing.rs",
        r"pub fair_probability_up:\s*f64,\s*pub fair_probability_down:\s*f64,",
        "TakerPricingResult remains an f64 boundary",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/entry_decision.rs",
        r"pub\(super\) fair_probability_up:\s*Option<f64>,\s*pub\(super\) fair_probability_down:\s*Option<f64>,",
        "EntryEvaluationLogFields remains an f64 evidence boundary",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/exit_decision.rs",
        r"pub\(super\) fair_probability_up:\s*Option<f64>,\s*pub\(super\) fair_probability_down:\s*Option<f64>,",
        "ExitEvaluationLogFields remains an f64 evidence boundary",
    ),
]


FORBIDDEN_PATTERNS = [
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"\bProbability\s*\(",
        "direct Probability tuple construction",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"\bimpl\s+From\s*<\s*f64\s*>\s+for\s+Probability|\bimpl\s+Into\s*<\s*Probability\s*>",
        "f64 conversion traits for Probability",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"#\[derive\([^\]]*\b(?:Eq|Hash|Serialize|Deserialize)\b[^\]]*\)\]\s*pub struct Probability",
        "Eq/Hash/serde derives on Probability",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"\binclude\s*!",
        "generated include in FinancialValue owner module",
    ),
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"#\s*\[\s*path\s*=",
        "path-loaded module in FinancialValue owner module",
    ),
    PatternCheck(
        "src/bolt_v3_decision_evidence.rs",
        r"pub\s+[A-Za-z0-9_]+:\s*(?:Option\s*<\s*)?Probability",
        "Probability in decision-evidence serde fields",
    ),
    PatternCheck(
        "src/bolt_v3_taker_updown_signal.rs",
        r"fair_probability:\s*Option<f64>|uncertainty_band_probability:\s*f64",
        "raw f64 probability in taker up/down compute inputs",
    ),
    PatternCheck(
        "src/bolt_v3_binary_outcome_edge.rs",
        r"fair_probability_up:\s*Option<f64>|adjusted_probability_up:\s*Option<f64>",
        "raw f64 probability in binary edge inputs",
    ),
    PatternCheck(
        "src/strategies/binary_oracle_edge_taker/entry_decision.rs",
        r"pub\(super\) fair_probability_up:\s*Option<f64>,\s*pub\(super\) uncertainty_band_probability:\s*Option<f64>,",
        "raw f64 probability in EntryEvaluation",
    ),
]


def read_source(root: Path, relative_path: str) -> str:
    return (root / relative_path).read_text(encoding="utf-8")


def scanner_source(root: Path, relative_path: str) -> str:
    source = read_source(root, relative_path)
    if relative_path.endswith(".rs"):
        return rust_source_scanner.strip_rust_comments_and_literals(source)
    return source


def rust_sources(root: Path) -> list[tuple[str, str]]:
    sources = []
    src_root = root / "src"
    if not src_root.exists():
        return sources
    for path in sorted(src_root.rglob("*.rs")):
        relative_path = path.relative_to(root).as_posix()
        raw_source = path.read_text(encoding="utf-8")
        sources.append((relative_path, rust_source_scanner.strip_rust_comments_and_literals(raw_source)))
    return sources


def missing_required(root: Path, checks: list[PatternCheck]) -> list[str]:
    findings = []
    for check in checks:
        source = scanner_source(root, check.path)
        if not re.search(check.pattern, source, re.DOTALL):
            findings.append(f"{check.path}: missing {check.description}")
    return findings


def present_forbidden(root: Path, checks: list[PatternCheck]) -> list[str]:
    findings = []
    for check in checks:
        source = scanner_source(root, check.path)
        if re.search(check.pattern, source, re.DOTALL):
            findings.append(f"{check.path}: forbidden {check.description}")
    return findings


def normalize_source_line(line: str) -> str:
    return " ".join(line.strip().split())


def normalized_source_lines(source: str) -> tuple[str, ...]:
    return tuple(
        normalized
        for line in source.splitlines()
        if (normalized := normalize_source_line(line))
    )


def financial_value_marker_lines(root: Path) -> Counter[tuple[str, str]]:
    lines: Counter[tuple[str, str]] = Counter()
    for relative_path, source in rust_sources(root):
        for line in source.splitlines():
            normalized = normalize_source_line(line)
            if FINANCIAL_VALUE_MARKER_TOKEN_PATTERN.search(normalized):
                lines[(relative_path, normalized)] += 1
    return lines


def verify_financial_value_marker_allowlist(root: Path) -> list[str]:
    expected = Counter(FINANCIAL_VALUE_MARKER_ALLOWLIST)
    actual = financial_value_marker_lines(root)
    if actual == expected:
        return []

    missing = sorted((expected - actual).elements())
    extra = sorted((actual - expected).elements())
    details = []
    if missing:
        details.append(f"missing {missing!r}")
    if extra:
        details.append(f"extra {extra!r}")
    return [f"src/: FinancialValue marker allowlist mismatch: {', '.join(details)}"]


def verify_financial_value_owner_risk_surface(root: Path) -> list[str]:
    source = rust_source_scanner.strip_rust_comments_and_literals(
        production_text(read_source(root, FINANCIAL_VALUE_OWNER_MODULE))
    )
    actual = tuple(
        line
        for line in normalized_source_lines(source)
        if any(token in line for token in FINANCIAL_VALUE_OWNER_RISK_TOKENS)
    )
    expected = FINANCIAL_VALUE_OWNER_PRODUCTION_RISK_LINE_ALLOWLIST
    if actual == expected:
        return []

    missing = list((Counter(expected) - Counter(actual)).elements())
    extra = list((Counter(actual) - Counter(expected)).elements())
    details = []
    if missing:
        details.append(f"missing {missing!r}")
    if extra:
        details.append(f"extra {extra!r}")
    if not details:
        details.append("order changed")
    return [
        f"{FINANCIAL_VALUE_OWNER_MODULE}: FinancialValue owner production risk surface mismatch: {', '.join(details)}"
    ]


def path_type_name(type_name: str) -> str:
    return re.sub(r"\s+", "", type_name).lstrip(":").split("::")[-1]


def registered_financial_value_types(root: Path) -> set[str]:
    source = scanner_source(root, FINANCIAL_VALUE_OWNER_MODULE)
    return {
        path_type_name(match.group("type"))
        for match in REGISTERED_FINANCIAL_VALUE_DEFAULT_CHECK_RE.finditer(source)
    }


def registered_financial_value_aliases(root: Path, registered_types: set[str]) -> dict[str, str]:
    aliases: dict[str, str] = {}
    for _, source in rust_sources(root):
        for match in TYPE_ALIAS_RE.finditer(source):
            aliases[match.group("alias")] = path_type_name(match.group("target"))

    resolved: dict[str, str] = {}
    for alias in aliases:
        seen: set[str] = set()
        current = alias
        while current in aliases and current not in seen:
            seen.add(current)
            current = aliases[current]
        if current in registered_types:
            resolved[alias] = current
    return resolved


def balanced_span(source: str, start: int, open_char: str, close_char: str) -> tuple[int, int] | None:
    depth = 0
    for index in range(start, len(source)):
        char = source[index]
        if char == open_char:
            depth += 1
        elif char == close_char:
            depth -= 1
            if depth == 0:
                return start, index + 1
    return None


def macro_matcher_variables(body: str) -> list[str]:
    variables: list[str] = []
    seen: set[str] = set()
    for match in MACRO_MATCHER_VAR_RE.finditer(body):
        name = match.group("name")
        if name not in seen:
            seen.add(name)
            variables.append(name)
    return variables


def macro_rule_arms(body: str) -> list[tuple[str, str]]:
    inner = body[1:-1] if body.startswith("{") and body.endswith("}") else body
    close_by_open = {"(": ")", "[": "]", "{": "}"}
    arms: list[tuple[str, str]] = []
    index = 0
    while index < len(inner):
        while index < len(inner) and (inner[index].isspace() or inner[index] == ";"):
            index += 1
        if index >= len(inner):
            break
        open_char = inner[index]
        close_char = close_by_open.get(open_char)
        if close_char is None:
            index += 1
            continue
        matcher_span = balanced_span(inner, index, open_char, close_char)
        if matcher_span is None:
            break
        matcher_text = inner[matcher_span[0] + 1:matcher_span[1] - 1]
        index = matcher_span[1]
        while index < len(inner) and inner[index].isspace():
            index += 1
        if not inner.startswith("=>", index):
            continue
        index += 2
        while index < len(inner) and inner[index].isspace():
            index += 1
        if index >= len(inner):
            break
        open_char = inner[index]
        close_char = close_by_open.get(open_char)
        if close_char is None:
            index += 1
            continue
        expansion_span = balanced_span(inner, index, open_char, close_char)
        if expansion_span is None:
            break
        expansion_text = inner[expansion_span[0] + 1:expansion_span[1] - 1]
        arms.append((matcher_text, expansion_text))
        index = expansion_span[1]
    return arms


def default_generating_macro_arms(source: str) -> dict[str, tuple[MacroDefaultArm, ...]]:
    macros: dict[str, tuple[MacroDefaultArm, ...]] = {}
    for match in MACRO_RULES_START_RE.finditer(source):
        span = balanced_span(source, match.end() - 1, "{", "}")
        if span is None:
            continue
        body = source[span[0]:span[1]]
        default_arms: list[MacroDefaultArm] = []
        for matcher_text, expansion_text in macro_rule_arms(body):
            matcher_variables = macro_matcher_variables(matcher_text)
            target_indexes = sorted(
                {
                    matcher_variables.index(target.group("name"))
                    for target in DEFAULT_IMPL_TARGET_VAR_RE.finditer(expansion_text)
                    if target.group("name") in matcher_variables
                }
            )
            if target_indexes:
                default_arms.append(
                    MacroDefaultArm(
                        argument_count=len(matcher_variables),
                        target_indexes=tuple(target_indexes),
                    )
                )
        if default_arms:
            macros[match.group("name")] = tuple(default_arms)
    return macros


def macro_invocation_arguments(source: str, macro_name: str) -> list[str]:
    invocation_re = re.compile(rf"\b{re.escape(macro_name)}\s*!\s*(?P<open>[\(\{{\[])")
    close_by_open = {"(": ")", "{": "}", "[": "]"}
    arguments: list[str] = []
    for match in invocation_re.finditer(source):
        open_char = match.group("open")
        close_char = close_by_open[open_char]
        span = balanced_span(source, match.end() - 1, open_char, close_char)
        if span is not None:
            arguments.append(source[span[0] + 1:span[1] - 1])
    return arguments


def split_macro_arguments(arguments: str) -> list[str]:
    chunks: list[str] = []
    start = 0
    round_depth = square_depth = brace_depth = angle_depth = 0
    for index, char in enumerate(arguments):
        if char == "(":
            round_depth += 1
        elif char == ")" and round_depth:
            round_depth -= 1
        elif char == "[":
            square_depth += 1
        elif char == "]" and square_depth:
            square_depth -= 1
        elif char == "{":
            brace_depth += 1
        elif char == "}" and brace_depth:
            brace_depth -= 1
        elif char == "<":
            angle_depth += 1
        elif char == ">" and angle_depth:
            angle_depth -= 1
        elif (
            char == ","
            and round_depth == square_depth == brace_depth == angle_depth == 0
        ):
            chunks.append(arguments[start:index].strip())
            start = index + 1
    tail = arguments[start:].strip()
    if tail:
        chunks.append(tail)
    return chunks


def registered_type_for_token(
    token: str,
    registered_types: set[str],
    registered_aliases: dict[str, str],
) -> str | None:
    target = path_type_name(token)
    return target if target in registered_types else registered_aliases.get(target)


def verify_registered_financial_value_default_surface(root: Path) -> list[str]:
    findings = []
    registered_types = registered_financial_value_types(root)
    if not registered_types:
        return [f"{FINANCIAL_VALUE_OWNER_MODULE}: no registered FinancialValue types; Default fence cannot run"]
    registered_aliases = registered_financial_value_aliases(root, registered_types)
    source_items = tuple(rust_sources(root))
    default_macro_arms: dict[str, list[MacroDefaultArm]] = {}
    for _, source in source_items:
        for macro_name, arms in default_generating_macro_arms(source).items():
            default_macro_arms.setdefault(macro_name, []).extend(arms)
    for relative_path, source in source_items:
        for match in REGISTERED_FINANCIAL_VALUE_DEFAULT_IMPL_RE.finditer(source):
            registered_type = registered_type_for_token(match.group("type"), registered_types, registered_aliases)
            if registered_type is not None:
                findings.append(
                    f"{relative_path}: registered FinancialValue Default impl/derive "
                    f"for {registered_type} is forbidden"
                )
        for match in TYPE_WITH_ATTRS_RE.finditer(source):
            if DEFAULT_DERIVE_ATTR_RE.search(match.group("attrs")):
                type_name = path_type_name(match.group("type"))
                if type_name in registered_types:
                    findings.append(
                        f"{relative_path}: registered FinancialValue Default impl/derive "
                        f"for {type_name} is forbidden"
                    )
        for macro_name, arms in default_macro_arms.items():
            for arguments in macro_invocation_arguments(source, macro_name):
                argument_chunks = split_macro_arguments(arguments)
                for arm in arms:
                    if len(argument_chunks) != arm.argument_count:
                        continue
                    for target_index in arm.target_indexes:
                        for token_match in RUST_TYPE_PATH_TOKEN_RE.finditer(argument_chunks[target_index]):
                            registered_type = registered_type_for_token(
                                token_match.group(0),
                                registered_types,
                                registered_aliases,
                            )
                            if registered_type is not None:
                                findings.append(
                                    f"{relative_path}: registered FinancialValue Default impl/derive "
                                    f"for {registered_type} is forbidden"
                                )
    return sorted(set(findings))


def verify(root: Path) -> list[str]:
    findings = []
    findings.extend(missing_required(root, REQUIRED_PATTERNS))
    findings.extend(missing_required(root, BOUNDARY_PATTERNS))
    findings.extend(present_forbidden(root, FORBIDDEN_PATTERNS))
    findings.extend(verify_financial_value_marker_allowlist(root))
    findings.extend(verify_financial_value_owner_risk_surface(root))
    findings.extend(verify_registered_financial_value_default_surface(root))
    return findings


def main() -> int:
    findings = verify(REPO_ROOT)
    if findings:
        for finding in findings:
            print(f"ERROR: {finding}", file=sys.stderr)
        return 1
    print("OK: Probability typed-value pilot verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
