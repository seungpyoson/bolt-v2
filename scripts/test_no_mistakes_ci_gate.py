#!/usr/bin/env python3
"""Self-tests for the no-mistakes exact-head CI gate."""

from __future__ import annotations

import importlib.util
import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "scripts" / "no_mistakes_ci_gate.py"


def load_module():
    spec = importlib.util.spec_from_file_location("no_mistakes_ci_gate", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load no_mistakes_ci_gate.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _init_repo(tmp_path: pathlib.Path) -> pathlib.Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=repo, check=True)
    subprocess.run(["git", "config", "user.name", "Test User"], cwd=repo, check=True)
    (repo / "README.md").write_text("test\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=repo, check=True)
    subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "init"], cwd=repo, check=True)
    return repo


def _head(repo: pathlib.Path) -> str:
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


def _fake_completed_runs(head: str) -> list[dict[str, object]]:
    return [
        {
            "databaseId": 1,
            "name": "CI",
            "status": "completed",
            "conclusion": "success",
            "headSha": head,
            "url": "https://example.invalid/ci",
        },
        {
            "databaseId": 2,
            "name": "CI docs pass stub",
            "status": "completed",
            "conclusion": "success",
            "headSha": head,
            "url": "https://example.invalid/docs",
        },
    ]


def test_evaluate_ci_gate_passes_when_exact_head_ci_is_green():
    module = load_module()
    tmpdir = tempfile.TemporaryDirectory()
    repo = _init_repo(pathlib.Path(tmpdir.name))
    head = _head(repo)

    def fake_json_output(argv, cwd, description):
        if argv[:3] == ["gh", "pr", "view"]:
            return {"number": 1, "headRefOid": head, "url": "https://example.invalid/pr/1"}
        if argv[:3] == ["gh", "run", "list"]:
            return _fake_completed_runs(head)
        raise AssertionError(f"unexpected command: {argv}")

    original = module._json_output
    module._json_output = fake_json_output
    try:
        ok, messages = module.evaluate_ci_gate(repo)
        assert ok is True
        assert any("CI completed successfully" in message for message in messages)
        assert any("CI docs pass stub completed successfully" in message for message in messages)
    finally:
        module._json_output = original
        tmpdir.cleanup()


def test_evaluate_ci_gate_rejects_stale_pr_head():
    module = load_module()
    tmpdir = tempfile.TemporaryDirectory()
    repo = _init_repo(pathlib.Path(tmpdir.name))

    def fake_json_output(argv, cwd, description):
        if argv[:3] == ["gh", "pr", "view"]:
            return {"number": 1, "headRefOid": "deadbeef", "url": "https://example.invalid/pr/1"}
        raise AssertionError(f"unexpected command: {argv}")

    original = module._json_output
    module._json_output = fake_json_output
    try:
        ok, messages = module.evaluate_ci_gate(repo)
        assert ok is False
        assert any("push current HEAD" in message for message in messages)
    finally:
        module._json_output = original
        tmpdir.cleanup()


def test_evaluate_ci_gate_rejects_pending_ci():
    module = load_module()
    tmpdir = tempfile.TemporaryDirectory()
    repo = _init_repo(pathlib.Path(tmpdir.name))
    head = _head(repo)

    def fake_json_output(argv, cwd, description):
        if argv[:3] == ["gh", "pr", "view"]:
            return {"number": 1, "headRefOid": head, "url": "https://example.invalid/pr/1"}
        if argv[:3] == ["gh", "run", "list"]:
            runs = _fake_completed_runs(head)
            runs[0] = dict(runs[0], status="in_progress", conclusion="")
            return runs
        raise AssertionError(f"unexpected command: {argv}")

    original = module._json_output
    module._json_output = fake_json_output
    try:
        ok, messages = module.evaluate_ci_gate(repo)
        assert ok is False
        assert any("status='in_progress'" in message for message in messages)
    finally:
        module._json_output = original
        tmpdir.cleanup()


def test_main_reports_prerequisite_errors_without_running_cargo():
    module = load_module()
    tmpdir = tempfile.TemporaryDirectory()
    repo = _init_repo(pathlib.Path(tmpdir.name))

    def fail_gate(cwd):
        raise RuntimeError("current branch has no PR")

    original = module.evaluate_ci_gate
    module.evaluate_ci_gate = fail_gate
    stderr = io.StringIO()
    try:
        with contextlib.redirect_stderr(stderr):
            exit_code = module.main(["test", "--repo", str(repo)])
        assert exit_code == 1
        assert "current branch has no PR" in stderr.getvalue()
        assert "managed local check" in stderr.getvalue()
    finally:
        module.evaluate_ci_gate = original
        tmpdir.cleanup()


def main() -> int:
    test_evaluate_ci_gate_passes_when_exact_head_ci_is_green()
    test_evaluate_ci_gate_rejects_stale_pr_head()
    test_evaluate_ci_gate_rejects_pending_ci()
    test_main_reports_prerequisite_errors_without_running_cargo()
    print("OK: no-mistakes CI gate self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
