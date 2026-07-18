#!/usr/bin/env python3
"""Tests for merge_queue_operator.py."""

from __future__ import annotations

import functools
import importlib.util
import io
import json
import os
import pathlib
import sys
import tempfile
from contextlib import redirect_stderr, redirect_stdout


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_queue_operator.py"
BASE_SHA = "a" * 40
HEAD_ONE = "b" * 40
HEAD_TWO = "c" * 40
REPOSITORY = "github.com/example/repo"
REMOTE_URL = "https://github.com/example/repo.git"


class FakeRunner:
    def __init__(self, preflight_payload: dict[str, object], preflight_returncode: int) -> None:
        self.commands: list[tuple[str, ...]] = []
        self.command_environments: list[tuple[tuple[str, ...], dict[str, str] | None]] = []
        self.command_cwds: list[tuple[tuple[str, ...], pathlib.Path]] = []
        self.preflight_payload = preflight_payload
        self.preflight_returncode = preflight_returncode

    def __call__(
        self,
        command: list[str],
        *,
        cwd: pathlib.Path,
        check: bool = False,
        input_text: str | None = None,
        timeout_seconds: int | None = None,
        environment: dict[str, str] | None = None,
    ) -> object:
        self.commands.append(tuple(command))
        self.command_environments.append((tuple(command), environment))
        self.command_cwds.append((tuple(command), cwd))
        if command[:4] == ["git", "config", "--local", "--get-all"]:
            assert command == ["git", "config", "--local", "--get-all", "remote.origin.url"], command
            return completed(command, stdout=f"{REMOTE_URL}\n")
        if command[:2] == ["git", "ls-remote"] and command[-1] == "refs/heads/main":
            assert timeout_seconds == 30, (command, timeout_seconds)
            assert command[3] == REMOTE_URL, command
            return completed(command, stdout=f"{BASE_SHA}\trefs/heads/main\n")
        if command[:2] == ["git", "ls-remote"] and command[-1] == "refs/pull/1/head":
            assert timeout_seconds == 30, (command, timeout_seconds)
            assert command[3] == REMOTE_URL, command
            return completed(command, stdout=f"{HEAD_ONE}\trefs/pull/1/head\n")
        if command[:2] == ["git", "ls-remote"] and command[-1] == "refs/pull/2/head":
            assert timeout_seconds == 30, (command, timeout_seconds)
            assert command[3] == REMOTE_URL, command
            return completed(command, stdout=f"{HEAD_TWO}\trefs/pull/2/head\n")
        if command[:2] == ["python3", str(REPO_ROOT / "scripts" / "merge_queue_preflight.py")]:
            assert timeout_seconds is None, (command, timeout_seconds)
            return completed(
                command,
                returncode=self.preflight_returncode,
                stdout=json.dumps(self.preflight_payload),
            )
        if command[:3] == ["gh", "pr", "comment"]:
            assert timeout_seconds == 30, (command, timeout_seconds)
            assert input_text == "@mergifyio queue\n", input_text
            assert command[4:6] == ["--repo", REPOSITORY], command
            return completed(command)
        raise AssertionError(f"unexpected command: {command!r}")


def completed(command: list[str], returncode: int = 0, stdout: str = "", stderr: str = "") -> object:
    module = load_operator_module()
    return module.CommandResult(tuple(command), returncode, stdout, stderr)


