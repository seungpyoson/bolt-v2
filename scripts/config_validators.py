"""Shared pure config-validation leaf helpers for script-local loaders."""

from __future__ import annotations

import re


CONFIG_TEMPLATE_PLACEHOLDER_RE = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")


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


def require_string_map(
    parent: dict[str, object],
    key: str,
    label: str,
    *,
    error_cls: type[Exception],
) -> dict[str, str]:
    value = parent.get(key)
    if not isinstance(value, dict) or not value:
        raise error_cls(f"{label}.{key} must be a non-empty string table")
    if not all(isinstance(item_key, str) and item_key.strip() for item_key in value):
        raise error_cls(f"{label}.{key} keys must be non-empty strings")
    if not all(
        isinstance(item_value, str) and item_value.strip()
        for item_value in value.values()
    ):
        raise error_cls(f"{label}.{key} values must be non-empty strings")
    return dict(value)


def render_config_string_template(
    template: str,
    template_vars: dict[str, str],
    label: str,
    *,
    error_cls: type[Exception],
    require_same_name_github_bindings: bool = False,
) -> str:
    placeholders = set(CONFIG_TEMPLATE_PLACEHOLDER_RE.findall(template))
    if not placeholders:
        raise error_cls(f"{label} must include at least one template placeholder")
    missing_vars = sorted(placeholders - set(template_vars))
    if missing_vars:
        raise error_cls(f"{label} missing template vars: {missing_vars!r}")
    unused_vars = sorted(set(template_vars) - placeholders)
    if unused_vars:
        raise error_cls(f"{label} has unused template vars: {unused_vars!r}")
    if require_same_name_github_bindings:
        for name in sorted(placeholders):
            expected = f"${{{{ github.{name} }}}}"
            if template_vars[name] != expected:
                raise error_cls(f"{label} must bind {name} to {expected}")
    rendered = template
    for name in sorted(placeholders):
        rendered = rendered.replace(f"{{{name}}}", template_vars[name])
    return rendered


def as_text(value: object) -> str:
    return "" if value is None else str(value)
