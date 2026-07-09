#!/usr/bin/env python3
"""Run static source-fence checks in one Python process.

Harness status lines are emitted on stderr. Module stdout is left untouched for
the imported verifier or test suite.
"""

from __future__ import annotations

import argparse
import dataclasses
import importlib.util
import inspect
import pathlib
import sys
import traceback
import unittest
from collections.abc import Iterator
from contextlib import contextmanager
from types import ModuleType
from typing import Any


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent
# These verifier families are owned by actionlint/ci-lint/full source-capture
# lanes. New static source-fence verifiers must not use these prefixes without
# adding explicit wiring here and in the owning lane.
NON_STATIC_VERIFY_PREFIXES = (
    "verify_ai_",
    "verify_ci_",
    "verify_runtime_capture_yaml",
)
# Add new standalone source-fence test suites here; paired test_verify_*.py
# suites are discovered automatically from their verifier filenames.
STANDALONE_TEST_FILENAMES = (
    "test_migrate_bolt_v3_decision_evidence_to_v15.py",
    "test_verify_runtime_capture_yaml.py",
    "test_local_verification_gate.py",
    "test_lane_governor.py",
    "test_run_fences.py",
    "test_verifier_io.py",
)


@dataclasses.dataclass
class FenceRunStats:
    glob_hits: int = 0
    glob_misses: int = 0
    read_text_hits: int = 0
    read_text_misses: int = 0
    rglob_hits: int = 0
    rglob_misses: int = 0


class SharedFenceCache:
    """Shared filesystem cache for read-only source fences.

    Fences run under this cache must be read-only with respect to repo-root
    paths. Test suites run outside the cache so fixture writes are never served
    stale contents.
    """

    def __init__(self, root: pathlib.Path) -> None:
        self.root = root.absolute()
        self.stats = FenceRunStats()
        self._glob_cache: dict[tuple[str, str, tuple[tuple[str, Any], ...]], tuple[pathlib.Path, ...]] = {}
        self._read_text_cache: dict[tuple[str, str | None, str | None], str] = {}
        self._rglob_cache: dict[tuple[str, str, tuple[tuple[str, Any], ...]], tuple[pathlib.Path, ...]] = {}

    def cacheable_path(self, path: pathlib.Path) -> pathlib.Path | None:
        absolute = path.absolute()
        try:
            absolute.relative_to(self.root)
        except ValueError:
            return None
        return absolute

    def glob(
        self,
        path: pathlib.Path,
        pattern: str,
        original_glob,
        **kwargs: Any,
    ) -> Iterator[pathlib.Path]:
        cache_path = self.cacheable_path(path)
        if cache_path is None:
            return original_glob(path, pattern, **kwargs)
        key = (str(cache_path), pattern, tuple(sorted(kwargs.items())))
        if key in self._glob_cache:
            self.stats.glob_hits += 1
        else:
            self.stats.glob_misses += 1
            self._glob_cache[key] = tuple(original_glob(path, pattern, **kwargs))
        return iter(self._glob_cache[key])

    def read_text(
        self,
        path: pathlib.Path,
        original_read_text,
        *args: Any,
        **kwargs: Any,
    ) -> str:
        encoding = kwargs.get("encoding")
        errors = kwargs.get("errors")
        if args:
            encoding = args[0]
        if len(args) > 1:
            errors = args[1]
        cache_path = self.cacheable_path(path)
        if cache_path is None:
            return original_read_text(path, *args, **kwargs)
        key = (str(cache_path), str(encoding) if encoding is not None else None, str(errors) if errors is not None else None)
        if key in self._read_text_cache:
            self.stats.read_text_hits += 1
        else:
            self.stats.read_text_misses += 1
            self._read_text_cache[key] = original_read_text(path, *args, **kwargs)
        return self._read_text_cache[key]

    def rglob(
        self,
        path: pathlib.Path,
        pattern: str,
        original_rglob,
        **kwargs: Any,
    ) -> Iterator[pathlib.Path]:
        cache_path = self.cacheable_path(path)
        if cache_path is None:
            return original_rglob(path, pattern, **kwargs)
        key = (str(cache_path), pattern, tuple(sorted(kwargs.items())))
        if key in self._rglob_cache:
            self.stats.rglob_hits += 1
        else:
            self.stats.rglob_misses += 1
            self._rglob_cache[key] = tuple(original_rglob(path, pattern, **kwargs))
        return iter(self._rglob_cache[key])


@contextmanager
def shared_filesystem_cache(cache: SharedFenceCache) -> Iterator[None]:
    original_glob = pathlib.Path.glob
    original_read_text = pathlib.Path.read_text
    original_rglob = pathlib.Path.rglob

    def cached_glob(path: pathlib.Path, pattern: str, **kwargs: Any):
        return cache.glob(path, pattern, original_glob, **kwargs)

    def cached_read_text(path: pathlib.Path, *args: Any, **kwargs: Any) -> str:
        return cache.read_text(path, original_read_text, *args, **kwargs)

    def cached_rglob(path: pathlib.Path, pattern: str, **kwargs: Any):
        return cache.rglob(path, pattern, original_rglob, **kwargs)

    pathlib.Path.glob = cached_glob
    pathlib.Path.read_text = cached_read_text
    pathlib.Path.rglob = cached_rglob
    try:
        yield
    finally:
        pathlib.Path.glob = original_glob
        pathlib.Path.read_text = original_read_text
        pathlib.Path.rglob = original_rglob


