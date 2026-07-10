#!/usr/bin/env python3
"""Tests for verify_fixture_git_helper_usage.py."""

from __future__ import annotations

import contextlib
import io
import pathlib
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


class ExecutionEdgeTests(unittest.TestCase):
    def assert_violation(self, source: str, lineno: int) -> None:
        rc, err = run_against(source)
        self.assertEqual(rc, 1)
        self.assertIn(f"scripts/test_planted.py:{lineno}:", err)

    def test_tuple_argv_fails(self) -> None:
        self.assert_violation(
            "import subprocess\nsubprocess.run(('git', 'commit'), check=True)\n", 2
        )

    def test_os_system_string_fails(self) -> None:
        self.assert_violation("import os\nos.system('git commit -m x')\n", 2)

    def test_shell_true_string_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run('git commit -m x', shell=True, check=True)\n",
            2,
        )

    def test_shell_c_argv_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(['sh', '-c', 'git commit -m x'], check=True)\n",
            2,
        )

    def test_name_bound_to_git_fails(self) -> None:
        self.assert_violation(
            "import subprocess\ng = 'git'\nsubprocess.run([g, 'commit'], check=True)\n",
            3,
        )

    def test_shutil_which_git_fails(self) -> None:
        self.assert_violation(
            "import shutil\nimport subprocess\n"
            "subprocess.run([shutil.which('git'), 'commit'], check=True)\n",
            3,
        )

    def test_f_string_command_fails(self) -> None:
        self.assert_violation(
            "import subprocess\nbranch = 'main'\n"
            "subprocess.run(f'git checkout {branch}', shell=True, check=True)\n",
            3,
        )

    def test_bare_imported_subprocess_name_fails(self) -> None:
        self.assert_violation(
            "from subprocess import run\nrun(['git', 'status'], check=True)\n", 2
        )

    def test_env_wrapper_argv_fails(self) -> None:
        self.assert_violation(
            "import subprocess\nsubprocess.run(['env', 'git', 'commit'], check=True)\n",
            2,
        )

    def test_local_execution_wrapper_with_tuple_argv_fails(self) -> None:
        self.assert_violation(
            "def _run(args, *, cwd=None):\n"
            "    import subprocess\n"
            "    return subprocess.run(args, cwd=cwd)\n"
            "_run(('git', 'commit'))\n",
            4,
        )

    def test_local_execution_wrapper_with_list_argv_fails(self) -> None:
        self.assert_violation(
            "def _run(args, *, cwd=None):\n"
            "    import subprocess\n"
            "    return subprocess.run(args, cwd=cwd)\n"
            "_run(['git', 'commit'])\n",
            4,
        )

    def test_wrapper_resolution_reaches_fixed_point_and_records_index(self) -> None:
        self.assert_violation(
            "def outer(cwd, command):\n"
            "    return inner(command)\n"
            "def inner(command):\n"
            "    import subprocess\n"
            "    return subprocess.run(command)\n"
            "outer(None, ('git', 'commit'))\n",
            6,
        )

    def test_starred_forwarded_parameter_marks_wrapper(self) -> None:
        self.assert_violation(
            "def _run(args):\n"
            "    import subprocess\n"
            "    return subprocess.run(*args)\n"
            "_run(('git', 'commit'))\n",
            4,
        )

    def test_keyword_tuple_argv_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(args=('git', 'commit'), cwd=repo)\n",
            2,
        )

    def test_keyword_shell_command_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(args='git commit', shell=True)\n",
            2,
        )

    def test_os_keyword_command_fails(self) -> None:
        self.assert_violation(
            "import os\nos.system(command='git commit')\n",
            2,
        )

    def test_getoutput_keyword_command_fails(self) -> None:
        self.assert_violation(
            "import subprocess\nsubprocess.getoutput(cmd='git status')\n",
            2,
        )

    def test_name_bound_to_tuple_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "cmd = ('git', 'commit')\n"
            "subprocess.run(cmd)\n",
            3,
        )

    def test_name_bound_to_shutil_which_fails(self) -> None:
        self.assert_violation(
            "import shutil\nimport subprocess\n"
            "g = shutil.which('git')\n"
            "subprocess.run([g, 'commit'])\n",
            4,
        )

    def test_env_options_and_assignments_before_git_fail(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(['env', '-u', 'HOME', "
            "'GIT_OPTIONAL_LOCKS=0', 'git', 'commit'])\n",
            2,
        )

    def test_nested_wrapper_fails(self) -> None:
        self.assert_violation(
            "def outer():\n"
            "    import subprocess\n"
            "    def _run(a, cwd=None):\n"
            "        return subprocess.run(a, cwd=cwd)\n"
            "    _run(('git', 'commit'), cwd=repo)\n"
            "outer()\n",
            5,
        )

    def test_method_wrapper_fails(self) -> None:
        self.assert_violation(
            "class Runner:\n"
            "    def run(self, command):\n"
            "        import subprocess\n"
            "        return subprocess.run(command)\n"
            "Runner().run(('git', 'commit'))\n",
            5,
        )

    def test_async_wrapper_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "async def run(command):\n"
            "    return subprocess.run(command)\n"
            "run(('git', 'commit'))\n",
            4,
        )

    def test_lambda_wrapper_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "run = lambda command: subprocess.run(command)\n"
            "run(('git', 'commit'))\n",
            3,
        )

    def test_literal_string_constructions_fail(self) -> None:
        sources = (
            "import subprocess\nsubprocess.run(('g' + 'it', 'commit'))\n",
            "import subprocess\nsubprocess.run((''.join(('g', 'it')), 'commit'))\n",
            "import subprocess\nsubprocess.run(('{}{}'.format('g', 'it'), 'commit'))\n",
            "import subprocess\nsubprocess.run(('%s%s' % ('g', 'it'), 'commit'))\n",
            "import subprocess\nsubprocess.run((f'{\"g\"}{\"it\"}', 'commit'))\n",
        )
        for source in sources:
            with self.subTest(source=source):
                self.assert_violation(source, 2)

    def test_unresolved_execution_command_fails_closed(self) -> None:
        self.assert_violation(
            "import subprocess\nsubprocess.run(command_factory())\n",
            2,
        )

    def test_unresolved_env_command_fails_closed(self) -> None:
        self.assert_violation(
            "import subprocess\nsubprocess.run(['env', *command_factory()])\n",
            2,
        )

    def test_executable_override_to_git_fails(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(['commit'], executable='git')\n",
            2,
        )

    def test_proven_non_git_command_passes(self) -> None:
        rc, err = run_against(
            "import subprocess\nsubprocess.run(['python3', '--version'])\n"
        )
        self.assertEqual(rc, 0, err)


