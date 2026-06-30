#!/usr/bin/env python3
"""Self-tests for the nextest archive fingerprint producer."""

from __future__ import annotations

import contextlib
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "nextest_fingerprint.py"

RUNNERS_CONFIG_TEXT = """
[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"
"""

FINGERPRINT_CONFIG_TEXT = """
[nextest_archive]
schema = 3
profile = "test"
shards = 4
tracked_inputs = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/",
    ".config/nextest.toml",
    "build.rs",
    "gated_source_roots.manifest",
    "justfile",
    "ci/nextest-fingerprint.toml",
    "ci/rust-verification.toml",
    "scripts/config_validators.py",
    "scripts/nextest_fingerprint.py",
    "scripts/root_bin_sidecars.py",
    "scripts/rust_verification.py",
    "scripts/command_understanding.py",
    ".github/actions/setup-environment/action.yml",
    "src/",
    "tests/",
    "config/root.toml",
]

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

ROOT_INPUT_SAFE_EXCLUDES = (
    "src/",
    "tests/",
    "benches/",
    "examples/",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/",
    ".config/",
    ".github/",
    "justfile",
    "ci/rust-verification.toml",
    "scripts/rust_verification.py",
)


@contextlib.contextmanager
def temporary_git_directory():
    tmp = tempfile.mkdtemp()
    try:
        yield tmp
    finally:
        last_error: OSError | None = None
        removed = False
        for attempt in range(5):
            try:
                shutil.rmtree(tmp)
                removed = True
                break
            except FileNotFoundError:
                removed = True
                break
            except OSError as exc:
                last_error = exc
                if attempt == 4:
                    break
                time.sleep(0.1)
        if not removed and last_error is not None:
            raise last_error


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
    write(repo / "rust-toolchain.toml", "[toolchain]\nchannel = \"stable\"\n")
    write(repo / ".cargo" / "config.toml", "[build]\n")
    write(repo / ".config" / "nextest.toml", "[profile.default]\n")
    write(repo / ".github" / "actions" / "setup-environment" / "action.yml", "name: setup\n")
    write(repo / ".github" / "workflows" / "ci.yml", "name: ci\n")
    write(repo / "ci" / "rust-verification.toml", "[local_compile_policy]\n")
    write(repo / "justfile", "default:\n    @true\n")
    write(repo / "gated_source_roots.manifest", "src\n")
    write(repo / "deploy" / "install.sh", "#!/usr/bin/env bash\necho install\n")
    write(repo / "build.rs", "fn main() {}\n")
    write(repo / "src" / "lib.rs", "pub fn root() {}\n")
    write(repo / "tests" / "root.rs", "#[test]\nfn root_test() {}\n")
    write(repo / "benches" / "root.rs", "fn main() {}\n")
    write(repo / "examples" / "root.rs", "fn main() {}\n")
    write(repo / "scripts" / "config_validators.py", "# tracked config helper placeholder\n")
    write(repo / "scripts" / "nextest_fingerprint.py", "# tracked producer placeholder\n")
    write(repo / "scripts" / "root_bin_sidecars.py", "# tracked sidecar helper placeholder\n")
    write(repo / "scripts" / "rust_verification.py", "# tracked verifier placeholder\n")
    write(repo / "scripts" / "command_understanding.py", "# tracked command parser placeholder\n")
    write(repo / "config" / "root.toml", "[root]\n")
    write(repo / "contracts" / "root.md", "# contract\n")
    write(repo / "docs" / "bolt-v3" / "index.md", "# bolt-v3\n")
    write(
        repo / "docs" / "bolt-v3" / "2026-04-25-bolt-v3-runtime-contracts.md",
        "# runtime contracts\n",
    )
    write(repo / "docs" / "extra" / "index.md", "# extra docs\n")
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
    env = os.environ.copy()
    env["GITHUB_OUTPUT"] = str(repo.parent / "github-output.txt")
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
        env=env,
        check=False,
    )


def assert_fingerprint_outputs_have_provenance_shape() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        key, outputs = fingerprint(repo)
        shape = r"^nextest-archive-v3-Linux-X64-test-profile-shards-4-[0-9a-f]{64}$"
        if re.fullmatch(shape, key) is None:
            raise AssertionError(key)
        if outputs.get("nextest_digest", "") != key.rsplit("-", 1)[-1]:
            raise AssertionError(outputs)
        if outputs.get("nextest_fingerprint") != key:
            raise AssertionError(outputs)
        artifact_shape = r"^nextest-archive-fingerprint-v3-Linux-X64-test-profile-shards-4-[0-9a-f]{64}$"
        if re.fullmatch(artifact_shape, outputs.get("nextest_fingerprint_artifact_name", "")) is None:
            raise AssertionError(outputs)
        if outputs.get("nextest_archive_prefix") != "nextest-archive-":
            raise AssertionError(outputs)
        if outputs.get("nextest_schema") != "3":
            raise AssertionError(outputs)
        if outputs.get("nextest_profile") != "test":
            raise AssertionError(outputs)
        if outputs.get("nextest_shards") != "4":
            raise AssertionError(outputs)


def assert_tree_digest_covers_runtime_inputs_and_mode_bits() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        first, _ = fingerprint(repo)

        write(repo / ".github" / "workflows" / "ci.yml", "name: ci\n# workflow-only change\n")
        commit_all(repo, "change unrelated workflow")
        unrelated_workflow_changed, _ = fingerprint(repo)
        if unrelated_workflow_changed != first:
            raise AssertionError("unrelated workflow changes must not change the nextest archive key")

        write(repo / "docs" / "extra" / "index.md", "# extra docs changed\n")
        commit_all(repo, "change unrelated docs")
        unrelated_docs_changed, _ = fingerprint(repo)
        if unrelated_docs_changed != first:
            raise AssertionError("unrelated docs changes must not change the nextest archive key")

        write(repo / "deploy" / "install.sh", "#!/usr/bin/env bash\necho changed\n")
        commit_all(repo, "change deploy")
        deploy_changed, _ = fingerprint(repo)
        if deploy_changed != first:
            raise AssertionError("runtime-only deploy changes must not change the nextest archive key")

        write(repo / "gated_source_roots.manifest", "src\ndeploy\n")
        commit_all(repo, "change gated manifest")
        manifest_changed, _ = fingerprint(repo)
        if manifest_changed == deploy_changed:
            raise AssertionError("gated_source_roots.manifest changes must change the nextest archive key")

        write(repo / "config" / "root.toml", "[root]\nchanged = true\n")
        commit_all(repo, "change compile-time root config")
        root_config_changed, _ = fingerprint(repo)
        if root_config_changed == manifest_changed:
            raise AssertionError("config/root.toml changes must change the nextest archive key")

        (repo / "build.rs").chmod(0o755)
        commit_all(repo, "make deploy executable")
        mode_changed, _ = fingerprint(repo)
        if mode_changed == root_config_changed:
            raise AssertionError("allowlisted tracked mode changes must change the nextest archive key")


def assert_self_governance_changes_affect_digest() -> None:
    with temporary_git_directory() as tmp:
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


def assert_tracked_inputs_must_match_head_tree() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "ci" / "nextest-fingerprint.toml",
            FINGERPRINT_CONFIG_TEXT.replace(
                '    "src/",\n',
                '    "missing-build-input/",\n    "src/",\n',
            ),
        )
        commit_all(repo, "add stale tracked input")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("tracked_inputs entries that match no tracked files must fail closed")
        if "nextest_archive.tracked_inputs entry matches no tracked files" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_compile_time_include_targets_must_be_tracked() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            'pub const INSTALL_SCRIPT: &str = include_str!("../deploy/install.sh");\n',
        )
        commit_all(repo, "include untracked file")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("compile-time include targets outside tracked_inputs must fail closed")
        if "compile-time include target is outside nextest tracked inputs: deploy/install.sh" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_compile_time_include_targets_must_not_be_prose_docs() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "ci" / "nextest-fingerprint.toml",
            FINGERPRINT_CONFIG_TEXT.replace(
                '    "config/root.toml",\n',
                '    "config/root.toml",\n    "docs/extra/index.md",\n',
            ),
        )
        write(
            repo / "src" / "lib.rs",
            'pub const EXTRA_DOC: &str = include_str!("../docs/extra/index.md");\n',
        )
        commit_all(repo, "include tracked prose doc")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("compile-time include targets for prose docs must fail closed")
        if "compile-time include target must not be a prose doc: docs/extra/index.md" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_commented_compile_time_include_targets_are_ignored() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            '// include_str!("../docs/extra/index.md")\n'
            'pub const ROOT: &str = include_str!("../tests/root.rs");\n',
        )
        commit_all(repo, "commented include")
        fingerprint(repo)


def assert_string_literal_include_text_is_ignored() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            'pub const DOC_EXAMPLE: &str = r#"\n'
            'let _ = include_str!("../docs/extra/index.md");\n'
            '"#;\n'
            'pub const ESCAPED_EXAMPLE: &str = "include_bytes!(\\"../docs/extra/index.md\\")";\n'
            'pub const ROOT: &str = include_str!("../tests/root.rs");\n',
        )
        commit_all(repo, "string literal include examples")
        fingerprint(repo)


def assert_compile_time_include_macro_syntax_variants_are_detected() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            'pub const COMMENT_AFTER_NAME: &str = include_str /* comment */ !("../deploy/install.sh");\n'
            'pub const COMMENT_AFTER_BANG: &str = include_str! /* comment */ ("../deploy/install.sh");\n'
            'pub const COMMENT_BEFORE_ARG: &[u8] = include_bytes!(/* comment */ "../deploy/install.sh");\n'
            'pub const BRACKET_DELIMITER: &str = include_str!["../deploy/install.sh"];\n'
            'pub const BRACE_DELIMITER: &str = include_str!{ "../deploy/install.sh" };\n',
        )
        commit_all(repo, "include macro syntax variants")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("compile-time include macro syntax variants must fail closed")
        if "compile-time include target is outside nextest tracked inputs: deploy/install.sh" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_compile_time_include_non_literal_arguments_fail_closed() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            'pub const CONCAT_DOC: &str = include_str!(concat!("../docs/extra/", "index.md"));\n',
        )
        commit_all(repo, "include concat expression")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("compile-time include non-literal arguments must fail closed")
        if "compile-time include argument must be a direct string literal" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_compile_time_include_targets_must_be_tracked_files() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        write(
            repo / "src" / "lib.rs",
            'pub const UNTRACKED_FIXTURE: &str = include_str!("../tests/fixtures/untracked.txt");\n',
        )
        git(repo, "add", "src/lib.rs")
        git(repo, "commit", "-m", "include untracked fixture")
        write(repo / "tests" / "fixtures" / "untracked.txt", "not in git\n")
        result = run_fingerprint_expect_failure(repo)
        if result.returncode == 0:
            raise AssertionError("compile-time include targets absent from HEAD must fail closed")
        if "compile-time include target is not tracked in HEAD: tests/fixtures/untracked.txt" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_safe_list_excludes_only_exact_backtester_prefix() -> None:
    with temporary_git_directory() as tmp:
        repo = init_repo(pathlib.Path(tmp))
        first, _ = fingerprint(repo)

        write(repo / "crates" / "backtesting-vertical-slice" / "src" / "lib.rs", "pub fn changed() {}\n")
        commit_all(repo, "change isolated backtester")
        safe_listed, _ = fingerprint(repo)
        if safe_listed != first:
            raise AssertionError("safe-listed isolated backtester changes must not change the root nextest key")

        write(repo / "crates" / "backtesting-vertical-slice-extra" / "src" / "lib.rs", "pub fn sibling() {}\n")
        write(
            repo / "ci" / "nextest-fingerprint.toml",
            FINGERPRINT_CONFIG_TEXT.replace(
                '    "tests/",\n',
                '    "tests/",\n    "crates/backtesting-vertical-slice-extra/",\n',
            ),
        )
        commit_all(repo, "track similarly named sibling")
        sibling_tracked, _ = fingerprint(repo)

        write(repo / "crates" / "backtesting-vertical-slice-extra" / "src" / "lib.rs", "pub fn sibling_changed() {}\n")
        commit_all(repo, "change similarly named sibling")
        sibling, _ = fingerprint(repo)
        if sibling == sibling_tracked:
            raise AssertionError("safe-list matching must not over-match similarly named siblings")


def assert_forbidden_safe_list_entries_fail_closed() -> None:
    for entry in FORBIDDEN_SAFE_EXCLUDES:
        with temporary_git_directory() as tmp:
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


def assert_root_inputs_cannot_be_safe_listed() -> None:
    for entry in ROOT_INPUT_SAFE_EXCLUDES:
        with temporary_git_directory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            write(
                repo / "ci" / "nextest-fingerprint.toml",
                FINGERPRINT_CONFIG_TEXT.replace(
                    'path = "crates/backtesting-vertical-slice/"',
                    f'path = "{entry}"',
                ),
            )
            commit_all(repo, f"reject root input {entry}")
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"safe-listing root input {entry} must fail closed")
            if (
                "safe-listed path overlaps tracked input" not in result.stderr
                and "safe-listed path must be a separate Cargo workspace" not in result.stderr
            ):
                raise AssertionError(result.stderr)


def assert_safe_list_rejects_non_workspace_paths() -> None:
    cases = {
        "non-workspace directory": "docs/extra/",
        "package without workspace": "crates/local-helper/",
    }
    for label, entry in cases.items():
        with temporary_git_directory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            if entry == "crates/local-helper/":
                write(
                    repo / "crates" / "local-helper" / "Cargo.toml",
                    """
