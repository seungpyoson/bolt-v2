#!/usr/bin/env python3
"""Self-tests for CI provenance emission and resolution."""

from __future__ import annotations

import contextlib
import http.server
import importlib.util
import io
import hashlib
import json
import os
import pathlib
import queue
import socketserver
import sys
import tempfile
import threading
import urllib.request
import zipfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "ci_provenance.py"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"
OTHER_SHA = "b" * 40
RUN_ID = 24623219988
CHECK_SUITE_ID = 65233803543
NEXTEST_FINGERPRINT = f"nextest-archive-v2-Linux-X64-test-profile-shards-4-{'a' * 64}"
NEXTEST_FINGERPRINT_ARTIFACT = f"nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-{'a' * 64}"
CAPTURE_PROVENANCE_CONFIG_DIGEST = "19260091d9871d34cf51fd2fa797ffbb12aa420b5aa059d0829dd7736d409993"

CONFIG_TOML = """
schema_version = 1

[workflows.ci]
host-health = "github_hosted"

[workflows.actionlint]
actionlint = "github_hosted"

[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"

[ci_provenance]
schema_version = 1
artifact_name_template = "ci-provenance-attempt-{run_attempt}"
workflow_key = "ci"
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"
fingerprint_source = "meter"

[ci_provenance.full_ci]
required_jobs = [
  "detector",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "nextest-fingerprint",
  "test-archive",
  "test",
]
conditional_jobs = ["build"]
conditional_job_outputs = { build = "detector.build_required" }

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.nextest-fingerprint]
check_name = "nextest fingerprint"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.test]
check_name = "test"

[ci_provenance.full_ci.jobs.build]
check_name = "build"
conditional = "detector.build_required"

[ci_provenance.deploy]
artifact_name = "bolt-v2-binary"
artifact_upload_if = "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}"
artifact_retention_days = 3
artifact_lookback_age_seconds = 259200
require_source_event = "push"
require_source_branch = "main"
require_gate_check = true

[ci_provenance.dispatch]
workflow_input = "full_ci"
run_name_default = "CI"
run_name_full = "CI [dispatch:full]"
run_name_iteration = "CI [dispatch:iteration]"
proof_gate_job = "gate"

[ci_provenance.gate_names]
gate_required = "gate"
gate_iteration = "gate-iteration"
gate_dispatch_full = "gate-dispatch"
backtester_required = "backtester-gate"
backtester_iteration = "backtester-gate-iteration"
backtester_dispatch_full = "backtester-gate-dispatch"

[ci_provenance.required_checks.gate]
context = "gate"
reporter = "ci.yml gate summary job"
integration_id = 15368
required = true
target = true
runs_on_tags = true
supports_carry_forward = true
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "tag_reuse"]
carry_forward = []

[ci_provenance.required_checks.backtester-gate]
context = "backtester-gate"
reporter = "backtester-ci.yml gate job"
integration_id = 15368
required = true
target = true
runs_on_tags = true
supports_carry_forward = true
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.backtester-gate.proof_rule]
fresh = ["full", "tag_reuse"]
carry_forward = []

[ci_provenance.required_checks.host-health]
context = "host-health"
reporter = "ci.yml host-health lane"
integration_id = 15368
required = true
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []

[ci_provenance.required_checks.actionlint]
context = "actionlint"
reporter = "actionlint.yml"
integration_id = 15368
required = true
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.actionlint.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []

[ci_provenance.required_checks.coverage-enforcer]
context = "coverage-enforcer"
reporter = "self"
integration_id = 15368
required = false
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.coverage-enforcer.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []

[ci_provenance.docs]
safe_paths = [
  "AGENTS.md",
  "CLAUDE.md",
  "GEMINI.md",
  "REASONIX.md",
  "LICENSE",
  "SECURITY.md",
  ".github/ISSUE_TEMPLATE/**",
  ".claude/**",
  ".codex/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".specify/**",
]
forbidden_ignored_build_paths = [
  ".claude/rust-verification.toml",
]
non_heavy_required_jobs = ["detector"]

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 1209600

[ci_provenance.artifacts]
retention_days = 14

[ci_provenance.policy]
draft_pr_synchronize = "iteration"
draft_pr_opened = "iteration"
draft_pr_reopened = "iteration"
draft_pr_edited = "iteration"
converted_to_draft = "iteration"
ready_pr = "iteration"
ready_pr_edited_no_base = "iteration"
ready_pr_reopened = "iteration"
ready_for_review = "iteration"
docs = "docs"
workflow_dispatch = "iteration"
workflow_dispatch_full_ci = "full"
main_push = "full"
merge_group = "full"
mergify_temp_pr = "full"
tag = "tag_reuse"
unknown_event = "full"

[ci_provenance.mergify]
temp_pr_head_ref_prefix = "mergify/merge-queue/"
mergify_temp_pr_actor_id = 37929162

[ci_provenance.policy.override]
force_full_ci = false
ignore_emit_failure = false
"""

CONFIG_TOML_REORDERED = """
schema_version = 1

[unrelated]
value = "kept out of the provenance digest"

[workflows.actionlint]
actionlint = "github_hosted"

[workflows.ci]
host-health = "github_hosted"

[ci_provenance.policy.override]
ignore_emit_failure = false
force_full_ci = false

[ci_provenance.policy]
unknown_event = "full"
tag = "tag_reuse"
mergify_temp_pr = "full"
merge_group = "full"
main_push = "full"
workflow_dispatch_full_ci = "full"
workflow_dispatch = "iteration"
docs = "docs"
ready_for_review = "iteration"
ready_pr_reopened = "iteration"
ready_pr_edited_no_base = "iteration"
ready_pr = "iteration"
converted_to_draft = "iteration"
draft_pr_edited = "iteration"
draft_pr_reopened = "iteration"
draft_pr_opened = "iteration"
draft_pr_synchronize = "iteration"

[ci_provenance.mergify]
temp_pr_head_ref_prefix = "mergify/merge-queue/"
mergify_temp_pr_actor_id = 37929162

[ci_provenance.artifacts]
retention_days = 14

[ci_provenance.api_limits]
max_lookback_age_seconds = 1209600
max_lookback_pages = 10
run_artifacts_per_page = 100
run_jobs_per_page = 100
workflow_runs_per_page = 100

[ci_provenance.dispatch]
workflow_input = "full_ci"
run_name_default = "CI"
run_name_full = "CI [dispatch:full]"
run_name_iteration = "CI [dispatch:iteration]"
proof_gate_job = "gate"

[ci_provenance.gate_names]
backtester_dispatch_full = "backtester-gate-dispatch"
backtester_iteration = "backtester-gate-iteration"
backtester_required = "backtester-gate"
gate_dispatch_full = "gate-dispatch"
gate_iteration = "gate-iteration"
gate_required = "gate"

[ci_provenance.required_checks.coverage-enforcer.proof_rule]
carry_forward = []
fresh = ["full", "docs", "iteration"]

[ci_provenance.required_checks.coverage-enforcer]
arrivals = ["pull_request", "merge_group"]
supports_carry_forward = false
runs_on_tags = false
target = true
required = false
integration_id = 15368
reporter = "self"
context = "coverage-enforcer"

[ci_provenance.required_checks.actionlint.proof_rule]
carry_forward = []
fresh = ["full", "docs", "iteration"]

[ci_provenance.required_checks.actionlint]
arrivals = ["pull_request", "merge_group"]
supports_carry_forward = false
runs_on_tags = false
target = true
required = true
integration_id = 15368
reporter = "actionlint.yml"
context = "actionlint"

[ci_provenance.required_checks.host-health.proof_rule]
carry_forward = []
fresh = ["full", "docs", "iteration"]

[ci_provenance.required_checks.host-health]
arrivals = ["pull_request", "merge_group"]
supports_carry_forward = false
runs_on_tags = false
target = true
required = true
integration_id = 15368
reporter = "ci.yml host-health lane"
context = "host-health"

[ci_provenance.required_checks.backtester-gate.proof_rule]
carry_forward = []
fresh = ["full", "tag_reuse"]

[ci_provenance.required_checks.backtester-gate]
arrivals = ["pull_request", "merge_group"]
supports_carry_forward = true
runs_on_tags = true
target = true
required = true
integration_id = 15368
reporter = "backtester-ci.yml gate job"
context = "backtester-gate"

[ci_provenance.required_checks.gate.proof_rule]
carry_forward = []
fresh = ["full", "tag_reuse"]

[ci_provenance.required_checks.gate]
arrivals = ["pull_request", "merge_group"]
supports_carry_forward = true
runs_on_tags = true
target = true
required = true
integration_id = 15368
reporter = "ci.yml gate summary job"
context = "gate"

[ci_provenance.docs]
non_heavy_required_jobs = ["detector"]
forbidden_ignored_build_paths = [
  ".claude/rust-verification.toml",
]
safe_paths = [
  "AGENTS.md",
  "CLAUDE.md",
  "GEMINI.md",
  "REASONIX.md",
  "LICENSE",
  "SECURITY.md",
  ".github/ISSUE_TEMPLATE/**",
  ".claude/**",
  ".codex/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".specify/**",
]

[ci_provenance.deploy]
require_gate_check = true
require_source_branch = "main"
require_source_event = "push"
artifact_lookback_age_seconds = 259200
artifact_retention_days = 3
artifact_upload_if = "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}"
artifact_name = "bolt-v2-binary"

[ci_provenance.full_ci.jobs.build]
conditional = "detector.build_required"
check_name = "build"

[ci_provenance.full_ci.jobs.test]
check_name = "test"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.nextest-fingerprint]
check_name = "nextest fingerprint"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci]
conditional_job_outputs = { build = "detector.build_required" }
conditional_jobs = ["build"]
required_jobs = [
  "detector",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "nextest-fingerprint",
  "test-archive",
  "test",
]

[ci_provenance]
fingerprint_source = "meter"
workflow_path = ".github/workflows/ci.yml"
workflow_name = "CI"
workflow_key = "ci"
artifact_name_template = "ci-provenance-attempt-{run_attempt}"
schema_version = 1

[meter]
fingerprint_workflow = "ci"
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
"""


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("ci_provenance", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load ci_provenance.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_config(
    tmpdir: pathlib.Path,
    text: str = CONFIG_TOML,
    name: str = "github-actions-runners.toml",
) -> pathlib.Path:
    path = tmpdir / name
    path.write_text(text, encoding="utf-8")
    return path


def strip_ci_provenance_config(config_text: str) -> str:
    lines = config_text.splitlines()
    kept: list[str] = []
    skip = False
    for line in lines:
        if line.startswith("[ci_provenance"):
            skip = True
            continue
        if skip and line.startswith("["):
            skip = False
        if not skip:
            kept.append(line)
    return "\n".join(kept).rstrip() + "\n"


def run_cli(args: list[str]) -> tuple[int, str, str]:
    module = load_script()
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        try:
            code = module.main(args)
        except SystemExit as exc:
            code = int(exc.code or 0)
    return code, stdout.getvalue(), stderr.getvalue()


@contextlib.contextmanager
def patched_env(values: dict[str, str]):
    old_values = {key: os.environ.get(key) for key in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, value in old_values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def run_cli_with_event_sender(args: list[str], sender: object) -> tuple[int, str, str]:
    with patched_env({"EVENT_SENDER_ID": str(sender)}):
        return run_cli(args)


def output_dict(stdout: str) -> dict[str, str]:
    return dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)


def assert_fails(fragment: str, args: list[str]) -> None:
    code, stdout, stderr = run_cli(args)
    if code == 0:
        raise AssertionError(f"expected failure for {args}, stdout={stdout!r}")
    combined = stdout + stderr
    if fragment not in combined:
        raise AssertionError(f"expected {fragment!r} in output, got {combined!r}")


def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001 - script exposes domain errors.
        if fragment not in str(exc):
            raise AssertionError(f"expected {fragment!r}, got {exc}") from exc
        return
    raise AssertionError(f"expected {fragment!r}")


def workflow_digest() -> str:
    return hashlib.sha256((REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()).hexdigest()


def valid_record(module, config_path: pathlib.Path) -> dict[str, object]:
    return {
        "schema_version": 1,
        "kind": "full-ci",
        "repository": "seungpyoson/bolt-v2",
        "workflow_path": ".github/workflows/ci.yml",
        "workflow_digest": workflow_digest(),
        "provenance_config_digest": module.provenance_config_digest(config_path),
        "head_sha": SHA,
        "tested_sha": SHA,
        "run_id": RUN_ID,
        "run_attempt": 1,
        "check_suite_id": CHECK_SUITE_ID,
        "event": "push",
        "head_branch": "main",
        "pull_request": {"number": None, "base_sha": None},
        "required_jobs": {
            "detector": "success",
            "deny": "success",
            "clippy": "success",
            "check-aarch64": "success",
            "source-fence": "success",
            "nextest-fingerprint": "success",
            "test-archive": "success",
            "test": "success",
        },
        "conditional_jobs": {"build": {"required": True, "result": "success"}},
        "nextest_fingerprint": None,
        "created_at": "2026-06-13T00:00:00Z",
    }


def record_with_fingerprint(
    module,
    config_path: pathlib.Path,
    fingerprint: object = NEXTEST_FINGERPRINT,
    **overrides: object,
) -> dict[str, object]:
    record = valid_record(module, config_path)
    record["nextest_fingerprint"] = fingerprint
    record.update(overrides)
    return record


def run_payload(**overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "id": RUN_ID,
        "name": "CI",
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "main",
        "head_sha": SHA,
        "status": "completed",
        "conclusion": "success",
        "run_attempt": 1,
        "check_suite_id": CHECK_SUITE_ID,
        "created_at": "2026-06-13T00:00:00Z",
        "updated_at": "2026-06-13T00:10:00Z",
        "html_url": "https://github.com/seungpyoson/bolt-v2/actions/runs/24623219988",
    }
    payload.update(overrides)
    return payload


def provenance_artifact(**overrides: object) -> dict[str, object]:
    artifact_id = overrides.get("id", 123)
    run_id = overrides.get("run_id", RUN_ID)
    run_attempt = overrides.get("run_attempt", 1)
    payload: dict[str, object] = {
        "id": artifact_id,
        "name": f"ci-provenance-attempt-{run_attempt}",
        "expired": False,
        "archive_download_url": f"artifact://{artifact_id}",
        "workflow_run": {
            "id": run_id,
            "head_branch": "main",
            "head_sha": SHA,
        },
    }
    payload.update(overrides)
    return payload


def fingerprint_artifact(**overrides: object) -> dict[str, object]:
    artifact_id = overrides.get("id", 456)
    run_id = overrides.get("run_id", RUN_ID)
    name = overrides.get("name", NEXTEST_FINGERPRINT_ARTIFACT)
    payload: dict[str, object] = {
        "id": artifact_id,
        "name": name,
        "expired": False,
        "workflow_run": {
            "id": run_id,
            "head_branch": "main",
            "head_sha": SHA,
        },
    }
    payload.update(overrides)
    return payload


def job_payload(name: str, conclusion: object = "success", status: object = "completed") -> dict[str, object]:
    return {"name": name, "status": status, "conclusion": conclusion}


def required_job_payloads(build_conclusion: object = "success") -> list[dict[str, object]]:
    return [
        job_payload("detector"),
        job_payload("deny"),
        job_payload("clippy"),
        job_payload("check-aarch64"),
        job_payload("source-fence"),
        job_payload("nextest fingerprint"),
        job_payload("nextest archive"),
        job_payload("test"),
        job_payload("build", conclusion=build_conclusion),
    ]


def with_required_job_conclusion(
    jobs: list[dict[str, object]], name: str, conclusion: object
) -> list[dict[str, object]]:
    for job in jobs:
        if job["name"] == name:
            job["conclusion"] = conclusion
            return jobs
    raise AssertionError(f"required job not found: {name}")


def artifact_zip(record: dict[str, object]) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("ci-provenance.json", json.dumps(record))
    return buffer.getvalue()


class FakeGitHub:
    def __init__(
        self,
        *,
        runs_pages: list[list[dict[str, object]]] | object,
        jobs_by_run_id: dict[int, dict[str, object]] | None = None,
        artifacts_by_run_id: dict[int, dict[str, object]] | None = None,
        records_by_artifact_id: dict[int, dict[str, object]] | None = None,
        workflow_bytes: bytes | None = None,
    ) -> None:
        self.runs_pages = runs_pages
        self.jobs_by_run_id = jobs_by_run_id or {}
        self.artifacts_by_run_id = artifacts_by_run_id or {}
        self.records_by_artifact_id = records_by_artifact_id or {}
        self.workflow_bytes = workflow_bytes
        self.queries: list[tuple[str, dict[str, str] | None]] = []

    def json(
        self,
        repo: str,
        token: str,
        path: str,
        query: dict[str, str] | None = None,
    ) -> dict[str, object]:
        self.queries.append((path, query))
        if path in {"actions/runs", "actions/workflows/ci.yml/runs"}:
            if not isinstance(self.runs_pages, list):
                return {"workflow_runs": self.runs_pages}
            page = int((query or {}).get("page", "1"))
            runs = self.runs_pages[page - 1] if page <= len(self.runs_pages) else []
            return {"workflow_runs": runs}
        if path.startswith("actions/runs/") and path.endswith("/artifacts"):
            run_id = int(path.split("/")[2])
            return self.artifacts_by_run_id.get(run_id, {"artifacts": []})
        if path.startswith("actions/runs/") and path.endswith("/jobs"):
            run_id = int(path.split("/")[2])
            return self.jobs_by_run_id.get(run_id, {"jobs": required_job_payloads()})
        raise AssertionError(f"unexpected JSON request {path} {query}")

    def bytes(self, repo: str, token: str, url: str) -> bytes:
        if url.startswith("artifact://"):
            artifact_id = int(url.removeprefix("artifact://"))
            return artifact_zip(self.records_by_artifact_id[artifact_id])
        if url.startswith("https://raw.githubusercontent.com/"):
            if self.workflow_bytes is not None:
                return self.workflow_bytes
            return (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()
        raise AssertionError(f"unexpected bytes request {url}")


def assert_unknown_mode_fails() -> None:
    assert_fails("unknown mode", ["not-a-mode"])


def assert_missing_config_table_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), strip_ci_provenance_config(CONFIG_TOML))
        assert_fails("missing [ci_provenance]", ["emit-full-ci", "--config", str(config)])


def assert_positive_int_config_rejects_booleans() -> None:
    module = load_script()
    cases = {
        "ci_provenance.api_limits.max_lookback_age_seconds must be a positive integer": CONFIG_TOML.replace(
            "max_lookback_age_seconds = 1209600",
            "max_lookback_age_seconds = true",
            1,
        ),
        "ci_provenance.artifacts.retention_days must be a positive integer": CONFIG_TOML.replace(
            "retention_days = 14",
            "retention_days = true",
            1,
        ),
        "ci_provenance.full_ci.jobs.test.shard_count must be a positive integer": CONFIG_TOML.replace(
            '[ci_provenance.full_ci.jobs.test]\ncheck_name = "test"',
            (
                '[ci_provenance.full_ci.jobs.test]\n'
                'check_name_template = "test ({shard}/{shard_count})"\n'
                "shard_count = true"
            ),
        ),
    }
    with tempfile.TemporaryDirectory() as tmp:
        for expected, text in cases.items():
            config = write_config(pathlib.Path(tmp), text)
            assert_raises(expected, lambda config=config: module.load_config(config))


def assert_deploy_artifact_window_uses_short_deploy_policy() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config_path = write_config(tmp_path, CONFIG_TOML)
        config = module.load_config(config_path)
        if config.deploy_artifact_retention_days != 3:
            raise AssertionError(
                "deploy binary retention must use the deploy-specific 3-day window, "
                f"got {config.deploy_artifact_retention_days}"
            )
        if config.deploy_artifact_lookback_age_seconds != 259200:
            raise AssertionError(
                "deploy binary lookup must use the deploy-specific 3-day lookback, "
                f"got {config.deploy_artifact_lookback_age_seconds}"
            )

        cases = {
            "ci_provenance.deploy.artifact_retention_days must be a positive integer": CONFIG_TOML.replace(
                "artifact_retention_days = 3",
                "artifact_retention_days = true",
                1,
            ),
            "ci_provenance.deploy.artifact_lookback_age_seconds must be a positive integer": CONFIG_TOML.replace(
                "artifact_lookback_age_seconds = 259200",
                "artifact_lookback_age_seconds = true",
                1,
            ),
            "ci_provenance.deploy.artifact_lookback_age_seconds must not exceed artifact retention": CONFIG_TOML.replace(
                "artifact_lookback_age_seconds = 259200",
                "artifact_lookback_age_seconds = 259201",
                1,
            ),
        }
        for expected, text in cases.items():
            config = write_config(tmp_path, text, expected.replace(" ", "_") + ".toml")
            assert_raises(expected, lambda config=config: module.load_config(config))


def assert_capture_config_can_omit_deploy_artifact_window() -> None:
    module = load_script()
    capture_config_toml = CONFIG_TOML.replace(
        "artifact_retention_days = 3\nartifact_lookback_age_seconds = 259200\n",
        "",
        1,
    )
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config_path = write_config(tmp_path, capture_config_toml)
        assert_raises(
            "ci_provenance.deploy.artifact_retention_days must be a positive integer",
            lambda: module.load_config(config_path),
        )

        config = module.load_config(config_path, require_deploy_window=False)
        if config.deploy_artifact_retention_days is not None:
            raise AssertionError("capture config must not synthesize deploy artifact retention")
        if config.deploy_artifact_lookback_age_seconds is not None:
            raise AssertionError("capture config must not synthesize deploy artifact lookback")

        record = valid_record(module, config_path)
        fake = FakeGitHub(
            runs_pages=[[run_payload(created_at="2026-06-01T00:00:00Z")]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
        )
        resolved = module.resolve_exact_sha_evidence(
            repo="seungpyoson/bolt-v2",
            token="token",
            requested_sha=SHA,
            config=config,
            config_path=config_path,
            api_json=fake.json,
            api_bytes=fake.bytes,
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
        )
        if resolved.run.get("id") != RUN_ID:
            raise AssertionError(f"expected capture evidence run {RUN_ID}, got {resolved.run}")


def assert_optional_deploy_window_rejects_partial_config() -> None:
    module = load_script()
    partial_config_toml = CONFIG_TOML.replace(
        "artifact_retention_days = 3\nartifact_lookback_age_seconds = 259200\n",
        "artifact_retention_days = 3\n",
        1,
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), partial_config_toml)
        assert_raises(
            "ci_provenance.deploy artifact retention and lookback must be configured together",
            lambda: module.load_config(config, require_deploy_window=False),
        )


def assert_emit_full_ci_records_nextest_fingerprint_argument() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)

        def fake_api_json(repo: str, token: str, path: str, query: dict[str, str] | None = None) -> dict[str, object]:
            if path == f"actions/runs/{RUN_ID}":
                return run_payload()
            raise AssertionError((repo, token, path, query))

        with patched_env(
            {
                "GITHUB_REPOSITORY": "seungpyoson/bolt-v2",
                "GITHUB_TOKEN": "token",
                "GITHUB_RUN_ID": str(RUN_ID),
                "GITHUB_RUN_ATTEMPT": "1",
                "GITHUB_SHA": SHA,
                "GITHUB_EVENT_NAME": "push",
            }
        ):
            record = module.emit_full_ci_record(
                config=module.load_config(config),
                config_path=config,
                required_job_values=[
                    "detector=success",
                    "deny=success",
                    "clippy=success",
                    "check-aarch64=success",
                    "source-fence=success",
                    "nextest-fingerprint=success",
                    "test-archive=success",
                    "test=success",
                ],
                conditional_job_values=["build.required=true", "build.result=success"],
                nextest_fingerprint=NEXTEST_FINGERPRINT,
                api_json=fake_api_json,
            )
        if record["nextest_fingerprint"] != NEXTEST_FINGERPRINT:
            raise AssertionError(record)