class ArgvSpellingTests(unittest.TestCase):
    def test_list_literal_fails(self) -> None:
        rc, err = run_against("def command():\n    return ['git', 'status']\n")
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:2:", err)

    def test_starred_git_argv_fails(self) -> None:
        rc, err = run_against("def g(*a):\n    return ['git', *a]\n")
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:2:", err)

    def test_absolute_git_path_fails(self) -> None:
        rc, err = run_against("command = ['/usr/bin/git', 'status']\n")
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:1:", err)

    def test_list_handed_to_non_execution_wrapper_fails(self) -> None:
        rc, err = run_against(
            "def _run(args):\n    return args\n_run(['git', 'init'])\n"
        )
        self.assertEqual(rc, 1)
        self.assertIn("scripts/test_planted.py:3:", err)


class ExpectedValueTests(unittest.TestCase):
    """Git argv used only as expected data stays legal."""

    def test_comparison_operand_passes(self) -> None:
        rc, _ = run_against("calls = []\nassert calls == ['git', 'fetch']\n")
        self.assertEqual(rc, 0)

    def test_completed_process_argument_passes(self) -> None:
        rc, _ = run_against(
            "import subprocess\n"
            "subprocess.CompletedProcess(['git', 'status'], 0)\n"
        )
        self.assertEqual(rc, 0)

    def test_called_process_error_argument_passes(self) -> None:
        rc, _ = run_against(
            "import subprocess\n"
            "subprocess.CalledProcessError(1, ['git', 'status'])\n"
        )
        self.assertEqual(rc, 0)

    def test_element_of_an_enclosing_literal_passes(self) -> None:
        rc, _ = run_against("EXPECTED = [['git', 'fetch'], ['git', 'merge']]\n")
        self.assertEqual(rc, 0)

    def test_tuple_assignment_passes(self) -> None:
        rc, _ = run_against("expected = ('git', 'status')\n")
        self.assertEqual(rc, 0)

    def test_tuple_argument_to_bare_assert_helper_passes(self) -> None:
        rc, _ = run_against(
            "assert_equal(calls[0]['args'], ('git', 'status'), 'label')\n"
        )
        self.assertEqual(rc, 0)

    def test_tuple_argument_to_data_holder_passes(self) -> None:
        rc, _ = run_against("CommandResult(('git', 'status'), 0)\n")
        self.assertEqual(rc, 0)

    def test_wrapper_using_repo_git_command_passes(self) -> None:
        rc, _ = run_against(
            "def git(repo, *args):\n"
            "    return run(repo_git_command(*args), cwd=repo)\n"
            "git(repo, 'commit')\n"
        )
        self.assertEqual(rc, 0)

    def test_non_execution_function_returning_process_data_passes(self) -> None:
        rc, _ = run_against(
            "def fake_run(command, **kwargs):\n"
            "    return subprocess.CompletedProcess(command, 0, 'ok', '')\n"
            "fake_run(('git', 'status'))\n"
        )
        self.assertEqual(rc, 0)

    def test_routed_through_helper_passes(self) -> None:
        rc, _ = run_against(
            "import subprocess\n"
            "subprocess.run(repo_git_command('commit'), check=True)\n"
        )
        self.assertEqual(rc, 0)


