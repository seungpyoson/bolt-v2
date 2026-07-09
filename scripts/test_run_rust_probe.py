#!/usr/bin/env python3
"""Self-tests for the manual Rust Probe runner script."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / ".github" / "scripts" / "run-rust-probe.sh"


def fake_workspace() -> tempfile.TemporaryDirectory[str]:
    temp = tempfile.TemporaryDirectory(prefix="rust-probe-selftest.")
    scripts_dir = Path(temp.name) / "scripts"
    bin_dir = Path(temp.name) / "bin"
    scripts_dir.mkdir()
    bin_dir.mkdir()
    (scripts_dir / "rust_verification.py").write_text(
        "\n".join(
            (
                "from __future__ import annotations",
                "import json",
                "from pathlib import Path",
                "import sys",
                "args = sys.argv[1:]",
                "Path('captured_args.json').write_text(json.dumps(args), encoding='utf-8')",
                "print('ARGS_JSON=' + json.dumps(args))",
            )
        )
        + "\n",
        encoding="utf-8",
    )
    (bin_dir / "git").write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        "if [ \"$#\" -eq 2 ] && [ \"$1\" = \"rev-parse\" ] && [ \"$2\" = \"HEAD\" ]; then\n"
        "  printf '%s\\n' \"${RUST_PROBE_FAKE_HEAD:?}\"\n"
        "  exit 0\n"
        "fi\n"
        "echo \"unexpected git args: $*\" >&2\n"
        "exit 99\n",
        encoding="utf-8",
    )
    (bin_dir / "git").chmod(0o755)
    return temp


def run_probe(
    workspace: str | None,
    mode: str,
    test_target: str,
    test_name: str,
    *script_args: str,
    expected_sha: str = "a" * 40,
    actual_sha: str | None = None,
    probe_id: str = "probe-test",
    compile_only: str | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    if workspace is None:
        env.pop("GITHUB_WORKSPACE", None)
    else:
        env["GITHUB_WORKSPACE"] = workspace
        env["PATH"] = f"{Path(workspace) / 'bin'}{os.pathsep}{env['PATH']}"
    env["RUST_PROBE_MODE"] = mode
    env["RUST_PROBE_TEST_TARGET"] = test_target
    env["RUST_PROBE_TEST_NAME"] = test_name
    env["RUST_PROBE_EXPECTED_SHA"] = expected_sha
    env["RUST_PROBE_FAKE_HEAD"] = actual_sha or expected_sha
    env["RUST_PROBE_ID"] = probe_id
    if compile_only is None:
        env.pop("RUST_PROBE_COMPILE_ONLY", None)
    else:
        env["RUST_PROBE_COMPILE_ONLY"] = compile_only
    return subprocess.run(
        ["bash", str(SCRIPT_PATH), *script_args],
        check=False,
        env=env,
        text=True,
        capture_output=True,
    )


def captured_args(workspace: str) -> list[str]:
    return json.loads((Path(workspace) / "captured_args.json").read_text(encoding="utf-8"))


def assert_valid(
    description: str,
    mode: str,
    test_target: str,
    test_name: str,
    expected_args: list[str],
    compile_only: str | None = None,
) -> None:
    with fake_workspace() as temp:
        result = run_probe(temp, mode, test_target, test_name, compile_only=compile_only)
        if result.returncode != 0:
            raise AssertionError(
                f"{description}: expected success, got {result.returncode}\n"
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
        expected_output = (
            f"Rust Probe id: probe-test\n"
            f"Rust Probe checkout SHA: {'a' * 40}\n"
            f"Rust Probe mode: {mode}\n"
            f"Rust Probe compile_only: <unused>\n"
            f"Rust Probe test_target: {test_target or '<empty>'}\n"
            f"Rust Probe test_name: {test_name or '<empty>'}\n"
        )
        if expected_output not in result.stdout:
            raise AssertionError(
                f"{description}: missing selected probe output {expected_output!r}\n"
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
        args = captured_args(temp)
        expected = ["cargo", "--repo", temp, "--", *expected_args]
        if args != expected:
            raise AssertionError(f"{description}: expected argv {expected!r}, got {args!r}")


def assert_invalid(
    description: str,
    expected_fragment: str,
    workspace: str | None,
    mode: str,
    test_target: str,
    test_name: str,
    *script_args: str,
    expected_sha: str = "a" * 40,
    actual_sha: str | None = None,
    probe_id: str = "probe-test",
) -> None:
    result = run_probe(
        workspace,
        mode,
        test_target,
        test_name,
        *script_args,
        expected_sha=expected_sha,
        actual_sha=actual_sha,
        probe_id=probe_id,
    )
    combined = f"{result.stdout}\n{result.stderr}"
    if result.returncode != 2:
        raise AssertionError(
            f"{description}: expected exit 2, got {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    if expected_fragment not in combined:
        raise AssertionError(
            f"{description}: expected message containing {expected_fragment!r}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    if workspace is not None and (Path(workspace) / "captured_args.json").exists():
        raise AssertionError(f"{description}: validation failure still invoked rust_verification.py")


def assert_invalid_with_workspace(
    description: str,
    expected_fragment: str,
    mode: str,
    test_target: str,
    test_name: str,
    *script_args: str,
) -> None:
    with fake_workspace() as temp:
        assert_invalid(
            description,
            expected_fragment,
            temp,
            mode,
            test_target,
            test_name,
            *script_args,
        )


def main() -> int:
    assert_valid("check-lib", "check-lib", "", "", ["check", "--locked", "--lib"])
    assert_valid(
        "check-test-target",
        "check-test-target",
        "build_script_git_head_rerun_paths",
        "",
        ["check", "--locked", "--test", "build_script_git_head_rerun_paths"],
    )
    assert_valid(
        "nextest-no-run-test-target",
        "nextest-no-run-test-target",
        "build_script_git_head_rerun_paths",
        "",
        ["nextest", "run", "--locked", "--no-run", "--test", "build_script_git_head_rerun_paths"],
    )
    assert_valid(
        "nextest-lib-name",
        "nextest-lib-name",
        "",
        "live_node_config::startup_watchdog_capture_failure_after_running_still_awaits_runner_result",
        [
            "nextest",
            "run",
            "--locked",
            "--lib",
            "live_node_config::startup_watchdog_capture_failure_after_running_still_awaits_runner_result",
        ],
    )
    assert_valid(
        "nextest-test-target",
        "nextest-test-target",
        "build_script_git_head_rerun_paths",
        "",
        ["nextest", "run", "--locked", "--test", "build_script_git_head_rerun_paths"],
    )
    assert_valid(
        "nextest-test-target-name",
        "nextest-test-target-name",
        "build_script_git_head_rerun_paths",
        "build_script_reads_manifest_dir_at_run_time",
        [
            "nextest",
            "run",
            "--locked",
            "--test",
            "build_script_git_head_rerun_paths",
            "build_script_reads_manifest_dir_at_run_time",
        ],
    )
    assert_valid(
        "nextest-test-target ignores stale compile-only env",
        "nextest-test-target",
        "build_script_git_head_rerun_paths",
        "",
        ["nextest", "run", "--locked", "--test", "build_script_git_head_rerun_paths"],
        compile_only="1",
    )
    assert_valid(
        "nextest-test-target-name ignores stale compile-only env",
        "nextest-test-target-name",
        "build_script_git_head_rerun_paths",
        "build_script_reads_manifest_dir_at_run_time",
        [
            "nextest",
            "run",
            "--locked",
            "--test",
            "build_script_git_head_rerun_paths",
            "build_script_reads_manifest_dir_at_run_time",
        ],
        compile_only="1",
    )
    assert_valid(
        "target regex accepts safe punctuation",
        "check-test-target",
        "_target.name-9",
        "",
        ["check", "--locked", "--test", "_target.name-9"],
    )
    assert_valid(
        "name regex accepts safe punctuation",
        "nextest-test-target-name",
        "build_script_git_head_rerun_paths",
        "test_mod::case/part@name-1",
        [
            "nextest",
            "run",
            "--locked",
            "--test",
            "build_script_git_head_rerun_paths",
            "test_mod::case/part@name-1",
        ],
    )

    assert_invalid("missing workspace", "GITHUB_WORKSPACE is required", None, "check-lib", "", "")
    assert_invalid(
        "nonexistent workspace",
        "GITHUB_WORKSPACE must be an existing directory",
        "/tmp/rust-probe-selftest-does-not-exist",
        "check-lib",
        "",
        "",
    )
    assert_invalid_with_workspace(
        "freeform script args",
        "does not accept command-line arguments",
        "check-lib",
        "",
        "",
        "--",
        "--help",
    )
    assert_invalid_with_workspace("unsupported mode", "unsupported mode", "bogus", "", "")
    assert_invalid_with_workspace(
        "check-lib forbids target",
        "test_target is forbidden",
        "check-lib",
        "target",
        "",
    )
    assert_invalid_with_workspace(
        "check-lib forbids name",
        "test_name is forbidden",
        "check-lib",
        "",
        "name",
    )
    assert_invalid_with_workspace(
        "check-test-target requires target",
        "test_target is required",
        "check-test-target",
        "",
        "",
    )
    assert_invalid_with_workspace(
        "check-test-target forbids name",
        "test_name is forbidden",
        "check-test-target",
        "target",
        "name",
    )
    assert_invalid_with_workspace(
        "nextest-no-run-test-target requires target",
        "test_target is required",
        "nextest-no-run-test-target",
        "",
        "",
    )
    assert_invalid_with_workspace(
        "nextest-no-run-test-target forbids name",
        "test_name is forbidden",
        "nextest-no-run-test-target",
        "target",
        "name",
    )
    assert_invalid_with_workspace(
        "nextest-test-target requires target",
        "test_target is required",
        "nextest-test-target",
        "",
        "",
    )
    assert_invalid_with_workspace(
        "nextest-lib-name forbids target",
        "test_target is forbidden",
        "nextest-lib-name",
        "build_script_git_head_rerun_paths",
        "live_node_config::startup_watchdog_capture_failure_after_running_still_awaits_runner_result",
    )
    assert_invalid_with_workspace(
        "nextest-lib-name requires name",
        "test_name is required",
        "nextest-lib-name",
        "",
        "",
    )
    assert_invalid_with_workspace(
        "nextest-test-target forbids name",
        "test_name is forbidden",
        "nextest-test-target",
        "target",
        "name",
    )
    assert_invalid_with_workspace(
        "nextest-test-target-name requires target",
        "test_target is required",
        "nextest-test-target-name",
        "",
        "name",
    )
    assert_invalid_with_workspace(
        "nextest-test-target-name requires name",
        "test_name is required",
        "nextest-test-target-name",
        "target",
        "",
    )
    assert_invalid_with_workspace(
        "target rejects slash",
        "test_target must match",
        "check-test-target",
        "bad/target",
        "",
    )
    assert_invalid_with_workspace(
        "target rejects leading hyphen",
        "test_target must match",
        "check-test-target",
        "--help",
        "",
    )
    assert_invalid_with_workspace(
        "target rejects leading dot",
        "test_target must match",
        "check-test-target",
        ".hidden",
        "",
    )
    assert_invalid_with_workspace(
        "name rejects space",
        "test_name must match",
        "nextest-test-target-name",
        "target",
        "bad name",
    )
    assert_invalid_with_workspace(
        "name rejects leading hyphen",
        "test_name must match",
        "nextest-test-target-name",
        "target",
        "--help",
    )
    assert_invalid_with_workspace(
        "name rejects leading colon",
        "test_name must match",
        "nextest-test-target-name",
        "target",
        "::tests",
    )
    with fake_workspace() as temp:
        assert_invalid(
            "missing expected SHA",
            "RUST_PROBE_EXPECTED_SHA is required",
            temp,
            "check-lib",
            "",
            "",
            expected_sha="",
        )
    with fake_workspace() as temp:
        assert_invalid(
            "invalid expected SHA",
            "RUST_PROBE_EXPECTED_SHA must be a full 40-character hex SHA",
            temp,
            "check-lib",
            "",
            "",
            expected_sha="abc",
        )
    with fake_workspace() as temp:
        assert_invalid(
            "checkout SHA mismatch",
            "checked-out SHA does not match RUST_PROBE_EXPECTED_SHA",
            temp,
            "check-lib",
            "",
            "",
            actual_sha="b" * 40,
        )
    with fake_workspace() as temp:
        assert_invalid(
            "missing probe id",
            "RUST_PROBE_ID is required",
            temp,
            "check-lib",
            "",
            "",
            probe_id="",
        )

    print("OK: Rust Probe runner self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
