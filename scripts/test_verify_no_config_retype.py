#!/usr/bin/env python3
"""Self-tests for the config-retype fence."""

from __future__ import annotations

import pathlib
import tempfile

import verify_no_config_retype as verifier


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def protected(value: str, source: str = "synthetic.toml") -> tuple[verifier.ProtectedString, ...]:
    return (verifier.ProtectedString(value=value, source=source),)


def test_strict_file_retype_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        target = scripts / "strict.py"
        write(target, 'VALUE = "managed_light"\n')

        hits = verifier.scan_literals(
            root=root,
            scripts_dir=scripts,
            strict_paths=frozenset({"scripts/strict.py"}),
            registered={},
            protected=protected("managed_light"),
        )
        errors, ratchet_count = verifier.evaluate_hits(hits, ratchet_baseline=0)

    if ratchet_count != 0:
        raise AssertionError(ratchet_count)
    if len(errors) != 1 or "strict.py:1" not in errors[0] or "managed_light" not in errors[0]:
        raise AssertionError(errors)


def test_registered_payloads_require_reasons_and_suppress_hits() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(scripts / "strict.py", 'VALUE = "managed_light"\n')

        try:
            verifier.registered_payloads((verifier.RegisteredPayload("managed_light", ""),))
        except ValueError as exc:
            if "reason" not in str(exc):
                raise AssertionError(exc)
        else:
            raise AssertionError("registered payload without a reason must fail")

        hits = verifier.scan_literals(
            root=root,
            scripts_dir=scripts,
            strict_paths=frozenset({"scripts/strict.py"}),
            registered=verifier.registered_payloads(
                (verifier.RegisteredPayload("managed_light", "synthetic malformed payload"),)
            ),
            protected=protected("managed_light"),
        )

    if hits:
        raise AssertionError(hits)


def test_ratchet_mode_allows_only_non_increasing_counts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        scripts = root / "scripts"
        write(scripts / "legacy.py", 'A = "managed_light"\nB = "managed_light"\n')

        hits = verifier.scan_literals(
            root=root,
            scripts_dir=scripts,
            strict_paths=frozenset({"scripts/strict.py"}),
            registered={},
            protected=protected("managed_light"),
        )
        high_errors, high_count = verifier.evaluate_hits(hits, ratchet_baseline=2)
        low_errors, low_count = verifier.evaluate_hits(hits, ratchet_baseline=1)

    if high_count != 2 or high_errors:
        raise AssertionError((high_count, high_errors))
    if low_count != 2 or not low_errors or "ratchet retype count increased" not in low_errors[0]:
        raise AssertionError((low_count, low_errors))


def test_config_lines_and_table_values_are_protected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write(root / "synthetic.toml", "managed_value = \"alpha\"\n")
        config_values = verifier.config_line_values(root, ("synthetic.toml",))
        flattened = set(verifier.flatten_string_values({"checks": ("managed_gate",)}))

    if 'managed_value = "alpha"' not in {entry.value for entry in config_values}:
        raise AssertionError(config_values)
    if flattened != {"managed_gate"}:
        raise AssertionError(flattened)


def test_missing_governed_artifact_is_loud() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        write(root / "present.toml", "managed_value = \"alpha\"\n")

        try:
            verifier.config_line_values(root, ("present.toml", "missing.toml"))
        except ValueError as exc:
            if "missing.toml" not in str(exc):
                raise AssertionError(exc)
        else:
            raise AssertionError("missing governed config artifact must fail loudly")


def test_flatten_string_values_is_closed_world() -> None:
    flattened = set(
        verifier.flatten_string_values(
            {
                "strings": ["managed_gate", ("managed_docs",)],
                "sets_numbers_and_bools": [{"managed_actionlint"}, 1, 2.5, True, False],
            }
        )
    )

    if flattened != {"managed_gate", "managed_docs", "managed_actionlint"}:
        raise AssertionError(flattened)
    try:
        list(verifier.flatten_string_values({"unsupported": b"bytes"}))
    except TypeError as exc:
        if "bytes" not in str(exc):
            raise AssertionError(exc)
    else:
        raise AssertionError("unsupported containers must fail loudly")


def main() -> int:
    test_strict_file_retype_fails()
    test_registered_payloads_require_reasons_and_suppress_hits()
    test_ratchet_mode_allows_only_non_increasing_counts()
    test_config_lines_and_table_values_are_protected()
    test_missing_governed_artifact_is_loud()
    test_flatten_string_values_is_closed_world()
    print("OK: config retype verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
