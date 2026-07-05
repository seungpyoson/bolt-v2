#!/usr/bin/env python3
"""Self-tests for the in-process source-fence runner."""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def load_runner():
    import run_fences

    return run_fences


def write_dummy_standalone_tests(runner, scripts: pathlib.Path) -> None:
    for filename in runner.STANDALONE_TEST_FILENAMES:
        write(scripts / filename, "def main(): return 0\n")


def assert_discovers_static_verify_modules_by_name() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        scripts = pathlib.Path(tmp) / "scripts"
        write(scripts / "verify_bolt_v3_alpha.py", "def main(): return 0\n")
        write(scripts / "verify_ra_beta.py", "def main(): return 0\n")
        write(scripts / "verify_ci_workflow_hygiene.py", "def main(): return 0\n")
        write(scripts / "verify_runtime_capture_yaml.py", "def main(): return 0\n")
        write(scripts / "test_verify_bolt_v3_alpha.py", "def main(): return 0\n")

        discovered = [path.name for path in runner.discover_fence_paths(scripts)]

    if discovered != ["verify_bolt_v3_alpha.py", "verify_ra_beta.py"]:
        raise AssertionError(discovered)


def assert_discovers_paired_and_standalone_test_modules() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        scripts = pathlib.Path(tmp) / "scripts"
        fence_paths = [
            scripts / "verify_bolt_v3_alpha.py",
            scripts / "verify_ra_beta.py",
        ]
        discovered = [path.name for path in runner.discover_test_paths(fence_paths, scripts)]

    expected = [
        "test_verify_bolt_v3_alpha.py",
        "test_verify_ra_beta.py",
        *runner.STANDALONE_TEST_FILENAMES,
    ]
    if discovered != expected:
        raise AssertionError(discovered)


def assert_reports_raised_fence_and_continues() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_ok.py",
            "def main():\n"
            "    print('OK: synthetic ok fence passed.')\n"
            "    return 0\n",
        )
        write(
            scripts / "verify_bolt_v3_boom.py",
            "def main():\n"
            "    raise RuntimeError('synthetic boom')\n",
        )
        stdout = io.StringIO()
        stderr = io.StringIO()

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = runner.run_fences(root=root, scripts_dir=scripts, run_tests=False)

    combined = stdout.getvalue() + stderr.getvalue()
    if status != 1:
        raise AssertionError(status)
    if "OK: synthetic ok fence passed." not in combined:
        raise AssertionError(combined)
    if "FAIL: verify_bolt_v3_boom.py raised an exception" not in combined:
        raise AssertionError(combined)
    if "RuntimeError: synthetic boom" not in combined:
        raise AssertionError(combined)


def assert_shared_filesystem_cache_spans_fences() -> None:
    runner = load_runner()
    module_text = (
        "from pathlib import Path\n"
        "REPO_ROOT = Path(__file__).absolute().parents[1]\n"
        "def main():\n"
        "    for _ in range(2):\n"
        "        for path in (REPO_ROOT / 'src').rglob('*.rs'):\n"
        "            path.read_text(encoding='utf-8')\n"
        "    print('OK: cached synthetic fence passed.')\n"
        "    return 0\n"
    )
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(root / "src" / "lib.rs", "fn main() {}\n")
        write(scripts / "verify_bolt_v3_cached_a.py", module_text)
        write(scripts / "verify_bolt_v3_cached_b.py", module_text)

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status, stats = runner.run_fences_with_stats(root=root, scripts_dir=scripts, run_tests=False)

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))
    if stats.rglob_misses != 1 or stats.rglob_hits < 3:
        raise AssertionError(stats)
    if stats.read_text_misses != 1 or stats.read_text_hits < 3:
        raise AssertionError(stats)


def assert_runner_argv_does_not_leak_to_fences() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_argv.py",
            "import sys\n"
            "def main():\n"
            "    if sys.argv != [__file__]:\n"
            "        raise RuntimeError(f'unexpected argv: {sys.argv!r}')\n"
            "    return 0\n",
        )

        stdout = io.StringIO()
        stderr = io.StringIO()
        original_argv = sys.argv
        sys.argv = ["run_fences.py", "--root", str(root)]
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                status = runner.run_fences(root=root, scripts_dir=scripts, run_tests=False)
        finally:
            sys.argv = original_argv

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))


def assert_system_exit_none_is_success() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_exit_none.py",
            "import sys\n"
            "def main():\n"
            "    sys.exit()\n",
        )

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = runner.run_fences(root=root, scripts_dir=scripts, run_tests=False)

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))


