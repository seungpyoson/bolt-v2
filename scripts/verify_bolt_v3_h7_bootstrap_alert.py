#!/usr/bin/env python3
"""Verify H7 bootstrap-const deferral alert plumbing is present."""

from __future__ import annotations

import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
BOOTSTRAP_ALERT_SOURCE = "src/bolt_v3_bootstrap_deferral_alert.rs"
DECISION_EVIDENCE_SOURCE = "src/bolt_v3_decision_evidence.rs"
LIB_SOURCE = "src/lib.rs"
MAIN_SOURCE = "src/main.rs"


def read_repo_texts(repo_root: Path = REPO_ROOT) -> dict[str, str]:
    texts: dict[str, str] = {}
    for relative in (
        BOOTSTRAP_ALERT_SOURCE,
        DECISION_EVIDENCE_SOURCE,
        LIB_SOURCE,
        MAIN_SOURCE,
    ):
        path = repo_root / relative
        texts[relative] = path.read_text(encoding="utf-8") if path.exists() else ""
    return texts


def require(text: str, needle: str, message: str, findings: list[str]) -> None:
    if needle not in text:
        findings.append(message)


def verify_texts(texts: dict[str, str]) -> list[str]:
    findings: list[str] = []
    alert = texts.get(BOOTSTRAP_ALERT_SOURCE, "")
    decision = texts.get(DECISION_EVIDENCE_SOURCE, "")
    lib = texts.get(LIB_SOURCE, "")
    main = texts.get(MAIN_SOURCE, "")

    require(
        lib,
        "pub mod bolt_v3_bootstrap_deferral_alert;",
        "lib.rs must expose the H7 bootstrap deferral alert module",
        findings,
    )
    require(
        alert,
        'pub const H7_BOOTSTRAP_CONST_ALERT_OWNER: &str = "H7_ALERT_OWNER_UNASSIGNED";',
        "H7 alert owner placeholder constant is missing or unnamed",
        findings,
    )
    require(
        alert,
        "pub const H7_BOOTSTRAP_CONST_TRACKING_ISSUE_URL: &str =",
        "H7 alert must carry the tracked hard-deadline issue URL",
        findings,
    )
    require(
        alert,
        '"https://github.com/seungpyoson/bolt-v2/issues/1079";',
        "H7 alert must carry the tracked hard-deadline issue URL",
        findings,
    )
    require(
        alert,
        "pub const H7_BOOTSTRAP_CONST_HARD_DEADLINE_UNIX_SECS: u64 = 1_785_456_000;",
        "H7 hard-deadline Unix timestamp must be an audited bootstrap const",
        findings,
    )
    require(
        alert,
        "pub const H7_BOOTSTRAP_CONST_PRE_EXPIRY_ALERT_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;",
        "H7 pre-expiry alert window must be an audited bootstrap const",
        findings,
    )
    require(
        alert,
        "pub const H7_BOOTSTRAP_CONST_ALERT_METRIC_NAME: &str =",
        "H7 alert metric name constant is missing",
        findings,
    )
    require(
        alert,
        '"bolt_v3_bootstrap_const_deferral_pre_expiry_alert_total";',
        "H7 alert metric name constant is missing",
        findings,
    )
    require(
        alert,
        "pub struct BoltV3BootstrapDeferralAlertEvidence",
        "H7 alert evidence payload type is missing",
        findings,
    )
    for field in (
        "pub owner: String,",
        "pub tracking_issue_url: String,",
        "pub deadline_unix_secs: u64,",
        "pub seconds_until_deadline: u64,",
        "pub alert_window_secs: u64,",
        "pub metric_name: String,",
        "pub metric_value: u64,",
    ):
        require(alert, field, f"H7 alert evidence field missing: {field}", findings)
    require(
        alert,
        "pub fn h7_bootstrap_deferral_alert_evidence(",
        "H7 alert timing helper is missing",
        findings,
    )
    require(
        alert,
        "fn bootstrap_const_pre_expiry_alert_fires_at_window_boundary()",
        "H7 deterministic test must prove the alert fires at the window boundary",
        findings,
    )
    require(
        alert,
        "fn bootstrap_const_pre_expiry_alert_does_not_fire_before_window()",
        "H7 deterministic test must prove the alert does not fire before the window",
        findings,
    )
    require(
        decision,
        "pub const BOLT_V3_BOOTSTRAP_DEFERRAL_ALERT_GATE_ID: &str = \"bolt_v3.bootstrap_deferral_alert\";",
        "H7 decision-evidence gate id is missing",
        findings,
    )
    require(
        decision,
        "pub const BOLT_V3_BOOTSTRAP_DEFERRAL_ALERT_RECORD_KIND: &str = \"bootstrap_deferral_alert\";",
        "H7 decision-evidence record kind is missing",
        findings,
    )
    require(
        decision,
        "pub trait BoltV3BootstrapAlertSink",
        "H7 alert sink trait is missing",
        findings,
    )
    require(
        decision,
        "fn record_bootstrap_deferral_alert(",
        "H7 alert sink record method is missing",
        findings,
    )
    require(
        decision,
        "impl BoltV3BootstrapAlertSink for JsonlBoltV3DecisionEvidenceWriter",
        "H7 alert route to JsonlBoltV3DecisionEvidenceWriter is missing",
        findings,
    )
    require(
        decision,
        "fn encode_bootstrap_deferral_alert_line(",
        "H7 alert JSONL encoder is missing",
        findings,
    )
    require(
        decision,
        "fn encode_bootstrap_deferral_alert_line_round_trips_through_owned_line()",
        "H7 negative routing/encoding test is missing",
        findings,
    )
    require(
        main,
        "emit_h7_bootstrap_deferral_alert_if_due(&loaded)?;",
        "ops launch must call the H7 alert route before starting the node",
        findings,
    )
    require(
        main,
        "JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded)?",
        "H7 alert route must use the production JSONL evidence sink, not stdout-only logging",
        findings,
    )
    return findings


def main() -> int:
    findings = verify_texts(read_repo_texts())
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1
    print("OK: H7 bootstrap deferral alert plumbing is present.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