def assert_emit_full_ci_hashes_explicit_tested_workflow() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        tested_workflow = tmp_path / "tested-ci.yml"
        tested_workflow.write_text("name: tested workflow\n", encoding="utf-8")
        expected_digest = hashlib.sha256(tested_workflow.read_bytes()).hexdigest()

        def fake_api_json(repo: str, token: str, path: str, query: dict[str, str] | None = None) -> dict[str, object]:
            if path == f"actions/runs/{RUN_ID}":
                return run_payload()
            raise AssertionError((repo, token, path, query))

        with patched_env(
            {
                "GITHUB_REPOSITORY": "seungpyoson/bolt-v2",
                "GITHUB_TOKEN": "token",
                "GITHUB_RUN_ID": str(RUN_ID),
                "GITHUB_RUN_ATTEMPT": "1",
                "GITHUB_SHA": SHA,
                "GITHUB_EVENT_NAME": "push",
            }
        ):
            record = module.emit_full_ci_record(
                config=module.load_config(config),
                config_path=config,
                workflow_file=tested_workflow,
                required_job_values=[
                    "detector=success",
                    "deny=success",
                    "clippy=success",
                    "check-aarch64=success",
                    "source-fence=success",
                    "nextest-fingerprint=success",
                    "test-archive=success",
                    "test=success",
                ],
                conditional_job_values=["build.required=true", "build.result=success"],
                nextest_fingerprint=NEXTEST_FINGERPRINT,
                api_json=fake_api_json,
            )
        if record["workflow_digest"] != expected_digest:
            raise AssertionError(record)


def assert_emit_docs_ci_record_requires_skipped_heavy_jobs() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)

        def fake_api_json(repo: str, token: str, path: str, query: dict[str, str] | None = None) -> dict[str, object]:
            if path == f"actions/runs/{RUN_ID}":
                return run_payload(event="pull_request", head_branch="feature", head_sha=SHA)
            raise AssertionError((repo, token, path, query))

        env = {
            "GITHUB_REPOSITORY": "seungpyoson/bolt-v2",
            "GITHUB_TOKEN": "token",
            "GITHUB_RUN_ID": str(RUN_ID),
            "GITHUB_RUN_ATTEMPT": "1",
            "GITHUB_SHA": OTHER_SHA,
            "GITHUB_EVENT_NAME": "pull_request",
            "PR_NUMBER": "960",
            "PR_BASE_SHA": "1" * 40,
        }
        required_values = [
            "detector=success",
            "deny=skipped",
            "clippy=skipped",
            "check-aarch64=skipped",
            "source-fence=skipped",
            "nextest-fingerprint=skipped",
            "test-archive=skipped",
            "test=skipped",
        ]
        with patched_env(env):
            record = module.emit_full_ci_record(
                config=module.load_config(config),
                config_path=config,
                ci_policy_path="docs",
                required_job_values=required_values,
                conditional_job_values=["build.required=false", "build.result=skipped"],
                nextest_fingerprint=None,
                api_json=fake_api_json,
            )
        if record["kind"] != "docs-ci":
            raise AssertionError(record)
        if record["required_jobs"]["clippy"] != "skipped":
            raise AssertionError(record)
        if record["pull_request"]["base_sha"] != "1" * 40:
            raise AssertionError(record)

        with patched_env(env):
            assert_raises(
                "docs required job clippy must be skipped",
                lambda: module.emit_full_ci_record(
                    config=module.load_config(config),
                    config_path=config,
                    ci_policy_path="docs",
                    required_job_values=[value.replace("clippy=skipped", "clippy=success") for value in required_values],
                    conditional_job_values=["build.required=false", "build.result=skipped"],
                    nextest_fingerprint=None,
                    api_json=fake_api_json,
                ),
            )


def assert_unknown_record_schema_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = tmp_path / "record.json"
        record.write_text(json.dumps({"schema_version": 999, "kind": "full-ci"}), encoding="utf-8")
        assert_fails(
            "unknown provenance schema",
            ["validate-record", "--config", str(config), "--record", str(record)],
        )


def resolve_fingerprint_with_fake(
    module,
    config_path: pathlib.Path,
    fake: FakeGitHub,
    *,
    current_fingerprint: str = NEXTEST_FINGERPRINT,
    current_run_id: int = RUN_ID + 999,
    now: str = "2026-06-13T00:30:00Z",
):
    return module.resolve_fingerprint_reuse(
        repo="seungpyoson/bolt-v2",
        token="token",
        current_fingerprint=current_fingerprint,
        current_run_id=current_run_id,
        config=module.load_config(config_path),
        config_path=config_path,
        api_json=fake.json,
        api_bytes=fake.bytes,
        now=module.parse_timestamp(now),
    )


def assert_fingerprint_reuse_prior_green_returns_reuse() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = record_with_fingerprint(module, config)
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={
                RUN_ID: {
                    "artifacts": [
                        fingerprint_artifact(id=11),
                        provenance_artifact(id=12),
                    ]
                }
            },
            records_by_artifact_id={12: record},
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not True:
            raise AssertionError(result)
        if result.source_run_id != str(RUN_ID):
            raise AssertionError(result)
        if result.source_sha != SHA:
            raise AssertionError(result)
        if result.source_artifact_id != "12":
            raise AssertionError(result)
        if "matched" not in result.reason:
            raise AssertionError(result)
        if not fake.queries or fake.queries[0][0] != "actions/workflows/ci.yml/runs":
            raise AssertionError(fake.queries)


def assert_fingerprint_reuse_no_prior_run_returns_no_reuse() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = resolve_fingerprint_with_fake(module, config, FakeGitHub(runs_pages=[[]]))
        if result.reuse_found is not False:
            raise AssertionError(result)
        if result.source_run_id or result.source_sha or result.source_artifact_id:
            raise AssertionError(result)
        if "no prior successful" not in result.reason:
            raise AssertionError(result)


def assert_fingerprint_reuse_rejects_failed_cancelled_and_wrong_workflow_runs() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        cases = [
            ("failed", run_payload(conclusion="failure")),
            ("cancelled", run_payload(conclusion="cancelled")),
            ("in-progress", run_payload(status="in_progress", conclusion=None)),
            ("wrong workflow", run_payload(path=".github/workflows/backtester-ci.yml")),
        ]
        for label, run in cases:
            result = resolve_fingerprint_with_fake(module, config, FakeGitHub(runs_pages=[[run]]))
            if result.reuse_found is not False:
                raise AssertionError((label, result))


def assert_fingerprint_reuse_rejects_ambiguous_and_expired_artifacts() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = record_with_fingerprint(module, config)
        cases = [
            (
                "ambiguous fingerprint",
                {
                    "artifacts": [
                        fingerprint_artifact(id=1),
                        fingerprint_artifact(id=2),
                        provenance_artifact(id=3),
                    ]
                },
                {3: record},
            ),
            (
                "expired fingerprint",
                {"artifacts": [fingerprint_artifact(expired=True), provenance_artifact(id=4)]},
                {4: record},
            ),
            (
                "expired provenance",
                {"artifacts": [fingerprint_artifact(id=5), provenance_artifact(id=6, expired=True)]},
                {6: record},
            ),
        ]
        for label, artifacts, records in cases:
            fake = FakeGitHub(
                runs_pages=[[run_payload()]],
                artifacts_by_run_id={RUN_ID: artifacts},
                records_by_artifact_id=records,
            )
            result = resolve_fingerprint_with_fake(module, config, fake)
            if result.reuse_found is not False:
                raise AssertionError((label, result))
            if label.split()[0] not in result.reason:
                raise AssertionError((label, result))


def assert_fingerprint_reuse_requires_exact_fingerprint_components() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        variants = [
            NEXTEST_FINGERPRINT.replace("Linux", "macOS"),
            NEXTEST_FINGERPRINT.replace("X64", "ARM64"),
            NEXTEST_FINGERPRINT.replace("test-profile", "default-profile"),
            NEXTEST_FINGERPRINT.replace("shards-4", "shards-8"),
            NEXTEST_FINGERPRINT.replace("v2", "v3", 1),
        ]
        for fingerprint in variants:
            record = record_with_fingerprint(module, config, fingerprint=fingerprint)
            artifact_name = fingerprint.replace("nextest-archive-", "nextest-archive-fingerprint-", 1)
            fake = FakeGitHub(
                runs_pages=[[run_payload()]],
                artifacts_by_run_id={
                    RUN_ID: {
                        "artifacts": [
                            fingerprint_artifact(id=1, name=artifact_name),
                            provenance_artifact(id=2),
                        ]
                    }
                },
                records_by_artifact_id={2: record},
            )
            result = resolve_fingerprint_with_fake(module, config, fake)
            if result.reuse_found is not False:
                raise AssertionError((fingerprint, result))


def assert_fingerprint_reuse_rejects_source_record_workflow_digest_mismatch() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        source_workflow_bytes = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()
        record = record_with_fingerprint(module, config)
        record["workflow_digest"] = "0" * 64
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}
            },
            records_by_artifact_id={2: record},
            workflow_bytes=source_workflow_bytes,
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not False:
            raise AssertionError(result)
        if "workflow_digest" not in result.reason:
            raise AssertionError(result)


def assert_fingerprint_reuse_allows_unrelated_workflow_drift() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        source_workflow_bytes = (
            (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()
            + b"\n# governance-only comment outside nextest archive reuse scope\n"
        )
        record = record_with_fingerprint(module, config)
        record["workflow_digest"] = hashlib.sha256(source_workflow_bytes).hexdigest()
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}
            },
            records_by_artifact_id={2: record},
            workflow_bytes=source_workflow_bytes,
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not True:
            raise AssertionError(result)


def assert_fingerprint_reuse_allows_deploy_only_env_drift() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        source_workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        source_workflow_text = source_workflow_text.replace(
            'S3_DEPLOY_PATH: "s3://bolt-deploy-artifacts/artifacts/bolt-v2"',
            'S3_DEPLOY_PATH: "s3://bolt-deploy-artifacts/artifacts/bolt-v2-previous"',
            1,
        )
        source_workflow_bytes = source_workflow_text.encode("utf-8")
        record = record_with_fingerprint(module, config)
        record["workflow_digest"] = hashlib.sha256(source_workflow_bytes).hexdigest()
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}
            },
            records_by_artifact_id={2: record},
            workflow_bytes=source_workflow_bytes,
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not True:
            raise AssertionError(result)


def assert_workflow_reuse_scope_digest_accepts_yaml_header_formatting() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        formatted_text = workflow_text.replace("env:\n", "env: # top-level env\n", 1)
        formatted_text = formatted_text.replace(
            '  JUST_VERSION: "1.49.0"',
            '  "JUST_VERSION": "1.49.0" # tool version',
            1,
        )
        formatted_text = formatted_text.replace("jobs:\n", "jobs: # workflow jobs\n", 1)
        for job_name in module.REUSE_RELEVANT_WORKFLOW_JOBS:
            formatted_text = formatted_text.replace(
                f"  {job_name}:",
                f'  "{job_name}": # reuse-relevant job',
                1,
            )

        expected = module.workflow_reuse_scope_digest_from_bytes(
            config,
            workflow_text.encode("utf-8"),
        )
        actual = module.workflow_reuse_scope_digest_from_bytes(
            config,
            formatted_text.encode("utf-8"),
        )
        if actual != expected:
            raise AssertionError((expected, actual))


def assert_workflow_reuse_scope_digest_distinguishes_quoted_hash_values() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        source_text = workflow_text.replace(
            '  JUST_VERSION: "1.49.0"',
            '  JUST_VERSION: "1.49.0#source"',
            1,
        )
        current_text = workflow_text.replace(
            '  JUST_VERSION: "1.49.0"',
            '  JUST_VERSION: "1.49.0#current"',
            1,
        )
        source_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            source_text.encode("utf-8"),
        )
        current_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            current_text.encode("utf-8"),
        )
        if source_digest == current_digest:
            raise AssertionError("quoted # content must remain part of the reuse-scope digest")


def assert_workflow_reuse_scope_digest_rejects_multiline_scoped_env() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        multiline_text = workflow_text.replace(
            '  JUST_VERSION: "1.49.0"',
            '  JUST_VERSION:\n    "1.49.0"',
            1,
        )
        try:
            module.workflow_reuse_scope_digest_from_bytes(
                config,
                multiline_text.encode("utf-8"),
            )
        except module.ProvenanceError as exc:
            if "env.JUST_VERSION" not in str(exc) or "same-line scalar" not in str(exc):
                raise AssertionError(exc)
        else:
            raise AssertionError("multiline scoped env value must fail closed")


def assert_workflow_reuse_scope_digest_rejects_folded_scoped_env() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        returned_digests = []
        for indicator in (">-", ">2", "|2"):
            for value in ("1.49.0", "9.9.9"):
                folded_text = replace_once(
                    workflow_text,
                    '  JUST_VERSION: "1.49.0"',
                    f"  JUST_VERSION: {indicator}\n    {value}",
                )
                try:
                    returned_digests.append(
                        module.workflow_reuse_scope_digest_from_bytes(
                            config,
                            folded_text.encode("utf-8"),
                        )
                    )
                except module.ProvenanceError as exc:
                    message = str(exc)
                    if "env.JUST_VERSION" not in message or "single-line scalar" not in message:
                        raise AssertionError(message) from exc

        if len(returned_digests) == 2 and returned_digests[0] == returned_digests[1]:
            raise AssertionError(
                "different folded JUST_VERSION values produced identical reuse-scope digest"
            )
        if returned_digests:
            raise AssertionError(f"folded JUST_VERSION values must be rejected: {returned_digests}")


