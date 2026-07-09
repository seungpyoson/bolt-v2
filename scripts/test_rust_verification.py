#!/usr/bin/env python3
"""Self-tests for the repo-local Rust verification owner."""

from __future__ import annotations

import contextlib
import argparse
import io
import os
import json
import importlib.util
import pathlib
import shlex
import subprocess
import sys
import tempfile
import textwrap
import tomllib

from test_fixtures import load_owner_module, rust_verification_policy_text, write_executable, write_policy


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


def load_ci_provenance_module() -> object:
    path = REPO_ROOT / "scripts" / "ci_provenance.py"
    spec = importlib.util.spec_from_file_location("ci_provenance_under_test", path)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load ci_provenance.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def assert_ci_provenance_gate_name_helpers_stay_in_parity() -> None:
    owner = load_owner_module()
    provenance = load_ci_provenance_module()
    if owner.GATE_NAME_KEYS != provenance.GATE_NAME_KEYS:
        raise AssertionError((owner.GATE_NAME_KEYS, provenance.GATE_NAME_KEYS))
    for value in (
        "gate",
        "gate ",
        " gate",
        "gate\nignored=1",
        "gate\tignored",
        "${{ github.ref }}",
        "gate }}",
        "backtester-gate-iteration",
    ):
        owner_result = owner.github_actions_output_safe_check_name(value)
        provenance_result = provenance.github_actions_output_safe_check_name(value)
        if owner_result != provenance_result:
            raise AssertionError((value, owner_result, provenance_result))
    collision_cases = (
        {"gate_required": "gate", "gate_iteration": "gate"},
        {"gate_required": "gate", "backtester_required": "gate"},
    )
    for gate_names in collision_cases:
        owner_errors = owner.gate_name_collision_errors(gate_names)
        provenance_errors = provenance.gate_name_collision_errors(gate_names)
        if owner_errors != provenance_errors:
            raise AssertionError((gate_names, owner_errors, provenance_errors))


def parse_log(path: pathlib.Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, value = line.split("=", 1)
        values[key] = value
    return values


def assert_minimal_toml_accepts_quoted_keys() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "policy.toml"
        path.write_text(
            textwrap.dedent(
                """\
                [merge_queue_preflight.required_check_workflows]
                "backtester-gate" = "Backtester CI"
                "host-health" = "CI"
                """
            ),
            encoding="utf-8",
        )
        parsed = owner.parse_minimal_toml(path)
    workflows = parsed["merge_queue_preflight"]["required_check_workflows"]
    if workflows != {"backtester-gate": "Backtester CI", "host-health": "CI"}:
        raise AssertionError(workflows)


def assert_minimal_toml_accepts_multiline_string_arrays() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "policy.toml"
        path.write_text(
            textwrap.dedent(
                """\
                [merge_queue_preflight]
                source_fence_full_profile_pathspecs = [
                  "scripts",
                  "justfile",
                  "ci/rust-verification.toml",
                ]
                """
            ),
            encoding="utf-8",
        )
        parsed = owner.parse_minimal_toml(path)
    pathspecs = parsed["merge_queue_preflight"]["source_fence_full_profile_pathspecs"]
    if pathspecs != ["scripts", "justfile", "ci/rust-verification.toml"]:
        raise AssertionError(pathspecs)


def assert_minimal_toml_matches_tomllib_for_rust_policy() -> None:
    minimal_toml_path = REPO_ROOT / "scripts" / "minimal_toml.py"
    spec = importlib.util.spec_from_file_location("minimal_toml_under_test", minimal_toml_path)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load scripts/minimal_toml.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    policy = REPO_ROOT / "ci" / "rust-verification.toml"

    with policy.open("rb") as handle:
        expected = tomllib.load(handle)
    parsed = module.load(policy)
    if parsed != expected:
        raise AssertionError("minimal_toml.py must match tomllib for ci/rust-verification.toml")


def assert_minimal_toml_rejects_non_ascii_bare_digits() -> None:
    minimal_toml_path = REPO_ROOT / "scripts" / "minimal_toml.py"
    spec = importlib.util.spec_from_file_location("minimal_toml_under_test", minimal_toml_path)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load scripts/minimal_toml.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "policy.toml"
        path.write_text("schema_version = \u00b2\n", encoding="utf-8")
        try:
            module.load(path, error_cls=RuntimeError)
        except RuntimeError as exc:
            if "unsupported value" not in str(exc):
                raise AssertionError(f"unexpected minimal TOML error: {exc}") from exc
        else:
            raise AssertionError("non-ASCII bare digits must stay in the parser error path")


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
            or "for full remote feedback on a draft PR: run: just verify-remote" not in next_steps
            or (
                "for merge proof: mark the PR ready, then run: just verify-remote "
                "to wait for the required PR gate, or use the merge-queue gate"
            )
            not in next_steps
            or "for merge proof: run: just verify-remote" in next_steps
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


def assert_rust_probe_guidance_distinguishes_feedback_from_proof() -> None:
    owner = load_owner_module()
    stale_fragments = (
        "run just verify-remote for proof",
        "verify-remote is final proof",
        "draft verify-remote is proof",
        "verify-remote only for final exact-head full-CI proof",
        "For final proof, use exact-head PR CI evidence through `just verify-remote`",
        "Full CI is proof. Rust Probe is debugging.",
        "dispatch Backtester CI with " + "full_ci" + "=true for this branch or mark ready",
    )
    operator_surfaces = (
        SCRIPT,
        REPO_ROOT / "AGENTS.md",
        REPO_ROOT / "docs" / "ci" / "ubicloud-cost-governance.md",
        REPO_ROOT / ".github" / "workflows" / "backtester-ci.yml",
    )
    for path in operator_surfaces:
        source = path.read_text(encoding="utf-8")
        if any(fragment in source for fragment in stale_fragments):
            raise AssertionError(f"{path.relative_to(REPO_ROOT)} contains stale verify-remote proof guidance")
    if any(fragment in owner.RUST_PROBE_HELP_EPILOG for fragment in stale_fragments):
        raise AssertionError(owner.RUST_PROBE_HELP_EPILOG)

    stdout = io.StringIO()
    run = {
        "databaseId": 1001,
        "event": "workflow_dispatch",
        "headSha": VERIFY_REMOTE_HEAD,
        "status": "completed",
        "conclusion": "success",
        "createdAt": "2026-06-13T00:00:00Z",
        "displayTitle": "Rust Probe abc123 check-lib",
        "url": "https://github.com/seungpyoson/bolt-v2/actions/runs/1001",
    }
    with contextlib.redirect_stdout(stdout):
        result = owner.evaluate_rust_probe_run(run, head=VERIFY_REMOTE_HEAD, probe_id="abc123")
    output = stdout.getvalue()
    if result != 0:
        raise AssertionError((result, output))
    if "draft verify-remote is feedback only" not in output:
        raise AssertionError(output)
    if any(fragment in output for fragment in stale_fragments):
        raise AssertionError(output)


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


def assert_validate_policy_rejects_unknown_cheap_lane_just_recipe() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        policy = repo / "ci" / "rust-verification.toml"
        policy.write_text(
            policy.read_text(encoding="utf-8").replace(
                "poll_interval_seconds = 1\n",
                'poll_interval_seconds = 1\ncheap_lane_just_recipes = ["missing-cheap-lane-recipe"]\n',
                1,
            ),
            encoding="utf-8",
        )

        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())

    if result.returncode != 2:
        raise AssertionError((result.returncode, result.stdout, result.stderr))
    if "missing from justfile" not in result.stderr:
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
            run_name_iteration = "CI [dispatch:iteration]"
            proof_gate_job = "gate"

            [ci_provenance.gate_names]
            gate_required = "gate"
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
    display_title: str | None = None,
) -> dict[str, object]:
    if display_title is None:
        display_title = "CI [dispatch:iteration]" if event == "workflow_dispatch" else "CI"
    return {
        "databaseId": database_id,
        "attempt": 1,
        "event": event,
        "headSha": VERIFY_REMOTE_HEAD,
        "status": status,
        "conclusion": conclusion,
        "createdAt": created_at,
        "displayTitle": display_title,
        "url": f"https://github.com/seungpyoson/bolt-v2/actions/runs/{database_id}",
    }


