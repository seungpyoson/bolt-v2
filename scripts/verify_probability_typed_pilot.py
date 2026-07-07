#!/usr/bin/env python3
"""Verify the Bolt-v3 Probability typed-value pilot surface."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

import rust_source_scanner


REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class PatternCheck:
    path: str
    pattern: str
    description: str


@dataclass(frozen=True)
class FinancialValueRegistration:
    path: str
    type_name: str


REGISTERED_FINANCIAL_VALUES = (
    FinancialValueRegistration("src/bolt_v3_numeric.rs", "Probability"),
    FinancialValueRegistration("src/bolt_v3_maker_mu_estimator.rs", "UsableMu"),
    FinancialValueRegistration("src/bolt_v3_realized_volatility.rs", "ValidRealizedVol"),
    FinancialValueRegistration("src/bolt_v3_realized_volatility.rs", "ReadyRealizedVol"),
)

RUST_PATH_SEGMENT = r"(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)"
RUST_TYPE_PATH = rf"(?:{RUST_PATH_SEGMENT}::)*([A-Za-z_][A-Za-z0-9_]*)"
FINANCIAL_VALUE_TRAIT_PATH = rf"(?:{RUST_PATH_SEGMENT}::)*FinancialValue"
SEALED_TRAIT_PATH = rf"(?:{RUST_PATH_SEGMENT}::)*financial_value_private::Sealed"


REQUIRED_PATTERNS = [
    PatternCheck(
        "src/bolt_v3_numeric.rs",
        r"pub\(crate\)\s+mod\s+financial_value_private\s*\{[^}]*pub\s+trait\s+Sealed\s*\{\s*\}",
        "FinancialValue sealing boundary module is crate-private",
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


def rust_sources(root: Path) -> list[tuple[str, str]]:
    sources = []
    src_root = root / "src"
    if not src_root.exists():
        return sources
    for path in src_root.rglob("*.rs"):
        relative_path = path.relative_to(root).as_posix()
        raw_source = path.read_text(encoding="utf-8")
        sources.append((relative_path, rust_source_scanner.strip_rust_comments_and_literals(raw_source)))
    return sources


def missing_required(root: Path, checks: list[PatternCheck]) -> list[str]:
    findings = []
    for check in checks:
        source = read_source(root, check.path)
        if not re.search(check.pattern, source, re.DOTALL):
            findings.append(f"{check.path}: missing {check.description}")
    return findings


def present_forbidden(root: Path, checks: list[PatternCheck]) -> list[str]:
    findings = []
    for check in checks:
        source = read_source(root, check.path)
        if re.search(check.pattern, source, re.DOTALL):
            findings.append(f"{check.path}: forbidden {check.description}")
    return findings


def financial_value_impl_set(root: Path) -> set[tuple[str, str]]:
    impls = set()
    for relative_path, source in rust_sources(root):
        for match in re.finditer(
            rf"\bimpl\s+{FINANCIAL_VALUE_TRAIT_PATH}\s+for\s+{RUST_TYPE_PATH}\b", source
        ):
            impls.add((relative_path, match.group(1)))
    return impls


def financial_value_sealed_impl_set(root: Path) -> set[tuple[str, str]]:
    impls = set()
    for relative_path, source in rust_sources(root):
        for match in re.finditer(
            rf"\bimpl\s+{SEALED_TRAIT_PATH}\s+for\s+{RUST_TYPE_PATH}\b",
            source,
        ):
            impls.add((relative_path, match.group(1)))
    return impls


def verify_financial_value_implementors(root: Path) -> list[str]:
    expected = {(registration.path, registration.type_name) for registration in REGISTERED_FINANCIAL_VALUES}
    actual = financial_value_impl_set(root)
    if actual == expected:
        return []

    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    details = []
    if missing:
        details.append(f"missing {missing!r}")
    if extra:
        details.append(f"extra {extra!r}")
    return [f"src/: FinancialValue implementor set mismatch: {', '.join(details)}"]


def verify_financial_value_sealing(root: Path) -> list[str]:
    findings = []
    sealed_impls = financial_value_sealed_impl_set(root)
    for registration in REGISTERED_FINANCIAL_VALUES:
        expected = (registration.path, registration.type_name)
        if expected not in sealed_impls:
            findings.append(
                f"{registration.path}: missing FinancialValue sealing boundary for {registration.type_name}"
            )
    numeric_source = rust_source_scanner.strip_rust_comments_and_literals(
        read_source(root, "src/bolt_v3_numeric.rs")
    )
    if re.search(r"\bpub\s+mod\s+financial_value_private\b", numeric_source):
        findings.append("src/bolt_v3_numeric.rs: forbidden public FinancialValue sealing boundary")
    return findings


def financial_value_type_pattern(type_name: str) -> str:
    return rf"(?:[A-Za-z_][A-Za-z0-9_]*::)*{re.escape(type_name)}\b"


def verify_financial_value_defaults(root: Path) -> list[str]:
    findings = []
    sources = rust_sources(root)
    for registration in REGISTERED_FINANCIAL_VALUES:
        type_pattern = financial_value_type_pattern(registration.type_name)
        impl_pattern = re.compile(rf"\bimpl\s+Default\s+for\s+{type_pattern}", re.DOTALL)
        derive_pattern = re.compile(
            rf"#\s*\[\s*derive\s*\([^\]]*\bDefault\b[^\]]*\)\s*\]\s*"
            rf"(?:pub(?:\([^)]*\))?\s+)?struct\s+{re.escape(registration.type_name)}\b",
            re.DOTALL,
        )
        for relative_path, source in sources:
            if impl_pattern.search(source):
                findings.append(
                    f"{relative_path}: forbidden Default impl for FinancialValue {registration.type_name}"
                )
            if derive_pattern.search(source):
                findings.append(
                    f"{relative_path}: forbidden Default derive for FinancialValue {registration.type_name}"
                )
    return findings


def verify(root: Path) -> list[str]:
    findings = []
    findings.extend(missing_required(root, REQUIRED_PATTERNS))
    findings.extend(missing_required(root, BOUNDARY_PATTERNS))
    findings.extend(present_forbidden(root, FORBIDDEN_PATTERNS))
    findings.extend(verify_financial_value_implementors(root))
    findings.extend(verify_financial_value_defaults(root))
    findings.extend(verify_financial_value_sealing(root))
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
