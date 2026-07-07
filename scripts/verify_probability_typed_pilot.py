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


FINANCIAL_VALUE_OWNER_MODULE = "src/bolt_v3_numeric.rs"
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
# Intentional global tripwire: any new `Default` token under src/ must be
# reviewed before allowlisting. Narrowing this to registered-type-adjacent
# lines would make cfg-inactive/off-file aliases depend on prediction again.
FINANCIAL_VALUE_DEFAULT_TOKEN_ALLOWLIST = (
    ("src/bolt_v3_live_node/risk_admission_loss.rs", "#[derive(Default)]"),
    ("src/bolt_v3_live_node/risk_admission_loss.rs", "#[derive(Debug, Default)]"),
    ("src/bolt_v3_live_node/tests/data_client_probe.rs", "clients: Default::default(),"),
    ("src/bolt_v3_live_node/tests/transport_scope.rs", "clients: Default::default(),"),
    ("src/bolt_v3_live_node/tests/transport_scope.rs", "clients: Default::default(),"),
    ("src/bolt_v3_numeric.rs", "trait AmbiguousIfDefault<A> {"),
    ("src/bolt_v3_numeric.rs", "impl<T: ?Sized> AmbiguousIfDefault<()> for T {}"),
    ("src/bolt_v3_numeric.rs", "impl<T: Default> AmbiguousIfDefault<Invalid> for T {}"),
    ("src/bolt_v3_numeric.rs", "let _ = <Probability as AmbiguousIfDefault<_>>::_check;"),
    (
        "src/bolt_v3_numeric.rs",
        "let _ = <crate::bolt_v3_maker_mu_estimator::UsableMu as AmbiguousIfDefault<_>>::_check;",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "let _ = <crate::bolt_v3_realized_volatility::ValidRealizedVol as AmbiguousIfDefault<_>>::_check;",
    ),
    (
        "src/bolt_v3_numeric.rs",
        "let _ = <crate::bolt_v3_realized_volatility::ReadyRealizedVol as AmbiguousIfDefault<_>>::_check;",
    ),
    ("src/bolt_v3_order_execution.rs", "#[derive(Debug, Default)]"),
    ("src/bolt_v3_order_execution.rs", "#[derive(Debug, Default)]"),
    ("src/bolt_v3_submit_admission.rs", "#[derive(Debug, Default)]"),
    ("src/bolt_v3_submit_admission.rs", "#[derive(Default)]"),
    ("src/shadow_pnl.rs", "#[derive(Debug, Clone, Default)]"),
    (
        "src/strategies/binary_oracle_edge_taker/tests/adverse_path_harness.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/adverse_path_harness.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs",
        "assert_eq!(order.trigger_type(), Some(TriggerType::Default));",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs",
        "#[derive(Debug, Default)]",
    ),
    (
        "src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs",
        "#[derive(Default)]",
    ),
    ("src/strategies/registry.rs", "..Default::default()"),
    ("src/strategies/registry.rs", "let raw = toml::Value::Table(Default::default());"),
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


def verify_financial_value_default_token_allowlist(root: Path) -> list[str]:
    actual = Counter(
        (relative_path, line)
        for relative_path, source in rust_sources(root)
        for line in normalized_source_lines(source)
        if "Default" in line
    )
    expected = Counter(
        (relative_path, line)
        for relative_path, line in FINANCIAL_VALUE_DEFAULT_TOKEN_ALLOWLIST
        if (root / relative_path).exists()
    )
    if actual == expected:
        return []

    missing = sorted((expected - actual).elements())
    extra = sorted((actual - expected).elements())
    details = []
    if missing:
        details.append(f"missing {missing!r}")
    if extra:
        details.append(f"extra {extra!r}")
    guidance = (
        "Add unrelated source lines containing the text Default to "
        "FINANCIAL_VALUE_DEFAULT_TOKEN_ALLOWLIST after review. Do not "
        "allowlist Default for Probability, UsableMu, ValidRealizedVol, "
        "ReadyRealizedVol, or aliases of them."
    )
    return [
        f"src/: FinancialValue Default token allowlist mismatch: {', '.join(details)}. {guidance}"
    ]


def verify(root: Path) -> list[str]:
    findings = []
    findings.extend(missing_required(root, REQUIRED_PATTERNS))
    findings.extend(missing_required(root, BOUNDARY_PATTERNS))
    findings.extend(present_forbidden(root, FORBIDDEN_PATTERNS))
    findings.extend(verify_financial_value_marker_allowlist(root))
    findings.extend(verify_financial_value_owner_risk_surface(root))
    findings.extend(verify_financial_value_default_token_allowlist(root))
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