def workflow_jobs(*, gate_conclusion: str | None = "success") -> dict[str, object]:
    return {
        "jobs": [
            {
                "databaseId": 9000,
                "name": "gate",
                "status": "completed",
                "conclusion": "success",
            },
            {
                "databaseId": 9001,
                "name": "gate-iteration",
                "status": "completed",
                "conclusion": gate_conclusion,
            }
        ]
    }


class VerifyRemoteHarness:
    def __init__(
        self,
        owner: object,
        repo: pathlib.Path,
        *,
        pr: dict[str, object],
        run_lists: list[list[dict[str, object]]],
        jobs_by_run_id: dict[int, dict[str, object]] | None = None,
        run_list_error: str | None = None,
        advance_after_dispatch: bool = False,
    ) -> None:
        self.owner = owner
        self.repo = repo
        self.pr = pr
        self.run_lists = list(run_lists)
        self.jobs_by_run_id = jobs_by_run_id or {}
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
        if argv[:3] == ["gh", "run", "view"] and argv[4:6] == ["--json", "jobs"]:
            run_id = int(argv[3])
            return self.jobs_by_run_id.get(run_id, workflow_jobs()), None
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


def assert_verify_remote_rejects_draft_full_ci_dispatch_removed() -> None:
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
        ) as harness:
            result, stdout, stderr = run_verify_remote_with_harness(harness)
        if result != 2:
            raise AssertionError((result, stdout, stderr))
        if harness.dispatches:
            raise AssertionError(harness.dispatches)
        if "draft PRs cannot run full CI through workflow_dispatch" not in stderr:
            raise AssertionError(stderr)
        if "just rust-probe" not in stderr:
            raise AssertionError(stderr)


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
        expected = "draft PRs cannot run full CI through workflow_dispatch"
        if result != 2 or expected not in stderr:
            raise AssertionError((result, stderr))


