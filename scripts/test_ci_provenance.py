#!/usr/bin/env python3
"""Self-tests for CI provenance emission and resolution."""

from __future__ import annotations

import contextlib
import http.server
import importlib.util
import io
import hashlib
import json
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
RUN_ID = 24623219988
CHECK_SUITE_ID = 65233803543

CONFIG_TOML = """
schema_version = 1

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
  "fmt-check",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "test-archive",
  "test-shards",
  "test",
]
conditional_jobs = ["build"]
conditional_job_outputs = { build = "detector.build_required" }

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci.jobs.fmt-check]
check_name = "fmt-check"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.test-shards]
check_name_template = "nextest shard {shard} of {shard_count}"
shard_count = 4

[ci_provenance.full_ci.jobs.test]
check_name = "test"

[ci_provenance.full_ci.jobs.build]
check_name = "build"
conditional = "detector.build_required"

[ci_provenance.deploy]
artifact_name = "bolt-v2-binary"
require_source_event = "push"
require_source_branch = "main"
require_gate_check = true

[ci_provenance.dispatch]
workflow_input = "full_ci"

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000

[ci_provenance.artifacts]
retention_days = 30

[ci_provenance.policy]
draft_pr_synchronize = "defer"
draft_pr_opened = "defer"
draft_pr_reopened = "defer"
converted_to_draft = "defer"
ready_pr = "full"
ready_for_review = "full"
workflow_dispatch = "full"
main_push = "full"
tag = "tag_reuse"
unknown_event = "full"

[ci_provenance.policy.override]
force_full_ci = false
ignore_emit_failure = false
"""

