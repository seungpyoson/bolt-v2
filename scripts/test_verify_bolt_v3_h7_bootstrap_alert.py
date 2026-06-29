#!/usr/bin/env python3
"""Self-tests for the H7 bootstrap deferral alert verifier."""

from __future__ import annotations

import importlib.util
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_h7_bootstrap_alert.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_h7_bootstrap_alert", SCRIPT_PATH)
assert SPEC and SPEC.loader
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


GOOD_TEXTS = {
    VERIFIER.LIB_SOURCE: "pub mod bolt_v3_bootstrap_deferral_alert;\n",
    VERIFIER.BOOTSTRAP_ALERT_SOURCE: """
pub const H7_BOOTSTRAP_CONST_ALERT_OWNER: &str = "H7_ALERT_OWNER_UNASSIGNED";
pub const H7_BOOTSTRAP_CONST_TRACKING_ISSUE_URL: &str = "https://github.com/seungpyoson/bolt-v2/issues/1079";
pub const H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS: u64 = 1_785_456_000;
pub const H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;
pub const H7_BOOTSTRAP_CONST_ALERT_METRIC_NAME: &str = "bolt_v3_bootstrap_const_deferral_pre_expiry_alert_total";
pub struct BoltV3BootstrapDeferralAlertEvidence {
    pub owner: String,
    pub tracking_issue_url: String,
    pub deadline_unix_secs: u64,
    pub seconds_until_deadline: u64,
    pub alert_window_secs: u64,
    pub metric_name: String,
    pub metric_value: u64,
}
pub fn h7_bootstrap_deferral_alert_evidence() {}
fn bootstrap_const_pre_expiry_alert_fires_at_window_boundary() {}
fn bootstrap_const_pre_expiry_alert_does_not_fire_before_window() {}
""",
    VERIFIER.DECISION_EVIDENCE_SOURCE: """
pub const BOLT_V3_BOOTSTRAP_DEFERRAL_ALERT_GATE_ID: &str = "bolt_v3.bootstrap_deferral_alert";
pub const BOLT_V3_BOOTSTRAP_DEFERRAL_ALERT_RECORD_KIND: &str = "bootstrap_deferral_alert";
pub trait BoltV3BootstrapAlertSink {
    fn record_bootstrap_deferral_alert(&self);
}
impl BoltV3BootstrapAlertSink for JsonlBoltV3DecisionEvidenceWriter {
    fn record_bootstrap_deferral_alert(&self) {}
}
fn encode_bootstrap_deferral_alert_line() {}
fn encode_bootstrap_deferral_alert_line_round_trips_through_owned_line() {}
""",
    VERIFIER.MAIN_SOURCE: """
emit_h7_bootstrap_deferral_alert_if_due(&loaded)?;
JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded)?
""",
}


def test_accepts_complete_contract() -> None:
    findings = VERIFIER.verify_texts(GOOD_TEXTS)
    if findings:
        raise AssertionError(findings)


def test_rejects_missing_alert_route() -> None:
    texts = dict(GOOD_TEXTS)
    texts[VERIFIER.DECISION_EVIDENCE_SOURCE] = texts[
        VERIFIER.DECISION_EVIDENCE_SOURCE
    ].replace(
        "impl BoltV3BootstrapAlertSink for JsonlBoltV3DecisionEvidenceWriter",
        "impl MissingSink for JsonlBoltV3DecisionEvidenceWriter",
    )
    findings = VERIFIER.verify_texts(texts)
    assert any("JsonlBoltV3DecisionEvidenceWriter" in finding for finding in findings), findings


def test_rejects_missing_timing_boundary_test() -> None:
    texts = dict(GOOD_TEXTS)
    texts[VERIFIER.BOOTSTRAP_ALERT_SOURCE] = texts[
        VERIFIER.BOOTSTRAP_ALERT_SOURCE
    ].replace("fn bootstrap_const_pre_expiry_alert_does_not_fire_before_window() {}\n", "")
    findings = VERIFIER.verify_texts(texts)
    assert any("does not fire before" in finding for finding in findings), findings


def test_current_repo_has_h7_alert_contract() -> None:
    findings = VERIFIER.verify_texts(VERIFIER.read_repo_texts())
    if findings:
        raise AssertionError(findings)


def main() -> None:
    test_accepts_complete_contract()
    test_rejects_missing_alert_route()
    test_rejects_missing_timing_boundary_test()
    test_current_repo_has_h7_alert_contract()
    print("OK: H7 bootstrap deferral alert verifier self-tests passed.")


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    main()