@functools.cache
def load_operator_module() -> object:
    spec = importlib.util.spec_from_file_location("merge_queue_operator", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("merge_queue_operator module spec unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_config(
    root: pathlib.Path,
    *,
    origin: str = "origin",
    repository: str = REPOSITORY,
) -> pathlib.Path:
    config = root / "preflight.toml"
    config.write_text(
        "\n".join(
            (
                "[merge_queue_preflight]",
                f"origin = {json.dumps(origin)}",
                'base = "main"',
                "",
                "[merge_queue_preflight.operator]",
                f"repository = {json.dumps(repository)}",
                'queue_command = "@mergifyio queue"',
                "ref_timeout_seconds = 30",
                "queue_timeout_seconds = 30",
                "",
                "[merge_queue_preflight.timeouts]",
                "input_seconds = 30",
                "",
            )
        ),
        encoding="utf-8",
    )
    return config


def run_operator(args: list[str], runner: FakeRunner) -> tuple[int, str, str]:
    module = load_operator_module()
    stdout = io.StringIO()
    stderr = io.StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        rc = module.main(args, runner=runner, repo=REPO_ROOT)
    return rc, stdout.getvalue(), stderr.getvalue()


def ready_payload(pr: int = 1) -> dict[str, object]:
    return {"verdict": "queue_as_one_wave", "requested_prs": [pr]}


def assert_queue_as_one_wave_posts_mergify_comments() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = FakeRunner(ready_payload(), 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 0, (rc, stdout, stderr)
    assert "queued PR #1" in stdout, stdout
    assert (
        "gh",
        "pr",
        "comment",
        "1",
        "--repo",
        REPOSITORY,
        "--body-file",
        "-",
    ) in runner.commands, runner.commands
    preflight_command = next(command for command in runner.commands if "merge_queue_preflight.py" in command[1])
    assert "--expected-base-sha" in preflight_command, preflight_command
    assert BASE_SHA in preflight_command, preflight_command
    assert f"1={HEAD_ONE}" in preflight_command, preflight_command


def assert_preflight_and_queue_use_pinned_repository_identity() -> None:
    previous_gh_host = os.environ.get("GH_HOST")
    os.environ["GH_HOST"] = "attacker.invalid"
    try:
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp))
            runner = FakeRunner(ready_payload(), 0)
            rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    finally:
        if previous_gh_host is None:
            os.environ.pop("GH_HOST", None)
        else:
            os.environ["GH_HOST"] = previous_gh_host
    assert rc == 0, (rc, stdout, stderr)
    preflight_command, preflight_environment = next(
        item
        for item in runner.command_environments
        if len(item[0]) > 1 and "merge_queue_preflight.py" in item[0][1]
    )
    assert preflight_command
    assert preflight_environment is not None
    assert preflight_environment["GH_REPO"] == REPOSITORY
    assert preflight_environment["GIT_CONFIG_NOSYSTEM"] == "1"
    assert preflight_environment["GIT_CONFIG_GLOBAL"] == os.devnull
    assert preflight_environment["GIT_CONFIG_KEY_0"] == "credential.https://github.com.helper"
    assert preflight_environment["GIT_CONFIG_VALUE_0"] == "!gh auth git-credential"
    resolution_command, resolution_environment = next(
        item
        for item in runner.command_environments
        if item[0][:4] == ("git", "config", "--local", "--get-all")
    )
    assert resolution_command[-1] == "remote.origin.url"
    assert resolution_environment is not None
    assert {key for key in resolution_environment if key.startswith("GIT_")} == {
        "GIT_CONFIG_NOSYSTEM",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
    }
    for command, environment in runner.command_environments:
        if command[:2] == ("git", "ls-remote"):
            assert environment == preflight_environment, (command, environment)
            cwd = next(recorded_cwd for recorded, recorded_cwd in runner.command_cwds if recorded == command)
            assert cwd != REPO_ROOT, cwd
    assert (
        "gh",
        "pr",
        "comment",
        "1",
        "--repo",
        REPOSITORY,
        "--body-file",
        "-",
    ) in runner.commands


def assert_alternate_repository_authorities_fail_before_transport() -> None:
    class ResolvedRemoteRunner(FakeRunner):
        def __init__(self, remote_url: str) -> None:
            super().__init__(ready_payload(), 0)
            self.remote_url = remote_url

        def __call__(self, command: list[str], **kwargs: object) -> object:
            if command[:4] == ["git", "config", "--local", "--get-all"]:
                self.commands.append(tuple(command))
                return completed(command, stdout=f"{self.remote_url}\n")
            return super().__call__(command, **kwargs)

    cases = (
        (write_config, {"origin": "../origin.git"}),
        (write_config, {"repository": "example.invalid/example/repo"}),
        (write_config, {"repository": "github.com/another/repository"}),
    )
    for writer, options in cases:
        with tempfile.TemporaryDirectory() as tmp:
            config = writer(pathlib.Path(tmp), **options)
            runner = FakeRunner(ready_payload(), 0)
            rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
        assert rc == 4, (options, rc, stdout, stderr)
        assert not any(command[:2] == ("git", "ls-remote") for command in runner.commands)
        assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands)

    for remote_url in (
        "http://github.com/example/repo.git",
        "ssh://git@github.com/example/repo.git",
        "git@github.com:example/repo.git",
        "file:///tmp/repo.git",
        "../repo.git",
        "https://github.com/other/repo.git",
        "https://example.invalid/repo.git",
        "https://github.com/example/repo.git?access_token=preflight-secret",
    ):
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp))
            runner = ResolvedRemoteRunner(remote_url)
            rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
        assert rc == 4, (remote_url, rc, stdout, stderr)
        assert remote_url not in stderr, stderr
        assert "preflight-secret" not in stderr, stderr
        assert not any(command[:2] == ("git", "ls-remote") for command in runner.commands)
        assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands)


