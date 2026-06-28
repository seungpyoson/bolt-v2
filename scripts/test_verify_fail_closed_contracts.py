#!/usr/bin/env python3
"""Self-tests for the fail-closed contract verifier."""

from __future__ import annotations

import functools
import importlib.util
import subprocess
import sys
import tempfile
import textwrap
from datetime import date
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_fail_closed_contracts.py"
TODAY = date(2026, 6, 29)
REPO_CONTRACT = REPO_ROOT / "ci" / "fail-closed-contracts.toml"
REPO_SCAN = '[scan]\ninclude = ["scripts/verify_*.py"]\nexclude = []'
FIXTURE_SCAN = '[scan]\ninclude = ["fixtures/**/*.py"]\nexclude = []'


CONTRACT_TOML = REPO_CONTRACT.read_text(encoding="utf-8").replace(REPO_SCAN, FIXTURE_SCAN)

EMPTY_EXCEPTIONS_TOML = """
[exceptions]
items = []
"""


@functools.cache
def load_verifier():
    assert SCRIPT.exists(), f"verifier script missing: {SCRIPT}"
    spec = importlib.util.spec_from_file_location("verify_fail_closed_contracts", SCRIPT)
    assert spec is not None and spec.loader is not None, f"failed to load {SCRIPT}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text).lstrip(), encoding="utf-8")


def write_contract(root: Path, text: str = CONTRACT_TOML) -> Path:
    write_file(root, "ci/fail-closed-contracts.toml", text)
    return root / "ci/fail-closed-contracts.toml"


def write_exceptions(root: Path, text: str = EMPTY_EXCEPTIONS_TOML) -> Path:
    write_file(root, "ci/fail-closed-exceptions.toml", text)
    return root / "ci/fail-closed-exceptions.toml"


def write_justfile(root: Path, *, command: str | None = None) -> None:
    verifier_command = command or (
        "python3 scripts/verify_fail_closed_contracts.py --contract "
        "ci/fail-closed-contracts.toml --exceptions ci/fail-closed-exceptions.toml"
    )
    write_file(
        root,
        "justfile",
        f"""
        source-fence-static-inner:
            python3 scripts/test_verify_fail_closed_contracts.py
            {verifier_command}
        """,
    )


def write_precise_fixture(root: Path) -> None:
    write_file(
        root,
        "fixtures/precise.py",
        """
        def parse(value):
            try:
                return int(value)
            except ValueError:
                return None
        """,
    )


def write_degradation_fixture(root: Path) -> None:
    write_file(
        root,
        "fixtures/degraded.py",
        """
        def degraded(logger):
            try:
                load()
            except Exception:
                logger.warning("degraded")
                return None
        """,
    )


def scan_fixture(root: Path) -> list[str]:
    verifier = load_verifier()
    return verifier.scan_root(
        root,
        contract_path=root / "ci/fail-closed-contracts.toml",
        exceptions_path=root / "ci/fail-closed-exceptions.toml",
        today=TODAY,
    )


def assert_finding(findings: list[str], expected: str) -> None:
    assert any(expected in finding for finding in findings), (
        f"expected finding containing {expected!r}, got {findings!r}"
    )


def test_precise_exception_handler_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_exceptions(root)
        write_justfile(root)
        write_precise_fixture(root)

        assert scan_fixture(root) == []


def test_blocked_patterns_map_to_stable_rule_ids() -> None:
    cases = {
        "FC001": """
            def bare_pass():
                try:
                    load()
                except:
                    pass
        """,
        "FC002": """
            def exception_pass():
                try:
                    load()
                except Exception:
                    pass
        """,
        "FC003": """
            def sentinel_return():
                try:
                    load()
                except Exception:
                    return []
        """,
        "FC004": """
            def logged_sentinel_return(logger):
                try:
                    load()
                except Exception:
                    logger.warning("degraded")
                    return False
        """,
    }
    for rule_id, source in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_contract(root)
            write_exceptions(root)
            write_justfile(root)
            write_file(root, f"fixtures/{rule_id.lower()}.py", source)

            findings = scan_fixture(root)

        assert_finding(findings, rule_id)


def test_classified_degradation_requires_central_exception() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_exceptions(root)
        write_justfile(root)
        write_degradation_fixture(root)

        findings = scan_fixture(root)

    assert_finding(findings, "FC004")
    assert_finding(findings, "missing central exception")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_justfile(root)
        write_degradation_fixture(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            """
            [[exceptions.items]]
            rule_id = "FC004"
            path = "fixtures/degraded.py"
            line = 4
            classification = "classified_degradation"
            expires_on = "2999-12-31"
            reason = "fixture exercises reviewed degradation"
            """,
        )

        assert scan_fixture(root) == []


