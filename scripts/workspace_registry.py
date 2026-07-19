#!/usr/bin/env python3
"""Authoritative governed-workspace registry and reconciliation."""

from __future__ import annotations

import pathlib
import subprocess
import tomllib
from dataclasses import dataclass
from typing import Any, Mapping


REGISTRY_PATH = pathlib.Path("ci/workspaces.toml")
GLOB_CHARS = frozenset("*?[]{}")


class RegistryError(RuntimeError):
    """Raised when workspace authority is missing, unsafe, or inconsistent."""


@dataclass(frozen=True)
class CheckOperation:
    command: tuple[str, ...]
    mutates: bool
    local_preflight: bool
    workspace_id: str | None = None

    def render(
        self,
        governance: pathlib.Path,
        subject: pathlib.Path,
        workspace: pathlib.Path | None = None,
    ) -> tuple[str, ...]:
        values = {
            "governance": str(governance.resolve()),
            "subject": str(subject.resolve()),
            "workspace": str((workspace or subject).resolve()),
        }
        return tuple(part.format_map(values) for part in self.command)


CHECK_OPERATIONS: dict[str, CheckOperation] = {
    "root_fmt_check": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "fmt-workspace-check-inner", "{workspace}"),
        False,
        True,
        "bolt_v2",
    ),
    "bvs_fmt_check": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "fmt-workspace-check-inner", "{workspace}"),
        False,
        True,
        "backtesting_vertical_slice",
    ),
    "root_deny": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "deny-workspace-inner", "{workspace}"),
        False,
        True,
        "bolt_v2",
    ),
    "bvs_deny": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "deny-workspace-inner", "{workspace}"),
        False,
        True,
        "backtesting_vertical_slice",
    ),
    "source_fence_static": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "source-fence-static-inner", "{subject}"),
        False,
        True,
    ),
    "workflow_lint": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "ci-lint-workflow-inner", "{subject}"),
        False,
        True,
    ),
    "root_fmt_write": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "fmt-workspace-inner", "{workspace}"),
        True,
        False,
        "bolt_v2",
    ),
    "bvs_fmt_write": CheckOperation(
        ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "fmt-workspace-inner", "{workspace}"),
        True,
        False,
        "backtesting_vertical_slice",
    ),
}


@dataclass(frozen=True)
class WorkspaceSpec:
    workspace_id: str
    path: pathlib.PurePosixPath
    manifest: pathlib.PurePosixPath
    lockfile: pathlib.PurePosixPath
    policy: pathlib.PurePosixPath
    members: tuple[pathlib.PurePosixPath, ...]
    cheap_checks: tuple[str, ...]
    formatter_check: str
    formatter_write: str


@dataclass(frozen=True)
class WorkspaceRegistry:
    schema_version: int
    repository_checks: tuple[str, ...]
    workspaces: tuple[WorkspaceSpec, ...]
    exempt_manifests: tuple[pathlib.PurePosixPath, ...]


@dataclass(frozen=True)
class ReconciliationReport:
    workspace_ids: tuple[str, ...]
    manifests: tuple[str, ...]