def assert_verify_remote_api_error_fails_closed() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_verify_remote_config(repo)
        with VerifyRemoteHarness(
            owner,
            repo,
            pr=verify_remote_pr(is_draft=False),
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
        calls: list[tuple[str, ...]] = []

        def dirty_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
            calls.append(args)
            if args == ("status", "--porcelain", "--untracked-files=normal"):
                return " M scripts/rust_verification.py\n", None
            raise AssertionError(f"unexpected command after dirty status: {args}")

        original_git_output = owner.git_output
        try:
            owner.git_output = dirty_git_output
            _head, _branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.git_output = original_git_output
        if error != "verify-remote requires a clean worktree, including untracked files":
            raise AssertionError(error)
        if calls != [("status", "--porcelain", "--untracked-files=normal")]:
            raise AssertionError(calls)

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()

        def stale_tracking_ref_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
            if args == ("status", "--porcelain", "--untracked-files=normal"):
                return "", None
            if args == ("rev-parse", "HEAD"):
                return VERIFY_REMOTE_HEAD, None
            if args == ("branch", "--show-current"):
                return "codex/slice", None
            if args == ("config", "branch.codex/slice.remote"):
                return "origin", None
            if args == ("config", "branch.codex/slice.merge"):
                return "refs/heads/codex/slice", None
            if args == ("remote", "get-url", "--push", "--all", "origin"):
                return "https://example.invalid/repo.git", None
            if args == ("ls-remote", "--heads", "--", "https://example.invalid/repo.git", "codex/slice"):
                return f"{VERIFY_REMOTE_HEAD}\trefs/heads/codex/slice\n", None
            raise AssertionError(f"unexpected command: {args}")

        original_git_output = owner.git_output
        try:
            owner.git_output = stale_tracking_ref_git_output
            head, branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.git_output = original_git_output
        if error is not None:
            raise AssertionError(error)
        if head != VERIFY_REMOTE_HEAD or branch != "codex/slice":
            raise AssertionError((head, branch))

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()

        def unpushed_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
            if args == ("status", "--porcelain", "--untracked-files=normal"):
                return "", None
            if args == ("rev-parse", "HEAD"):
                return VERIFY_REMOTE_HEAD, None
            if args == ("branch", "--show-current"):
                return "codex/slice", None
            if args == ("config", "branch.codex/slice.remote"):
                return "origin", None
            if args == ("config", "branch.codex/slice.merge"):
                return "refs/heads/codex/slice", None
            if args == ("remote", "get-url", "--push", "--all", "origin"):
                return "https://example.invalid/repo.git", None
            if args == ("ls-remote", "--heads", "--", "https://example.invalid/repo.git", "codex/slice"):
                return f"{'b' * 40}\trefs/heads/codex/slice\n", None
            raise AssertionError(f"unexpected command: {args}")

        original_git_output = owner.git_output
        try:
            owner.git_output = unpushed_git_output
            _head, _branch, error = owner.ensure_verify_remote_preconditions(repo)
        finally:
            owner.git_output = original_git_output
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


def assert_ci_logs_command_fails_closed_for_draft_pr_without_full_ci() -> None:
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
                stdout = io.StringIO()
                stderr = io.StringIO()
                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    result = owner.cmd_ci_logs(args)
        finally:
            owner.emit_failed_job_diagnostics = original_emit
        if result != 2:
            raise AssertionError((result, stdout.getvalue(), stderr.getvalue()))
        if emitted:
            raise AssertionError(emitted)
        if "draft PRs cannot run full CI through workflow_dispatch" not in stderr.getvalue():
            raise AssertionError(stderr.getvalue())


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


@contextlib.contextmanager
def _patched_environ(values: dict[str, "str | None"]):
    saved = {key: os.environ.get(key) for key in values}
    try:
        for key, value in values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        yield
    finally:
        for key, previous in saved.items():
            if previous is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = previous


REMOTE_COMPILE_CACHE_POLICY = {
    "enabled": True,
    "enable_env": "BOLT_RUST_VERIFICATION_SCCACHE",
    "ci_env": "GITHUB_ACTIONS",
    "wrapper_env": "SCCACHE_PATH",
    "wrapper_program": "sccache",
}


REMOTE_FAST_LINKER_POLICY = {
    "enabled": True,
    "ci_env": "GITHUB_ACTIONS",
    "linker_env": "BOLT_RUST_FAST_LINKER",
    "programs": ["mold", "lld"],
}


def assert_validate_remote_compile_cache_policy_contract() -> None:
    owner = load_owner_module()
    # A well-formed opt-in policy validates; an absent table means the feature is
    # simply off and is also allowed.
    owner.validate_remote_compile_cache_policy({"remote_compile_cache": dict(REMOTE_COMPILE_CACHE_POLICY)})
    owner.validate_remote_compile_cache_policy({})
    rejects = [
        {**REMOTE_COMPILE_CACHE_POLICY, "enabled": False},
        {**REMOTE_COMPILE_CACHE_POLICY, "enable_env": "bad lower"},
        {**REMOTE_COMPILE_CACHE_POLICY, "ci_env": "NOT_GITHUB_ACTIONS"},
        {**REMOTE_COMPILE_CACHE_POLICY, "wrapper_program": "not-sccache"},
        {**REMOTE_COMPILE_CACHE_POLICY, "unexpected_key": "x"},
    ]
    for bad in rejects:
        try:
            owner.validate_remote_compile_cache_policy({"remote_compile_cache": bad})
        except owner.PolicyError:
            continue
        raise AssertionError(f"expected PolicyError for remote_compile_cache={bad!r}")


def assert_managed_remote_compile_cache_env_fails_open() -> None:
    owner = load_owner_module()
    policy = {"remote_compile_cache": dict(REMOTE_COMPILE_CACHE_POLICY)}

    # Active only when every gate is satisfied: opt-in == "1", CI marker == "true",
    # and an explicit, clean wrapper path is present.
    with _patched_environ(
        {
            "BOLT_RUST_VERIFICATION_SCCACHE": "1",
            "GITHUB_ACTIONS": "true",
            "SCCACHE_PATH": "/opt/sccache/sccache",
        }
    ):
        if owner.managed_remote_compile_cache_env(policy) != {"RUSTC_WRAPPER": "/opt/sccache/sccache"}:
            raise AssertionError("wrapper must be injected when every gate is satisfied")

    # Each gate, when unmet, yields no wrapper (today's build) -- never an error.
    gate_off_cases = [
        {"BOLT_RUST_VERIFICATION_SCCACHE": "0", "GITHUB_ACTIONS": "true", "SCCACHE_PATH": "/opt/sccache/sccache"},
        {"BOLT_RUST_VERIFICATION_SCCACHE": "1", "GITHUB_ACTIONS": None, "SCCACHE_PATH": "/opt/sccache/sccache"},
        {"BOLT_RUST_VERIFICATION_SCCACHE": "1", "GITHUB_ACTIONS": "false", "SCCACHE_PATH": "/opt/sccache/sccache"},
    ]
    for env in gate_off_cases:
        with _patched_environ(env):
            if owner.managed_remote_compile_cache_env(policy) != {}:
                raise AssertionError(f"wrapper must stay off when a gate is unmet: {env!r}")

    # Gates satisfied but a missing/malformed wrapper path degrades to no wrapper
    # (fail-open) rather than raising -- the cache must never fail the build.
    for path in (None, "", "/opt/sc cache/sccache", "/opt/sccache/other"):
        with _patched_environ(
            {
                "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                "GITHUB_ACTIONS": "true",
                "SCCACHE_PATH": path,
            }
        ):
            if owner.managed_remote_compile_cache_env(policy) != {}:
                raise AssertionError(f"malformed wrapper must fail open to no wrapper: {path!r}")


def assert_validate_remote_fast_linker_policy_contract() -> None:
    owner = load_owner_module()
    owner.validate_remote_fast_linker_policy({"remote_fast_linker": dict(REMOTE_FAST_LINKER_POLICY)})
    owner.validate_remote_fast_linker_policy({})
    rejects = [
        {**REMOTE_FAST_LINKER_POLICY, "enabled": False},
        {**REMOTE_FAST_LINKER_POLICY, "ci_env": "NOT_GITHUB_ACTIONS"},
        {**REMOTE_FAST_LINKER_POLICY, "linker_env": "bad lower"},
        {**REMOTE_FAST_LINKER_POLICY, "programs": []},
        {**REMOTE_FAST_LINKER_POLICY, "programs": ["lld", "mold"]},
        {**REMOTE_FAST_LINKER_POLICY, "unexpected_key": "x"},
    ]
    for bad in rejects:
        try:
            owner.validate_remote_fast_linker_policy({"remote_fast_linker": bad})
        except owner.PolicyError:
            continue
        raise AssertionError(f"expected PolicyError for remote_fast_linker={bad!r}")


def assert_managed_remote_fast_linker_env_selects_available_program() -> None:
    owner = load_owner_module()
    policy = {
        "target_namespace": "rust-verification-fast-linker-test",
        "remote_fast_linker": dict(REMOTE_FAST_LINKER_POLICY),
    }

    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / "bin"
        bin_dir.mkdir()
        write_executable(bin_dir / "cc", "#!/usr/bin/env bash\nexit 0\n")
        write_executable(bin_dir / "mold", "#!/usr/bin/env bash\nexit 0\n")
        base_path = os.environ.get("PATH", "")
        with _patched_environ(
            {
                "PATH": f"{bin_dir}{os.pathsep}{base_path}",
                "RUST_VERIFICATION_ROOT_BASE": str(pathlib.Path(tmp) / "rv-root"),
                "GITHUB_ACTIONS": "true",
                "BOLT_RUST_FAST_LINKER": "mold",
            }
        ):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper_dir = pathlib.Path(env["PATH"].split(os.pathsep)[0])
        if "RUSTFLAGS" in env:
            raise AssertionError("fast linker path must not inject RUSTFLAGS because it invalidates sccache keys")
        if not (wrapper_dir / "cc").is_file():
            raise AssertionError("fast linker path must prepend a generated cc wrapper")
        if bin_dir.as_posix() not in env["PATH"]:
            raise AssertionError(env)

    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / "bin"
        bin_dir.mkdir()
        write_executable(bin_dir / "cc", "#!/usr/bin/env bash\nexit 0\n")
        write_executable(bin_dir / "lld", "#!/usr/bin/env bash\nexit 0\n")
        base_path = os.environ.get("PATH", "")
        with _patched_environ(
            {
                "PATH": f"{bin_dir}{os.pathsep}{base_path}",
                "RUST_VERIFICATION_ROOT_BASE": str(pathlib.Path(tmp) / "rv-root"),
                "GITHUB_ACTIONS": "true",
                "BOLT_RUST_FAST_LINKER": "lld",
            }
        ):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper_dir = pathlib.Path(env["PATH"].split(os.pathsep)[0])
        if "RUSTFLAGS" in env:
            raise AssertionError("fast linker path must not inject RUSTFLAGS because it invalidates sccache keys")
        if not (wrapper_dir / "cc").is_file():
            raise AssertionError("fast linker path must prepend a generated cc wrapper")
        if bin_dir.as_posix() not in env["PATH"]:
            raise AssertionError(env)

    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / "bin"
        bin_dir.mkdir()
        fake_real_cc = bin_dir / "cc"
        write_executable(fake_real_cc, "#!/usr/bin/env bash\nexit 0\n")
        write_executable(bin_dir / "mold", "#!/usr/bin/env bash\nexit 0\n")
        rv_root = pathlib.Path(tmp) / "rv-root"
        with _patched_environ({"RUST_VERIFICATION_ROOT_BASE": str(rv_root)}):
            wrapper_dir = owner.target_dir(REPO_ROOT, policy) / "fast-linker-bin"
        wrapper_dir.mkdir(parents=True)
        fake_recursive_cc = wrapper_dir / "cc"
        write_executable(fake_recursive_cc, "#!/usr/bin/env bash\nexit 99\n")
        base_path = os.environ.get("PATH", "")
        with _patched_environ(
            {
                "PATH": f"{wrapper_dir}{os.pathsep}{bin_dir}{os.pathsep}{base_path}",
                "RUST_VERIFICATION_ROOT_BASE": str(rv_root),
                "GITHUB_ACTIONS": "true",
                "BOLT_RUST_FAST_LINKER": "mold",
            }
        ):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper = pathlib.Path(env["PATH"].split(os.pathsep)[0]) / "cc"
        wrapper_text = wrapper.read_text(encoding="utf-8")
        if f"real_cc={shlex.quote(str(fake_real_cc))}" not in wrapper_text:
            raise AssertionError(f"fast linker wrapper must resolve real cc outside wrapper dir: {wrapper_text!r}")
        if str(fake_recursive_cc) in wrapper_text:
            raise AssertionError(f"fast linker wrapper must not resolve itself as real cc: {wrapper_text!r}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        fake_real_cc = bin_dir / "cc"
        write_executable(fake_real_cc, "#!/usr/bin/env bash\nexit 0\n")
        write_executable(bin_dir / "mold", "#!/usr/bin/env bash\nexit 0\n")
        rv_root = tmp_path / "rv-root"
        with _patched_environ({"RUST_VERIFICATION_ROOT_BASE": str(rv_root)}):
            wrapper_dir = owner.target_dir(REPO_ROOT, policy) / "fast-linker-bin"
        wrapper_dir.mkdir(parents=True)
        fake_recursive_cc = wrapper_dir / "cc"
        write_executable(fake_recursive_cc, "#!/usr/bin/env bash\nexit 99\n")
        wrapper_dir_link = tmp_path / "fast-linker-bin-link"
        wrapper_dir_link.symlink_to(wrapper_dir, target_is_directory=True)
        base_path = os.environ.get("PATH", "")
        with _patched_environ(
            {
                "PATH": f"{wrapper_dir_link}{os.pathsep}{bin_dir}{os.pathsep}{base_path}",
                "RUST_VERIFICATION_ROOT_BASE": str(rv_root),
                "GITHUB_ACTIONS": "true",
                "BOLT_RUST_FAST_LINKER": "mold",
            }
        ):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper = pathlib.Path(env["PATH"].split(os.pathsep)[0]) / "cc"
        wrapper_text = wrapper.read_text(encoding="utf-8")
        if f"real_cc={shlex.quote(str(fake_real_cc))}" not in wrapper_text:
            raise AssertionError(
                f"fast linker wrapper must resolve real cc outside symlinked wrapper dir: {wrapper_text!r}"
            )
        if str(wrapper_dir_link / "cc") in wrapper_text or str(fake_recursive_cc) in wrapper_text:
            raise AssertionError(f"fast linker wrapper must not resolve symlinked wrapper as real cc: {wrapper_text!r}")

    gate_off_cases = [
        {"GITHUB_ACTIONS": None, "BOLT_RUST_FAST_LINKER": "mold"},
        {"GITHUB_ACTIONS": "false", "BOLT_RUST_FAST_LINKER": "mold"},
        {"GITHUB_ACTIONS": "true", "BOLT_RUST_FAST_LINKER": None},
        {"GITHUB_ACTIONS": "true", "BOLT_RUST_FAST_LINKER": "gold"},
    ]
    for env_values in gate_off_cases:
        with _patched_environ(env_values):
            if owner.managed_remote_fast_linker_env(REPO_ROOT, policy) != {}:
                raise AssertionError(f"fast linker must stay off when a gate is unmet: {env_values!r}")


def assert_managed_env_scrubs_then_reinjects_wrapper() -> None:
    owner = load_owner_module()
    policy = {
        "target_namespace": "rust-verification-sccache-test",
        "remote_compile_cache": dict(REMOTE_COMPILE_CACHE_POLICY),
    }
    # Gate ON: a pre-existing RUSTC_WRAPPER is scrubbed first (hermetic), then the
    # opt-in re-injects the resolved sccache path.
    with _patched_environ(
        {
            "RUSTC_WRAPPER": "/evil/wrapper",
            "BOLT_RUST_VERIFICATION_SCCACHE": "1",
            "GITHUB_ACTIONS": "true",
            "SCCACHE_PATH": "/opt/sccache/sccache",
        }
    ):
        env = owner.managed_env(REPO_ROOT, policy)
    if env.get("RUSTC_WRAPPER") != "/opt/sccache/sccache":
        raise AssertionError("managed_env must scrub a pre-existing wrapper and re-inject the sccache path")
    # Gate OFF (no CI marker): wrapper stays scrubbed -- it must not leak to local
    # runs or to any lane that does not set the opt-in.
    with _patched_environ(
        {
            "RUSTC_WRAPPER": "/evil/wrapper",
            "BOLT_RUST_VERIFICATION_SCCACHE": "1",
            "GITHUB_ACTIONS": None,
            "SCCACHE_PATH": "/opt/sccache/sccache",
        }
    ):
        env = owner.managed_env(REPO_ROOT, policy)
    if "RUSTC_WRAPPER" in env:
        raise AssertionError("managed_env must not inject a wrapper outside CI (GITHUB_ACTIONS unset)")


def remote_compile_policy_text() -> str:
    return (
        rust_verification_policy_text(target_namespace="rust-verification-remote-cache-test")
        + textwrap.dedent(
            """\

            [remote_compile_cache]
            enabled = true
            enable_env = "BOLT_RUST_VERIFICATION_SCCACHE"
            ci_env = "GITHUB_ACTIONS"
            wrapper_env = "SCCACHE_PATH"
            wrapper_program = "sccache"
            """
        )
    )


def install_owner_process_spies(owner: object, calls: list[tuple[list[str], str | None]], results: list[int]) -> tuple[object, object, object]:
    def fake_disk_preflight(_repo: pathlib.Path, _policy: dict[str, object]) -> None:
        calls.append((["__disk_preflight__"], None))
        return None

    @contextlib.contextmanager
    def fake_cache_lock(_policy: dict[str, object], *, exclusive: bool):
        calls.append((["__cache_lock__", str(exclusive)], None))
        yield

    def fake_run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
        calls.append((list(argv), env.get("RUSTC_WRAPPER")))
        if results:
            return results.pop(0)
        return 0

    original_preflight = owner.disk_preflight_refusal_payload
    original_cache_lock = owner.cache_lock
    original_run_process = owner.run_process
    owner.disk_preflight_refusal_payload = fake_disk_preflight
    owner.cache_lock = fake_cache_lock
    owner.run_process = fake_run_process
    return original_preflight, original_cache_lock, original_run_process


def restore_owner_process_spies(owner: object, originals: tuple[object, object, object]) -> None:
    owner.disk_preflight_refusal_payload = originals[0]
    owner.cache_lock = originals[1]
    owner.run_process = originals[2]


def assert_managed_test_splits_nextest_run_inside_owner() -> None:
    owner = load_owner_module()
    calls: list[tuple[list[str], str | None]] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, policy_text=remote_compile_policy_text())
        originals = install_owner_process_spies(owner, calls, [])
        try:
            with _patched_environ(
                {
                    "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                    "GITHUB_ACTIONS": "true",
                    "SCCACHE_PATH": "/opt/sccache/sccache",
                }
            ):
                result = owner.cmd_run(
                    argparse.Namespace(
                        repo=str(repo),
                        command="test",
                        args=["--config-file", "nextest.toml", "--", "--skip", "slow_case"],
                        args_separator=False,
                    )
                )
        finally:
            restore_owner_process_spies(owner, originals)
    if result != 0:
        raise AssertionError(result)
    process_calls = [call for call in calls if call[0][0] != "__cache_lock__"]
    if [call[0] for call in process_calls].count(["__disk_preflight__"]) != 1:
        raise AssertionError(process_calls)
    run_calls = [call for call in process_calls if call[0][0] == "cargo"]
    expected_compile = [
        "cargo",
        "nextest",
        "run",
        "--locked",
        "--no-run",
        "--config-file",
        "nextest.toml",
    ]
    expected_test = [
        "cargo",
        "nextest",
        "run",
        "--locked",
        "--config-file",
        "nextest.toml",
        "--",
        "--skip",
        "slow_case",
    ]
    if run_calls != [(expected_compile, "/opt/sccache/sccache"), (expected_test, None)]:
        raise AssertionError(run_calls)


def assert_managed_test_uses_configured_compile_args() -> None:
    owner = load_owner_module()
    calls: list[tuple[list[str], str | None]] = []
    policy_text = remote_compile_policy_text().replace(
        '"--locked", "--no-run"', '"--locked", "--workspace", "--no-run"', 1
    )
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, policy_text=policy_text)
        originals = install_owner_process_spies(owner, calls, [])
        try:
            with _patched_environ(
                {
                    "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                    "GITHUB_ACTIONS": "true",
                    "SCCACHE_PATH": "/opt/sccache/sccache",
                }
            ):
                result = owner.cmd_run(
                    argparse.Namespace(
                        repo=str(repo),
                        command="test",
                        args=["--config-file", "nextest.toml", "--ff"],
                        args_separator=False,
                    )
                )
        finally:
            restore_owner_process_spies(owner, originals)
    if result != 0:
        raise AssertionError(result)
    run_calls = [call for call in calls if call[0][0] == "cargo"]
    expected_compile = [
        "cargo",
        "nextest",
        "run",
        "--locked",
        "--workspace",
        "--no-run",
        "--config-file",
        "nextest.toml",
    ]
    expected_test = [
        "cargo",
        "nextest",
        "run",
        "--locked",
        "--config-file",
        "nextest.toml",
        "--ff",
    ]
    if run_calls != [(expected_compile, "/opt/sccache/sccache"), (expected_test, None)]:
        raise AssertionError(run_calls)


