#!/usr/bin/env python3
"""Resolve governed sccache eligibility for GitHub Actions."""

from __future__ import annotations

import hashlib
import hmac
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass


@dataclass(frozen=True)
class SccacheEligibility:
    eligible: bool
    role_arn: str
    region: str
    cache_mode: str
    bucket: str
    key_prefix: str
    version: str
    idle_timeout_seconds: int | None
    executable_sha256: str
    active: bool
    vars_present: bool
    location_valid: bool


def _location_value(location: Mapping[str, object], key: str) -> str:
    value = location.get(key)
    if not isinstance(value, str) or not value:
        return ""
    if "\n" in value or "\r" in value:
        return ""
    return value


def _architecture_digest(location: Mapping[str, object], runner_arch: str) -> tuple[str, bool]:
    raw_digests = location.get("executable_sha256")
    if not isinstance(raw_digests, Mapping) or set(raw_digests) != {"ARM64", "X64"}:
        return "", False
    digests_valid = all(
        isinstance(arch, str)
        and re.fullmatch(r"[A-Z0-9_]+", arch)
        and isinstance(digest, str)
        and re.fullmatch(r"[0-9a-f]{64}", digest)
        for arch, digest in raw_digests.items()
    )
    selected = raw_digests.get(runner_arch)
    return (selected if isinstance(selected, str) else ""), digests_valid


def resolve_sccache_eligibility(
    *,
    active: bool,
    event_name: str,
    github_ref: str,
    read_role_arn: str,
    write_role_arn: str,
    runner_arch: str,
    location: Mapping[str, object],
) -> SccacheEligibility:
    write_requested = bool(write_role_arn)
    trusted_write = write_requested and (
        (event_name == "push" and github_ref == "refs/heads/main")
        or (event_name == "workflow_dispatch" and github_ref == "refs/heads/main")
    )
    read_allowed = event_name in {"pull_request", "merge_group", "workflow_dispatch", "schedule"}
    if trusted_write:
        role_arn = write_role_arn
        cache_mode = "read_write"
    elif read_allowed:
        role_arn = read_role_arn
        cache_mode = "read_only"
    else:
        role_arn = ""
        cache_mode = "none"

    bucket = _location_value(location, "bucket")
    region = _location_value(location, "region")
    key_prefix = _location_value(location, "key_prefix")
    version = _location_value(location, "version")
    raw_idle_timeout = location.get("idle_timeout_seconds")
    idle_timeout_seconds = (
        raw_idle_timeout
        if isinstance(raw_idle_timeout, int)
        and not isinstance(raw_idle_timeout, bool)
        and raw_idle_timeout >= 0
        else None
    )
    executable_sha256, architecture_digests_valid = _architecture_digest(location, runner_arch)
    vars_present = all((role_arn, bucket, region, key_prefix, version, executable_sha256))
    location_valid = bool(
        bucket
        and region
        and key_prefix.endswith("/")
        and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", version)
        and idle_timeout_seconds is not None
        and architecture_digests_valid
        and re.fullmatch(r"[A-Z0-9_]+", runner_arch)
        and executable_sha256
    )
    eligible = active and vars_present and location_valid
    return SccacheEligibility(
        eligible=eligible,
        role_arn=role_arn,
        region=region,
        cache_mode=cache_mode,
        bucket=bucket,
        key_prefix=key_prefix,
        version=version,
        idle_timeout_seconds=idle_timeout_seconds,
        executable_sha256=executable_sha256,
        active=active,
        vars_present=vars_present,
        location_valid=location_valid,
    )


