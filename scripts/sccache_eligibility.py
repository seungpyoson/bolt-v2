#!/usr/bin/env python3
"""Resolve governed sccache eligibility for GitHub Actions."""

from __future__ import annotations

import os
import pathlib
import re
import stat
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from typing import Mapping, Sequence


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
VERSION_RE = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
ASSET_TARGET_RE = re.compile(r"^[a-z0-9_]+(?:-[a-z0-9_]+)+$")
EXECUTABLE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


class SccacheConfigError(ValueError):
    """The repository-owned sccache configuration is not exact."""


class SccacheStrictError(RuntimeError):
    """A mandatory producer sccache precondition failed."""


@dataclass(frozen=True)
class SccacheConfig:
    location: Mapping[str, object]
    version: str
    asset_target: str
    executable: str
    version_output: str
    executable_sha256: str


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


@dataclass(frozen=True)
class SccacheWrapperIdentity:
    path: pathlib.Path
    device: int
    inode: int
    digest: str


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
    if type(config["schema_version"]) is not int or config["schema_version"] != 2:
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
        {"version", "asset_target", "executable", "version_output", "executable_sha256"},
        f"{label}.installer",
    )
    location_values = {key: _location_value(location, key) for key in location}
    if not all(location_values.values()) or not location_values["key_prefix"].endswith("/"):
        raise SccacheConfigError(f"{label}.location contains an invalid value")
    installer_values: dict[str, str] = {}
    for key in ("version", "asset_target", "executable", "version_output", "executable_sha256"):
        value = installer[key]
        if not isinstance(value, str) or not value or value != value.strip() or "\n" in value or "\r" in value:
            raise SccacheConfigError(f"{label}.installer.{key} must be a non-empty single-line string")
        installer_values[key] = value
    if not VERSION_RE.fullmatch(installer_values["version"]):
        raise SccacheConfigError(f"{label}.installer.version must be a pinned semantic version")
    if not ASSET_TARGET_RE.fullmatch(installer_values["asset_target"]):
        raise SccacheConfigError(f"{label}.installer.asset_target must be a safe target triple")
    if not EXECUTABLE_RE.fullmatch(installer_values["executable"]):
        raise SccacheConfigError(f"{label}.installer.executable must be a safe executable name")
    if not SHA256_RE.fullmatch(installer_values["executable_sha256"]):
        raise SccacheConfigError(
            f"{label}.installer.executable_sha256 must be lowercase SHA-256"
        )
    return SccacheConfig(location=location_values, **installer_values)


def load_sccache_config(config_path: pathlib.Path) -> SccacheConfig:
    try:
        text = config_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise SccacheConfigError(f"unable to read {config_path}: {exc}") from exc
    return parse_sccache_config(text, label=str(config_path))


def _fd_sha256(fd: int) -> str:
    import hashlib

    digest = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while chunk := os.read(fd, 1024 * 1024):
        digest.update(chunk)
    return digest.hexdigest()