def test_call_payload_normalization_keeps_return_sentinel_exact() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_exceptions(root)
        write_justfile(root)
        write_file(
            root,
            "fixtures/log_payload.py",
            """
            def degraded(audit):
                try:
                    load()
                except Exception:
                    audit.error("different message", extra={"code": "x"})
                    return None
            """,
        )

        findings = scan_fixture(root)

    assert_finding(findings, "FC004")
    assert_finding(findings, "missing central exception")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_exceptions(root)
        write_justfile(root)
        write_file(
            root,
            "fixtures/non_sentinel.py",
            """
            def non_sentinel(logger):
                try:
                    load()
                except Exception:
                    logger.warning("degraded")
                    return 0
            """,
        )

        assert scan_fixture(root) == []


def test_contract_file_absent_empty_invalid_unavailable_fail_closed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_exceptions(root)
        missing = verifier.scan_root(root, contract_path=root / "missing.toml", exceptions_path=root / "ci/fail-closed-exceptions.toml", today=TODAY)
        assert_finding(missing, "contract file absent")

        write_file(root, "empty.toml", "")
        empty = verifier.scan_root(root, contract_path=root / "empty.toml", exceptions_path=root / "ci/fail-closed-exceptions.toml", today=TODAY)
        assert_finding(empty, "contract file empty")

        write_file(root, "invalid.toml", "[")
        invalid = verifier.scan_root(root, contract_path=root / "invalid.toml", exceptions_path=root / "ci/fail-closed-exceptions.toml", today=TODAY)
        assert_finding(invalid, "contract file invalid")

        unavailable = verifier.scan_root(root, contract_path=root, exceptions_path=root / "ci/fail-closed-exceptions.toml", today=TODAY)
        assert_finding(unavailable, "contract file unavailable")


def test_exceptions_file_absent_empty_invalid_unavailable_fail_closed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        contract = write_contract(root)
        missing = verifier.scan_root(root, contract_path=contract, exceptions_path=root / "missing.toml", today=TODAY)
        assert_finding(missing, "exceptions file absent")

        write_file(root, "empty.toml", "")
        empty = verifier.scan_root(root, contract_path=contract, exceptions_path=root / "empty.toml", today=TODAY)
        assert_finding(empty, "exceptions file empty")

        write_file(root, "invalid.toml", "[")
        invalid = verifier.scan_root(root, contract_path=contract, exceptions_path=root / "invalid.toml", today=TODAY)
        assert_finding(invalid, "exceptions file invalid")

        unavailable = verifier.scan_root(root, contract_path=contract, exceptions_path=root, today=TODAY)
        assert_finding(unavailable, "exceptions file unavailable")


def test_duplicate_ambiguous_rule_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root, CONTRACT_TOML.replace('id = "FC001"', 'id = "FC002"', 1))
        write_exceptions(root)
        write_justfile(root)
        write_precise_fixture(root)

        findings = scan_fixture(root)

    assert_finding(findings, "duplicate/ambiguous rule")


def test_duplicate_ambiguous_exception_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_justfile(root)
        write_degradation_fixture(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            """
            [[exceptions.items]]
            rule_id = "FC004"
            path = "fixtures/degraded.py"
            line = 4
            classification = "classified_degradation"
            expires_on = "2999-12-31"
            reason = "first"

            [[exceptions.items]]
            rule_id = "FC004"
            path = "fixtures/degraded.py"
            line = 4
            classification = "classified_degradation"
            expires_on = "2999-12-31"
            reason = "second"
            """,
        )

        findings = scan_fixture(root)

    assert_finding(findings, "duplicate/ambiguous exception")


def test_stale_expired_exception_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_justfile(root)
        write_degradation_fixture(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            """
            [[exceptions.items]]
            rule_id = "FC004"
            path = "fixtures/degraded.py"
            line = 4
            classification = "classified_degradation"
            expires_on = "2026-06-28"
            reason = "expired"
            """,
        )

        findings = scan_fixture(root)

    assert_finding(findings, "stale/expired exception")


def test_source_fence_command_mismatch_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_contract(root)
        write_exceptions(root)
        write_justfile(root, command="python3 scripts/verify_fail_closed_contracts.py --contract ci/other.toml --exceptions ci/fail-closed-exceptions.toml")
        write_precise_fixture(root)

        findings = scan_fixture(root)

    assert_finding(findings, "source-fence command mismatch")


def test_cli_requires_explicit_contract() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT)],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    assert result.returncode == 2
    assert "--contract" in result.stderr


def main() -> int:
    tests = [
        test_precise_exception_handler_passes,
        test_blocked_patterns_map_to_stable_rule_ids,
        test_classified_degradation_requires_central_exception,
        test_call_payload_normalization_keeps_return_sentinel_exact,
        test_contract_file_absent_empty_invalid_unavailable_fail_closed,
        test_exceptions_file_absent_empty_invalid_unavailable_fail_closed,
        test_duplicate_ambiguous_rule_fails_closed,
        test_duplicate_ambiguous_exception_fails_closed,
        test_stale_expired_exception_fails_closed,
        test_source_fence_command_mismatch_fails_closed,
        test_cli_requires_explicit_contract,
    ]
    for test in tests:
        test()
    print("OK: fail-closed contract verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
