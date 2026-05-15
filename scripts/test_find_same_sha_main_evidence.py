#!/usr/bin/env python3
"""Self-tests for same-SHA main-run evidence selection."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "find_same_sha_main_evidence.py"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("find_same_sha_main_evidence", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load find_same_sha_main_evidence.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def run_payload(**overrides):
    payload = {
        "id": 24623219988,
        "name": "CI",
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "main",
        "head_sha": SHA,
        "status": "completed",
        "conclusion": "success",
        "check_suite_id": 65233803543,
        "html_url": "https://github.com/seungpyoson/bolt-v2/actions/runs/24623219988",
    }
    payload.update(overrides)
    return payload


def job(name: str, conclusion: str = "success"):
    return {"name": name, "status": "completed", "conclusion": conclusion}


def required_jobs():
    return [
        job("detector"),
        job("fmt-check"),
        job("deny"),
        job("clippy"),
        job("check-aarch64"),
        job("source-fence"),
        job("test (1)"),
        job("test (2)"),
        job("test (3)"),
        job("test (4)"),
        job("build"),
        job("gate"),
    ]


def artifact(**overrides):
    payload = {
        "id": 6516430716,
        "name": "bolt-v2-binary",
        "expired": False,
        "size_in_bytes": 12631205,
        "workflow_run": {
            "id": 24623219988,
            "head_branch": "main",
            "head_sha": SHA,
        },
    }
    payload.update(overrides)
    return payload


def select(runs, jobs=None, artifacts=None):
    module = load_script()
    return module.select_same_sha_main_evidence(
        runs_payload={"workflow_runs": runs},
        jobs_payload_by_run_id={24623219988: {"jobs": jobs if jobs is not None else required_jobs()}},
        artifacts_payload_by_run_id={
            24623219988: {"artifacts": artifacts if artifacts is not None else [artifact()]}
        },
        expected_sha=SHA,
        current_run_id=24623274722,
    )


def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001 - script exposes a domain error.
        if fragment not in str(exc):
            raise AssertionError(f"expected error containing {fragment!r}, got: {exc}") from exc
        return
    raise AssertionError(f"expected error containing {fragment!r}")


def assert_selects_exact_main_run() -> None:
    evidence = select([run_payload()])
    if evidence.source_run_id != "24623219988":
        raise AssertionError(evidence)
    if evidence.check_suite_id != "65233803543":
        raise AssertionError(evidence)
    if evidence.artifact_id != "6516430716":
        raise AssertionError(evidence)
    if evidence.source_sha != SHA:
        raise AssertionError(evidence)


def assert_rejects_non_main_or_wrong_sha_runs() -> None:
    assert_raises("no successful main CI run", lambda: select([run_payload(head_branch="release")]))
    assert_raises("no successful main CI run", lambda: select([run_payload(head_sha="0" * 40)]))
    assert_raises("no successful main CI run", lambda: select([run_payload(path=".github/workflows/summary.yml")]))


def assert_rejects_incomplete_required_jobs() -> None:
    broken_jobs = required_jobs()
    broken_jobs[5] = job("source-fence", "skipped")
    assert_raises("source-fence", lambda: select([run_payload()], jobs=broken_jobs))
    missing_test_shard = [job_payload for job_payload in required_jobs() if job_payload["name"] != "test (4)"]
    assert_raises("test shards", lambda: select([run_payload()], jobs=missing_test_shard))


def assert_rejects_untrusted_artifacts() -> None:
    assert_raises("artifact expired", lambda: select([run_payload()], artifacts=[artifact(expired=True)]))
    wrong_sha_artifact = artifact(
        workflow_run={"id": 24623219988, "head_branch": "main", "head_sha": "0" * 40}
    )
    assert_raises("artifact SHA", lambda: select([run_payload()], artifacts=[wrong_sha_artifact]))


def assert_writes_github_output() -> None:
    module = load_script()
    evidence = select([run_payload()])
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = pathlib.Path(tmpdir) / "github-output"
        module.write_github_output(evidence, output_path)
        output = output_path.read_text()
    for line in (
        "source_run_id=24623219988",
        "check_suite_id=65233803543",
        "artifact_id=6516430716",
        f"source_sha={SHA}",
    ):
        if line not in output:
            raise AssertionError(output)


def assert_api_failures_are_bounded() -> None:
    module = load_script()
    original_urlopen = module.urllib.request.urlopen

    def raises_url_error(request, timeout):  # noqa: ANN001 - local fake matches urllib call shape.
        raise module.urllib.error.URLError("offline")

    class InvalidJsonResponse:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):  # noqa: ANN001 - context manager protocol.
            return False

        def read(self) -> bytes:
            return b"not-json"

    def invalid_json(request, timeout):  # noqa: ANN001 - local fake matches urllib call shape.
        return InvalidJsonResponse()

    try:
        module.urllib.request.urlopen = raises_url_error
        assert_raises("GitHub API request failed", lambda: module.github_api_json("owner/repo", "token", "actions/runs"))
        module.urllib.request.urlopen = invalid_json
        assert_raises("GitHub API request failed", lambda: module.github_api_json("owner/repo", "token", "actions/runs"))
    finally:
        module.urllib.request.urlopen = original_urlopen


def main() -> int:
    assert_selects_exact_main_run()
    assert_rejects_non_main_or_wrong_sha_runs()
    assert_rejects_incomplete_required_jobs()
    assert_rejects_untrusted_artifacts()
    assert_writes_github_output()
    assert_api_failures_are_bounded()
    print("OK: same-SHA main evidence self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