def verify_sccache_executable(
    executable: pathlib.Path,
    *,
    expected_version: str,
    expected_sha256: str,
) -> bool:
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", expected_version):
        print("::warning::configured sccache version is invalid")
        return False
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        print("::warning::configured sccache executable digest is invalid")
        return False
    if not executable.is_file() or executable.is_symlink():
        print("::warning::installed sccache path is not a regular file")
        return False

    digest = hashlib.sha256()
    try:
        with executable.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as exc:
        print(f"::warning::unable to hash installed sccache: {exc}")
        return False
    if not hmac.compare_digest(digest.hexdigest(), expected_sha256):
        print("::warning::installed sccache executable digest does not match governed bytes")
        return False

    try:
        result = subprocess.run(
            [str(executable), "--version"],
            check=False,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        print(f"::warning::unable to query installed sccache version: {exc}")
        return False
    expected_output = f"sccache {expected_version.removeprefix('v')}"
    if result.returncode != 0 or result.stdout.strip() != expected_output:
        print("::warning::installed sccache version does not match governed version")
        return False
    return True


def _write_line(path: str, line: str) -> None:
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(f"{line}\n")


def _load_location(config_path: pathlib.Path) -> Mapping[str, object]:
    try:
        config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except OSError as exc:
        print(f"::warning::unable to read {config_path}: {exc}; compiling without sccache")
        return {}
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        print(f"::warning::{config_path} is invalid TOML: {exc}; compiling without sccache")
        return {}
    location = config.get("location") if isinstance(config, dict) else None
    return location if isinstance(location, dict) else {}


def main() -> int:
    config_path = pathlib.Path(os.environ["CONFIG_PATH"])
    eligibility = resolve_sccache_eligibility(
        active=os.environ.get("SCCACHE_ACTIVE") == "true",
        event_name=os.environ.get("GITHUB_EVENT_NAME", ""),
        github_ref=os.environ.get("GITHUB_REF", ""),
        read_role_arn=os.environ.get("READ_ROLE_ARN", ""),
        write_role_arn=os.environ.get("WRITE_ROLE_ARN", ""),
        runner_arch=os.environ.get("RUNNER_ARCH", ""),
        location=_load_location(config_path),
    )

    output_path = os.environ["GITHUB_OUTPUT"]
    _write_line(output_path, f"eligible={str(eligibility.eligible).lower()}")
    _write_line(output_path, f"role_arn={eligibility.role_arn}")
    _write_line(output_path, f"region={eligibility.region}")
    _write_line(output_path, f"cache_mode={eligibility.cache_mode}")
    _write_line(output_path, f"version={eligibility.version}")
    _write_line(output_path, f"executable_sha256={eligibility.executable_sha256}")

    if eligibility.eligible:
        env_path = os.environ["GITHUB_ENV"]
        _write_line(env_path, f"SCCACHE_BUCKET={eligibility.bucket}")
        _write_line(env_path, f"SCCACHE_REGION={eligibility.region}")
        _write_line(env_path, f"SCCACHE_S3_KEY_PREFIX={eligibility.key_prefix}")
        _write_line(env_path, "SCCACHE_S3_SERVER_SIDE_ENCRYPTION=true")
        _write_line(env_path, f"SCCACHE_IDLE_TIMEOUT={eligibility.idle_timeout_seconds}")
        _write_line(env_path, "SCCACHE_IGNORE_SERVER_IO_ERROR=1")

    print(
        "sccache cache "
        f"active={str(eligibility.active).lower()} "
        f"event={os.environ.get('GITHUB_EVENT_NAME', '')} "
        f"ref={os.environ.get('GITHUB_REF', '')} "
        f"cache_mode={eligibility.cache_mode} "
        f"vars_present={str(eligibility.vars_present).lower()} "
        f"location_valid={str(eligibility.location_valid).lower()}"
    )
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "verify-executable":
        raise SystemExit(
            0
            if verify_sccache_executable(
                pathlib.Path(sys.argv[2]),
                expected_version=sys.argv[3],
                expected_sha256=sys.argv[4],
            )
            else 1
        )
    if len(sys.argv) != 1:
        print("usage: sccache_eligibility.py [verify-executable PATH VERSION SHA256]", file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(main())