def assert_workflow_reuse_scope_digest_rejects_alias_scoped_env() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        for alias_value in ("&just_version \"1.49.0\"", "*just_version"):
            alias_text = replace_once(
                workflow_text,
                '  JUST_VERSION: "1.49.0"',
                f"  JUST_VERSION: {alias_value}",
            )
            try:
                module.workflow_reuse_scope_digest_from_bytes(
                    config,
                    alias_text.encode("utf-8"),
                )
            except module.ProvenanceError as exc:
                message = str(exc)
                if "env.JUST_VERSION" not in message or "YAML anchors or aliases" not in message:
                    raise AssertionError(message) from exc
            else:
                raise AssertionError(f"alias scoped env value must fail closed: {alias_value}")


def assert_workflow_reuse_scope_digest_ignores_nested_env_decoys() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        decoy_text = replace_once(workflow_text, '  JUST_VERSION: "1.49.0"\n', "")
        decoy_text = replace_once(
            decoy_text,
            "  CARGO_TERM_COLOR: always\n",
            '  CARGO_TERM_COLOR: |\n    JUST_VERSION: "1.49.0"\n',
        )
        try:
            module.workflow_reuse_scope_digest_from_bytes(
                config,
                decoy_text.encode("utf-8"),
            )
        except module.ProvenanceError as exc:
            if "missing env.JUST_VERSION" not in str(exc):
                raise AssertionError(exc)
        else:
            raise AssertionError("nested env decoy must not satisfy top-level JUST_VERSION")


def assert_workflow_reuse_scope_digest_preserves_block_scalar_content() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        source_text = workflow_text.replace(
            "          python3 scripts/nextest_fingerprint.py",
            "          # source block-scalar content\n          python3 scripts/nextest_fingerprint.py",
            1,
        )
        current_text = workflow_text.replace(
            "          python3 scripts/nextest_fingerprint.py",
            "          # current block-scalar content\n          python3 scripts/nextest_fingerprint.py",
            1,
        )
        source_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            source_text.encode("utf-8"),
        )
        current_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            current_text.encode("utf-8"),
        )
        if source_digest == current_digest:
            raise AssertionError("block scalar comment content must remain part of the reuse-scope digest")


def assert_workflow_reuse_scope_digest_preserves_indicated_block_scalar_content() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        source_text = workflow_text.replace(
            "        run: |\n          python3 scripts/nextest_fingerprint.py",
            "        run: |2\n          # source indicated block-scalar content\n          python3 scripts/nextest_fingerprint.py",
            1,
        )
        current_text = workflow_text.replace(
            "        run: |\n          python3 scripts/nextest_fingerprint.py",
            "        run: |2\n          # current indicated block-scalar content\n          python3 scripts/nextest_fingerprint.py",
            1,
        )
        source_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            source_text.encode("utf-8"),
        )
        current_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            current_text.encode("utf-8"),
        )
        if source_digest == current_digest:
            raise AssertionError(
                "indicated block scalar comment content must remain part of the reuse-scope digest"
            )


def assert_workflow_reuse_scope_digest_preserves_block_scalar_trailing_spaces() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        line_without_trailing_spaces = (
            '          if [[ "${{ steps.sccache-eligible.outputs.eligible }}" == "true" \\\n'
        )
        line_with_trailing_spaces = (
            '          if [[ "${{ steps.sccache-eligible.outputs.eligible }}" == "true" \\   \n'
        )
        current_text = replace_once(
            workflow_text,
            line_without_trailing_spaces,
            line_with_trailing_spaces,
        )
        source_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            workflow_text.encode("utf-8"),
        )
        current_digest = module.workflow_reuse_scope_digest_from_bytes(
            config,
            current_text.encode("utf-8"),
        )
        if source_digest == current_digest:
            raise AssertionError(
                "block scalar trailing spaces must remain part of the reuse-scope digest"
            )


def assert_fingerprint_reuse_rejects_reuse_relevant_workflow_drift() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        workflow_text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        display_names = {
            "nextest-fingerprint": "nextest fingerprint",
            "test-archive": "nextest archive",
            "test": "test",
            "build": "build",
        }
        for job_name in module.REUSE_RELEVANT_WORKFLOW_JOBS:
            display_name = display_names[job_name]
            source_workflow_text = replace_once(
                workflow_text,
                f"    name: {display_name}",
                f"    name: {display_name} drift",
            )
            source_workflow_bytes = source_workflow_text.encode("utf-8")
            record = record_with_fingerprint(module, config)
            record["workflow_digest"] = hashlib.sha256(source_workflow_bytes).hexdigest()
            fake = FakeGitHub(
                runs_pages=[[run_payload()]],
                artifacts_by_run_id={
                    RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}
                },
                records_by_artifact_id={2: record},
                workflow_bytes=source_workflow_bytes,
            )
            result = resolve_fingerprint_with_fake(module, config, fake)
            if result.reuse_found is not False:
                raise AssertionError((job_name, result))
            if "workflow reuse scope" not in result.reason:
                raise AssertionError((job_name, result))


def assert_fingerprint_reuse_malformed_fingerprint_fails_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        result = resolve_fingerprint_with_fake(
            module,
            config,
            FakeGitHub(runs_pages=[[run_payload()]]),
            current_fingerprint="not-a-nextest-fingerprint",
        )
        if result.reuse_found is not False or "malformed current fingerprint" not in result.reason:
            raise AssertionError(result)

        record = record_with_fingerprint(module, config, fingerprint="not-a-nextest-fingerprint")
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}},
            records_by_artifact_id={2: record},
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not False:
            raise AssertionError(result)


def assert_fingerprint_reuse_rejects_failed_source_archive_through_resolver() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = record_with_fingerprint(module, config)
        failed_jobs = required_job_payloads()
        for job in failed_jobs:
            if job["name"] == "nextest archive":
                job["conclusion"] = "failure"
                break
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            jobs_by_run_id={RUN_ID: {"jobs": failed_jobs}},
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [fingerprint_artifact(id=1), provenance_artifact(id=2)]}
            },
            records_by_artifact_id={2: record},
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.reuse_found is not False:
            raise AssertionError(result)
        if "nextest archive" not in result.reason:
            raise AssertionError(result)


def assert_fingerprint_reuse_source_run_must_be_trusted_main_push() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp)))
        pr_run = run_payload()
        pr_run["event"] = "pull_request"
        pr_run["head_branch"] = "attacker/fingerprint-reuse"
        if module.run_matches_fingerprint_reuse(pr_run, config, current_run_id=None):
            raise AssertionError("fingerprint reuse must not source evidence from pull_request runs")
        branch_run = run_payload()
        branch_run["event"] = config.deploy_source_event
        branch_run["head_branch"] = "feature/not-main"
        if module.run_matches_fingerprint_reuse(branch_run, config, current_run_id=None):
            raise AssertionError("fingerprint reuse must not source evidence from non-main branch runs")


def assert_missing_current_fingerprint_arg_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        code, stdout, stderr = run_cli(
            [
                "resolve-fingerprint",
                "--config",
                str(config),
                "--repo",
                "seungpyoson/bolt-v2",
                "--token",
                "token",
                "--current-run-id",
                str(RUN_ID),
            ]
        )
        if code != 0:
            raise AssertionError((code, stdout, stderr))
        if stderr:
            raise AssertionError(stderr)
        expected = {
            "reuse_found=false",
            "source_run_id=",
            "source_sha=",
            "source_artifact_id=",
            "reason=missing current fingerprint",
        }
        if set(stdout.splitlines()) != expected:
            raise AssertionError(stdout)


def assert_nextest_fingerprint_path_args_are_rejected() -> None:
    assert_fails("unrecognized arguments", ["emit-full-ci", "--nextest-fingerprint-path", "cache-key.txt"])
    assert_fails("unrecognized arguments", ["resolve-fingerprint", "--current-fingerprint-path", "cache-key.txt"])


def assert_fingerprint_reuse_api_errors_fail_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))

        def failing_api_json(repo, token, path, query):  # noqa: ARG001 - test fake matches API shape.
            raise module.ProvenanceError("GitHub API request failed for actions/workflows/ci.yml/runs")

        result = module.resolve_fingerprint_reuse(
            repo="seungpyoson/bolt-v2",
            token="token",
            current_fingerprint=NEXTEST_FINGERPRINT,
            current_run_id=RUN_ID + 999,
            config=module.load_config(config),
            config_path=config,
            api_json=failing_api_json,
            api_bytes=lambda _repo, _token, _url: b"",
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
        )
        if result.reuse_found is not False:
            raise AssertionError(result)
        if "fingerprint reuse lookup failed" not in result.reason:
            raise AssertionError(result)


def assert_fingerprint_reuse_selects_newest_valid_prior_green() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        older = record_with_fingerprint(module, config, run_id=RUN_ID)
        newer = record_with_fingerprint(
            module,
            config,
            run_id=RUN_ID + 1,
            head_sha=OTHER_SHA,
            tested_sha=OTHER_SHA,
        )
        fake = FakeGitHub(
            runs_pages=[
                [
                    run_payload(id=RUN_ID + 1, head_sha=OTHER_SHA, updated_at="2026-06-13T00:20:00Z"),
                    run_payload(id=RUN_ID, updated_at="2026-06-13T00:10:00Z"),
                ]
            ],
            artifacts_by_run_id={
                RUN_ID + 1: {
                    "artifacts": [
                        fingerprint_artifact(
                            id=21,
                            run_id=RUN_ID + 1,
                            workflow_run={"id": RUN_ID + 1, "head_branch": "main", "head_sha": OTHER_SHA},
                        ),
                        provenance_artifact(
                            id=22,
                            run_id=RUN_ID + 1,
                            workflow_run={"id": RUN_ID + 1, "head_branch": "main", "head_sha": OTHER_SHA},
                        ),
                    ]
                },
                RUN_ID: {"artifacts": [fingerprint_artifact(id=11), provenance_artifact(id=12)]},
            },
            records_by_artifact_id={12: older, 22: newer},
        )
        result = resolve_fingerprint_with_fake(module, config, fake)
        if result.source_run_id != str(RUN_ID + 1) or result.source_sha != OTHER_SHA:
            raise AssertionError(result)


def assert_top_level_help_is_supported() -> None:
    code, stdout, stderr = run_cli(["--help"])
    if code != 2:
        raise AssertionError(f"expected help to exit 2, got {code}")
    combined = stdout + stderr
    if "Usage: ci_provenance.py <mode> [options]" not in combined:
        raise AssertionError(f"expected top-level usage output, got {combined!r}")
    if "resolve-exact-sha" not in combined:
        raise AssertionError(f"expected supported modes in help output, got {combined!r}")
    if "artifact-metadata" not in combined:
        raise AssertionError(f"expected artifact metadata mode in help output, got {combined!r}")


def assert_artifact_metadata_outputs_configured_name_only() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        code, stdout, stderr = run_cli(
            [
                "artifact-metadata",
                "--config",
                str(config),
                "--run-attempt",
                "7",
            ]
        )
    if code != 0:
        raise AssertionError(f"artifact-metadata failed with {code}: stdout={stdout!r} stderr={stderr!r}")
    lines = stdout.strip().splitlines()
    if "artifact_name=ci-provenance-attempt-7" not in lines:
        raise AssertionError(f"artifact metadata must derive artifact name from config, got {stdout!r}")
    if any(line.startswith("retention_days=") for line in lines):
        raise AssertionError(f"artifact metadata must not emit upload retention policy, got {stdout!r}")


def assert_artifact_metadata_accepts_capture_config_without_workflows() -> None:
    module = load_script()
    capture_config = REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml"
    text = capture_config.read_text(encoding="utf-8")
    if "[workflows" in text:
        raise AssertionError("capture provenance config must not require workflow registry data")
    module.load_config(
        capture_config, require_workflows=False, require_deploy_window=False
    )
    digest = module.provenance_config_digest(capture_config)
    if digest != CAPTURE_PROVENANCE_CONFIG_DIGEST:
        raise AssertionError(f"capture provenance config digest changed: {digest}")

    code, stdout, stderr = run_cli(
        [
            "artifact-metadata",
            "--config",
            str(capture_config),
            "--run-attempt",
            "3",
        ]
    )

    if code != 0:
        raise AssertionError(
            f"capture artifact-metadata failed with {code}: stdout={stdout!r} stderr={stderr!r}"
        )
    lines = stdout.strip().splitlines()
    if "artifact_name=chainlink-reference-fixture-capture-attempt-3" not in lines:
        raise AssertionError(f"capture artifact metadata derived wrong artifact name: {stdout!r}")
    if any(line.startswith("retention_days=") for line in lines):
        raise AssertionError(f"capture artifact metadata must not emit upload retention policy: {stdout!r}")


def assert_ci_policy_rejects_event_sender_cli_override_arguments() -> None:
    base_args = [
        "ci-policy",
        "--event-name",
        "pull_request",
        "--event-action",
        "opened",
        "--pull-request-draft",
        "true",
        "--pull-request-head-ref",
        "mergify/merge-queue/83d4b0be7e",
        "--pull-request-base-changed",
        "false",
        "--workflow-dispatch-full-ci",
        "",
        "--docs-only",
        "false",
        "--ref",
        "refs/pull/965/merge",
    ]
    for flag, value in (
        ("--event-sender-id", "37929162"),
        ("--event-sender", "37929162"),
        ("--event-nam", "pull_request"),
    ):
        code, stdout, stderr = run_cli([*base_args, flag, value])
        combined = stdout + stderr
        if code == 0:
            raise AssertionError(f"{flag} must be rejected, got stdout={stdout!r}")
        if "unrecognized arguments" not in combined or flag not in combined:
            raise AssertionError(f"{flag} must fail as unrecognized, got {combined!r}")


