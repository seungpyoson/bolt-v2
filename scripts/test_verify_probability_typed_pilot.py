#!/usr/bin/env python3
"""Self-tests for the Probability typed-value pilot verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_probability_typed_pilot.py")
SPEC = importlib.util.spec_from_file_location("verify_probability_typed_pilot", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def write_sources(root: Path, overrides: dict[str, str] | None = None) -> None:
    sources = {
        "src/bolt_v3_numeric.rs": """
mod financial_value_private {
    pub trait Sealed {}
}

#[allow(private_bounds)]
pub trait FinancialValue: financial_value_private::Sealed {}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability { value: f64 }
impl financial_value_private::Sealed for Probability {}
impl FinancialValue for Probability {}
impl financial_value_private::Sealed for crate::bolt_v3_maker_mu_estimator::UsableMu {}
impl FinancialValue for crate::bolt_v3_maker_mu_estimator::UsableMu {}
impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ValidRealizedVol {}
impl FinancialValue for crate::bolt_v3_realized_volatility::ValidRealizedVol {}
impl financial_value_private::Sealed for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}
impl FinancialValue for crate::bolt_v3_realized_volatility::ReadyRealizedVol {}
impl Probability {
    pub fn new(value: f64) -> Option<Self> { sanitize_probability(value).map(|value| Self { value }) }
    pub fn clamped(value: f64) -> Option<Self> { Some(Self { value }) }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn financial_value_marker_is_implemented_for_registered_types() {
        fn assert_financial_value<T: FinancialValue>() {}

        assert_financial_value::<Probability>();
        assert_financial_value::<crate::bolt_v3_maker_mu_estimator::UsableMu>();
        assert_financial_value::<crate::bolt_v3_realized_volatility::ReadyRealizedVol>();
        assert_financial_value::<crate::bolt_v3_realized_volatility::ValidRealizedVol>();
    }

    #[test]
    fn financial_values_do_not_implement_default() {
        trait AmbiguousIfDefault<A> {
            fn _check() {}
        }
        impl<T: ?Sized> AmbiguousIfDefault<()> for T {}
        struct Invalid;
        impl<T: ?Sized + Default> AmbiguousIfDefault<Invalid> for T {}

        let _ = <super::Probability as AmbiguousIfDefault<_>>::_check;
        let _ =
            <crate::bolt_v3_maker_mu_estimator::UsableMu as AmbiguousIfDefault<_>>::_check;
        let _ = <crate::bolt_v3_realized_volatility::ValidRealizedVol as AmbiguousIfDefault<
            _,
        >>::_check;
        let _ = <crate::bolt_v3_realized_volatility::ReadyRealizedVol as AmbiguousIfDefault<
            _,
        >>::_check;
    }
}
""",
        "src/bolt_v3_maker_mu_estimator.rs": """
use crate::bolt_v3_numeric::{is_positive_finite, sanitize_probability};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsableMu(f64);
impl UsableMu {
    fn new(value: f64) -> Self { Self(value) }
    pub fn get(self) -> f64 { self.0 }
}
""",
        "src/bolt_v3_realized_volatility.rs": """
