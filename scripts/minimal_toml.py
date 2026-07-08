"""Minimal TOML subset reader for repo bootstrap scripts."""

from __future__ import annotations

# BEGIN embedded scripts/minimal_toml.py
import json
import pathlib
import re
from typing import Any


SAFE_TOML_IDENTIFIER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


class MinimalTomlError(ValueError):
    pass


def _display_path(path: pathlib.Path, display_path: pathlib.Path | str | None) -> str:
    return str(display_path if display_path is not None else path)


def _fail(error_cls: type[Exception], message: str) -> None:
    raise error_cls(message)


def check_size(
    path: pathlib.Path,
    *,
    max_bytes: int | None = None,
    display_path: pathlib.Path | str | None = None,
    error_cls: type[Exception] = MinimalTomlError,
) -> None:
    if max_bytes is None:
        return
    size = path.stat().st_size
    if size > max_bytes:
        shown = _display_path(path, display_path)
        _fail(error_cls, f"{shown} exceeds maximum size of {max_bytes} bytes")


def load(
    path: pathlib.Path,
    *,
    max_bytes: int | None = None,
    display_path: pathlib.Path | str | None = None,
    error_cls: type[Exception] = MinimalTomlError,
) -> dict[str, Any]:
    check_size(path, max_bytes=max_bytes, display_path=display_path, error_cls=error_cls)
    shown = _display_path(path, display_path)
    data: dict[str, Any] = {}
    current: dict[str, Any] = data
    with path.open("r", encoding="utf-8") as handle:
        lines = enumerate(handle, start=1)
        for lineno, raw_line in lines:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("["):
                if not line.endswith("]"):
                    _fail(error_cls, f"{shown}:{lineno}: invalid TOML table header")
                current = data
                for part in line[1:-1].split("."):
                    if not part or not SAFE_TOML_IDENTIFIER_RE.match(part):
                        _fail(error_cls, f"{shown}:{lineno}: unsupported table name")
                    child = current.setdefault(part, {})
                    if not isinstance(child, dict):
                        _fail(error_cls, f"{shown}:{lineno}: table conflicts with scalar")
                    current = child
                continue
            key, sep, value_text = line.partition("=")
            if not sep:
                _fail(error_cls, f"{shown}:{lineno}: expected key = value")
            key = key.strip()
            if key.startswith('"') and key.endswith('"'):
                try:
                    parsed_key = json.loads(key)
                except json.JSONDecodeError as exc:
                    raise error_cls(f"{shown}:{lineno}: invalid key") from exc
                if not isinstance(parsed_key, str) or not parsed_key:
                    _fail(error_cls, f"{shown}:{lineno}: invalid key")
                key = parsed_key
            elif not SAFE_TOML_IDENTIFIER_RE.match(key):
                _fail(error_cls, f"{shown}:{lineno}: unsupported key")
            value_text = value_text.strip()
            if value_text.startswith('"') and value_text.endswith('"'):
                try:
                    value: Any = json.loads(value_text)
                except json.JSONDecodeError as exc:
                    raise error_cls(f"{shown}:{lineno}: invalid string") from exc
            elif value_text.startswith("[") and value_text.endswith("]"):
                try:
                    value = json.loads(value_text)
                except json.JSONDecodeError as exc:
                    raise error_cls(f"{shown}:{lineno}: invalid array") from exc
                if not all(isinstance(item, str) for item in value):
                    _fail(error_cls, f"{shown}:{lineno}: unsupported array")
            elif value_text == "[":
                value = []
                for array_lineno, raw_array_line in lines:
                    item_text = raw_array_line.strip()
                    if not item_text or item_text.startswith("#"):
                        continue
                    if item_text == "]":
                        break
                    if item_text.endswith(","):
                        item_text = item_text[:-1].strip()
                    if not item_text.startswith('"') or not item_text.endswith('"'):
                        _fail(error_cls, f"{shown}:{array_lineno}: unsupported array")
                    try:
                        item = json.loads(item_text)
                    except json.JSONDecodeError as exc:
                        raise error_cls(f"{shown}:{array_lineno}: invalid string") from exc
                    if not isinstance(item, str):
                        _fail(error_cls, f"{shown}:{array_lineno}: unsupported array")
                    value.append(item)
                else:
                    _fail(error_cls, f"{shown}:{lineno}: unterminated array")
            elif value_text in ("true", "false"):
                value = value_text == "true"
            elif value_text.isdigit():
                value = int(value_text)
            else:
                _fail(error_cls, f"{shown}:{lineno}: unsupported value")
            current[key] = value
    return data
# END embedded scripts/minimal_toml.py
