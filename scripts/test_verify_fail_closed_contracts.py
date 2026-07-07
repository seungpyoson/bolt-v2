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
INPUT_ERROR_PREFIX = "FAIL: fail-closed contract verifier input error:"


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


def write_justfile(root: Path, *, include_harness: bool = True, extra_commands: tuple[str, ...] = ()) -> None:
    commands = []
    if include_harness:
        commands.append("python3 scripts/run_fences.py")
    commands.extend(extra_commands)
    command_block = "\n".join(f"    {command}" for command in commands)
    write_file(
        root,
        "justfile",
        f"source-fence-static-inner:\n{command_block}\n",
    )


def config_text(
    *,
    version: str = "1",
    exclude_globs: str = "[]",
    logging_call_names: str = '["logger.exception", "logging.exception"]',
    extra_settings: str = "",
) -> str:
    return f"""
    [fail_closed_contracts]
    version = {version}
    include_globs = ["pkg/**/*.py"]
    exclude_globs = {exclude_globs}
    broad_exception_names = ["Exception", "BaseException"]
    logging_call_names = {logging_call_names}
    {extra_settings}

    [fail_closed_contracts.rule_ids]
    bare_except_pass = "FLC001"
    broad_except_pass = "FLC002"
    broad_sentinel_return = "FLC003"
    broad_logged_sentinel_return = "FLC004"
    """


def exceptions_text(*entries: str, version: str = "1") -> str:
    body = "\n".join(entries)
    return f"""
    [fail_closed_exceptions]
    version = {version}
    {body}
    """


def exception_entry(*, rule_id: str = "FLC003", path: str, line: str = "4",
                    reason: str = '"Classified degradation fixture."') -> str:
    return f"""
    [[fail_closed_exceptions.exceptions]]
    rule_id = "{rule_id}"
    path = "{path}"
    line = {line}
    reason = {reason}
    """


def write_config(root: Path) -> None:
    write_file(root, "ci/fail-closed-contracts.toml", config_text())
    write_justfile(root)


def collect(root: Path) -> list[str]:
    verifier = load_verifier()
    return verifier.collect_findings(root, root / "ci" / "fail-closed-contracts.toml")


def test_empty_selected_paths_fail_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            exceptions_text(
                exception_entry(
                    path="pkg/missing.py",
                    reason='"Fixture proves empty selected paths suppress stale exception noise."',
                )
            ),
        )

        findings = collect(root)

    assert findings == ["fail-closed contract selected paths: enforcement set is empty"], findings


def test_empty_selected_paths_suppresses_source_fence_wiring_noise() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "ci/fail-closed-contracts.toml", config_text())

        findings = collect(root)

    assert findings == ["fail-closed contract selected paths: enforcement set is empty"], findings


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
            "pkg/broad_return.py",
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
            "pkg/broad_logged_return.py",
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

    assert findings == [], findings


def test_project_exception_named_exception_is_not_broad() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/project_exception.py",
            """
            def load_contract(mymodule):
                try:
                    return parse()
                except mymodule.Exception:
                    return None
            """,
        )

        findings = collect(root)

    assert findings == [], findings


