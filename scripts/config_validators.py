"""Shared pure config-validation leaf helpers for script-local loaders."""

from __future__ import annotations


def require_table(
    parent: dict[str, object],
    key: str,
    label: str,
    *,
    error_cls: type[Exception],
) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise error_cls(f"{label}.{key} must be a table")
    return value


def require_string(
    parent: dict[str, object],
    key: str,
    label: str,
    *,
    error_cls: type[Exception],
) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise error_cls(f"{label}.{key} must be a non-empty string")
    return value


def require_positive_int(
    parent: dict[str, object],
    key: str,
    label: str,
    *,
    error_cls: type[Exception],
) -> int:
    value = parent.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise error_cls(f"{label}.{key} must be a positive integer")
    return value


def as_text(value: object) -> str:
    return "" if value is None else str(value)