use crate::bolt_v3_numeric::{is_positive_finite, ZERO_F64};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ValidRealizedVol(f64);
impl ValidRealizedVol {
    pub fn new(value: f64) -> Option<Self> { Some(Self(value)) }
    pub fn get(self) -> f64 { self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ReadyRealizedVol(ValidRealizedVol);
impl ReadyRealizedVol {
    pub fn get(self) -> f64 { self.0.get() }
}
""",
        "src/bolt_v3_market_families/mod.rs": """
pub fair_probability_up: fn(&FairProbabilityInputs) -> Option<Probability>,
pub fn fair_probability_up_for_family(family_key: &str, inputs: &FairProbabilityInputs) -> Option<Probability> { None }
""",
        "src/bolt_v3_taker_updown_signal.rs": """
pub(crate) fn price_agreement_corr(observed_price: f64, anchor_price: f64) -> Option<Probability> { None }
pub(crate) fn price_gap_probability(observed_price: f64, reference_price: f64) -> Option<Probability> { None }
struct UncertaintyBandInputs { lead_gap_probability: Probability }
struct WorstCaseEvInputs { fair_probability: Option<Probability> }
""",
        "src/bolt_v3_binary_outcome_edge.rs": """
struct BinaryOutcomeEdgeInputs {
    fair_probability_up: Option<Probability>,
    adjusted_probability_up: Option<Probability>,
}
""",
        "src/bolt_v3_taker_pricing.rs": """
struct TakerPricingResult {
    pub fair_probability_up: f64,
    pub fair_probability_down: f64,
}
struct TakerPricingState {
    last_lead_gap_probability: Option<Probability>,
    last_jitter_penalty_probability: Option<Probability>,
}
""",
        "src/strategies/binary_oracle_edge_taker/entry_decision.rs": """
struct EntryEvaluation {
    fair_probability_up: Option<Probability>,
    uncertainty_band_probability: Option<Probability>,
}
struct EntryEvaluationLogFields {
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
}
""",
        "src/strategies/binary_oracle_edge_taker/mod.rs": """
fn current_fair_probability_up_at(&self, now_ms: u64) -> Option<Probability> { None }
fn current_uncertainty_band_probability_at(&self, now_ms: u64) -> Option<Probability> { None }
""",
        "src/bolt_v3_decision_evidence.rs": """
pub(crate) fn probability_evidence(probability: Probability) -> String { String::new() }
""",
        "src/bolt_v3_fair_value_pricing.rs": """
struct FairValuePricingResult {
    pub fair_probability_up: f64,
    pub fair_probability_down: f64,
}
""",
        "src/strategies/binary_oracle_edge_taker/exit_decision.rs": """
struct ExitEvaluationLogFields {
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
}
""",
    }
    if overrides:
        sources.update(overrides)
    for relative_path, source in sources.items():
        path = root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")


def test_verify_accepts_expected_typed_surface() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(root)
        findings = VERIFIER.verify(root)
        if findings:
            raise AssertionError(f"expected no findings, got {findings!r}")


def test_verify_rejects_raw_entry_evaluation_probability() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/strategies/binary_oracle_edge_taker/entry_decision.rs": """
struct EntryEvaluation {
    pub(super) fair_probability_up: Option<f64>,
    pub(super) uncertainty_band_probability: Option<f64>,
}
struct EntryEvaluationLogFields {
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("EntryEvaluation" in finding for finding in findings):
            raise AssertionError(f"expected EntryEvaluation finding, got {findings!r}")


def test_verify_rejects_missing_financial_value_implementor() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_realized_volatility.rs": """
use crate::bolt_v3_numeric::{FinancialValue, financial_value_private};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ValidRealizedVol(f64);
impl financial_value_private::Sealed for ValidRealizedVol {}
impl FinancialValue for ValidRealizedVol {}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ReadyRealizedVol(ValidRealizedVol);
impl financial_value_private::Sealed for ReadyRealizedVol {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue marker allowlist" in finding for finding in findings):
            raise AssertionError(f"expected FinancialValue marker finding, got {findings!r}")


def test_verify_rejects_unregistered_financial_value_implementor() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_sizing.rs": """
use crate::bolt_v3_numeric::{FinancialValue, financial_value_private};

pub struct SizingScale(f64);
impl crate::bolt_v3_numeric::financial_value_private::Sealed for SizingScale {}
impl crate::bolt_v3_numeric::FinancialValue for SizingScale {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue marker allowlist" in finding for finding in findings):
            raise AssertionError(f"expected extra FinancialValue marker finding, got {findings!r}")


def test_verify_rejects_aliased_financial_value_implementor() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_sizing.rs": """
use crate::bolt_v3_numeric::{FinancialValue as FV, financial_value_private::Sealed as S};

pub struct SizingScale(f64);
impl S for SizingScale {}
impl FV for SizingScale {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue marker allowlist" in finding for finding in findings):
            raise AssertionError(f"expected FinancialValue marker finding, got {findings!r}")


def test_verify_rejects_generic_financial_value_implementor() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_sizing.rs": """
pub struct SizingScale<T>(T);
impl<T> crate::bolt_v3_numeric::financial_value_private::Sealed for SizingScale<T> {}
impl<T> crate::bolt_v3_numeric::FinancialValue for SizingScale<T> {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue marker allowlist" in finding for finding in findings):
            raise AssertionError(f"expected generic FinancialValue marker finding, got {findings!r}")


def test_verify_rejects_missing_financial_value_default_compile_guard() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(root)
        numeric_path = root / "src/bolt_v3_numeric.rs"
        numeric_path.write_text(
            numeric_path.read_text(encoding="utf-8").replace(
                "fn financial_values_do_not_implement_default()",
                "fn removed_financial_values_do_not_implement_default()",
            ),
            encoding="utf-8",
        )
        findings = VERIFIER.verify(root)
        if not any("!Default guard" in finding for finding in findings):
            raise AssertionError(f"expected missing !Default guard finding, got {findings!r}")


def test_verify_rejects_public_financial_value_field() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_maker_mu_estimator.rs": """
use crate::bolt_v3_numeric::{FinancialValue, financial_value_private};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsableMu(pub f64);
impl financial_value_private::Sealed for UsableMu {}
impl FinancialValue for UsableMu {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("UsableMu has a private f64 field" in finding for finding in findings):
            raise AssertionError(f"expected private-field finding, got {findings!r}")


def test_verify_rejects_comment_decoy_private_field() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_maker_mu_estimator.rs": """
use crate::bolt_v3_numeric::{FinancialValue, financial_value_private};

// pub struct UsableMu(f64);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsableMu(pub f64);
impl financial_value_private::Sealed for UsableMu {}
impl FinancialValue for UsableMu {}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("UsableMu has a private f64 field" in finding for finding in findings):
            raise AssertionError(f"expected comment-decoy private-field finding, got {findings!r}")


def test_verify_rejects_unsealed_financial_value_trait() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_numeric.rs": """
pub trait FinancialValue {}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability { value: f64 }
impl FinancialValue for Probability {}
impl Probability {
    pub fn new(value: f64) -> Option<Self> { sanitize_probability(value).map(|value| Self { value }) }
    pub fn clamped(value: f64) -> Option<Self> { Some(Self { value }) }
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue sealing boundary" in finding for finding in findings):
            raise AssertionError(f"expected sealing-boundary finding, got {findings!r}")


def test_verify_rejects_public_financial_value_sealing_module() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_numeric.rs": """
pub mod financial_value_private {
    pub trait Sealed {}
}

#[allow(private_bounds)]
pub trait FinancialValue: financial_value_private::Sealed {}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability { value: f64 }
impl financial_value_private::Sealed for Probability {}
impl FinancialValue for Probability {}
impl Probability {
    pub fn new(value: f64) -> Option<Self> { sanitize_probability(value).map(|value| Self { value }) }
    pub fn clamped(value: f64) -> Option<Self> { Some(Self { value }) }
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("FinancialValue sealing boundary" in finding for finding in findings):
            raise AssertionError(f"expected public sealing-module finding, got {findings!r}")


def test_verify_rejects_raw_taker_jitter_penalty_probability() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_taker_pricing.rs": """
struct TakerPricingResult {
    pub fair_probability_up: f64,
    pub fair_probability_down: f64,
}
struct TakerPricingState {
    last_lead_gap_probability: Option<Probability>,
    last_jitter_penalty_probability: Option<f64>,
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("jitter-penalty state" in finding for finding in findings):
            raise AssertionError(f"expected jitter-penalty finding, got {findings!r}")


def test_verify_rejects_typed_evidence_boundary() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/strategies/binary_oracle_edge_taker/entry_decision.rs": """
struct EntryEvaluation {
    fair_probability_up: Option<Probability>,
    uncertainty_band_probability: Option<Probability>,
}
struct EntryEvaluationLogFields {
    pub(super) fair_probability_up: Option<Probability>,
    pub(super) fair_probability_down: Option<Probability>,
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any(
            "EntryEvaluationLogFields remains an f64 evidence boundary" in finding
            for finding in findings
        ):
            raise AssertionError(f"expected evidence-boundary finding, got {findings!r}")


def test_verify_rejects_probability_tuple_construction() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_numeric.rs": """
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability"""
                + """(f64);
impl Probability {
    pub fn new(value: f64) -> Option<Self> { sanitize_probability(value).map(Probability) }
    pub fn clamped(value: f64) -> Option<Self> { Some(Probability"""
                + """(value)) }
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("tuple construction" in finding for finding in findings):
            raise AssertionError(f"expected tuple-construction finding, got {findings!r}")


def test_verify_rejects_probability_serde_derive() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_numeric.rs": """
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
pub struct Probability { value: f64 }
impl Probability {
    pub fn new(value: f64) -> Option<Self> { sanitize_probability(value).map(|value| Self { value }) }
    pub fn clamped(value: f64) -> Option<Self> { Some(Self { value }) }
}
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("serde derives" in finding for finding in findings):
            raise AssertionError(f"expected serde-derive finding, got {findings!r}")


def test_verify_rejects_decision_evidence_probability_field() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write_sources(
            root,
            {
                "src/bolt_v3_decision_evidence.rs": """
pub(crate) fn probability_evidence(probability: Probability) -> String { String::new() }
struct WireEvidence { pub fair_probability_up: Probability }
""",
            },
        )
        findings = VERIFIER.verify(root)
        if not any("decision-evidence serde fields" in finding for finding in findings):
            raise AssertionError(f"expected decision-evidence field finding, got {findings!r}")


def main() -> int:
    tests = [
        test_verify_accepts_expected_typed_surface,
        test_verify_rejects_raw_entry_evaluation_probability,
        test_verify_rejects_missing_financial_value_implementor,
        test_verify_rejects_unregistered_financial_value_implementor,
        test_verify_rejects_aliased_financial_value_implementor,
        test_verify_rejects_generic_financial_value_implementor,
        test_verify_rejects_missing_financial_value_default_compile_guard,
        test_verify_rejects_public_financial_value_field,
        test_verify_rejects_comment_decoy_private_field,
        test_verify_rejects_unsealed_financial_value_trait,
        test_verify_rejects_public_financial_value_sealing_module,
        test_verify_rejects_raw_taker_jitter_penalty_probability,
        test_verify_rejects_typed_evidence_boundary,
        test_verify_rejects_probability_tuple_construction,
        test_verify_rejects_probability_serde_derive,
        test_verify_rejects_decision_evidence_probability_field,
    ]
    for test in tests:
        test()
    print("OK: Probability typed-value pilot verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