CONFIG_TOML_REORDERED = """
schema_version = 1

[unrelated]
value = "kept out of the provenance digest"

[ci_provenance.policy.override]
ignore_emit_failure = false
force_full_ci = false

[ci_provenance.policy]
unknown_event = "full"
tag = "tag_reuse"
main_push = "full"
workflow_dispatch = "full"
ready_for_review = "full"
ready_pr = "full"
converted_to_draft = "defer"
draft_pr_reopened = "defer"
draft_pr_opened = "defer"
draft_pr_synchronize = "defer"

[ci_provenance.artifacts]
retention_days = 30

[ci_provenance.api_limits]
max_lookback_age_seconds = 2592000
max_lookback_pages = 10
run_artifacts_per_page = 100
run_jobs_per_page = 100
workflow_runs_per_page = 100

[ci_provenance.dispatch]
workflow_input = "full_ci"

[ci_provenance.deploy]
require_gate_check = true
require_source_branch = "main"
require_source_event = "push"
artifact_name = "bolt-v2-binary"

[ci_provenance.full_ci.jobs.build]
conditional = "detector.build_required"
check_name = "build"

[ci_provenance.full_ci.jobs.test]
check_name = "test"

[ci_provenance.full_ci.jobs.test-shards]
shard_count = 4
check_name_template = "nextest shard {shard} of {shard_count}"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.fmt-check]
check_name = "fmt-check"

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci]
conditional_job_outputs = { build = "detector.build_required" }
conditional_jobs = ["build"]
required_jobs = [
  "detector",
  "fmt-check",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "test-archive",
  "test-shards",
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
        code = module.main(args)
    return code, stdout.getvalue(), stderr.getvalue()


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
            "fmt-check": "success",
            "deny": "success",
            "clippy": "success",
            "check-aarch64": "success",
            "source-fence": "success",
            "test-archive": "success",
            "test-shards": "success",
            "test": "success",
        },
        "conditional_jobs": {"build": {"required": True, "result": "success"}},
        "nextest_fingerprint": None,
        "created_at": "2026-06-13T00:00:00Z",
    }


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


def job_payload(name: str, conclusion: object = "success", status: object = "completed") -> dict[str, object]:
    return {"name": name, "status": status, "conclusion": conclusion}


def required_job_payloads(build_conclusion: object = "success") -> list[dict[str, object]]:
    return [
        job_payload("detector"),
        job_payload("fmt-check"),
        job_payload("deny"),
        job_payload("clippy"),
        job_payload("check-aarch64"),
        job_payload("source-fence"),
        job_payload("nextest archive"),
        job_payload("nextest shard 1 of 4"),
        job_payload("nextest shard 2 of 4"),
        job_payload("nextest shard 3 of 4"),
        job_payload("nextest shard 4 of 4"),
        job_payload("test"),
        job_payload("build", conclusion=build_conclusion),
    ]


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
    ) -> None:
        self.runs_pages = runs_pages
        self.jobs_by_run_id = jobs_by_run_id or {}
        self.artifacts_by_run_id = artifacts_by_run_id or {}
        self.records_by_artifact_id = records_by_artifact_id or {}

    def json(
        self,
        repo: str,
        token: str,
        path: str,
        query: dict[str, str] | None = None,
    ) -> dict[str, object]:
        if path == "actions/runs":
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
            return (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()
        raise AssertionError(f"unexpected bytes request {url}")


def assert_unknown_mode_fails() -> None:
    assert_fails("unknown mode", ["not-a-mode"])


def assert_missing_config_table_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), strip_ci_provenance_config(CONFIG_TOML))
        assert_fails("missing [ci_provenance]", ["emit-full-ci", "--config", str(config)])


def assert_invalid_shard_count_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML.replace("shard_count = 4", "shard_count = 0"))
        assert_fails("shard_count", ["emit-full-ci", "--config", str(config)])


def assert_literal_shard_count_template_loads() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config_text = CONFIG_TOML.replace(
            'check_name_template = "nextest shard {shard} of {shard_count}"',
            'check_name_template = "nextest shard {shard} of 4"',
        )
        config = write_config(pathlib.Path(tmp), config_text)
        module = load_script()
        loaded = module.load_config(config)
        names = module.expanded_check_names(loaded, "test-shards")
        expected = (
            "nextest shard 1 of 4",
            "nextest shard 2 of 4",
            "nextest shard 3 of 4",
            "nextest shard 4 of 4",
        )
        if names != expected:
            raise AssertionError(f"unexpected expanded shard names: {names}")


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


def assert_resolve_fingerprint_is_rejected() -> None:
    assert_fails("resolve-fingerprint is not supported", ["resolve-fingerprint"])


def assert_top_level_help_is_supported() -> None:
    code, stdout, stderr = run_cli(["--help"])
    if code != 2:
        raise AssertionError(f"expected help to exit 2, got {code}")
    combined = stdout + stderr
    if "Usage: ci_provenance.py <mode> [options]" not in combined:
        raise AssertionError(f"expected top-level usage output, got {combined!r}")
    if "resolve-exact-sha" not in combined:
        raise AssertionError(f"expected supported modes in help output, got {combined!r}")


def assert_ci_policy_outputs_matrix() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), CONFIG_TOML)
        cases = [
            ("push", "", "false", "refs/heads/main", "full"),
            ("push", "", "false", "refs/tags/v1.2.3", "tag_reuse"),
            ("pull_request", "opened", "true", "refs/pull/1/merge", "defer"),
            ("pull_request", "synchronize", "true", "refs/pull/1/merge", "defer"),
            ("pull_request", "reopened", "true", "refs/pull/1/merge", "defer"),
            ("pull_request", "converted_to_draft", "true", "refs/pull/1/merge", "defer"),
            ("pull_request", "opened", "false", "refs/pull/1/merge", "full"),
            ("pull_request", "ready_for_review", "true", "refs/pull/1/merge", "full"),
            ("workflow_dispatch", "", "true", "refs/heads/codex/branch", "full"),
            ("unknown_event", "", "true", "refs/heads/codex/branch", "full"),
        ]
        for event_name, action, draft, ref, expected in cases:
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
            if output.get("full_ci_deferred") != str(expected == "defer").lower():
                raise AssertionError(f"full_ci_deferred must derive from {expected}: {output}")
            if not output.get("reason"):
                raise AssertionError(f"ci-policy must include reason: {output}")
            if output.get("ignore_emit_failure") != "false":
                raise AssertionError(f"ci-policy must expose ignore_emit_failure: {output}")

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
                "--ref",
                "refs/pull/1/merge",
            ]
        )
        if code != 0:
            raise AssertionError(f"force_full_ci ci-policy failed: {stderr}")
        output = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
        if output.get("ci_policy_path") != "full":
            raise AssertionError(f"force_full_ci must force draft PR events to full, got {output}")


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
            CONFIG_TOML.replace("max_lookback_age_seconds = 2592000", "max_lookback_age_seconds = 1"),
        )
        fake = FakeGitHub(runs_pages=[[run_payload(created_at="2020-01-01T00:00:00Z")]])
        assert_raises("lookback age limit exhausted", lambda: resolve_with_fake(module, config, fake))


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


def assert_shard_job_failures_rejected() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        loaded = module.load_config(config)
        record = valid_record(module, config)
        missing_shard = [job for job in required_job_payloads() if job["name"] != "nextest shard 4 of 4"]
        assert_raises(
            "missing required job nextest shard 4 of 4",
            lambda: module.validate_job_evidence({"jobs": missing_shard}, loaded, record, deploy_reuse_requested=True),
        )
        failed_shard = required_job_payloads()
        failed_shard[8] = job_payload("nextest shard 2 of 4", "failure")
        assert_raises(
            "nextest shard 2 of 4",
            lambda: module.validate_job_evidence({"jobs": failed_shard}, loaded, record, deploy_reuse_requested=True),
        )
        neutral_shard = required_job_payloads()
        neutral_shard[8] = job_payload("nextest shard 2 of 4", "neutral")
        assert_raises(
            "neutral",
            lambda: module.validate_job_evidence({"jobs": neutral_shard}, loaded, record, deploy_reuse_requested=True),
        )
        null_shard = required_job_payloads()
        null_shard[8] = job_payload("nextest shard 2 of 4", None)
        assert_raises(
            "None",
            lambda: module.validate_job_evidence({"jobs": null_shard}, loaded, record, deploy_reuse_requested=True),
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


def assert_directory_nextest_fingerprint_is_ignored() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        directory = pathlib.Path(tmp) / "cache-key.txt"
        directory.mkdir()
        if module.read_nextest_fingerprint(directory) is not None:
            raise AssertionError("directory fingerprint path should be ignored")


def main() -> int:
    assert_unknown_mode_fails()
    assert_missing_config_table_fails()
    assert_invalid_shard_count_fails()
    assert_literal_shard_count_template_loads()
    assert_unknown_record_schema_fails()
    assert_resolve_fingerprint_is_rejected()
    assert_top_level_help_is_supported()
    assert_ci_policy_outputs_matrix()
    assert_config_digest_is_canonical()
    assert_github_api_bytes_strips_authorization_on_cross_host_redirect()
    assert_record_schema_requires_head_and_tested_sha()
    assert_pr_event_record_cannot_validate_for_pr_head_reuse()
    assert_digest_mismatches_fail()
    assert_no_candidate_evidence_fails()
    assert_lookback_exhaustion_fails()
    assert_lookback_age_exhaustion_fails()
    assert_artifact_rejections()
    assert_latest_successful_attempt_selected()
    assert_record_attempt_mismatch_rejected()
    assert_malformed_api_payload_rejected()
    assert_job_evidence_success_passes()
    assert_shard_job_failures_rejected()
    assert_test_archive_and_build_rules()
    assert_directory_nextest_fingerprint_is_ignored()
    print("OK: CI provenance self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
