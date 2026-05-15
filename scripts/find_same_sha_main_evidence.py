#!/usr/bin/env python3
"""Find exact same-SHA main CI evidence for smoke-tag deploy reuse."""

from __future__ import annotations

import dataclasses
import json
import os
import pathlib
import sys
import urllib.parse
import urllib.request


WORKFLOW_PATH = ".github/workflows/ci.yml"
WORKFLOW_NAME = "CI"
ARTIFACT_NAME = "bolt-v2-binary"
REQUIRED_NON_TEST_JOBS = (
    "detector",
    "fmt-check",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "build",
    "gate",
)
REQUIRED_TEST_SHARDS = 4


class EvidenceError(RuntimeError):
    """Raised when exact same-SHA evidence is missing or unsafe to reuse."""


@dataclasses.dataclass(frozen=True)
class SameShaMainEvidence:
    source_run_id: str
    source_run_url: str
    check_suite_id: str
    artifact_id: str
    artifact_name: str
    artifact_size: str
    source_sha: str


def as_text(value: object) -> str:
    return "" if value is None else str(value)


def is_successful_main_ci_run(run: dict[str, object], expected_sha: str, current_run_id: int | str | None) -> bool:
    if current_run_id is not None and as_text(run.get("id")) == as_text(current_run_id):
        return False
    return (
        as_text(run.get("name")) == WORKFLOW_NAME
        and as_text(run.get("path")) == WORKFLOW_PATH
        and as_text(run.get("event")) == "push"
        and as_text(run.get("head_branch")) == "main"
        and as_text(run.get("head_sha")) == expected_sha
        and as_text(run.get("status")) == "completed"
        and as_text(run.get("conclusion")) == "success"
    )


def is_successful_job(job: dict[str, object]) -> bool:
    return as_text(job.get("status")) == "completed" and as_text(job.get("conclusion")) == "success"


def validate_required_jobs(jobs_payload: dict[str, object], run_id: str) -> None:
    jobs = jobs_payload.get("jobs")
    if not isinstance(jobs, list):
        raise EvidenceError(f"source run {run_id} jobs payload is malformed")

    by_name = {as_text(job.get("name")): job for job in jobs if isinstance(job, dict)}
    for name in REQUIRED_NON_TEST_JOBS:
        job = by_name.get(name)
        if job is None:
            raise EvidenceError(f"source run {run_id} missing required job {name}")
        if not is_successful_job(job):
            raise EvidenceError(
                f"source run {run_id} required job {name} was "
                f"{as_text(job.get('status'))}/{as_text(job.get('conclusion'))}"
            )

    test_jobs = [
        job
        for job in jobs
        if isinstance(job, dict)
        and (as_text(job.get("name")) == "test" or as_text(job.get("name")).startswith("test ("))
    ]
    if len(test_jobs) < REQUIRED_TEST_SHARDS:
        raise EvidenceError(
            f"source run {run_id} has {len(test_jobs)} successful-looking test shards; "
            f"expected {REQUIRED_TEST_SHARDS} test shards"
        )
    bad_test_jobs = [job for job in test_jobs if not is_successful_job(job)]
    if bad_test_jobs:
        names = ", ".join(as_text(job.get("name")) for job in bad_test_jobs)
        raise EvidenceError(f"source run {run_id} has non-successful test shards: {names}")


def validate_artifact(artifacts_payload: dict[str, object], run_id: str, expected_sha: str) -> dict[str, object]:
    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        raise EvidenceError(f"source run {run_id} artifacts payload is malformed")
    matches = [
        artifact
        for artifact in artifacts
        if isinstance(artifact, dict) and as_text(artifact.get("name")) == ARTIFACT_NAME
    ]
    if not matches:
        raise EvidenceError(f"source run {run_id} missing artifact {ARTIFACT_NAME}")
    if len(matches) > 1:
        ids = ", ".join(as_text(artifact.get("id")) for artifact in matches)
        raise EvidenceError(f"source run {run_id} has ambiguous {ARTIFACT_NAME} artifacts: {ids}")

    artifact = matches[0]
    if artifact.get("expired") is not False:
        raise EvidenceError(f"source run {run_id} artifact expired or has unknown expiry state")

    workflow_run = artifact.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise EvidenceError(f"source run {run_id} artifact workflow_run payload is malformed")
    if as_text(workflow_run.get("id")) != run_id:
        raise EvidenceError(f"artifact run ID does not match source run {run_id}")
    if as_text(workflow_run.get("head_branch")) != "main":
        raise EvidenceError(f"artifact branch is {as_text(workflow_run.get('head_branch'))}, expected main")
    if as_text(workflow_run.get("head_sha")) != expected_sha:
        raise EvidenceError(
            f"artifact SHA {as_text(workflow_run.get('head_sha'))} does not match expected {expected_sha}"
        )
    return artifact


def run_sort_key(run: dict[str, object]) -> tuple[str, str]:
    return (as_text(run.get("updated_at")), as_text(run.get("id")))