def test_builtins_exception_qualified_name_is_broad() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/builtins_exception.py",
            """
            def load_contract(builtins):
                try:
                    return parse()
                except builtins.Exception:
                    return None
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/builtins_exception.py:4:") for finding in findings)


def test_bare_return_fails_as_return_from_catch_all() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/bare_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/bare_return.py:4:") for finding in findings)


def test_bare_except_sentinel_return_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/bare_except_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except:
                    return None
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/bare_except_return.py:4:") for finding in findings)


def test_broad_except_tuple_sentinel_return_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/tuple_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception as exc:
                    return None, str(exc)
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/tuple_return.py:4:") for finding in findings)


def test_self_logger_exception_classifies_logged_sentinel_return() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/self_logger.py",
            """
            def load_contract(self):
                try:
                    return parse()
                except Exception:
                    self.logger.exception("contract failed")
                    return None
            """,
        )

        findings = collect(root)

    assert findings == [
        "FLC004:pkg/self_logger.py:4: catch-all exception handler logs then returns a sentinel"
    ], findings


def test_chained_logger_exception_classifies_logged_sentinel_return() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/chained_logger.py",
            """
            def load_contract(get_logger):
                try:
                    return parse()
                except Exception:
                    get_logger().exception("contract failed")
                    return None
            """,
        )

        findings = collect(root)

    assert findings == [
        "FLC004:pkg/chained_logger.py:4: catch-all exception handler logs then returns a sentinel"
    ], findings


def test_stdlib_getlogger_exception_classifies_logged_sentinel_return() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/stdlib_getlogger.py",
            """
            import logging

            def load_contract():
                try:
                    return parse()
                except Exception:
                    logging.getLogger(__name__).exception("contract failed")
                    return None
            """,
        )

        findings = collect(root)

    assert findings == [
        "FLC004:pkg/stdlib_getlogger.py:6: catch-all exception handler logs then returns a sentinel"
    ], findings


def test_non_logger_exception_method_does_not_classify_as_logged() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/non_logger_exception_method.py",
            """
            def load_contract(client):
                try:
                    return parse()
                except Exception:
                    client.exception("domain method, not logging")
                    return None
            """,
        )

        findings = collect(root)

    assert findings == [
        "FLC003:pkg/non_logger_exception_method.py:4: catch-all exception handler returns a sentinel"
    ], findings


def test_conditional_sentinel_return_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/conditional_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return [] if degraded else parsed
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/conditional_return.py:4:") for finding in findings)


def test_boolean_sentinel_return_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/boolean_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return parsed or []
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/boolean_return.py:4:") for finding in findings)


def test_conditional_broad_exception_type_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/conditional_except.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception if enabled else ValueError:
                    return None
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC003:pkg/conditional_except.py:4:") for finding in findings)


def test_repeated_pass_fails_as_silent_catch_all() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/repeated_pass.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    pass
                    pass
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC002:pkg/repeated_pass.py:4:") for finding in findings)


def test_ellipsis_fails_as_silent_catch_all() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/ellipsis.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    ...
            """,
        )

        findings = collect(root)

    assert any(finding.startswith("FLC002:pkg/ellipsis.py:4:") for finding in findings)


def test_nested_function_return_inside_handler_is_not_handler_return() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/nested_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    def helper():
                        return None
                    recover()
            """,
        )

        findings = collect(root)

    assert findings == [], findings


def test_nested_precise_exception_return_inside_handler_is_not_outer_handler_return() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/nested_precise_except_return.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    try:
                        recover()
                    except ValueError:
                        return None
                    raise
            """,
        )

        findings = collect(root)

    assert findings == [], findings


