#!/usr/bin/env python3
"""Relocated CI workflow hygiene analyzer tests."""

from __future__ import annotations

import errno
import json
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import time

import ci_workflow_hygiene_test_helpers as hygiene_helpers
from ci_workflow_hygiene_test_helpers import (
    BASE_WORKFLOW,
    DEBUG_TEST_WORKFLOW_PATH,
    assert_error,
    commit_repo,
    copy_self_authorizing_base_tree,
    init_self_authorizing_fixture_repo,
    load_verifier,
    replace_once,
    replace_once_after,
    repo_source_text,
    repo_workflow_text,
    run_repo_git,
    write_repo_text,
)

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]

def assert_run_repo_git_suppresses_background_maintenance() -> None:
    calls: list[tuple[str, ...]] = []
    original_run = hygiene_helpers.subprocess.run

    def fake_run(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(tuple(command))
        return subprocess.CompletedProcess(command, 0, "ok\n", "")

    hygiene_helpers.subprocess.run = fake_run
    try:
        output = hygiene_helpers.run_repo_git(pathlib.Path("/tmp/repo"), "status")
    finally:
        hygiene_helpers.subprocess.run = original_run

    if output != "ok\n":
        raise AssertionError(f"run_repo_git must return stdout, got: {output!r}")
    expected = (
        "git",
        *hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS,
        "status",
    )
    if calls != [expected]:
        raise AssertionError(f"run_repo_git must suppress background maintenance, got: {calls}")


def assert_suppression_args_match_suppression_config() -> None:
    """The command and persisted-config suppression paths must stay in agreement."""
    argv = hygiene_helpers.repo_git_command()
    if argv[0] != "git":
        raise AssertionError(f"repo_git_command must start with 'git', got: {argv!r}")

    parsed_items: list[tuple[str, str]] = []
    tokens = argv[1:]
    index = 0
    while index < len(tokens):
        if tokens[index] != "-c":
            raise AssertionError(
                "repo_git_command suppression args must be flat '-c KEY=VALUE' pairs, "
                f"got unexpected token {tokens[index]!r} at argv index {index + 1}: {argv!r}"
            )
        if index + 1 >= len(tokens):
            raise AssertionError(
                "repo_git_command suppression args must provide KEY=VALUE after every '-c', "
                f"got: {argv!r}"
            )
        key, separator, value = tokens[index + 1].partition("=")
        if not separator or not key:
            raise AssertionError(
                "repo_git_command suppression settings must use non-empty KEY=VALUE syntax, "
                f"got {tokens[index + 1]!r}: {argv!r}"
            )
        parsed_items.append((key, value))
        index += 2

    parsed_config = dict(parsed_items)
    declared_config = dict(hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG)
    if parsed_config != declared_config:
        raise AssertionError(
            "repo_git_command suppression settings must match the persisted suppression config, "
            f"got {parsed_config!r}, want {declared_config!r}"
        )

    parsed_order = tuple(key for key, _value in parsed_items)
    declared_order = tuple(
        key for key, _value in hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG
    )
    if parsed_order != declared_order:
        raise AssertionError(
            "repo_git_command suppression key order must match the persisted suppression config, "
            f"got {parsed_order!r}, want {declared_order!r}"
        )


def _maintenance_children(trace_path: pathlib.Path) -> int:
    """`git maintenance`/`git gc` processes recorded in a GIT_TRACE2 event log."""
    if not trace_path.exists():
        return 0
    total = 0
    for line in trace_path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("event") != "child_start":
            continue
        argv = " ".join(event.get("argv", []))
        if "maintenance" in argv or argv.startswith("git gc"):
            total += 1
    return total


def assert_init_fixture_repo_persists_suppression() -> None:
    """A fixture remote must carry the suppression in its own config.

    Git drops the repo-scoped config environment when it runs against another
    repository, so `-c gc.auto=0` on a `git push` never reaches the remote's
    `receive-pack`.
    """
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        bare = hygiene_helpers.init_fixture_repo(root / "origin.git", "--bare")
        for key, value in hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
            actual = hygiene_helpers.read_persisted_repo_config(bare, key)
            if actual is None:
                raise AssertionError(
                    f"fixture remote never persisted {key!r}; "
                    "a git process launched outside this suite will not see it"
                )
            if actual != value:
                raise AssertionError(f"fixture remote {key}={actual!r}, want {value!r}")


def assert_self_authorizing_fixture_repo_persists_suppression() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = hygiene_helpers.init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        for key, value in hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
            actual = hygiene_helpers.read_persisted_repo_config(repo, key)
            if actual != value:
                raise AssertionError(
                    f"self-authorizing fixture repo {key}={actual!r}, want {value!r}"
                )


def assert_clone_fixture_repo_persists_suppression() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        source = hygiene_helpers.init_fixture_repo(root / "origin.git", "--bare")
        clone = hygiene_helpers.clone_fixture_repo(source, root / "clone")
        plain_clone = hygiene_helpers.clone_fixture_repo_without_suppression(
            source, root / "plain-clone"
        )

        for key, value in hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
            actual = hygiene_helpers.read_persisted_repo_config(clone, key)
            if actual != value:
                raise AssertionError(f"fixture clone {key}={actual!r}, want {value!r}")
            plain_actual = hygiene_helpers.read_persisted_repo_config(plain_clone, key)
            if plain_actual is not None:
                raise AssertionError(
                    f"plain git clone unexpectedly persisted {key}={plain_actual!r}"
                )


def assert_push_to_fixture_remote_spawns_no_background_maintenance() -> None:
    """Push into a helper-built remote and prove `receive-pack` detaches nothing."""
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        bare = hygiene_helpers.init_fixture_repo(root / "origin.git", "--bare")
        work = hygiene_helpers.init_fixture_repo(root / "work", "-b", "main")
        run_repo_git(work, "config", "user.email", "fixture@example.invalid")
        run_repo_git(work, "config", "user.name", "Fixture")
        run_repo_git(work, "commit", "--allow-empty", "-m", "seed")
        run_repo_git(work, "remote", "add", "origin", str(bare))

        trace = root / "trace.json"
        environ = dict(os.environ, GIT_TRACE2_EVENT=str(trace))
        subprocess.run(
            hygiene_helpers.repo_git_command("push", "origin", "main"),
            cwd=work,
            env=environ,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        spawned = _maintenance_children(trace)
        if spawned:
            raise AssertionError(
                f"push into a fixture remote spawned {spawned} background maintenance children"
            )


def assert_routed_fixture_module_spawns_no_background_maintenance() -> None:
    """A newly routed fixture suite must not detach a writer into its tmpdir.

    `test_ci_input_sets.py` built its own `git` argv before #1323 and spawned a
    background writer per commit, into a `TemporaryDirectory` it then deleted.
    """
    module = "scripts/test_ci_input_sets.py"
    with tempfile.TemporaryDirectory() as tmp:
        trace = pathlib.Path(tmp) / "trace.json"
        environ = dict(os.environ, GIT_TRACE2_EVENT=str(trace))
        completed = subprocess.run(
            [sys.executable, module],
            cwd=REPO_ROOT,
            env=environ,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if completed.returncode != 0:
            raise AssertionError(f"{module} failed: {completed.stderr[-2000:]}")
        spawned = _maintenance_children(trace)
        if spawned:
            raise AssertionError(f"{module} spawned {spawned} background maintenance children")


def assert_temp_git_fixture_cleanup_stress_blocks_background_writer() -> None:
    real_git = shutil.which("git")
    if real_git is None:
        raise AssertionError("git executable not found")
    writer_code = """
from __future__ import annotations
import os
import pathlib
import time
repo = pathlib.Path(os.environ["BOLT1323_WRITER_REPO"])
log_path = pathlib.Path(os.environ["BOLT1323_WRITER_LOG"])
objects = repo / ".git" / "objects"
deadline = time.time() + 1.0
count = 0
while time.time() < deadline:
    try:
        target_dir = objects / "zz"
        target_dir.mkdir(parents=True, exist_ok=True)
        (target_dir / f"race-{os.getpid()}-{count}").write_text("x", encoding="utf-8")
        with log_path.open("a", encoding="utf-8") as handle:
            handle.write(f"write {repo} {count}\\n")
        count += 1
    except Exception:
        pass
    time.sleep(0.0005)
"""
    fake_git_template = """\
#!{executable}
from __future__ import annotations
import os
import pathlib
import subprocess
import sys

args = sys.argv[1:]
real_git = os.environ["BOLT1323_REAL_GIT"]
result = subprocess.run([real_git, *args])
suppression_args = {suppression_args!r}
missing_suppression = not any(
    tuple(args[index : index + len(suppression_args)]) == suppression_args
    for index in range(len(args) - len(suppression_args) + 1)
)
if result.returncode == 0 and "commit" in args and missing_suppression:
    env = os.environ.copy()
    env["BOLT1323_WRITER_REPO"] = str(pathlib.Path.cwd())
    subprocess.Popen(
        [sys.executable, "-c", {writer_code!r}],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
raise SystemExit(result.returncode)
"""
    iterations = 12
    writer_log_text = ""
    with tempfile.TemporaryDirectory() as harness_tmp:
        harness = pathlib.Path(harness_tmp)
        bin_dir = harness / "bin"
        bin_dir.mkdir()
        writer_log = harness / "writers.log"
        fake_git = bin_dir / "git"
        fake_git.write_text(
            textwrap.dedent(
                fake_git_template.format(
                    executable=sys.executable,
                    suppression_args=hygiene_helpers.GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS,
                    writer_code=writer_code,
                )
            ),
            encoding="utf-8",
        )
        fake_git.chmod(fake_git.stat().st_mode | stat.S_IXUSR)
        old_path = os.environ.get("PATH", "")
        old_real_git = os.environ.get("BOLT1323_REAL_GIT")
        old_writer_log = os.environ.get("BOLT1323_WRITER_LOG")
        os.environ["PATH"] = f"{bin_dir}{os.pathsep}{old_path}"
        os.environ["BOLT1323_REAL_GIT"] = real_git
        os.environ["BOLT1323_WRITER_LOG"] = str(writer_log)
        failures = 0
        try:
            for index in range(iterations):
                try:
                    with tempfile.TemporaryDirectory() as tmp:
                        repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
                        write_repo_text(repo, "head.txt", f"head {index}\n")
                        commit_repo(repo, f"head {index}")
                except OSError as exc:
                    if exc.errno != errno.ENOTEMPTY:
                        raise
                    failures += 1
        finally:
            os.environ["PATH"] = old_path
            if old_real_git is None:
                os.environ.pop("BOLT1323_REAL_GIT", None)
            else:
                os.environ["BOLT1323_REAL_GIT"] = old_real_git
            if old_writer_log is None:
                os.environ.pop("BOLT1323_WRITER_LOG", None)
            else:
                os.environ["BOLT1323_WRITER_LOG"] = old_writer_log
        writer_log_text = writer_log.read_text(encoding="utf-8") if writer_log.exists() else ""
    if failures:
        raise AssertionError(f"background writer raced fixture cleanup {failures}/{iterations} times")
    if "write " in writer_log_text:
        raise AssertionError("suppressed git helper still spawned the background writer")

def self_authorizing_errors_for_changes(
    changes: dict[str, str],
) -> list[str]:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        base = run_repo_git(repo, "rev-parse", "HEAD").strip()
        for relative, text in changes.items():
            write_repo_text(repo, relative, text)
        head = commit_repo(repo, "head")
        return verifier.self_authorizing_governance_diff_errors(repo, base, head)

def assert_self_authorizing_governance_detector_contract() -> None:
    positive_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "SSM is primary. JULES_API_KEY is allowed for advisory repo maintenance.\n",
            ".specify/memory/constitution.md": "JULES_API_KEY advisory carve-out is allowed.\n",
            ".pr_agent.toml": 'rule_6 = "JULES_API_KEY advisory carve-out is allowed."\n',
            "ci/ai-review.toml": 'rule_6 = "JULES_API_KEY advisory carve-out is allowed."\n',
            ".github/workflows/weekly-cleanup.yml": """\
name: Jules Weekly Cleanup
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        },
    )
    if not any("self-authorizing governance edit" in error for error in positive_errors):
        raise AssertionError(f"#1060-style coupling must be blocked, got: {positive_errors}")
    if not any("split this into two PRs" in error for error in positive_errors):
        raise AssertionError(f"failure must explain split-PR resolution, got: {positive_errors}")

    bracket_secret_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "SSM is primary. JULES_API_KEY is allowed for advisory repo maintenance.\n",
            ".github/workflows/weekly-cleanup.yml": """\
name: Jules Weekly Cleanup
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets['JULES_API_KEY'] }}
          OTHER_TOKEN: ${{ secrets["OTHER_TOKEN"] }}
        run: echo advisory
""",
        },
    )
    if not any("secret reference secrets.JULES_API_KEY" in error for error in bracket_secret_errors):
        raise AssertionError(f"bracket-form secret reference must be blocked, got: {bracket_secret_errors}")
    if not any("secret reference secrets.OTHER_TOKEN" in error for error in bracket_secret_errors):
        raise AssertionError(f"double-quoted bracket secret reference must be blocked, got: {bracket_secret_errors}")

    expression_secret_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Dynamic secret selectors are allowed for governed automation.\n",
            ".github/workflows/dynamic-secret.yml": """\
name: Dynamic Secret
permissions: {}
jobs:
  jules:
    steps:
      - env:
          TOKEN: ${{ secrets[env.SECRET_NAME] }}
          OTHER_TOKEN: ${{ secrets . OTHER_TOKEN }}
        run: echo advisory
""",
        },
    )
    if not any("secret reference secrets[env.SECRET_NAME]" in error for error in expression_secret_errors):
        raise AssertionError(f"dynamic secret index must be blocked, got: {expression_secret_errors}")
    if not any("secret reference secrets.OTHER_TOKEN" in error for error in expression_secret_errors):
        raise AssertionError(f"whitespace property secret reference must be blocked, got: {expression_secret_errors}")

    inherited_secret_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Reusable workflows may inherit repository secrets after ratification.\n",
            ".github/workflows/reuse.yml": """\
name: Reuse
permissions: {}
jobs:
  call:
    uses: ./.github/workflows/target.yml
    secrets: inherit
""",
        },
    )
    if not any("secret inheritance secrets: inherit" in error for error in inherited_secret_errors):
        raise AssertionError(f"secrets: inherit must be blocked, got: {inherited_secret_errors}")

    quoted_inherited_secret_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Reusable workflows may inherit repository secrets after ratification.\n",
            ".github/workflows/reuse.yml": """\
name: Reuse
permissions: {}
jobs:
  call:
    uses: ./.github/workflows/target.yml
    secrets: "inherit"
""",
        },
    )
    if not any("secret inheritance secrets: inherit" in error for error in quoted_inherited_secret_errors):
        raise AssertionError(f"quoted secrets: inherit must be blocked, got: {quoted_inherited_secret_errors}")

    permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "GitHub OIDC is allowed for a future governed automation lane.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions:\n  contents: read\n  id-token: write\n",
        },
    )
    if not any("permissions grant id-token: write" in error for error in permission_errors):
        raise AssertionError(f"governance plus new permissions grant must be blocked, got: {permission_errors}")

    flow_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "GitHub OIDC is allowed for a future governed automation lane.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions: { id-token: write }\n",
        },
    )
    if not any("permissions grant id-token: write" in error for error in flow_permission_errors):
        raise AssertionError(f"flow-map permissions grant must be blocked, got: {flow_permission_errors}")

    scalar_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Broad workflow token permissions are allowed after ratification.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions: write-all\n",
        },
    )
    if not any("permissions grant permissions: write-all" in error for error in scalar_permission_errors):
        raise AssertionError(f"scalar permissions grant must be blocked, got: {scalar_permission_errors}")

    scalar_read_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Broad workflow token permissions are allowed after ratification.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions: read-all\n",
        },
    )
    if not any("permissions grant permissions: read-all" in error for error in scalar_read_permission_errors):
        raise AssertionError(f"scalar read-all permissions grant must be blocked, got: {scalar_read_permission_errors}")

    flow_none_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Workflow token permissions may be explicitly denied.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions: { id-token: none }\n",
        },
    )
    if flow_none_permission_errors:
        raise AssertionError(f"flow-map permissions denied with none must pass, got: {flow_none_permission_errors}")

    quoted_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "GitHub OIDC is allowed for a future governed automation lane.\n",
            ".github/workflows/ci.yml": 'name: CI\n"permissions":\n  contents: read\n  id-token: write\n',
        },
    )
    if not any("permissions grant id-token: write" in error for error in quoted_permission_errors):
        raise AssertionError(f"quoted permissions key must be parsed, got: {quoted_permission_errors}")

    inherited_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Default workflow token permissions are allowed after ratification.\n",
            ".github/workflows/ci.yml": "name: CI\n",
        },
    )
    if not any("permissions grant inherited default" in error for error in inherited_permission_errors):
        raise AssertionError(f"removed restrictive permissions block must be blocked, got: {inherited_permission_errors}")

    null_permission_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Default workflow token permissions are allowed after ratification.\n",
            ".github/workflows/ci.yml": "name: CI\npermissions:\n",
        },
    )
    if not any("permissions grant inherited default" in error for error in null_permission_errors):
        raise AssertionError(f"null permissions block must be treated as inherited default, got: {null_permission_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        job_permissions_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        write_repo_text(
            job_permissions_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
permissions:
  contents: read
jobs:
  test:
    permissions: {}
    steps:
      - run: echo test
""",
        )
        commit_repo(job_permissions_repo, "base job permissions")
        job_permissions_base = run_repo_git(job_permissions_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            job_permissions_repo,
            "AGENTS.md",
            "Default workflow token permissions are allowed after ratification.\n",
        )
        write_repo_text(
            job_permissions_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
permissions:
  contents: read
jobs:
  test:
    steps:
      - run: echo test
""",
        )
        job_permissions_head = commit_repo(job_permissions_repo, "remove job permissions")
        job_permissions_errors = load_verifier().self_authorizing_governance_diff_errors(
            job_permissions_repo,
            job_permissions_base,
            job_permissions_head,
        )
    if not any("permissions grant inherited default" in error for error in job_permissions_errors):
        raise AssertionError(
            f"removed job-level permissions block must be blocked, got: {job_permissions_errors}"
        )

    with tempfile.TemporaryDirectory() as tmp:
        relocated_permissions_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        write_repo_text(
            relocated_permissions_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
permissions:
  contents: read
jobs:
  first:
    permissions: {}
    steps:
      - run: echo first
  second:
    steps:
      - run: echo second
""",
        )
        commit_repo(relocated_permissions_repo, "base scoped permissions")
        relocated_permissions_base = run_repo_git(relocated_permissions_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            relocated_permissions_repo,
            "AGENTS.md",
            "Default workflow token permissions are allowed after ratification.\n",
        )
        write_repo_text(
            relocated_permissions_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
permissions:
  contents: read
jobs:
  first:
    steps:
      - run: echo first
  second:
    permissions: {}
    steps:
      - run: echo second
""",
        )
        relocated_permissions_head = commit_repo(relocated_permissions_repo, "relocate job permissions")
        relocated_permissions_errors = load_verifier().self_authorizing_governance_diff_errors(
            relocated_permissions_repo,
            relocated_permissions_base,
            relocated_permissions_head,
        )
    if not any("permissions grant inherited default" in error for error in relocated_permissions_errors):
        raise AssertionError(
            "removed job-level permissions block must be detected even when another block is added, "
            f"got: {relocated_permissions_errors}"
        )

    with tempfile.TemporaryDirectory() as tmp:
        grant_swap_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        write_repo_text(
            grant_swap_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
jobs:
  first:
    permissions:
      contents: read
    steps:
      - run: echo first
  second:
    permissions:
      id-token: write
    steps:
      - run: echo second
""",
        )
        commit_repo(grant_swap_repo, "base split grants")
        grant_swap_base = run_repo_git(grant_swap_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            grant_swap_repo,
            "AGENTS.md",
            "GitHub OIDC is allowed for a future governed automation lane.\n",
        )
        write_repo_text(
            grant_swap_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
jobs:
  first:
    permissions:
      contents: read
      id-token: write
    steps:
      - run: echo first
  second:
    permissions:
      contents: read
    steps:
      - run: echo second
""",
        )
        grant_swap_head = commit_repo(grant_swap_repo, "swap scoped grant")
        grant_swap_errors = load_verifier().self_authorizing_governance_diff_errors(
            grant_swap_repo,
            grant_swap_base,
            grant_swap_head,
        )
    if not any("permissions grant jobs.first id-token: write" in error for error in grant_swap_errors):
        raise AssertionError(f"per-job permission broadening must be blocked, got: {grant_swap_errors}")

    allowlist_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Boundary evidence exemptions are allowed after owner ratification.\n",
            "ci/bolt-v3-boundary-exemptions.toml": """\
[[exemptions]]
key = "provider-runtime-metadata"
reason = "owner-ratified"
""",
        },
    )
    if not any("allowlist/exemption entry" in error for error in allowlist_errors):
        raise AssertionError(f"governance plus new allowlist entry must be blocked, got: {allowlist_errors}")

    governance_only_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "SSM is primary. JULES_API_KEY is allowed after owner ratification.\n",
        },
    )
    if governance_only_errors:
        raise AssertionError(f"governance-only edit must pass, got: {governance_only_errors}")

    capability_only_errors = self_authorizing_errors_for_changes(
        {
            ".github/workflows/weekly-cleanup.yml": """\
name: Jules Weekly Cleanup
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        },
    )
    if capability_only_errors:
        raise AssertionError(f"capability-only edit must pass, got: {capability_only_errors}")

    inline_comment_secret_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Governance text clarification without capability changes.\n",
            ".github/workflows/comment-only.yml": """\
name: Comment Only
permissions: {}
jobs:
  test:
    steps:
      - run: echo ok # not using secrets[env.SECRET_NAME]
""",
        },
    )
    if inline_comment_secret_errors:
        raise AssertionError(
            f"inline comments mentioning secrets syntax must not be blocked, got: {inline_comment_secret_errors}"
        )

    secret_before_comment_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "Governed automation may use a dynamic repository secret.\n",
            ".github/workflows/secret-before-comment.yml": """\
name: Secret Before Comment
permissions: {}
jobs:
  test:
    steps:
      - env:
          TOKEN: ${{ secrets[env.SECRET_NAME] }} # real secret before a comment
        run: echo ok
""",
        },
    )
    if not any("secret reference secrets[env.SECRET_NAME]" in error for error in secret_before_comment_errors):
        raise AssertionError(f"real secret before trailing comment must still block, got: {secret_before_comment_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        split_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        write_repo_text(
            split_repo,
            "AGENTS.md",
            "SSM is primary. JULES_API_KEY is allowed after owner ratification.\n",
        )
        base_after_governance = commit_repo(split_repo, "ratified governance")
        write_repo_text(
            split_repo,
            ".github/workflows/weekly-cleanup.yml",
            """\
name: Jules Weekly Cleanup
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        )
        capability_head = commit_repo(split_repo, "capability")
        split_errors = load_verifier().self_authorizing_governance_diff_errors(
            split_repo,
            base_after_governance,
            capability_head,
        )
    if split_errors:
        raise AssertionError(f"split governance/capability PRs must pass, got: {split_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        prefixed_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        run_repo_git(prefixed_repo, "config", "diff.noprefix", "true")
        run_repo_git(prefixed_repo, "config", "diff.mnemonicprefix", "true")
        prefixed_base = run_repo_git(prefixed_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            prefixed_repo,
            "AGENTS.md",
            "SSM is primary. JULES_API_KEY is allowed for advisory repo maintenance.\n",
        )
        write_repo_text(
            prefixed_repo,
            ".github/workflows/weekly-cleanup.yml",
            """\
name: Jules Weekly Cleanup
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        )
        prefixed_head = commit_repo(prefixed_repo, "head")
        prefixed_errors = load_verifier().self_authorizing_governance_diff_errors(
            prefixed_repo,
            prefixed_base,
            prefixed_head,
        )
    if not any("secret reference secrets.JULES_API_KEY" in error for error in prefixed_errors):
        raise AssertionError(f"diff prefix config must not hide added secret lines, got: {prefixed_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        attributes_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        attributes_base = run_repo_git(attributes_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            attributes_repo,
            ".gitattributes",
            "*.yml -diff\n",
        )
        write_repo_text(
            attributes_repo,
            "AGENTS.md",
            "SSM is primary. NEW_SECRET is allowed for advisory repo maintenance.\n",
        )
        write_repo_text(
            attributes_repo,
            ".github/workflows/ci.yml",
            """\
name: CI
permissions:
  contents: read
jobs:
  jules:
    steps:
      - env:
          NEW_SECRET: ${{ secrets.NEW_SECRET }}
        run: echo advisory
""",
        )
        attributes_head = commit_repo(attributes_repo, "head")
        attributes_errors = load_verifier().self_authorizing_governance_diff_errors(
            attributes_repo,
            attributes_base,
            attributes_head,
        )
    if not any("secret reference secrets.NEW_SECRET" in error for error in attributes_errors):
        raise AssertionError(f".gitattributes diff suppression must not hide added secret lines, got: {attributes_errors}")

    unicode_workflow_errors = self_authorizing_errors_for_changes(
        {
            "AGENTS.md": "SSM is primary. NEW_SECRET is allowed for advisory repo maintenance.\n",
            ".github/workflows/검증.yml": """\
name: Unicode Path
permissions: {}
jobs:
  jules:
    steps:
      - env:
          NEW_SECRET: ${{ secrets.NEW_SECRET }}
        run: echo advisory
""",
        },
    )
    if not any("secret reference secrets.NEW_SECRET" in error for error in unicode_workflow_errors):
        raise AssertionError(f"quoted git paths must not hide workflow secret lines, got: {unicode_workflow_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        moved_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        write_repo_text(
            moved_repo,
            "scratch/inactive.yml",
            """\
name: Later Active
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        )
        commit_repo(moved_repo, "inactive")
        moved_base = run_repo_git(moved_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            moved_repo,
            "AGENTS.md",
            "SSM is primary. JULES_API_KEY is allowed for advisory repo maintenance.\n",
        )
        run_repo_git(moved_repo, "mv", "scratch/inactive.yml", ".github/workflows/moved.yml")
        moved_head = commit_repo(moved_repo, "move active")
        moved_errors = load_verifier().self_authorizing_governance_diff_errors(
            moved_repo,
            moved_base,
            moved_head,
        )
    if not any("secret reference secrets.JULES_API_KEY" in error for error in moved_errors):
        raise AssertionError(f"moving secret-using workflow into active path must be blocked, got: {moved_errors}")

    with tempfile.TemporaryDirectory() as tmp:
        large_data_repo = init_self_authorizing_fixture_repo(pathlib.Path(tmp))
        large_data_path = "data/source-universe-execution-pack.json"
        large_data_line_count = 100_001
        large_data_final_id = large_data_line_count + 1
        large_data_body = "".join(
            f'  {{"id": {index}, "token": "stable-{index}"}},\n'
            for index in range(large_data_line_count)
        )
        data_secret_sentinel = "DATA_JSON_TOKEN"
        write_repo_text(
            large_data_repo,
            large_data_path,
            "[\n"
            + large_data_body
            + f'  {{"id": {large_data_final_id}, "token": "base-final"}}\n'
            + "]\n",
        )
        commit_repo(large_data_repo, "base large data")
        large_data_base = run_repo_git(large_data_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            large_data_repo,
            "AGENTS.md",
            "SSM is primary. JULES_API_KEY is allowed for advisory repo maintenance.\n",
        )
        write_repo_text(
            large_data_repo,
            large_data_path,
            "[\n"
            + large_data_body
            + f'  {{"id": {large_data_final_id}, "token": "${{{{ secrets.{data_secret_sentinel} }}}}"}}\n'
            + "]\n",
        )
        write_repo_text(
            large_data_repo,
            ".github/workflows/large-data-signal.yml",
            """\
name: Large Data Signal
permissions: {}
jobs:
  jules:
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo advisory
""",
        )
        large_data_head = commit_repo(large_data_repo, "head large data")
        verifier = load_verifier()
        real_sequence_matcher = verifier.difflib.SequenceMatcher

        class RejectLargeDataSequenceMatcher:
            def __init__(self, isjunk, a, b, autojunk=True):
                if any(data_secret_sentinel in line for line in b):
                    raise AssertionError("large data file must not be diffed for self-authorizing signals")
                self._matcher = real_sequence_matcher(
                    isjunk,
                    a,
                    b,
                    autojunk=autojunk,
                )

            def get_opcodes(self):
                return self._matcher.get_opcodes()

        verifier.difflib.SequenceMatcher = RejectLargeDataSequenceMatcher
        try:
            started = time.perf_counter()
            large_data_errors = verifier.self_authorizing_governance_diff_errors(
                large_data_repo,
                large_data_base,
                large_data_head,
            )
            elapsed = time.perf_counter() - started
        finally:
            verifier.difflib.SequenceMatcher = real_sequence_matcher
    if elapsed >= 5.0:
        raise AssertionError(f"large irrelevant data diff must complete in <5s, took {elapsed:.3f}s")
    if not any("secret reference secrets.JULES_API_KEY" in error for error in large_data_errors):
        raise AssertionError(f"workflow secret signal must still be blocked, got: {large_data_errors}")
    if any(large_data_path in error or data_secret_sentinel in error for error in large_data_errors):
        raise AssertionError(f"large data changes must be ignored, got: {large_data_errors}")

    required_self_authorizing_archive = (
        'git archive "$base_ref"',
        ".github/",
        ".config/",
        "ci/",
        "crates/backtesting-vertical-slice/ci/",
        "scripts/",
        "tests/",
        "AGENTS.md",
        "Cargo.toml",
        "justfile",
        ".mergify.yml",
        ".no-mistakes.yaml",
        ".pr_agent.toml",
        '| tar -x -C "$base_tree"',
    )
    self_authorizing_archive_start = BASE_WORKFLOW.find('git archive "$base_ref"', BASE_WORKFLOW.find("self-authorizing-governance-base-tree"))
    self_authorizing_archive_end = BASE_WORKFLOW.find(
        'python3 "$base_tree/scripts/verify_ci_workflow_hygiene.py"',
        self_authorizing_archive_start,
    )
    self_authorizing_archive = BASE_WORKFLOW[self_authorizing_archive_start:self_authorizing_archive_end]
    missing_archive_inputs = [
        value for value in required_self_authorizing_archive if value not in self_authorizing_archive
    ]
    if missing_archive_inputs:
        raise AssertionError(
            "self-authorizing base-tree bootstrap must archive trusted verifier inputs, "
            f"missing {missing_archive_inputs}"
        )

    with tempfile.TemporaryDirectory() as tmp:
        cli_fixture = pathlib.Path(tmp) / "fixture"
        cli_fixture.mkdir()
        cli_repo = init_self_authorizing_fixture_repo(cli_fixture)
        cli_base = run_repo_git(cli_repo, "rev-parse", "HEAD").strip()
        write_repo_text(
            cli_repo,
            "AGENTS.md",
            "SSM is primary. JULES_API_KEY is allowed after owner ratification.\n",
        )
        cli_head = commit_repo(cli_repo, "governance only")
        base_tree = copy_self_authorizing_base_tree(pathlib.Path(tmp))
        completed = subprocess.run(
            [
                sys.executable,
                "scripts/verify_ci_workflow_hygiene.py",
                "self-authorizing-governance",
                "--repo",
                str(cli_repo),
                "--base",
                cli_base,
                "--head",
                cli_head,
            ],
            cwd=base_tree,
            env={**os.environ, "GITHUB_ACTIONS": "true"},
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    if completed.returncode != 0:
        raise AssertionError(
            "base-tree self-authorizing CLI must run for governance-only PRs, "
            f"got {completed.returncode}: stdout={completed.stdout!r} stderr={completed.stderr!r}"
        )

    assert_error(
        "detector must inspect self-authorizing governance rule-files",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Block self-authorizing governance edits",
            "AGENTS.md",
            "README.md",
        ),
    )
    assert_error(
        "detector self-authorizing governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Block self-authorizing governance edits",
            "        shell: bash\n",
            """        shell: bash
        continue-on-error: true
""",
        ),
    )
    assert_error(
        "detector must hard-block self-authorizing governance edits",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Block self-authorizing governance edits",
            'python3 "$base_tree/scripts/verify_ci_workflow_hygiene.py" self-authorizing-governance',
            'echo "::warning::self-authorizing governance edit detected"',
        ),
    )

def assert_debug_test_workflow_contract() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(DEBUG_TEST_WORKFLOW_PATH)
    workflows = {
        DEBUG_TEST_WORKFLOW_PATH: workflow,
        ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml"),
    }
    expected_scoped_grants = {
        ("permissions", "contents", "read"),
        ("jobs.debug-test", "contents", "read"),
        ("jobs.debug-test", "id-token", "write"),
    }
    scoped_grants = verifier.yaml_permissions_scoped_grants(workflow)
    if scoped_grants != expected_scoped_grants:
        raise AssertionError(f"debug-test permissions grants drifted: {scoped_grants!r}")

    errors = verifier.verify_debug_test_workflow(workflows, repo_source_text("justfile"))
    if errors:
        raise AssertionError(f"debug-test workflow must satisfy its contract, got: {errors}")

    mutations = (
        (
            "debug-test workflow must be workflow_dispatch-only",
            workflow.replace("on:\n  workflow_dispatch:\n", "on:\n  push:\n    branches: [main]\n  workflow_dispatch:\n", 1),
        ),
        (
            "debug-test workflow permissions must match scoped allowlist",
            workflow.replace("permissions:\n  contents: read\n", "permissions:\n  contents: read\n  actions: read\n", 1),
        ),
        (
            "debug-test workflow permissions must match scoped allowlist",
            workflow.replace("    permissions:\n      contents: read\n      id-token: write\n", "    permissions:\n      contents: read\n      id-token: write\n      statuses: write\n", 1),
        ),
        (
            "debug-test workflow permissions must match scoped allowlist",
            workflow.replace("    permissions:\n      contents: read\n      id-token: write\n", "    permissions: write-all\n", 1),
        ),
        (
            "debug-test workflow permissions must match scoped allowlist",
            workflow.replace("      id-token: write\n", "", 1),
        ),
        (
            "debug-test workflow must not declare concurrency",
            workflow.replace("permissions:\n  contents: read\n", "concurrency:\n  group: debug-test\n\npermissions:\n  contents: read\n", 1),
        ),
        (
            "debug-test workflow must run on vars.CI_RUNNER_MANAGED_HEAVY",
            workflow.replace("runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}", "runs-on: ubuntu-latest", 1),
        ),
        (
            "debug-test workflow timeout must be 30 minutes",
            workflow.replace("timeout-minutes: 30", "timeout-minutes: 60", 1),
        ),
        (
            "debug-test workflow must call managed just debug-test recipe",
            workflow.replace("just debug-test", "true"),
        ),
        (
            "debug-test workflow must use the PR-readonly cache role only",
            workflow.replace("AWS_CI_CACHE_PR_READONLY_ROLE_ARN", "AWS_CI_CACHE_ROLE_ARN", 1),
        ),
        (
            "Resolve debug archive cache eligibility' must bind PR_READONLY_ROLE_ARN to the PR-readonly role var",
            replace_once(
                workflow,
                "          PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
                "          PR_READONLY_ROLE_ARN: ${{ vars.CI_SCCACHE_BUCKET }}\n",
            ),
        ),
        (
            "Resolve debug archive cache eligibility' must output PR_READONLY_ROLE_ARN as role_arn",
            replace_once(
                workflow,
                '          echo "role_arn=$PR_READONLY_ROLE_ARN" >> "$GITHUB_OUTPUT"\n',
                '          echo "role_arn=arn:aws:iam::123456789012:role/debug-archive-hijack" >> "$GITHUB_OUTPUT"\n',
            ),
        ),
        (
            "Configure AWS credentials for debug archive cache' must assume the resolved debug archive role",
            replace_once(
                workflow,
                "          role-to-assume: ${{ steps.debug-archive-cache.outputs.role_arn }}\n",
                "          role-to-assume: arn:aws:iam::123456789012:role/debug-archive-hijack\n",
            ),
        ),
        (
            "debug-test workflow must route sccache through the shared read-only setup action",
            replace_once(
                workflow,
                "uses: ./.github/actions/sccache-setup",
                "uses: ./.github/actions/not-sccache-setup",
            ),
        ),
        (
            "debug-test workflow must route sccache through the shared read-only setup action",
            replace_once(
                workflow,
                "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
                "",
            ),
        ),
        (
            "debug-test workflow must not reference provenance or gate jobs",
            workflow.replace("name: debug-test", "name: debug-test\ngate: ignored", 1),
        ),
    )
    for fragment, mutated in mutations:
        mutated_errors = verifier.verify_debug_test_workflow(
            {DEBUG_TEST_WORKFLOW_PATH: mutated, ".github/workflows/ci.yml": workflows[".github/workflows/ci.yml"]},
            repo_source_text("justfile"),
        )
        if not any(fragment in error for error in mutated_errors):
            raise AssertionError(f"expected {fragment!r}, got: {mutated_errors}")

    mergify_text = repo_source_text(".mergify.yml") + "\n# debug-test\n"
    mergify_errors = verifier.verify_debug_test_workflow(workflows, repo_source_text("justfile"), mergify_text)
    if not any("debug-test workflow must not be referenced by .mergify.yml" in error for error in mergify_errors):
        raise AssertionError(f"debug-test mergify reference must be rejected, got: {mergify_errors}")

    justfile_without_recipe = repo_source_text("justfile").replace(
        'debug-test filter package="" *extra_args: check-workspace require-rust-verification-owner\n',
        "",
        1,
    )
    recipe_errors = verifier.verify_debug_test_workflow(workflows, justfile_without_recipe)
    if not any("justfile must define debug-test filter package" in error for error in recipe_errors):
        raise AssertionError(f"missing debug-test just recipe must be rejected, got: {recipe_errors}")

    unsafe_justfile = repo_source_text("justfile").replace(
        'if [[ -z "$filter" ]]; then filter={{quote(filter)}}; fi\n',
        'filter="${DEBUG_TEST_FILTER:-{{filter}}}"\n',
        1,
    )
    quote_errors = verifier.verify_debug_test_workflow(workflows, unsafe_justfile)
    if not any("justfile debug-test recipe must shell-quote direct filter/package arguments" in error for error in quote_errors):
        raise AssertionError(f"unsafe direct debug-test filter interpolation must be rejected, got: {quote_errors}")

    unguarded_justfile = repo_source_text("justfile").replace(
        '    if [[ -z "$filter" ]]; then echo "ERROR: debug-test filter must be non-empty" >&2; exit 2; fi\n',
        "",
        1,
    )
    guard_errors = verifier.verify_debug_test_workflow(workflows, unguarded_justfile)
    if not any("justfile debug-test recipe must fail closed on an empty filter" in error for error in guard_errors):
        raise AssertionError(f"empty debug-test filter guard must be rejected, got: {guard_errors}")


def main() -> int:
    assert_run_repo_git_suppresses_background_maintenance()
    assert_suppression_args_match_suppression_config()
    assert_init_fixture_repo_persists_suppression()
    assert_self_authorizing_fixture_repo_persists_suppression()
    assert_clone_fixture_repo_persists_suppression()
    assert_push_to_fixture_remote_spawns_no_background_maintenance()
    assert_routed_fixture_module_spawns_no_background_maintenance()
    assert_temp_git_fixture_cleanup_stress_blocks_background_writer()
    assert_self_authorizing_governance_detector_contract()
    assert_debug_test_workflow_contract()
    print("OK: governance diff analysis tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