class FixtureConstructorTests(unittest.TestCase):
    def assert_violation(self, source: str, lineno: int = 1) -> None:
        rc, err = run_against(source)
        self.assertEqual(rc, 1)
        self.assertIn(f"scripts/test_planted.py:{lineno}:", err)
        self.assertIn("init_fixture_repo", err)
        self.assertIn("clone_fixture_repo", err)

    def test_repo_git_command_init_fails(self) -> None:
        self.assert_violation("repo_git_command('init', '-q')\n")

    def test_run_git_init_fails(self) -> None:
        self.assert_violation("run_git(repo, 'init')\n")

    def test_git_clone_fails(self) -> None:
        self.assert_violation("git(root, 'clone', a, b)\n")

    def test_commit_message_init_passes(self) -> None:
        rc, _ = run_against("repo_git_command('commit', '-m', 'init')\n")
        self.assertEqual(rc, 0)

    def test_fixture_helper_passes(self) -> None:
        rc, _ = run_against("init_fixture_repo(r, '--bare')\n")
        self.assertEqual(rc, 0)

    def test_global_options_before_init_fail(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(repo_git_command('-C', str(repo), 'init'))\n",
            2,
        )

    def test_git_config_pairs_before_clone_fail(self) -> None:
        self.assert_violation(
            "import subprocess\n"
            "subprocess.run(['git', '-c', 'gc.auto=0', 'clone', src, dst])\n",
            2,
        )

    def test_unrelated_init_and_clone_strings_pass(self) -> None:
        sources = (
            "print('init')\n",
            "config.get('init')\n",
            "import pathlib\npathlib.Path('clone')\n",
        )
        for source in sources:
            with self.subTest(source=source):
                rc, err = run_against(source)
                self.assertEqual(rc, 0, err)


class RepositoryTests(unittest.TestCase):
    def test_repository_is_clean(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
            rc = verifier.main(["--repo-root", str(REPO_ROOT)])
        self.assertEqual(rc, 0, stderr.getvalue())


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