def is_static_fence_path(path: pathlib.Path) -> bool:
    name = path.name
    return (
        name.startswith("verify_")
        and name.endswith(".py")
        and not any(name.startswith(prefix) for prefix in NON_STATIC_VERIFY_PREFIXES)
    )


def discover_fence_paths(scripts_dir: pathlib.Path = SCRIPTS_DIR) -> list[pathlib.Path]:
    return sorted(path for path in scripts_dir.glob("verify_*.py") if is_static_fence_path(path))


def discover_test_paths(
    fence_paths: list[pathlib.Path],
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
) -> list[pathlib.Path]:
    # These suites validate fence logic with fixtures; merged-tree scanning belongs to the verify phase.
    paths: list[pathlib.Path] = []
    seen: set[pathlib.Path] = set()
    for fence_path in fence_paths:
        test_path = scripts_dir / f"test_{fence_path.name}"
        paths.append(test_path)
        seen.add(test_path)
    for filename in STANDALONE_TEST_FILENAMES:
        test_path = scripts_dir / filename
        if test_path not in seen:
            paths.append(test_path)
            seen.add(test_path)
    return paths


def import_module_from_path(path: pathlib.Path, index: int, phase: str) -> ModuleType:
    if not path.is_file():
        raise FileNotFoundError(path)
    module_name = f"_source_fence_{phase}_{index}_{path.stem}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not import {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def call_module(module: ModuleType) -> int:
    main = getattr(module, "main", None)
    if callable(main):
        signature = inspect.signature(main)
        required = [
            parameter
            for parameter in signature.parameters.values()
            if parameter.default is inspect.Signature.empty
            and parameter.kind
            in (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD, inspect.Parameter.KEYWORD_ONLY)
        ]
        if required:
            status = main([])
        else:
            status = main()
        if status is None:
            return 0
        if not isinstance(status, int):
            raise RuntimeError(f"{module.__name__}.main() returned non-integer status {status!r}")
        return status

    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    tests = [
        value
        for name, value in sorted(vars(module).items())
        if name.startswith("test_") and callable(value)
    ]
    if suite.countTestCases() > 0 and tests:
        raise RuntimeError(f"{module.__name__} mixes unittest TestCase classes and top-level test_* functions")
    if suite.countTestCases() == 0:
        if not tests:
            raise RuntimeError(f"{module.__name__} does not define callable main(), unittest cases, or test_* functions")
        for test in tests:
            test()
        return 0
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    return 0 if result.wasSuccessful() else 1


def run_module(
    path: pathlib.Path,
    index: int,
    *,
    phase: str,
    cache: SharedFenceCache | None = None,
) -> int:
    try:
        module = import_module_from_path(path, index, phase)
        original_argv = sys.argv
        sys.argv = [str(path)]
        try:
            if cache is None:
                status = call_module(module)
            else:
                with shared_filesystem_cache(cache):
                    status = call_module(module)
        finally:
            sys.argv = original_argv
    except SystemExit as exc:
        status = exc.code if isinstance(exc.code, int) else (0 if exc.code is None else 1)
    except Exception:
        print(f"FAIL: {path.name} raised an exception", file=sys.stderr)
        traceback.print_exc()
        return 1
    if status == 0:
        print(f"OK: {path.name}", file=sys.stderr)
        return 0
    print(f"FAIL: {path.name} exited {status}", file=sys.stderr)
    return 1


def run_fences_with_stats(
    *,
    root: pathlib.Path = REPO_ROOT,
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
    run_tests: bool = True,
) -> tuple[int, FenceRunStats]:
    cache = SharedFenceCache(root)
    failed = 0
    fence_paths = discover_fence_paths(scripts_dir)
    for index, path in enumerate(fence_paths, start=1):
        failed |= run_module(path, index, phase="verify", cache=cache)
    if run_tests:
        for index, path in enumerate(discover_test_paths(fence_paths, scripts_dir), start=1):
            failed |= run_module(path, index, phase="test")
    return (1 if failed else 0), cache.stats


def run_fences(
    *,
    root: pathlib.Path = REPO_ROOT,
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
    run_tests: bool = True,
) -> int:
    status, _stats = run_fences_with_stats(root=root, scripts_dir=scripts_dir, run_tests=run_tests)
    return status


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--root", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--scripts-dir", type=pathlib.Path, default=SCRIPTS_DIR)
    parser.add_argument("--fences-only", action="store_true", help="skip source-fence test suites")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    return run_fences(root=args.root, scripts_dir=args.scripts_dir, run_tests=not args.fences_only)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
