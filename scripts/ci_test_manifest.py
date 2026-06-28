#!/usr/bin/env python3
"""Parse Cargo [[test]] harnesses and their member modules."""

from __future__ import annotations

from dataclasses import dataclass
import pathlib
import re
from typing import Any

try:
    import tomllib as _toml
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback.
    try:
        import tomli as _toml  # type: ignore[no-redef]
    except ModuleNotFoundError:  # pragma: no cover - system Python may run with -S.
        _toml = None


class CiTestManifestError(ValueError):
    pass


@dataclass(frozen=True)
class CiTestManifest:
    member_to_harness: dict[str, str]
    harness_to_members: dict[str, tuple[str, ...]]


MOD_DECL_RE = re.compile(r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
PATH_ATTR_RE = re.compile(r"""^\s*#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]\s*$""")
# Rust char / byte-char literal: '{', '}', 'a', '\n', '\x41', '\u{7b}', '"',
# b'{', ...
# The closing quote distinguishes a char literal from a lifetime ('a, 'static),
# which must be left intact.
CHAR_LITERAL_RE = re.compile(r"b?'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|.)|[^'\\])'")


def build_test_manifest(manifest_path: pathlib.Path | str, tests_root: pathlib.Path | str) -> CiTestManifest:
    manifest = pathlib.Path(manifest_path)
    root = pathlib.Path(tests_root)
    data = _read_toml(manifest)
    entries = data.get("test", [])
    if not isinstance(entries, list):
        raise CiTestManifestError("[[test]] entries must parse as a TOML array")

    seen_harnesses: set[str] = set()
    test_roots: list[tuple[str, pathlib.Path]] = []
    root_stem_to_harness: dict[str, str] = {}
    for entry in entries:
        harness, root_file = _test_entry_root(manifest, root, entry)
        if harness in seen_harnesses:
            raise CiTestManifestError(f"duplicate [[test]] harness target {harness!r}")
        seen_harnesses.add(harness)
        existing_root_harness = root_stem_to_harness.get(root_file.stem)
        if existing_root_harness is not None and existing_root_harness != harness:
            raise CiTestManifestError(
                f"test root {root_file.name!r} belongs to both {existing_root_harness!r} and {harness!r}"
            )
        root_stem_to_harness[root_file.stem] = harness
        test_roots.append((harness, root_file))

    member_to_harness: dict[str, str] = {}
    harness_to_members: dict[str, tuple[str, ...]] = {}
    root_test_stems = {path.stem for path in root.glob("*.rs")}
    for harness, root_file in test_roots:
        root_stem = root_file.stem
        members = _ordered_unique(
            [
                root_stem,
                *(
                    mod_name
                    for mod_name, path_attr in _top_level_mod_declarations(root_file)
                    if _mod_is_root_test_member(
                        root,
                        root_file.parent,
                        root_test_stems,
                        root_stem_to_harness,
                        harness,
                        mod_name,
                        path_attr,
                    )
                ),
            ]
        )
        harness_to_members[harness] = tuple(members)
        for member in members:
            existing = member_to_harness.get(member)
            if existing is not None and existing != harness:
                raise CiTestManifestError(
                    f"test member {member!r} belongs to both {existing!r} and {harness!r}"
                )
            member_to_harness[member] = harness

    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)


def _read_toml(manifest_path: pathlib.Path) -> dict[str, Any]:
    if _toml is None:
        raise CiTestManifestError("tomllib or tomli is required to parse Cargo test manifests")
    try:
        with manifest_path.open("rb") as handle:
            return _toml.load(handle)
    except _toml.TOMLDecodeError as exc:
        raise CiTestManifestError(f"invalid Cargo manifest TOML: {manifest_path}") from exc


def _test_entry_root(
    manifest_path: pathlib.Path,
    tests_root: pathlib.Path,
    entry: Any,
) -> tuple[str, pathlib.Path]:
    if not isinstance(entry, dict):
        raise CiTestManifestError("[[test]] entry must be a TOML table")
    name = entry.get("name")
    path = entry.get("path")
    if not isinstance(name, str) or not name:
        raise CiTestManifestError("[[test]] entry must have a non-empty string name")
    if not isinstance(path, str) or not path:
        raise CiTestManifestError(f"[[test]] {name!r} must have a non-empty string path")

    root_file = (manifest_path.parent / pathlib.PurePosixPath(path.replace("\\", "/"))).resolve()
    expected_root = tests_root.resolve()
    if root_file.parent != expected_root or root_file.suffix != ".rs":
        raise CiTestManifestError(f"[[test]] {name!r} path must be a root file under {tests_root}")
    if not root_file.is_file():
        raise CiTestManifestError(f"[[test]] {name!r} root file does not exist: {root_file}")
    return name, root_file


