#!/usr/bin/env python3
"""Verify deterministic configuration evidence for redemption preparation."""

from __future__ import annotations

import pathlib
import sys
import tomllib
from collections.abc import Iterator, Mapping

import generate_polymarket_redemption_config as generator


OWNER = pathlib.Path("src/bolt_v3_polymarket_redemption.rs")
GENERATED = pathlib.Path("src/bolt_v3_polymarket_redemption/generated.rs")
RUNTIME = pathlib.Path("config/polymarket-redemption.toml")
ROOT_RUNTIME = pathlib.Path("config/root.toml")
EVIDENCE = pathlib.Path("config/polymarket-redemption-source-evidence.toml")
COMPILE_TEST = pathlib.Path("tests/polymarket_redemption_preparation.rs")
COMPILE_FAIL = pathlib.Path("tests/polymarket_redemption_preparation_compile_fail.rs")
EXPECTED_RUNTIME_AUTHORITY_PATHS = {
    "standard_adapter_target": ("redemption", "standard_adapter_target"),
    "negative_risk_adapter_target": ("redemption", "negative_risk_adapter_target"),
}
ROOT_OWNED_WALLET_FIELDS = frozenset(
    {"aws_region", "safe_address", "signer_private_key_ssm_path"}
)


def _read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def _toml(path: pathlib.Path) -> dict[str, object]:
    return tomllib.loads(_read(path))


def _repository_toml(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not ignored.intersection(path.relative_to(root).parts)
    )


def _key_locations(
    value: object,
    prefix: tuple[str, ...] = (),
) -> Iterator[tuple[str, tuple[str, ...]]]:
    if isinstance(value, Mapping):
        for key, child in value.items():
            path = (*prefix, key)
            yield key, path
            yield from _key_locations(child, path)
        return
    if isinstance(value, list):
        for index, child in enumerate(value):
            yield from _key_locations(child, (*prefix, f"[{index}]"))


def _manifest_errors(cargo: Mapping[str, object]) -> list[str]:
    errors: list[str] = []
    dependencies = cargo.get("dependencies")
    if not isinstance(dependencies, Mapping):
        return ["Cargo.toml dependencies must be a table"]
    for dependency in ("alloy-signer", "alloy-signer-local"):
        if dependencies.get(dependency) != "=2.1.1":
            errors.append(
                f"direct signer dependency must remain exact and locked: {dependency} = =2.1.1"
            )

    tests = cargo.get("test")
    expected_target = {
        "name": "polymarket_redemption_preparation",
        "path": str(COMPILE_TEST),
    }
    if not isinstance(tests, list) or expected_target not in tests:
        errors.append("compile-fail test target is not wired")
    return errors


def boundary_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    required = [
        OWNER,
        GENERATED,
        RUNTIME,
        ROOT_RUNTIME,
        EVIDENCE,
        COMPILE_TEST,
        COMPILE_FAIL,
        pathlib.Path("Cargo.toml"),
    ]
    missing = [str(path) for path in required if not (root / path).is_file()]
    if missing:
        return [f"missing required redemption preparation artifact(s): {missing}"]

    try:
        runtime = _toml(root / RUNTIME)
        cargo = _toml(root / "Cargo.toml")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        return [f"cannot inspect redemption preparation artifacts: {error}"]

    authorities: dict[str, list[tuple[pathlib.Path, tuple[str, ...]]]] = {
        key: [] for key in EXPECTED_RUNTIME_AUTHORITY_PATHS
    }
    for path in _repository_toml(root):
        try:
            parsed = _toml(path)
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect TOML authority {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key, key_path in _key_locations(parsed):
            if key in authorities:
                authorities[key].append((relative, key_path))

    for key, expected_path in EXPECTED_RUNTIME_AUTHORITY_PATHS.items():
        expected = [(RUNTIME, expected_path)]
        if authorities[key] != expected:
            errors.append(
                f"runtime field {key} must have one parsed TOML authority at "
                f"{RUNTIME}:{'.'.join(expected_path)}; found {authorities[key]}"
            )

    runtime_wallet_duplicates = sorted(
        key
        for key, _ in _key_locations(runtime)
        if key in ROOT_OWNED_WALLET_FIELDS
    )
    if runtime_wallet_duplicates:
        errors.append(
            "redemption wallet and signer fields must remain single-sourced from config/root.toml: "
            f"{runtime_wallet_duplicates}"
        )

    try:
        generator.check_generated_projection(
            root / RUNTIME,
            root / EVIDENCE,
            root / ROOT_RUNTIME,
            root / GENERATED,
        )
    except (generator.ConfigError, OSError, UnicodeDecodeError) as error:
        errors.append(f"redemption configuration evidence is invalid: {error}")

    errors.extend(_manifest_errors(cargo))
    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = boundary_errors(root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print("polymarket redemption preparation boundary: deterministic evidence verified")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
