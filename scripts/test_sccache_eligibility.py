#!/usr/bin/env python3
"""Self-tests for the governed sccache eligibility resolver."""

from __future__ import annotations

import contextlib
import hashlib
import io
import os
import pathlib
import shlex
import tempfile
import tomllib
from unittest import mock

import sccache_eligibility
from sccache_eligibility import resolve_sccache_eligibility


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCATION = tomllib.loads((REPO_ROOT / "ci" / "sccache-location.toml").read_text(encoding="utf-8"))["location"]


def assert_case(
    label: str,
    *,
    active: bool = True,
    event_name: str,
    github_ref: str,
    read_role_arn: str = "arn:aws:iam::123456789012:role/read",
    write_role_arn: str = "arn:aws:iam::123456789012:role/write",
    runner_arch: str = "ARM64",
    location: dict[str, object] | None = None,
    expected_eligible: bool,
    expected_role: str,
    expected_mode: str,
    expected_digest: str | None = None,
    expected_idle_timeout: int | None = 0,
) -> None:
    result = resolve_sccache_eligibility(
        active=active,
        event_name=event_name,
        github_ref=github_ref,
        read_role_arn=read_role_arn,
        write_role_arn=write_role_arn,
        runner_arch=runner_arch,
        location=LOCATION if location is None else location,
    )
    actual = (result.eligible, result.role_arn, result.cache_mode)
    expected = (expected_eligible, expected_role, expected_mode)
    if actual != expected:
        raise AssertionError(f"{label}: expected {expected!r}, got {actual!r}")
    if expected_digest is not None and result.executable_sha256 != expected_digest:
        raise AssertionError(f"{label}: expected digest {expected_digest!r}, got {result.executable_sha256!r}")
    if result.idle_timeout_seconds != expected_idle_timeout:
        raise AssertionError(
            f"{label}: expected idle timeout {expected_idle_timeout!r}, got {result.idle_timeout_seconds!r}"
        )