def assert_ci_policy_outputs_matrix() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        expected_event_classes = {
            "draft_pr_synchronize": "iteration",
            "draft_pr_opened": "iteration",
            "draft_pr_reopened": "iteration",
            "draft_pr_edited": "iteration",
            "converted_to_draft": "iteration",
            "ready_pr": "iteration",
            "ready_pr_edited_no_base": "iteration",
            "ready_pr_reopened": "iteration",
            "ready_for_review": "iteration",
            "docs": "docs",
            "workflow_dispatch": "iteration",
            "workflow_dispatch_full_ci": "full",
            "main_push": "full",
            "merge_group": "full",
            "mergify_temp_pr": "full",
            "tag": "tag_reuse",
            "unknown_event": "full",
        }
        gate_names = {
            "full": ("gate", "backtester-gate"),
            "tag_reuse": ("gate", "backtester-gate"),
            "docs": ("gate", "backtester-gate"),
            "defer": ("gate", "backtester-gate"),
            "iteration": ("gate-iteration", "backtester-gate-iteration"),
            "noop": ("gate", "backtester-gate"),
            "workflow_dispatch_full_ci": ("gate-dispatch", "backtester-gate-dispatch"),
        }
        # Queue-only rework (#981): every ordinary pull_request action defers heavy
        # lanes (ci_policy_path == "iteration", full_ci_required false) and publishes
        # only the non-required gate-iteration; the merge-boundary mergify temp PR is
        # the SOLE producer of the required "gate". docs-only no longer downgrades a
        # PR to the required docs path because no ordinary PR path is "full" anymore,
        # so the docs override (which only fires on path == "full") never engages.
        cases = [
            ("push", "", "false", "false", "", "refs/heads/main", "false", "full", "main_push"),
            ("push", "", "false", "false", "true", "refs/heads/main", "false", "full", "main_push"),
            ("push", "", "false", "false", "", "refs/tags/v1.2.3", "false", "tag_reuse", "tag"),
            ("pull_request", "opened", "true", "false", "", "refs/pull/1/merge", "false", "iteration", "draft_pr_opened"),
            ("pull_request", "synchronize", "true", "false", "", "refs/pull/1/merge", "false", "iteration", "draft_pr_synchronize"),
            ("pull_request", "reopened", "true", "false", "", "refs/pull/1/merge", "false", "iteration", "draft_pr_reopened"),
            ("pull_request", "edited", "true", "false", "", "refs/pull/1/merge", "false", "iteration", "draft_pr_edited"),
            ("pull_request", "converted_to_draft", "true", "false", "", "refs/pull/1/merge", "false", "iteration", "converted_to_draft"),
            ("pull_request", "opened", "false", "false", "", "refs/pull/1/merge", "false", "iteration", "ready_pr"),
            ("pull_request", "opened", "false", "false", "", "refs/pull/1/merge", "true", "iteration", "ready_pr"),
            ("pull_request", "edited", "false", "false", "", "refs/pull/1/merge", "true", "iteration", "ready_pr_edited_no_base"),
            ("pull_request", "edited", "false", "true", "", "refs/pull/1/merge", "true", "iteration", "ready_pr"),
            ("pull_request", "reopened", "false", "false", "", "refs/pull/1/merge", "false", "iteration", "ready_pr_reopened"),
            ("pull_request", "ready_for_review", "false", "false", "", "refs/pull/1/merge", "true", "iteration", "ready_for_review"),
            ("workflow_dispatch", "", "true", "false", "true", "refs/heads/codex/branch", "true", "full", "workflow_dispatch_full_ci"),
            ("workflow_dispatch", "", "true", "false", "false", "refs/heads/codex/branch", "true", "iteration", "workflow_dispatch"),
            ("workflow_dispatch", "", "true", "false", "", "refs/heads/codex/branch", "true", "iteration", "workflow_dispatch"),
            ("workflow_dispatch", "", "true", "false", "TRUE", "refs/heads/codex/branch", "false", "iteration", "workflow_dispatch"),
            ("workflow_dispatch", "", "true", "false", " true ", "refs/heads/codex/branch", "false", "iteration", "workflow_dispatch"),
            ("workflow_dispatch", "", "true", "false", "1", "refs/heads/codex/branch", "false", "iteration", "workflow_dispatch"),
            ("workflow_dispatch", "", "true", "false", "yes", "refs/heads/codex/branch", "false", "iteration", "workflow_dispatch"),
            ("merge_group", "checks_requested", "false", "false", "", "refs/heads/gh-readonly-queue/main/pr-1-deadbeef", "true", "full", "merge_group"),
            ("unknown_event", "", "true", "false", "", "refs/heads/codex/branch", "true", "full", "unknown_event"),
        ]
        # Queue-only rework (#981): the policy table pins every row to a non-"defer" value,
        # so no matrix row can exercise full_ci_deferred's True branch. Assert that invariant
        # explicitly; the deferred branch is exercised by dedicated VALIDATOR tests that feed
        # policy_path="defer" directly (verify_ci_provenance / backtester), not via this table.
        if any(expected == "defer" for *_, expected, _reason in cases):
            raise AssertionError("policy matrix must not expect 'defer' — defer is unreachable via the table")
        for event_name, action, draft, base_changed, workflow_dispatch_full_ci, ref, docs_only, expected, reason in cases:
            code, stdout, stderr = run_cli(
                [
                    "ci-policy",
                    "--config",
                    str(config),
                    "--event-name",
                    event_name,
                    "--event-action",
                    action,
                    "--pull-request-draft",
                    draft,
                    "--pull-request-head-ref",
                    "",
                    "--pull-request-base-changed",
                    base_changed,
                    "--workflow-dispatch-full-ci",
                    workflow_dispatch_full_ci,
                    "--docs-only",
                    docs_only,
                    "--ref",
                    ref,
                ]
            )
            if code != 0:
                raise AssertionError(f"ci-policy failed for {event_name}/{action}: {stderr}")
            output = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
            if output.get("ci_policy_path") != expected:
                raise AssertionError((event_name, action, draft, ref, expected, output))
            if output.get("full_ci_required") != str(expected == "full").lower():
                raise AssertionError(f"full_ci_required must derive from {expected}: {output}")
            if output.get("full_ci_deferred") != "false":
                raise AssertionError(f"every policy-table row must resolve full_ci_deferred false: {output}")
            if output.get("reason") != reason:
                raise AssertionError(f"ci-policy must expose reason {reason}: {output}")
            if output.get("expected_event_class") != expected_event_classes[reason]:
                raise AssertionError(f"ci-policy must expose expected_event_class for {reason}: {output}")
            name_key = reason if reason == "workflow_dispatch_full_ci" else expected
            expected_gate, expected_backtester_gate = gate_names[name_key]
            if output.get("gate_name") != expected_gate:
                raise AssertionError(f"ci-policy must expose gate_name {expected_gate}: {output}")
            if output.get("backtester_gate_name") != expected_backtester_gate:
                raise AssertionError(
                    f"ci-policy must expose backtester_gate_name {expected_backtester_gate}: {output}"
                )
            if output.get("ignore_emit_failure") != "false":
                raise AssertionError(f"ci-policy must expose ignore_emit_failure: {output}")

        code, stdout, stderr = run_cli_with_event_sender(
            [
                "ci-policy",
                "--config",
                str(config),
                "--event-name",
                "pull_request",
                "--event-action",
                "opened",
                "--pull-request-draft",
                "true",
                "--pull-request-head-ref",
                "mergify/merge-queue/83d4b0be7e",
                "--pull-request-base-changed",
                "false",
                "--workflow-dispatch-full-ci",
                "",
                "--docs-only",
                "false",
                "--ref",
                "refs/pull/965/merge",
            ],
            "37929162",
        )
        if code != 0:
            raise AssertionError(f"Mergify ci-policy failed: {stderr}")
        output = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if (
            output.get("ci_policy_path") != "full"
            or output.get("gate_name") != "gate"
            or output.get("backtester_gate_name") != "backtester-gate"
            or output.get("expected_event_class") != "full"
            or output.get("reason") != "mergify_temp_pr"
        ):
            raise AssertionError(f"Mergify temp PR must resolve to required full CI: {output}")

        code, stdout, stderr = run_cli_with_event_sender(
            [
                "ci-policy",
                "--config",
                str(config),
                "--event-name",
                "pull_request",
                "--event-action",
                "synchronize",
                "--pull-request-draft",
                "true",
                "--pull-request-head-ref",
                "mergify/merge-queue/83d4b0be7e",
                "--pull-request-base-changed",
                "false",
                "--workflow-dispatch-full-ci",
                "",
                "--docs-only",
                "true",
                "--ref",
                "refs/pull/965/merge",
            ],
            "37929162",
        )
        if code != 0:
            raise AssertionError(f"Mergify docs-only ci-policy failed: {stderr}")
        output = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if output.get("ci_policy_path") != "full" or output.get("reason") != "mergify_temp_pr":
            raise AssertionError(f"Mergify temp PR docs-only hint must not bypass full CI: {output}")

        code, stdout, stderr = run_cli(
            [
                "ci-policy",
                "--config",
                str(config),
                "--event-name",
                "pull_request",
                "--event-action",
                "ready_for_review",
                "--pull-request-draft",
                "true",
                "--pull-request-head-ref",
                "",
                "--pull-request-base-changed",
                "false",
                "--workflow-dispatch-full-ci",
                "",
                "--docs-only",
                "true",
                "--ref",
                "refs/pull/1/merge",
            ]
        )
        if code == 0 or "ready_for_review cannot be on a draft PR" not in stderr:
            raise AssertionError(f"ready_for_review draft event must fail closed, got {code=} {stdout=} {stderr=}")

        force_config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("force_full_ci = false", "force_full_ci = true"),
            "force.toml",
        )
        code, stdout, stderr = run_cli(
            [
                "ci-policy",
                "--config",
                str(force_config),
                "--event-name",
                "pull_request",
                "--event-action",
                "synchronize",
                "--pull-request-draft",
                "true",
                "--pull-request-head-ref",
                "",
                "--pull-request-base-changed",
                "false",
                "--workflow-dispatch-full-ci",
                "",
                "--docs-only",
                "true",
                "--ref",
                "refs/pull/1/merge",
            ]
        )
        if code != 0:
            raise AssertionError(f"force_full_ci ci-policy failed: {stderr}")
        output = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if output.get("ci_policy_path") != "full":
            raise AssertionError(f"force_full_ci must force draft PR events to full, got {output}")
        # Queue-only rework (#981): force_full_ci keeps ci_policy_path == "full" (heavy
        # lanes still run) but the gate name is now a pure function of the event, so a
        # pull_request — whose head run is never proof of the squash-merged commit —
        # publishes the NON-required gate-iteration, not the required "gate".
        if output.get("gate_name") != "gate-iteration" or output.get("backtester_gate_name") != "backtester-gate-iteration":
            raise AssertionError(f"force_full_ci on a pull_request must demote to non-required gate names, got {output}")
        if output.get("expected_event_class") != "full":
            raise AssertionError(f"force_full_ci must publish full event class, got {output}")


def assert_ci_policy_gate_names_are_event_based() -> None:
    # Queue-only rework (#981): the gate name is a pure function of (event_name,
    # reason) and never the policy VALUE. The required "gate" is reachable only by
    # merge_group, push/main_push, tag, and the actor-verified mergify temp PR.
    # Every ordinary pull_request — regardless of action/draft/base-change — earns
    # only the non-required gate-iteration, so a PR-head run cannot satisfy the
    # required gate that the merge queue actually merges on.
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
    actor_id = config.mergify_temp_pr_actor_id
    gate_required = config.gate_names["gate_required"]
    gate_iteration = config.gate_names["gate_iteration"]
    backtester_required = config.gate_names["backtester_required"]
    backtester_iteration = config.gate_names["backtester_iteration"]

    # Non-PR events that genuinely validate the to-be-merged commit keep the
    # required gate name, plus the SOLE pull_request that earns it: the actor-bound
    # mergify merge-queue temp PR (draft head ref + sender == bound mergify actor).
    required_cases = [
        ("push", "", False, False, "", "refs/heads/main", -1, ""),
        ("push", "", False, False, "", "refs/tags/v1.2.3", -1, ""),
        ("merge_group", "checks_requested", False, False, "", "refs/heads/gh-readonly-queue/main/pr-1-deadbeef", -1, ""),
        ("unknown_event", "", False, False, "", "refs/heads/codex/branch", -1, ""),
    ]
    for action in ("opened", "synchronize", "reopened"):
        required_cases.append(
            ("pull_request", action, True, False, "", "refs/pull/965/merge", actor_id, "mergify/merge-queue/abc123")
        )
    saw_mergify_required = False
    for event_name, action, draft, base_changed, wfd, ref, sender, head_ref in required_cases:
        result = module.evaluate_ci_policy(
            config,
            event_name=event_name,
            event_action=action,
            pull_request_draft=draft,
            pull_request_head_ref=head_ref,
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci=wfd,
            event_sender_id=sender,
            ref=ref,
        )
        if (result.gate_name, result.backtester_gate_name) != (gate_required, backtester_required):
            raise AssertionError(f"{event_name}/{action} must publish the required gate names: {result}")
        if event_name == "pull_request" and result.reason == "mergify_temp_pr":
            saw_mergify_required = True
    if not saw_mergify_required:
        raise AssertionError("matrix must exercise the actor-bound mergify temp PR required gate")

    # workflow_dispatch publishes feedback-only names (dispatch_full vs iteration).
    saw_dispatch_full = saw_dispatch_iteration = False
    for full_ci in ("", "false", "true", "TRUE", " true "):
        result = module.evaluate_ci_policy(
            config,
            event_name="workflow_dispatch",
            event_action="",
            pull_request_draft=False,
            pull_request_head_ref="",
            pull_request_base_changed=False,
            workflow_dispatch_full_ci=full_ci,
            event_sender_id=-1,
            ref="refs/heads/codex/branch",
        )
        if full_ci == "true":
            saw_dispatch_full = True
            expected_names = ("gate-dispatch", "backtester-gate-dispatch")
        else:
            saw_dispatch_iteration = True
            expected_names = (gate_iteration, backtester_iteration)
        if (result.gate_name, result.backtester_gate_name) != expected_names:
            raise AssertionError(f"workflow_dispatch must publish feedback-only names: {full_ci!r} {result}")

    # Every ordinary pull_request (sender is NOT the mergify actor) publishes ONLY
    # the non-required gate-iteration regardless of action/draft/base-change.
    saw_pr_iteration = False
    for action in ("opened", "synchronize", "reopened", "ready_for_review", "converted_to_draft", "edited", "labeled"):
        for draft in (False, True):
            for base_changed in (False, True):
                if action == "ready_for_review" and draft:
                    assert_raises(
                        "ready_for_review cannot be on a draft PR",
                        lambda action=action, draft=draft, base_changed=base_changed: module.evaluate_ci_policy(
                            config,
                            event_name="pull_request",
                            event_action=action,
                            pull_request_draft=draft,
                            pull_request_head_ref="",
                            pull_request_base_changed=base_changed,
                            workflow_dispatch_full_ci="",
                            event_sender_id=4242,
                            ref="refs/pull/1/merge",
                        ),
                    )
                    continue
                result = module.evaluate_ci_policy(
                    config,
                    event_name="pull_request",
                    event_action=action,
                    pull_request_draft=draft,
                    pull_request_head_ref="",
                    pull_request_base_changed=base_changed,
                    workflow_dispatch_full_ci="",
                    event_sender_id=4242,
                    ref="refs/pull/1/merge",
                )
                case_label = ("pull_request", action, draft, base_changed)
                if result.gate_name == gate_required or result.backtester_gate_name == backtester_required:
                    raise AssertionError(f"ordinary pull_request must never publish the required gate: {case_label} {result}")
                if (result.gate_name, result.backtester_gate_name) != (gate_iteration, backtester_iteration):
                    raise AssertionError(f"ordinary pull_request must publish gate-iteration: {case_label} {result}")
                saw_pr_iteration = True
    if not (saw_dispatch_full and saw_dispatch_iteration and saw_pr_iteration):
        raise AssertionError("event-based naming matrix must exercise dispatch full/iteration and PR iteration")


def required_gate_event_classes_from_resolver(module, config) -> set[str]:
    gate_required = config.gate_names["gate_required"]
    backtester_required = config.gate_names["backtester_required"]
    actor_id = config.mergify_temp_pr_actor_id
    required_event_classes: set[str] = set()
    event_names = ("push", "pull_request", "merge_group", "workflow_dispatch", "unknown_event")
    actions = (
        "",
        "opened",
        "synchronize",
        "reopened",
        "edited",
        "ready_for_review",
        "converted_to_draft",
        "labeled",
        "checks_requested",
    )
    refs = (
        "refs/heads/main",
        "refs/tags/v1.2.3",
        "refs/heads/codex/branch",
        "refs/pull/1/merge",
        "refs/heads/gh-readonly-queue/main/pr-1-deadbeef",
    )
    head_refs = (
        "",
        "feature",
        "mergify/merge-queue/abc123abcd",
        "tmp-mergify/merge-queue/abc123abcd",
    )
    workflow_dispatch_values = ("", "false", "true", "TRUE", " true ", "1")
    sender_ids = (-1, 4242, actor_id)

    for event_name in event_names:
        for action in actions:
            for draft in (False, True):
                for base_changed in (False, True):
                    for workflow_dispatch_full_ci in workflow_dispatch_values:
                        for ref in refs:
                            for sender_id in sender_ids:
                                for head_ref in head_refs:
                                    for docs_only in (False, True):
                                        try:
                                            result = module.evaluate_ci_policy(
                                                config,
                                                event_name=event_name,
                                                event_action=action,
                                                pull_request_draft=draft,
                                                pull_request_head_ref=head_ref,
                                                pull_request_base_changed=base_changed,
                                                workflow_dispatch_full_ci=workflow_dispatch_full_ci,
                                                docs_only=docs_only,
                                                event_sender_id=sender_id,
                                                ref=ref,
                                            )
                                        except module.ProvenanceError:
                                            continue
                                        suffix = module.gate_name_suffix_for(
                                            event_name,
                                            result.reason,
                                            result.ci_policy_path,
                                        )
                                        has_required_names = (
                                            result.gate_name == gate_required
                                            and result.backtester_gate_name == backtester_required
                                        )
                                        if suffix == "required" and not has_required_names:
                                            raise AssertionError(
                                                f"required suffix must publish required gate names: {result}"
                                            )
                                        if has_required_names and suffix != "required":
                                            raise AssertionError(
                                                f"required gate names must derive from required suffix: {result}"
                                            )
                                        if suffix == "required":
                                            required_event_classes.add(result.expected_event_class)
    return required_event_classes


def assert_required_gate_proof_event_classes_match_resolver() -> None:
    module = load_script()
    configs = [module.load_config(module.DEFAULT_CONFIG)]
    with tempfile.TemporaryDirectory() as tmp:
        configs.append(module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML)))
    for config in configs:
        expected = required_gate_event_classes_from_resolver(module, config)
        actual = set(module.REQUIRED_GATE_PROOF_EVENT_CLASSES)
        if actual != expected:
            raise AssertionError(
                "REQUIRED_GATE_PROOF_EVENT_CLASSES must match resolver-required gate event classes: "
                f"actual={sorted(actual)!r} expected={sorted(expected)!r}"
            )


def assert_mergify_temp_pr_requires_actor_binding() -> None:
    # GAP-1 canary: a head-ref prefix alone must NEVER earn the required gate. The
    # mergify merge-queue temp PR is recognized only when github.event.sender.id is
    # the bound mergify actor; a spoofed head ref from any other sender (or an absent
    # sender id) fails closed to the non-required gate-iteration.
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        sender_absent = object()

        def run_with_event_sender(args: list[str], sender: object = sender_absent) -> tuple[int, str, str]:
            had_previous = "EVENT_SENDER_ID" in os.environ
            previous = os.environ.get("EVENT_SENDER_ID")
            try:
                if sender is sender_absent:
                    os.environ.pop("EVENT_SENDER_ID", None)
                else:
                    os.environ["EVENT_SENDER_ID"] = str(sender)
                return run_cli(args)
            finally:
                if had_previous and previous is not None:
                    os.environ["EVENT_SENDER_ID"] = previous
                else:
                    os.environ.pop("EVENT_SENDER_ID", None)

        def with_head_ref(args: list[str], head_ref: str) -> list[str]:
            replaced = list(args)
            replaced[replaced.index("--pull-request-head-ref") + 1] = head_ref
            return replaced

        def with_draft(args: list[str], draft: str) -> list[str]:
            replaced = list(args)
            replaced[replaced.index("--pull-request-draft") + 1] = draft
            return replaced

        base_args = [
            "ci-policy",
            "--config",
            str(config),
            "--event-name",
            "pull_request",
            "--event-action",
            "opened",
            "--pull-request-draft",
            "true",
            "--pull-request-head-ref",
            "mergify/merge-queue/83d4b0be7e",
            "--pull-request-base-changed",
            "false",
            "--workflow-dispatch-full-ci",
            "",
            "--docs-only",
            "false",
            "--ref",
            "refs/pull/965/merge",
        ]

        code, stdout, stderr = run_with_event_sender(base_args, "12345")
        if code != 0:
            raise AssertionError(f"spoofed mergify ci-policy failed: {stderr}")
        spoof = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if spoof.get("reason") == "mergify_temp_pr":
            raise AssertionError(f"head-ref prefix alone must not earn mergify_temp_pr: {spoof}")
        if (
            spoof.get("gate_name") != "gate-iteration"
            or spoof.get("backtester_gate_name") != "backtester-gate-iteration"
            or spoof.get("full_ci_required") != "false"
        ):
            raise AssertionError(f"spoofed mergify head ref must demote to gate-iteration: {spoof}")

        code, stdout, stderr = run_cli(base_args)
        if code != 0:
            raise AssertionError(f"sender-less mergify ci-policy failed: {stderr}")
        absent = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if absent.get("reason") == "mergify_temp_pr" or absent.get("gate_name") != "gate-iteration":
            raise AssertionError(f"absent sender id must fail closed to gate-iteration: {absent}")

        code, stdout, stderr = run_with_event_sender(base_args, "37929162")
        if code != 0:
            raise AssertionError(f"env-bound mergify ci-policy failed: {stderr}")
        env_bound = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if env_bound.get("reason") != "mergify_temp_pr" or env_bound.get("gate_name") != "gate":
            raise AssertionError(f"env EVENT_SENDER_ID must bind mergify temp PR to required gate: {env_bound}")

        code, stdout, stderr = run_with_event_sender(base_args, "37929162")
        if code != 0:
            raise AssertionError(f"bound-actor mergify ci-policy failed: {stderr}")
        bound = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if bound.get("reason") != "mergify_temp_pr":
            raise AssertionError(f"bound mergify actor must earn mergify_temp_pr: {bound}")
        if (
            bound.get("gate_name") != "gate"
            or bound.get("backtester_gate_name") != "backtester-gate"
            or bound.get("full_ci_required") != "true"
        ):
            raise AssertionError(f"bound mergify actor must earn the required gate and full CI: {bound}")

        tmp_args = with_head_ref(base_args, "tmp-mergify/merge-queue/83d4b0be7e")
        code, stdout, stderr = run_with_event_sender(tmp_args, "37929162")
        if code != 0:
            raise AssertionError(f"env-bound tmp mergify ci-policy failed: {stderr}")
        tmp_bound = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if (
            tmp_bound.get("reason") != "mergify_temp_pr"
            or tmp_bound.get("gate_name") != "gate"
            or tmp_bound.get("backtester_gate_name") != "backtester-gate"
            or tmp_bound.get("full_ci_required") != "true"
        ):
            raise AssertionError(f"tmp mergify ref from env-bound actor must earn required full CI: {tmp_bound}")

        code, stdout, stderr = run_with_event_sender(with_draft(tmp_args, "false"), "37929162")
        if code != 0:
            raise AssertionError(f"non-draft tmp mergify ci-policy failed: {stderr}")
        tmp_non_draft = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if tmp_non_draft.get("reason") == "mergify_temp_pr" or tmp_non_draft.get("gate_name") != "gate-iteration":
            raise AssertionError(f"non-draft tmp mergify ref must demote to gate-iteration: {tmp_non_draft}")

        code, stdout, stderr = run_with_event_sender(tmp_args, "12345")
        if code != 0:
            raise AssertionError(f"non-actor tmp mergify ci-policy failed: {stderr}")
        tmp_non_actor = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if tmp_non_actor.get("reason") == "mergify_temp_pr" or tmp_non_actor.get("gate_name") != "gate-iteration":
            raise AssertionError(f"non-actor tmp mergify ref must demote to gate-iteration: {tmp_non_actor}")

        code, stdout, stderr = run_with_event_sender(tmp_args, "not-a-number")
        if code != 0:
            raise AssertionError(f"malformed-env tmp mergify ci-policy failed: {stderr}")
        tmp_malformed_env = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if (
            tmp_malformed_env.get("reason") == "mergify_temp_pr"
            or tmp_malformed_env.get("gate_name") != "gate-iteration"
        ):
            raise AssertionError(f"malformed EVENT_SENDER_ID must demote tmp ref: {tmp_malformed_env}")


