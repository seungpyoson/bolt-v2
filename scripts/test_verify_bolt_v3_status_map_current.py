#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 status-map verifier."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_status_map_current.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_status_map_current", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def test_script_reference_regex_matches_backticked_and_plain_script_paths() -> None:
    text = """
    Backticked: `scripts/verify_bolt_v3_status_map_current.py`
    Plain: python3 scripts/verify_bolt_v3_pure_rust_runtime.py
    Non-script: docs/scripts/not_a_verifier.py
    Non-python: scripts/not_python.txt
    """

    refs = set(VERIFIER.SCRIPT_REF_RE.findall(text))
    expected = {
        "scripts/verify_bolt_v3_status_map_current.py",
        "scripts/verify_bolt_v3_pure_rust_runtime.py",
    }
    if refs != expected:
        raise AssertionError(f"unexpected script refs: expected {sorted(expected)}, got {sorted(refs)}")


def test_missing_evidence_flags_absence_without_rejecting_negative_proof() -> None:
    missing_values = [
        "",
        "missing",
        "No test found",
        "Verifier not found",
        "not implemented",
    ]
    for value in missing_values:
        if not VERIFIER.missing_evidence(value):
            raise AssertionError(f"expected missing evidence for {value!r}")

    evidence_values = [
        "`scripts/verify_bolt_v3_pure_rust_runtime.py`",
        "`Cargo.toml` carries no PyO3 bridge",
        "No `VenueKind` enum remains in the verified Bolt-v3 core boundary.",
    ]
    for value in evidence_values:
        if VERIFIER.missing_evidence(value):
            raise AssertionError(f"expected accepted evidence for {value!r}")


def test_parse_rows_selects_status_rows_only() -> None:
    rows = VERIFIER.parse_rows(
        """
        | # | Area | Status | Source evidence | Test/verifier evidence | Gap |
        |---|---|---|---|---|---|
        | 3 | No Python runtime layer | Implemented | `Cargo.toml` | `scripts/verify.py` | none |
        | notes | not | a | status | row | here |
        """,
    )

    if len(rows) != 1 or rows[0].number != "3" or rows[0].area != "No Python runtime layer":
        raise AssertionError(f"unexpected parsed rows: {rows!r}")


def test_pure_rust_area_terms_accept_copyedits() -> None:
    area = "No Python runtime bridge"
    if not all(term in area.lower() for term in VERIFIER.PURE_RUST_AREA_TERMS):
        raise AssertionError("pure-Rust row area terms should tolerate copyedits")


def main() -> int:
    tests = [
        test_script_reference_regex_matches_backticked_and_plain_script_paths,
        test_missing_evidence_flags_absence_without_rejecting_negative_proof,
        test_parse_rows_selects_status_rows_only,
        test_pure_rust_area_terms_accept_copyedits,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 status-map verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
