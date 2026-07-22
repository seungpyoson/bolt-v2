#!/usr/bin/env python3
"""Validate strict-sccache build metadata and release evidence."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import pathlib
import re
import sys
import tomllib
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
    verification_cache_mode: str
    replicas: tuple[str, ...]
    attestation_attempts: int
    attestation_interval_seconds: int
    targets: Mapping[str, TargetConfig]


CandidateManifest = dict[str, object]


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
        frozenset({"schema_version", "source", "build", "verification", "targets"}),
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
    verification_cache_mode = _string(
        verification["cache_mode"], "verification.cache_mode"
    )
    if verification_cache_mode not in {"READ_ONLY", "READ_WRITE"}:
        raise ValueError("verification.cache_mode must be READ_ONLY or READ_WRITE")
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
    for value, name in (
        (attestation_attempts, "verification.attestation_attempts"),
        (
            attestation_interval_seconds,
            "verification.attestation_interval_seconds",
        ),
    ):
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"{name} must be a positive integer")

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
        verification_cache_mode=verification_cache_mode,
        replicas=tuple(replicas_value),
        attestation_attempts=attestation_attempts,
        attestation_interval_seconds=attestation_interval_seconds,
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
        "verification_cache_mode": config.verification_cache_mode,
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
        "verification_cache_mode",
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
            "verification_cache_mode": config.verification_cache_mode,
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
            "verification_cache_mode": config.verification_cache_mode,
            "replicas": list(config.replicas),
        },
        "release_verification": {
            "attestation_attempts": config.attestation_attempts,
            "attestation_interval_seconds": config.attestation_interval_seconds,
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
) -> None:
    if release.get("tag_name") != expected.release_tag:
        raise ValueError("release tag does not match")
    if release.get("target_commitish") != expected.head_sha:
        raise ValueError("release target does not match exact head")
    if release.get("draft") is not False:
        raise ValueError("release is still a draft")
    if release.get("immutable") is not True:
        raise ValueError("release is not immutable")
    if tag_ref.get("ref") != f"refs/tags/{expected.release_tag}":
        raise ValueError("release tag ref does not match")
    tag_object = _mapping(tag_ref.get("object"), "release tag object")
    if tag_object.get("type") != "commit" or tag_object.get("sha") != expected.head_sha:
        raise ValueError("release tag does not target exact head commit")
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


def _release_records(value: object) -> list[Mapping[str, object]]:
    if isinstance(value, Mapping):
        return [value]
    if not isinstance(value, list):
        raise ValueError("release cleanup input must be a record or array")
    records: list[Mapping[str, object]] = []
    for item in value:
        records.extend(_release_records(item))
    return records


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


def validate_cleanup_tag(
    tag_ref: Mapping[str, object], *, tag: str, head_sha: str
) -> None:
    _full_sha(head_sha, "head_sha")
    if tag_ref.get("ref") != f"refs/tags/{tag}":
        raise ValueError("cleanup tag ref does not match")
    tag_object = _mapping(tag_ref.get("object"), "cleanup tag object")
    if tag_object.get("type") != "commit" or tag_object.get("sha") != head_sha:
        raise ValueError("cleanup tag does not target exact head commit")


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

    cleanup_tag = subparsers.add_parser("validate-cleanup-tag")
    cleanup_tag.add_argument("--tag-json", required=True, type=pathlib.Path)
    cleanup_tag.add_argument("--tag", required=True)
    cleanup_tag.add_argument("--head-sha", required=True)
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
                "max_frame_bytes": config.max_frame_bytes,
                "verification_cache_mode": config.verification_cache_mode,
                "replicas": list(config.replicas),
                "attestation_attempts": config.attestation_attempts,
                "attestation_interval_seconds": config.attestation_interval_seconds,
                "derivative_identity": _derivative_identity(config, args.architecture),
            }
            _emit_json(output)
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
            verify_release_record(verified, release_document, tag_document)
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
        if args.command == "validate-cleanup-tag":
            tag_document = _mapping(
                _read_json(args.tag_json, "cleanup tag record"),
                "cleanup tag record",
            )
            validate_cleanup_tag(
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