def _table(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise RegistryError(f"{label} must be a TOML table")
    return value


def _exact_keys(table: dict[str, Any], expected: set[str], label: str) -> None:
    missing = sorted(expected - set(table))
    extra = sorted(set(table) - expected)
    if missing:
        raise RegistryError(f"{label} is missing required keys: {', '.join(missing)}")
    if extra:
        raise RegistryError(f"{label} contains unknown keys: {', '.join(extra)}")


def _string(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise RegistryError(f"{label} must be a non-empty string")
    return value


def _path(value: object, label: str, *, exact: bool = False) -> pathlib.PurePosixPath:
    raw = _string(value, label)
    path = pathlib.PurePosixPath(raw)
    unsafe = path.is_absolute() or ".." in path.parts or "\\" in raw or raw.startswith("~")
    if unsafe:
        raise RegistryError(f"{label} must be a safe repository-relative path")
    if exact and any(char in raw for char in GLOB_CHARS):
        raise RegistryError(f"{label} must be an exact path, not a glob")
    return path


def _string_list(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise RegistryError(f"{label} must be a list of non-empty strings")
    if len(value) != len(set(value)):
        raise RegistryError(f"{label} must not contain duplicates")
    return tuple(value)


def _checks(
    value: object,
    label: str,
    *,
    mutates: bool,
    workspace_id: str | None = None,
) -> tuple[str, ...]:
    checks = _string_list(value, label)
    for check in checks:
        operation = CHECK_OPERATIONS.get(check)
        if operation is None:
            raise RegistryError(f"{label} references unknown check operation {check}")
        if operation.mutates != mutates:
            mode = "mutating formatter" if mutates else "non-mutating local check"
            raise RegistryError(f"{label} operation {check} is not a {mode}")
        if not mutates and not operation.local_preflight:
            raise RegistryError(f"{label} operation {check} is not permitted in local preflight")
        if workspace_id is not None and operation.workspace_id != workspace_id:
            raise RegistryError(
                f"{label} operation {check} belongs to workspace {operation.workspace_id or 'repository'}"
            )
    return checks


def load_registry(repo: pathlib.Path) -> WorkspaceRegistry:
    path = repo / REGISTRY_PATH
    if not path.is_file():
        raise RegistryError(f"{REGISTRY_PATH} is required")
    try:
        with path.open("rb") as handle:
            raw = tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        raise RegistryError(f"{REGISTRY_PATH} is invalid TOML: {exc}") from exc
    _exact_keys(raw, {"schema_version", "repository", "exempt_manifests", "workspaces"}, str(REGISTRY_PATH))
    if raw["schema_version"] != 1:
        raise RegistryError(f"{REGISTRY_PATH}.schema_version must equal 1")
    repository = _table(raw["repository"], "repository")
    _exact_keys(repository, {"cheap_checks"}, "repository")
    repository_checks = _checks(repository["cheap_checks"], "repository.cheap_checks", mutates=False)
    exemptions = _table(raw["exempt_manifests"], "exempt_manifests")
    _exact_keys(exemptions, {"paths"}, "exempt_manifests")
    exempt_manifests = tuple(
        _path(value, f"exempt_manifests.paths[{index}]", exact=True)
        for index, value in enumerate(_string_list(exemptions["paths"], "exempt_manifests.paths"))
    )
    workspaces_raw = _table(raw["workspaces"], "workspaces")
    if not workspaces_raw:
        raise RegistryError("workspaces must register at least one workspace")
    workspaces: list[WorkspaceSpec] = []
    seen_paths: set[pathlib.PurePosixPath] = set()
    seen_manifests: set[pathlib.PurePosixPath] = set()
    expected_keys = {
        "path",
        "manifest",
        "lockfile",
        "policy",
        "members",
        "cheap_checks",
        "formatter_check",
        "formatter_write",
    }
    for workspace_id in sorted(workspaces_raw):
        if not workspace_id.replace("_", "").isalnum():
            raise RegistryError(f"workspace ID {workspace_id!r} must contain only letters, digits, and underscores")
        table = _table(workspaces_raw[workspace_id], f"workspaces.{workspace_id}")
        _exact_keys(table, expected_keys, f"workspaces.{workspace_id}")
        workspace_path = _path(table["path"], f"workspaces.{workspace_id}.path", exact=True)
        manifest = _path(table["manifest"], f"workspaces.{workspace_id}.manifest", exact=True)
        lockfile = _path(table["lockfile"], f"workspaces.{workspace_id}.lockfile", exact=True)
        policy = _path(table["policy"], f"workspaces.{workspace_id}.policy", exact=True)
        members = tuple(
            _path(value, f"workspaces.{workspace_id}.members[{index}]", exact=True)
            for index, value in enumerate(_string_list(table["members"], f"workspaces.{workspace_id}.members"))
        )
        cheap_checks = _checks(
            table["cheap_checks"],
            f"workspaces.{workspace_id}.cheap_checks",
            mutates=False,
            workspace_id=workspace_id,
        )
        formatter_check = _checks(
            [table["formatter_check"]],
            f"workspaces.{workspace_id}.formatter_check",
            mutates=False,
            workspace_id=workspace_id,
        )[0]
        formatter_write = _checks(
            [table["formatter_write"]],
            f"workspaces.{workspace_id}.formatter_write",
            mutates=True,
            workspace_id=workspace_id,
        )[0]
        if formatter_check not in cheap_checks:
            raise RegistryError(f"workspaces.{workspace_id}.formatter_check must be listed in cheap_checks")
        if workspace_path in seen_paths:
            raise RegistryError(f"duplicate workspace path {workspace_path}")
        if manifest in seen_manifests:
            raise RegistryError(f"duplicate workspace manifest {manifest}")
        seen_paths.add(workspace_path)
        seen_manifests.add(manifest)
        workspaces.append(
            WorkspaceSpec(
                workspace_id=workspace_id,
                path=workspace_path,
                manifest=manifest,
                lockfile=lockfile,
                policy=policy,
                members=members,
                cheap_checks=cheap_checks,
                formatter_check=formatter_check,
                formatter_write=formatter_write,
            )
        )
    return WorkspaceRegistry(1, repository_checks, tuple(workspaces), exempt_manifests)


def _inside_repo(repo: pathlib.Path, relative: pathlib.PurePosixPath, label: str) -> pathlib.Path:
    root = repo.resolve()
    candidate = (root / pathlib.Path(relative.as_posix())).resolve(strict=False)
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise RegistryError(f"{label} escapes the repository") from exc
    return candidate


def _discovered_manifests(repo: pathlib.Path) -> tuple[str, ...]:
    result = subprocess.run(
        [
            "git",
            "--no-optional-locks",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "Cargo.toml",
            ":(glob)**/Cargo.toml",
        ],
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or f"exit {result.returncode}"
        raise RegistryError(f"cannot discover Cargo manifests: {detail}")
    return tuple(sorted(set(line for line in result.stdout.splitlines() if line)))


def validate_operation_recipes(
    governance: pathlib.Path,
    *,
    operations: Mapping[str, CheckOperation] = CHECK_OPERATIONS,
) -> None:
    for operation_id, operation in operations.items():
        command = operation.render(governance, governance)
        if len(command) < 6 or command[:2] != ("just", "--justfile") or command[3] != "--working-directory":
            raise RegistryError(f"check operation {operation_id} has an invalid protected Just command")
        recipe = command[5]
        result = subprocess.run(
            ["just", "--justfile", str(governance / "justfile"), "--working-directory", str(governance), "--show", recipe],
            cwd=governance,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise RegistryError(f"check operation {operation_id} references missing private recipe {recipe}")


def reconcile_registry(repo: pathlib.Path, registry: WorkspaceRegistry) -> ReconciliationReport:
    classified: dict[str, str] = {}

    def classify_manifest(relative: pathlib.PurePosixPath, owner: str) -> None:
        key = relative.as_posix()
        lexical = repo / pathlib.Path(key)
        if lexical.is_symlink():
            raise RegistryError(f"Cargo manifest {key} must not be a symlink")
        path = _inside_repo(repo, relative, f"Cargo manifest {key}")
        if not path.is_file():
            raise RegistryError(f"Cargo manifest {key} does not exist")
        if key in classified:
            raise RegistryError(f"Cargo manifest {key} has multiple classifications")
        classified[key] = owner

    for workspace in registry.workspaces:
        for relative, kind in (
            (workspace.manifest, "manifest"),
            (workspace.lockfile, "lockfile"),
            (workspace.policy, "policy"),
        ):
            path = _inside_repo(repo, relative, f"workspace {workspace.workspace_id} {kind}")
            if not path.is_file():
                raise RegistryError(f"workspace {workspace.workspace_id} {kind} does not exist: {relative}")
        workspace_root = _inside_repo(repo, workspace.path, f"workspace {workspace.workspace_id} path")
        if not workspace_root.is_dir():
            raise RegistryError(f"workspace {workspace.workspace_id} path does not exist: {workspace.path}")
        for manifest in (workspace.manifest, *workspace.members):
            classify_manifest(manifest, workspace.workspace_id)
    for manifest in registry.exempt_manifests:
        classify_manifest(manifest, "nongoverned")

    discovered = _discovered_manifests(repo)
    for manifest in discovered:
        if manifest not in classified:
            raise RegistryError(f"unregistered Cargo manifest {manifest}")
    missing = sorted(set(classified) - set(discovered))
    if missing:
        raise RegistryError(f"registered Cargo manifest was not discovered: {missing[0]}")
    return ReconciliationReport(
        workspace_ids=tuple(sorted(workspace.workspace_id for workspace in registry.workspaces)),
        manifests=discovered,
    )
