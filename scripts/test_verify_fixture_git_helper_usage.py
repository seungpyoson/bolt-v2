#!/usr/bin/env python3
"""Tests for verify_fixture_git_helper_usage.py."""

from __future__ import annotations

import io
import contextlib
import pathlib
import sys
import tempfile
import unittest

import verify_fixture_git_helper_usage as verifier

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def run_against(source: str) -> tuple[int, str]:
    """Run the verifier over a throwaway repo holding one `scripts/test_x.py`."""
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "scripts").mkdir()
        (root / "scripts" / "test_planted.py").write_text(source, encoding="utf-8")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
            rc = verifier.main(["--repo-root", str(root)])
        return rc, stderr.getvalue()


class PlantedViolationTests(unittest.TestCase):
    def test_direct_subprocess_git_argv_fails(self) -> None:
        rc, err = run_against(
            "import subprocess\n"
            "subprocess.run(['git', 'commit', '-m', 'x'], cwd='/tmp', check=True)\n"
        )
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:2", err)

    def test_git_argv_handed_to_a_local_wrapper_fails(self) -> None:
        """The worst offender built the argv and passed it to its own `_run`."""
        rc, err = run_against(
            "def _run(args, cwd=None):\n    return args\n"
            "_run(['git', 'init', '--bare', '/tmp/x'])\n"
        )
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:3", err)

    def test_starred_git_argv_fails(self) -> None:
        rc, _ = run_against("def g(*a):\n    return ['git', *a]\n")
        self.assertEqual(rc, 1)

    def test_routed_through_the_helper_passes(self) -> None:
        rc, _ = run_against(
            "from ci_workflow_hygiene_test_helpers import repo_git_command\n"
            "import subprocess\n"
            "subprocess.run(repo_git_command('commit', '-m', 'x'), check=True)\n"
        )
        self.assertEqual(rc, 0)


class ExpectedValueTests(unittest.TestCase):
    """`["git", ...]` as data, not as a command line, stays legal."""

    def test_comparison_operand_passes(self) -> None:
        rc, _ = run_against("calls = []\nassert calls == ['git', 'fetch']\n")
        self.assertEqual(rc, 0)

    def test_completed_process_argument_passes(self) -> None:
        rc, _ = run_against(
            "import subprocess\n"
            "subprocess.CompletedProcess(['git', 'status'], 0)\n"
        )
        self.assertEqual(rc, 0)

    def test_element_of_an_enclosing_literal_passes(self) -> None:
        rc, _ = run_against("EXPECTED = [['git', 'fetch'], ['git', 'merge']]\n")
        self.assertEqual(rc, 0)


class RepositoryTests(unittest.TestCase):
    def test_repository_is_clean(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            rc = verifier.main(["--repo-root", str(REPO_ROOT)])
        self.assertEqual(rc, 0, "scripts/test_*.py must route git through the helper")


if __name__ == "__main__":
    unittest.main(verbosity=2)
