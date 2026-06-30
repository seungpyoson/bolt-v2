#!/usr/bin/env python3
"""Find exact same-SHA main CI evidence for smoke-tag deploy reuse."""

from __future__ import annotations

import dataclasses
import json
import os
import pathlib
import sys


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import ci_provenance  # noqa: E402
import config_validators as _cv  # noqa: E402


GATE_CHECK_NAME = "gate"


class EvidenceError(RuntimeError):
    """Raised when exact same-SHA deploy evidence is missing or unsafe to reuse."""


as_text = _cv.as_text


@dataclasses.dataclass(frozen=True)
class SameShaMainEvidence:
    source_run_id: str
    source_run_url: str
    check_suite_id: str
    artifact_id: str
    artifact_name: str
    artifact_size: str
    source_sha: str

def validate_gate_success(jobs_payload: dict[str, object]) -> None:
    try:
        ci_provenance.require_job_success(ci_provenance.jobs_by_name(jobs_payload), GATE_CHECK_NAME)
    except ci_provenance.ProvenanceError as exc:
        raise EvidenceError(str(exc)) from exc


def validate_deploy_artifact(
    artifacts_payload: dict[str, object],
    *,
    config: ci_provenance.ProvenanceConfig,
    run_id: str,
    expected_sha: str,
) -> dict[str, object]:
    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        raise EvidenceError(f"source run {run_id} artifacts payload is malformed")
    matches = [
        artifact
        for artifact in artifacts
        if isinstance(artifact, dict) and as_text(artifact.get("name")) == config.deploy_artifact_name
    ]
    if not matches:
        raise EvidenceError(f"source run {run_id} missing artifact {config.deploy_artifact_name}")
    if len(matches) > 1:
        ids = ", ".join(as_text(artifact.get("id")) for artifact in matches)
        raise EvidenceError(f"source run {run_id} has ambiguous {config.deploy_artifact_name} artifacts: {ids}")

    artifact = matches[0]
    if artifact.get("expired") is not False:
        raise EvidenceError(f"source run {run_id} artifact expired or has unknown expiry state")
    if artifact.get("size_in_bytes") is None:
        raise EvidenceError(f"source run {run_id} artifact_size is missing")

    workflow_run = artifact.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise EvidenceError(f"source run {run_id} artifact workflow_run payload is malformed")
    if as_text(workflow_run.get("id")) != run_id:
        raise EvidenceError(f"artifact run ID does not match source run {run_id}")
    if as_text(workflow_run.get("head_branch")) != config.deploy_source_branch:
        raise EvidenceError(
            f"artifact branch is {as_text(workflow_run.get('head_branch'))}, expected {config.deploy_source_branch}"
        )
    if as_text(workflow_run.get("head_sha")) != expected_sha:
        raise EvidenceError(
            f"artifact SHA {as_text(workflow_run.get('head_sha'))} does not match expected {expected_sha}"
        )
    return artifact


def resolve_same_sha_main_evidence(
    *,
    repo: str,
    token: str,
    sha: str,
    current_run_id: int | str | None,
    config_path: pathlib.Path = ci_provenance.DEFAULT_CONFIG,
    api_json=ci_provenance.github_api_json,
    api_bytes=ci_provenance.github_api_bytes,
    now=None,
) -> SameShaMainEvidence:
    config = ci_provenance.load_config(config_path)
    try:
        resolved = ci_provenance.resolve_exact_sha_evidence(
            repo=repo,
            token=token,
            requested_sha=sha,
            config=config,
            config_path=config_path,
            current_run_id=current_run_id,
            api_json=api_json,
            api_bytes=api_bytes,
            now=now,
        )
    except ci_provenance.ProvenanceError as exc:
        raise EvidenceError(str(exc)) from exc

    run_id = as_text(resolved.run.get("id"))
    jobs_payload = api_json(
        repo,
        token,
        f"actions/runs/{run_id}/jobs",
        {"per_page": str(config.run_jobs_per_page)},
    )
    validate_gate_success(jobs_payload)

    artifacts_payload = api_json(
        repo,
        token,
        f"actions/runs/{run_id}/artifacts",
        {"per_page": str(config.run_artifacts_per_page)},
    )
    artifact = validate_deploy_artifact(
        artifacts_payload,
        config=config,
        run_id=run_id,
        expected_sha=sha,
    )
    return SameShaMainEvidence(
        source_run_id=run_id,
        source_run_url=as_text(resolved.run.get("html_url")),
        check_suite_id=as_text(resolved.run.get("check_suite_id")),
        artifact_id=as_text(artifact.get("id")),
        artifact_name=config.deploy_artifact_name,
        artifact_size=as_text(artifact.get("size_in_bytes")),
        source_sha=sha,
    )


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
        evidence = resolve_same_sha_main_evidence(
            repo=require_env("GITHUB_REPOSITORY"),
            token=require_env("GITHUB_TOKEN"),
            sha=require_env("GITHUB_SHA"),
            current_run_id=os.environ.get("GITHUB_RUN_ID"),
        )
        print(
            "same-SHA main evidence: "
            + json.dumps(dataclasses.asdict(evidence), sort_keys=True)
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