def assert_nextest_compile_preflight_omits_run_only_flags() -> None:
    owner = load_owner_module()
    run_args = [
        "nextest",
        "run",
        "--locked",
        "--config-file",
        "nextest.toml",
        "--no-fail-fast",
        "--fail-fast",
        "--ff",
        "--nff",
        "--max-fail",
        "1",
        "--max-fail=all",
    ]
    compile_args = owner.nextest_run_compile_preflight_args(run_args)
    if compile_args != ["nextest", "run", "--locked", "--config-file", "nextest.toml", "--no-run"]:
        raise AssertionError(compile_args)


def assert_nextest_compile_failure_retries_without_retrying_tests() -> None:
    owner = load_owner_module()
    calls: list[tuple[list[str], str | None]] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, policy_text=remote_compile_policy_text())
        originals = install_owner_process_spies(owner, calls, [86, 0, 42])
        try:
            with _patched_environ(
                {
                    "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                    "GITHUB_ACTIONS": "true",
                    "SCCACHE_PATH": "/opt/sccache/sccache",
                }
            ):
                result = owner.cmd_run(
                    argparse.Namespace(repo=str(repo), command="test", args=["--no-fail-fast"], args_separator=False)
                )
        finally:
            restore_owner_process_spies(owner, originals)
    if result != 42:
        raise AssertionError((result, calls))
    run_calls = [call for call in calls if call[0][0] == "cargo"]
    if len(run_calls) != 3:
        raise AssertionError(run_calls)
    if run_calls[0][1] != "/opt/sccache/sccache" or run_calls[1][1] is not None or run_calls[2][1] is not None:
        raise AssertionError(run_calls)
    if run_calls[2][0] != ["cargo", "nextest", "run", "--locked", "--no-fail-fast"]:
        raise AssertionError(run_calls)
    if "--no-fail-fast" in run_calls[0][0]:
        raise AssertionError(run_calls)