def _run_text(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def _candidate_path(config: SccacheConfig, candidate: str) -> pathlib.Path:
    if (
        not candidate
        or not pathlib.Path(candidate).is_absolute()
        or candidate.strip() != candidate
        or any(char.isspace() for char in candidate)
    ):
        raise SccacheStrictError("installed sccache path is not an absolute executable file")
    wrapper = pathlib.Path(candidate)
    if wrapper.name != config.executable:
        raise SccacheStrictError("installed sccache path does not identify the configured executable")
    return wrapper


def _open_wrapper(wrapper: pathlib.Path) -> tuple[int, os.stat_result]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(wrapper, flags)
    except OSError as exc:
        raise SccacheStrictError("installed sccache path is not an absolute executable file") from exc
    metadata = os.fstat(fd)
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o111 == 0:
        os.close(fd)
        raise SccacheStrictError("installed sccache path is not an absolute executable file")
    return fd, metadata


def validate_wrapper_identity(
    config: SccacheConfig,
    path: pathlib.Path,
    *,
    device: int,
    inode: int,
    digest: str,
) -> SccacheWrapperIdentity:
    if digest != config.executable_sha256 or path.name != config.executable:
        raise SccacheStrictError("installed sccache identity does not match repository config")
    fd, metadata = _open_wrapper(path)
    try:
        actual_digest = _fd_sha256(fd)
    finally:
        os.close(fd)
    if (metadata.st_dev, metadata.st_ino, actual_digest) != (device, inode, digest):
        raise SccacheStrictError("installed sccache device, inode, or digest changed")
    return SccacheWrapperIdentity(path=path, device=device, inode=inode, digest=digest)


def promote_installed_wrapper(config: SccacheConfig, candidate: str) -> SccacheWrapperIdentity:
    wrapper = _candidate_path(config, candidate)
    fd, metadata = _open_wrapper(wrapper)
    try:
        digest = _fd_sha256(fd)
    finally:
        os.close(fd)
    if digest != config.executable_sha256:
        raise SccacheStrictError("installed sccache executable digest does not match repository config")
    host = _run_text(["uname", "-m"])
    expected_machine = config.asset_target.split("-", 1)[0]
    if host.returncode != 0 or host.stdout.strip() != expected_machine:
        raise SccacheStrictError("installed sccache target does not match the producer host")

    private_root = pathlib.Path(
        tempfile.mkdtemp(prefix=f".{config.executable}-live-", dir=wrapper.parent)
    )
    content_root = private_root / digest
    content_root.mkdir(mode=0o700)
    live_wrapper = content_root / config.executable
    try:
        os.rename(wrapper, live_wrapper)
    except OSError as exc:
        raise SccacheStrictError("approved sccache inode could not be atomically promoted") from exc
    identity = validate_wrapper_identity(
        config,
        live_wrapper,
        device=metadata.st_dev,
        inode=metadata.st_ino,
        digest=digest,
    )
    if os.path.lexists(wrapper):
        raise SccacheStrictError("installed sccache path remained after promotion")
    version = _run_text([str(live_wrapper), "--version"])
    if version.returncode != 0 or version.stdout.strip() != config.version_output:
        raise SccacheStrictError("installed sccache version does not match repository config")
    return validate_wrapper_identity(
        config,
        identity.path,
        device=identity.device,
        inode=identity.inode,
        digest=identity.digest,
    )


def _required_outcome(name: str) -> None:
    if os.environ.get(name, "") != "success":
        raise SccacheStrictError("root-artifact requires successful sccache setup outcomes")


def strict_setup_main() -> int:
    try:
        _required_outcome("ELIGIBILITY_OUTCOME")
        _required_outcome("AWS_OUTCOME")
        _required_outcome("INSTALL_OUTCOME")
        if os.environ.get("SCCACHE_ELIGIBLE", "") != "true":
            raise SccacheStrictError("root-artifact is not eligible for mandatory sccache")
        config = load_sccache_config(pathlib.Path(os.environ["CONFIG_PATH"]))
        identity = promote_installed_wrapper(config, os.environ.get("SCCACHE_PATH", ""))
        start = _run_text([str(identity.path), "--start-server"])
        if start.returncode != 0:
            raise SccacheStrictError("mandatory sccache server failed to start")
        identity = validate_wrapper_identity(
            config,
            identity.path,
            device=identity.device,
            inode=identity.inode,
            digest=identity.digest,
        )
        _run_text([str(identity.path), "--zero-stats"])
        _write_line(os.environ["GITHUB_OUTPUT"], f"wrapper_path={identity.path}")
        _write_line(os.environ["GITHUB_OUTPUT"], f"wrapper_device={identity.device}")
        _write_line(os.environ["GITHUB_OUTPUT"], f"wrapper_inode={identity.inode}")
        _write_line(os.environ["GITHUB_OUTPUT"], f"wrapper_digest={identity.digest}")
        _write_line(os.environ["GITHUB_OUTPUT"], "enabled=true")
        _write_line(os.environ["GITHUB_ENV"], f"SCCACHE_PATH={identity.path}")
        _write_line(os.environ["GITHUB_ENV"], "BOLT_RUST_VERIFICATION_SCCACHE=1")
    except (KeyError, OSError, SccacheConfigError, SccacheStrictError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return 0


def legacy_enable_main() -> int:
    enabled = False
    if (
        os.environ.get("SCCACHE_ELIGIBLE", "") == "true"
        and os.environ.get("AWS_OUTCOME", "") == "success"
        and os.environ.get("INSTALL_OUTCOME", "") == "success"
        and os.environ.get("SCCACHE_PATH", "")
    ):
        wrapper = os.environ["SCCACHE_PATH"]
        if _run_text([wrapper, "--start-server"]).returncode == 0:
            _run_text([wrapper, "--zero-stats"])
            enabled = True
        else:
            print("sccache server failed to start; building without cache")
    _write_line(os.environ["GITHUB_OUTPUT"], f"enabled={str(enabled).lower()}")
    print(f"sccache enabled={str(enabled).lower()}")
    return 0


def eligibility_main() -> int:
    config_path = pathlib.Path(os.environ["CONFIG_PATH"])
    output_path = os.environ["GITHUB_OUTPUT"]
    try:
        config = load_sccache_config(config_path)
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
    _write_line(output_path, f"config_path={config_path.resolve()}")
    _write_line(output_path, f"installer_version={config.version}")
    _write_line(output_path, f"installer_asset_target={config.asset_target}")
    _write_line(output_path, f"installer_executable={config.executable}")
    _write_line(output_path, f"installer_version_output={config.version_output}")
    _write_line(output_path, f"installer_executable_sha256={config.executable_sha256}")

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


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args:
        return eligibility_main()
    if args == ["strict-setup"]:
        return strict_setup_main()
    if args == ["legacy-enable"]:
        return legacy_enable_main()
    print("usage: sccache_eligibility.py [strict-setup|legacy-enable]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
