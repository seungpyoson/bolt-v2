#!/usr/bin/env python3
"""Shared resolution of the Bolt-v3 gated source roots for Python gates.

This module is the SINGLE Python-side owner of the gated source root paths and
the canonical `.rs` walk order. It mirrors the Rust registry in
`src/source_canonicalization.rs` (`GATED_SOURCE_ROOTS`): each gated source set
contains one or more roots. Each root may resolve to a single `.rs` file OR a
directory of `.rs` files, and the canonical order is lexicographic by the
repo-relative path's raw POSIX bytes (locale/OS independent, with backslash path
components rejected to match the Rust canonicalizer).

Python gates that read a gated source must resolve its files through this module
so they follow file moves (e.g. the A3 strategy split from a single file to the
strategy directory module) without hardcoding a layout. There is exactly ONE
place each root path lives on the Python side, pointing at the same paths the
Rust registry owns.
"""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Repo-relative roots, mirroring `GATED_SOURCE_ROOTS` in
# `src/source_canonicalization.rs`. A root may be a file or a directory; the
# walk below resolves whichever it is at runtime.
STRATEGY_SOURCE_ROOTS = (
    "src/strategies/binary_oracle_edge_taker",
    "src/bolt_v3_book_sizing.rs",
    "src/bolt_v3_binary_outcome_edge.rs",
    "src/bolt_v3_executable_cost.rs",
)
STRATEGY_SOURCE_ROOT = STRATEGY_SOURCE_ROOTS[0]
SUBMIT_ADMISSION_SOURCE_ROOTS = ("src/bolt_v3_submit_admission.rs",)
SUBMIT_ADMISSION_SOURCE_ROOT = SUBMIT_ADMISSION_SOURCE_ROOTS[0]
MAX_SOURCE_FILE_BYTES = 8 * 1024 * 1024


def source_files(relative_root: str) -> list[Path]:
    """Return the gated root's `.rs` files in canonical order.

    IDENTITY case (the root is a regular `.rs` file): a single-element list with
    that file. DIRECTORY case: every `*.rs` file under the root, sorted
    lexicographically by the relative path's raw POSIX bytes — the same canonical
    order the Rust walk uses for framing and hashing.
    """
    root = REPO_ROOT / relative_root
    if root.is_symlink():
        raise ValueError(f"source root is a symlink: {root}")
    if root.is_file():
        return [root]
    if not root.is_dir():
        raise FileNotFoundError(
            f"gated source root is neither a regular file nor a directory: {root}"
        )
    files = []
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"source root contains a symlink: {path}")
        if path.is_file() and path.suffix == ".rs":
            relative_parts = path.relative_to(root).parts
            if any("\\" in part for part in relative_parts):
                raise ValueError(
                    f"source relative path component contains a backslash: {path}"
                )
            files.append(path)
    files.sort(key=lambda path: path.relative_to(root).as_posix().encode("utf-8"))
    return files


def _normalized_relative_root(relative_root: str) -> str:
    root = Path(relative_root)
    if root.is_absolute():
        raise ValueError(f"source root must be repo-relative: {relative_root}")
    parts = root.parts
    if not parts:
        raise ValueError("source root must not be empty")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"source root contains an unsupported component: {relative_root}")
    if any("\\" in part for part in parts):
        raise ValueError(f"source root component contains a backslash: {relative_root}")
    return "/".join(parts)


def source_set_files(relative_roots: tuple[str, ...]) -> list[Path]:
    """Return every source-set `.rs` file in canonical repo-relative order."""
    ordered: list[tuple[bytes, Path]] = []
    for relative_root in relative_roots:
        root_label = _normalized_relative_root(relative_root)
        root = REPO_ROOT / relative_root
        root_is_file = root.is_file()
        for path in source_files(relative_root):
            if root_is_file:
                label = root_label
            else:
                label = f"{root_label}/{path.relative_to(root).as_posix()}"
            ordered.append((label.encode("utf-8"), path))
    ordered.sort(key=lambda item: item[0])
    return [path for _label, path in ordered]


def module_text(relative_root: str | tuple[str, ...]) -> str:
    """Whole-module UTF-8 text of a gated root or source set, joined in canonical order.

    DIRECTORY case: each file's text concatenated in canonical order WITHOUT any
    framing bytes — the same content order as the Rust `module_source_text`
    accessor, suitable for grepping function/marker presence across the module.
    """
    texts = []
    if isinstance(relative_root, str):
        paths = source_files(relative_root)
    else:
        paths = source_set_files(relative_root)
    for path in paths:
        if path.stat().st_size > MAX_SOURCE_FILE_BYTES:
            raise ValueError(f"source file exceeds 8 MiB limit: {path}")
        texts.append(path.read_text(encoding="utf-8"))
    return "".join(texts)
