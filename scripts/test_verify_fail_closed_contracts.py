#!/usr/bin/env python3
"""Self-tests for the fail-closed contract verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_fail_closed_contracts.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_fail_closed_contracts", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel_path: str, text: str) -> None:
    path = root / rel_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text).lstrip(), encoding="utf-8")


def line_of(text: str, needle: str) -> int:
    for index, line in enumerate(textwrap.dedent(text).lstrip().splitlines(), start=1):
        if needle in line:
            return index
    raise AssertionError(f"missing {needle!r}")


def config_text(*, exceptions: str = "[]") -> str:
    return f"""
    [fail_closed_contracts]
    version = 1
    include_globs = ["pkg/**/*.py"]
    exclude_globs = []
    broad_exception_names = ["Exception", "BaseException"]
    logging_call_names = ["logger.exception", "logging.exception"]
    sentinel_return_shapes = ["none", "empty_list", "empty_dict", "empty_string", "false"]
    exceptions = {exceptions}

    [fail_closed_contracts.rule_ids]
    bare_except_pass = "FLC001"
    broad_except_pass = "FLC002"
    broad_sentinel_return = "FLC003"
    broad_logged_sentinel_return = "FLC004"
    central_exception_invalid = "FLC900"
    """


def write_config(root: Path, *, exceptions: str = "[]") -> None:
    write_file(root, "ci/fail-closed-contracts.toml", config_text(exceptions=exceptions))


def collect(root: Path) -> list[str]:
    verifier = load_verifier()
    return verifier.collect_findings(root, root / "ci" / "fail-closed-contracts.toml")


def test_bad_fixtures_fail_with_stable_rule_ids() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/bare_pass.py",
            """
            def load_contract():
                try:
                    return parse()
                except:
                    pass
            """,
        )
        write_file(
            root,
            "pkg/broad_pass.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    pass
            """,
        )
        write_file(
            root,
            "pkg/broad_sentinel.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return None
            """,
        )
        write_file(
            root,
            "pkg/broad_logged_sentinel.py",
            """
            def load_contract(logger):
                try:
                    return parse()
                except Exception:
                    logger.exception("contract failed")
                    return False
            """,
        )

        findings = collect(root)

    assert {finding.split(":", maxsplit=1)[0] for finding in findings} == {
        "FLC001",
        "FLC002",
        "FLC003",
        "FLC004",
    }


def test_precise_exception_fixture_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/precise.py",
            """
            def load_contract():
                try:
                    return parse()
                except ValueError:
                    return None
            """,
        )

        findings = collect(root)

    assert findings == []


def test_conditional_precise_exception_expression_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/conditional_precise.py",
            """
            def load_contract():
                try:
                    return parse()
                except TOMLDecodeError if HAS_TOML else ValueError:
                    return None
            """,
        )

        findings = collect(root)

    assert findings == []


def test_classified_degradation_requires_valid_central_exception() -> None:
    source = """
    def load_optional_contract():
        try:
            return parse_optional()
        except Exception:
            return []
    """
    handler_line = line_of(source, "except Exception:")
    exception = (
        "[{ path = \"pkg/degradation.py\", "
        f"line = {handler_line}, "
        "rule_id = \"FLC003\", "
        "classification = \"classified_degradation\", "
        "reason = \"Optional development report keeps fail-closed runtime paths separate.\" }]"
    )
    invalid_exception = (
        "[{ path = \"pkg/degradation.py\", "
        f"line = {handler_line}, "
        "rule_id = \"FLC003\", "
        "classification = \"\", "
        "reason = \"missing classification\" }]"
    )

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(root, "pkg/degradation.py", source)
        unexcepted = collect(root)

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root, exceptions=invalid_exception)
        write_file(root, "pkg/degradation.py", source)
        invalid = collect(root)

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root, exceptions=exception)
        write_file(root, "pkg/degradation.py", source)
        allowed = collect(root)

    assert any(finding.startswith("FLC003:pkg/degradation.py:4:") for finding in unexcepted)
    assert any(finding.startswith("FLC003:pkg/degradation.py:4:") for finding in invalid)
    assert any(finding.startswith("FLC900:") for finding in invalid)
    assert allowed == []


def test_invalid_central_exception_disables_all_exception_suppression() -> None:
    source = """
    def load_contract():
        try:
            return parse()
        except Exception:
            return []
    """
    handler_line = line_of(source, "except Exception:")
    exceptions = (
        "[{ path = \"pkg/degradation.py\", "
        f"line = {handler_line}, "
        "rule_id = \"FLC003\", "
        "classification = \"classified_degradation\", "
        "reason = \"Development-only report has explicit degraded-state ownership.\" }, "
        "{ path = \"pkg/other.py\", "
        f"line = {handler_line}, "
        "rule_id = \"FLC003\", "
        "classification = \"\", "
        "reason = \"invalid ledger entry\" }]"
    )

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root, exceptions=exceptions)
        write_file(root, "pkg/degradation.py", source)
        findings = collect(root)

    assert any(finding.startswith("FLC900:") for finding in findings)
    assert any(finding.startswith("FLC003:pkg/degradation.py:4:") for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/bad.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return ""
            """,
        )

        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--root",
                str(root),
                "--config",
                str(root / "ci" / "fail-closed-contracts.toml"),
            ],
            cwd=REPO_ROOT,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    assert result.returncode == 1
    assert "FAIL: fail-closed contract violations" in result.stderr
    assert "FLC003:pkg/bad.py:4:" in result.stderr


def main() -> int:
    tests = [
        test_bad_fixtures_fail_with_stable_rule_ids,
        test_precise_exception_fixture_passes,
        test_conditional_precise_exception_expression_passes,
        test_classified_degradation_requires_valid_central_exception,
        test_invalid_central_exception_disables_all_exception_suppression,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: fail-closed contract verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