def assert_direct_nextest_run_splits_inside_owner() -> None:
    owner = load_owner_module()
    calls: list[tuple[list[str], str | None]] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, policy_text=remote_compile_policy_text())
        originals = install_owner_process_spies(owner, calls, [])
        try:
            with _patched_environ(
                {
                    "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                    "GITHUB_ACTIONS": "true",
                    "SCCACHE_PATH": "/opt/sccache/sccache",
                }
            ):
                result = owner.cmd_cargo(
                    argparse.Namespace(repo=str(repo), args=["--", "nextest", "run", "--locked", "-p", "bolt-v2"])
                )
        finally:
            restore_owner_process_spies(owner, originals)
    if result != 0:
        raise AssertionError(result)
    run_calls = [call for call in calls if call[0][0] == "cargo"]
    if run_calls != [
        (["cargo", "nextest", "run", "--locked", "-p", "bolt-v2", "--no-run"], "/opt/sccache/sccache"),
        (["cargo", "nextest", "run", "--locked", "-p", "bolt-v2"], None),
    ]:
        raise AssertionError(run_calls)


def assert_managed_env_scrubs_then_injects_fast_linker_wrapper() -> None:
    owner = load_owner_module()
    policy = {
        "target_namespace": "rust-verification-fast-linker-test",
        "remote_fast_linker": dict(REMOTE_FAST_LINKER_POLICY),
    }
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / "bin"
        bin_dir.mkdir()
        cc_log = pathlib.Path(tmp) / "cc.log"
        write_executable(
            bin_dir / "cc",
            "#!/usr/bin/env bash\n"
            f"printf '%s\\n' \"$@\" >> {cc_log}\n"
            "exit 0\n",
        )
        write_executable(bin_dir / "mold", "#!/usr/bin/env bash\nexit 0\n")
        base_path = os.environ.get("PATH", "")
        with _patched_environ(
            {
                "PATH": f"{bin_dir}{os.pathsep}{base_path}",
                "RUSTFLAGS": "-C link-arg=-fuse-ld=gold",
                "RUST_VERIFICATION_ROOT_BASE": str(pathlib.Path(tmp) / "rv-root"),
                "GITHUB_ACTIONS": "true",
                "BOLT_RUST_FAST_LINKER": "mold",
            }
        ):
            env = owner.managed_env(REPO_ROOT, policy)
        wrapper_dir = pathlib.Path(env["PATH"].split(os.pathsep)[0])
        wrapper = wrapper_dir / "cc"
        if not wrapper.is_file():
            raise AssertionError("managed_env must prepend a generated cc wrapper for the configured fast linker")
        run = subprocess.run(["cc", "input.o", "-o", "output"], executable=str(wrapper), check=False)
        if run.returncode != 0:
            raise AssertionError(f"fast linker wrapper failed with rc={run.returncode}")
        if "RUSTFLAGS" in env:
            raise AssertionError("managed_env must keep RUSTFLAGS scrubbed so sccache keys remain stable")
        logged_args = cc_log.read_text(encoding="utf-8").splitlines()
        if logged_args[:1] != ["-fuse-ld=mold"]:
            raise AssertionError(
                f"fast linker wrapper must add mold link arg before link command args: {logged_args!r}"
            )
        cc_log.write_text("", encoding="utf-8")
        pass_through_cases = [
            (["-c", "input.c"], "compile-only"),
            (["-S", "input.c"], "assembly-only"),
            (["-E", "input.c"], "preprocess-only"),
            (["-M", "input.c"], "dependency-only"),
            (["-MM", "input.c"], "user-dependency-only"),
            (["-print-prog-name=ld"], "compiler-query"),
            (["-dumpmachine"], "compiler-query"),
            (["-dumpspecs"], "compiler-query"),
            (["--help=warnings"], "compiler-query"),
            (["--version"], "compiler-query"),
            (["-fuse-ld=gold", "input.o", "-o", "output"], "explicit-linker"),
        ]
        for args, description in pass_through_cases:
            cc_log.write_text("", encoding="utf-8")
            run = subprocess.run(["cc", *args], executable=str(wrapper), check=False)
            if run.returncode != 0:
                raise AssertionError(f"fast linker wrapper {description} pass-through failed with rc={run.returncode}")
            pass_through_args = cc_log.read_text(encoding="utf-8").splitlines()
            if "-fuse-ld=mold" in pass_through_args:
                raise AssertionError(
                    f"fast linker wrapper must not add link args to {description} commands: {pass_through_args!r}"
                )
        cc_log.write_text("", encoding="utf-8")
        run = subprocess.run(["cc", "-Xlinker", "-E", "input.o", "-o", "output"], executable=str(wrapper), check=False)
        if run.returncode != 0:
            raise AssertionError(f"fast linker wrapper link command with forwarded -E failed with rc={run.returncode}")
        forwarded_link_args = cc_log.read_text(encoding="utf-8").splitlines()
        if forwarded_link_args[:1] != ["-fuse-ld=mold"]:
            raise AssertionError(
                "fast linker wrapper must still add mold when -E is forwarded as a linker argument: "
                f"{forwarded_link_args!r}"
            )

    with _patched_environ(
        {
            "RUSTFLAGS": "-C link-arg=-fuse-ld=gold",
            "GITHUB_ACTIONS": None,
            "BOLT_RUST_FAST_LINKER": "mold",
        }
    ):
        env = owner.managed_env(REPO_ROOT, policy)
    if "RUSTFLAGS" in env:
        raise AssertionError("managed_env must not inject fast linker flags outside CI")


