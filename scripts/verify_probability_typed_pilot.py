#!/usr/bin/env python3
"""Verify the Bolt-v3 Probability typed-value pilot surface."""

from __future__ import annotations

import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

import rust_source_scanner


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
FINANCIAL_VALUE_OWNER_MACRO_INVOCATION_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_])([A-Za-z_][A-Za-z0-9_]*)\s*!\s*[\(\[\{]"
)
FINANCIAL_VALUE_OWNER_ALLOWED_MACROS = frozenset(("assert", "assert_eq", "matches"))
FINANCIAL_VALUE_OWNER_BANG_OPERATOR_KEYWORDS = frozenset(("if", "while"))
FINANCIAL_VALUE_OWNER_ATTRIBUTE_PATTERN = re.compile(r"^\s*#\s*\[[^\]]+\]\s*$")
FINANCIAL_VALUE_OWNER_ALLOWED_ATTRIBUTES = frozenset(
    (
        "#[allow(dead_code)]",
        "#[allow(private_bounds)]",
        "#[cfg(test)]",
        "#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]",
        "#[test]",
    )
)
FINANCIAL_VALUE_OWNER_USE_PATTERN = re.compile(r"^\s*use\b.*$")
FINANCIAL_VALUE_OWNER_ALLOWED_USES = frozenset(("use super::*;",))
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
        r"impl<T:\s*\?Sized\s*\+\s*Default>\s+AmbiguousIfDefault<Invalid>\s+for\s+T\s*\{\}",
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
        "src/bolt_v3_numeric.rs",
        r"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|test)\s*\]\s*fn\s+financial_values_do_not_implement_default",
        "test-only FinancialValue !Default guard",
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


def verify_financial_value_owner_macro_allowlist(root: Path) -> list[str]:
    source = scanner_source(root, FINANCIAL_VALUE_OWNER_MODULE)
    macro_extras = sorted(
        {
            match.group(1)
            for match in FINANCIAL_VALUE_OWNER_MACRO_INVOCATION_PATTERN.finditer(source)
            if match.group(1) not in FINANCIAL_VALUE_OWNER_ALLOWED_MACROS
            and match.group(1) not in FINANCIAL_VALUE_OWNER_BANG_OPERATOR_KEYWORDS
        }
    )
    attribute_extras = sorted(
        {
            normalize_source_line(line)
            for line in source.splitlines()
            if FINANCIAL_VALUE_OWNER_ATTRIBUTE_PATTERN.fullmatch(line)
            and normalize_source_line(line) not in FINANCIAL_VALUE_OWNER_ALLOWED_ATTRIBUTES
        }
    )
    use_extras = sorted(
        {
            normalize_source_line(line)
            for line in source.splitlines()
            if FINANCIAL_VALUE_OWNER_USE_PATTERN.fullmatch(line)
            and normalize_source_line(line) not in FINANCIAL_VALUE_OWNER_ALLOWED_USES
        }
    )

    findings = []
    if macro_extras:
        findings.append(
            f"{FINANCIAL_VALUE_OWNER_MODULE}: forbidden macro invocation in FinancialValue owner module: {macro_extras!r}"
        )
    if attribute_extras:
        findings.append(
            f"{FINANCIAL_VALUE_OWNER_MODULE}: forbidden attribute in FinancialValue owner module: {attribute_extras!r}"
        )
    if use_extras:
        findings.append(
            f"{FINANCIAL_VALUE_OWNER_MODULE}: forbidden use import in FinancialValue owner module: {use_extras!r}"
        )
    return findings


def verify(root: Path) -> list[str]:
    findings = []
    findings.extend(missing_required(root, REQUIRED_PATTERNS))
    findings.extend(missing_required(root, BOUNDARY_PATTERNS))
    findings.extend(present_forbidden(root, FORBIDDEN_PATTERNS))
    findings.extend(verify_financial_value_marker_allowlist(root))
    findings.extend(verify_financial_value_owner_macro_allowlist(root))
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
