#!/usr/bin/env python3
"""Self-tests for CI provenance emission and resolution."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import hashlib
import json
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "ci_provenance.py"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"

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
        "run_id": 24623219988,
        "run_attempt": 1,
        "check_suite_id": 65233803543,
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


def main() -> int:
    assert_unknown_mode_fails()
    assert_missing_config_table_fails()
    assert_invalid_shard_count_fails()
    assert_unknown_record_schema_fails()
    assert_resolve_fingerprint_is_rejected()
    assert_config_digest_is_canonical()
    assert_record_schema_requires_head_and_tested_sha()
    assert_pr_event_record_cannot_validate_for_pr_head_reuse()
    assert_digest_mismatches_fail()
    print("OK: CI provenance self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