def main() -> int:
    executable_digests = LOCATION.get("executable_sha256")
    if not isinstance(executable_digests, dict) or set(executable_digests) != {"ARM64", "X64"}:
        raise AssertionError("sccache executable provenance must cover the supported ARM64 and X64 runners")
    arm64_digest = executable_digests["ARM64"]
    x64_digest = executable_digests["X64"]
    read_role = "arn:aws:iam::123456789012:role/read"
    write_role = "arn:aws:iam::123456789012:role/write"
    assert_case(
        "main push may write",
        event_name="push",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
        expected_digest=arm64_digest,
    )
    assert_case(
        "main push without writer compiles cold",
        event_name="push",
        github_ref="refs/heads/main",
        write_role_arn="",
        expected_eligible=False,
        expected_role="",
        expected_mode="none",
    )
    assert_case(
        "main dispatch may write",
        event_name="workflow_dispatch",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
    )
    assert_case(
        "feature push cannot write",
        event_name="push",
        github_ref="refs/heads/feature",
        expected_eligible=False,
        expected_role="",
        expected_mode="none",
    )
    assert_case(
        "feature dispatch reads only",
        event_name="workflow_dispatch",
        github_ref="refs/heads/feature",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "pull request reads only",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
        runner_arch="X64",
        expected_digest=x64_digest,
    )
    assert_case(
        "merge group reads only",
        event_name="merge_group",
        github_ref="refs/heads/gh-readonly-queue/main/pr-1302",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "schedule reads only",
        event_name="schedule",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "tag push gets no role",
        event_name="push",
        github_ref="refs/tags/v1.2.3",
        expected_eligible=False,
        expected_role="",
        expected_mode="none",
    )
    assert_case(
        "inactive compiles without cache",
        active=False,
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing read role fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        read_role_arn="",
        expected_eligible=False,
        expected_role="",
        expected_mode="read_only",
    )
    assert_case(
        "invalid prefix fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "key_prefix": "sccache/bolt-v2"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing idle timeout fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={key: value for key, value in LOCATION.items() if key != "idle_timeout_seconds"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
        expected_idle_timeout=None,
    )
    assert_case(
        "location newlines are rejected",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "bucket": "bucket\nSCCACHE_REGION=evil"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing executable version fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={key: value for key, value in LOCATION.items() if key != "version"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing executable digest fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={key: value for key, value in LOCATION.items() if key != "executable_sha256"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing current architecture digest fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        runner_arch="X64",
        location={**LOCATION, "executable_sha256": {"ARM64": arm64_digest}},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing peer architecture digest fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "executable_sha256": {"ARM64": arm64_digest}},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "extra unknown architecture digest fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={
            **LOCATION,
            "executable_sha256": {
                **executable_digests,
                "RISCV64": "0" * 64,
            },
        },
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "unknown runner architecture fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        runner_arch="RISCV64",
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
        expected_digest="",
    )

    with tempfile.TemporaryDirectory() as tmp:
        output_path = pathlib.Path(tmp) / "output"
        env_path = pathlib.Path(tmp) / "env"
        common_environment = {
            "CONFIG_PATH": str(REPO_ROOT / "ci" / "sccache-location.toml"),
            "SCCACHE_ACTIVE": "true",
            "READ_ROLE_ARN": read_role,
            "WRITE_ROLE_ARN": write_role,
            "RUNNER_ARCH": "ARM64",
            "GITHUB_OUTPUT": str(output_path),
            "GITHUB_ENV": str(env_path),
        }
        common_cache_environment = {
            f"SCCACHE_BUCKET={LOCATION['bucket']}",
            f"SCCACHE_REGION={LOCATION['region']}",
            f"SCCACHE_S3_KEY_PREFIX={LOCATION['key_prefix']}",
            "SCCACHE_S3_SERVER_SIDE_ENCRYPTION=true",
            f"SCCACHE_IDLE_TIMEOUT={LOCATION['idle_timeout_seconds']}",
            "SCCACHE_IGNORE_SERVER_IO_ERROR=1",
        }
        for label, event_name, github_ref, expected_environment in (
            (
                "pull request",
                "pull_request",
                "refs/pull/1488/merge",
                common_cache_environment | {"SCCACHE_S3_RW_MODE=READ_ONLY"},
            ),
            (
                "main push",
                "push",
                "refs/heads/main",
                common_cache_environment | {"SCCACHE_S3_RW_MODE=READ_WRITE"},
            ),
            ("feature push", "push", "refs/heads/feature", set()),
        ):
            output_path.write_text("", encoding="utf-8")
            env_path.write_text("", encoding="utf-8")
            with mock.patch.dict(
                os.environ,
                {**common_environment, "GITHUB_EVENT_NAME": event_name, "GITHUB_REF": github_ref},
                clear=True,
            ), contextlib.redirect_stdout(io.StringIO()):
                if sccache_eligibility.main() != 0:
                    raise AssertionError(f"{label}: environment export failed")
            environment = set(env_path.read_text(encoding="utf-8").splitlines())
            if environment != expected_environment:
                raise AssertionError(
                    f"{label}: expected environment {expected_environment!r}, got {environment!r}"
                )

    verify = getattr(sccache_eligibility, "verify_sccache_executable", None)
    if not callable(verify):
        raise AssertionError("sccache eligibility owner must verify installed executable bytes and version")
    with tempfile.TemporaryDirectory() as tmp:
        executable = pathlib.Path(tmp) / "sccache"
        executable.write_text("#!/bin/sh\nprintf 'sccache 0.9.0\\n'\n", encoding="utf-8")
        executable.chmod(0o755)
        digest = hashlib.sha256(executable.read_bytes()).hexdigest()
        with contextlib.redirect_stdout(io.StringIO()):
            wrong_version_accepted = verify(executable, expected_version="v0.10.0", expected_sha256=digest)
        if wrong_version_accepted:
            raise AssertionError("version-compatible impostor must fail exact-version verification")

        execution_marker = pathlib.Path(tmp) / "executed"
        executable.write_text(
            "#!/bin/sh\n"
            f"touch {shlex.quote(str(execution_marker))}\n"
            "printf 'sccache 0.10.0\\n'\n",
            encoding="utf-8",
        )
        executable.chmod(0o755)
        digest = hashlib.sha256(executable.read_bytes()).hexdigest()
        with contextlib.redirect_stdout(io.StringIO()):
            wrong_digest_accepted = verify(executable, expected_version="v0.10.0", expected_sha256="0" * 64)
        if wrong_digest_accepted:
            raise AssertionError("wrong executable digest must fail verification")
        if execution_marker.exists():
            raise AssertionError("wrong executable bytes must be rejected before execution")
        if not verify(executable, expected_version="v0.10.0", expected_sha256=digest):
            raise AssertionError("matching executable bytes and exact version must pass verification")
    print("OK: sccache eligibility self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
