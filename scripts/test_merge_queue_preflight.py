#!/usr/bin/env python3
"""Tests for merge_queue_preflight.py."""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_queue_preflight.py"


def run(command: list[str], cwd: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def git(cwd: pathlib.Path, *args: str) -> str:
    return run(["git", *args], cwd).stdout.strip()


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def commit(repo: pathlib.Path, message: str) -> str:
    git(repo, "add", ".")
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD")


class GitFixture:
    def __init__(self, root: pathlib.Path) -> None:
        self.root = root
        self.remote = root / "origin.git"
        self.repo = root / "repo"
        git(root, "init", "--bare", str(self.remote))
        git(root, "clone", str(self.remote), str(self.repo))
        git(self.repo, "config", "user.email", "preflight@example.invalid")
        git(self.repo, "config", "user.name", "Merge Queue Preflight Test")
        write(self.repo / "shared.txt", "base\n")
        self.base = commit(self.repo, "base")
        git(self.repo, "branch", "-M", "main")
        git(self.repo, "push", "origin", "main")

    def make_pr(self, number: int, edits: dict[str, str]) -> str:
        branch = f"pr-{number}"
        git(self.repo, "checkout", "-B", branch, "main")
        for path, text in edits.items():
            write(self.repo / path, text)
        head = commit(self.repo, f"pr {number}")
        git(self.repo, "push", "origin", f"HEAD:refs/pull/{number}/head")
        git(self.repo, "checkout", "main")
        return head


def run_preflight(
    repo: pathlib.Path,
    origin: pathlib.Path,
    *args: str,
    expect_success: bool = True,
) -> tuple[int, str, str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
        "--no-gh",
        "--verifier-profile",
        "none",
        "--json",
        *args,
    ]
    result = subprocess.run(
        command,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if expect_success and result.returncode != 0:
        raise AssertionError(
            f"preflight failed unexpectedly: rc={result.returncode}\nSTDOUT:\n{result.stdout}\nSTDERR:\n{result.stderr}"
        )
    if not expect_success and result.returncode == 0:
        raise AssertionError(f"preflight unexpectedly passed: {result.stdout}")
    return result.returncode, result.stdout, result.stderr


def parse_json(stdout: str) -> dict[str, object]:
    payload = json.loads(stdout)
    if not isinstance(payload, dict):
        raise AssertionError(payload)
    return payload


def write_preflight_config(
    root: pathlib.Path,
    profile: str,
    commands: list[str],
    *,
    verifier_stream_max_lines: int = 40,
    verifier_stream_max_bytes: int = 4000,
) -> pathlib.Path:
    rendered_commands = ", ".join(json.dumps(command) for command in commands)
    path = root / "preflight.toml"
    write(
        path,
        "[merge_queue_preflight]\n"
        'origin = "origin"\n'
        'base = "main"\n'
        f"default_verifier_profile = {json.dumps(profile)}\n\n"
        "[merge_queue_preflight.output]\n"
        f"verifier_stream_max_lines = {verifier_stream_max_lines}\n"
        f"verifier_stream_max_bytes = {verifier_stream_max_bytes}\n\n"
        f"[merge_queue_preflight.verifier_profiles.{profile}]\n"
        f"commands = [{rendered_commands}]\n",
    )
    return path


def write_fake_gh(
    root: pathlib.Path,
    *,
    views: dict[int, dict[str, object]],
    checks: dict[int, list[dict[str, object]]] | None = None,
    failed_views: dict[int, str] | None = None,
) -> pathlib.Path:
    bin_dir = root / "bin"
    bin_dir.mkdir()
    path = bin_dir / "gh"
    write(
        path,
        "#!/usr/bin/env python3\n"
        "import json\n"
        "import sys\n"
        f"views = {views!r}\n"
        f"checks = {(checks or {})!r}\n"
        f"failed_views = {(failed_views or {})!r}\n"
        "args = sys.argv[1:]\n"
        "if len(args) >= 3 and args[0:2] == ['pr', 'view']:\n"
        "    if int(args[2]) in failed_views:\n"
        "        print(failed_views[int(args[2])], file=sys.stderr)\n"
        "        raise SystemExit(1)\n"
        "    print(json.dumps(views[int(args[2])]))\n"
        "elif len(args) >= 3 and args[0:2] == ['pr', 'checks']:\n"
        "    print(json.dumps(checks.get(int(args[2]), [])))\n"
        "else:\n"
        "    raise SystemExit(f'unexpected gh args: {args}')\n",
    )
    path.chmod(0o755)
    return bin_dir


def approved_pr_view(head: str, *, base: str = "main") -> dict[str, object]:
    return {
        "number": 1,
        "state": "OPEN",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED",
        "headRefOid": head,
        "baseRefName": base,
        "title": "one",
        "url": "https://example.invalid/pull/1",
    }


def run_preflight_with_gh(
    repo: pathlib.Path,
    origin: pathlib.Path,
    bin_dir: pathlib.Path,
    *prs: str,
) -> subprocess.CompletedProcess[str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
        "--verifier-profile",
        "none",
        "--json",
        *prs,
    ]
    env = os.environ.copy()
    env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
    return subprocess.run(
        command,
        cwd=repo,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def assert_clean_prs_batch_together() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"two.txt": "two\n"})

        _, stdout, _ = run_preflight(fixture.repo, fixture.remote, "1", "2")
        payload = parse_json(stdout)

        batches = payload["batches"]
        if batches != [{"index": 1, "prs": [1, 2], "status": "ready", "verifiers": []}]:
            raise AssertionError(batches)
        if payload["blocked_prs"] != []:
            raise AssertionError(payload["blocked_prs"])
        if payload["conflicts"] != []:
            raise AssertionError(payload["conflicts"])


def assert_conflicting_pr_starts_later_batch() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"shared.txt": "first\n"})
        fixture.make_pr(2, {"shared.txt": "second\n"})

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            expect_success=False,
        )
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        batches = payload["batches"]
        if [batch["prs"] for batch in batches] != [[1], [2]]:
            raise AssertionError(batches)
        conflicts = payload["conflicts"]
        expected = {
            "pr": 2,
            "against_batch": [1],
            "files": ["shared.txt"],
            "type": "batch_conflict",
        }
        if conflicts != [expected]:
            raise AssertionError(conflicts)


