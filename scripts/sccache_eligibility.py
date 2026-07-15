#!/usr/bin/env python3
"""Resolve governed sccache eligibility for GitHub Actions."""

from __future__ import annotations

import os
import pathlib
import re
import tomllib
from dataclasses import dataclass
from typing import Mapping


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class SccacheConfigError(ValueError):
    """The repository-owned sccache configuration is not exact."""


@dataclass(frozen=True)
class SccacheConfig:
    location: Mapping[str, object]
    version: str
    target: str
    executable: str
    version_output: str
    sha256: str


@dataclass(frozen=True)
class SccacheEligibility:
    eligible: bool
    role_arn: str
    region: str
    cache_mode: str
    bucket: str
    key_prefix: str
    active: bool
    vars_present: bool
    location_valid: bool
    strict_context_valid: bool


def _location_value(location: Mapping[str, object], key: str) -> str:
    value = location.get(key)
    if not isinstance(value, str) or not value:
        return ""
    if "\n" in value or "\r" in value:
        return ""
    return value


def resolve_sccache_eligibility(
    *,
    active: bool,
    event_name: str,
    github_ref: str,
    read_role_arn: str,
    write_role_arn: str,
    location: Mapping[str, object],
    required: bool = False,
    operation: str = "",
    github_sha: str = "",
    expected_sha: str = "",
    checked_out_sha: str = "",
    remote_main_sha: str = "",
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
    vars_present = all((role_arn, bucket, region, key_prefix))
    location_valid = bool(bucket and region and key_prefix.endswith("/"))
    strict_shas = (github_sha, expected_sha, checked_out_sha, remote_main_sha)
    strict_context_valid = not required or (
        operation == "root-artifact"
        and event_name == "workflow_dispatch"
        and github_ref == "refs/heads/main"
        and all(GIT_SHA_RE.fullmatch(value) for value in strict_shas)
        and len(set(strict_shas)) == 1
        and cache_mode == "read_write"
    )
    eligible = active and vars_present and location_valid and strict_context_valid
    return SccacheEligibility(
        eligible=eligible,
        role_arn=role_arn,
        region=region,
        cache_mode=cache_mode,
        bucket=bucket,
        key_prefix=key_prefix,
        active=active,
        vars_present=vars_present,
        location_valid=location_valid,
        strict_context_valid=strict_context_valid,
    )


def _write_line(path: str, line: str) -> None:
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(f"{line}\n")


def _exact_keys(table: Mapping[str, object], expected: set[str], label: str) -> None:
    missing = sorted(expected - set(table))
    unknown = sorted(set(table) - expected)
    if missing:
        raise SccacheConfigError(f"{label} missing keys: {missing!r}")
    if unknown:
        raise SccacheConfigError(f"{label} contains unknown keys: {unknown!r}")


def parse_sccache_config(text: str, *, label: str) -> SccacheConfig:
    try:
        config = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise SccacheConfigError(f"{label} is invalid TOML: {exc}") from exc
    _exact_keys(config, {"schema_version", "location", "installer"}, label)
    if config["schema_version"] != 2:
        raise SccacheConfigError(f"{label} schema_version must be 2")
    location = config["location"]
    installer = config["installer"]
    if not isinstance(location, dict):
        raise SccacheConfigError(f"{label}.location must be a table")
    if not isinstance(installer, dict):
        raise SccacheConfigError(f"{label}.installer must be a table")
    _exact_keys(location, {"bucket", "region", "key_prefix"}, f"{label}.location")
    _exact_keys(
        installer,
        {"version", "target", "executable", "version_output", "sha256"},
        f"{label}.installer",
    )
    location_values = {key: _location_value(location, key) for key in location}
    if not all(location_values.values()) or not location_values["key_prefix"].endswith("/"):
        raise SccacheConfigError(f"{label}.location contains an invalid value")
    installer_values: dict[str, str] = {}
    for key in ("version", "target", "executable", "version_output", "sha256"):
        value = installer[key]
        if not isinstance(value, str) or not value or value != value.strip() or "\n" in value or "\r" in value:
            raise SccacheConfigError(f"{label}.installer.{key} must be a non-empty single-line string")
        installer_values[key] = value
    if installer_values["version"] != "v0.10.0":
        raise SccacheConfigError(f"{label}.installer.version must be v0.10.0")
    if installer_values["target"] != "aarch64-unknown-linux-gnu":
        raise SccacheConfigError(f"{label}.installer.target must be aarch64-unknown-linux-gnu")
    if installer_values["executable"] != "sccache":
        raise SccacheConfigError(f"{label}.installer.executable must be sccache")
    if installer_values["version_output"] != "sccache 0.10.0":
        raise SccacheConfigError(f"{label}.installer.version_output must identify configured version")
    if not SHA256_RE.fullmatch(installer_values["sha256"]):
        raise SccacheConfigError(f"{label}.installer.sha256 must be lowercase SHA-256")
    return SccacheConfig(location=location_values, **installer_values)


def _load_config(config_path: pathlib.Path) -> SccacheConfig:
    try:
        text = config_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise SccacheConfigError(f"unable to read {config_path}: {exc}") from exc
    return parse_sccache_config(text, label=str(config_path))


def main() -> int:
    config_path = pathlib.Path(os.environ["CONFIG_PATH"])
    output_path = os.environ["GITHUB_OUTPUT"]
    try:
        config = _load_config(config_path)
    except SccacheConfigError as exc:
        _write_line(output_path, "eligible=false")
        _write_line(output_path, "cache_mode=none")
        print(f"::warning::{exc}; compiling without sccache")
        return 2
    eligibility = resolve_sccache_eligibility(
        active=os.environ.get("SCCACHE_ACTIVE") == "true",
        event_name=os.environ.get("GITHUB_EVENT_NAME", ""),
        github_ref=os.environ.get("GITHUB_REF", ""),
        read_role_arn=os.environ.get("READ_ROLE_ARN", ""),
        write_role_arn=os.environ.get("WRITE_ROLE_ARN", ""),
        location=config.location,
        required=os.environ.get("SCCACHE_REQUIRED") == "true",
        operation=os.environ.get("SCCACHE_OPERATION", ""),
        github_sha=os.environ.get("GITHUB_SHA", ""),
        expected_sha=os.environ.get("SCCACHE_EXPECTED_SHA", ""),
        checked_out_sha=os.environ.get("SCCACHE_CHECKED_OUT_SHA", ""),
        remote_main_sha=os.environ.get("SCCACHE_REMOTE_MAIN_SHA", ""),
    )

    _write_line(output_path, f"eligible={str(eligibility.eligible).lower()}")
    _write_line(output_path, f"role_arn={eligibility.role_arn}")
    _write_line(output_path, f"region={eligibility.region}")
    _write_line(output_path, f"cache_mode={eligibility.cache_mode}")
    _write_line(output_path, f"installer_version={config.version}")
    _write_line(output_path, f"installer_target={config.target}")
    _write_line(output_path, f"installer_executable={config.executable}")
    _write_line(output_path, f"installer_version_output={config.version_output}")
    _write_line(output_path, f"installer_sha256={config.sha256}")

    if eligibility.eligible:
        env_path = os.environ["GITHUB_ENV"]
        _write_line(env_path, f"SCCACHE_BUCKET={eligibility.bucket}")
        _write_line(env_path, f"SCCACHE_REGION={eligibility.region}")
        _write_line(env_path, f"SCCACHE_S3_KEY_PREFIX={eligibility.key_prefix}")
        _write_line(env_path, "SCCACHE_S3_SERVER_SIDE_ENCRYPTION=true")
        _write_line(env_path, "SCCACHE_IGNORE_SERVER_IO_ERROR=1")

    print(
        "sccache cache "
        f"active={str(eligibility.active).lower()} "
        f"event={os.environ.get('GITHUB_EVENT_NAME', '')} "
        f"ref={os.environ.get('GITHUB_REF', '')} "
        f"cache_mode={eligibility.cache_mode} "
        f"vars_present={str(eligibility.vars_present).lower()} "
        f"location_valid={str(eligibility.location_valid).lower()}"
        f" strict_context_valid={str(eligibility.strict_context_valid).lower()}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