def assert_tests_run_without_filesystem_cache() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_stale.py",
            "from pathlib import Path\n"
            "REPO_ROOT = Path(__file__).absolute().parents[1]\n"
            "def main():\n"
            "    path = REPO_ROOT / 'state.txt'\n"
            "    path.write_text('before', encoding='utf-8')\n"
            "    path.read_text(encoding='utf-8')\n"
            "    path.read_text(encoding='utf-8')\n"
            "    return 0\n",
        )
        write(
            scripts / "test_verify_bolt_v3_stale.py",
            "from pathlib import Path\n"
            "REPO_ROOT = Path(__file__).absolute().parents[1]\n"
            "def main():\n"
            "    path = REPO_ROOT / 'state.txt'\n"
            "    path.write_text('after', encoding='utf-8')\n"
            "    if path.read_text(encoding='utf-8') != 'after':\n"
            "        raise RuntimeError('test read saw cached verifier content')\n"
            "    return 0\n",
        )
        write_dummy_standalone_tests(runner, scripts)

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status, stats = runner.run_fences_with_stats(root=root, scripts_dir=scripts)

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))
    if stats.read_text_misses != 1 or stats.read_text_hits < 1:
        raise AssertionError(stats)


def assert_fences_only_cli_skips_test_phase() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(scripts / "verify_bolt_v3_cli_flag.py", "def main(): return 0\n")
        write(
            scripts / "test_verify_bolt_v3_cli_flag.py",
            "def main():\n"
            "    raise RuntimeError('paired test phase should not run')\n",
        )
        write_dummy_standalone_tests(runner, scripts)
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            fences_only_status = runner.main(
                [
                    "--root",
                    str(root),
                    "--scripts-dir",
                    str(scripts),
                    "--fences-only",
                ]
            )
        if fences_only_status != 0:
            raise AssertionError((fences_only_status, stdout.getvalue(), stderr.getvalue()))

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            full_status = runner.main(["--root", str(root), "--scripts-dir", str(scripts)])
    combined = stdout.getvalue() + stderr.getvalue()
    if full_status != 1:
        raise AssertionError((full_status, combined))
    if "paired test phase should not run" not in combined:
        raise AssertionError(combined)


def assert_filesystem_cache_skips_paths_outside_root() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside_tmp:
        root = pathlib.Path(tmp)
        outside = pathlib.Path(outside_tmp) / "outside.txt"
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_outside_root.py",
            "from pathlib import Path\n"
            f"OUTSIDE = Path({str(outside)!r})\n"
            "def main():\n"
            "    OUTSIDE.write_text('before', encoding='utf-8')\n"
            "    OUTSIDE.read_text(encoding='utf-8')\n"
            "    OUTSIDE.write_text('after', encoding='utf-8')\n"
            "    if OUTSIDE.read_text(encoding='utf-8') != 'after':\n"
            "        raise RuntimeError('outside-root path was cached')\n"
            "    return 0\n",
        )

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status, stats = runner.run_fences_with_stats(root=root, scripts_dir=scripts, run_tests=False)

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))
    if stats.read_text_misses != 0 or stats.read_text_hits != 0:
        raise AssertionError(stats)


def assert_mixed_unittest_and_bare_tests_fail_loud() -> None:
    runner = load_runner()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(
            scripts / "verify_bolt_v3_mixed_tests.py",
            "def main(): return 0\n",
        )
        write(
            scripts / "test_verify_bolt_v3_mixed_tests.py",
            "import unittest\n"
            "class MixedTest(unittest.TestCase):\n"
            "    def test_case(self):\n"
            "        pass\n"
            "def test_bare():\n"
            "    pass\n",
        )
        write_dummy_standalone_tests(runner, scripts)

        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            status = runner.run_fences(root=root, scripts_dir=scripts)

    combined = stdout.getvalue() + stderr.getvalue()
    if status != 1:
        raise AssertionError((status, combined))
    if "mixes unittest TestCase classes and top-level test_* functions" not in combined:
        raise AssertionError(combined)


def main() -> int:
    assert_discovers_static_verify_modules_by_name()
    assert_discovers_paired_and_standalone_test_modules()
    assert_reports_raised_fence_and_continues()
    assert_shared_filesystem_cache_spans_fences()
    assert_runner_argv_does_not_leak_to_fences()
    assert_system_exit_none_is_success()
    assert_tests_run_without_filesystem_cache()
    assert_fences_only_cli_skips_test_phase()
    assert_filesystem_cache_skips_paths_outside_root()
    assert_mixed_unittest_and_bare_tests_fail_loud()
    print("OK: run_fences self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
