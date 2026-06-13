#!/usr/bin/env python3
"""Emit and resolve CI provenance evidence."""

from __future__ import annotations

import argparse
import dataclasses
import json
import pathlib
import sys
import tomllib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
SUPPORTED_MODES = {"emit-full-ci", "resolve-exact-sha", "validate-record"}
POLICY_VALUES = {"full", "defer", "tag_reuse"}
POLICY_ROWS = (
    "draft_pr_synchronize",
    "draft_pr_opened",
    "draft_pr_reopened",
    "converted_to_draft",
    "ready_pr",
    "ready_for_review",
    "workflow_dispatch",
    "main_push",
    "tag",
    "unknown_event",
)


class ProvenanceError(RuntimeError):
    """Raised when provenance evidence is absent, malformed, or unsafe."""


@dataclasses.dataclass(frozen=True)
class JobConfig:
    logical_name: str
    check_name: str | None
    check_name_template: str | None
    shard_count: int | None
    conditional: str | None


@dataclasses.dataclass(frozen=True)
class ProvenanceConfig:
    schema_version: int
    artifact_name_template: str
    workflow_key: str
    workflow_name: str
    workflow_path: str
    fingerprint_source: str
    fingerprint_artifact_prefix: str
    fingerprint_workflow: str
    required_jobs: tuple[str, ...]
    conditional_jobs: tuple[str, ...]
    conditional_job_outputs: dict[str, str]
    jobs: dict[str, JobConfig]
    deploy_artifact_name: str
    deploy_source_event: str
    deploy_source_branch: str
    deploy_require_gate_check: bool
    dispatch_workflow_input: str
    workflow_runs_per_page: int
    run_jobs_per_page: int
    run_artifacts_per_page: int
    max_lookback_pages: int
    max_lookback_age_seconds: int
    artifact_retention_days: int
    policy: dict[str, str]
    force_full_ci: bool
    ignore_emit_failure: bool


