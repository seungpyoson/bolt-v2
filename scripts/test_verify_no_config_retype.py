#!/usr/bin/env python3
"""Self-tests for the no-config-retype fence."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import textwrap
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_no_config_retype.py")
SPEC = importlib.util.spec_from_file_location("verify_no_config_retype", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text).lstrip(), encoding="utf-8")


def test_missing_governed_artifact_is_loud_error() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/missing.toml",),
            strict_paths=frozenset(),
            ratchet_baseline_count=0,
        )
        if not any("governed config artifact missing: ci/missing.toml" in finding for finding in findings):
            raise AssertionError(f"expected missing-artifact finding, got {findings!r}")


def test_unlisted_governed_artifact_is_loud_error() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "ci" / "new-governed-config.toml", """
        schema_version = 1
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset(),
            ratchet_baseline_count=0,
        )
        if not any("governed config artifact unlisted: ci/new-governed-config.toml" in finding for finding in findings):
            raise AssertionError(f"expected unlisted-artifact finding, got {findings!r}")


def test_flatten_string_values_rejects_unhandled_container() -> None:
    try:
        list(VERIFIER.flatten_string_values({"outer": ("not", "toml")}, "ci_provenance"))
    except TypeError as exc:
        if "unhandled container type" not in str(exc):
            raise AssertionError(f"unexpected error: {exc}") from exc
    else:
        raise AssertionError("flatten_string_values must reject unhandled containers")


def test_flatten_string_values_ignores_numbers_and_bools() -> None:
    values = list(
        VERIFIER.flatten_string_values(
            {"text": "gate", "enabled": True, "count": 3, "ratio": 1.5},
            "ci_provenance",
        )
    )
    if values != ["gate"]:
        raise AssertionError(f"numbers and bools must be ignored deliberately, got {values!r}")


def test_strict_touched_file_rejects_unregistered_retype() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "changed.py", """
        VALUE = "single-source-value"
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/changed.py"}),
            ratchet_baseline_count=0,
        )
        if not any("strict no-config-retype violation" in finding for finding in findings):
            raise AssertionError(f"expected strict violation, got {findings!r}")


def test_registered_payload_allows_strict_retype_with_reason() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "changed.py", """
        VALUE = "single-source-value"
        """)
        registration = VERIFIER.RegisteredRetype(
            path="scripts/changed.py",
            value="single-source-value",
            reason="fixture pins a representative config value",
        )
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/changed.py"}),
            ratchet_baseline_count=0,
            registered_retypes=(registration,),
        )
        if findings:
            raise AssertionError(f"registered retype should be accepted, got {findings!r}")


def test_wildcard_registered_payload_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "changed.py", """
        VALUE = "single-source-value"
        """)
        registration = VERIFIER.RegisteredRetype(
            path="*",
            value="single-source-value",
            reason="wildcards would bypass path-specific review",
        )
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/changed.py"}),
            ratchet_baseline_count=0,
            registered_retypes=(registration,),
        )
        if not any("wildcard" in finding for finding in findings):
            raise AssertionError(f"expected wildcard rejection, got {findings!r}")


def test_extensionless_strict_script_is_scanned() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "cargo-shim", """
        VALUE = "single-source-value"
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/cargo-shim"}),
            ratchet_baseline_count=0,
        )
        if not any("scripts/cargo-shim" in finding for finding in findings):
            raise AssertionError(f"expected extensionless script violation, got {findings!r}")


def test_registered_payload_assignment_skip_is_limited_to_verifier_file() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "changed.py", """
        REGISTERED_RETYPE_PAYLOADS = (
            "single-source-value",
        )
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/changed.py"}),
            ratchet_baseline_count=0,
        )
        if not any("scripts/changed.py" in finding for finding in findings):
            raise AssertionError(f"expected non-verifier assignment violation, got {findings!r}")


def test_ratchet_mode_fails_when_unregistered_count_exceeds_baseline() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "old.py", """
        VALUE = "single-source-value"
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset(),
            ratchet_baseline_count=0,
        )
        if not any("ratchet no-config-retype count increased" in finding for finding in findings):
            raise AssertionError(f"expected ratchet finding, got {findings!r}")


def test_registered_payload_literals_do_not_self_violate() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        write(root / "ci" / "github-actions-runners.toml", """
        [ci_provenance]
        protected_value = "single-source-value"
        """)
        write(root / "scripts" / "verify_no_config_retype.py", """
        REGISTERED_RETYPE_PAYLOADS = (
            "single-source-value",
        )
        """)
        findings = VERIFIER.collect_findings(
            root,
            governed_config_artifacts=("ci/github-actions-runners.toml",),
            strict_paths=frozenset({"scripts/verify_no_config_retype.py"}),
            ratchet_baseline_count=0,
        )
        if findings:
            raise AssertionError(f"registered payload literals should be control-plane data, got {findings!r}")


def main() -> int:
    tests = (
        test_missing_governed_artifact_is_loud_error,
        test_unlisted_governed_artifact_is_loud_error,
        test_flatten_string_values_rejects_unhandled_container,
        test_flatten_string_values_ignores_numbers_and_bools,
        test_strict_touched_file_rejects_unregistered_retype,
        test_registered_payload_allows_strict_retype_with_reason,
        test_wildcard_registered_payload_is_rejected,
        test_extensionless_strict_script_is_scanned,
        test_registered_payload_assignment_skip_is_limited_to_verifier_file,
        test_ratchet_mode_fails_when_unregistered_count_exceeds_baseline,
        test_registered_payload_literals_do_not_self_violate,
    )
    for test in tests:
        test()
    print("OK: no-config-retype verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