def assert_mergify_temp_pr_ready_event_uses_author_binding() -> None:
    # #1104 proof PRs can be marked ready by a human, so github.event.sender.id is not
    # always mergify[bot]. Bind to pull_request.user.id as well; a spoofed head ref
    # with a non-Mergify author must still demote.
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        base_args = [
            "ci-policy",
            "--config",
            str(config),
            "--event-name",
            "pull_request",
            "--event-action",
            "ready_for_review",
            "--pull-request-draft",
            "false",
            "--pull-request-head-ref",
            "mergify/merge-queue/016a10652b",
            "--pull-request-author-id",
            "37929162",
            "--pull-request-base-changed",
            "false",
            "--workflow-dispatch-full-ci",
            "",
            "--docs-only",
            "false",
            "--ref",
            "refs/pull/1104/merge",
        ]

        code, stdout, stderr = run_cli_with_event_sender(base_args, "1376128")
        if code != 0:
            raise AssertionError(f"human-ready Mergify proof PR ci-policy failed: {stderr}")
        proof = output_dict(stdout)
        if (
            proof.get("ci_policy_path") != "full"
            or proof.get("full_ci_required") != "true"
            or proof.get("gate_name") != "gate"
            or proof.get("backtester_gate_name") != "backtester-gate"
            or proof.get("expected_event_class") != "full"
            or proof.get("reason") != "mergify_temp_pr"
        ):
            raise AssertionError(f"human-ready Mergify proof PR must stay full queue proof: {proof}")

        spoof_args = list(base_args)
        spoof_args[spoof_args.index("--pull-request-author-id") + 1] = "1376128"
        code, stdout, stderr = run_cli_with_event_sender(spoof_args, "1376128")
        if code != 0:
            raise AssertionError(f"human-ready spoof proof PR ci-policy failed: {stderr}")
        spoof = output_dict(stdout)
        if spoof.get("reason") == "mergify_temp_pr" or spoof.get("gate_name") != "gate-iteration":
            raise AssertionError(f"non-Mergify-authored proof-shaped PR must demote: {spoof}")

        half_spoof_args = list(base_args)
        half_spoof_args[half_spoof_args.index("--pull-request-author-id") + 1] = "1376128"
        code, stdout, stderr = run_cli_with_event_sender(half_spoof_args, "37929162")
        if code != 0:
            raise AssertionError(f"split-identity ready proof PR ci-policy failed: {stderr}")
        half_spoof = output_dict(stdout)
        if half_spoof.get("reason") == "mergify_temp_pr" or half_spoof.get("gate_name") != "gate-iteration":
            raise AssertionError(f"Mergify sender with non-Mergify author must demote: {half_spoof}")


def assert_mergify_temp_pr_synchronize_requires_sender_binding() -> None:
    # Author binding exists only to preserve the human ready_for_review transition on a
    # Mergify-authored proof PR. Byte-changing events still need the sender to be
    # Mergify; otherwise a human-triggered update could earn the required queue gate.
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        args = [
            "ci-policy",
            "--config",
            str(config),
            "--event-name",
            "pull_request",
            "--event-action",
            "synchronize",
            "--pull-request-draft",
            "false",
            "--pull-request-head-ref",
            "mergify/merge-queue/016a10652b",
            "--pull-request-author-id",
            "37929162",
            "--pull-request-base-changed",
            "false",
            "--workflow-dispatch-full-ci",
            "",
            "--docs-only",
            "false",
            "--ref",
            "refs/pull/1104/merge",
        ]

        code, stdout, stderr = run_cli_with_event_sender(args, "1376128")
        if code != 0:
            raise AssertionError(f"human-sync Mergify proof PR ci-policy failed: {stderr}")
        result = output_dict(stdout)
        if result.get("reason") == "mergify_temp_pr" or result.get("gate_name") != "gate-iteration":
            raise AssertionError(f"human-sync Mergify proof PR must demote without sender binding: {result}")


def assert_mergify_temp_pr_edited_event_splits_metadata_from_base_change() -> None:
    # Mergify temp PR title/body edits arrive as pull_request/edited without a base
    # change. They must not start a fresh full proof run; only edited events that
    # actually change the base ref still publish required gates.
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        args = [
            "ci-policy",
            "--config",
            str(config),
            "--event-name",
            "pull_request",
            "--event-action",
            "edited",
            "--pull-request-draft",
            "true",
            "--pull-request-head-ref",
            "mergify/merge-queue/46623f919f",
            "--pull-request-author-id",
            "37929162",
            "--pull-request-base-changed",
            "false",
            "--workflow-dispatch-full-ci",
            "",
            "--docs-only",
            "false",
            "--ref",
            "refs/pull/1134/merge",
        ]

        code, stdout, stderr = run_cli_with_event_sender(args, "37929162")
        if code != 0:
            raise AssertionError(f"Mergify edited metadata PR ci-policy failed: {stderr}")
        result = output_dict(stdout)
        expected = {
            "ci_policy_path": "iteration",
            "full_ci_required": "false",
            "gate_name": "gate-iteration",
            "backtester_gate_name": "backtester-gate-iteration",
            "expected_event_class": "iteration",
            "reason": "draft_pr_edited",
        }
        actual = {key: result.get(key) for key in expected}
        if actual != expected:
            raise AssertionError(f"Mergify edited metadata PR must stay iteration-only: {actual}")

        base_change_args = list(args)
        base_change_args[base_change_args.index("--pull-request-base-changed") + 1] = "true"
        code, stdout, stderr = run_cli_with_event_sender(base_change_args, "37929162")
        if code != 0:
            raise AssertionError(f"Mergify edited base-change PR ci-policy failed: {stderr}")
        result = output_dict(stdout)
        expected = {
            "ci_policy_path": "full",
            "full_ci_required": "true",
            "gate_name": "gate",
            "backtester_gate_name": "backtester-gate",
            "expected_event_class": "full",
            "reason": "mergify_temp_pr",
        }
        actual = {key: result.get(key) for key in expected}
        if actual != expected:
            raise AssertionError(f"Mergify edited base-change PR must publish required gates: {actual}")

        non_draft_base_change_args = list(base_change_args)
        non_draft_base_change_args[
            non_draft_base_change_args.index("--pull-request-draft") + 1
        ] = "false"
        code, stdout, stderr = run_cli_with_event_sender(non_draft_base_change_args, "37929162")
        if code != 0:
            raise AssertionError(f"ready Mergify edited base-change PR ci-policy failed: {stderr}")
        result = output_dict(stdout)
        actual = {key: result.get(key) for key in expected}
        if actual != expected:
            raise AssertionError(f"ready Mergify edited base-change PR must publish required gates: {actual}")


def assert_parse_event_sender_id_fails_closed() -> None:
    module = load_script()
    cases = {
        "37929162": 37929162,
        "  37929162 ": 37929162,
        "-1": -1,
        "": -1,
        "bot": -1,
        "37929162x": -1,
        None: -1,
        37929162: 37929162,
    }
    for raw, expected in cases.items():
        # Suppress the fail-loud stderr here; its contract is asserted explicitly below.
        with contextlib.redirect_stderr(io.StringIO()):
            got = module.parse_event_sender_id(raw)
        if got != expected:
            raise AssertionError(f"parse_event_sender_id({raw!r}) -> {got}, expected {expected}")

    # Fail-loud contract: a non-empty malformed sender id warns on stderr (a wiring bug
    # that would otherwise silently deadlock the queue); empty (the expected senderless
    # case) and valid ids stay quiet.
    def _stderr_for(raw: object) -> str:
        buf = io.StringIO()
        with contextlib.redirect_stderr(buf):
            module.parse_event_sender_id(raw)
        return buf.getvalue()

    if "warning:" not in _stderr_for("bot"):
        raise AssertionError("non-empty malformed EVENT_SENDER_ID must warn on stderr (fail loud)")
    if _stderr_for("").strip():
        raise AssertionError("empty EVENT_SENDER_ID (senderless) must stay quiet")
    if _stderr_for("37929162").strip():
        raise AssertionError("valid EVENT_SENDER_ID must not warn")


def assert_ci_policy_non_numeric_sender_id_does_not_crash() -> None:
    # A malformed EVENT_SENDER_ID must demote, never crash the ci-policy job.
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        code, stdout, stderr = run_cli_with_event_sender(
            [
                "ci-policy",
                "--config",
                str(config),
                "--event-name",
                "pull_request",
                "--event-action",
                "opened",
                "--pull-request-draft",
                "true",
                "--pull-request-head-ref",
                "mergify/merge-queue/83d4b0be7e",
                "--pull-request-base-changed",
                "false",
                "--workflow-dispatch-full-ci",
                "",
                "--docs-only",
                "false",
                "--ref",
                "refs/pull/965/merge",
            ],
            "not-a-number",
        )
    if code != 0:
        raise AssertionError(f"non-numeric sender id must not crash ci-policy: {stderr}")
    out = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
    if out.get("reason") == "mergify_temp_pr" or out.get("gate_name") != "gate-iteration":
        raise AssertionError(f"malformed sender id must fail closed to gate-iteration: {out}")


def assert_mergify_actor_binding_demotes_every_full_action() -> None:
    # GAP-1 canary across the WHOLE full-CI action set (imported, never re-listed): a
    # spoofed mergify head ref from a non-actor sender must NEVER earn reason
    # mergify_temp_pr and must demote to the non-required gate-iteration, for every
    # action that mergify_temp_pr_requires_full_ci treats as full.
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
    gate_iteration = config.gate_names["gate_iteration"]
    non_actor = config.mergify_temp_pr_actor_id + 1
    cases = [(a, False) for a in sorted(module.MERGIFY_TEMP_PR_FULL_ACTIONS)] + [("edited", True)]
    for action, base_changed in cases:
        # ready_for_review is the only full action delivered on a NON-draft PR; every other
        # action a mergify temp PR fires is on a draft. A temp PR is always a draft, so here
        # the actor binding is the only clause that should fail.
        draft = action != "ready_for_review"
        result = module.evaluate_ci_policy(
            config,
            event_name="pull_request",
            event_action=action,
            pull_request_draft=draft,
            pull_request_head_ref="mergify/merge-queue/abc123",
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci="",
            event_sender_id=non_actor,
            ref="refs/pull/965/merge",
        )
        if result.reason == "mergify_temp_pr":
            raise AssertionError(f"{action}: non-actor sender must never earn mergify_temp_pr: {result}")
        if result.full_ci_required or result.gate_name != gate_iteration:
            raise AssertionError(f"{action}: non-actor spoof must demote to gate-iteration: {result}")


def assert_ready_pr_defers_proof_to_merge_boundary_without_required_gate() -> None:
    # Comprehensive #981 proof: a ready PR defers heavy lanes and never earns the
    # required gate; the required gate is produced ONLY at the merge boundary
    # (merge_group, the actor-bound mergify temp PR, push to main). The iteration
    # verdict succeeds with heavy jobs skipped, while a full/required verdict over the
    # SAME iteration-class evidence fails closed.
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
    actor_id = config.mergify_temp_pr_actor_id
    gate_required = config.gate_names["gate_required"]
    gate_iteration = config.gate_names["gate_iteration"]
    backtester_required = config.gate_names["backtester_required"]
    backtester_iteration = config.gate_names["backtester_iteration"]

    # (A) ready PR defers.
    ready = module.evaluate_ci_policy(
        config,
        event_name="pull_request",
        event_action="opened",
        pull_request_draft=False,
        pull_request_head_ref="",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=4242,
        ref="refs/pull/1/merge",
    )
    if ready.ci_policy_path != "iteration" or ready.full_ci_required or ready.full_ci_deferred:
        raise AssertionError(f"ready PR must defer to iteration with full_ci_required false: {ready}")
    if ready.expected_event_class != "iteration":
        raise AssertionError(f"ready PR must resolve expected_event_class iteration: {ready}")
    # (B) ready PR earns only the non-required gate.
    if ready.gate_name != gate_iteration or ready.gate_name == gate_required:
        raise AssertionError(f"ready PR must publish gate-iteration, never the required gate: {ready}")
    if ready.backtester_gate_name != backtester_iteration:
        raise AssertionError(f"ready PR must publish backtester-gate-iteration: {ready}")
    # (C) iteration verdict succeeds on skipped-heavy evidence; the negative control
    # — a full/required verdict over that same evidence — fails closed.
    iteration_jobs = base_ci_gate_jobs(
        deny="skipped",
        clippy="skipped",
        **{
            "check-aarch64": "skipped",
            "source-fence": "skipped",
            "nextest-fingerprint": "skipped",
            "test-archive": "skipped",
            "nextest-fingerprint-reuse": "skipped",
            "test": "skipped",
            "build": "skipped",
            "ci-provenance-emit": "skipped",
        },
    )
    module.evaluate_ci_gate_verdict(
        policy_path="iteration",
        expected_event_class="iteration",
        full_ci_deferred=False,
        ignore_emit_failure=False,
        reuse_found=False,
        carry_forward_verified=False,
        job_results=iteration_jobs,
        build_required=False,
    )
    assert_raises(
        "full CI policy outside resolver-permitted event class 'iteration'",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="iteration",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results=iteration_jobs,
            build_required=False,
        ),
    )
    # (D) the merge boundary produces the required gate from every trusted producer.
    merge_group = module.evaluate_ci_policy(
        config,
        event_name="merge_group",
        event_action="checks_requested",
        pull_request_draft=False,
        pull_request_head_ref="",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=-1,
        ref="refs/heads/gh-readonly-queue/main/pr-1-deadbeef",
    )
    if (
        merge_group.gate_name != gate_required
        or merge_group.backtester_gate_name != backtester_required
        or not merge_group.full_ci_required
    ):
        raise AssertionError(f"merge_group must produce the required gate and full CI: {merge_group}")
    mergify = module.evaluate_ci_policy(
        config,
        event_name="pull_request",
        event_action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/abc123",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if mergify.reason != "mergify_temp_pr" or mergify.gate_name != gate_required:
        raise AssertionError(f"actor-bound mergify temp PR must produce the required gate: {mergify}")
    main_push = module.evaluate_ci_policy(
        config,
        event_name="push",
        event_action="",
        pull_request_draft=False,
        pull_request_head_ref="",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=-1,
        ref="refs/heads/main",
    )
    if main_push.gate_name != gate_required:
        raise AssertionError(f"push to main must produce the required gate: {main_push}")
    # (E) force_full_ci on a pull_request keeps full lanes but demotes the gate.
    with tempfile.TemporaryDirectory() as tmp:
        forced = module.load_config(
            write_config(pathlib.Path(tmp), CONFIG_TOML.replace("force_full_ci = false", "force_full_ci = true"))
        )
    forced_result = module.evaluate_ci_policy(
        forced,
        event_name="pull_request",
        event_action="synchronize",
        pull_request_draft=False,
        pull_request_head_ref="",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=4242,
        ref="refs/pull/1/merge",
    )
    if forced_result.reason != "force_full_ci" or forced_result.ci_policy_path != "full":
        raise AssertionError(f"force_full_ci must keep ci_policy_path full: {forced_result}")
    if forced_result.gate_name != gate_iteration:
        raise AssertionError(f"force_full_ci on a pull_request must demote to gate-iteration: {forced_result}")
    # (F) a spoofed mergify head ref from a non-actor sender stays gate-iteration.
    spoof = module.evaluate_ci_policy(
        config,
        event_name="pull_request",
        event_action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/abc123",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
        event_sender_id=4242,
        ref="refs/pull/965/merge",
    )
    if spoof.reason == "mergify_temp_pr" or spoof.gate_name != gate_iteration:
        raise AssertionError(f"spoofed mergify head ref must stay gate-iteration: {spoof}")


def assert_dispatch_run_names_come_from_config() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
    if config.dispatch_run_name_default != "CI":
        raise AssertionError(config)
    if config.dispatch_run_name_full != "CI [dispatch:full]":
        raise AssertionError(config)
    if config.dispatch_run_name_iteration != "CI [dispatch:iteration]":
        raise AssertionError(config)
    if config.dispatch_proof_gate_job != "gate":
        raise AssertionError(config)
    if config.gate_names["gate_dispatch_full"] != "gate-dispatch":
        raise AssertionError(config)
    if config.gate_names["backtester_dispatch_full"] != "backtester-gate-dispatch":
        raise AssertionError(config)


def assert_gate_names_reject_github_output_control_chars() -> None:
    module = load_script()
    unsafe_values = [
        ("gate_dispatch_full", "gate-dispatch", "gate-dispatch "),
        ("backtester_dispatch_full", "backtester-gate-dispatch", " backtester-gate-dispatch"),
        ("gate_dispatch_full", "gate-dispatch", "gate\\nignored=1"),
        ("backtester_dispatch_full", "backtester-gate-dispatch", "backtester-gate\\rignored=1"),
        ("gate_iteration", "gate-iteration", "${{ github.ref }}"),
        ("backtester_iteration", "backtester-gate-iteration", "backtester-gate-iteration }}"),
    ]
    for key, original, replacement in unsafe_values:
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(
                pathlib.Path(tmp),
                CONFIG_TOML.replace(f'{key} = "{original}"', f'{key} = "{replacement}"'),
            )
            try:
                module.load_config(config)
            except module.ProvenanceError as exc:
                if "must be a GitHub Actions output-safe check name" not in str(exc):
                    raise AssertionError(f"unexpected error for {key}: {exc}") from exc
            else:
                raise AssertionError(f"unsafe gate name {key}={replacement!r} must be rejected")


def assert_gate_names_reject_collisions() -> None:
    module = load_script()
    cases = {
        "ci_provenance.gate_names.gate_iteration must not equal gate_required": CONFIG_TOML.replace(
            'gate_iteration = "gate-iteration"',
            'gate_iteration = "gate"',
        ),
        "ci_provenance.gate_names.backtester_iteration must not equal backtester_required": CONFIG_TOML.replace(
            'backtester_iteration = "backtester-gate-iteration"',
            'backtester_iteration = "backtester-gate"',
        ),
        "ci_provenance.gate_names.gate_dispatch_full must not equal gate_required": CONFIG_TOML.replace(
            'gate_dispatch_full = "gate-dispatch"',
            'gate_dispatch_full = "gate"',
        ),
        "ci_provenance.gate_names.backtester_dispatch_full must not equal backtester_required": CONFIG_TOML.replace(
            'backtester_dispatch_full = "backtester-gate-dispatch"',
            'backtester_dispatch_full = "backtester-gate"',
        ),
    }
    for fragment, config_text in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp), config_text)
            try:
                module.load_config(config)
            except module.ProvenanceError as exc:
                if fragment not in str(exc):
                    raise AssertionError(f"expected {fragment!r}, got {exc}") from exc
            else:
                raise AssertionError(f"gate-name collision must be rejected: {fragment}")


