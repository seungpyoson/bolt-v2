#!/usr/bin/env python3
"""Validate strict-sccache build metadata and release evidence."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import ipaddress
import json
import pathlib
import re
import sys
import tomllib
import urllib.parse
from collections.abc import Mapping
from types import MappingProxyType
from typing import Any


_HEX_40 = re.compile(r"[0-9a-f]{40}\Z")
_HEX_64 = re.compile(r"[0-9a-f]{64}\Z")
_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")
_TARGET_NAMES = frozenset({"ARM64", "X64"})


@dataclasses.dataclass(frozen=True)
class TargetConfig:
    triple: str
    elf_machine: str


@dataclasses.dataclass(frozen=True)
class VerificationConsumerConfig:
    abstract_socket_template: str
    compiler_path: str
    compiler_family: str
    s3_bucket: str
    s3_region: str
    s3_key_prefix: str
    s3_endpoint_prefix: str
    s3_no_credentials: bool
    s3_use_ssl: bool
    s3_enable_virtual_host_style: bool


@dataclasses.dataclass(frozen=True)
class PublisherConfig:
    environment: str
    api_version: str
    gh_version: str
    gh_archive_url: str
    gh_archive_sha256: str
    gh_archive_member: str
    releases_per_page: int
    max_release_pages: int


@dataclasses.dataclass(frozen=True)
class StrictBuildConfig:
    schema_version: int
    repo_root: pathlib.Path
    source_version: str
    source_commit: str
    source_url: str
    source_sha256: str
    source_date_epoch: int
    patch_path: pathlib.Path
    workflow_path: pathlib.Path
    recipe_path: pathlib.Path
    driver_path: pathlib.Path
    container: str
    container_digest: str
    rustc_release: str
    rustc_commit: str
    features: tuple[str, ...]
    default_features: bool
    profile: str
    verification_timeout_ms: int
    max_frame_bytes: int
    max_runtime_timeout_ms: int
    snapshot_max_bytes: int
    abstract_name_max_bytes: int
    cache_format_token: str
    cache_compression_level: int
    verification_cache_mode: str
    replicas: tuple[str, ...]
    attestation_attempts: int
    attestation_interval_seconds: int
    attestation_max_wait_seconds: int
    verification_consumer: VerificationConsumerConfig
    publisher: PublisherConfig
    targets: Mapping[str, TargetConfig]


CandidateManifest = dict[str, object]


def gnu_timeout_duration(milliseconds: int) -> str:
    """Render governed milliseconds as an exact GNU timeout duration."""
    if isinstance(milliseconds, bool) or not isinstance(milliseconds, int):
        raise ValueError("timeout milliseconds must be an integer")
    if milliseconds <= 0:
        raise ValueError("timeout milliseconds must be positive")
    seconds, remainder = divmod(milliseconds, 1_000)
    if remainder == 0:
        return f"{seconds}s"
    fraction = f"{remainder:03d}".rstrip("0")
    return f"{seconds}.{fraction}s"


@dataclasses.dataclass(frozen=True)
class VerifiedAsset:
    architecture: str
    target: str
    name: str
    sha256: str
    size: int
    path: pathlib.Path


@dataclasses.dataclass(frozen=True)
class VerifiedCandidateSet:
    repository: str
    run_id: str
    run_attempt: str
    head_sha: str
    source_version: str
    source_commit: str
    source_sha256: str
    source_date_epoch: int
    patch_sha256: str
    container: str
    rustc_release: str
    rustc_commit: str
    features: tuple[str, ...]
    default_features: bool
    profile: str
    build_identity_sha256: str
    release_tag: str
    assets: Mapping[str, VerifiedAsset]
    provenance_name: str
    provenance_bytes: bytes
    provenance_sha256: str


def _mapping(value: object, name: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{name} must be a table")
    if not all(isinstance(key, str) for key in value):
        raise ValueError(f"{name} keys must be strings")
    return value


def _exact_keys(
    value: Mapping[str, object], expected: frozenset[str], name: str
) -> None:
    missing = expected - value.keys()
    unknown = value.keys() - expected
    if missing:
        raise ValueError(f"missing {name} key: {sorted(missing)[0]}")
    if unknown:
        raise ValueError(f"unknown {name} key: {sorted(unknown)[0]}")


def _string(value: object, name: str) -> str:
    if not isinstance(value, str) or not value or value.strip() != value:
        raise ValueError(f"{name} must be a non-empty trimmed string")
    return value


def _full_sha(value: object, name: str) -> str:
    text = _string(value, name)
    if not _HEX_40.fullmatch(text):
        raise ValueError(f"{name} must be a full lowercase commit SHA")
    return text


def _sha256(value: object, name: str) -> str:
    text = _string(value, name)
    if not _HEX_64.fullmatch(text):
        raise ValueError(f"{name} must be a lowercase SHA-256")
    return text


def load_document(
    document: Mapping[str, object], *, repo_root: pathlib.Path
) -> StrictBuildConfig:
    top = _mapping(document, "document")
    _exact_keys(
        top,
        frozenset(
            {
                "schema_version",
                "source",
                "build",
                "verification",
                "runtime_contract",
                "publisher",
                "targets",
            }
        ),
        "top-level",
    )
    schema_version = top["schema_version"]
    if type(schema_version) is not int or schema_version != 1:
        raise ValueError("schema_version must be integer 1")

    source = _mapping(top["source"], "source")
    _exact_keys(
        source,
        frozenset(
            {
                "version",
                "commit",
                "archive_url",
                "archive_sha256",
                "source_date_epoch",
                "patch",
            }
        ),
        "source",
    )
    source_version = _string(source["version"], "source.version")
    if not _VERSION.fullmatch(source_version):
        raise ValueError("source.version must be semantic version X.Y.Z")
    source_commit = _full_sha(source["commit"], "source.commit")
    source_url = _string(source["archive_url"], "source.archive_url")
    if not source_url.startswith("https://"):
        raise ValueError("source.archive_url must use HTTPS")
    source_sha256 = _sha256(source["archive_sha256"], "source.archive_sha256")
    source_date_epoch = source["source_date_epoch"]
    if (
        isinstance(source_date_epoch, bool)
        or not isinstance(source_date_epoch, int)
        or source_date_epoch <= 0
    ):
        raise ValueError("source_date_epoch must be a positive integer")
    patch_relative = pathlib.PurePosixPath(_string(source["patch"], "source.patch"))
    if (
        patch_relative.is_absolute()
        or ".." in patch_relative.parts
        or patch_relative.suffix != ".patch"
    ):
        raise ValueError("source.patch must be a repository-relative patch path")
    patch_path = repo_root.joinpath(*patch_relative.parts)

    build = _mapping(top["build"], "build")
    _exact_keys(
        build,
        frozenset(
            {
                "container",
                "rustc_release",
                "rustc_commit",
                "features",
                "default_features",
                "profile",
                "workflow",
                "recipe",
                "driver",
            }
        ),
        "build",
    )
    container = _string(build["container"], "build.container")
    container_match = re.fullmatch(r"[^@\s]+@(sha256:([0-9a-f]{64}))", container)
    if container_match is None:
        raise ValueError("container must use sha256 digest")
    container_digest = container_match.group(1)
    rustc_release = _string(build["rustc_release"], "build.rustc_release")
    if not _VERSION.fullmatch(rustc_release):
        raise ValueError("build.rustc_release must be semantic version X.Y.Z")
    rustc_commit = _full_sha(build["rustc_commit"], "build.rustc_commit")
    features_value = build["features"]
    if not isinstance(features_value, list) or features_value != [
        "s3",
        "vendored-openssl",
    ]:
        raise ValueError("build.features must be exactly ['s3', 'vendored-openssl']")
    default_features = build["default_features"]
    if default_features is not False:
        raise ValueError("build.default_features must be false")
    profile = _string(build["profile"], "build.profile")
    if profile != "release":
        raise ValueError("build.profile must be release")
    workflow_relative = pathlib.PurePosixPath(
        _string(build["workflow"], "build.workflow")
    )
    if (
        workflow_relative.is_absolute()
        or ".." in workflow_relative.parts
        or workflow_relative.suffix not in {".yml", ".yaml"}
    ):
        raise ValueError("build.workflow must be a repository-relative workflow path")
    workflow_path = repo_root.joinpath(*workflow_relative.parts)
    recipe_relative = pathlib.PurePosixPath(_string(build["recipe"], "build.recipe"))
    if (
        recipe_relative.is_absolute()
        or ".." in recipe_relative.parts
        or recipe_relative.suffix != ".sh"
    ):
        raise ValueError("build.recipe must be a repository-relative shell recipe")
    recipe_path = repo_root.joinpath(*recipe_relative.parts)
    driver_relative = pathlib.PurePosixPath(_string(build["driver"], "build.driver"))
    if (
        driver_relative.is_absolute()
        or ".." in driver_relative.parts
        or driver_relative.suffix != ".py"
    ):
        raise ValueError("build.driver must be a repository-relative Python path")
    driver_path = repo_root.joinpath(*driver_relative.parts)

    verification = _mapping(top["verification"], "verification")
    _exact_keys(
        verification,
        frozenset(
            {
                "strict_timeout_ms",
                "max_frame_bytes",
                "cache_mode",
                "replicas",
                "attestation_attempts",
                "attestation_interval_seconds",
                "attestation_max_wait_seconds",
                "consumer",
            }
        ),
        "verification",
    )
    verification_timeout_ms = verification["strict_timeout_ms"]
    if (
        isinstance(verification_timeout_ms, bool)
        or not isinstance(verification_timeout_ms, int)
        or verification_timeout_ms <= 0
    ):
        raise ValueError("verification.strict_timeout_ms must be a positive integer")
    max_frame_bytes = verification["max_frame_bytes"]
    if (
        isinstance(max_frame_bytes, bool)
        or not isinstance(max_frame_bytes, int)
        or max_frame_bytes <= 0
        or max_frame_bytes > 2**32 - 1
    ):
        raise ValueError(
            "verification.max_frame_bytes must be a positive 32-bit integer"
        )
    runtime_contract = _mapping(top["runtime_contract"], "runtime_contract")
    _exact_keys(
        runtime_contract,
        frozenset(
            {
                "max_runtime_timeout_ms",
                "snapshot_max_bytes",
                "abstract_name_max_bytes",
                "cache_format_token",
                "cache_compression_level",
            }
        ),
        "runtime_contract",
    )
    max_runtime_timeout_ms = runtime_contract["max_runtime_timeout_ms"]
    snapshot_max_bytes = runtime_contract["snapshot_max_bytes"]
    abstract_name_max_bytes = runtime_contract["abstract_name_max_bytes"]
    cache_format_token = _string(
        runtime_contract["cache_format_token"],
        "runtime_contract.cache_format_token",
    )
    cache_compression_level = runtime_contract["cache_compression_level"]
    if (
        isinstance(cache_compression_level, bool)
        or not isinstance(cache_compression_level, int)
        or not 1 <= cache_compression_level <= 22
    ):
        raise ValueError(
            "runtime_contract.cache_compression_level must be between 1 and 22"
        )
    if (
        isinstance(max_runtime_timeout_ms, bool)
        or not isinstance(max_runtime_timeout_ms, int)
        or max_runtime_timeout_ms < verification_timeout_ms
    ):
        raise ValueError(
            "runtime_contract.max_runtime_timeout_ms must be an integer no smaller than the verification timeout"
        )
    if (
        isinstance(snapshot_max_bytes, bool)
        or not isinstance(snapshot_max_bytes, int)
        or snapshot_max_bytes <= 0
        or snapshot_max_bytes > max_frame_bytes
    ):
        raise ValueError(
            "runtime_contract.snapshot_max_bytes must be a positive integer no larger than max_frame_bytes"
        )
    if (
        isinstance(abstract_name_max_bytes, bool)
        or not isinstance(abstract_name_max_bytes, int)
        or abstract_name_max_bytes <= 0
        or abstract_name_max_bytes > 107
    ):
        raise ValueError(
            "runtime_contract.abstract_name_max_bytes must fit Linux sockaddr_un"
        )
    if (
        len(cache_format_token) > 64
        or not cache_format_token[0].isalnum()
        or any(
            not (character.isascii() and (character.isalnum() or character in "._-"))
            for character in cache_format_token
        )
    ):
        raise ValueError(
            "runtime_contract.cache_format_token must be a bounded portable identifier"
        )
    verification_cache_mode = _string(
        verification["cache_mode"], "verification.cache_mode"
    )
    if verification_cache_mode not in {"READ_ONLY", "READ_WRITE"}:
        raise ValueError("verification.cache_mode must be READ_ONLY or READ_WRITE")
    consumer = _mapping(verification["consumer"], "verification.consumer")
    _exact_keys(
        consumer,
        frozenset(
            {
                "abstract_socket_template",
                "compiler_path",
                "compiler_family",
                "s3_bucket",
                "s3_region",
                "s3_key_prefix",
                "s3_endpoint_prefix",
                "s3_no_credentials",
                "s3_use_ssl",
                "s3_enable_virtual_host_style",
            }
        ),
        "verification.consumer",
    )
    abstract_socket_template = _string(
        consumer["abstract_socket_template"],
        "verification.consumer.abstract_socket_template",
    )
    if (
        abstract_socket_template.count("{job_identity}") != 1
        or not abstract_socket_template.isascii()
        or len(abstract_socket_template.encode()) > abstract_name_max_bytes
    ):
        raise ValueError(
            "verification consumer socket template must contain one job identity placeholder and fit the compiled ceiling"
        )
    compiler_path = _string(
        consumer["compiler_path"], "verification.consumer.compiler_path"
    )
    if not pathlib.PurePosixPath(compiler_path).is_absolute():
        raise ValueError("verification consumer compiler path must be absolute")
    compiler_family = _string(
        consumer["compiler_family"], "verification.consumer.compiler_family"
    )
    if compiler_family not in {"rust", "gcc", "clang"}:
        raise ValueError("verification consumer compiler family is invalid")
    s3_bucket = _string(consumer["s3_bucket"], "verification.consumer.s3_bucket")
    s3_region = _string(consumer["s3_region"], "verification.consumer.s3_region")
    s3_key_prefix = _string(
        consumer["s3_key_prefix"], "verification.consumer.s3_key_prefix"
    )
    if any(
        re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/-]*", value) is None
        for value in (s3_bucket, s3_region, s3_key_prefix)
    ):
        raise ValueError("verification consumer S3 values must be portable identifiers")
    s3_endpoint_prefix = _string(
        consumer["s3_endpoint_prefix"],
        "verification.consumer.s3_endpoint_prefix",
    )
    endpoint_probe = urllib.parse.urlsplit(f"{s3_endpoint_prefix}1")
    try:
        endpoint_address = ipaddress.ip_address(endpoint_probe.hostname or "")
    except ValueError as error:
        raise ValueError("verification consumer S3 endpoint prefix is invalid") from error
    if (
        endpoint_probe.scheme != "http"
        or not endpoint_address.is_loopback
        or endpoint_probe.port != 1
        or endpoint_probe.path
        or endpoint_probe.query
        or endpoint_probe.fragment
    ):
        raise ValueError("verification consumer S3 endpoint prefix is invalid")
    for value, name in (
        (consumer["s3_no_credentials"], "s3_no_credentials"),
        (consumer["s3_use_ssl"], "s3_use_ssl"),
        (
            consumer["s3_enable_virtual_host_style"],
            "s3_enable_virtual_host_style",
        ),
    ):
        if not isinstance(value, bool):
            raise ValueError(f"verification.consumer.{name} must be a boolean")
    replicas_value = verification["replicas"]
    if (
        not isinstance(replicas_value, list)
        or len(replicas_value) < 2
        or any(
            not isinstance(replica, str)
            or re.fullmatch(r"[a-z][a-z0-9-]*", replica) is None
            for replica in replicas_value
        )
        or len(set(replicas_value)) != len(replicas_value)
    ):
        raise ValueError(
            "verification.replicas must contain at least two unique governed names"
        )
    attestation_attempts = verification["attestation_attempts"]
    attestation_interval_seconds = verification["attestation_interval_seconds"]
    attestation_max_wait_seconds = verification["attestation_max_wait_seconds"]
    for value, name in (
        (attestation_attempts, "verification.attestation_attempts"),
        (
            attestation_interval_seconds,
            "verification.attestation_interval_seconds",
        ),
        (
            attestation_max_wait_seconds,
            "verification.attestation_max_wait_seconds",
        ),
    ):
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"{name} must be a positive integer")
    if (
        attestation_attempts * attestation_interval_seconds
        > attestation_max_wait_seconds
    ):
        raise ValueError("attestation retry wait exceeds its governed ceiling")

    publisher = _mapping(top["publisher"], "publisher")
    _exact_keys(
        publisher,
        frozenset(
            {
                "environment",
                "api_version",
                "gh_version",
                "gh_archive_url",
                "gh_archive_sha256",
                "gh_archive_member",
                "releases_per_page",
                "max_release_pages",
            }
        ),
        "publisher",
    )
    publisher_environment = _string(
        publisher["environment"], "publisher.environment"
    )
    if re.fullmatch(r"[a-z][a-z0-9-]*", publisher_environment) is None:
        raise ValueError("publisher.environment must be a portable name")
    publisher_api_version = _string(
        publisher["api_version"], "publisher.api_version"
    )
    if re.fullmatch(r"20[0-9]{2}-[0-9]{2}-[0-9]{2}", publisher_api_version) is None:
        raise ValueError("publisher.api_version must be an exact date")
    gh_version = _string(publisher["gh_version"], "publisher.gh_version")
    if not _VERSION.fullmatch(gh_version):
        raise ValueError("publisher.gh_version must be semantic version X.Y.Z")
    gh_archive_url = _string(
        publisher["gh_archive_url"], "publisher.gh_archive_url"
    )
    expected_gh_url = (
        f"https://github.com/cli/cli/releases/download/v{gh_version}/"
        f"gh_{gh_version}_linux_amd64.tar.gz"
    )
    if gh_archive_url != expected_gh_url:
        raise ValueError("publisher.gh_archive_url must match the pinned Linux archive")
    gh_archive_sha256 = _sha256(
        publisher["gh_archive_sha256"], "publisher.gh_archive_sha256"
    )
    gh_archive_member = _string(
        publisher["gh_archive_member"], "publisher.gh_archive_member"
    )
    if gh_archive_member != f"gh_{gh_version}_linux_amd64/bin/gh":
        raise ValueError("publisher.gh_archive_member must match the pinned archive")
    releases_per_page = publisher["releases_per_page"]
    max_release_pages = publisher["max_release_pages"]
    if type(releases_per_page) is not int or not 1 <= releases_per_page <= 100:
        raise ValueError("publisher.releases_per_page must be between 1 and 100")
    if type(max_release_pages) is not int or not 1 <= max_release_pages <= 100:
        raise ValueError("publisher.max_release_pages must be between 1 and 100")

    target_table = _mapping(top["targets"], "targets")
    if frozenset(target_table) != _TARGET_NAMES:
        raise ValueError("targets must be exactly ARM64 and X64")
    targets: dict[str, TargetConfig] = {}
    triples: set[str] = set()
    for architecture in sorted(target_table):
        target = _mapping(target_table[architecture], f"targets.{architecture}")
        _exact_keys(
            target, frozenset({"triple", "elf_machine"}), f"target {architecture}"
        )
        triple = _string(target["triple"], f"targets.{architecture}.triple")
        elf_machine = _string(
            target["elf_machine"], f"targets.{architecture}.elf_machine"
        )
        if triple in triples:
            raise ValueError("target triples must be unique")
        triples.add(triple)
        targets[architecture] = TargetConfig(triple=triple, elf_machine=elf_machine)

    return StrictBuildConfig(
        schema_version=1,
        repo_root=repo_root.resolve(),
        source_version=source_version,
        source_commit=source_commit,
        source_url=source_url,
        source_sha256=source_sha256,
        source_date_epoch=source_date_epoch,
        patch_path=patch_path,
        workflow_path=workflow_path,
        recipe_path=recipe_path,
        driver_path=driver_path,
        container=container,
        container_digest=container_digest,
        rustc_release=rustc_release,
        rustc_commit=rustc_commit,
        features=("s3", "vendored-openssl"),
        default_features=False,
        profile=profile,
        verification_timeout_ms=verification_timeout_ms,
        max_frame_bytes=max_frame_bytes,
        max_runtime_timeout_ms=max_runtime_timeout_ms,
        snapshot_max_bytes=snapshot_max_bytes,
        abstract_name_max_bytes=abstract_name_max_bytes,
        cache_format_token=cache_format_token,
        cache_compression_level=cache_compression_level,
        verification_cache_mode=verification_cache_mode,
        replicas=tuple(replicas_value),
        attestation_attempts=attestation_attempts,
        attestation_interval_seconds=attestation_interval_seconds,
        attestation_max_wait_seconds=attestation_max_wait_seconds,
        verification_consumer=VerificationConsumerConfig(
            abstract_socket_template=abstract_socket_template,
            compiler_path=compiler_path,
            compiler_family=compiler_family,
            s3_bucket=s3_bucket,
            s3_region=s3_region,
            s3_key_prefix=s3_key_prefix,
            s3_endpoint_prefix=s3_endpoint_prefix,
            s3_no_credentials=consumer["s3_no_credentials"],
            s3_use_ssl=consumer["s3_use_ssl"],
            s3_enable_virtual_host_style=consumer[
                "s3_enable_virtual_host_style"
            ],
        ),
        publisher=PublisherConfig(
            environment=publisher_environment,
            api_version=publisher_api_version,
            gh_version=gh_version,
            gh_archive_url=gh_archive_url,
            gh_archive_sha256=gh_archive_sha256,
            gh_archive_member=gh_archive_member,
            releases_per_page=releases_per_page,
            max_release_pages=max_release_pages,
        ),
        targets=MappingProxyType(targets),
    )


def load_config(path: pathlib.Path) -> StrictBuildConfig:
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"unable to load strict-sccache config: {error}") from error
    return load_document(document, repo_root=path.resolve().parents[1])


def _file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as file:
            for chunk in iter(lambda: file.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        raise ValueError(f"unable to hash {path}: {error}") from error
    return digest.hexdigest()


def _canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
        + "\n"
    ).encode("utf-8")


def _derivative_identity(config: StrictBuildConfig, architecture: str) -> str:
    target = config.targets[architecture]
    document = {
        "schema_version": 1,
        "source_commit": config.source_commit,
        "source_sha256": config.source_sha256,
        "patch_sha256": _file_sha256(config.patch_path),
        "workflow_sha256": _file_sha256(config.workflow_path),
        "recipe_sha256": _file_sha256(config.recipe_path),
        "driver_sha256": _file_sha256(config.driver_path),
        "container": config.container,
        "rustc_release": config.rustc_release,
        "rustc_commit": config.rustc_commit,
        "features": list(config.features),
        "default_features": config.default_features,
        "profile": config.profile,
        "max_runtime_timeout_ms": config.max_runtime_timeout_ms,
        "snapshot_max_bytes": config.snapshot_max_bytes,
        "abstract_name_max_bytes": config.abstract_name_max_bytes,
        "cache_format_token": config.cache_format_token,
        "cache_compression_level": config.cache_compression_level,
        "verification_consumer": dataclasses.asdict(config.verification_consumer),
        "publisher": dataclasses.asdict(config.publisher),
        "target": target.triple,
    }
    return hashlib.sha256(_canonical_json(document)).hexdigest()


def _write_exclusive(
    path: pathlib.Path, value: bytes, *, repo_root: pathlib.Path
) -> None:
    resolved = path.resolve()
    if resolved == repo_root or repo_root in resolved.parents:
        raise ValueError("output path must be outside the repository")
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with path.open("xb") as output:
            output.write(value)
    except OSError as error:
        raise ValueError(f"unable to create output {path}: {error}") from error


def _emit_json(value: object) -> None:
    print(_canonical_json(value).decode("utf-8"), end="")


def _candidate_binary_name(config: StrictBuildConfig, architecture: str) -> str:
    target = config.targets[architecture]
    return f"sccache-v{config.source_version}-strict-{target.triple}"


def write_verification_consumer(
    *, output_path: pathlib.Path, config: StrictBuildConfig, s3_endpoint: str
) -> None:
    consumer = config.verification_consumer
    if not s3_endpoint.startswith(consumer.s3_endpoint_prefix):
        raise ValueError("verification S3 endpoint does not match the governed prefix")
    port = s3_endpoint.removeprefix(consumer.s3_endpoint_prefix)
    if not port.isascii() or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise ValueError("verification S3 endpoint port is invalid")

    def quote(value: str) -> str:
        return json.dumps(value, ensure_ascii=True)

    timeout = config.verification_timeout_ms
    document = f"""schema_version = 1

