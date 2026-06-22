#!/usr/bin/env python3
"""Self-tests for the nextest archive fingerprint producer."""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "nextest_fingerprint.py"

RUNNERS_CONFIG_TEXT = """
[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"
"""

FINGERPRINT_CONFIG_TEXT = """
[nextest_archive]
schema = 2
profile = "test"
shards = 4

[[safe_excludes]]
path = "crates/backtesting-vertical-slice/"
justification = "Separate Cargo workspace; root package has no path dependency on it and root nextest does not run it."
"""

FORBIDDEN_SAFE_EXCLUDES = (
    "deploy/",
    "gated_source_roots.manifest",
    "config/",
    "contracts/",
    "docs/bolt-v3/",
    "specs/",
    "ci/nextest-fingerprint.toml",
    "scripts/nextest_fingerprint.py",
)


def run(
    args: list[str],
    *,
    cwd: pathlib.Path,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed: {' '.join(args)}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def git(repo: pathlib.Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run(["git", *args], cwd=repo)


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def commit_all(repo: pathlib.Path, message: str) -> None:
    git(repo, "add", ".")
    git(repo, "commit", "-m", message)


def init_repo(tmp_path: pathlib.Path, *, cargo_toml: str | None = None) -> pathlib.Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init")
    git(repo, "config", "user.email", "ci@example.invalid")
    git(repo, "config", "user.name", "CI Test")
    git(repo, "config", "core.filemode", "true")
    git(repo, "config", "core.hooksPath", "/dev/null")
    write(repo / "ci" / "github-actions-runners.toml", RUNNERS_CONFIG_TEXT)
    write(repo / "ci" / "nextest-fingerprint.toml", FINGERPRINT_CONFIG_TEXT)
    write(
        repo / "Cargo.toml",
        cargo_toml
        or """
[package]
name = "root"
version = "0.1.0"
edition = "2021"
""",
    )
    write(repo / "Cargo.lock", "# lock\n")
    write(repo / "gated_source_roots.manifest", "src\n")
    write(repo / "deploy" / "install.sh", "#!/usr/bin/env bash\necho install\n")
    write(repo / "src" / "lib.rs", "pub fn root() {}\n")
    write(repo / "scripts" / "nextest_fingerprint.py", "# tracked producer placeholder\n")
    write(repo / "config" / "root.toml", "[root]\n")
    write(repo / "contracts" / "root.md", "# contract\n")
    write(repo / "docs" / "bolt-v3" / "index.md", "# bolt-v3\n")
    write(repo / "specs" / "root.md", "# spec\n")
    write(repo / "crates" / "backtesting-vertical-slice" / "src" / "lib.rs", "pub fn bvs() {}\n")
    write(
        repo / "crates" / "backtesting-vertical-slice" / "Cargo.toml",
        """
[workspace]

[package]
name = "backtesting-vertical-slice"
version = "0.1.0"
edition = "2021"
""",
    )
    commit_all(repo, "initial")
    return repo


def fingerprint(repo: pathlib.Path) -> tuple[str, dict[str, str]]:
    output_path = repo.parent / "cache-key.txt"
    github_output = repo.parent / "github-output.txt"
    env = os.environ.copy()
    env["GITHUB_OUTPUT"] = str(github_output)
    run(
        [
            "python3",
            str(SCRIPT_PATH),
            "--repo-root",
            str(repo),
            "--config",
            str(repo / "ci" / "nextest-fingerprint.toml"),
            "--runners-config",
            str(repo / "ci" / "github-actions-runners.toml"),
            "--runner-os",
            "Linux",
            "--runner-arch",
            "X64",
            "--output-path",
            str(output_path),
        ],
        cwd=repo,
        env=env,
    )
    key = output_path.read_text(encoding="utf-8").strip()
    outputs = parse_github_outputs(github_output.read_text(encoding="utf-8"))
    return key, outputs


def parse_github_outputs(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in text.splitlines():
        name, separator, value = line.partition("=")
        if not separator:
            raise AssertionError(f"invalid GitHub output line: {line!r}")
        values[name] = value
    return values


def run_fingerprint_expect_failure(repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return run(
        [
            "python3",
            str(SCRIPT_PATH),
            "--repo-root",
            str(repo),
            "--config",
            str(repo / "ci" / "nextest-fingerprint.toml"),
            "--runners-config",
            str(repo / "ci" / "github-actions-runners.toml"),
            "--runner-os",
            "Linux",
            "--runner-arch",
            "X64",
            "--output-path",
            str(repo.parent / "cache-key.txt"),
        ],
        cwd=repo,
        check=False,
    )


def assert_fingerprint_outputs_have_provenance_shape() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        key, outputs = fingerprint(repo)
        shape = r"^nextest-archive-v2-Linux-X64-test-profile-shards-4-[0-9a-f]{64}$"
        if re.fullmatch(shape, key) is None:
            raise AssertionError(key)
        if outputs.get("nextest_digest", "") != key.rsplit("-", 1)[-1]:
            raise AssertionError(outputs)
        if outputs.get("nextest_fingerprint") != key:
            raise AssertionError(outputs)
        artifact_shape = r"^nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-[0-9a-f]{64}$"
        if re.fullmatch(artifact_shape, outputs.get("nextest_fingerprint_artifact_name", "")) is None:
            raise AssertionError(outputs)
        if outputs.get("nextest_archive_prefix") != "nextest-archive-":
            raise AssertionError(outputs)
        if outputs.get("nextest_schema") != "2":
            raise AssertionError(outputs)
        if outputs.get("nextest_profile") != "test":
            raise AssertionError(outputs)
        if outputs.get("nextest_shards") != "4":
            raise AssertionError(outputs)


def assert_tree_digest_covers_runtime_inputs_and_mode_bits() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        first, _ = fingerprint(repo)

        write(repo / "deploy" / "install.sh", "#!/usr/bin/env bash\necho changed\n")
        commit_all(repo, "change deploy")
        deploy_changed, _ = fingerprint(repo)
        if deploy_changed == first:
            raise AssertionError("deploy changes must change the nextest archive key")

        write(repo / "gated_source_roots.manifest", "src\ndeploy\n")
        commit_all(repo, "change gated manifest")
        manifest_changed, _ = fingerprint(repo)
        if manifest_changed == deploy_changed:
            raise AssertionError("gated_source_roots.manifest changes must change the nextest archive key")

        (repo / "deploy" / "install.sh").chmod(0o755)
        commit_all(repo, "make deploy executable")
        mode_changed, _ = fingerprint(repo)
        if mode_changed == manifest_changed:
            raise AssertionError("tracked mode changes must change the nextest archive key")


def assert_self_governance_changes_affect_digest() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        first, _ = fingerprint(repo)

        write(repo / "ci" / "nextest-fingerprint.toml", FINGERPRINT_CONFIG_TEXT + "\n# governed\n")
        commit_all(repo, "change fingerprint config")
        config_changed, _ = fingerprint(repo)
        if config_changed == first:
            raise AssertionError("fingerprint config changes must change the digest")

        write(repo / "scripts" / "nextest_fingerprint.py", "# tracked producer placeholder changed\n")
        commit_all(repo, "change fingerprint producer")
        script_changed, _ = fingerprint(repo)
        if script_changed == config_changed:
            raise AssertionError("fingerprint producer changes must change the digest")


def assert_safe_list_excludes_only_exact_backtester_prefix() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        first, _ = fingerprint(repo)

        write(repo / "crates" / "backtesting-vertical-slice" / "src" / "lib.rs", "pub fn changed() {}\n")
        commit_all(repo, "change isolated backtester")
        safe_listed, _ = fingerprint(repo)
        if safe_listed != first:
            raise AssertionError("safe-listed isolated backtester changes must not change the root nextest key")

        write(repo / "crates" / "backtesting-vertical-slice-extra" / "src" / "lib.rs", "pub fn sibling() {}\n")
        commit_all(repo, "add similarly named sibling")
        sibling, _ = fingerprint(repo)
        if sibling == safe_listed:
            raise AssertionError("safe-list matching must not over-match similarly named siblings")


def assert_forbidden_safe_list_entries_fail_closed() -> None:
    for entry in FORBIDDEN_SAFE_EXCLUDES:
        with tempfile.TemporaryDirectory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            write(
                repo / "ci" / "nextest-fingerprint.toml",
                FINGERPRINT_CONFIG_TEXT.replace(
                    'path = "crates/backtesting-vertical-slice/"',
                    f'path = "{entry}"',
                ),
            )
            commit_all(repo, f"forbid {entry}")
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"safe-listing {entry} must fail closed")
            if "safe-listed path is forbidden" not in result.stderr:
                raise AssertionError(result.stderr)


def assert_safe_list_rejects_root_workspace_membership() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(
            pathlib.Path(tmp),
            cargo_toml="""
[workspace]
members = ["crates/backtesting-vertical-slice"]
""",
        )
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("root workspace safe-list membership must fail closed")
        if "safe-listed path is a root workspace member" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_safe_list_rejects_root_path_dependencies() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(
            pathlib.Path(tmp),
            cargo_toml="""
[package]
name = "root"
version = "0.1.0"
edition = "2021"

[dependencies]
backtesting-vertical-slice = { path = "crates/backtesting-vertical-slice" }
""",
        )
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("root path dependency safe-list must fail closed")
        if "safe-listed path is referenced by root Cargo.toml path dependency" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_dirty_worktree_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(repo / "Cargo.toml", (repo / "Cargo.toml").read_text(encoding="utf-8") + "\n# dirty\n")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("dirty worktree must fail closed")
        if "worktree must match HEAD before computing nextest fingerprint" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_invalid_shards_fail_closed() -> None:
    cases = {
        "missing": FINGERPRINT_CONFIG_TEXT.replace("shards = 4\n", ""),
        "zero": FINGERPRINT_CONFIG_TEXT.replace("shards = 4", "shards = 0"),
        "non-numeric": FINGERPRINT_CONFIG_TEXT.replace("shards = 4", 'shards = "4"'),
    }
    for label, text in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            write(repo / "ci" / "nextest-fingerprint.toml", text)
            commit_all(repo, f"invalid shards {label}")
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"{label} shards must fail closed")
            if "nextest_archive.shards must be a positive integer" not in result.stderr:
                raise AssertionError(result.stderr)


def assert_missing_or_malformed_config_fails_closed() -> None:
    cases = {
        "missing schema": FINGERPRINT_CONFIG_TEXT.replace("schema = 2\n", ""),
        "missing profile": FINGERPRINT_CONFIG_TEXT.replace('profile = "test"\n', ""),
        "malformed toml": "[nextest_archive\n",
    }
    for label, text in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            write(repo / "ci" / "nextest-fingerprint.toml", text)
            commit_all(repo, label)
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"{label} must fail closed")

    with tempfile.TemporaryDirectory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(repo / "ci" / "github-actions-runners.toml", "[meter]\n")
        commit_all(repo, "missing meter prefix")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("missing meter prefix must fail closed")
        if "meter.fingerprint_artifact_prefix must be a non-empty string" not in result.stderr:
            raise AssertionError(result.stderr)


def main() -> int:
    assert_fingerprint_outputs_have_provenance_shape()
    assert_tree_digest_covers_runtime_inputs_and_mode_bits()
    assert_self_governance_changes_affect_digest()
    assert_safe_list_excludes_only_exact_backtester_prefix()
    assert_forbidden_safe_list_entries_fail_closed()
    assert_safe_list_rejects_root_workspace_membership()
    assert_safe_list_rejects_root_path_dependencies()
    assert_dirty_worktree_fails_closed()
    assert_invalid_shards_fail_closed()
    assert_missing_or_malformed_config_fails_closed()
    print("OK: nextest fingerprint self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