def assert_fast_linker_programs_command_reads_policy() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        policy_text = (
            rust_verification_policy_text()
            + textwrap.dedent(
                """\

                [remote_fast_linker]
                enabled = true
                ci_env = "GITHUB_ACTIONS"
                linker_env = "BOLT_RUST_FAST_LINKER"
                programs = ["mold", "lld"]
                """
            )
        )
        write_policy(repo, policy_text=policy_text)
        result = run_owner(
            ["fast-linker-programs", "--repo", str(repo)],
            env=os.environ.copy(),
        )
    if result.returncode != 0:
        raise AssertionError((result.returncode, result.stdout, result.stderr))
    if result.stdout.splitlines() != ["mold", "lld"]:
        raise AssertionError(result.stdout)


def run_global_cargo_config_assertion(
    repo: pathlib.Path, *, home: pathlib.Path, root_base: pathlib.Path, cargo_home: pathlib.Path | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    if cargo_home is not None:
        env["CARGO_HOME"] = str(cargo_home)
    env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
    return run_owner(["assert-global-cargo-target-dir", "--repo", str(repo)], env=env)


def assert_global_cargo_target_dir_config_is_created_and_idempotent() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        first = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if first.returncode != 0:
            raise AssertionError((first.returncode, first.stdout, first.stderr))
        config = home / ".cargo" / "config.toml"
        first_content = config.read_text(encoding="utf-8")
        if "[build]" not in first_content or f'target-dir = "{expected_target}"' not in first_content:
            raise AssertionError(first_content)

        second = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if second.returncode != 0:
            raise AssertionError((second.returncode, second.stdout, second.stderr))
        if config.read_text(encoding="utf-8") != first_content:
            raise AssertionError("global Cargo config assertion is not idempotent")


def assert_global_cargo_target_dir_config_preserves_existing_content() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_text(
            textwrap.dedent(
                """\
                [net]
                git-fetch-with-cli = true

                [build]
                rustflags = ["-Dwarnings"]
                """
            ),
            encoding="utf-8",
        )
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding="utf-8")
        for preserved in ("[net]", "git-fetch-with-cli = true", 'rustflags = ["-Dwarnings"]'):
            if preserved not in content:
                raise AssertionError(content)
        if f'target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)
        second = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if second.returncode != 0:
            raise AssertionError((second.returncode, second.stdout, second.stderr))
        if config.read_text(encoding="utf-8") != content:
            raise AssertionError("assertion rewrote existing config on second run")


