#!/usr/bin/env python3
"""Run static source-fence verifiers in one Python process."""

from __future__ import annotations

import argparse
import dataclasses
import importlib.util
import inspect
import pathlib
import sys
import traceback
from collections.abc import Iterator
from contextlib import contextmanager
from types import ModuleType
from typing import Any


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent
NON_STATIC_VERIFY_PREFIXES = (
    "verify_ai_",
    "verify_ci_",
    "verify_runtime_capture_yaml",
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
    def __init__(self) -> None:
        self.stats = FenceRunStats()
        self._glob_cache: dict[tuple[str, str], tuple[pathlib.Path, ...]] = {}
        self._read_text_cache: dict[tuple[str, str | None, str | None], str] = {}
        self._rglob_cache: dict[tuple[str, str], tuple[pathlib.Path, ...]] = {}

    def glob(
        self,
        path: pathlib.Path,
        pattern: str,
        original_glob,
        **kwargs: Any,
    ) -> Iterator[pathlib.Path]:
        key = (str(path.resolve()), pattern, tuple(sorted(kwargs.items())))
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
        key = (str(path.resolve()), str(encoding) if encoding is not None else None, str(errors) if errors is not None else None)
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
        key = (str(path.resolve()), pattern, tuple(sorted(kwargs.items())))
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


def import_fence(path: pathlib.Path, index: int) -> ModuleType:
    module_name = f"_source_fence_{index}_{path.stem}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not import {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def call_main(module: ModuleType) -> int:
    main = getattr(module, "main", None)
    if not callable(main):
        raise RuntimeError(f"{module.__name__} does not define callable main()")
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


def run_fence(path: pathlib.Path, index: int, cache: SharedFenceCache) -> int:
    try:
        module = import_fence(path, index)
        with shared_filesystem_cache(cache):
            original_argv = sys.argv
            sys.argv = [str(path)]
            try:
                status = call_main(module)
            finally:
                sys.argv = original_argv
    except SystemExit as exc:
        status = exc.code if isinstance(exc.code, int) else 1
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
) -> tuple[int, FenceRunStats]:
    del root
    cache = SharedFenceCache()
    failed = 0
    for index, path in enumerate(discover_fence_paths(scripts_dir), start=1):
        failed |= run_fence(path, index, cache)
    return (1 if failed else 0), cache.stats


def run_fences(
    *,
    root: pathlib.Path = REPO_ROOT,
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
) -> int:
    status, _stats = run_fences_with_stats(root=root, scripts_dir=scripts_dir)
    return status


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--scripts-dir", type=pathlib.Path, default=SCRIPTS_DIR)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    return run_fences(root=args.root, scripts_dir=args.scripts_dir)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