def assert_config_snapshot_is_immutable_across_operator_and_preflight() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        original_text = config.read_text(encoding="utf-8")

        class MutatingConfigRunner(FakeRunner):
            def __call__(self, command: list[str], **kwargs: object) -> object:
                if command[:4] == ["git", "config", "--local", "--get-all"]:
                    config.write_text(
                        original_text.replace('origin = "origin"', 'origin = "changed-origin"'),
                        encoding="utf-8",
                    )
                if command[:2] == ["python3", str(REPO_ROOT / "scripts" / "merge_queue_preflight.py")]:
                    snapshot = pathlib.Path(command[command.index("--config") + 1])
                    assert snapshot != config, (snapshot, config)
                    assert snapshot.read_text(encoding="utf-8") == original_text
                return super().__call__(command, **kwargs)

        runner = MutatingConfigRunner(ready_payload(), 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 0, (rc, stdout, stderr)


def assert_multiple_prs_are_rejected_by_parser() -> None:
    module = load_operator_module()
    stderr = io.StringIO()
    try:
        with redirect_stderr(stderr):
            module.parser().parse_args(["1", "2"])
    except SystemExit as exc:
        assert exc.code == 2, (exc.code, stderr.getvalue())
    else:
        raise AssertionError("merge queue operator must accept exactly one PR")


def assert_ready_payload_must_match_requested_pr() -> None:
    malformed_payloads = {
        "missing": {"verdict": "queue_as_one_wave"},
        "empty": {"verdict": "queue_as_one_wave", "requested_prs": []},
        "multiple": {"verdict": "queue_as_one_wave", "requested_prs": [1, 2]},
        "wrong-type": {"verdict": "queue_as_one_wave", "requested_prs": "1"},
        "mismatched": {"verdict": "queue_as_one_wave", "requested_prs": [2]},
    }
    for label, payload in malformed_payloads.items():
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp))
            runner = FakeRunner(payload, 0)
            rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
        assert rc == 4, (label, rc, stdout, stderr)
        assert "requested PR" in stderr, (label, stderr)
        assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), (label, runner.commands)


def assert_operator_imports_preflight_verdict_constants() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    required_import = "from merge_queue_preflight import VERDICT_QUEUE_AS_ONE_WAVE"
    if required_import not in source:
        raise AssertionError("merge_queue_operator must import queue verdict constants from merge_queue_preflight")
    forbidden_literals = (
        'QUEUE_READY_VERDICT = "queue_as_one_wave"',
        'VERDICT_SPLIT_ADVISED = "split_advised"',
    )
    leaked = [literal for literal in forbidden_literals if literal in source]
    if leaked:
        raise AssertionError(f"merge_queue_operator redefines preflight verdict literal(s): {leaked}")