def test_config_excludes_nested_test_files() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "ci/fail-closed-contracts.toml",
            config_text(exclude_globs='["pkg/test_*.py", "pkg/**/test_*.py"]'),
        )
        write_justfile(root)
        write_file(
            root,
            "pkg/test_top_level.py",
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
            "pkg/subdir/test_bad.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return None
            """,
        )
        write_file(root, "pkg/clean.py", "def load_contract():\n    return parse()\n")

        findings = collect(root)

    assert findings == [], findings


def test_repo_config_excludes_nested_script_test_files() -> None:
    verifier = load_verifier()
    config = verifier.load_config(REPO_ROOT / "ci" / "fail-closed-contracts.toml")

    assert "scripts/test_*.py" in config.exclude_globs
    assert "scripts/**/test_*.py" in config.exclude_globs


def test_source_fence_static_wiring_is_required() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_justfile(root, include_harness=False)
        write_file(
            root,
            "pkg/precise.py",
            """
            def load_contract(value):
                try:
                    return int(value)
                except ValueError:
                    return None
            """,
        )

        findings = collect(root)

    expected = "source-fence-static-inner must contain only python3 scripts/run_fences.py"
    assert findings == [expected], findings


def test_source_fence_static_wiring_rejects_appended_command() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_justfile(root, extra_commands=("python3 scripts/verify_fail_closed_contracts.py",))
        write_file(
            root,
            "pkg/precise.py",
            """
            def load_contract(value):
                try:
                    return int(value)
                except ValueError:
                    return None
            """,
        )

        findings = collect(root)

    expected = "source-fence-static-inner must contain only python3 scripts/run_fences.py"
    assert findings == [expected], findings


def test_repo_source_fence_static_wiring_is_current() -> None:
    verifier = load_verifier()

    findings = verifier.source_fence_wiring_findings(REPO_ROOT)

    assert findings == [], findings


def test_source_fence_static_parser_requires_indented_commands() -> None:
    verifier = load_verifier()
    justfile_text = """
    source-fence-static-inner:
        python3 scripts/run_fences.py
    python3 scripts/verify_fail_closed_contracts.py
    """

    commands = verifier.source_fence_static_commands(textwrap.dedent(justfile_text).lstrip())

    assert commands == ("python3 scripts/run_fences.py",), commands


def test_source_fence_static_parser_ignores_inline_comments() -> None:
    verifier = load_verifier()
    justfile_text = """
    source-fence-static-inner:
        python3 scripts/run_fences.py  # static fence harness
    """

    commands = verifier.source_fence_static_commands(textwrap.dedent(justfile_text).lstrip())

    assert commands == ("python3 scripts/run_fences.py",), commands


def test_source_fence_static_parser_stops_at_top_level_non_recipe_lines() -> None:
    verifier = load_verifier()
    justfile_text = """
    source-fence-static-inner:
        python3 scripts/run_fences.py
    import "other.just"
        python3 scripts/verify_fail_closed_contracts.py
    """

    commands = verifier.source_fence_static_commands(textwrap.dedent(justfile_text).lstrip())

    assert commands == ("python3 scripts/run_fences.py",), commands


def test_source_fence_static_parser_ignores_malformed_top_level_colon_lines() -> None:
    verifier = load_verifier()
    justfile_text = """
    source-fence-static-inner:
        python3 scripts/run_fences.py
    := value
        python3 scripts/verify_fail_closed_contracts.py
    """

    commands = verifier.source_fence_static_commands(textwrap.dedent(justfile_text).lstrip())

    assert commands == ("python3 scripts/run_fences.py",), commands


def test_classified_degradation_requires_central_exception() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/degraded.py",
            """
            def load_contract():
                try:
                    return parse()
                except Exception as exc:
                    return None, str(exc)
            """,
        )

        findings_without_exception = collect(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            exceptions_text(
                exception_entry(
                    path="pkg/degraded.py",
                    reason='"Classified degradation: caller receives null payload plus explicit error."',
                )
            ),
        )
        findings_with_exception = collect(root)

    assert any(finding.startswith("FLC003:pkg/degraded.py:4:") for finding in findings_without_exception)
    assert findings_with_exception == [], findings_with_exception


def test_stale_central_exception_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(root, "pkg/clean.py", "def load_contract():\n    return parse()\n")
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            exceptions_text(
                exception_entry(
                    path="pkg/missing.py",
                    reason='"Fixture proves stale exception detection."',
                )
            ),
        )

        findings = collect(root)

    assert findings == ["FLC000:pkg/missing.py:4: stale fail-closed exception for FLC003"], findings


def test_dot_prefixed_exception_path_is_normalized() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/degraded.py",
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
            "ci/fail-closed-exceptions.toml",
            exceptions_text(
                exception_entry(path="./pkg/degraded.py")
            ),
        )

        findings = collect(root)

    assert findings == [], findings


def test_exception_config_rejects_invalid_shapes() -> None:
    cases = [
        (
            "bad rule id",
            exceptions_text(exception_entry(rule_id="FLC999", path="pkg/degraded.py")),
            "exception rule_id",
        ),
        (
            "duplicate entry",
            exceptions_text(
                exception_entry(path="pkg/degraded.py"),
                exception_entry(path="pkg/degraded.py"),
            ),
            "duplicate fail-closed exception",
        ),
        (
            "absolute path",
            exceptions_text(exception_entry(path="/tmp/degraded.py")),
            "repository-relative path",
        ),
        (
            "parent path",
            exceptions_text(exception_entry(path="pkg/../degraded.py")),
            "repository-relative path",
        ),
        (
            "zero line",
            exceptions_text(exception_entry(path="pkg/degraded.py", line="0")),
            "positive integer",
        ),
        (
            "negative line",
            exceptions_text(exception_entry(path="pkg/degraded.py", line="-1")),
            "positive integer",
        ),
        (
            "boolean line",
            exceptions_text(exception_entry(path="pkg/degraded.py", line="true")),
            "positive integer",
        ),
        (
            "unsupported version",
            exceptions_text(exception_entry(path="pkg/degraded.py"), version="2"),
            "exceptions version",
        ),
        (
            "boolean version",
            exceptions_text(exception_entry(path="pkg/degraded.py"), version="true"),
            "exceptions version",
        ),
        (
            "empty reason",
            exceptions_text(exception_entry(path="pkg/degraded.py", reason='""')),
            "non-empty string",
        ),
        (
            "non-list exceptions",
            """
            [fail_closed_exceptions]
            version = 1
            exceptions = "not a list"
            """,
            "exceptions must be a list of tables",
        ),
    ]
    for label, exceptions_config, expected_message in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_config(root)
            write_file(root, "ci/fail-closed-exceptions.toml", exceptions_config)

            try:
                collect(root)
            except TypeError as exc:
                assert expected_message in str(exc), (label, exc)
            else:
                raise AssertionError(f"accepted invalid fail-closed exception config: {label}")


def test_exception_line_drift_fails_closed_both_ways() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "pkg/drifted.py",
            """
            HEADER = 1
            def load_contract():
                try:
                    return parse()
                except Exception:
                    return None
            """,
        )
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            exceptions_text(exception_entry(path="pkg/drifted.py", line="4")),
        )

        findings = collect(root)

    assert findings == [
        "FLC003:pkg/drifted.py:5: catch-all exception handler returns a sentinel",
        "FLC000:pkg/drifted.py:4: stale fail-closed exception for FLC003",
    ], findings


def test_cli_reports_config_errors_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "ci/fail-closed-contracts.toml", "not = [valid")

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

    assert result.returncode == 2
    assert INPUT_ERROR_PREFIX in result.stderr
    assert "Traceback" not in result.stderr


def test_cli_reports_exception_config_errors_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            """
            [wrong_table]
            version = 1
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

    assert result.returncode == 2
    assert INPUT_ERROR_PREFIX in result.stderr
    assert "Traceback" not in result.stderr


def test_cli_reports_source_syntax_errors_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(root, "pkg/broken.py", "def load_contract(\n")

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

    assert result.returncode == 2
    assert INPUT_ERROR_PREFIX in result.stderr
    assert "Traceback" not in result.stderr


def test_cli_reports_source_decode_errors_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        path = root / "pkg" / "bad_encoding.py"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"\xff\xfe\x00")

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

    assert result.returncode == 2
    assert INPUT_ERROR_PREFIX in result.stderr
    assert "Traceback" not in result.stderr