[consumer]
abstract_socket_template = {quote(consumer.abstract_socket_template)}
startup_timeout_ms = {timeout}
ipc_frame_timeout_ms = {timeout}
cache_read_timeout_ms = {timeout}
required_write_timeout_ms = {timeout}
classification_timeout_ms = {timeout}
compiler_timeout_ms = {timeout}
termination_grace_ms = {timeout}
cleanup_timeout_ms = {timeout}
max_frame_bytes = {config.max_frame_bytes}
max_child_output_bytes = {config.max_frame_bytes}
cache_mode = {quote(config.verification_cache_mode)}

[consumer.storage]
bucket = {quote(consumer.s3_bucket)}
region = {quote(consumer.s3_region)}
key_prefix = {quote(consumer.s3_key_prefix)}
no_credentials = {str(consumer.s3_no_credentials).lower()}
endpoint = {quote(s3_endpoint)}
use_ssl = {str(consumer.s3_use_ssl).lower()}
enable_virtual_host_style = {str(consumer.s3_enable_virtual_host_style).lower()}

[[consumer.compilers]]
path = {quote(consumer.compiler_path)}
family = {quote(consumer.compiler_family)}
""".encode()
    _write_exclusive(output_path, document, repo_root=config.repo_root)


def write_candidate_manifest(
    *,
    output_path: pathlib.Path,
    config: StrictBuildConfig,
    binary_path: pathlib.Path,
    repository: str,
    run_id: str,
    run_attempt: str,
    head_sha: str,
    architecture: str,
    replica: str,
) -> CandidateManifest:
    if not re.fullmatch(r"[^/\s]+/[^/\s]+", repository):
        raise ValueError("repository must be OWNER/REPO")
    if not run_id.isdecimal():
        raise ValueError("run_id must be decimal")
    if not run_attempt.isdecimal() or int(run_attempt) <= 0:
        raise ValueError("run_attempt must be a positive decimal")
    _full_sha(head_sha, "head_sha")
    if architecture not in config.targets:
        raise ValueError("architecture is not governed")
    if replica not in config.replicas:
        raise ValueError("replica is not governed")
    try:
        binary_size = binary_path.stat().st_size
    except OSError as error:
        raise ValueError(f"unable to stat candidate binary: {error}") from error
    if binary_size <= 0:
        raise ValueError("candidate binary must not be empty")
    manifest: CandidateManifest = {
        "schema_version": 1,
        "repository": repository,
        "run_id": run_id,
        "run_attempt": run_attempt,
        "head_sha": head_sha,
        "architecture": architecture,
        "target": config.targets[architecture].triple,
        "replica": replica,
        "source_commit": config.source_commit,
        "source_sha256": config.source_sha256,
        "source_date_epoch": config.source_date_epoch,
        "patch_sha256": _file_sha256(config.patch_path),
        "workflow_sha256": _file_sha256(config.workflow_path),
        "recipe_sha256": _file_sha256(config.recipe_path),
        "driver_sha256": _file_sha256(config.driver_path),
        "derivative_identity": _derivative_identity(config, architecture),
        "container": config.container,
        "rustc_release": config.rustc_release,
        "rustc_commit": config.rustc_commit,
        "features": list(config.features),
        "default_features": config.default_features,
        "profile": config.profile,
        "verification_timeout_ms": config.verification_timeout_ms,
        "max_frame_bytes": config.max_frame_bytes,
        "max_runtime_timeout_ms": config.max_runtime_timeout_ms,
        "snapshot_max_bytes": config.snapshot_max_bytes,
        "abstract_name_max_bytes": config.abstract_name_max_bytes,
        "cache_format_token": config.cache_format_token,
        "cache_compression_level": config.cache_compression_level,
        "verification_cache_mode": config.verification_cache_mode,
        "verification_consumer": dataclasses.asdict(config.verification_consumer),
        "publisher": dataclasses.asdict(config.publisher),
        "binary_name": _candidate_binary_name(config, architecture),
        "binary_sha256": _file_sha256(binary_path),
        "binary_size": binary_size,
    }
    _write_exclusive(output_path, _canonical_json(manifest), repo_root=config.repo_root)
    return manifest


_CANDIDATE_KEYS = frozenset(
    {
        "schema_version",
        "repository",
        "run_id",
        "run_attempt",
        "head_sha",
        "architecture",
        "target",
        "replica",
        "source_commit",
        "source_sha256",
        "source_date_epoch",
        "patch_sha256",
        "workflow_sha256",
        "recipe_sha256",
        "driver_sha256",
        "derivative_identity",
        "container",
        "rustc_release",
        "rustc_commit",
        "features",
        "default_features",
        "profile",
        "verification_timeout_ms",
        "max_frame_bytes",
        "max_runtime_timeout_ms",
        "snapshot_max_bytes",
        "abstract_name_max_bytes",
        "cache_format_token",
        "cache_compression_level",
        "verification_cache_mode",
        "verification_consumer",
        "publisher",
        "binary_name",
        "binary_sha256",
        "binary_size",
    }
)


def verify_candidate_set(
    manifests: list[Mapping[str, object]],
    binary_paths: Mapping[tuple[str, str], pathlib.Path],
    *,
    config: StrictBuildConfig,
    repository: str,
    run_id: str,
    run_attempt: str,
    head_sha: str,
) -> VerifiedCandidateSet:
    expected_pairs = {
        (architecture, replica)
        for architecture in config.targets
        for replica in config.replicas
    }
    if len(manifests) != len(expected_pairs) or set(binary_paths) != expected_pairs:
        raise ValueError("candidate set must contain every configured replica")
    patch_sha256 = _file_sha256(config.patch_path)
    workflow_sha256 = _file_sha256(config.workflow_path)
    recipe_sha256 = _file_sha256(config.recipe_path)
    driver_sha256 = _file_sha256(config.driver_path)
    by_pair: dict[tuple[str, str], Mapping[str, object]] = {}
    for raw_manifest in manifests:
        manifest = _mapping(raw_manifest, "candidate manifest")
        _exact_keys(manifest, _CANDIDATE_KEYS, "candidate manifest")
        if manifest["repository"] != repository:
            raise ValueError("candidates must belong to the same repository")
        if manifest["run_id"] != run_id:
            raise ValueError("candidates must come from the same workflow run")
        if manifest["run_attempt"] != run_attempt:
            raise ValueError("candidates must come from the same workflow attempt")
        if manifest["head_sha"] != head_sha:
            raise ValueError("candidates must bind the exact head SHA")
        architecture = manifest["architecture"]
        replica = manifest["replica"]
        if not isinstance(architecture, str) or not isinstance(replica, str):
            raise ValueError("candidate architecture and replica must be strings")
        pair = (architecture, replica)
        if pair not in expected_pairs or pair in by_pair:
            raise ValueError("candidate architecture/replica set is invalid")
        if (
            type(manifest["schema_version"]) is not int
            or manifest["schema_version"] != 1
        ):
            raise ValueError("candidate schema_version must be integer 1")
        if manifest["target"] != config.targets[architecture].triple:
            raise ValueError("candidate target does not match governed config")
        expected_common = {
            "source_commit": config.source_commit,
            "source_sha256": config.source_sha256,
            "source_date_epoch": config.source_date_epoch,
            "patch_sha256": patch_sha256,
            "workflow_sha256": workflow_sha256,
            "recipe_sha256": recipe_sha256,
            "driver_sha256": driver_sha256,
            "derivative_identity": _derivative_identity(config, architecture),
            "container": config.container,
            "rustc_release": config.rustc_release,
            "rustc_commit": config.rustc_commit,
            "features": list(config.features),
            "default_features": config.default_features,
            "profile": config.profile,
            "verification_timeout_ms": config.verification_timeout_ms,
            "max_frame_bytes": config.max_frame_bytes,
            "max_runtime_timeout_ms": config.max_runtime_timeout_ms,
            "snapshot_max_bytes": config.snapshot_max_bytes,
            "abstract_name_max_bytes": config.abstract_name_max_bytes,
            "cache_format_token": config.cache_format_token,
            "cache_compression_level": config.cache_compression_level,
            "verification_cache_mode": config.verification_cache_mode,
            "verification_consumer": dataclasses.asdict(config.verification_consumer),
            "publisher": dataclasses.asdict(config.publisher),
            "binary_name": _candidate_binary_name(config, architecture),
        }
        for key, expected_value in expected_common.items():
            if manifest[key] != expected_value:
                raise ValueError(f"candidate {key} does not match governed input")
        binary_path = binary_paths[pair]
        binary_digest = _file_sha256(binary_path)
        if manifest["binary_sha256"] != binary_digest:
            raise ValueError("candidate binary digest does not match bytes")
        try:
            binary_size = binary_path.stat().st_size
        except OSError as error:
            raise ValueError(f"unable to stat candidate binary: {error}") from error
        if manifest["binary_size"] != binary_size:
            raise ValueError("candidate binary size does not match bytes")
        by_pair[pair] = manifest
    if set(by_pair) != expected_pairs:
        raise ValueError("candidate set must contain every governed replica")

    assets: dict[str, VerifiedAsset] = {}
    for architecture in sorted(config.targets):
        first_replica = config.replicas[0]
        first = by_pair[(architecture, first_replica)]
        if any(
            by_pair[(architecture, replica)]["binary_sha256"] != first["binary_sha256"]
            or by_pair[(architecture, replica)]["binary_size"] != first["binary_size"]
            for replica in config.replicas[1:]
        ):
            raise ValueError(f"{architecture} replicas are not byte-identical")
        assets[architecture] = VerifiedAsset(
            architecture=architecture,
            target=config.targets[architecture].triple,
            name=str(first["binary_name"]),
            sha256=str(first["binary_sha256"]),
            size=int(first["binary_size"]),
            path=binary_paths[(architecture, first_replica)],
        )

    build_identity_document = {
        "schema_version": 1,
        "publication": {
            "repository": repository,
            "head_sha": head_sha,
        },
        "source": {
            "version": config.source_version,
            "commit": config.source_commit,
            "archive_url": config.source_url,
            "archive_sha256": config.source_sha256,
            "source_date_epoch": config.source_date_epoch,
            "patch_sha256": patch_sha256,
            "workflow_sha256": workflow_sha256,
            "recipe_sha256": recipe_sha256,
            "driver_sha256": driver_sha256,
        },
        "build": {
            "container": config.container,
            "rustc_release": config.rustc_release,
            "rustc_commit": config.rustc_commit,
            "features": list(config.features),
            "default_features": config.default_features,
            "profile": config.profile,
            "verification_timeout_ms": config.verification_timeout_ms,
            "max_frame_bytes": config.max_frame_bytes,
            "max_runtime_timeout_ms": config.max_runtime_timeout_ms,
            "snapshot_max_bytes": config.snapshot_max_bytes,
            "abstract_name_max_bytes": config.abstract_name_max_bytes,
            "cache_format_token": config.cache_format_token,
            "cache_compression_level": config.cache_compression_level,
            "verification_cache_mode": config.verification_cache_mode,
            "verification_consumer": dataclasses.asdict(config.verification_consumer),
            "publisher": dataclasses.asdict(config.publisher),
            "replicas": list(config.replicas),
        },
        "release_verification": {
            "attestation_attempts": config.attestation_attempts,
            "attestation_interval_seconds": config.attestation_interval_seconds,
            "attestation_max_wait_seconds": config.attestation_max_wait_seconds,
        },
        "targets": [
            {
                "architecture": architecture,
                "triple": config.targets[architecture].triple,
                "elf_machine": config.targets[architecture].elf_machine,
            }
            for architecture in sorted(config.targets)
        ],
    }
    build_identity_sha256 = hashlib.sha256(
        _canonical_json(build_identity_document)
    ).hexdigest()
    release_tag = (
        f"tooling-sccache-v{config.source_version}-strict-{build_identity_sha256}"
    )
    provenance_name = f"{release_tag}-provenance.json"
    provenance_document = {
        "schema_version": 1,
        "repository": repository,
        "run_id": run_id,
        "run_attempt": run_attempt,
        "head_sha": head_sha,
        "release_tag": release_tag,
        "build_identity_sha256": build_identity_sha256,
        "build_identity": build_identity_document,
        "assets": [
            {
                "architecture": asset.architecture,
                "target": asset.target,
                "name": asset.name,
                "sha256": asset.sha256,
                "size": asset.size,
            }
            for asset in assets.values()
        ],
    }
    provenance_bytes = _canonical_json(provenance_document)
    return VerifiedCandidateSet(
        repository=repository,
        run_id=run_id,
        run_attempt=run_attempt,
        head_sha=head_sha,
        source_version=config.source_version,
        source_commit=config.source_commit,
        source_sha256=config.source_sha256,
        source_date_epoch=config.source_date_epoch,
        patch_sha256=patch_sha256,
        container=config.container,
        rustc_release=config.rustc_release,
        rustc_commit=config.rustc_commit,
        features=config.features,
        default_features=config.default_features,
        profile=config.profile,
        build_identity_sha256=build_identity_sha256,
        release_tag=release_tag,
        assets=MappingProxyType(assets),
        provenance_name=provenance_name,
        provenance_bytes=provenance_bytes,
        provenance_sha256=hashlib.sha256(provenance_bytes).hexdigest(),
    )


def validate_publish_context(
    *,
    event_name: str,
    requested_sha: str,
    event_sha: str,
    remote_main_sha: str,
    event_ref: str,
    environment_document: Mapping[str, object],
    environment_name: str,
) -> None:
    if event_name != "workflow_dispatch":
        raise ValueError("publication requires workflow_dispatch")
    for value, name in (
        (requested_sha, "requested_sha"),
        (event_sha, "event_sha"),
        (remote_main_sha, "remote_main_sha"),
    ):
        _full_sha(value, name)
    if requested_sha != event_sha or requested_sha != remote_main_sha:
        raise ValueError("publication SHA is not exact-current main")
    if event_ref != "refs/heads/main":
        raise ValueError("publication ref must be refs/heads/main")
    environment = _mapping(environment_document, "publisher environment document")
    if not environment:
        raise ValueError("publisher environment is absent")
    if environment.get("name") != environment_name:
        raise ValueError("publisher environment name does not match")
    deployment = environment.get("deployment_branch_policy")
    if not isinstance(deployment, Mapping):
        raise ValueError("publisher environment has no deployment branch policy")
    if deployment.get("protected_branches") is not True:
        raise ValueError("publisher environment must admit protected branches")
    if deployment.get("custom_branch_policies") is not False:
        raise ValueError("publisher environment branch policy is ambiguous")


def verify_release_record(
    expected: VerifiedCandidateSet,
    release: Mapping[str, object],
    tag_ref: Mapping[str, object],
    tag_object: Mapping[str, object],
    ownership_marker: str,
) -> None:
    if release.get("tag_name") != expected.release_tag:
        raise ValueError("release tag does not match")
    if release.get("target_commitish") != expected.head_sha:
        raise ValueError("release target does not match exact head")
    if release.get("draft") is not False:
        raise ValueError("release is still a draft")
    if release.get("immutable") is not True:
        raise ValueError("release is not immutable")
    validate_owned_annotated_tag(
        tag_ref,
        tag_object,
        tag=expected.release_tag,
        head_sha=expected.head_sha,
        ownership_marker=ownership_marker,
    )
    raw_assets = release.get("assets")
    if not isinstance(raw_assets, list):
        raise ValueError("release assets must be a list")
    actual_assets: dict[str, Mapping[str, object]] = {}
    for raw_asset in raw_assets:
        asset = _mapping(raw_asset, "release asset")
        name = asset.get("name")
        if not isinstance(name, str) or name in actual_assets:
            raise ValueError("release asset names must be unique strings")
        actual_assets[name] = asset
    expected_assets = {
        asset.name: (asset.size, asset.sha256) for asset in expected.assets.values()
    }
    expected_assets[expected.provenance_name] = (
        len(expected.provenance_bytes),
        expected.provenance_sha256,
    )
    if set(actual_assets) != set(expected_assets):
        raise ValueError("release asset set does not match")
    for name, (size, digest) in expected_assets.items():
        asset = actual_assets[name]
        if asset.get("size") != size or asset.get("digest") != f"sha256:{digest}":
            raise ValueError(f"release asset {name} does not match expected bytes")


def release_ownership_marker(
    *,
    repository: str,
    run_id: str,
    run_attempt: str,
    head_sha: str,
    tag: str,
) -> str:
    if not re.fullmatch(r"[^/\s]+/[^/\s]+", repository):
        raise ValueError("repository must be OWNER/REPO")
    if not run_id.isdecimal():
        raise ValueError("run_id must be decimal")
    if not run_attempt.isdecimal() or int(run_attempt) <= 0:
        raise ValueError("run_attempt must be a positive decimal")
    _full_sha(head_sha, "head_sha")
    if not tag:
        raise ValueError("release tag must not be empty")
    return (
        _canonical_json(
            {
                "kind": "governed-strict-sccache-publication",
                "repository": repository,
                "run_id": run_id,
                "run_attempt": run_attempt,
                "head_sha": head_sha,
                "tag": tag,
            }
        )
        .decode("utf-8")
        .rstrip("\n")
    )


def validate_release_tag(
    tag_ref: Mapping[str, object], *, tag: str, head_sha: str
) -> None:
    _full_sha(head_sha, "head_sha")
    if tag_ref.get("ref") != f"refs/tags/{tag}":
        raise ValueError("release tag ref does not match")
    tag_object = _mapping(tag_ref.get("object"), "release tag object")
    if tag_object.get("type") != "commit" or tag_object.get("sha") != head_sha:
        raise ValueError("release tag does not target exact head commit")


def validate_owned_annotated_tag(
    tag_ref: Mapping[str, object],
    tag_object: Mapping[str, object],
    *,
    tag: str,
    head_sha: str,
    ownership_marker: str,
) -> None:
    _full_sha(head_sha, "head_sha")
    if tag_ref.get("ref") != f"refs/tags/{tag}":
        raise ValueError("annotated release tag ref does not match")
    ref_object = _mapping(tag_ref.get("object"), "annotated release tag ref object")
    object_sha = _full_sha(tag_object.get("sha"), "annotated tag object sha")
    if ref_object.get("type") != "tag" or ref_object.get("sha") != object_sha:
        raise ValueError("release tag ref does not target the annotated tag object")
    if tag_object.get("tag") != tag:
        raise ValueError("annotated release tag name does not match")
    if tag_object.get("message") != ownership_marker:
        raise ValueError("annotated release tag ownership does not match")
    target = _mapping(tag_object.get("object"), "annotated release tag target")
    if target.get("type") != "commit" or target.get("sha") != head_sha:
        raise ValueError("annotated release tag does not target exact head commit")


def validate_tag_create_response(response: str, *, tag: str, head_sha: str) -> None:
    separator = "\r\n\r\n" if "\r\n\r\n" in response else "\n\n"
    parts = response.split(separator, 1)
    if len(parts) != 2 or separator in parts[1]:
        raise ValueError("tag-create response framing is malformed")
    status = parts[0].replace("\r\n", "\n").split("\n", 1)[0]
    if re.fullmatch(r"HTTP/[0-9.]+ 201(?: Created)?", status) is None:
        raise ValueError("tag-create response status is not 201")
    try:
        tag_ref = _mapping(json.loads(parts[1]), "tag-create response")
    except json.JSONDecodeError as error:
        raise ValueError("tag-create response body is malformed JSON") from error
    validate_release_tag(tag_ref, tag=tag, head_sha=head_sha)


def validate_release_page_response(
    response: str,
    *,
    repository: str,
    repository_id: int,
    expected_page: int,
    per_page: int,
    visited: set[str],
) -> dict[str, object]:
    if not re.fullmatch(r"[^/\s]+/[^/\s]+", repository):
        raise ValueError("repository must be OWNER/REPO")
    if repository_id <= 0 or expected_page <= 0 or not 1 <= per_page <= 100:
        raise ValueError("release page identity is invalid")
    separator = "\r\n\r\n" if "\r\n\r\n" in response else "\n\n"
    parts = response.split(separator, 1)
    if len(parts) != 2 or separator in parts[1]:
        raise ValueError("release page response framing is malformed")
    header_text, body_text = parts
    header_lines = header_text.replace("\r\n", "\n").split("\n")
    if re.fullmatch(r"HTTP/[0-9.]+ 200(?: OK)?", header_lines[0]) is None:
        raise ValueError("release page response status is not 200")
    headers: dict[str, list[str]] = {}
    for line in header_lines[1:]:
        name, delimiter, value = line.partition(":")
        if not delimiter or not name:
            raise ValueError("release page response header is malformed")
        headers.setdefault(name.lower(), []).append(value.strip())
    if len(headers.get("link", [])) > 1:
        raise ValueError("release page response has duplicate Link headers")
    try:
        releases = json.loads(body_text)
    except json.JSONDecodeError as error:
        raise ValueError("release page body is malformed JSON") from error
    if not isinstance(releases, list) or len(releases) > per_page:
        raise ValueError("release page body is not a bounded array")

    owner, name = repository.split("/", 1)
    configured_path = f"/repos/{owner}/{name}/releases"
    numeric_path = f"/repositories/{repository_id}/releases"

    def canonical_endpoint(url: str, relation: str) -> tuple[str, int]:
        parsed = urllib.parse.urlsplit(url)
        if (
            parsed.scheme != "https"
            or parsed.netloc != "api.github.com"
            or parsed.username is not None
            or parsed.password is not None
            or parsed.fragment
            or urllib.parse.unquote(parsed.path) not in {configured_path, numeric_path}
        ):
            raise ValueError("release continuation origin or repository path is invalid")
        query = urllib.parse.parse_qsl(
            parsed.query, keep_blank_values=True, strict_parsing=True
        )
        if len(query) != 2 or {key for key, _ in query} != {"page", "per_page"}:
            raise ValueError("release continuation query is invalid")
        values = dict(query)
        if values["per_page"] != str(per_page) or not values["page"].isdecimal():
            raise ValueError("release continuation query values are invalid")
        page = int(values["page"])
        if page <= 0:
            raise ValueError("release continuation page is invalid")
        endpoint = f"{parsed.path}?per_page={per_page}&page={page}".removeprefix("/")
        if relation == "next" and (page != expected_page + 1 or endpoint in visited):
            raise ValueError("release continuation is repeated or non-sequential")
        return endpoint, page

    relations: dict[str, tuple[str, int]] = {}
    if headers.get("link"):
        for segment in headers["link"][0].split(","):
            match = re.fullmatch(r'\s*<([^>]+)>;\s*rel="(next|prev|first|last)"\s*', segment)
            if match is None or match.group(2) in relations:
                raise ValueError("release continuation Link header is malformed or ambiguous")
            relations[match.group(2)] = canonical_endpoint(
                match.group(1), match.group(2)
            )
    next_endpoint = relations.get("next")
    if next_endpoint is not None and "last" not in relations:
        raise ValueError("release continuation has next without last")
    if next_endpoint is not None and relations["last"][1] < next_endpoint[1]:
        raise ValueError("release continuation last page precedes next")
    return {
        "releases": releases,
        "next_endpoint": next_endpoint[0] if next_endpoint is not None else None,
    }


def _release_records(value: object) -> list[Mapping[str, object]]:
    if isinstance(value, Mapping):
        return [value]
    if not isinstance(value, list):
        raise ValueError("release cleanup input must be a record or array")
    records: list[Mapping[str, object]] = []
    for item in value:
        records.extend(_release_records(item))
    return records


def validate_owned_tag_cleanup(releases: object, *, tag: str) -> None:
    if any(release.get("tag_name") == tag for release in _release_records(releases)):
        raise ValueError("release still exists for owned tag")


def select_owned_mutable_release(
    releases: object,
    *,
    expected_id: int | None,
    tag: str,
    head_sha: str,
    ownership_marker: str,
) -> int | None:
    _full_sha(head_sha, "head_sha")
    matches: list[int] = []
    for release in _release_records(releases):
        release_id = release.get("id")
        if isinstance(release_id, bool) or not isinstance(release_id, int):
            raise ValueError("release id must be an integer")
        if expected_id is not None and release_id != expected_id:
            continue
        if (
            release.get("tag_name") == tag
            and release.get("target_commitish") == head_sha
            and (release.get("draft") is True or release.get("draft") is False)
            and release.get("immutable") is not True
            and release.get("body") == ownership_marker
        ):
            matches.append(release_id)
    if len(matches) != 1:
        return None
    return matches[0]


def _read_json(path: pathlib.Path, name: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"unable to load {name}: {error}") from error


def _candidate_arguments(
    values: list[list[str]],
) -> tuple[list[Mapping[str, object]], dict[tuple[str, str], pathlib.Path]]:
    manifests: list[Mapping[str, object]] = []
    binaries: dict[tuple[str, str], pathlib.Path] = {}
    for architecture, replica, manifest_name, binary_name in values:
        raw_manifest = _read_json(pathlib.Path(manifest_name), "candidate manifest")
        manifest = _mapping(raw_manifest, "candidate manifest")
        manifests.append(manifest)
        pair = (architecture, replica)
        if pair in binaries:
            raise ValueError("candidate argument is duplicated")
        binaries[pair] = pathlib.Path(binary_name)
    return manifests, binaries


def _verified_summary(verified: VerifiedCandidateSet) -> dict[str, object]:
    return {
        "repository": verified.repository,
        "run_id": verified.run_id,
        "run_attempt": verified.run_attempt,
        "head_sha": verified.head_sha,
        "build_identity_sha256": verified.build_identity_sha256,
        "release_tag": verified.release_tag,
        "provenance_name": verified.provenance_name,
        "provenance_sha256": verified.provenance_sha256,
        "assets": {
            architecture: {
                "name": asset.name,
                "sha256": asset.sha256,
                "size": asset.size,
                "target": asset.target,
            }
            for architecture, asset in verified.assets.items()
        },
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    show_target = subparsers.add_parser("show-target")
    show_target.add_argument("--config", required=True, type=pathlib.Path)
    show_target.add_argument("--architecture", required=True)

    consumer = subparsers.add_parser("write-verification-consumer")
    consumer.add_argument("--config", required=True, type=pathlib.Path)
    consumer.add_argument("--output", required=True, type=pathlib.Path)
    consumer.add_argument("--s3-endpoint", required=True)

    candidate = subparsers.add_parser("candidate-manifest")
    candidate.add_argument("--config", required=True, type=pathlib.Path)
    candidate.add_argument("--binary", required=True, type=pathlib.Path)
    candidate.add_argument("--repository", required=True)
    candidate.add_argument("--run-id", required=True)
    candidate.add_argument("--run-attempt", required=True)
    candidate.add_argument("--head-sha", required=True)
    candidate.add_argument("--architecture", required=True)
    candidate.add_argument("--replica", required=True)
    candidate.add_argument("--output", required=True, type=pathlib.Path)

    verify = subparsers.add_parser("verify-candidates")
    verify.add_argument("--config", required=True, type=pathlib.Path)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--run-id", required=True)
    verify.add_argument("--run-attempt", required=True)
    verify.add_argument("--head-sha", required=True)
    verify.add_argument(
        "--candidate",
        required=True,
        action="append",
        nargs=4,
        metavar=("ARCH", "REPLICA", "MANIFEST", "BINARY"),
    )
    verify.add_argument("--provenance-output", required=True, type=pathlib.Path)

    publish = subparsers.add_parser("validate-publish-context")
    publish.add_argument("--event-name", required=True)
    publish.add_argument("--requested-sha", required=True)
    publish.add_argument("--event-sha", required=True)
    publish.add_argument("--remote-main-sha", required=True)
    publish.add_argument("--event-ref", required=True)
    publish.add_argument("--environment-json", required=True, type=pathlib.Path)
    publish.add_argument("--environment-name", required=True)

    release = subparsers.add_parser("verify-release-record")
    release.add_argument("--config", required=True, type=pathlib.Path)
    release.add_argument("--repository", required=True)
    release.add_argument("--run-id", required=True)
    release.add_argument("--run-attempt", required=True)
    release.add_argument("--head-sha", required=True)
    release.add_argument(
        "--candidate",
        required=True,
        action="append",
        nargs=4,
        metavar=("ARCH", "REPLICA", "MANIFEST", "BINARY"),
    )
    release.add_argument("--release-json", required=True, type=pathlib.Path)
    release.add_argument("--tag-json", required=True, type=pathlib.Path)
    release.add_argument("--tag-object-json", required=True, type=pathlib.Path)

    marker = subparsers.add_parser("release-ownership-marker")
    marker.add_argument("--repository", required=True)
    marker.add_argument("--run-id", required=True)
    marker.add_argument("--run-attempt", required=True)
    marker.add_argument("--head-sha", required=True)
    marker.add_argument("--tag", required=True)

    cleanup = subparsers.add_parser("select-cleanup-release")
    cleanup.add_argument("--releases-json", required=True, type=pathlib.Path)
    cleanup.add_argument("--release-id", type=int)
    cleanup.add_argument("--tag", required=True)
    cleanup.add_argument("--head-sha", required=True)
    cleanup.add_argument("--ownership-marker", required=True)

    tag_cleanup = subparsers.add_parser("validate-owned-tag-cleanup")
    tag_cleanup.add_argument("--releases-json", required=True, type=pathlib.Path)
    tag_cleanup.add_argument("--tag", required=True)

    owned_tag = subparsers.add_parser("validate-owned-annotated-tag")
    owned_tag.add_argument("--tag-ref-json", required=True, type=pathlib.Path)
    owned_tag.add_argument("--tag-object-json", required=True, type=pathlib.Path)
    owned_tag.add_argument("--tag", required=True)
    owned_tag.add_argument("--head-sha", required=True)
    owned_tag.add_argument("--ownership-marker", required=True)

    release_page = subparsers.add_parser("validate-release-page")
    release_page.add_argument("--response", required=True, type=pathlib.Path)
    release_page.add_argument("--repository", required=True)
    release_page.add_argument("--repository-id", required=True, type=int)
    release_page.add_argument("--expected-page", required=True, type=int)
    release_page.add_argument("--per-page", required=True, type=int)
    release_page.add_argument("--visited", action="append", default=[])

    tag_create = subparsers.add_parser("validate-tag-create-response")
    tag_create.add_argument("--response", required=True, type=pathlib.Path)
    tag_create.add_argument("--tag", required=True)
    tag_create.add_argument("--head-sha", required=True)

    release_tag = subparsers.add_parser("validate-release-tag")
    release_tag.add_argument("--tag-json", required=True, type=pathlib.Path)
    release_tag.add_argument("--tag", required=True)
    release_tag.add_argument("--head-sha", required=True)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "show-target":
            config = load_config(args.config)
            if args.architecture not in config.targets:
                raise ValueError("architecture is not governed")
            target = config.targets[args.architecture]
            output = {
                "architecture": args.architecture,
                "target": target.triple,
                "elf_machine": target.elf_machine,
                "source_version": config.source_version,
                "source_url": config.source_url,
                "source_sha256": config.source_sha256,
                "source_date_epoch": config.source_date_epoch,
                "patch": str(config.patch_path),
                "workflow": str(config.workflow_path),
                "recipe": str(config.recipe_path),
                "driver": str(config.driver_path),
                "container": config.container,
                "rustc_release": config.rustc_release,
                "rustc_commit": config.rustc_commit,
                "features": list(config.features),
                "default_features": config.default_features,
                "profile": config.profile,
                "verification_timeout_ms": config.verification_timeout_ms,
                "verification_timeout_duration": gnu_timeout_duration(
                    config.verification_timeout_ms
                ),
                "max_frame_bytes": config.max_frame_bytes,
                "max_runtime_timeout_ms": config.max_runtime_timeout_ms,
                "snapshot_max_bytes": config.snapshot_max_bytes,
                "abstract_name_max_bytes": config.abstract_name_max_bytes,
                "cache_format_token": config.cache_format_token,
                "cache_compression_level": config.cache_compression_level,
                "verification_cache_mode": config.verification_cache_mode,
                "verification_consumer": dataclasses.asdict(
                    config.verification_consumer
                ),
                "publisher": dataclasses.asdict(config.publisher),
                "replicas": list(config.replicas),
                "attestation_attempts": config.attestation_attempts,
                "attestation_interval_seconds": config.attestation_interval_seconds,
                "attestation_max_wait_seconds": config.attestation_max_wait_seconds,
                "derivative_identity": _derivative_identity(config, args.architecture),
            }
            _emit_json(output)
            return 0
        if args.command == "write-verification-consumer":
            config = load_config(args.config)
            write_verification_consumer(
                output_path=args.output,
                config=config,
                s3_endpoint=args.s3_endpoint,
            )
            _emit_json({"consumer_config": str(args.output)})
            return 0
        if args.command == "candidate-manifest":
            config = load_config(args.config)
            manifest = write_candidate_manifest(
                output_path=args.output,
                config=config,
                binary_path=args.binary,
                repository=args.repository,
                run_id=args.run_id,
                run_attempt=args.run_attempt,
                head_sha=args.head_sha,
                architecture=args.architecture,
                replica=args.replica,
            )
            _emit_json(manifest)
            return 0
        if args.command in {"verify-candidates", "verify-release-record"}:
            config = load_config(args.config)
            manifests, binaries = _candidate_arguments(args.candidate)
            verified = verify_candidate_set(
                manifests,
                binaries,
                config=config,
                repository=args.repository,
                run_id=args.run_id,
                run_attempt=args.run_attempt,
                head_sha=args.head_sha,
            )
            if args.command == "verify-candidates":
                _write_exclusive(
                    args.provenance_output,
                    verified.provenance_bytes,
                    repo_root=config.repo_root,
                )
                _emit_json(_verified_summary(verified))
                return 0
            release_document = _mapping(
                _read_json(args.release_json, "release record"), "release record"
            )
            tag_document = _mapping(
                _read_json(args.tag_json, "release tag record"), "release tag record"
            )
            tag_object_document = _mapping(
                _read_json(args.tag_object_json, "release annotated tag object"),
                "release annotated tag object",
            )
            ownership_marker = release_ownership_marker(
                repository=args.repository,
                run_id=args.run_id,
                run_attempt=args.run_attempt,
                head_sha=args.head_sha,
                tag=verified.release_tag,
            )
            verify_release_record(
                verified,
                release_document,
                tag_document,
                tag_object_document,
                ownership_marker,
            )
            _emit_json({"release_verified": True})
            return 0
        if args.command == "validate-publish-context":
            environment = _mapping(
                _read_json(args.environment_json, "publisher environment document"),
                "publisher environment document",
            )
            validate_publish_context(
                event_name=args.event_name,
                requested_sha=args.requested_sha,
                event_sha=args.event_sha,
                remote_main_sha=args.remote_main_sha,
                event_ref=args.event_ref,
                environment_document=environment,
                environment_name=args.environment_name,
            )
            _emit_json({"publisher_ready": True})
            return 0
        if args.command == "release-ownership-marker":
            print(
                release_ownership_marker(
                    repository=args.repository,
                    run_id=args.run_id,
                    run_attempt=args.run_attempt,
                    head_sha=args.head_sha,
                    tag=args.tag,
                )
            )
            return 0
        if args.command == "select-cleanup-release":
            release_id = select_owned_mutable_release(
                _read_json(args.releases_json, "cleanup release records"),
                expected_id=args.release_id,
                tag=args.tag,
                head_sha=args.head_sha,
                ownership_marker=args.ownership_marker,
            )
            if release_id is None:
                return 1
            print(release_id)
            return 0
        if args.command == "validate-owned-tag-cleanup":
            validate_owned_tag_cleanup(
                _read_json(args.releases_json, "tag cleanup release records"),
                tag=args.tag,
            )
            return 0
        if args.command == "validate-owned-annotated-tag":
            tag_ref = _mapping(
                _read_json(args.tag_ref_json, "annotated release tag ref"),
                "annotated release tag ref",
            )
            tag_object = _mapping(
                _read_json(args.tag_object_json, "annotated release tag object"),
                "annotated release tag object",
            )
            validate_owned_annotated_tag(
                tag_ref,
                tag_object,
                tag=args.tag,
                head_sha=args.head_sha,
                ownership_marker=args.ownership_marker,
            )
            return 0
        if args.command == "validate-release-page":
            try:
                response = args.response.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                raise ValueError(f"unable to load release page response: {error}") from error
            _emit_json(
                validate_release_page_response(
                    response,
                    repository=args.repository,
                    repository_id=args.repository_id,
                    expected_page=args.expected_page,
                    per_page=args.per_page,
                    visited=set(args.visited),
                )
            )
            return 0
        if args.command == "validate-tag-create-response":
            try:
                response = args.response.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                raise ValueError(f"unable to load tag-create response: {error}") from error
            validate_tag_create_response(
                response,
                tag=args.tag,
                head_sha=args.head_sha,
            )
            return 0
        if args.command == "validate-release-tag":
            tag_document = _mapping(
                _read_json(args.tag_json, "release tag record"),
                "release tag record",
            )
            validate_release_tag(
                tag_document,
                tag=args.tag,
                head_sha=args.head_sha,
            )
            return 0
        raise ValueError("unsupported command")
    except ValueError as error:
        print(f"strict-sccache validation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
