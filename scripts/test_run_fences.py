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
            status = runner.run_fences(root=root, scripts_dir=scripts)

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
        "REPO_ROOT = Path(__file__).resolve().parents[1]\n"
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
            status, stats = runner.run_fences_with_stats(root=root, scripts_dir=scripts)

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
                status = runner.run_fences(root=root, scripts_dir=scripts)
        finally:
            sys.argv = original_argv

    if status != 0:
        raise AssertionError((status, stdout.getvalue(), stderr.getvalue()))


def main() -> int:
    assert_discovers_static_verify_modules_by_name()
    assert_reports_raised_fence_and_continues()
    assert_shared_filesystem_cache_spans_fences()
    assert_runner_argv_does_not_leak_to_fences()
    print("OK: run_fences self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