def assert_order_dependent_conflict_context_is_reported() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"shared.txt": "second\n"})
        fixture.make_pr(3, {"shared.txt": "third\n"})

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            "3",
            expect_success=False,
        )
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        batches = payload["batches"]
        if [batch["prs"] for batch in batches] != [[1, 2], [3]]:
            raise AssertionError(batches)
        conflicts = payload["conflicts"]
        if conflicts != [
            {
                "pr": 3,
                "against_batch": [1, 2],
                "files": ["shared.txt"],
                "type": "batch_conflict",
            }
        ]:
            raise AssertionError(conflicts)


def assert_pr_that_conflicts_with_base_is_blocked() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"shared.txt": "stale branch change\n"})
        git(fixture.repo, "checkout", "main")
        write(fixture.repo / "shared.txt", "new main\n")
        commit(fixture.repo, "advance main")
        git(fixture.repo, "push", "origin", "main")

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            expect_success=False,
        )
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload["batches"])
        blocked = payload["blocked_prs"]
        if blocked != [
            {
                "pr": 2,
                "reason": "conflicts with base",
                "files": ["shared.txt"],
                "type": "base_conflict",
            }
        ]:
            raise AssertionError(blocked)


def assert_verifier_failure_blocks_bad_pr_before_batching() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"safe.txt": "safe\n"})
        fixture.make_pr(2, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('fail.txt is not allowed')\n"
            "    sys.exit(7)\n",
        )

        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(
                f"expected verifier failure exit 1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload["batches"])
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "fail.txt is not allowed" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)


