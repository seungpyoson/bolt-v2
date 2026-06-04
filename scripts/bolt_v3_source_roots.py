#!/usr/bin/env python3
"""Shared resolution of the Bolt-v3 gated source roots for Python gates.

This module is the SINGLE Python-side owner of the gated source root paths and
the canonical `.rs` walk order. It mirrors the Rust registry in
`src/source_canonicalization.rs` (`GATED_SOURCE_ROOTS`): each gated root may
resolve to a single `.rs` file OR a directory of `.rs` files, and the canonical
order is lexicographic by the relative path's raw POSIX bytes (locale/OS
independent, `\\` normalized to `/`).

Python gates that read a gated source must resolve its files through this module
so they follow file moves (e.g. the A3 strategy split from a single file to the
directory `{mod.rs, selection.rs}`) without hardcoding a layout. There is exactly
ONE place each root path lives on the Python side, pointing at the same paths the
Rust registry owns.
"""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Repo-relative roots, mirroring `GATED_SOURCE_ROOTS` in
# `src/source_canonicalization.rs`. A root may be a file or a directory; the
# walk below resolves whichever it is at runtime.
STRATEGY_SOURCE_ROOT = "src/strategies/binary_oracle_edge_taker"
SUBMIT_ADMISSION_SOURCE_ROOT = "src/bolt_v3_submit_admission.rs"
MAX_SOURCE_FILE_BYTES = 1024 * 1024


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
            files.append(path)
    files.sort(key=lambda path: path.relative_to(root).as_posix().encode("utf-8"))
    return files


def module_text(relative_root: str) -> str:
    """Whole-module UTF-8 text of a gated root, joined in canonical order.

    DIRECTORY case: each file's text concatenated in canonical order WITHOUT any
    framing bytes — the same content order as the Rust `module_source_text`
    accessor, suitable for grepping function/marker presence across the module.
    """
    texts = []
    for path in source_files(relative_root):
        if path.stat().st_size > MAX_SOURCE_FILE_BYTES:
            raise ValueError(f"source file exceeds 1 MiB limit: {path}")
        texts.append(path.read_text(encoding="utf-8"))
    return "".join(texts)