def assert_blocked_verdict_does_not_queue() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = FakeRunner({"verdict": "blocked", "summary": "blocked"}, 2)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 2, (rc, stdout, stderr)
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_unexpected_success_payload_is_operator_error() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = FakeRunner({"verdict": "inconclusive", "summary": "unexpected"}, 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 4, (rc, stdout, stderr)
    assert "merge queue preflight did not queue: verdict='inconclusive'" in stdout, stdout
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_malformed_config_is_operator_error() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = pathlib.Path(tmp) / "preflight.toml"
        config.write_text("[merge_queue_preflight\n", encoding="utf-8")
        runner = FakeRunner({"verdict": "queue_as_one_wave", "summary": "ready"}, 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 4, (rc, stdout, stderr)
    assert "unable to read config" in stderr, stderr
    assert not runner.commands, runner.commands


def assert_malformed_preflight_json_does_not_queue() -> None:
    class BadJsonRunner(FakeRunner):
        def __call__(self, command: list[str], **kwargs: object) -> object:
            if command[:2] == ["python3", str(REPO_ROOT / "scripts" / "merge_queue_preflight.py")]:
                self.commands.append(tuple(command))
                return completed(command, stdout="not json")
            return super().__call__(command, **kwargs)

    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = BadJsonRunner({}, 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 4, (rc, stdout, stderr)
    assert "did not emit valid JSON" in stderr, stderr
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_non_object_preflight_json_does_not_queue() -> None:
    class NonObjectJsonRunner(FakeRunner):
        def __call__(self, command: list[str], **kwargs: object) -> object:
            if command[:2] == ["python3", str(REPO_ROOT / "scripts" / "merge_queue_preflight.py")]:
                self.commands.append(tuple(command))
                return completed(command, stdout="[]")
            return super().__call__(command, **kwargs)

    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = NonObjectJsonRunner({}, 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 4, (rc, stdout, stderr)
    assert "must be an object" in stderr, stderr
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_nonzero_preflight_ready_payload_does_not_queue() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = FakeRunner({"verdict": "queue_as_one_wave", "summary": "contradiction"}, 3)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 3, (rc, stdout, stderr)
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_dry_run_does_not_queue() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = FakeRunner(ready_payload(), 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "--dry-run", "1"], runner)
    assert rc == 0, (rc, stdout, stderr)
    assert "would queue PR #1" in stdout, stdout
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_missing_ref_does_not_queue() -> None:
    class MissingRefRunner(FakeRunner):
        def __call__(self, command: list[str], **kwargs: object) -> object:
            if command[:2] == ["git", "ls-remote"] and command[-1] == "refs/pull/1/head":
                self.commands.append(tuple(command))
                raise load_operator_module().OperatorError("missing ref")
            return super().__call__(command, **kwargs)

    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        runner = MissingRefRunner({"verdict": "queue_as_one_wave", "summary": "ready"}, 0)
        rc, stdout, stderr = run_operator(["--config", str(config), "1"], runner)
    assert rc == 4, (rc, stdout, stderr)
    assert "missing ref" in stderr, stderr
    assert not any(command[:3] == ("gh", "pr", "comment") for command in runner.commands), runner.commands


def assert_run_command_missing_executable_is_operator_error() -> None:
    module = load_operator_module()
    try:
        module.run_command(["definitely-missing-merge-queue-operator-test-binary"], cwd=REPO_ROOT)
    except module.OperatorError as exc:
        assert "unavailable" in str(exc), exc
    else:
        raise AssertionError("missing executable did not raise OperatorError")


def assert_run_command_timeout_is_operator_error() -> None:
    module = load_operator_module()
    try:
        module.run_command(
            ["python3", "-c", "import time; time.sleep(5)"],
            cwd=REPO_ROOT,
            timeout_seconds=1,
        )
    except module.OperatorError as exc:
        assert "timed out after 1 seconds" in str(exc), exc
    else:
        raise AssertionError("timeout did not raise OperatorError")


def assert_run_command_timeout_reports_operator_error() -> None:
    module = load_operator_module()
    try:
        module.run_command(
            ["python3", "-c", "import time; time.sleep(30)"],
            cwd=REPO_ROOT,
            timeout_seconds=1,
        )
    except module.OperatorError as exc:
        assert "timed out after 1 seconds" in str(exc), exc
    else:
        raise AssertionError("timeout did not raise OperatorError")


def assert_ad_hoc_run_verifier_is_not_an_operator_flag() -> None:
    module = load_operator_module()
    stderr = io.StringIO()
    try:
        with redirect_stderr(stderr):
            module.parser().parse_args(["--run-verifier", "just fmt-check", "1"])
    except SystemExit as exc:
        assert exc.code == 2, exc.code
    else:
        raise AssertionError("--run-verifier should not be accepted by merge_queue_operator")
    assert stderr.getvalue(), "argparse should explain why the flag was rejected"


def assert_verifier_profile_is_not_an_operator_flag() -> None:
    module = load_operator_module()
    stderr = io.StringIO()
    try:
        with redirect_stderr(stderr):
            module.parser().parse_args(["--verifier-profile", "local", "1"])
    except SystemExit as exc:
        assert exc.code == 2, exc.code
    else:
        raise AssertionError("--verifier-profile should not be accepted by merge_queue_operator")
    assert stderr.getvalue(), "argparse should explain why the flag was rejected"


def main() -> int:
    assert_operator_imports_preflight_verdict_constants()
    assert_queue_as_one_wave_posts_mergify_comments()
    assert_preflight_and_queue_use_pinned_repository_identity()
    assert_alternate_repository_authorities_fail_before_transport()
    assert_config_snapshot_is_immutable_across_operator_and_preflight()
    assert_multiple_prs_are_rejected_by_parser()
    assert_ready_payload_must_match_requested_pr()
    assert_blocked_verdict_does_not_queue()
    assert_unexpected_success_payload_is_operator_error()
    assert_malformed_config_is_operator_error()
    assert_malformed_preflight_json_does_not_queue()
    assert_non_object_preflight_json_does_not_queue()
    assert_nonzero_preflight_ready_payload_does_not_queue()
    assert_dry_run_does_not_queue()
    assert_missing_ref_does_not_queue()
    assert_run_command_missing_executable_is_operator_error()
    assert_run_command_timeout_is_operator_error()
    assert_run_command_timeout_reports_operator_error()
    assert_ad_hoc_run_verifier_is_not_an_operator_flag()
    assert_verifier_profile_is_not_an_operator_flag()
    print("OK: merge_queue_operator tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
