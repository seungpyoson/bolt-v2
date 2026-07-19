#!/usr/bin/env python3
"""Self-tests for the authoritative workspace registry."""

from __future__ import annotations

import pathlib
import subprocess
import tempfile
import textwrap

from workspace_registry import CheckOperation, RegistryError, load_registry, reconcile_registry, validate_operation_recipes


ROOT_REGISTRY = """
schema_version = 1

[repository]
cheap_checks = ["source_fence_static", "workflow_lint"]

[exempt_manifests]
paths = []

[workspaces.bolt_v2]
path = "."
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
policy = "ci/rust-verification.toml"
members = []
cheap_checks = ["root_fmt_check", "root_deny"]
formatter_check = "root_fmt_check"
formatter_write = "root_fmt_write"
"""

BVS_REGISTRY = """
[workspaces.backtesting_vertical_slice]
path = "crates/backtesting-vertical-slice"
manifest = "crates/backtesting-vertical-slice/Cargo.toml"
lockfile = "crates/backtesting-vertical-slice/Cargo.lock"
policy = "crates/backtesting-vertical-slice/ci/rust-verification.toml"
members = []
cheap_checks = ["bvs_fmt_check", "bvs_deny"]
formatter_check = "bvs_fmt_check"
formatter_write = "bvs_fmt_write"
"""


def git(repo: pathlib.Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def write(path: pathlib.Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content).lstrip(), encoding="utf-8")


def fixture_repo(
    *,
    include_bvs: bool = False,
    bvs_path: str = "crates/backtesting-vertical-slice",
) -> tuple[tempfile.TemporaryDirectory[str], pathlib.Path]:
    tmp = tempfile.TemporaryDirectory()
    repo = pathlib.Path(tmp.name)
    git(repo, "init", "-b", "main")
    git(repo, "config", "user.name", "Workspace Registry Test")
    git(repo, "config", "user.email", "workspace-registry@example.invalid")
    write(repo / "Cargo.toml", "[package]\nname='root'\nversion='0.0.0'\n")
    write(repo / "Cargo.lock", "# lock\n")
    write(repo / "ci/rust-verification.toml", "schema_version = 2\n")
    write(
        repo / "justfile",
        """
        [private]
        fmt-workspace-check-inner workspace:
            @true
        [private]
        deny-workspace-inner workspace:
            @true
        [private]
        source-fence-static-inner subject='.':
            @true
        [private]
        ci-lint-workflow-inner subject='.':
            @true
        [private]
        fmt-workspace-inner workspace:
            @true
        """,
    )
    registry = ROOT_REGISTRY
    if include_bvs:
        write(
            repo / bvs_path / "Cargo.toml",
            "[workspace]\n\n[package]\nname='bvs'\nversion='0.0.0'\n",
        )
        write(repo / bvs_path / "Cargo.lock", "# lock\n")
        write(
            repo / bvs_path / "ci/rust-verification.toml",
            "schema_version = 2\n",
        )
        registry += BVS_REGISTRY.replace("crates/backtesting-vertical-slice", bvs_path)
    write(repo / "ci/workspaces.toml", registry)
    git(repo, "add", ".")
    git(repo, "commit", "-m", "fixture")
    return tmp, repo


def expect_error(repo: pathlib.Path, expected: str) -> None:
    try:
        reconcile_registry(repo, load_registry(repo))
    except RegistryError as exc:
        if expected not in str(exc):
            raise AssertionError(f"expected {expected!r}, got {str(exc)!r}") from exc
    else:
        raise AssertionError(f"expected RegistryError containing {expected!r}")


def assert_root_and_bvs_reconcile() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    try:
        report = reconcile_registry(repo, load_registry(repo))
        if report.workspace_ids != ("backtesting_vertical_slice", "bolt_v2"):
            raise AssertionError(report.workspace_ids)
        if report.manifests != ("Cargo.toml", "crates/backtesting-vertical-slice/Cargo.toml"):
            raise AssertionError(report.manifests)
        registry = load_registry(repo)
        by_id = {workspace.workspace_id: workspace for workspace in registry.workspaces}
        if by_id["bolt_v2"].cheap_checks != ("root_fmt_check", "root_deny"):
            raise AssertionError(by_id["bolt_v2"].cheap_checks)
        if by_id["backtesting_vertical_slice"].cheap_checks != ("bvs_fmt_check", "bvs_deny"):
            raise AssertionError(by_id["backtesting_vertical_slice"].cheap_checks)
        if by_id["backtesting_vertical_slice"].formatter_write != "bvs_fmt_write":
            raise AssertionError(by_id["backtesting_vertical_slice"].formatter_write)
    finally:
        tmp.cleanup()


def assert_workspace_cannot_remap_checks_to_another_workspace() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    try:
        registry = (repo / "ci/workspaces.toml").read_text(encoding="utf-8")
        registry = registry.replace(
            'cheap_checks = ["bvs_fmt_check", "bvs_deny"]\nformatter_check = "bvs_fmt_check"\nformatter_write = "bvs_fmt_write"',
            'cheap_checks = ["root_fmt_check", "root_deny"]\nformatter_check = "root_fmt_check"\nformatter_write = "root_fmt_write"',
        )
        (repo / "ci/workspaces.toml").write_text(registry, encoding="utf-8")
        try:
            load_registry(repo)
        except RegistryError as exc:
            if "belongs to workspace bolt_v2" not in str(exc):
                raise AssertionError(str(exc)) from exc
        else:
            raise AssertionError("BVS accepted root workspace operations")
    finally:
        tmp.cleanup()


