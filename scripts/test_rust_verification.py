#!/usr/bin/env python3
"""Self-tests for the repo-local Rust verification owner."""

from __future__ import annotations

import contextlib
import io
import os
import json
import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"
VERIFY_REMOTE_HEAD = "a" * 40
VERIFY_REMOTE_BRANCH = "codex/verify-remote-test"


def run_owner(args: list[str], *, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def load_owner_module() -> object:
    spec = importlib.util.spec_from_file_location("rust_verification_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_executable(path: pathlib.Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def write_policy(repo: pathlib.Path) -> None:
    (repo / "ci").mkdir()
    (repo / "ci" / "rust-verification.toml").write_text(
        textwrap.dedent(
            """\
            schema_version = 2
            project_id = "bolt-v2"
            target_namespace = "bolt-v2"

            [local_compile_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
            refused_managed_commands = ["test", "clippy", "build"]
            refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]

            [local_lane_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            lock_dir = "/tmp/rust-verification-lanes"
            acquire_timeout_seconds = 1800
            heartbeat_seconds = 15
            poll_interval_seconds = 1

            [commands]

            [commands.test]
            recipe = "managed-test"

            [commands.clippy]
            recipe = "managed-clippy"

            [commands.build]
            recipe = "managed-build"
            artifact_layout = "cargo"
            profile = "release"
            target = "aarch64-unknown-linux-gnu"
            """
        ),
        encoding="utf-8",
    )
    (repo / "justfile").write_text("", encoding="utf-8")


def parse_log(path: pathlib.Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, value = line.split("=", 1)
        values[key] = value
    return values


def same_path(left: str, right: pathlib.Path) -> bool:
    return pathlib.Path(left).resolve() == right.resolve()


def assert_repo_local_owner_contract() -> None:
    if not SCRIPT.exists():
        raise AssertionError(f"missing repo-local owner script: {SCRIPT}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        cargo_log = tmp_path / "cargo.log"
        just_log = tmp_path / "just.log"
        write_executable(
            bin_dir / "cargo",
            f"""#!/usr/bin/env bash
printf 'cwd=%s\\n' "$PWD" > {cargo_log}
printf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {cargo_log}
printf 'args=%s\\n' "$*" >> {cargo_log}
""",
        )
        write_executable(
            bin_dir / "just",
            f"""#!/usr/bin/env bash
printf 'cwd=%s\\n' "$PWD" > {just_log}
printf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {just_log}
printf 'args=%s\\n' "$*" >> {just_log}
""",
        )

        root_base = tmp_path / "rust-root"
        env = os.environ.copy()
        env.pop("GITHUB_ACTIONS", None)
        env.pop("BOLT_ALLOW_LOCAL_RUST", None)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        target_dir = root_base / "bolt-v2" / "target"
        result = run_owner(["target-dir", "--repo", str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(target_dir):
            raise AssertionError((result.stdout, target_dir))
        if not target_dir.is_dir():
            raise AssertionError(f"target-dir did not create {target_dir}")

        binary = target_dir / "aarch64-unknown-linux-gnu" / "release" / "bolt-v2"
        binary.parent.mkdir(parents=True)
        binary.write_text("binary", encoding="utf-8")
        result = run_owner(["binary-path", "--repo", str(repo), "--bin", "bolt-v2"], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(binary):
            raise AssertionError((result.stdout, binary))

        result = run_owner(["cargo", "--repo", str(repo), "--", "fmt", "--check"], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        cargo_values = parse_log(cargo_log)
        if not same_path(cargo_values["cwd"], repo) or cargo_values["target"] != "" or cargo_values["args"] != "fmt --check":
            raise AssertionError(cargo_values)

        result = run_owner(["run", "--repo", str(repo), "build", "--flag"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        refusal = json.loads(result.stderr)
        next_steps = "\n".join(refusal.get("next_steps", []))
        if (
            refusal.get("refusal_code") != "local_compile_disabled"
            or "just rust-probe suggest" not in next_steps
            or "for merge proof: run: just verify-remote" not in next_steps
        ):
            raise AssertionError(refusal)

        allowed_env = env.copy()
        allowed_env["GITHUB_ACTIONS"] = "true"
        result = run_owner(["run", "--repo", str(repo), "build", "--flag"], env=allowed_env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        just_values = parse_log(just_log)
        expected_args = f"-f {repo / 'justfile'} --working-directory {repo} -- managed-build --flag"
        if not same_path(just_values["cwd"], repo) or just_values["target"] != str(target_dir) or just_values["args"] != expected_args:
            raise AssertionError(just_values)

        break_glass_env = env.copy()
        break_glass_env["BOLT_ALLOW_LOCAL_RUST"] = "1"
        result = run_owner(["run", "--repo", str(repo), "build", "--break-glass"], env=break_glass_env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        just_values = parse_log(just_log)
        expected_args = f"-f {repo / 'justfile'} --working-directory {repo} -- managed-build --break-glass"
        if not same_path(just_values["cwd"], repo) or just_values["target"] != str(target_dir) or just_values["args"] != expected_args:
            raise AssertionError(just_values)

        result = run_owner(["validate-policy", "--repo", str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        payload = json.loads(result.stdout)
        expected_payload = {
            "build_profile": "release",
            "build_target": "aarch64-unknown-linux-gnu",
            "policy": str(repo / "ci" / "rust-verification.toml"),
            "project_id": "bolt-v2",
            "status": "ok",
        }
        if payload != expected_payload:
            raise AssertionError(payload)


def assert_fmt_avoids_managed_cache_lock() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        observed: dict[str, object] = {}

        def forbidden_cache_lock(_policy: dict[str, object], *, exclusive: bool) -> object:
            raise AssertionError("cargo fmt must not touch the managed cache lock")

        def fake_run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
            observed["argv"] = argv
            observed["env"] = env
            return 0

        original_cache_lock = owner.cache_lock
        original_run_process = owner.run_process
        try:
            owner.cache_lock = forbidden_cache_lock
            owner.run_process = fake_run_process
            args = type("Args", (), {"repo": str(repo), "args": ["fmt", "--check"]})()
            result = owner.cmd_cargo(args)
        finally:
            owner.cache_lock = original_cache_lock
            owner.run_process = original_run_process
    if result != 0:
        raise AssertionError(result)
    env = observed.get("env")
    if not isinstance(env, dict) or "CARGO_TARGET_DIR" in env or "BOLT_ALLOW_LOCAL_RUST" in env:
        raise AssertionError(observed)


def assert_system_python_contract() -> None:
    system_python = pathlib.Path("/usr/bin/python3")
    if not system_python.exists():
        return
    result = subprocess.run(
        [str(system_python), "-S", str(SCRIPT), "repo-status", "--repo", str(REPO_ROOT)],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    if result.stdout.strip() != "managed":
        raise AssertionError(result.stdout)


def assert_oversized_policy_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        policy = repo / "ci" / "rust-verification.toml"
        policy.write_text("schema_version = 1\n" + ("# padding\n" * 140_000), encoding="utf-8")

        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "exceeds maximum size" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_remote_diagnostics_policy_loads() -> None:
    owner = load_owner_module()
    policy = {
        "remote_verification": {
            "poll_interval_seconds": 15,
            "checks_appear_timeout_seconds": 300,
            "overall_timeout_seconds": 3600,
            "diagnostic_log_max_lines": 160,
            "diagnostic_log_max_bytes": 20000,
            "diagnostic_unavailable_notice_interval_polls": 4,
        }
    }
    loaded = owner.remote_verification_policy(policy)
    if loaded["diagnostic_log_max_lines"] != 160:
        raise AssertionError(loaded)
    if loaded["diagnostic_log_max_bytes"] != 20000:
        raise AssertionError(loaded)
    if loaded["diagnostic_unavailable_notice_interval_polls"] != 4:
        raise AssertionError(loaded)


def write_verify_remote_config(repo: pathlib.Path) -> None:
    (repo / "ci").mkdir(exist_ok=True)
    (repo / "ci" / "github-actions-runners.toml").write_text(
        textwrap.dedent(
            """\
            [ci_provenance]
            workflow_name = "CI"
            workflow_path = ".github/workflows/ci.yml"

            [ci_provenance.dispatch]
            workflow_input = "full_ci"
            """
        ),
        encoding="utf-8",
    )


def verify_remote_pr(*, is_draft: bool = True, owner: str = "seungpyoson", repo: str = "bolt-v2") -> dict[str, object]:
    return {
        "number": 648,
        "url": "https://github.com/seungpyoson/bolt-v2/pull/648",
        "headRefOid": VERIFY_REMOTE_HEAD,
        "headRefName": VERIFY_REMOTE_BRANCH,
        "state": "OPEN",
        "isDraft": is_draft,
        "headRepositoryOwner": {"login": owner},
        "headRepository": {"name": repo, "nameWithOwner": f"{owner}/{repo}"},
    }


def workflow_run(
    database_id: int,
    *,
    event: str = "workflow_dispatch",
    status: str = "completed",
    conclusion: str | None = "success",
    created_at: str = "2026-06-13T00:00:00Z",
) -> dict[str, object]:
    return {
        "databaseId": database_id,
        "attempt": 1,
        "event": event,
        "headSha": VERIFY_REMOTE_HEAD,
        "status": status,
        "conclusion": conclusion,
        "createdAt": created_at,
        "url": f"https://github.com/seungpyoson/bolt-v2/actions/runs/{database_id}",
    }


class VerifyRemoteHarness:
    def __init__(
        self,
        owner: object,
        repo: pathlib.Path,
        *,
        pr: dict[str, object],
        run_lists: list[list[dict[str, object]]],
        run_list_error: str | None = None,
        advance_after_dispatch: bool = False,
    ) -> None:
        self.owner = owner
        self.repo = repo
        self.pr = pr
        self.run_lists = list(run_lists)
        self.run_list_error = run_list_error
        self.advance_after_dispatch = advance_after_dispatch
        self.dispatches: list[list[str]] = []
        self.pr_checks_calls = 0
        self.sleep_calls = 0
        self._time = 1000.0
        self._saved: dict[str, object] = {}

    def __enter__(self) -> "VerifyRemoteHarness":
        self._saved = {
            "load_policy": self.owner.load_policy,
            "ensure_verify_remote_preconditions": self.owner.ensure_verify_remote_preconditions,
            "pr_for_exact_head": self.owner.pr_for_exact_head,
            "load_json_command": self.owner.load_json_command,
            "run_capture": self.owner.run_capture,
            "monotonic": self.owner.time.monotonic,
            "sleep": self.owner.time.sleep,
        }
        self.owner.load_policy = self.fake_load_policy
        self.owner.ensure_verify_remote_preconditions = self.fake_preconditions
        self.owner.pr_for_exact_head = self.fake_pr_for_exact_head
        self.owner.load_json_command = self.fake_load_json_command
        self.owner.run_capture = self.fake_run_capture
        self.owner.time.monotonic = self.fake_monotonic
        self.owner.time.sleep = self.fake_sleep
        return self

    def __exit__(self, _exc_type: object, _exc: object, _tb: object) -> None:
        for name, value in self._saved.items():
            if name == "monotonic":
                self.owner.time.monotonic = value
            elif name == "sleep":
                self.owner.time.sleep = value
            else:
                setattr(self.owner, name, value)

    def fake_load_policy(self, _repo: pathlib.Path) -> dict[str, object]:
        return {
            "remote_verification": {
                "poll_interval_seconds": 1,
                "checks_appear_timeout_seconds": 2,
                "overall_timeout_seconds": 8,
                "diagnostic_log_max_lines": 160,
                "diagnostic_log_max_bytes": 20000,
                "diagnostic_unavailable_notice_interval_polls": 4,
            }
        }

    def fake_preconditions(self, _repo: pathlib.Path) -> tuple[str, str, None]:
        return VERIFY_REMOTE_HEAD, VERIFY_REMOTE_BRANCH, None

    def fake_pr_for_exact_head(
        self,
        _repo: pathlib.Path,
        _branch: str,
        _head: str,
        *,
        during_watch: bool,
    ) -> tuple[dict[str, object] | None, str | None]:
        if during_watch and self.advance_after_dispatch and self.dispatches:
            return (
                None,
                "PR branch advanced during watch: headRefOid bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb "
                f"no longer matches local HEAD {VERIFY_REMOTE_HEAD}; fetch the branch and rerun verify-remote",
            )
        return self.pr, None

    def fake_load_json_command(self, argv: list[str], *, repo: pathlib.Path) -> tuple[object | None, str | None]:
        if repo != self.repo:
            raise AssertionError(repo)
        if argv[:3] == ["gh", "run", "list"]:
            if self.run_list_error is not None:
                return None, self.run_list_error
            if self.run_lists:
                return self.run_lists.pop(0), None
            return [], None
        if argv[:3] == ["gh", "run", "view"]:
            run_id = int(argv[3])
            for run_list in self.run_lists:
                for run in run_list:
                    if int(run["databaseId"]) == run_id:
                        return run, None
            return workflow_run(run_id), None
        if argv[:3] == ["gh", "repo", "view"]:
            return {"name": "bolt-v2", "owner": {"login": "seungpyoson"}}, None
        raise AssertionError(f"unexpected JSON command: {argv}")

    def fake_run_capture(self, argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        if repo != self.repo:
            raise AssertionError(repo)
        if argv[:3] == ["gh", "workflow", "run"]:
            self.dispatches.append(argv)
            return subprocess.CompletedProcess(argv, 0, "", "")
        if argv[:3] == ["gh", "run", "list"]:
            payload, error = self.fake_load_json_command(argv, repo=repo)
            return subprocess.CompletedProcess(argv, 1 if error else 0, json.dumps(payload), error or "")
        if argv[:3] == ["gh", "run", "view"]:
            payload, error = self.fake_load_json_command(argv, repo=repo)
            return subprocess.CompletedProcess(argv, 1 if error else 0, json.dumps(payload), error or "")
        if argv[:3] == ["gh", "repo", "view"]:
            payload, error = self.fake_load_json_command(argv, repo=repo)
            return subprocess.CompletedProcess(argv, 1 if error else 0, json.dumps(payload), error or "")
        if argv[:3] == ["gh", "pr", "checks"]:
            self.pr_checks_calls += 1
            stale_deferred_gate = [
                {
                    "name": "gate",
                    "bucket": "fail",
                    "state": "FAILURE",
                    "link": "https://github.com/seungpyoson/bolt-v2/actions/runs/100",
                    "workflow": "CI",
                }
            ]
            return subprocess.CompletedProcess(argv, 0, json.dumps(stale_deferred_gate), "")
        raise AssertionError(f"unexpected command: {argv}")

    def fake_monotonic(self) -> float:
        self._time += 0.5
        return self._time

    def fake_sleep(self, _seconds: int) -> None:
        self.sleep_calls += 1
        self._time += 1.0


def run_verify_remote_with_harness(harness: VerifyRemoteHarness) -> tuple[int, str, str]:
    args = type("Args", (), {"repo": str(harness.repo)})()
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        result = harness.owner.cmd_verify_remote(args)
    return result, stdout.getvalue(), stderr.getvalue()


def assert_verify_remote_dispatches_draft_full_ci_and_waits_run_scoped() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[
                [],
                [workflow_run(201, status="in_progress", conclusion=None)],
                [workflow_run(201, status="completed", conclusion="success")],
            ],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))
        if len(harness.dispatches) != 1:
            raise AssertionError(harness.dispatches)
        dispatch_text = " ".join(harness.dispatches[0])
        if ".github/workflows/ci.yml" not in dispatch_text or "full_ci=true" not in dispatch_text:
            raise AssertionError(dispatch_text)
        if "final-proof full CI" not in stdout or "just rust-probe suggest" not in stdout:
            raise AssertionError(stdout)
        if harness.pr_checks_calls:
            raise AssertionError("draft dispatch wait must not use aggregate gh pr checks")


def assert_verify_remote_dispatch_wait_does_not_depend_on_local_clock() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[
                [],
                [workflow_run(204, status="in_progress", conclusion=None, created_at="2026-06-13T00:00:01Z")],
                [workflow_run(204, status="completed", conclusion="success", created_at="2026-06-13T00:00:01Z")],
            ],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))
        if len(harness.dispatches) != 1:
            raise AssertionError(harness.dispatches)


def assert_verify_remote_reuses_existing_matching_full_ci_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[[workflow_run(202, status="completed", conclusion="success")]],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))
        if harness.dispatches:
            raise AssertionError(harness.dispatches)


def assert_verify_remote_fails_when_branch_advances_after_dispatch() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[[], [workflow_run(203, status="in_progress", conclusion=None)]],
            advance_after_dispatch=True,
        ) as harness:
            result, _stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 2 or "PR branch advanced during watch" not in stderr:
            raise AssertionError((result, stderr))
        if len(harness.dispatches) != 1:
            raise AssertionError(harness.dispatches)


def assert_verify_remote_waits_on_pending_full_run_over_stale_deferred_gate() -> None:
    owner = load_owner_module()
    stale_deferred = workflow_run(
        301,
        event="pull_request",
        status="completed",
        conclusion="failure",
        created_at="2026-06-13T00:00:00Z",
    )
    pending_full = workflow_run(
        302,
        event="pull_request",
        status="in_progress",
        conclusion=None,
        created_at="2026-06-13T00:02:00Z",
    )
    green_full = workflow_run(
        302,
        event="pull_request",
        status="completed",
        conclusion="success",
        created_at="2026-06-13T00:02:00Z",
    )
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[stale_deferred, pending_full], [stale_deferred, green_full]],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))
        if harness.sleep_calls < 1:
            raise AssertionError("expected verify-remote to wait on the pending full-CI run")


def assert_verify_remote_ready_pr_waits_for_full_run_after_stale_deferred_gate() -> None:
    owner = load_owner_module()
    stale_deferred = workflow_run(
        303,
        event="pull_request",
        status="completed",
        conclusion="failure",
        created_at="2026-06-13T00:00:00Z",
    )
    pending_full = workflow_run(
        304,
        event="pull_request",
        status="in_progress",
        conclusion=None,
        created_at="2026-06-13T00:02:00Z",
    )
    green_full = workflow_run(
        304,
        event="pull_request",
        status="completed",
        conclusion="success",
        created_at="2026-06-13T00:02:00Z",
    )
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[stale_deferred], [stale_deferred, pending_full], [stale_deferred, green_full]],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))
        if harness.sleep_calls < 1:
            raise AssertionError("expected verify-remote to wait past stale deferred gate for ready full-CI run")


def assert_verify_remote_uses_green_full_run_over_stale_deferred_gate() -> None:
    owner = load_owner_module()
    stale_deferred = workflow_run(
        401,
        event="pull_request",
        status="completed",
        conclusion="failure",
        created_at="2026-06-13T00:00:00Z",
    )
    green_full = workflow_run(
        402,
        event="pull_request",
        status="completed",
        conclusion="success",
        created_at="2026-06-13T00:03:00Z",
    )
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[stale_deferred, green_full]],
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 0:
            raise AssertionError((result, stdout, stderr))


def assert_verify_remote_fork_draft_fails_closed() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True, owner="outside-contributor"),
            run_lists=[],
        ) as harness:
            result, _stdout, stderr = run_verify_remote_with_harness(harness)
        expected = (
            "draft fork PRs cannot dispatch upstream full CI; mark the PR ready for review "
            "or have a maintainer move the branch into the upstream repository"
        )
        if result != 2 or expected not in stderr:
            raise AssertionError((result, stderr))