def replace_once(text: str, old: str, new: str) -> str:
    if old not in text:
        raise AssertionError(f"missing replacement target: {old!r}")
    return text.replace(old, new, 1)


EXPECTED_REQUIRED_CHECK_PROOF_RULES = {
    "gate": {
        "runs_on_tags": True,
        "supports_carry_forward": True,
        "fresh": ("full", "tag_reuse"),
        "carry_forward": (),
    },
    "backtester-gate": {
        "runs_on_tags": True,
        "supports_carry_forward": True,
        "fresh": ("full", "tag_reuse"),
        "carry_forward": (),
    },
    "host-health": {
        "runs_on_tags": False,
        "supports_carry_forward": False,
        "fresh": ("full", "docs", "iteration"),
        "carry_forward": (),
    },
    "actionlint": {
        "runs_on_tags": False,
        "supports_carry_forward": False,
        "fresh": ("full", "docs", "iteration"),
        "carry_forward": (),
    },
    "coverage-enforcer": {
        "runs_on_tags": False,
        "supports_carry_forward": False,
        "fresh": ("full", "docs", "iteration"),
        "carry_forward": (),
    },
}


def assert_required_check_proof_rules(config) -> None:
    for context, expected in EXPECTED_REQUIRED_CHECK_PROOF_RULES.items():
        check = config.required_checks[context]
        if check.runs_on_tags != expected["runs_on_tags"]:
            raise AssertionError(f"{context} runs_on_tags drifted: {check}")
        if check.supports_carry_forward != expected["supports_carry_forward"]:
            raise AssertionError(f"{context} supports_carry_forward drifted: {check}")
        if check.fresh_event_classes != expected["fresh"]:
            raise AssertionError(f"{context} fresh proof rule drifted: {check}")
        if check.carry_forward_event_classes != expected["carry_forward"]:
            raise AssertionError(f"{context} carry-forward proof rule drifted: {check}")


def assert_required_checks_registry_matches_sources() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        assert_required_check_proof_rules(
            module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
        )
    assert_required_check_proof_rules(module.load_config(module.DEFAULT_CONFIG))


def assert_required_checks_registry_rejects_unreachable_proof_rules() -> None:
    module = load_script()
    mutations = {
        "supports_carry_forward=false": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []
""",
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs"]
carry_forward = ["iteration"]
""",
        ),
        "carry_forward must be": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []
""",
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = ["tag_reuse"]
""",
        ),
        "runs_on_tags=false": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration"]
carry_forward = []
""",
            """[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration", "tag_reuse"]
carry_forward = []
""",
        ),
    }
    for fragment, config_text in mutations.items():
        with tempfile.TemporaryDirectory() as tmp:
            config_path = write_config(pathlib.Path(tmp), config_text)
            assert_raises(fragment, lambda: module.load_config(config_path))


def assert_required_checks_registry_is_closed() -> None:
    module = load_script()
    dead_context = """

[ci_provenance.required_checks.dead-context]
context = "dead-context"
reporter = "stale"
integration_id = 15368
required = false
target = false
runs_on_tags = true
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.dead-context.proof_rule]
fresh = ["full", "docs", "iteration", "tag_reuse", "defer", "noop"]
carry_forward = []
"""
    with tempfile.TemporaryDirectory() as tmp:
        config_path = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace(
                "\n[ci_provenance.docs]",
                dead_context + "\n[ci_provenance.docs]",
            ),
        )
        assert_raises(
            "required-check registry contexts must be closed",
            lambda: module.load_config(config_path),
        )


def assert_required_checks_registry_rejects_arrival_and_target_drift() -> None:
    module = load_script()
    mutations = {
        "arrivals must be": replace_once(
            CONFIG_TOML,
            'arrivals = ["pull_request", "merge_group"]',
            'arrivals = ["pull_request"]',
        ),
        "coverage-enforcer must be required=false target=true": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.coverage-enforcer]
context = "coverage-enforcer"
reporter = "self"
integration_id = 15368
required = false
target = true
""",
            """[ci_provenance.required_checks.coverage-enforcer]
context = "coverage-enforcer"
reporter = "self"
integration_id = 15368
required = true
target = true
""",
        ),
    }
    for fragment, config_text in mutations.items():
        with tempfile.TemporaryDirectory() as tmp:
            config_path = write_config(pathlib.Path(tmp), config_text)
            assert_raises(fragment, lambda: module.load_config(config_path))


def assert_required_checks_registry_rejects_drift() -> None:
    module = load_script()
    mutations = {
        "required-check registry contexts": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.host-health]
context = "host-health"
reporter = "ci.yml host-health lane"
integration_id = 15368
required = true
""",
            """[ci_provenance.required_checks.host-health]
context = "host-health"
reporter = "ci.yml host-health lane"
integration_id = 15368
required = false
""",
        ),
        "integration_id": replace_once(
            CONFIG_TOML,
            "integration_id = 15368",
            "integration_id = 15369",
        ),
        "proof_rule.fresh must be": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "tag_reuse"]
carry_forward = []
""",
            """[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "docs", "tag_reuse"]
carry_forward = []
""",
        ),
        "fresh must be": replace_once(
            CONFIG_TOML,
            """[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "tag_reuse"]
carry_forward = []
""",
            """[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "iteration", "tag_reuse"]
carry_forward = []
""",
        ),
    }
    for fragment, config_text in mutations.items():
        with tempfile.TemporaryDirectory() as tmp:
            config_path = write_config(pathlib.Path(tmp), config_text)
            assert_raises(fragment, lambda: module.load_config(config_path))


def assert_policy_contract_rejects_required_gate_holes() -> None:
    module = load_script()
    # Queue-only rework (#981): every ordinary pull_request row is pinned to
    # "iteration"; only the genuine merge-boundary rows stay "full". A non-iteration
    # value on a PR row, or a non-full value on a boundary row, is a required-gate
    # hole and must fail the load-time contract closed.
    cases = {
        "ci_provenance.policy.workflow_dispatch must be iteration": CONFIG_TOML.replace(
            'workflow_dispatch = "iteration"',
            'workflow_dispatch = "full"',
        ),
        "ci_provenance.policy.draft_pr_synchronize must be iteration": CONFIG_TOML.replace(
            'draft_pr_synchronize = "iteration"',
            'draft_pr_synchronize = "full"',
        ),
        "ci_provenance.policy.draft_pr_opened must be iteration": CONFIG_TOML.replace(
            'draft_pr_opened = "iteration"',
            'draft_pr_opened = "full"',
        ),
        "ci_provenance.policy.draft_pr_reopened must be iteration": CONFIG_TOML.replace(
            'draft_pr_reopened = "iteration"',
            'draft_pr_reopened = "full"',
        ),
        "ci_provenance.policy.draft_pr_edited must be iteration": CONFIG_TOML.replace(
            'draft_pr_edited = "iteration"',
            'draft_pr_edited = "full"',
        ),
        "ci_provenance.policy.converted_to_draft must be iteration": CONFIG_TOML.replace(
            'converted_to_draft = "iteration"',
            'converted_to_draft = "full"',
        ),
        "ci_provenance.policy.ready_pr must be iteration": CONFIG_TOML.replace(
            'ready_pr = "iteration"',
            'ready_pr = "full"',
        ),
        "ci_provenance.policy.ready_for_review must be iteration": CONFIG_TOML.replace(
            'ready_for_review = "iteration"',
            'ready_for_review = "full"',
        ),
        "ci_provenance.policy.ready_pr_edited_no_base must be iteration": CONFIG_TOML.replace(
            'ready_pr_edited_no_base = "iteration"',
            'ready_pr_edited_no_base = "full"',
        ),
        "ci_provenance.policy.ready_pr_reopened must be iteration": CONFIG_TOML.replace(
            'ready_pr_reopened = "iteration"',
            'ready_pr_reopened = "full"',
        ),
        "ci_provenance.policy.main_push must be full": CONFIG_TOML.replace(
            'main_push = "full"',
            'main_push = "iteration"',
        ),
        "ci_provenance.policy.merge_group must be full": CONFIG_TOML.replace(
            'merge_group = "full"',
            'merge_group = "iteration"',
        ),
        "ci_provenance.policy.mergify_temp_pr must be full": CONFIG_TOML.replace(
            'mergify_temp_pr = "full"',
            'mergify_temp_pr = "iteration"',
        ),
        "ci_provenance.policy.unknown_event must be full": CONFIG_TOML.replace(
            'unknown_event = "full"',
            'unknown_event = "iteration"',
        ),
        "ci_provenance.policy.workflow_dispatch_full_ci must remain full": CONFIG_TOML.replace(
            'workflow_dispatch_full_ci = "full"',
            'workflow_dispatch_full_ci = "iteration"',
        ),
        "ci_provenance.policy.docs must be docs": CONFIG_TOML.replace(
            'docs = "docs"',
            'docs = "full"',
        ),
        "ci_provenance.policy has unexpected keys": CONFIG_TOML.replace(
            "\n[ci_provenance.mergify]",
            '\nsynthetic_new = "full"\n\n[ci_provenance.mergify]',
        ),
    }
    for fragment, config_text in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp), config_text)
            try:
                module.load_config(config)
            except module.ProvenanceError as exc:
                if fragment not in str(exc):
                    raise AssertionError(f"expected {fragment!r}, got {exc}") from exc
            else:
                raise AssertionError(f"unsafe policy mutation must be rejected: {fragment}")

    with tempfile.TemporaryDirectory() as tmp:
        policy = dict(module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML)).policy)
        original_rows = module.POLICY_ROWS
        try:
            module.POLICY_ROWS = (*original_rows, "synthetic_uncontracted")
            policy["synthetic_uncontracted"] = "iteration"
            errors = module.policy_contract_errors(policy)
        finally:
            module.POLICY_ROWS = original_rows
    if not any("rows must define required or allowed contract: synthetic_uncontracted" in error for error in errors):
        raise AssertionError(f"uncontracted policy rows must fail closed, got: {errors}")


def assert_main_evidence_matching_ignores_mutable_run_name() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = module.load_config(write_config(pathlib.Path(tmp), CONFIG_TOML))
    exact = {
        "id": 100,
        "name": "CI [dispatch:iteration]",
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "main",
        "head_sha": SHA,
        "status": "completed",
        "conclusion": "success",
    }
    if not module.run_matches_exact_sha(exact, config, SHA, current_run_id=None):
        raise AssertionError("exact-SHA evidence must not depend on mutable run.name")
    if not module.run_matches_fingerprint_reuse(exact, config, current_run_id=None):
        raise AssertionError("fingerprint reuse evidence must not depend on mutable run.name")


def assert_config_digest_is_canonical() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        base_config = write_config(tmp_path, CONFIG_TOML, "base.toml")
        reordered_config = write_config(tmp_path, CONFIG_TOML_REORDERED, "reordered.toml")
        if module.provenance_config_digest(base_config) != module.provenance_config_digest(reordered_config):
            raise AssertionError("logical TOML order must not affect provenance config digest")

        changed_meter = write_config(
            tmp_path,
            CONFIG_TOML.replace(
                'fingerprint_artifact_prefix = "nextest-archive-fingerprint-"',
                'fingerprint_artifact_prefix = "changed-"',
            ),
            "changed-meter.toml",
        )
        if module.provenance_config_digest(base_config) == module.provenance_config_digest(changed_meter):
            raise AssertionError("referenced meter values must affect provenance config digest")

        unrelated = write_config(tmp_path, CONFIG_TOML + "\n[unrelated]\nvalue = 1\n", "unrelated.toml")
        if module.provenance_config_digest(base_config) != module.provenance_config_digest(unrelated):
            raise AssertionError("unrelated TOML tables must not affect provenance config digest")


def assert_github_api_bytes_strips_authorization_on_cross_host_redirect() -> None:
    module = load_script()
    seen_headers: queue.Queue[dict[str, str]] = queue.Queue()

    class ArtifactHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            seen_headers.put(dict(self.headers))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"artifact")

        def log_message(self, _format: str, *args: object) -> None:
            pass

    class RedirectHandler(http.server.BaseHTTPRequestHandler):
        redirect_target = ""

        def do_GET(self) -> None:
            self.send_response(302)
            self.send_header("Location", self.redirect_target)
            self.end_headers()

        def log_message(self, _format: str, *args: object) -> None:
            pass

    artifact_server = socketserver.TCPServer(("127.0.0.1", 0), ArtifactHandler)
    redirect_server = socketserver.TCPServer(("127.0.0.1", 0), RedirectHandler)
    artifact_thread = threading.Thread(target=artifact_server.serve_forever, daemon=True)
    redirect_thread = threading.Thread(target=redirect_server.serve_forever, daemon=True)
    try:
        artifact_thread.start()
        redirect_thread.start()
        RedirectHandler.redirect_target = f"http://127.0.0.1:{artifact_server.server_address[1]}/artifact"
        start_url = f"http://localhost:{redirect_server.server_address[1]}/artifact"
        payload = module.github_api_bytes("owner/repo", "secret-token", start_url)
    finally:
        redirect_server.shutdown()
        artifact_server.shutdown()
        redirect_server.server_close()
        artifact_server.server_close()

    if payload != b"artifact":
        raise AssertionError(f"unexpected payload: {payload!r}")
    redirected_headers = seen_headers.get(timeout=5)
    if "Authorization" in redirected_headers:
        raise AssertionError(f"redirected request leaked authorization: {redirected_headers}")
    if "Accept" in redirected_headers:
        raise AssertionError(f"redirected request leaked GitHub Accept header: {redirected_headers}")
    if "X-GitHub-Api-Version" in redirected_headers:
        raise AssertionError(f"redirected request leaked GitHub API version header: {redirected_headers}")


def assert_sensitive_headers_only_survive_same_https_origin_redirects() -> None:
    module = load_script()
    preserve = module.redirect_preserves_github_api_headers
    if not preserve(
        "https://api.github.com/repos/owner/repo/actions",
        "https://api.github.com/repos/owner/repo/runs",
    ):
        raise AssertionError("same HTTPS origin redirect should preserve GitHub API headers")
    if not preserve(
        "https://API.github.com:443/repos/owner/repo/actions",
        "https://api.GITHUB.com/repos/owner/repo/runs",
    ):
        raise AssertionError("case-only host redirects and default HTTPS ports should preserve GitHub API headers")
    if not preserve(
        "https://user:pass@api.github.com/repos/owner/repo/actions",
        "https://user:pass@api.github.com/repos/owner/repo/runs",
    ):
        raise AssertionError("unchanged same-origin userinfo should preserve GitHub API headers")
    if preserve(
        "https://api.github.com/repos/owner/repo/actions",
        "http://api.github.com/repos/owner/repo/runs",
    ):
        raise AssertionError("same-netloc HTTPS downgrade must not preserve GitHub API headers")
    if preserve(
        "https://api.github.com/repos/owner/repo/actions",
        "https://api.github.com:8443/repos/owner/repo/runs",
    ):
        raise AssertionError("same-host port change must not preserve GitHub API headers")
    if preserve(
        "https://api.github.com/repos/owner/repo/actions",
        "https://user@api.github.com/repos/owner/repo/runs",
    ):
        raise AssertionError("userinfo changes must not preserve GitHub API headers")
    if preserve(
        "https://api.github.com/repos/owner/repo/actions",
        "https://objects.githubusercontent.com/artifact",
    ):
        raise AssertionError("cross-host redirect must not preserve GitHub API headers")


def assert_github_api_json_rejects_invalid_utf8_as_provenance_error() -> None:
    module = load_script()

    class BadUtf8Response:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):
            return False

        def read(self) -> bytes:
            return b"\xff"

    original_open = module.open_github_api_request
    module.open_github_api_request = lambda *args, **kwargs: BadUtf8Response()
    try:
        assert_raises(
            "GitHub API request failed for actions/runs",
            lambda: module.github_api_json("owner/repo", "token", "actions/runs"),
        )
    finally:
        module.open_github_api_request = original_open


def assert_artifact_record_rejects_invalid_utf8_json() -> None:
    module = load_script()
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("ci-provenance.json", b"\xff")
    assert_raises(
        "ci-provenance.json is invalid JSON",
        lambda: module.artifact_record_from_zip(buffer.getvalue()),
    )


def assert_record_schema_requires_head_and_tested_sha() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        for key in ("head_sha", "tested_sha"):
            broken = dict(record)
            broken.pop(key)
            try:
                module.validate_record_schema(broken, loaded, config_path=config)
            except Exception as exc:  # noqa: BLE001 - domain error.
                if key not in str(exc):
                    raise AssertionError(f"expected {key} error, got {exc}") from exc
            else:
                raise AssertionError(f"missing {key} must fail")


def assert_pr_event_record_cannot_validate_for_pr_head_reuse() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        record["event"] = "pull_request"
        record["head_branch"] = "feature"
        record["tested_sha"] = "0" * 40
        record["pull_request"] = {"number": 648, "base_sha": "1" * 40}
        try:
            module.validate_exact_sha_record(record, loaded, requested_sha=SHA, config_path=config)
        except Exception as exc:  # noqa: BLE001 - domain error.
            if "pull_request" not in str(exc):
                raise AssertionError(f"expected pull_request rejection, got {exc}") from exc
        else:
            raise AssertionError("pull_request record must not validate as PR-head exact-SHA evidence")


def assert_digest_mismatches_fail() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        for key in ("workflow_digest", "provenance_config_digest"):
            broken = dict(record)
            broken[key] = "0" * 64
            try:
                module.validate_record_schema(broken, loaded, config_path=config)
            except Exception as exc:  # noqa: BLE001 - domain error.
                if key not in str(exc):
                    raise AssertionError(f"expected {key} error, got {exc}") from exc
            else:
                raise AssertionError(f"{key} mismatch must fail")


def resolve_with_fake(
    module,
    config_path: pathlib.Path,
    fake: FakeGitHub,
    *,
    now: str = "2026-06-13T00:30:00Z",
):
    return module.resolve_exact_sha_evidence(
        repo="seungpyoson/bolt-v2",
        token="token",
        requested_sha=SHA,
        config=module.load_config(config_path),
        config_path=config_path,
        api_json=fake.json,
        api_bytes=fake.bytes,
        now=module.parse_timestamp(now),
    )