def require_table(parent: dict[str, object], key: str, prefix: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise ProvenanceError(f"{prefix}.{key} must be a table")
    return value


def require_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise ProvenanceError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or value <= 0:
        raise ProvenanceError(f"{prefix}.{key} must be a positive integer")
    return value


def require_string_list(parent: dict[str, object], key: str, prefix: str) -> tuple[str, ...]:
    value = parent.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ProvenanceError(f"{prefix}.{key} must be a non-empty string list")
    return tuple(value)


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ProvenanceError(f"config missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ProvenanceError(f"config is invalid TOML: {exc}") from exc
    except OSError as exc:
        raise ProvenanceError(f"config could not be read: {exc}") from exc


def load_config(path: pathlib.Path = DEFAULT_CONFIG) -> ProvenanceConfig:
    data = load_toml(path)
    meter = data.get("meter")
    if not isinstance(meter, dict):
        raise ProvenanceError("missing [meter]")
    ci_provenance = data.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        raise ProvenanceError("missing [ci_provenance]")
    if ci_provenance.get("schema_version") != 1:
        raise ProvenanceError("ci_provenance.schema_version must be 1")

    duplicated_fingerprint_keys = {
        "fingerprint_artifact_prefix",
        "fingerprint_workflow",
    } & set(ci_provenance)
    if duplicated_fingerprint_keys:
        names = ", ".join(sorted(duplicated_fingerprint_keys))
        raise ProvenanceError(f"[ci_provenance] must reference [meter] fingerprint keys, duplicated {names}")

    artifact_name_template = require_string(
        ci_provenance, "artifact_name_template", "ci_provenance"
    )
    if "{run_attempt}" not in artifact_name_template:
        raise ProvenanceError("ci_provenance.artifact_name_template must include {run_attempt}")

    full_ci = require_table(ci_provenance, "full_ci", "ci_provenance")
    required_jobs = require_string_list(full_ci, "required_jobs", "ci_provenance.full_ci")
    conditional_jobs = require_string_list(
        full_ci, "conditional_jobs", "ci_provenance.full_ci"
    )
    conditional_job_outputs = full_ci.get("conditional_job_outputs")
    if not isinstance(conditional_job_outputs, dict) or not all(
        isinstance(key, str) and isinstance(value, str)
        for key, value in conditional_job_outputs.items()
    ):
        raise ProvenanceError("ci_provenance.full_ci.conditional_job_outputs must map strings")

    job_tables = require_table(full_ci, "jobs", "ci_provenance.full_ci")
    jobs: dict[str, JobConfig] = {}
    for job in (*required_jobs, *conditional_jobs):
        job_table = job_tables.get(job)
        if not isinstance(job_table, dict):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job} missing")
        check_name = job_table.get("check_name")
        check_name_template = job_table.get("check_name_template")
        if check_name is not None and (not isinstance(check_name, str) or not check_name):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.check_name must be a non-empty string")
        if check_name_template is not None and (
            not isinstance(check_name_template, str) or not check_name_template
        ):
            raise ProvenanceError(
                f"ci_provenance.full_ci.jobs.{job}.check_name_template must be a non-empty string"
            )
        if check_name is None and check_name_template is None:
            raise ProvenanceError(
                f"ci_provenance.full_ci.jobs.{job} must define check_name or check_name_template"
            )
        shard_count = job_table.get("shard_count")
        if check_name_template is not None:
            if not isinstance(shard_count, int) or shard_count <= 0:
                raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.shard_count must be a positive integer")
            if "{shard}" not in check_name_template:
                raise ProvenanceError(
                    f"ci_provenance.full_ci.jobs.{job}.check_name_template must include {{shard}}"
                )
            if "{shard_count}" not in check_name_template:
                raise ProvenanceError(
                    f"ci_provenance.full_ci.jobs.{job}.check_name_template must include {{shard_count}}"
                )
        elif shard_count is not None:
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.shard_count requires check_name_template")
        conditional = job_table.get("conditional")
        if conditional is not None and (not isinstance(conditional, str) or not conditional):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.conditional must be a non-empty string")
        if job in conditional_jobs and conditional != conditional_job_outputs.get(job):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.conditional must match conditional_job_outputs")
        jobs[job] = JobConfig(
            logical_name=job,
            check_name=check_name,
            check_name_template=check_name_template,
            shard_count=shard_count if isinstance(shard_count, int) else None,
            conditional=conditional,
        )

    deploy = require_table(ci_provenance, "deploy", "ci_provenance")
    dispatch = require_table(ci_provenance, "dispatch", "ci_provenance")
    api_limits = require_table(ci_provenance, "api_limits", "ci_provenance")
    artifacts = require_table(ci_provenance, "artifacts", "ci_provenance")
    policy_table = require_table(ci_provenance, "policy", "ci_provenance")
    overrides = require_table(policy_table, "override", "ci_provenance.policy")

    retention_days = require_positive_int(artifacts, "retention_days", "ci_provenance.artifacts")
    max_lookback_age_seconds = require_positive_int(
        api_limits, "max_lookback_age_seconds", "ci_provenance.api_limits"
    )
    if max_lookback_age_seconds > retention_days * 24 * 60 * 60:
        raise ProvenanceError("max lookback age must not exceed artifact retention")

    policy: dict[str, str] = {}
    for row in POLICY_ROWS:
        value = policy_table.get(row)
        if value not in POLICY_VALUES:
            raise ProvenanceError(f"ci_provenance.policy.{row} must be full, defer, or tag_reuse")
        policy[row] = value

    force_full_ci = overrides.get("force_full_ci")
    ignore_emit_failure = overrides.get("ignore_emit_failure")
    if force_full_ci is not False:
        raise ProvenanceError("ci_provenance.policy.override.force_full_ci must default to false")
    if ignore_emit_failure is not False:
        raise ProvenanceError("ci_provenance.policy.override.ignore_emit_failure must default to false")

    return ProvenanceConfig(
        schema_version=1,
        artifact_name_template=artifact_name_template,
        workflow_key=require_string(ci_provenance, "workflow_key", "ci_provenance"),
        workflow_name=require_string(ci_provenance, "workflow_name", "ci_provenance"),
        workflow_path=require_string(ci_provenance, "workflow_path", "ci_provenance"),
        fingerprint_source=require_string(ci_provenance, "fingerprint_source", "ci_provenance"),
        fingerprint_artifact_prefix=require_string(meter, "fingerprint_artifact_prefix", "meter"),
        fingerprint_workflow=require_string(meter, "fingerprint_workflow", "meter"),
        required_jobs=required_jobs,
        conditional_jobs=conditional_jobs,
        conditional_job_outputs=dict(conditional_job_outputs),
        jobs=jobs,
        deploy_artifact_name=require_string(deploy, "artifact_name", "ci_provenance.deploy"),
        deploy_source_event=require_string(deploy, "require_source_event", "ci_provenance.deploy"),
        deploy_source_branch=require_string(deploy, "require_source_branch", "ci_provenance.deploy"),
        deploy_require_gate_check=deploy.get("require_gate_check") is True,
        dispatch_workflow_input=require_string(dispatch, "workflow_input", "ci_provenance.dispatch"),
        workflow_runs_per_page=require_positive_int(
            api_limits, "workflow_runs_per_page", "ci_provenance.api_limits"
        ),
        run_jobs_per_page=require_positive_int(
            api_limits, "run_jobs_per_page", "ci_provenance.api_limits"
        ),
        run_artifacts_per_page=require_positive_int(
            api_limits, "run_artifacts_per_page", "ci_provenance.api_limits"
        ),
        max_lookback_pages=require_positive_int(
            api_limits, "max_lookback_pages", "ci_provenance.api_limits"
        ),
        max_lookback_age_seconds=max_lookback_age_seconds,
        artifact_retention_days=retention_days,
        policy=policy,
        force_full_ci=force_full_ci,
        ignore_emit_failure=ignore_emit_failure,
    )


def load_json(path: pathlib.Path) -> dict[str, object]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ProvenanceError(f"record missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ProvenanceError(f"record is invalid JSON: {exc}") from exc
    except OSError as exc:
        raise ProvenanceError(f"record could not be read: {exc}") from exc
    if not isinstance(data, dict):
        raise ProvenanceError("record must be a JSON object")
    return data


def validate_record_schema(record: dict[str, object], config: ProvenanceConfig) -> None:
    if record.get("schema_version") != config.schema_version:
        raise ProvenanceError(f"unknown provenance schema {record.get('schema_version')!r}")
    if record.get("kind") != "full-ci":
        raise ProvenanceError("record kind must be full-ci")


def emit_full_ci(config: ProvenanceConfig) -> dict[str, object]:
    return {
        "schema_version": config.schema_version,
        "kind": "full-ci",
        "required_jobs": {job: None for job in config.required_jobs},
        "conditional_jobs": {
            job: {"required": None, "result": None} for job in config.conditional_jobs
        },
        "nextest_fingerprint": None,
    }


def resolve_exact_sha(_config: ProvenanceConfig) -> None:
    raise ProvenanceError("no exact-SHA provenance evidence found")


def parser_for_mode(mode: str) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog=f"ci_provenance.py {mode}")
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    if mode == "validate-record":
        parser.add_argument("--record", type=pathlib.Path, required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    if not argv:
        print(f"ERROR: unknown mode; expected one of {', '.join(sorted(SUPPORTED_MODES))}", file=sys.stderr)
        return 2
    mode, rest = argv[0], argv[1:]
    if mode == "resolve-fingerprint":
        print("ERROR: resolve-fingerprint is not supported in Slice 2", file=sys.stderr)
        return 2
    if mode not in SUPPORTED_MODES:
        print(f"ERROR: unknown mode {mode!r}", file=sys.stderr)
        return 2

    parser = parser_for_mode(mode)
    try:
        args = parser.parse_args(rest)
        config = load_config(args.config)
        if mode == "emit-full-ci":
            print(json.dumps(emit_full_ci(config), sort_keys=True))
        elif mode == "validate-record":
            validate_record_schema(load_json(args.record), config)
            print("record valid")
        elif mode == "resolve-exact-sha":
            resolve_exact_sha(config)
        return 0
    except ProvenanceError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