def test_exception_config_rejects_bad_rule_id() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_config(root)
        write_file(
            root,
            "ci/fail-closed-exceptions.toml",
            exceptions_text(exception_entry(rule_id="FLC999", path="pkg/degraded.py")),
        )

        try:
            collect(root)
        except TypeError as exc:
            assert "exception rule_id" in str(exc)
        else:
            raise AssertionError("accepted invalid fail-closed exception rule id")


def test_config_string_arrays_reject_invalid_shapes() -> None:
    for malformed_logging_names in ('"logger.exception"', "[1]"):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_file(
                root,
                "ci/fail-closed-contracts.toml",
                config_text(logging_call_names=malformed_logging_names),
            )

            try:
                collect(root)
            except TypeError as exc:
                assert "logging_call_names" in str(exc)
            else:
                raise AssertionError(f"accepted malformed logging_call_names: {malformed_logging_names}")


def test_exception_suppression_config_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "ci/fail-closed-contracts.toml",
            config_text(extra_settings='exceptions = []'),
        )

        try:
            collect(root)
        except TypeError as exc:
            assert "fail_closed_contracts keys" in str(exc)
        else:
            raise AssertionError("accepted exception suppression config")


def test_config_version_rejects_unsupported_shapes() -> None:
    for malformed_version in ("2", "true", "1.0", "-1"):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_file(
                root,
                "ci/fail-closed-contracts.toml",
                config_text(version=malformed_version),
            )

            try:
                collect(root)
            except TypeError as exc:
                assert "version" in str(exc)
            else:
                raise AssertionError(f"accepted malformed version: {malformed_version}")


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
        test_empty_selected_paths_fail_closed,
        test_empty_selected_paths_suppresses_source_fence_wiring_noise,
        test_bad_fixtures_fail_with_stable_rule_ids,
        test_precise_exception_fixture_passes,
        test_project_exception_named_exception_is_not_broad,
        test_builtins_exception_qualified_name_is_broad,
        test_bare_return_fails_as_return_from_catch_all,
        test_bare_except_sentinel_return_fails_closed,
        test_broad_except_tuple_sentinel_return_fails_closed,
        test_self_logger_exception_classifies_logged_sentinel_return,
        test_chained_logger_exception_classifies_logged_sentinel_return,
        test_stdlib_getlogger_exception_classifies_logged_sentinel_return,
        test_non_logger_exception_method_does_not_classify_as_logged,
        test_conditional_sentinel_return_fails_closed,
        test_boolean_sentinel_return_fails_closed,
        test_conditional_broad_exception_type_fails_closed,
        test_repeated_pass_fails_as_silent_catch_all,
        test_ellipsis_fails_as_silent_catch_all,
        test_nested_function_return_inside_handler_is_not_handler_return,
        test_nested_precise_exception_return_inside_handler_is_not_outer_handler_return,
        test_config_excludes_nested_test_files,
        test_repo_config_excludes_nested_script_test_files,
        test_source_fence_static_wiring_is_required,
        test_source_fence_static_wiring_rejects_appended_command,
        test_repo_source_fence_static_wiring_is_current,
        test_source_fence_static_parser_requires_indented_commands,
        test_source_fence_static_parser_ignores_inline_comments,
        test_source_fence_static_parser_stops_at_top_level_non_recipe_lines,
        test_source_fence_static_parser_ignores_malformed_top_level_colon_lines,
        test_classified_degradation_requires_central_exception,
        test_stale_central_exception_fails_closed,
        test_dot_prefixed_exception_path_is_normalized,
        test_exception_config_rejects_invalid_shapes,
        test_exception_line_drift_fails_closed_both_ways,
        test_cli_reports_config_errors_without_traceback,
        test_cli_reports_exception_config_errors_without_traceback,
        test_cli_reports_source_syntax_errors_without_traceback,
        test_cli_reports_source_decode_errors_without_traceback,
        test_exception_config_rejects_bad_rule_id,
        test_config_string_arrays_reject_invalid_shapes,
        test_exception_suppression_config_is_rejected,
        test_config_version_rejects_unsupported_shapes,
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