def assert_repository_owner_requires_owner_separator() -> None:
    owner = load_owner_module()
    if owner.repository_owner("bolt-v2") is not None:
        raise AssertionError("bare repository names must not be parsed as owners")
    if owner.repository_owner("seungpyoson/bolt-v2") != "seungpyoson":
        raise AssertionError("owner/repo strings must expose the owner")
    if owner.repository_name("seungpyoson/bolt-v2") != "bolt-v2":
        raise AssertionError("owner/repo strings must expose the repository name")


def assert_verify_remote_api_error_fails_closed() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[],
            run_list_error="API rate limit exceeded while listing workflow runs",
        ) as harness:
            result, _stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 2 or "API rate limit exceeded" not in stderr:
            raise AssertionError((result, stderr))
        if harness.dispatches:
            raise AssertionError(harness.dispatches)


def assert_verify_remote_preflight_rejects_dirty_or_unpushed_head_before_ci() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        calls: list[list[str]] = []

        def dirty_run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
            calls.append(argv)
            if argv[:3] == ["git", "status", "--porcelain"]:
                return subprocess.CompletedProcess(argv, 0, " M scripts/rust_verification.py\n", "")
            raise AssertionError(f"unexpected command after dirty status: {argv}")

        original_run_capture = owner.run_capture
        try:
            owner.run_capture = dirty_run_capture
            _head, _branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.run_capture = original_run_capture
        if error != "verify-remote requires a clean worktree, including untracked files":
            raise AssertionError(error)
        if calls != [["git", "status", "--porcelain", "--untracked-files=normal"]]:
            raise AssertionError(calls)

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()

        def stale_tracking_ref_run_capture(
            argv: list[str], *, repo: pathlib.Path
        ) -> subprocess.CompletedProcess[str]:
            if argv == ["git", "status", "--porcelain", "--untracked-files=normal"]:
                return subprocess.CompletedProcess(argv, 0, "", "")
            if argv == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(argv, 0, VERIFY_REMOTE_HEAD + "\n", "")
            if argv == ["git", "rev-parse", "@{u}"]:
                return subprocess.CompletedProcess(argv, 0, ("b" * 40) + "\n", "")
            if argv == ["git", "branch", "--show-current"]:
                return subprocess.CompletedProcess(argv, 0, "codex/slice\n", "")
            if argv == ["git", "config", "branch.codex/slice.remote"]:
                return subprocess.CompletedProcess(argv, 0, "origin\n", "")
            if argv == ["git", "config", "branch.codex/slice.merge"]:
                return subprocess.CompletedProcess(argv, 0, "refs/heads/codex/slice\n", "")
            if argv == ["git", "ls-remote", "--heads", "origin", "codex/slice"]:
                return subprocess.CompletedProcess(
                    argv,
                    0,
                    f"{VERIFY_REMOTE_HEAD}\trefs/heads/codex/slice\n",
                    "",
                )
            raise AssertionError(f"unexpected command: {argv}")

        original_run_capture = owner.run_capture
        try:
            owner.run_capture = stale_tracking_ref_run_capture
            head, branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.run_capture = original_run_capture
        if error is not None:
            raise AssertionError(error)
        if head != VERIFY_REMOTE_HEAD or branch != "codex/slice":
            raise AssertionError((head, branch))

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()

        def unpushed_run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
            if argv == ["git", "status", "--porcelain", "--untracked-files=normal"]:
                return subprocess.CompletedProcess(argv, 0, "", "")
            if argv == ["git", "rev-parse", "HEAD"]:
                return subprocess.CompletedProcess(argv, 0, VERIFY_REMOTE_HEAD + "\n", "")
            if argv == ["git", "rev-parse", "@{u}"]:
                return subprocess.CompletedProcess(argv, 0, ("b" * 40) + "\n", "")
            if argv == ["git", "branch", "--show-current"]:
                return subprocess.CompletedProcess(argv, 0, "codex/slice\n", "")
            if argv == ["git", "config", "branch.codex/slice.remote"]:
                return subprocess.CompletedProcess(argv, 0, "origin\n", "")
            if argv == ["git", "config", "branch.codex/slice.merge"]:
                return subprocess.CompletedProcess(argv, 0, "refs/heads/codex/slice\n", "")
            if argv == ["git", "ls-remote", "--heads", "origin", "codex/slice"]:
                return subprocess.CompletedProcess(
                    argv,
                    0,
                    f"{'b' * 40}\trefs/heads/codex/slice\n",
                    "",
                )
            raise AssertionError(f"unexpected command: {argv}")

        original_run_capture = owner.run_capture
        try:
            owner.run_capture = unpushed_run_capture
            _head, _branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.run_capture = original_run_capture
        if error != "verify-remote requires HEAD to be pushed to the upstream branch":
            raise AssertionError(error)


