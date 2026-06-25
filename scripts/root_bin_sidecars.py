#!/usr/bin/env python3
"""Pack root Cargo binary sidecars for archived integration tests."""

from __future__ import annotations

import argparse
import os
import pathlib
import sys
import tarfile
import tomllib


class SidecarError(Exception):
    """Raised when root binary sidecar production must fail closed."""


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SidecarError(f"missing Cargo manifest: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise SidecarError(f"invalid Cargo manifest: {path}: {exc}") from exc


def require_package_name(cargo: dict[str, object]) -> str:
    package = cargo.get("package")
    if not isinstance(package, dict):
        raise SidecarError("Cargo.toml must contain [package]")
    name = package.get("name")
    if not isinstance(name, str) or not name:
        raise SidecarError("Cargo.toml package.name must be a non-empty string")
    return name


def explicit_bin_names(cargo: dict[str, object]) -> set[str]:
    raw_bins = cargo.get("bin", [])
    if raw_bins is None:
        return set()
    if not isinstance(raw_bins, list):
        raise SidecarError("Cargo.toml [[bin]] entries must be an array")
    names: set[str] = set()
    for index, raw_bin in enumerate(raw_bins, start=1):
        if not isinstance(raw_bin, dict):
            raise SidecarError(f"Cargo.toml [[bin]] entry {index} must be a table")
        name = raw_bin.get("name")
        if not isinstance(name, str) or not name:
            raise SidecarError(f"Cargo.toml [[bin]] entry {index} must have a non-empty name")
        names.add(name)
    return names


def implicit_src_bin_names(repo_root: pathlib.Path) -> set[str]:
    src_bin = repo_root / "src" / "bin"
    if not src_bin.is_dir():
        return set()
    names: set[str] = set()
    for child in src_bin.iterdir():
        if child.is_file() and child.suffix == ".rs":
            names.add(child.stem)
        elif child.is_dir() and (child / "main.rs").is_file():
            names.add(child.name)
    return names


def expected_bin_names(repo_root: pathlib.Path) -> tuple[str, ...]:
    cargo = load_toml(repo_root / "Cargo.toml")
    names = explicit_bin_names(cargo) | implicit_src_bin_names(repo_root)
    if (repo_root / "src" / "main.rs").is_file():
        names.add(require_package_name(cargo))
    if not names:
        raise SidecarError("no root binary targets found")
    return tuple(sorted(names))


def relative_sidecar_paths(repo_root: pathlib.Path) -> tuple[pathlib.Path, ...]:
    return tuple(pathlib.Path("debug") / name for name in expected_bin_names(repo_root))


def validate_sidecars(repo_root: pathlib.Path, target_dir: pathlib.Path) -> tuple[pathlib.Path, ...]:
    missing: list[str] = []
    sidecars: list[pathlib.Path] = []
    for relative_path in relative_sidecar_paths(repo_root):
        path = target_dir / relative_path
        if not path.is_file() or not os.access(path, os.X_OK):
            missing.append(relative_path.as_posix())
        else:
            sidecars.append(relative_path)
    if missing:
        raise SidecarError(f"missing root binary sidecars: {', '.join(missing)}")
    return tuple(sidecars)


def pack_sidecars(repo_root: pathlib.Path, target_dir: pathlib.Path, output: pathlib.Path) -> None:
    sidecars = validate_sidecars(repo_root, target_dir)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, "w:gz") as archive:
        for relative_path in sidecars:
            archive.add(target_dir / relative_path, arcname=relative_path.as_posix())


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    expected = subparsers.add_parser("expected", help="print expected root binary names")
    expected.add_argument("--repo-root", required=True)

    pack = subparsers.add_parser("pack", help="pack expected root binaries from a target dir")
    pack.add_argument("--repo-root", required=True)
    pack.add_argument("--target-dir", required=True)
    pack.add_argument("--output", required=True)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    repo_root = pathlib.Path(args.repo_root).resolve()
    try:
        if args.command == "expected":
            for name in expected_bin_names(repo_root):
                print(name)
        elif args.command == "pack":
            pack_sidecars(
                repo_root,
                pathlib.Path(args.target_dir).resolve(),
                pathlib.Path(args.output).resolve(),
            )
        else:
            raise SidecarError(f"unsupported command: {args.command}")
    except SidecarError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