def _top_level_mod_declarations(root_file: pathlib.Path) -> list[tuple[str, str | None]]:
    text = root_file.read_text(encoding="utf-8")
    masked = _mask_rust_non_code(text)
    depth = 0
    pending_path_attr: str | None = None
    members: list[tuple[str, str | None]] = []
    for original_line, masked_line in zip(text.splitlines(), masked.splitlines()):
        if depth == 0:
            attr_match = PATH_ATTR_RE.match(original_line)
            if attr_match is not None:
                pending_path_attr = attr_match.group(1)
            match = MOD_DECL_RE.match(masked_line)
            if match is not None:
                members.append((match.group(1), pending_path_attr))
                pending_path_attr = None
            elif attr_match is None and masked_line.strip() and not masked_line.lstrip().startswith("#"):
                pending_path_attr = None
        for char in masked_line:
            if char == "{":
                depth += 1
            elif char == "}":
                depth = max(0, depth - 1)
    return members


def _mod_is_root_test_member(
    tests_root: pathlib.Path,
    root_file_parent: pathlib.Path,
    root_test_stems: set[str],
    root_stem_to_harness: dict[str, str],
    current_harness: str,
    mod_name: str,
    path_attr: str | None,
) -> bool:
    explicit_harness = root_stem_to_harness.get(mod_name)
    if explicit_harness is not None and explicit_harness != current_harness:
        return False
    if path_attr is None:
        return mod_name in root_test_stems
    member_path = (root_file_parent / pathlib.PurePosixPath(path_attr.replace("\\", "/"))).resolve()
    try:
        member_path.relative_to(tests_root.resolve())
    except ValueError:
        return False
    return member_path.suffix == ".rs"


def _mask_rust_non_code(text: str) -> str:
    chars = list(text)
    i = 0
    while i < len(chars):
        raw_end = _raw_string_end(text, i)
        if raw_end is not None:
            _blank(chars, i, raw_end)
            i = raw_end
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            if end == -1:
                end = len(chars)
            _blank(chars, i, end)
            i = end
            continue
        if text.startswith("/*", i):
            end = _block_comment_end(text, i + 2)
            _blank(chars, i, end)
            i = end
            continue
        char_literal_match = CHAR_LITERAL_RE.match(text, i)
        if char_literal_match is not None:
            _blank(chars, i, char_literal_match.end())
            i = char_literal_match.end()
            continue
        if text.startswith('b"', i) or text[i] == '"':
            start = i
            i += 2 if text.startswith('b"', i) else 1
            escaped = False
            while i < len(chars):
                char = text[i]
                i += 1
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    break
            _blank(chars, start, i)
            continue
        i += 1
    return "".join(chars)


def _raw_string_end(text: str, start: int) -> int | None:
    prefix_len = 0
    if text.startswith("br", start):
        prefix_len = 2
    elif text.startswith("r", start):
        prefix_len = 1
    else:
        return None
    hashes_start = start + prefix_len
    hashes_end = hashes_start
    while hashes_end < len(text) and text[hashes_end] == "#":
        hashes_end += 1
    if hashes_end >= len(text) or text[hashes_end] != '"':
        return None
    hashes = text[hashes_start:hashes_end]
    close = '"' + hashes
    end = text.find(close, hashes_end + 1)
    return len(text) if end == -1 else end + len(close)


def _block_comment_end(text: str, start: int) -> int:
    depth = 1
    i = start
    while i < len(text):
        if text.startswith("/*", i):
            depth += 1
            i += 2
        elif text.startswith("*/", i):
            depth -= 1
            i += 2
            if depth == 0:
                return i
        else:
            i += 1
    return len(text)


def _blank(chars: list[str], start: int, end: int) -> None:
    for index in range(start, min(end, len(chars))):
        if chars[index] != "\n":
            chars[index] = " "


def _ordered_unique(values: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            ordered.append(value)
    return ordered