def assert_global_cargo_target_dir_config_refuses_conflict() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        original = textwrap.dedent(
            """\
            [build]
            target-dir = "/tmp/raw-target"
            """
        )
        config.write_text(original, encoding="utf-8")

        result = run_global_cargo_config_assertion(
            repo,
            home=home,
            root_base=tmp_path / "rust-root",
        )
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "build.target-dir" not in result.stderr or "/tmp/raw-target" not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding="utf-8") != original:
            raise AssertionError("conflicting global Cargo config was rewritten")


def assert_global_cargo_target_dir_config_accepts_resolved_equivalent_path() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        actual_root = tmp_path / "actual-rust-root"
        alias_root = tmp_path / "alias-rust-root"
        actual_root.mkdir()
        alias_root.symlink_to(actual_root, target_is_directory=True)
        original = textwrap.dedent(
            f"""\
            [build]
            target-dir = "{alias_root / "bolt-v2" / "target"}"
            """
        )
        config.write_text(original, encoding="utf-8")

        result = run_global_cargo_config_assertion(repo, home=home, root_base=actual_root)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "already-configured" not in result.stdout:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding="utf-8") != original:
            raise AssertionError("resolved-equivalent global Cargo config was rewritten")


def assert_global_cargo_target_dir_config_uses_effective_cargo_home() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        cargo_home = tmp_path / "cargo-home"
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        result = run_global_cargo_config_assertion(
            repo, home=home, root_base=root_base, cargo_home=cargo_home,
        )
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        effective_config = cargo_home / "config.toml"
        home_config = home / ".cargo" / "config.toml"
        if f'target-dir = "{expected_target}"' not in effective_config.read_text(encoding="utf-8"):
            raise AssertionError(effective_config.read_text(encoding="utf-8"))
        if home_config.exists():
            raise AssertionError("assertion wrote HOME Cargo config while CARGO_HOME was set")


