#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import tomllib
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
CARGO_BIN_EXE_RE = re.compile(r"CARGO_BIN_EXE_([A-Za-z0-9_]+)")
RUST_TEST_ATTR_RE = re.compile(
    r"(?m)^\s*#\s*\[\s*(?:test|tokio::test|async_std::test|rstest)(?:\s*\(|\s*\])"
)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def package_name(crate: Path, cargo: dict[str, object]) -> str:
    package = cargo.get("package")
    if not isinstance(package, dict):
        raise SystemExit(f"{crate / 'Cargo.toml'} missing [package]")
    name = package.get("name")
    if not isinstance(name, str) or not name:
        raise SystemExit(f"{crate / 'Cargo.toml'} missing package.name")
    return name


def cargo_toml(crate: Path) -> dict[str, object]:
    manifest = crate / "Cargo.toml"
    try:
        return tomllib.loads(read_text(manifest))
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{manifest} is invalid TOML: {exc}") from exc


def table_array(cargo: dict[str, object], key: str) -> list[dict[str, object]]:
    value = cargo.get(key, [])
    if not isinstance(value, list):
        raise SystemExit(f"Cargo.toml [{key}] entries must be tables")
    tables: list[dict[str, object]] = []
    for item in value:
        if not isinstance(item, dict):
            raise SystemExit(f"Cargo.toml [{key}] entries must be tables")
        tables.append(item)
    return tables


def explicit_test_targets(cargo: dict[str, object]) -> list[str]:
    names: list[str] = []
    for entry in table_array(cargo, "test"):
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise SystemExit("Cargo.toml [[test]] entries must define name")
        names.append(name)
    return sorted(set(names))


def auto_discovery_enabled(cargo: dict[str, object], key: str) -> bool:
    package = cargo.get("package")
    if not isinstance(package, dict):
        return True
    value = package.get(key)
    return value is not False


def conventional_test_targets(crate: Path) -> list[str]:
    tests_dir = crate / "tests"
    if not tests_dir.exists():
        return []
    names: set[str] = set()
    for source in tests_dir.glob("*.rs"):
        names.add(source.stem)
    for main_rs in tests_dir.glob("*/main.rs"):
        names.add(main_rs.parent.name)
    return sorted(names)


def test_targets(crate: Path, cargo: dict[str, object]) -> list[str]:
    names = set(explicit_test_targets(cargo))
    if auto_discovery_enabled(cargo, "autotests"):
        names.update(conventional_test_targets(crate))
    return sorted(names)


def explicit_bin_targets(crate: Path, cargo: dict[str, object]) -> dict[str, Path]:
    package = package_name(crate, cargo)
    targets: dict[str, Path] = {}
    for entry in table_array(cargo, "bin"):
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise SystemExit("Cargo.toml [[bin]] entries must define name")
        path = entry.get("path")
        if isinstance(path, str) and path:
            targets[name] = crate / path
        else:
            targets[name] = default_bin_path(crate, package, name)
    return targets


def default_bin_path(crate: Path, package: str, name: str) -> Path:
    candidates: list[Path] = []
    if name == package:
        candidates.append(crate / "src" / "main.rs")
    candidates.extend(
        [
            crate / "src" / "bin" / f"{name}.rs",
            crate / "src" / "bin" / name / "main.rs",
        ]
    )
    for candidate in candidates:
        if candidate.exists():
            return candidate
    raise SystemExit(f"Cargo.toml [[bin]] {name} omitted path and no conventional source exists")


def convention_bin_targets(crate: Path, package: str) -> dict[str, Path]:
    targets: dict[str, Path] = {}
    main = crate / "src" / "main.rs"
    if main.exists():
        targets[package] = main
    bin_dir = crate / "src" / "bin"
    if not bin_dir.exists():
        return targets
    for source in sorted(bin_dir.glob("*.rs")):
        targets[source.stem] = source
    for main_rs in sorted(bin_dir.glob("*/main.rs")):
        targets[main_rs.parent.name] = main_rs
    return targets


def bin_targets(crate: Path, cargo: dict[str, object]) -> dict[str, Path]:
    package = package_name(crate, cargo)
    targets = convention_bin_targets(crate, package) if auto_discovery_enabled(cargo, "autobins") else {}
    targets.update(explicit_bin_targets(crate, cargo))
    return targets


def has_rust_test_attr(path: Path) -> bool:
    if not path.exists():
        raise SystemExit(f"declared target source does not exist: {path}")
    return bool(RUST_TEST_ATTR_RE.search(read_text(path)))


def lib_target_enabled(crate: Path, cargo: dict[str, object]) -> bool:
    if isinstance(cargo.get("lib"), dict):
        return True
    return auto_discovery_enabled(cargo, "autolib") and (crate / "src" / "lib.rs").exists()


def archive_args(crate: Path) -> list[str]:
    cargo = cargo_toml(crate)
    args = ["--lib"] if lib_target_enabled(crate, cargo) else []
    for test in test_targets(crate, cargo):
        args.extend(["--test", test])
    for name, path in sorted(bin_targets(crate, cargo).items()):
        if has_rust_test_attr(path):
            args.extend(["--bin", name])
    return args


def sidecars(crate: Path) -> list[str]:
    tests_dir = crate / "tests"
    names: set[str] = set()
    for path in sorted(tests_dir.rglob("*.rs")):
        names.update(CARGO_BIN_EXE_RE.findall(read_text(path)))
    return sorted(names)


def print_lines(lines: Iterable[str]) -> None:
    for line in lines:
        print(line)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Discover Rust CI test archive targets and sidecars.")
    parser.add_argument("mode", choices=("archive-args", "sidecars"))
    parser.add_argument("--crate", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    crate = args.crate.resolve()
    if args.mode == "archive-args":
        print_lines(archive_args(crate))
    elif args.mode == "sidecars":
        print_lines(sidecars(crate))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