def assert_no_candidate_evidence_fails() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        fake = FakeGitHub(runs_pages=[[]])
        assert_raises("no candidate provenance evidence", lambda: resolve_with_fake(module, config, fake))
        _, query = fake.queries[0]
        if query is None or query.get("sort") != "created" or query.get("direction") != "desc":
            raise AssertionError(f"workflow run query must request explicit newest-first ordering, got {query}")


def assert_lookback_exhaustion_fails() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 1").replace(
                "max_lookback_pages = 10", "max_lookback_pages = 2"
            ),
        )
        fake = FakeGitHub(
            runs_pages=[
                [run_payload(id=1, conclusion="failure")],
                [run_payload(id=2, conclusion="failure")],
            ]
        )
        assert_raises("lookback page limit exhausted", lambda: resolve_with_fake(module, config, fake))


def assert_lookback_age_exhaustion_fails() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("max_lookback_age_seconds = 1209600", "max_lookback_age_seconds = 1"),
        )
        fake = FakeGitHub(runs_pages=[[run_payload(created_at="2020-01-01T00:00:00Z")]])
        assert_raises("lookback age limit exhausted", lambda: resolve_with_fake(module, config, fake))


def assert_deploy_evidence_uses_deploy_artifact_lookback() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(
            tmp_path,
            CONFIG_TOML.replace("artifact_lookback_age_seconds = 259200", "artifact_lookback_age_seconds = 1"),
        )
        record = valid_record(module, config)
        fake = FakeGitHub(
            runs_pages=[[run_payload(created_at="2026-06-13T00:00:00Z")]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
        )
        assert_raises("lookback age limit exhausted", lambda: resolve_with_fake(module, config, fake))


def assert_lookback_age_does_not_stop_same_page_scan() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        fake = FakeGitHub(
            runs_pages=[
                [
                    run_payload(id=RUN_ID + 1, conclusion="failure", created_at="2020-01-01T00:00:00Z"),
                    run_payload(),
                ]
            ],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
        )
        resolved = resolve_with_fake(module, config, fake)
        if resolved.run.get("id") != RUN_ID:
            raise AssertionError(f"expected fresh candidate run {RUN_ID}, got {resolved.run}")


def assert_missing_created_at_fails_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        fake = FakeGitHub(runs_pages=[[run_payload(created_at=None)]])
        assert_raises("workflow run created_at must be a string", lambda: resolve_with_fake(module, config, fake))


def assert_artifact_rejections() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        run = run_payload()
        ambiguous = FakeGitHub(
            runs_pages=[[run]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1), provenance_artifact(id=2)]}},
            records_by_artifact_id={1: record, 2: record},
        )
        assert_raises("ambiguous provenance artifacts", lambda: resolve_with_fake(module, config, ambiguous))

        expired = FakeGitHub(
            runs_pages=[[run]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(expired=True)]}},
            records_by_artifact_id={123: record},
        )
        assert_raises("expired", lambda: resolve_with_fake(module, config, expired))

        multi_same_attempt = FakeGitHub(
            runs_pages=[[run, run_payload(id=RUN_ID + 1)]],
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [provenance_artifact(id=1)]},
                RUN_ID + 1: {"artifacts": [provenance_artifact(id=2, run_id=RUN_ID + 1)]},
            },
            records_by_artifact_id={1: record, 2: {**record, "run_id": RUN_ID + 1}},
        )
        assert_raises(
            "multiple provenance artifacts for attempt 1",
            lambda: resolve_with_fake(module, config, multi_same_attempt),
        )


def assert_artifact_page_saturation_fails_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        artifacts = [provenance_artifact(id=index, name=f"unrelated-{index}") for index in range(100)]
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={RUN_ID: {"artifacts": artifacts}},
        )
        assert_raises("artifacts page is saturated", lambda: resolve_with_fake(module, config, fake))


def assert_artifact_page_total_count_boundary_is_accepted() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        artifacts = [provenance_artifact(id=1)] + [
            provenance_artifact(id=index + 2, name=f"unrelated-{index}") for index in range(99)
        ]
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={RUN_ID: {"total_count": 100, "artifacts": artifacts}},
            records_by_artifact_id={1: record},
        )
        resolved = resolve_with_fake(module, config, fake)
        if resolved.artifact.get("id") != 1:
            raise AssertionError(f"expected provenance artifact 1, got {resolved.artifact}")


def assert_jobs_page_saturation_fails_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        jobs = required_job_payloads() + [
            job_payload(f"extra-{index}") for index in range(100 - len(required_job_payloads()))
        ]
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
            jobs_by_run_id={RUN_ID: {"jobs": jobs}},
        )
        assert_raises("jobs page is saturated", lambda: resolve_with_fake(module, config, fake))


def assert_jobs_page_total_count_boundary_is_accepted() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        jobs = required_job_payloads() + [
            job_payload(f"extra-{index}") for index in range(100 - len(required_job_payloads()))
        ]
        fake = FakeGitHub(
            runs_pages=[[run_payload()]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
            jobs_by_run_id={RUN_ID: {"total_count": 100, "jobs": jobs}},
        )
        resolved = resolve_with_fake(module, config, fake)
        if resolved.run.get("id") != RUN_ID:
            raise AssertionError(f"expected run {RUN_ID}, got {resolved.run}")


def assert_incomplete_exact_sha_runs_require_explicit_job_validated_opt_in() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        queued_run = run_payload(status="queued", conclusion=None)
        fake = FakeGitHub(
            runs_pages=[[queued_run]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
            jobs_by_run_id={RUN_ID: {"jobs": required_job_payloads()}},
        )
        assert_raises("no candidate provenance evidence", lambda: resolve_with_fake(module, config, fake))
        resolved = module.resolve_exact_sha_evidence(
            repo="seungpyoson/bolt-v2",
            token="token",
            requested_sha=SHA,
            config=module.load_config(config),
            config_path=config,
            api_json=fake.json,
            api_bytes=fake.bytes,
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            allow_incomplete_run_with_successful_jobs=True,
        )
        if resolved.run.get("id") != RUN_ID:
            raise AssertionError(f"expected queued run {RUN_ID}, got {resolved.run}")

        failed_job_fake = FakeGitHub(
            runs_pages=[[queued_run]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1)]}},
            records_by_artifact_id={1: record},
            jobs_by_run_id={
                RUN_ID: {"jobs": with_required_job_conclusion(required_job_payloads(), "source-fence", "failure")}
            },
        )
        assert_raises(
            "required job source-fence was 'completed'/'failure'",
            lambda: module.resolve_exact_sha_evidence(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                config=module.load_config(config),
                config_path=config,
                api_json=failed_job_fake.json,
                api_bytes=failed_job_fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
                allow_incomplete_run_with_successful_jobs=True,
            ),
        )


def assert_complete_first_page_rejects_incomplete_or_malformed_counts() -> None:
    module = load_script()
    full_page = [object() for _ in range(100)]
    assert_raises(
        "source run 1 jobs page is saturated",
        lambda: module.require_complete_first_page(
            {"total_count": 101},
            full_page,
            per_page=100,
            label="source run 1 jobs",
        ),
    )
    for total_count in ("100", True, 100.0, 99):
        assert_raises(
            "source run 1 jobs total_count is malformed",
            lambda total_count=total_count: module.require_complete_first_page(
                {"total_count": total_count},
                full_page,
                per_page=100,
                label="source run 1 jobs",
            ),
        )


def assert_latest_successful_attempt_selected() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        first = valid_record(module, config)
        second = {**first, "run_id": RUN_ID + 1, "run_attempt": 2}
        fake = FakeGitHub(
            runs_pages=[[run_payload(), run_payload(id=RUN_ID + 1, run_attempt=2)]],
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [provenance_artifact(id=1)]},
                RUN_ID + 1: {"artifacts": [provenance_artifact(id=2, run_id=RUN_ID + 1, run_attempt=2)]},
            },
            records_by_artifact_id={1: first, 2: second},
        )
        evidence = resolve_with_fake(module, config, fake)
        if evidence.record["run_attempt"] != 2:
            raise AssertionError(evidence)


def assert_record_attempt_mismatch_rejected() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        config = write_config(tmp_path)
        record = valid_record(module, config)
        record["run_attempt"] = 2
        fake = FakeGitHub(
            runs_pages=[[run_payload(run_attempt=1)]],
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact(id=1, run_attempt=1)]}},
            records_by_artifact_id={1: record},
        )
        assert_raises("record run_attempt", lambda: resolve_with_fake(module, config, fake))


def assert_malformed_api_payload_rejected() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        fake = FakeGitHub(runs_pages={"not": "a-list"})
        assert_raises("workflow runs payload is malformed", lambda: resolve_with_fake(module, config, fake))


def assert_job_evidence_success_passes() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        module.validate_job_evidence(
            {"jobs": required_job_payloads()},
            loaded,
            record,
            deploy_reuse_requested=True,
        )


def assert_nextest_archive_job_failures_rejected() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        missing_archive = [job for job in required_job_payloads() if job["name"] != "nextest archive"]
        assert_raises(
            "missing required job nextest archive",
            lambda: module.validate_job_evidence({"jobs": missing_archive}, loaded, record, deploy_reuse_requested=True),
        )
        failed_archive = with_required_job_conclusion(required_job_payloads(), "nextest archive", "failure")
        assert_raises(
            "nextest archive",
            lambda: module.validate_job_evidence({"jobs": failed_archive}, loaded, record, deploy_reuse_requested=True),
        )
        neutral_archive = with_required_job_conclusion(required_job_payloads(), "nextest archive", "neutral")
        assert_raises(
            "neutral",
            lambda: module.validate_job_evidence({"jobs": neutral_archive}, loaded, record, deploy_reuse_requested=True),
        )
        null_archive = with_required_job_conclusion(required_job_payloads(), "nextest archive", None)
        assert_raises(
            "None",
            lambda: module.validate_job_evidence({"jobs": null_archive}, loaded, record, deploy_reuse_requested=True),
        )


def assert_test_archive_and_build_rules() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        missing_archive = [job for job in required_job_payloads() if job["name"] != "nextest archive"]
        assert_raises(
            "missing required job nextest archive",
            lambda: module.validate_job_evidence({"jobs": missing_archive}, loaded, record, deploy_reuse_requested=True),
        )
        missing_build = [job for job in required_job_payloads() if job["name"] != "build"]
        assert_raises(
            "missing required job build",
            lambda: module.validate_job_evidence({"jobs": missing_build}, loaded, record, deploy_reuse_requested=True),
        )

        build_skipped_record = valid_record(module, config)
        build_skipped_record["conditional_jobs"] = {"build": {"required": False, "result": "skipped"}}
        module.validate_job_evidence(
            {"jobs": required_job_payloads(build_conclusion="skipped")},
            loaded,
            build_skipped_record,
            deploy_reuse_requested=False,
        )
        assert_raises(
            "deploy reuse requires build success",
            lambda: module.validate_job_evidence(
                {"jobs": required_job_payloads(build_conclusion="skipped")},
                loaded,
                build_skipped_record,
                deploy_reuse_requested=True,
            ),
        )


def pull_request_record(module, config_path: pathlib.Path, *, base_sha: str = "1" * 40) -> dict[str, object]:
    record = valid_record(module, config_path)
    record.update(
        {
            "event": "pull_request",
            "head_branch": "feature",
            "tested_sha": OTHER_SHA,
            "pull_request": {"number": 960, "base_sha": base_sha},
        }
    )
    return record


def assert_gate_carry_forward_requires_same_base_pr_provenance() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        record = pull_request_record(module, config, base_sha="1" * 40)
        prior_run = run_payload(
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
        )
        fake = FakeGitHub(
            runs_pages=[[prior_run]],
            jobs_by_run_id={RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]}},
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        result = module.resolve_gate_carry_forward(
            repo="seungpyoson/bolt-v2",
            token="token",
            requested_sha=SHA,
            base_sha="1" * 40,
            current_run_id=RUN_ID + 1,
            gate_name="gate",
            workflow_path=".github/workflows/ci.yml",
            config=module.load_config(config),
            config_path=config,
            require_provenance_base=True,
            api_json=fake.json,
            api_bytes=fake.bytes,
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
        )
        if result.source_run_id != str(RUN_ID) or not result.carry_forward_verified:
            raise AssertionError(result)

        assert_raises(
            "base_sha does not match current PR base",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="2" * 40,
                current_run_id=RUN_ID + 1,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_refuses_when_newest_same_sha_run_failed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        record = pull_request_record(module, config, base_sha="1" * 40)
        older_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        newer_failure = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            updated_at="2026-06-13T00:20:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[newer_failure, older_success]],
            jobs_by_run_id={RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]}},
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_refuses_when_newest_same_sha_run_in_progress() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        record = pull_request_record(module, config, base_sha="1" * 40)
        older_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        newer_running = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="in_progress",
            conclusion=None,
            updated_at="2026-06-13T00:20:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[newer_running, older_success]],
            jobs_by_run_id={RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]}},
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_uses_newest_success_with_provenance() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 1"),
        )
        record = pull_request_record(module, config, base_sha="1" * 40)
        older_provenance_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        intervening_older_failure = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            updated_at="2026-06-13T00:15:00Z",
        )
        newer_carry_forward_success = run_payload(
            id=RUN_ID + 2,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:20:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[newer_carry_forward_success], [intervening_older_failure, older_provenance_success]],
            jobs_by_run_id={
                RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]},
                RUN_ID + 2: {"jobs": [job_payload("gate")]},
            },
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        result = module.resolve_gate_carry_forward(
            repo="seungpyoson/bolt-v2",
            token="token",
            requested_sha=SHA,
            base_sha="1" * 40,
            current_run_id=RUN_ID + 3,
            gate_name="gate",
            workflow_path=".github/workflows/ci.yml",
            config=module.load_config(config),
            config_path=config,
            require_provenance_base=True,
            api_json=fake.json,
            api_bytes=fake.bytes,
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
        )
        if result.source_run_id != str(RUN_ID) or not result.carry_forward_verified:
            raise AssertionError(result)


def assert_gate_carry_forward_newest_failure_blocks_across_pages() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 1"),
        )
        record = pull_request_record(module, config, base_sha="1" * 40)
        older_provenance_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-13T00:00:00Z",
            updated_at="2026-06-13T00:10:00Z",
        )
        newer_updated_failure_on_later_page = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            created_at="2026-06-13T00:01:00Z",
            updated_at="2026-06-13T00:30:00Z",
        )
        newer_created_success_without_provenance = run_payload(
            id=RUN_ID + 2,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-13T00:02:00Z",
            updated_at="2026-06-13T00:20:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[
                [newer_created_success_without_provenance],
                [newer_updated_failure_on_later_page],
                [older_provenance_success],
            ],
            jobs_by_run_id={
                RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]},
                RUN_ID + 2: {"jobs": [*required_job_payloads(), job_payload("gate")]},
            },
            artifacts_by_run_id={
                RUN_ID: {"artifacts": [provenance_artifact(run_id=RUN_ID)]},
                RUN_ID + 2: {"artifacts": []},
            },
            records_by_artifact_id={123: record},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 3,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:40:00Z"),
            ),
        )


def assert_gate_carry_forward_refuses_page_cap_without_natural_boundary() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 1").replace(
                "max_lookback_pages = 10", "max_lookback_pages = 2"
            ),
        )
        same_sha_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        full_page_filler = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/other.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:11:00Z",
        )
        hidden_same_sha_failure = run_payload(
            id=RUN_ID + 2,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            updated_at="2026-06-13T00:12:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[same_sha_success], [full_page_filler], [hidden_same_sha_failure]],
            jobs_by_run_id={RUN_ID: {"jobs": [job_payload("gate")]}},
        )
        assert_raises(
            "lookback page limit exhausted before candidate evidence was found",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 3,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=False,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_blocks_equal_updated_at_failure_tie() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        success = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        tied_failure = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            updated_at="2026-06-13T00:10:00Z",
        )
        newer_failure = dict(tied_failure)
        newer_failure["updated_at"] = "2026-06-13T00:10:01Z"

        control = FakeGitHub(
            runs_pages=[[success, newer_failure]],
            jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=False,
                api_json=control.json,
                api_bytes=control.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )

        tied = FakeGitHub(
            runs_pages=[[success, tied_failure]],
            jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=False,
                api_json=tied.json,
                api_bytes=tied.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_blocks_equal_updated_at_non_completed_tie(status: str) -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        success = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:10:00Z",
        )
        tied_non_completed = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status=status,
            conclusion=None,
            updated_at="2026-06-13T00:10:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[success, tied_non_completed]],
            jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=False,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-13T00:30:00Z"),
            ),
        )


def assert_gate_carry_forward_blocks_equal_updated_at_in_progress_tie() -> None:
    assert_gate_carry_forward_blocks_equal_updated_at_non_completed_tie("in_progress")


def assert_gate_carry_forward_blocks_equal_updated_at_queued_tie() -> None:
    assert_gate_carry_forward_blocks_equal_updated_at_non_completed_tie("queued")


def assert_gate_carry_forward_blocks_equal_updated_at_waiting_tie() -> None:
    assert_gate_carry_forward_blocks_equal_updated_at_non_completed_tie("waiting")


def assert_gate_carry_forward_blocks_old_created_equal_updated_at_non_completed_ties() -> None:
    module = load_script()
    for status in ("in_progress", "queued", "waiting"):
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp))
            success = run_payload(
                id=RUN_ID + 1,
                event="pull_request",
                head_branch="feature",
                head_sha=SHA,
                path=".github/workflows/ci.yml",
                status="completed",
                conclusion="success",
                created_at="2026-06-13T00:00:00Z",
                updated_at="2026-06-29T00:00:00Z",
            )
            old_non_completed = run_payload(
                id=RUN_ID,
                event="pull_request",
                head_branch="feature",
                head_sha=SHA,
                path=".github/workflows/ci.yml",
                status=status,
                conclusion=None,
                created_at="2026-05-01T00:00:00Z",
                updated_at="2026-06-29T00:00:00Z",
            )
            fake = FakeGitHub(
                runs_pages=[[success, old_non_completed]],
                jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
            )
            assert_raises(
                "newest same-SHA carry-forward run",
                lambda: module.resolve_gate_carry_forward(
                    repo="seungpyoson/bolt-v2",
                    token="token",
                    requested_sha=SHA,
                    base_sha="1" * 40,
                    current_run_id=RUN_ID + 2,
                    gate_name="gate",
                    workflow_path=".github/workflows/ci.yml",
                    config=module.load_config(config),
                    config_path=config,
                    require_provenance_base=False,
                    api_json=fake.json,
                    api_bytes=fake.bytes,
                    now=module.parse_timestamp("2026-06-30T00:00:00Z"),
                ),
            )