def assert_ci_logs_command_uses_exact_head_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        write_verify_remote_config(repo)
        harness = VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[workflow_run(301, event="pull_request", status="in_progress", conclusion=None)]],
        )
        emitted: list[int] = []
        original_emit = owner.emit_failed_job_diagnostics
        try:
            with harness:
                owner.emit_failed_job_diagnostics = lambda **kwargs: emitted.append(int(kwargs["run"]["databaseId"]))
                args = type("Args", (), {"repo": str(repo)})()
                result = owner.cmd_ci_logs(args)
        finally:
            owner.emit_failed_job_diagnostics = original_emit
        if result != 0:
            raise AssertionError(result)
        if emitted != [301]:
            raise AssertionError(emitted)


def assert_ci_logs_command_uses_draft_aware_events() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        write_verify_remote_config(repo)
        harness = VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[
                [
                    workflow_run(401, event="workflow_dispatch", status="completed", conclusion="failure"),
                    workflow_run(402, event="pull_request", status="in_progress", conclusion=None),
                ]
            ],
        )
        emitted: list[int] = []
        original_emit = owner.emit_failed_job_diagnostics
        try:
            with harness:
                owner.emit_failed_job_diagnostics = lambda **kwargs: emitted.append(int(kwargs["run"]["databaseId"]))
                args = type("Args", (), {"repo": str(repo)})()
                result = owner.cmd_ci_logs(args)
        finally:
            owner.emit_failed_job_diagnostics = original_emit
        if result != 0:
            raise AssertionError(result)
        if emitted != [402]:
            raise AssertionError(emitted)