[package]
name = "local-helper"
version = "0.1.0"
edition = "2021"
""",
                )
                write(repo / "crates" / "local-helper" / "src" / "lib.rs", "pub fn helper() {}\n")
            write(
                repo / "ci" / "nextest-fingerprint.toml",
                FINGERPRINT_CONFIG_TEXT.replace(
                    'path = "crates/backtesting-vertical-slice/"',
                    f'path = "{entry}"',
                ),
            )
            commit_all(repo, f"reject {label}")
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"safe-listing {label} must fail closed")
            if "safe-listed path must be a separate Cargo workspace" not in result.stderr:
                raise AssertionError(result.stderr)


def assert_safe_list_rejects_root_workspace_membership() -> None:
    with temporary_git_directory() as tmp:
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
    with temporary_git_directory() as tmp:
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
    with temporary_git_directory() as tmp:
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
        with temporary_git_directory() as tmp:
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
        "missing schema": (
            FINGERPRINT_CONFIG_TEXT.replace("schema = 3\n", ""),
            "nextest_archive.schema must be a positive integer",
        ),
        "missing profile": (
            FINGERPRINT_CONFIG_TEXT.replace('profile = "test"\n', ""),
            "nextest_archive.profile must be a non-empty string",
        ),
        "missing tracked inputs": (
            re.sub(r"tracked_inputs = \[[\s\S]*?\]\n\n", "", FINGERPRINT_CONFIG_TEXT),
            "nextest_archive.tracked_inputs must be a non-empty string list",
        ),
        "tracked inputs missing source root": (
            FINGERPRINT_CONFIG_TEXT.replace('    "src/",\n', ""),
            "nextest_archive.tracked_inputs must include src/",
        ),
        "tracked inputs missing config validators": (
            FINGERPRINT_CONFIG_TEXT.replace('    "scripts/config_validators.py",\n', ""),
            "nextest_archive.tracked_inputs must include scripts/config_validators.py",
        ),
        "malformed toml": ("[nextest_archive\n", "nextest fingerprint config invalid TOML"),
    }
    for label, (text, expected_error) in cases.items():
        with temporary_git_directory() as tmp:
            repo = init_repo(pathlib.Path(tmp))
            write(repo / "ci" / "nextest-fingerprint.toml", text)
            commit_all(repo, label)
            result = run_fingerprint_expect_failure(repo)
            if result.returncode == 0:
                raise AssertionError(f"{label} must fail closed")
            if expected_error not in result.stderr:
                raise AssertionError(result.stderr)

    with temporary_git_directory() as tmp:
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
    assert_tracked_inputs_must_match_head_tree()
    assert_compile_time_include_targets_must_be_tracked()
    assert_compile_time_include_targets_must_not_be_prose_docs()
    assert_commented_compile_time_include_targets_are_ignored()
    assert_string_literal_include_text_is_ignored()
    assert_compile_time_include_macro_syntax_variants_are_detected()
    assert_compile_time_include_non_literal_arguments_fail_closed()
    assert_compile_time_include_targets_must_be_tracked_files()
    assert_safe_list_excludes_only_exact_backtester_prefix()
    assert_forbidden_safe_list_entries_fail_closed()
    assert_root_inputs_cannot_be_safe_listed()
    assert_safe_list_rejects_non_workspace_paths()
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