def assert_gate_carry_forward_blocks_old_created_strictly_newer_in_progress() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        success = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-13T00:00:00Z",
            updated_at="2026-06-29T00:00:00Z",
        )
        old_in_progress = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="in_progress",
            conclusion=None,
            created_at="2026-05-01T00:00:00Z",
            updated_at="2026-06-29T00:00:01Z",
        )
        fake = FakeGitHub(
            runs_pages=[[success, old_in_progress]],
            jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=False,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-30T00:00:00Z"),
            ),
        )


def assert_gate_carry_forward_newer_success_supersedes_older_failure() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        older_failure = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            updated_at="2026-06-13T00:10:00Z",
        )
        newer_success = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            updated_at="2026-06-13T00:20:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[newer_success, older_failure]],
            jobs_by_run_id={RUN_ID + 1: {"jobs": [job_payload("gate")]}},
        )
        result = module.resolve_gate_carry_forward(
            repo="seungpyoson/bolt-v2",
            token="token",
            requested_sha=SHA,
            base_sha="1" * 40,
            current_run_id=RUN_ID + 2,
            gate_name="gate",
            workflow_path=".github/workflows/ci.yml",
            config=module.load_config(config),
            config_path=config,
            require_provenance_base=False,
            api_json=fake.json,
            api_bytes=fake.bytes,
            now=module.parse_timestamp("2026-06-13T00:30:00Z"),
        )
        if result.source_run_id != str(RUN_ID + 1) or not result.carry_forward_verified:
            raise AssertionError(result)


def assert_gate_carry_forward_blocks_failure_hidden_behind_all_old_page() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(
            pathlib.Path(tmp),
            CONFIG_TOML.replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 2"),
        )
        record = pull_request_record(module, config, base_sha="1" * 40)
        fresh_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-13T00:00:00Z",
            updated_at="2026-06-13T00:10:00Z",
        )
        fresh_filler = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/other.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-13T00:01:00Z",
            updated_at="2026-06-13T00:11:00Z",
        )
        old_success_a = run_payload(
            id=RUN_ID + 2,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-05-01T00:00:00Z",
            updated_at="2026-05-01T00:00:00Z",
        )
        old_success_b = run_payload(
            id=RUN_ID + 3,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-05-01T00:01:00Z",
            updated_at="2026-05-01T00:01:00Z",
        )
        old_failure_rerun_after_old_page = run_payload(
            id=RUN_ID + 4,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            created_at="2026-05-01T00:02:00Z",
            updated_at="2026-06-29T00:00:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[
                [fresh_success, fresh_filler],
                [old_success_a, old_success_b],
                [old_failure_rerun_after_old_page],
            ],
            jobs_by_run_id={RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]}},
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 5,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-30T00:00:00Z"),
            ),
        )


def base_ci_gate_jobs(**overrides: str) -> dict[str, str]:
    jobs = {
        "ci-policy": "success",
        "detector": "success",
        "deny": "success",
        "clippy": "success",
        "check-aarch64": "success",
        "source-fence": "success",
        "nextest-fingerprint": "success",
        "test-archive": "success",
        "nextest-fingerprint-reuse": "skipped",
        "test": "success",
        "build": "success",
        "ci-provenance-emit": "success",
        "same-sha-main-evidence": "skipped",
    }
    jobs.update(overrides)
    return jobs


def assert_gate_carry_forward_blocks_old_failure_rerun_after_cutoff() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        record = pull_request_record(module, config, base_sha="1" * 40)
        within_window_success = run_payload(
            id=RUN_ID,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="success",
            created_at="2026-06-10T00:00:00Z",
            updated_at="2026-06-10T00:00:00Z",
        )
        # Original run for this SHA: created before the lookback window opened
        # but re-run to failure recently, so it is the newest activity by
        # updated_at. Its failure must still dominate the within-window success.
        old_failure_rerun = run_payload(
            id=RUN_ID + 1,
            event="pull_request",
            head_branch="feature",
            head_sha=SHA,
            path=".github/workflows/ci.yml",
            status="completed",
            conclusion="failure",
            created_at="2026-05-01T00:00:00Z",
            updated_at="2026-06-29T00:00:00Z",
        )
        fake = FakeGitHub(
            runs_pages=[[within_window_success, old_failure_rerun]],
            jobs_by_run_id={RUN_ID: {"jobs": [*required_job_payloads(), job_payload("gate")]}},
            artifacts_by_run_id={RUN_ID: {"artifacts": [provenance_artifact()]}},
            records_by_artifact_id={123: record},
        )
        assert_raises(
            "newest same-SHA carry-forward run",
            lambda: module.resolve_gate_carry_forward(
                repo="seungpyoson/bolt-v2",
                token="token",
                requested_sha=SHA,
                base_sha="1" * 40,
                current_run_id=RUN_ID + 2,
                gate_name="gate",
                workflow_path=".github/workflows/ci.yml",
                config=module.load_config(config),
                config_path=config,
                require_provenance_base=True,
                api_json=fake.json,
                api_bytes=fake.bytes,
                now=module.parse_timestamp("2026-06-30T00:00:00Z"),
            ),
        )


def assert_ci_gate_verdict_requires_real_docs_or_carry_forward_proof() -> None:
    module = load_script()
    skipped_heavy = base_ci_gate_jobs(
        deny="skipped",
        clippy="skipped",
        **{
            "check-aarch64": "skipped",
            "source-fence": "skipped",
            "nextest-fingerprint": "skipped",
            "test-archive": "skipped",
            "nextest-fingerprint-reuse": "skipped",
            "test": "skipped",
            "build": "skipped",
        },
    )
    assert_raises(
        "verified carry-forward",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="noop",
            expected_event_class="noop",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results={**skipped_heavy, "ci-provenance-emit": "skipped"},
            build_required=False,
        ),
    )
    module.evaluate_ci_gate_verdict(
        policy_path="noop",
        expected_event_class="noop",
        full_ci_deferred=False,
        ignore_emit_failure=False,
        reuse_found=False,
        carry_forward_verified=True,
        job_results={**skipped_heavy, "ci-provenance-emit": "skipped"},
        build_required=False,
    )
    module.evaluate_ci_gate_verdict(
        policy_path="docs",
        expected_event_class="docs",
        full_ci_deferred=False,
        ignore_emit_failure=False,
        reuse_found=False,
        carry_forward_verified=False,
        job_results=skipped_heavy,
        build_required=False,
    )
    assert_raises(
        "clippy unexpectedly ran during docs",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="docs",
            expected_event_class="docs",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results={**skipped_heavy, "clippy": "failure"},
            build_required=False,
        ),
    )


def assert_ci_gate_verdict_hardens_full_and_reuse_proof() -> None:
    module = load_script()
    full_jobs = base_ci_gate_jobs()
    module.evaluate_ci_gate_verdict(
        policy_path="full",
        expected_event_class="full",
        full_ci_deferred=False,
        ignore_emit_failure=False,
        reuse_found=False,
        carry_forward_verified=False,
        job_results=full_jobs,
        build_required=True,
    )
    assert_raises(
        "test-archive did not succeed",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="full",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results={**full_jobs, "test-archive": "skipped"},
            build_required=True,
        ),
    )
    reuse_jobs = base_ci_gate_jobs(
        **{
            "test-archive": "skipped",
            "nextest-fingerprint-reuse": "success",
            "ci-provenance-emit": "skipped",
        }
    )
    module.evaluate_ci_gate_verdict(
        policy_path="full",
        expected_event_class="full",
        full_ci_deferred=False,
        ignore_emit_failure=False,
        reuse_found=True,
        carry_forward_verified=False,
        job_results=reuse_jobs,
        build_required=True,
    )
    assert_raises(
        "nextest fingerprint did not succeed during reuse",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="full",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=True,
            carry_forward_verified=False,
            job_results={**reuse_jobs, "nextest-fingerprint": "failure"},
            build_required=True,
        ),
    )
    assert_raises(
        "test-archive unexpectedly ran during nextest fingerprint reuse",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="full",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=True,
            carry_forward_verified=False,
            job_results={**reuse_jobs, "test-archive": "success"},
            build_required=True,
        ),
    )
    assert_raises(
        "ignore_emit_failure cannot satisfy the required gate",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="full",
            full_ci_deferred=False,
            ignore_emit_failure=True,
            reuse_found=False,
            carry_forward_verified=False,
            job_results={**full_jobs, "ci-provenance-emit": "failure"},
            build_required=True,
        ),
    )
    assert_raises(
        "full CI policy outside resolver-permitted event class",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="iteration",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results=full_jobs,
            build_required=True,
        ),
    )
    assert_raises(
        "full_ci_deferred must match policy_path",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="full",
            expected_event_class="full",
            full_ci_deferred=True,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results=full_jobs,
            build_required=True,
        ),
    )
    tag_jobs = base_ci_gate_jobs(
        deny="skipped",
        clippy="skipped",
        **{
            "source-fence": "skipped",
            "nextest-fingerprint": "skipped",
            "test-archive": "skipped",
            "nextest-fingerprint-reuse": "skipped",
            "test": "skipped",
            "build": "skipped",
            "ci-provenance-emit": "skipped",
            "same-sha-main-evidence": "success",
        },
    )
    assert_raises(
        "tag reuse CI policy outside resolver-permitted event class",
        lambda: module.evaluate_ci_gate_verdict(
            policy_path="tag_reuse",
            expected_event_class="full",
            full_ci_deferred=False,
            ignore_emit_failure=False,
            reuse_found=False,
            carry_forward_verified=False,
            job_results=tag_jobs,
            build_required=False,
        ),
    )
    assert_raises(
        "duplicate --job result for test",
        lambda: module.parse_job_result_values(["test=success", "test=failure"]),
    )
    assert_raises(
        "missing-lane missing or not skipped during test path",
        lambda: module.require_jobs_skipped({}, ("missing-lane",), "test path"),
    )


def assert_backtester_gate_verdict_recomputes_noop_and_defer_for_crate_changes() -> None:
    module = load_script()
    skipped_jobs = {
        "ci-policy": "success",
        "detect": "success",
        "fmt": "skipped",
        "clippy": "skipped",
        "test-archive": "skipped",
    }
    module.evaluate_backtester_gate_verdict(
        policy_path="full",
        expected_event_class="full",
        full_ci_deferred=False,
        job_results=skipped_jobs,
        bvs_changed=False,
    )
    module.evaluate_backtester_gate_verdict(
        policy_path="defer",
        expected_event_class="defer",
        full_ci_deferred=True,
        job_results=skipped_jobs,
        bvs_changed=False,
    )
    module.evaluate_backtester_gate_verdict(
        policy_path="noop",
        expected_event_class="noop",
        full_ci_deferred=False,
        job_results=skipped_jobs,
        bvs_changed=False,
    )
    assert_raises(
        "clippy unexpectedly ran during backtester no-crate",
        lambda: module.evaluate_backtester_gate_verdict(
            policy_path="noop",
            expected_event_class="noop",
            full_ci_deferred=False,
            job_results={**skipped_jobs, "clippy": "success"},
            bvs_changed=False,
        ),
    )

    proof_jobs = {
        "ci-policy": "success",
        "detect": "success",
        "fmt": "success",
        "clippy": "success",
        "test-archive": "success",
    }
    module.evaluate_backtester_gate_verdict(
        policy_path="defer",
        expected_event_class="defer",
        full_ci_deferred=True,
        job_results=proof_jobs,
        bvs_changed=True,
    )
    module.evaluate_backtester_gate_verdict(
        policy_path="noop",
        expected_event_class="noop",
        full_ci_deferred=False,
        job_results=proof_jobs,
        bvs_changed=True,
    )
    assert_raises(
        "bvs-clippy did not succeed",
        lambda: module.evaluate_backtester_gate_verdict(
            policy_path="defer",
            expected_event_class="defer",
            full_ci_deferred=True,
            job_results={**proof_jobs, "clippy": "skipped"},
            bvs_changed=True,
        ),
    )


def main() -> int:
    assert_unknown_mode_fails()
    assert_missing_config_table_fails()
    assert_positive_int_config_rejects_booleans()
    assert_deploy_artifact_window_uses_short_deploy_policy()
    assert_capture_config_can_omit_deploy_artifact_window()
    assert_optional_deploy_window_rejects_partial_config()
    assert_emit_full_ci_records_nextest_fingerprint_argument()
    assert_emit_full_ci_hashes_explicit_tested_workflow()
    assert_emit_docs_ci_record_requires_skipped_heavy_jobs()
    assert_unknown_record_schema_fails()
    assert_fingerprint_reuse_prior_green_returns_reuse()
    assert_fingerprint_reuse_no_prior_run_returns_no_reuse()
    assert_fingerprint_reuse_rejects_failed_cancelled_and_wrong_workflow_runs()
    assert_fingerprint_reuse_rejects_ambiguous_and_expired_artifacts()
    assert_fingerprint_reuse_requires_exact_fingerprint_components()
    assert_fingerprint_reuse_rejects_source_record_workflow_digest_mismatch()
    assert_fingerprint_reuse_allows_unrelated_workflow_drift()
    assert_fingerprint_reuse_allows_deploy_only_env_drift()
    assert_workflow_reuse_scope_digest_accepts_yaml_header_formatting()
    assert_workflow_reuse_scope_digest_distinguishes_quoted_hash_values()
    assert_workflow_reuse_scope_digest_rejects_multiline_scoped_env()
    assert_workflow_reuse_scope_digest_rejects_folded_scoped_env()
    assert_workflow_reuse_scope_digest_rejects_alias_scoped_env()
    assert_workflow_reuse_scope_digest_ignores_nested_env_decoys()
    assert_workflow_reuse_scope_digest_preserves_block_scalar_content()
    assert_workflow_reuse_scope_digest_preserves_indicated_block_scalar_content()
    assert_workflow_reuse_scope_digest_preserves_block_scalar_trailing_spaces()
    assert_fingerprint_reuse_rejects_reuse_relevant_workflow_drift()
    assert_fingerprint_reuse_malformed_fingerprint_fails_closed()
    assert_fingerprint_reuse_rejects_failed_source_archive_through_resolver()
    assert_fingerprint_reuse_source_run_must_be_trusted_main_push()
    assert_missing_current_fingerprint_arg_fails_closed()
    assert_nextest_fingerprint_path_args_are_rejected()
    assert_fingerprint_reuse_api_errors_fail_closed()
    assert_fingerprint_reuse_selects_newest_valid_prior_green()
    assert_top_level_help_is_supported()
    assert_artifact_metadata_outputs_configured_name_only()
    assert_artifact_metadata_accepts_capture_config_without_workflows()
    assert_ci_policy_rejects_event_sender_cli_override_arguments()
    assert_ci_policy_outputs_matrix()
    assert_ci_policy_gate_names_are_event_based()
    assert_required_gate_proof_event_classes_match_resolver()
    assert_mergify_temp_pr_requires_actor_binding()
    assert_mergify_temp_pr_ready_event_uses_author_binding()
    assert_mergify_temp_pr_synchronize_requires_sender_binding()
    assert_mergify_temp_pr_edited_event_splits_metadata_from_base_change()
    assert_parse_event_sender_id_fails_closed()
    assert_ci_policy_non_numeric_sender_id_does_not_crash()
    assert_mergify_actor_binding_demotes_every_full_action()
    assert_ready_pr_defers_proof_to_merge_boundary_without_required_gate()
    assert_dispatch_run_names_come_from_config()
    assert_gate_names_reject_github_output_control_chars()
    assert_gate_names_reject_collisions()
    assert_required_checks_registry_matches_sources()
    assert_required_checks_registry_rejects_unreachable_proof_rules()
    assert_required_checks_registry_is_closed()
    assert_required_checks_registry_rejects_arrival_and_target_drift()
    assert_required_checks_registry_rejects_drift()
    assert_policy_contract_rejects_required_gate_holes()
    assert_main_evidence_matching_ignores_mutable_run_name()
    assert_config_digest_is_canonical()
    assert_github_api_bytes_strips_authorization_on_cross_host_redirect()
    assert_sensitive_headers_only_survive_same_https_origin_redirects()
    assert_github_api_json_rejects_invalid_utf8_as_provenance_error()
    assert_artifact_record_rejects_invalid_utf8_json()
    assert_record_schema_requires_head_and_tested_sha()
    assert_pr_event_record_cannot_validate_for_pr_head_reuse()
    assert_digest_mismatches_fail()
    assert_no_candidate_evidence_fails()
    assert_lookback_exhaustion_fails()
    assert_lookback_age_exhaustion_fails()
    assert_lookback_age_does_not_stop_same_page_scan()
    assert_missing_created_at_fails_closed()
    assert_artifact_rejections()
    assert_artifact_page_saturation_fails_closed()
    assert_artifact_page_total_count_boundary_is_accepted()
    assert_jobs_page_saturation_fails_closed()
    assert_jobs_page_total_count_boundary_is_accepted()
    assert_incomplete_exact_sha_runs_require_explicit_job_validated_opt_in()
    assert_complete_first_page_rejects_incomplete_or_malformed_counts()
    assert_latest_successful_attempt_selected()
    assert_record_attempt_mismatch_rejected()
    assert_malformed_api_payload_rejected()
    assert_job_evidence_success_passes()
    assert_nextest_archive_job_failures_rejected()
    assert_deploy_evidence_uses_deploy_artifact_lookback()
    assert_test_archive_and_build_rules()
    assert_gate_carry_forward_requires_same_base_pr_provenance()
    assert_gate_carry_forward_refuses_when_newest_same_sha_run_failed()
    assert_gate_carry_forward_refuses_when_newest_same_sha_run_in_progress()
    assert_gate_carry_forward_uses_newest_success_with_provenance()
    assert_gate_carry_forward_newest_failure_blocks_across_pages()
    assert_gate_carry_forward_refuses_page_cap_without_natural_boundary()
    assert_gate_carry_forward_blocks_equal_updated_at_failure_tie()
    assert_gate_carry_forward_blocks_equal_updated_at_in_progress_tie()
    assert_gate_carry_forward_blocks_equal_updated_at_queued_tie()
    assert_gate_carry_forward_blocks_equal_updated_at_waiting_tie()
    assert_gate_carry_forward_blocks_old_created_equal_updated_at_non_completed_ties()
    assert_gate_carry_forward_blocks_old_created_strictly_newer_in_progress()
    assert_gate_carry_forward_newer_success_supersedes_older_failure()
    assert_gate_carry_forward_blocks_failure_hidden_behind_all_old_page()
    assert_gate_carry_forward_blocks_old_failure_rerun_after_cutoff()
    assert_ci_gate_verdict_requires_real_docs_or_carry_forward_proof()
    assert_ci_gate_verdict_hardens_full_and_reuse_proof()
    assert_backtester_gate_verdict_recomputes_noop_and_defer_for_crate_changes()
    print("OK: CI provenance self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