def assert_ci_logs_command_uses_draft_workflow_dispatch_events() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        write_verify_remote_config(repo)
        harness = VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=True),
            run_lists=[
                [
                    workflow_run(501, event="pull_request", status="completed", conclusion="failure"),
                    workflow_run(502, event="workflow_dispatch", status="in_progress", conclusion=None),
                ]
            ],
        )
        emitted: list[int] = []
        original_emit = owner.emit_failed_job_diagnostics
        try:
            with harness:
                owner.emit_failed_job_diagnostics = lambda **kwargs: emitted.append(int(kwargs["run"]["databaseId"]))
                args = type("Args", (), {"repo": str(repo)})()
                result = owner.cmd_ci_logs(args)
        finally:
            owner.emit_failed_job_diagnostics = original_emit
        if result != 0:
            raise AssertionError(result)
        if emitted != [502]:
            raise AssertionError(emitted)


def assert_ci_logs_command_fails_when_diagnostics_unavailable() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        write_verify_remote_config(repo)
        harness = VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
            run_lists=[[workflow_run(302, event="pull_request", status="in_progress", conclusion=None)]],
        )
        original_jobs = owner.workflow_run_jobs
        try:
            with harness:
                owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (None, "jobs API unavailable")
                args = type("Args", (), {"repo": str(repo)})()
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    result = owner.cmd_ci_logs(args)
        finally:
            owner.workflow_run_jobs = original_jobs
        if result != 2:
            raise AssertionError((result, stderr.getvalue()))
        if "jobs API unavailable" not in stderr.getvalue():
            raise AssertionError(stderr.getvalue())


