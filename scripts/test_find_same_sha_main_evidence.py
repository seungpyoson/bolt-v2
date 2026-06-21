#!/usr/bin/env python3
"""Self-tests for same-SHA main-run deploy evidence selection."""

from __future__ import annotations

import importlib.util
import hashlib
import io
import json
import pathlib
import sys
import tempfile
import zipfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "find_same_sha_main_evidence.py"
PROVENANCE_PATH = REPO_ROOT / "scripts" / "ci_provenance.py"
CONFIG_PATH = REPO_ROOT / "ci" / "github-actions-runners.toml"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"
RUN_ID = 24623219988
CHECK_SUITE_ID = 65233803543


def load_script(path: pathlib.Path = SCRIPT_PATH, module_name: str = "find_same_sha_main_evidence"):
    if not path.exists():
        raise AssertionError(f"missing script: {path}")
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load {path.name}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_provenance():
    return load_script(PROVENANCE_PATH, "ci_provenance")


def workflow_digest() -> str:
    return hashlib.sha256((REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()).hexdigest()


def record_payload(**overrides: object) -> dict[str, object]:
    provenance = load_provenance()
    payload: dict[str, object] = {
        "schema_version": 1,
        "kind": "full-ci",
        "repository": "seungpyoson/bolt-v2",
        "workflow_path": ".github/workflows/ci.yml",
        "workflow_digest": workflow_digest(),
        "provenance_config_digest": provenance.provenance_config_digest(CONFIG_PATH),
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
    payload.update(overrides)
    return payload


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


def job(name: str, conclusion: object = "success") -> dict[str, object]:
    return {"name": name, "status": "completed", "conclusion": conclusion}


def jobs(gate_conclusion: object = "success") -> list[dict[str, object]]:
    return [
        job("detector"),
        job("deny"),
        job("clippy"),
        job("check-aarch64"),
        job("source-fence"),
        job("nextest fingerprint"),
        job("nextest archive"),
        job("test"),
        job("build"),
        job("gate", gate_conclusion),
    ]


def provenance_artifact(**overrides: object) -> dict[str, object]:
    artifact_id = overrides.get("id", 123)
    payload: dict[str, object] = {
        "id": artifact_id,
        "name": "ci-provenance-attempt-1",
        "expired": False,
        "archive_download_url": f"artifact://{artifact_id}",
        "workflow_run": {
            "id": RUN_ID,
            "head_branch": "main",
            "head_sha": SHA,
        },
    }
    payload.update(overrides)
    return payload


def deploy_artifact(**overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "id": 6516430716,
        "name": "bolt-v2-binary",
        "expired": False,
        "size_in_bytes": 12631205,
        "workflow_run": {
            "id": RUN_ID,
            "head_branch": "main",
            "head_sha": SHA,
        },
    }
    payload.update(overrides)
    return payload


def artifact_zip(record: dict[str, object]) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("ci-provenance.json", json.dumps(record))
    return buffer.getvalue()


class FakeGitHub:
    def __init__(
        self,
        *,
        runs: list[dict[str, object]] | None = None,
        jobs_payload: list[dict[str, object]] | None = None,
        artifacts: list[dict[str, object]] | None = None,
        record: dict[str, object] | None = None,
    ) -> None:
        self.runs = runs if runs is not None else [run_payload()]
        self.jobs_payload = jobs_payload if jobs_payload is not None else jobs()
        self.artifacts = artifacts if artifacts is not None else [provenance_artifact(), deploy_artifact()]
        self.record = record if record is not None else record_payload()

    def json(
        self,
        repo: str,
        token: str,
        path: str,
        query: dict[str, str] | None = None,
    ) -> dict[str, object]:
        if path == "actions/runs":
            return {"workflow_runs": self.runs}
        if path.endswith("/jobs"):
            return {"jobs": self.jobs_payload}
        if path.endswith("/artifacts"):
            return {"artifacts": self.artifacts}
        raise AssertionError(f"unexpected JSON request {path} {query}")

    def bytes(self, repo: str, token: str, url: str) -> bytes:
        if url.startswith("artifact://"):
            return artifact_zip(self.record)
        if url.startswith("https://raw.githubusercontent.com/"):
            return (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_bytes()
        raise AssertionError(f"unexpected bytes request {url}")


def select(fake: FakeGitHub | None = None, current_run_id: int | str | None = 24623274722):
    module = load_script()
    fake = fake or FakeGitHub()
    return module.resolve_same_sha_main_evidence(
        repo="seungpyoson/bolt-v2",
        token="token",
        sha=SHA,
        current_run_id=current_run_id,
        config_path=CONFIG_PATH,
        api_json=fake.json,
        api_bytes=fake.bytes,
        now=load_provenance().parse_timestamp("2026-06-13T00:30:00Z"),
    )


def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001 - wrapper exposes a domain error.
        if fragment not in str(exc):
            raise AssertionError(f"expected {fragment!r}, got: {exc}") from exc
        return
    raise AssertionError(f"expected error containing {fragment!r}")


def assert_selects_exact_main_run_and_outputs() -> None:
    module = load_script()
    evidence = select()
    if evidence.source_run_id != str(RUN_ID):
        raise AssertionError(evidence)
    if evidence.source_run_url != "https://github.com/seungpyoson/bolt-v2/actions/runs/24623219988":
        raise AssertionError(evidence)
    if evidence.check_suite_id != str(CHECK_SUITE_ID):
        raise AssertionError(evidence)
    if evidence.artifact_id != "6516430716":
        raise AssertionError(evidence)
    if evidence.artifact_name != "bolt-v2-binary":
        raise AssertionError(evidence)
    if evidence.artifact_size != "12631205":
        raise AssertionError(evidence)
    if evidence.source_sha != SHA:
        raise AssertionError(evidence)

    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = pathlib.Path(tmpdir) / "github-output"
        module.write_github_output(evidence, output_path)
        output = output_path.read_text()
    for line in (
        f"source_run_id={RUN_ID}",
        "source_run_url=https://github.com/seungpyoson/bolt-v2/actions/runs/24623219988",
        f"check_suite_id={CHECK_SUITE_ID}",
        "artifact_id=6516430716",
        "artifact_name=bolt-v2-binary",
        "artifact_size=12631205",
        f"source_sha={SHA}",
    ):
        if line not in output:
            raise AssertionError(output)


def assert_rejects_current_tag_run_as_source() -> None:
    assert_raises("no candidate provenance evidence", lambda: select(current_run_id=RUN_ID))


def assert_rejects_gate_failure() -> None:
    assert_raises("gate", lambda: select(FakeGitHub(jobs_payload=jobs(gate_conclusion="failure"))))


def assert_rejects_artifact_size_and_expiry() -> None:
    missing_size = deploy_artifact()
    missing_size.pop("size_in_bytes")
    assert_raises("artifact_size", lambda: select(FakeGitHub(artifacts=[provenance_artifact(), missing_size])))
    for value in (True, None):
        assert_raises(
            "expired",
            lambda value=value: select(FakeGitHub(artifacts=[provenance_artifact(), deploy_artifact(expired=value)])),
        )


def assert_rejects_artifact_binding_and_ambiguity() -> None:
    wrong_branch = deploy_artifact(workflow_run={"id": RUN_ID, "head_branch": "release", "head_sha": SHA})
    assert_raises("artifact branch", lambda: select(FakeGitHub(artifacts=[provenance_artifact(), wrong_branch])))
    wrong_sha = deploy_artifact(workflow_run={"id": RUN_ID, "head_branch": "main", "head_sha": "0" * 40})
    assert_raises("artifact SHA", lambda: select(FakeGitHub(artifacts=[provenance_artifact(), wrong_sha])))
    assert_raises(
        "ambiguous",
        lambda: select(FakeGitHub(artifacts=[provenance_artifact(), deploy_artifact(), deploy_artifact(id=6516430717)])),
    )


def main() -> int:
    assert_selects_exact_main_run_and_outputs()
    assert_rejects_current_tag_run_as_source()
    assert_rejects_gate_failure()
    assert_rejects_artifact_size_and_expiry()
    assert_rejects_artifact_binding_and_ambiguity()
    print("OK: same-SHA main evidence self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