def assert_global_cargo_target_dir_config_updates_legacy_config_when_present() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        legacy_config = home / ".cargo" / "config"
        legacy_config.parent.mkdir(parents=True)
        legacy_config.write_text("[net]\ngit-fetch-with-cli = true\n", encoding="utf-8")
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = legacy_config.read_text(encoding="utf-8")
        if "[net]" not in content or f'target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)
        if (home / ".cargo" / "config.toml").exists():
            raise AssertionError("assertion wrote config.toml even though Cargo will read legacy config")


def assert_global_cargo_target_dir_config_preserves_dotted_build_keys() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_text(
            textwrap.dedent(
                """\
                build.rustflags = ["-Dwarnings"]

                [net]
                git-fetch-with-cli = true
                """
            ),
            encoding="utf-8",
        )
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding="utf-8")
        if 'build.rustflags = ["-Dwarnings"]' not in content:
            raise AssertionError(content)
        if f'build.target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)


def assert_global_cargo_target_dir_config_handles_quoted_build_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_text('[ "build" ]\nrustflags = ["-Dwarnings"]\n', encoding="utf-8")
        root_base = tmp_path / "rust-root"
        expected_target = (root_base / "bolt-v2" / "target").resolve()

        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding="utf-8")
        if f'target-dir = "{expected_target}"' not in content or 'rustflags = ["-Dwarnings"]' not in content:
            raise AssertionError(content)


def assert_global_cargo_target_dir_config_refuses_inline_build_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        original = 'build = { rustflags = ["-Dwarnings"] }\n'
        config.write_text(original, encoding="utf-8")

        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / "rust-root")
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "cannot be safely edited" not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding="utf-8") != original:
            raise AssertionError("unsupported inline Cargo config was rewritten")


def assert_global_cargo_target_dir_config_reports_non_utf8_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_bytes(b"\xff")

        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / "rust-root")
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "Traceback" in result.stderr:
            raise AssertionError(result.stderr)


def assert_global_cargo_target_dir_config_preserves_symlink() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / "home"
        config = home / ".cargo" / "config.toml"
        target_config = tmp_path / "dotfiles" / "cargo-config.toml"
        config.parent.mkdir(parents=True)
        target_config.parent.mkdir()
        target_config.write_text("[net]\ngit-fetch-with-cli = true\n", encoding="utf-8")
        config.symlink_to(target_config)

        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / "rust-root")
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if not config.is_symlink():
            raise AssertionError("Cargo config symlink was replaced")
        if "target-dir" not in target_config.read_text(encoding="utf-8"):
            raise AssertionError(target_config.read_text(encoding="utf-8"))


def assert_setup_recipe_asserts_global_cargo_target_dir() -> None:
    source = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    if "assert-global-cargo-target-dir" not in source:
        raise AssertionError("just setup must assert the machine-global Cargo target-dir")


def main() -> int:
    assert_repo_local_owner_contract()
    assert_validate_remote_compile_cache_policy_contract()
    assert_managed_remote_compile_cache_env_fails_open()
    assert_managed_env_scrubs_then_reinjects_wrapper()
    assert_managed_test_splits_nextest_run_inside_owner()
    assert_managed_test_uses_configured_compile_args()
    assert_nextest_compile_preflight_omits_run_only_flags()
    assert_nextest_compile_failure_retries_without_retrying_tests()
    assert_direct_nextest_run_splits_inside_owner()
    assert_validate_remote_fast_linker_policy_contract()
    assert_managed_remote_fast_linker_env_selects_available_program()
    assert_managed_env_scrubs_then_injects_fast_linker_wrapper()
    assert_fast_linker_programs_command_reads_policy()
    assert_ci_provenance_gate_name_helpers_stay_in_parity()
    assert_rust_probe_guidance_distinguishes_feedback_from_proof()
    assert_fmt_avoids_managed_cache_lock()
    assert_minimal_toml_accepts_quoted_keys()
    assert_minimal_toml_accepts_multiline_string_arrays()
    assert_minimal_toml_matches_tomllib_for_rust_policy()
    assert_minimal_toml_rejects_non_ascii_bare_digits()
    assert_system_python_contract()
    assert_oversized_policy_fails_closed()
    assert_validate_policy_rejects_unknown_cheap_lane_just_recipe()
    assert_remote_diagnostics_policy_loads()
    assert_verify_remote_rejects_draft_full_ci_dispatch_removed()
    assert_verify_remote_waits_on_pending_full_run_over_stale_deferred_gate()
    assert_verify_remote_ready_pr_waits_for_full_run_after_stale_deferred_gate()
    assert_verify_remote_uses_green_full_run_over_stale_deferred_gate()
    assert_verify_remote_fork_draft_fails_closed()
    assert_verify_remote_api_error_fails_closed()
    assert_verify_remote_preflight_rejects_dirty_or_unpushed_head_before_ci()
    assert_ci_logs_command_uses_exact_head_run()
    assert_ci_logs_command_uses_draft_aware_events()
    assert_ci_logs_command_fails_closed_for_draft_pr_without_full_ci()
    assert_ci_logs_command_fails_when_diagnostics_unavailable()
    assert_global_cargo_target_dir_config_is_created_and_idempotent()
    assert_global_cargo_target_dir_config_preserves_existing_content()
    assert_global_cargo_target_dir_config_refuses_conflict()
    assert_global_cargo_target_dir_config_accepts_resolved_equivalent_path()
    assert_global_cargo_target_dir_config_uses_effective_cargo_home()
    assert_global_cargo_target_dir_config_updates_legacy_config_when_present()
    assert_global_cargo_target_dir_config_preserves_dotted_build_keys()
    assert_global_cargo_target_dir_config_handles_quoted_build_table()
    assert_global_cargo_target_dir_config_refuses_inline_build_table()
    assert_global_cargo_target_dir_config_reports_non_utf8_without_traceback()
    assert_global_cargo_target_dir_config_preserves_symlink()
    assert_setup_recipe_asserts_global_cargo_target_dir()
    print("OK: Rust verification owner self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
