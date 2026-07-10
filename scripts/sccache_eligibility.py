#!/usr/bin/env python3
"""Resolve governed sccache eligibility for GitHub Actions."""

from __future__ import annotations

import os
import pathlib
import tomllib
from dataclasses import dataclass
from typing import Mapping


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
    eligible = active and vars_present and location_valid
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
    )


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
        location=_load_location(config_path),
    )

    output_path = os.environ["GITHUB_OUTPUT"]
    _write_line(output_path, f"eligible={str(eligibility.eligible).lower()}")
    _write_line(output_path, f"role_arn={eligibility.role_arn}")
    _write_line(output_path, f"region={eligibility.region}")
    _write_line(output_path, f"cache_mode={eligibility.cache_mode}")

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
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
