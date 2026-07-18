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
    for value in (True, False, 0, -1):
        expect_error(
            CustomConfigError,
            "config.limit must be a positive integer",
            lambda value=value: cv.require_positive_int(
                {"limit": value}, "limit", "config", error_cls=CustomConfigError
            ),
        )
    if cv.require_positive_int({"limit": 1}, "limit", "config", error_cls=CustomConfigError) != 1:
        raise AssertionError("positive integer should be returned unchanged")


def assert_string_template_validation_is_exact_and_shared() -> None:
    variables = cv.require_string_map(
        {
            "vars": {
                "run_id": "${{ github.run_id }}",
                "run_attempt": "${{ github.run_attempt }}",
            }
        },
        "vars",
        "config",
        error_cls=CustomConfigError,
    )
    rendered = cv.render_config_string_template(
        "artifact-{run_id}-{run_attempt}",
        variables,
        "config.template",
        error_cls=CustomConfigError,
        require_same_name_github_bindings=True,
    )
    if rendered != "artifact-${{ github.run_id }}-${{ github.run_attempt }}":
        raise AssertionError(f"template rendered unexpected value: {rendered!r}")

    repeated = cv.render_config_string_template(
        "artifact-{run_id}-{run_id}",
        {"run_id": "${{ github.run_id }}"},
        "config.template",
        error_cls=CustomConfigError,
        require_same_name_github_bindings=True,
    )
    if repeated != "artifact-${{ github.run_id }}-${{ github.run_id }}":
        raise AssertionError(f"repeated placeholder rendered unexpected value: {repeated!r}")

    malformed_templates = (
        "artifact-{run_id}-{",
        "artifact-{run_id}-}",
        "artifact-{{run_id}}",
        "artifact-{run-id}",
        "artifact-{9run_id}",
        "artifact-{}",
        "artifact-{run_id}{run_attempt}}",
    )
    for template in malformed_templates:
        expect_error(
            CustomConfigError,
            "config.template contains malformed template placeholder syntax",
            lambda template=template: cv.render_config_string_template(
                template,
                {
                    "run_id": "${{ github.run_id }}",
                    "run_attempt": "${{ github.run_attempt }}",
                },
                "config.template",
                error_cls=CustomConfigError,
            ),
        )

    expect_error(
        CustomConfigError,
        "config.vars values must be non-empty strings",
        lambda: cv.require_string_map(
            {"vars": {"run_id": 7}},
            "vars",
            "config",
            error_cls=CustomConfigError,
        ),
    )
    expect_error(
        CustomConfigError,
        "config.template missing template vars: ['run_attempt']",
        lambda: cv.render_config_string_template(
            "artifact-{run_id}-{run_attempt}",
            {"run_id": "${{ github.run_id }}"},
            "config.template",
            error_cls=CustomConfigError,
        ),
    )
    expect_error(
        CustomConfigError,
        "config.template has unused template vars: ['rogue']",
        lambda: cv.render_config_string_template(
            "artifact-{run_id}",
            {
                "run_id": "${{ github.run_id }}",
                "rogue": "${{ github.rogue }}",
            },
            "config.template",
            error_cls=CustomConfigError,
        ),
    )
    expect_error(
        CustomConfigError,
        "config.template must bind run_id to ${{ github.run_id }}",
        lambda: cv.render_config_string_template(
            "artifact-{run_id}",
            {"run_id": "${{ github.sha }}"},
            "config.template",
            error_cls=CustomConfigError,
            require_same_name_github_bindings=True,
        ),
    )


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
    assert_string_template_validation_is_exact_and_shared()
    assert_as_text_matches_canonical_semantics()
    assert_error_cls_is_required_and_propagated()
    print("OK: config_validators tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