def main() -> int:
    assert_repo_local_owner_contract()
    assert_fmt_avoids_managed_cache_lock()
    assert_system_python_contract()
    assert_oversized_policy_fails_closed()
    assert_remote_diagnostics_policy_loads()
    assert_verify_remote_dispatches_draft_full_ci_and_waits_run_scoped()
    assert_verify_remote_dispatch_wait_does_not_depend_on_local_clock()
    assert_verify_remote_reuses_existing_matching_full_ci_run()
    assert_verify_remote_fails_when_branch_advances_after_dispatch()
    assert_verify_remote_waits_on_pending_full_run_over_stale_deferred_gate()
    assert_verify_remote_ready_pr_waits_for_full_run_after_stale_deferred_gate()
    assert_verify_remote_uses_green_full_run_over_stale_deferred_gate()
    assert_verify_remote_fork_draft_fails_closed()
    assert_repository_owner_requires_owner_separator()
    assert_verify_remote_api_error_fails_closed()
    assert_verify_remote_preflight_rejects_dirty_or_unpushed_head_before_ci()
    assert_ci_logs_command_uses_exact_head_run()
    assert_ci_logs_command_uses_draft_aware_events()
    assert_ci_logs_command_uses_draft_workflow_dispatch_events()
    assert_ci_logs_command_fails_when_diagnostics_unavailable()
    print("OK: Rust verification owner self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
