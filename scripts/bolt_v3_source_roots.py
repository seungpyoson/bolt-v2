#!/usr/bin/env python3
"""Shared resolution of the Bolt-v3 gated source roots for Python gates.

This module is the Python reader of the gated source roots. The list itself
lives in ONE place — the repo-root ``gated_source_roots.manifest`` — which is
also parsed by ``build.rs`` to generate the Rust ``GATED_SOURCE_ROOTS``
constant. There is no hand-maintained Python copy of the list; this module
parses the same manifest, so the Rust registry and the Python gates can never
drift.

Each gated source set contains one or more roots. Each root may resolve to a
single ``.rs`` file OR a directory of ``.rs`` files, and the canonical order is
lexicographic by the repo-relative path's raw POSIX bytes (locale/OS
independent, with backslash path components rejected to match the Rust
canonicalizer).

Python gates that read a gated source must resolve its files through this module
so they follow file moves (e.g. a strategy split from a single file to a
directory module) without hardcoding a layout.
"""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The single owner of the gated source-root list, shared with build.rs.
GATED_SOURCE_ROOTS_MANIFEST = REPO_ROOT / "gated_source_roots.manifest"

# Registry keys; must match STRATEGY_KEY / SUBMIT_ADMISSION_KEY /
# OUTCOME_GROUP_KEY / MAKER_KEY in src/source_canonicalization.rs and the
# ``[section]`` headers in the manifest.
STRATEGY_KEY = "strategy"
SUBMIT_ADMISSION_KEY = "submit_admission"
OUTCOME_GROUP_KEY = "outcome_group"
MAKER_KEY = "maker"


# The exact set of characters Rust ``str::trim()`` strips — the Unicode
# ``White_Space`` property (``char::is_whitespace()``). Python's bare
# ``str.strip()`` strips a SUPERSET: it additionally removes U+001C–U+001F (the
# information separators FS/GS/RS/US), which Rust keeps. Stripping this explicit
# set instead of calling ``.strip()`` keeps the Python parser byte-for-byte
# equivalent to build.rs's ``raw_line.trim()`` / key ``.trim()``.
_RUST_TRIM_WHITESPACE = (
    "\t\n\x0b\x0c\r "  # U+0009-U+000D, U+0020 space
    "\x85\xa0"  # U+0085 NEL, U+00A0 NBSP
    "\u1680"  # Ogham space mark
    "\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a"  # U+2000-U+200A
    "\u2028\u2029"  # line / paragraph separator
    "\u202f\u205f\u3000"  # narrow & medium math space, ideographic space
)


def _load_gated_source_roots() -> dict[str, tuple[str, ...]]:
    """Parse ``gated_source_roots.manifest`` into ``{key: (roots...)}``."""
    return _parse_manifest_text(
        GATED_SOURCE_ROOTS_MANIFEST.read_text(encoding="utf-8"),
        source_label=str(GATED_SOURCE_ROOTS_MANIFEST),
    )