def assert_unregistered_tracked_manifest_fails() -> None:
    tmp, repo = fixture_repo()
    try:
        write(repo / "crates/new/Cargo.toml", "[package]\nname='new'\nversion='0.0.0'\n")
        git(repo, "add", "crates/new/Cargo.toml")
        expect_error(repo, "unregistered Cargo manifest crates/new/Cargo.toml")
    finally:
        tmp.cleanup()


def assert_unregistered_untracked_manifest_fails() -> None:
    tmp, repo = fixture_repo()
    try:
        write(repo / "scratch/Cargo.toml", "[package]\nname='scratch'\nversion='0.0.0'\n")
        expect_error(repo, "unregistered Cargo manifest scratch/Cargo.toml")
    finally:
        tmp.cleanup()


def assert_stale_registry_path_fails() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    try:
        (repo / "crates/backtesting-vertical-slice/Cargo.toml").unlink()
        expect_error(repo, "manifest does not exist")
    finally:
        tmp.cleanup()


def assert_member_and_exempt_manifest_symlinks_fail() -> None:
    for registry_key in ("members", "paths"):
        tmp, repo = fixture_repo()
        external = pathlib.Path(tmp.name).parent / f"external-{pathlib.Path(tmp.name).name}.toml"
        try:
            external.write_text("[package]\nname='external'\nversion='0.0.0'\n", encoding="utf-8")
            (repo / "linked").mkdir()
            (repo / "linked/Cargo.toml").symlink_to(external)
            registry = (repo / "ci/workspaces.toml").read_text(encoding="utf-8")
            registry = registry.replace(f"{registry_key} = []", f'{registry_key} = ["linked/Cargo.toml"]')
            (repo / "ci/workspaces.toml").write_text(registry, encoding="utf-8")
            git(repo, "add", "ci/workspaces.toml", "linked/Cargo.toml")
            expect_error(repo, "must not be a symlink")
        finally:
            external.unlink(missing_ok=True)
            tmp.cleanup()


def assert_unsafe_and_unknown_fields_fail() -> None:
    tmp, repo = fixture_repo()
    try:
        registry = repo / "ci/workspaces.toml"
        registry.write_text(registry.read_text(encoding="utf-8").replace('path = "."', 'path = "../escape"'), encoding="utf-8")
        try:
            load_registry(repo)
        except RegistryError as exc:
            if "safe repository-relative path" not in str(exc):
                raise
        else:
            raise AssertionError("unsafe workspace path was accepted")

        registry.write_text(ROOT_REGISTRY.replace('cheap_checks = ["root_fmt_check", "root_deny"]', 'cheap_checks = ["cargo_test"]'), encoding="utf-8")
        try:
            load_registry(repo)
        except RegistryError as exc:
            if "unknown check operation cargo_test" not in str(exc):
                raise
        else:
            raise AssertionError("compile-heavy unknown check was accepted")
    finally:
        tmp.cleanup()


def assert_nongoverned_paths_are_exact() -> None:
    tmp, repo = fixture_repo()
    try:
        registry = repo / "ci/workspaces.toml"
        registry.write_text(ROOT_REGISTRY.replace("paths = []", 'paths = ["fixtures/**/Cargo.toml"]'), encoding="utf-8")
        try:
            load_registry(repo)
        except RegistryError as exc:
            if "must be an exact path" not in str(exc):
                raise
        else:
            raise AssertionError("glob exclusion was accepted")
    finally:
        tmp.cleanup()


def assert_repository_registry_reconciles() -> None:
    repo = pathlib.Path(__file__).resolve().parents[1]
    report = reconcile_registry(repo, load_registry(repo))
    if report.workspace_ids != ("backtesting_vertical_slice", "bolt_v2"):
        raise AssertionError(report.workspace_ids)


def assert_operation_recipe_resolution_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        repo = pathlib.Path(tmp_raw)
        write(repo / "justfile", "[private]\nexisting subject='.':\n    @true\n")
        operations = {
            "existing": CheckOperation(
                ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "existing", "{subject}"),
                False,
                True,
            ),
            "missing": CheckOperation(
                ("just", "--justfile", "{governance}/justfile", "--working-directory", "{governance}", "missing", "{subject}"),
                False,
                True,
            ),
        }
        try:
            validate_operation_recipes(repo, operations=operations)
        except RegistryError as exc:
            if "missing" not in str(exc):
                raise
        else:
            raise AssertionError("dangling private recipe was accepted")


def main() -> int:
    assert_root_and_bvs_reconcile()
    assert_workspace_cannot_remap_checks_to_another_workspace()
    assert_unregistered_tracked_manifest_fails()
    assert_unregistered_untracked_manifest_fails()
    assert_stale_registry_path_fails()
    assert_member_and_exempt_manifest_symlinks_fail()
    assert_unsafe_and_unknown_fields_fail()
    assert_nongoverned_paths_are_exact()
    assert_repository_registry_reconciles()
    assert_operation_recipe_resolution_fails_closed()
    print("OK: workspace registry tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