def select_same_sha_main_evidence(
    *,
    runs_payload: dict[str, object],
    jobs_payload_by_run_id: dict[int | str, dict[str, object]],
    artifacts_payload_by_run_id: dict[int | str, dict[str, object]],
    expected_sha: str,
    current_run_id: int | str | None = None,
) -> SameShaMainEvidence:
    runs = runs_payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise EvidenceError("workflow runs payload is malformed")

    candidates = [
        run
        for run in runs
        if isinstance(run, dict) and is_successful_main_ci_run(run, expected_sha, current_run_id)
    ]
    if not candidates:
        raise EvidenceError(f"no successful main CI run found for exact SHA {expected_sha}")

    last_error: EvidenceError | None = None
    for run in sorted(candidates, key=run_sort_key, reverse=True):
        run_id = as_text(run.get("id"))
        try:
            jobs_payload = jobs_payload_by_run_id.get(run_id) or jobs_payload_by_run_id.get(int(run_id))
            artifacts_payload = artifacts_payload_by_run_id.get(run_id) or artifacts_payload_by_run_id.get(int(run_id))
            if jobs_payload is None:
                raise EvidenceError(f"source run {run_id} jobs payload is missing")
            if artifacts_payload is None:
                raise EvidenceError(f"source run {run_id} artifacts payload is missing")
            validate_required_jobs(jobs_payload, run_id)
            artifact = validate_artifact(artifacts_payload, run_id, expected_sha)
            return SameShaMainEvidence(
                source_run_id=run_id,
                source_run_url=as_text(run.get("html_url")),
                check_suite_id=as_text(run.get("check_suite_id")),
                artifact_id=as_text(artifact.get("id")),
                artifact_name=ARTIFACT_NAME,
                artifact_size=as_text(artifact.get("size_in_bytes")),
                source_sha=expected_sha,
            )
        except EvidenceError as exc:
            last_error = exc

    if last_error is not None:
        raise last_error
    raise EvidenceError(f"no complete same-SHA main CI evidence found for {expected_sha}")


def github_api_json(repo: str, token: str, path: str, query: dict[str, str] | None = None) -> dict[str, object]:
    url = f"https://api.github.com/repos/{repo}/{path}"
    if query:
        url += "?" + urllib.parse.urlencode(query)
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def fetch_same_sha_payloads(repo: str, token: str, sha: str) -> tuple[dict[str, object], dict[str, dict[str, object]], dict[str, dict[str, object]]]:
    runs_payload = github_api_json(
        repo,
        token,
        "actions/runs",
        {
            "event": "push",
            "branch": "main",
            "head_sha": sha,
            "status": "success",
            "per_page": "20",
        },
    )
    jobs_by_run_id: dict[str, dict[str, object]] = {}
    artifacts_by_run_id: dict[str, dict[str, object]] = {}
    runs = runs_payload.get("workflow_runs")
    if isinstance(runs, list):
        for run in runs:
            if not isinstance(run, dict):
                continue
            run_id = as_text(run.get("id"))
            if not run_id:
                continue
            jobs_by_run_id[run_id] = github_api_json(repo, token, f"actions/runs/{run_id}/jobs", {"per_page": "100"})
            artifacts_by_run_id[run_id] = github_api_json(
                repo,
                token,
                f"actions/runs/{run_id}/artifacts",
                {"per_page": "100"},
            )
    return runs_payload, jobs_by_run_id, artifacts_by_run_id


def write_github_output(evidence: SameShaMainEvidence, output_path: str | pathlib.Path) -> None:
    lines = (
        f"source_run_id={evidence.source_run_id}",
        f"source_run_url={evidence.source_run_url}",
        f"check_suite_id={evidence.check_suite_id}",
        f"artifact_id={evidence.artifact_id}",
        f"artifact_name={evidence.artifact_name}",
        f"artifact_size={evidence.artifact_size}",
        f"source_sha={evidence.source_sha}",
    )
    with pathlib.Path(output_path).open("a", encoding="utf-8") as handle:
        for line in lines:
            handle.write(line)
            handle.write("\n")


def require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise EvidenceError(f"missing required environment variable {name}")
    return value


def main() -> int:
    try:
        repo = require_env("GITHUB_REPOSITORY")
        token = require_env("GITHUB_TOKEN")
        sha = require_env("GITHUB_SHA")
        runs_payload, jobs_by_run_id, artifacts_by_run_id = fetch_same_sha_payloads(repo, token, sha)
        evidence = select_same_sha_main_evidence(
            runs_payload=runs_payload,
            jobs_payload_by_run_id=jobs_by_run_id,
            artifacts_payload_by_run_id=artifacts_by_run_id,
            expected_sha=sha,
            current_run_id=os.environ.get("GITHUB_RUN_ID"),
        )
        print(
            "same-SHA main evidence: "
            f"source_run_id={evidence.source_run_id} "
            f"check_suite_id={evidence.check_suite_id} "
            f"artifact_id={evidence.artifact_id} "
            f"source_sha={evidence.source_sha}"
        )
        output_path = os.environ.get("GITHUB_OUTPUT")
        if output_path:
            write_github_output(evidence, output_path)
        return 0
    except EvidenceError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