def assert_configured_verifier_profile_blocks_bad_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"safe.txt": "safe\n"})
        fixture.make_pr(2, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('configured verifier rejected fail.txt')\n"
            "    sys.exit(7)\n",
        )
        config = write_preflight_config(root, "strict", [f"{sys.executable} {verifier}"])

        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--config",
            str(config),
            "--json",
            "1",
            "2",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(
                f"expected configured verifier failure, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "configured verifier rejected fail.txt" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)


def assert_plain_output_includes_verifier_failure_details() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('plain verifier rejected fail.txt')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        if f"verifier {sys.executable} {verifier}: exit 7" not in result.stdout:
            raise AssertionError(result.stdout)
        if "plain verifier rejected fail.txt" not in result.stdout:
            raise AssertionError(result.stdout)


def assert_plain_output_omits_successful_verifier_streams() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"safe.txt": "safe\n"})
        verifier = root / "successful_verifier.py"
        write(
            verifier,
            "print('success output should stay quiet')\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(f"expected verifier rc=0, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        if f"verifier {sys.executable} {verifier}: exit 0" not in result.stdout:
            raise AssertionError(result.stdout)
        if "success output should stay quiet" in result.stdout:
            raise AssertionError(result.stdout)


def assert_plain_output_bounds_failed_verifier_streams() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "noisy_failure.py"
        write(
            verifier,
            "import sys\n"
            "for line in ['line-1', 'line-2', 'line-3']:\n"
            "    print(line)\n"
            "sys.exit(7)\n",
        )
        config = write_preflight_config(
            root,
            "strict",
            [f"{sys.executable} {verifier}"],
            verifier_stream_max_lines=2,
            verifier_stream_max_bytes=200,
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--config",
            str(config),
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        if "line-1" not in result.stdout or "line-2" not in result.stdout:
            raise AssertionError(result.stdout)
        if "line-3" in result.stdout:
            raise AssertionError(result.stdout)
        if "truncated" not in result.stdout:
            raise AssertionError(result.stdout)


def assert_json_output_uses_bounded_verifier_previews() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "noisy_json_failure.py"
        write(
            verifier,
            "import sys\n"
            "for line in ['json-line-1', 'json-line-2', 'json-line-3']:\n"
            "    print(line)\n"
            "sys.exit(7)\n",
        )
        config = write_preflight_config(
            root,
            "strict",
            [f"{sys.executable} {verifier}"],
            verifier_stream_max_lines=2,
            verifier_stream_max_bytes=200,
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--config",
            str(config),
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1:
            raise AssertionError(blocked)
        verifier_result = blocked[0]
        if "stdout" in verifier_result or "stderr" in verifier_result:
            raise AssertionError(verifier_result)
        if verifier_result.get("stdout_preview") != "json-line-1\njson-line-2":
            raise AssertionError(verifier_result)
        if verifier_result.get("stdout_truncated") is not True:
            raise AssertionError(verifier_result)
        if "json-line-3" in json.dumps(verifier_result):
            raise AssertionError(verifier_result)


def assert_head_oid_mismatch_blocks_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view("0" * 40)},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        if result.returncode != 1:
            raise AssertionError(
                f"expected head mismatch rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "readiness_failed":
            raise AssertionError(blocked)
        if "does not match fetched PR head" not in blocked[0]["reason"]:
            raise AssertionError(blocked)


def assert_wrong_base_ref_blocks_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head, base="release")},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        if result.returncode != 1:
            raise AssertionError(f"expected wrong base rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "readiness_failed":
            raise AssertionError(blocked)
        if "PR targets base 'release', expected 'main'" not in blocked[0]["reason"]:
            raise AssertionError(blocked)


def assert_partial_gh_metadata_failure_preserves_other_readiness() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"two.txt": "two\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            failed_views={2: "simulated metadata failure"},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1", "2")
        if result.returncode != 1:
            raise AssertionError(
                f"expected partial metadata failure rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        readiness = {item["pr"]: item for item in payload["readiness"]}
        if "metadata" not in readiness[1]:
            raise AssertionError(payload)
        if readiness[2].get("metadata_unavailable") is not True:
            raise AssertionError(payload)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "metadata_unavailable":
            raise AssertionError(payload)
        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload)
        warnings = payload.get("metadata_warnings")
        if not isinstance(warnings, list) or "PR #2" not in warnings[0]:
            raise AssertionError(payload)


def assert_invalid_pr_input_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        rc, stdout, stderr = run_preflight(
            fixture.repo,
            fixture.remote,
            "abc",
            expect_success=False,
        )
        if rc != 2:
            raise AssertionError((rc, stdout, stderr))
        if "PR numbers must be positive integers" not in stderr:
            raise AssertionError(stderr)


def assert_missing_gh_reports_inconclusive_metadata() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = root / "bin"
        bin_dir.mkdir()
        git_bin = shutil.which("git")
        if git_bin is None:
            raise AssertionError("git executable not found")
        (bin_dir / "git").symlink_to(git_bin)
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        env = os.environ.copy()
        env["PATH"] = str(bin_dir)
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 1:
            raise AssertionError(
                f"expected inconclusive metadata rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        warnings = payload.get("metadata_warnings")
        if not isinstance(warnings, list) or not warnings:
            raise AssertionError(payload)
        if "gh executable not found" not in warnings[0]:
            raise AssertionError(warnings)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "metadata_unavailable":
            raise AssertionError(payload)
        if payload["batches"] != []:
            raise AssertionError(payload["batches"])


def main() -> int:
    assert_clean_prs_batch_together()
    assert_conflicting_pr_starts_later_batch()
    assert_order_dependent_conflict_context_is_reported()
    assert_pr_that_conflicts_with_base_is_blocked()
    assert_verifier_failure_blocks_bad_pr_before_batching()
    assert_configured_verifier_profile_blocks_bad_pr()
    assert_plain_output_includes_verifier_failure_details()
    assert_plain_output_omits_successful_verifier_streams()
    assert_plain_output_bounds_failed_verifier_streams()
    assert_json_output_uses_bounded_verifier_previews()
    assert_head_oid_mismatch_blocks_pr()
    assert_wrong_base_ref_blocks_pr()
    assert_partial_gh_metadata_failure_preserves_other_readiness()
    assert_invalid_pr_input_is_rejected()
    assert_missing_gh_reports_inconclusive_metadata()
    print("OK: merge_queue_preflight tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
