#!/usr/bin/env python3
"""Unit tests for shared config-validation leaf helpers."""

from __future__ import annotations

import config_validators as cv


class CustomConfigError(Exception):
    """Test-specific error class for propagation checks."""


def expect_error(expected_type: type[Exception], expected_message: str, fn) -> None:
    try:
        fn()
    except expected_type as exc:
        if str(exc) != expected_message:
            raise AssertionError(f"expected {expected_message!r}, got {str(exc)!r}") from exc
    else:
        raise AssertionError(f"expected {expected_type.__name__}: {expected_message}")


def assert_require_table_rejects_non_tables() -> None:
    expect_error(
        CustomConfigError,
        "config.api must be a table",
        lambda: cv.require_table({"api": []}, "api", "config", error_cls=CustomConfigError),
    )


def assert_require_string_validates_required_strings() -> None:
    expect_error(
        CustomConfigError,
        "config.name must be a non-empty string",
        lambda: cv.require_string({"name": ""}, "name", "config", error_cls=CustomConfigError),
    )
    expect_error(
        CustomConfigError,
        "config.name must be a non-empty string",
        lambda: cv.require_string({"name": 7}, "name", "config", error_cls=CustomConfigError),
    )
    value = cv.require_string({"name": "   "}, "name", "config", error_cls=CustomConfigError)
    if value != "   ":
        raise AssertionError(f"whitespace string should be preserved, got {value!r}")


def assert_require_positive_int_rejects_bool_and_non_positive_values() -> None:
    for value in (True, 0, -1):
        expect_error(
            CustomConfigError,
            "config.limit must be a positive integer",
            lambda value=value: cv.require_positive_int(
                {"limit": value}, "limit", "config", error_cls=CustomConfigError
            ),
        )
    if cv.require_positive_int({"limit": 1}, "limit", "config", error_cls=CustomConfigError) != 1:
        raise AssertionError("positive integer should be returned unchanged")


def assert_as_text_matches_canonical_semantics() -> None:
    cases = (
        (None, ""),
        (5, "5"),
        ("value", "value"),
        (["a", 1], "['a', 1]"),
    )
    for raw, expected in cases:
        actual = cv.as_text(raw)
        if actual != expected:
            raise AssertionError(f"as_text({raw!r}) = {actual!r}, expected {expected!r}")


def assert_error_cls_is_required_and_propagated() -> None:
    expect_error(
        CustomConfigError,
        "root.child must be a table",
        lambda: cv.require_table({"child": None}, "child", "root", error_cls=CustomConfigError),
    )
    try:
        cv.require_table({"child": {}}, "child", "root")
    except TypeError:
        pass
    else:
        raise AssertionError("missing error_cls must fail loudly")


def main() -> int:
    assert_require_table_rejects_non_tables()
    assert_require_string_validates_required_strings()
    assert_require_positive_int_rejects_bool_and_non_positive_values()
    assert_as_text_matches_canonical_semantics()
    assert_error_cls_is_required_and_propagated()
    print("OK: config_validators tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