def _parse_manifest_text(
    text: str, *, source_label: str = str(GATED_SOURCE_ROOTS_MANIFEST)
) -> dict[str, tuple[str, ...]]:
    """Parse manifest ``text`` into ``{key: (roots...)}``.

    Mirrors the build.rs ``parse_gated_source_roots`` function: ``#`` comments
    and blank lines are ignored; ``[key]`` starts a section; every other line is
    a repo-relative root. Invalid roots (absolute, backslash, or
    ``.``/``..``/empty components) and structural errors raise ``ValueError``
    with a ``file:line`` location, so a malformed manifest fails loudly on both
    the Rust and Python sides.

    The two Unicode-sensitive primitives are kept exactly in lock-step with
    build.rs so the parsers are equivalent for ALL inputs, not just ASCII:

    * Line splitting uses ``str.split("\\n")`` — NOT ``str.splitlines()`` — so the
      recognized terminators are exactly Rust ``str::lines()`` (``\\n`` and
      ``\\r\\n`` only). ``str.splitlines()`` additionally breaks on a bare ``\\r``
      and Unicode separators (U+2028/U+2029/…); using it would let a manifest the
      Rust build rejects parse cleanly in Python. A final empty line (from a
      trailing ``\\n``) is skipped as blank.
    * Whitespace trimming strips ``_RUST_TRIM_WHITESPACE`` — the Unicode
      ``White_Space`` set Rust ``str::trim()`` uses — NOT bare ``str.strip()``,
      which would also strip U+001C–U+001F (the information separators Rust
      keeps). This removes the trailing ``\\r`` of a ``\\r\\n`` terminator while
      matching ``raw_line.trim()`` / key ``.trim()`` on the Rust side.

    Every other operation (the ``[``/``]``/``#``/``/`` checks and the ``/`` and
    ``\\`` scans) is an ASCII-literal comparison identical in both languages, so
    with these two primitives aligned the parsers are exhaustively equivalent.
    Invalid input raises ``ValueError`` here and fails the build on the Rust side.
    """
    sections: dict[str, list[str]] = {}
    order: list[str] = []
    current: str | None = None
    for index, raw_line in enumerate(text.split("\n"), start=1):
        line = raw_line.strip(_RUST_TRIM_WHITESPACE)
        if not line or line.startswith("#"):
            continue
        location = f"{source_label}:{index}"
        if line.startswith("["):
            if not line.endswith("]"):
                raise ValueError(f"{location}: malformed section header {line!r}")
            key = line[1:-1].strip(_RUST_TRIM_WHITESPACE)
            if not key:
                raise ValueError(f"{location}: empty section key")
            if key in sections:
                raise ValueError(f"{location}: duplicate section [{key}]")
            sections[key] = []
            order.append(key)
            current = key
            continue
        components = line.split("/")
        if (
            line.startswith("/")
            or "\\" in line
            or any(component in ("", ".", "..") for component in components)
        ):
            raise ValueError(f"{location}: invalid repo-relative root {line!r}")
        if current is None:
            raise ValueError(f"{location}: root {line!r} precedes any [section] header")
        sections[current].append(line)

    if not order:
        raise ValueError(f"{source_label}: no gated source roots defined")
    for key in order:
        if not sections[key]:
            raise ValueError(f"{source_label}: section [{key}] has no roots")
    # The manifest must declare EXACTLY these four registry keys (mirrors the
    # build.rs parser): reject both missing and unexpected sections so a typo'd
    # header fails loudly here and on the Rust side instead of silently dropping
    # roots from the gated set.
    required_keys = (
        STRATEGY_KEY,
        SUBMIT_ADMISSION_KEY,
        OUTCOME_GROUP_KEY,
        MAKER_KEY,
    )
    for required in required_keys:
        if required not in sections:
            raise ValueError(f"{source_label}: required section [{required}] is missing")
    for key in order:
        if key not in required_keys:
            raise ValueError(
                f"{source_label}: unexpected section [{key}] "
                f"(expected exactly {list(required_keys)})"
            )
    return {key: tuple(sections[key]) for key in order}


_GATED_SOURCE_ROOTS = _load_gated_source_roots()

# Repo-relative roots, parsed from the manifest (the single owner). A root may be
# a file or a directory; the walk below resolves whichever it is at runtime.
STRATEGY_SOURCE_ROOTS = _GATED_SOURCE_ROOTS[STRATEGY_KEY]
STRATEGY_SOURCE_ROOT = STRATEGY_SOURCE_ROOTS[0]
SUBMIT_ADMISSION_SOURCE_ROOTS = _GATED_SOURCE_ROOTS[SUBMIT_ADMISSION_KEY]
SUBMIT_ADMISSION_SOURCE_ROOT = SUBMIT_ADMISSION_SOURCE_ROOTS[0]
OUTCOME_GROUP_SOURCE_ROOTS = _GATED_SOURCE_ROOTS[OUTCOME_GROUP_KEY]
MAKER_SOURCE_ROOTS = _GATED_SOURCE_ROOTS[MAKER_KEY]
MAKER_SOURCE_ROOT = MAKER_SOURCE_ROOTS[0]
MAX_SOURCE_FILE_BYTES = 8 * 1024 * 1024


def source_files(relative_root: str, repo_root: Path | None = None) -> list[Path]:
    """Return the gated root's `.rs` files in canonical order.

    IDENTITY case (the root is a regular `.rs` file): a single-element list with
    that file. DIRECTORY case: every `*.rs` file under the root, sorted
    lexicographically by the relative path's raw POSIX bytes — the same canonical
    order the Rust walk uses.
    """
    resolved_repo_root = REPO_ROOT if repo_root is None else repo_root
    root = resolved_repo_root / relative_root
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


def source_set_files(
    relative_roots: tuple[str, ...], repo_root: Path | None = None
) -> list[Path]:
    """Return every source-set `.rs` file in canonical repo-relative order."""
    resolved_repo_root = REPO_ROOT if repo_root is None else repo_root
    ordered: list[tuple[bytes, Path]] = []
    for relative_root in relative_roots:
        root_label = _normalized_relative_root(relative_root)
        root = resolved_repo_root / relative_root
        root_is_file = root.is_file()
        for path in source_files(relative_root, repo_root=resolved_repo_root):
            if root_is_file:
                label = root_label
            else:
                label = f"{root_label}/{path.relative_to(root).as_posix()}"
            ordered.append((label.encode("utf-8"), path))
    ordered.sort(key=lambda item: item[0])
    return [path for _label, path in ordered]


def module_text(relative_root: str | tuple[str, ...]) -> str:
    """Whole-module UTF-8 text of a gated root or source set, joined in canonical order.

    DIRECTORY case: each file's text concatenated in canonical order (raw file
    contents, no separators) — the same content order as the Rust
    `module_source_text` accessor, suitable for grepping function/marker presence
    across the module.
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
