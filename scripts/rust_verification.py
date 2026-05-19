#!/usr/bin/env python3
"""Repo-local Rust verification owner for bolt-v2."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
from typing import Any

try:
    import tomllib as _toml
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback.
    try:
        import tomli as _toml  # type: ignore[no-redef]
    except ModuleNotFoundError:  # pragma: no cover - exercised by system Python on macOS.
        _toml = None


POLICY_RELATIVE_PATH = pathlib.Path("ci/rust-verification.toml")
MAX_POLICY_BYTES = 1024 * 1024
SAFE_IDENTIFIER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SCRUB_ENV_KEYS = (
    "BOLT_RUST_VERIFICATION_ROOT",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_TARGET_DIR",
    "RUST_VERIFICATION_PRESERVE_ROUTING_ENV",
    "RUST_VERIFICATION_REAL_CARGO",
    "RUST_VERIFICATION_ROOT_BASE",
)


class PolicyError(RuntimeError):
    pass


def check_policy_size(path: pathlib.Path) -> None:
    size = path.stat().st_size
    if size > MAX_POLICY_BYTES:
        raise PolicyError(f"{POLICY_RELATIVE_PATH} exceeds maximum size of {MAX_POLICY_BYTES} bytes")


def parse_minimal_toml(path: pathlib.Path) -> dict[str, Any]:
    check_policy_size(path)
    data: dict[str, Any] = {}
    current: dict[str, Any] = data
    with path.open("r", encoding="utf-8") as handle:
        lines = enumerate(handle, start=1)
        for lineno, raw_line in lines:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                current = data
                for part in line[1:-1].split("."):
                    if not part or not SAFE_IDENTIFIER_RE.match(part):
                        raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported table name")
                    child = current.setdefault(part, {})
                    if not isinstance(child, dict):
                        raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: table conflicts with scalar")
                    current = child
                continue
            key, sep, value_text = line.partition("=")
            if not sep:
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: expected key = value")
            key = key.strip()
            if not SAFE_IDENTIFIER_RE.match(key):
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported key")
            value_text = value_text.strip()
            if value_text.startswith('"') and value_text.endswith('"'):
                try:
                    value: Any = json.loads(value_text)
                except json.JSONDecodeError as exc:
                    raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: invalid string") from exc
            elif value_text.isdigit():
                value = int(value_text)
            else:
                raise PolicyError(f"{POLICY_RELATIVE_PATH}:{lineno}: unsupported value")
            current[key] = value
    return data


def load_toml(path: pathlib.Path) -> dict[str, Any]:
    if _toml is None:
        return parse_minimal_toml(path)
    check_policy_size(path)
    try:
        with path.open("rb") as handle:
            return _toml.load(handle)
    except _toml.TOMLDecodeError as exc:
        raise PolicyError(f"{POLICY_RELATIVE_PATH} is invalid TOML: {exc}") from exc


def repo_path(raw: str) -> pathlib.Path:
    return pathlib.Path(raw).expanduser().absolute()


def policy_path(repo: pathlib.Path) -> pathlib.Path:
    return repo / POLICY_RELATIVE_PATH


def load_policy(repo: pathlib.Path) -> dict[str, Any]:
    path = policy_path(repo)
    if not path.exists():
        raise FileNotFoundError(path)
    data = load_toml(path)
    validate_policy_data(data)
    return data


def validate_policy_data(data: dict[str, Any]) -> None:
    if data.get("schema_version") != 1:
        raise PolicyError("schema_version must be 1")
    project_id = data.get("project_id")
    namespace = data.get("target_namespace")
    if not isinstance(project_id, str) or not SAFE_IDENTIFIER_RE.match(project_id):
        raise PolicyError("project_id must be a safe identifier")
    if not isinstance(namespace, str) or not SAFE_IDENTIFIER_RE.match(namespace):
        raise PolicyError("target_namespace must be a safe identifier")
    commands = data.get("commands")
    if not isinstance(commands, dict):
        raise PolicyError("commands table is required")
    for name in ("test", "clippy", "build"):
        command = commands.get(name)
        if not isinstance(command, dict):
            raise PolicyError(f"commands.{name} table is required")
        recipe = command.get("recipe")
        if not isinstance(recipe, str) or not SAFE_IDENTIFIER_RE.match(recipe):
            raise PolicyError(f"commands.{name}.recipe must be a safe identifier")
    build = commands["build"]
    for key in ("target", "profile"):
        value = build.get(key)
        if not isinstance(value, str) or not SAFE_IDENTIFIER_RE.match(value):
            raise PolicyError(f"commands.build.{key} must be a safe identifier")
    if build.get("artifact_layout") != "cargo":
        raise PolicyError("commands.build.artifact_layout must be 'cargo'")


def status_for_repo(repo: pathlib.Path) -> str:
    if not policy_path(repo).exists():
        return "unmanaged"
    try:
        load_policy(repo)
    except (OSError, PolicyError):
        return "invalid-policy"
    return "managed"


def root_base() -> pathlib.Path:
    raw = os.environ.get("RUST_VERIFICATION_ROOT_BASE")
    if raw:
        return pathlib.Path(raw).expanduser()
    return pathlib.Path.home() / ".cache" / "rust-verification"


def target_dir(repo: pathlib.Path, policy: dict[str, Any] | None = None) -> pathlib.Path:
    data = policy if policy is not None else load_policy(repo)
    namespace = data["target_namespace"]
    return root_base() / namespace / "target"


def managed_env(repo: pathlib.Path, policy: dict[str, Any] | None = None) -> dict[str, str]:
    env = os.environ.copy()
    for key in SCRUB_ENV_KEYS:
        env.pop(key, None)
    env["CARGO_TARGET_DIR"] = str(target_dir(repo, policy))
    env["RUST_VERIFICATION_PRESERVE_ROUTING_ENV"] = "1"
    return env


def command_args(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
    return subprocess.run(argv, cwd=repo, env=env, check=False).returncode


def cmd_repo_status(args: argparse.Namespace) -> int:
    print(status_for_repo(repo_path(args.repo)))
    return 0


def cmd_is_managed(args: argparse.Namespace) -> int:
    status = status_for_repo(repo_path(args.repo))
    if status == "managed":
        return 0
    if status == "invalid-policy":
        return 2
    return 1


def cmd_validate_policy(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
    except FileNotFoundError:
        print(f"missing {POLICY_RELATIVE_PATH}", file=sys.stderr)
        return 2
    except (OSError, PolicyError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    build = policy["commands"]["build"]
    print(
        json.dumps(
            {
                "build_profile": build["profile"],
                "build_target": build["target"],
                "policy": str(policy_path(repo)),
                "project_id": policy["project_id"],
                "status": "ok",
            },
            sort_keys=True,
        )
    )
    return 0


def cmd_target_dir(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        path = target_dir(repo)
        path.mkdir(parents=True, exist_ok=True)
        print(path)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return 0


def cmd_binary_path(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    build = policy["commands"]["build"]
    binary = target_dir(repo, policy) / build["target"] / build["profile"] / args.bin
    print(binary)
    return 0


def cmd_cargo(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    cargo = os.environ.get("RUST_VERIFICATION_REAL_CARGO", "cargo")
    return run_process([cargo, *command_args(args.args)], repo=repo, env=managed_env(repo, policy))


def cmd_run(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    try:
        policy = load_policy(repo)
        command = policy["commands"][args.command]
    except KeyError:
        print(f"unknown managed command: {args.command}", file=sys.stderr)
        return 2
    except (OSError, PolicyError, FileNotFoundError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    justfile = repo / "justfile"
    argv = ["just", "-f", str(justfile), "--working-directory", str(repo), "--", command["recipe"], *args.args]
    return run_process(argv, repo=repo, env=managed_env(repo, policy))


def cmd_scrub_env_keys(_args: argparse.Namespace) -> int:
    for key in SCRUB_ENV_KEYS:
        print(key)
    return 0


def cmd_describe(args: argparse.Namespace) -> int:
    repo = repo_path(args.repo)
    status = status_for_repo(repo)
    payload: dict[str, Any] = {"status": status, "policy": str(policy_path(repo))}
    if status == "managed":
        policy = load_policy(repo)
        payload["target_dir"] = str(target_dir(repo, policy))
        payload["project_id"] = policy["project_id"]
    print(json.dumps(payload, sort_keys=True))
    return 0


def cmd_cleanup(_args: argparse.Namespace) -> int:
    print(json.dumps({"status": "ok", "removed": []}, sort_keys=True))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command_name", required=True)

    repo_status = subparsers.add_parser("repo-status")
    repo_status.add_argument("--repo", required=True)
    repo_status.set_defaults(func=cmd_repo_status)

    is_managed = subparsers.add_parser("is-managed-rust-repo")
    is_managed.add_argument("--repo", required=True)
    is_managed.set_defaults(func=cmd_is_managed)

    validate = subparsers.add_parser("validate-policy")
    validate.add_argument("--repo", required=True)
    validate.set_defaults(func=cmd_validate_policy)

    target = subparsers.add_parser("target-dir")
    target.add_argument("--repo", required=True)
    target.set_defaults(func=cmd_target_dir)

    binary = subparsers.add_parser("binary-path")
    binary.add_argument("--repo", required=True)
    binary.add_argument("--bin", required=True)
    binary.set_defaults(func=cmd_binary_path)

    cargo = subparsers.add_parser("cargo")
    cargo.add_argument("--repo", required=True)
    cargo.add_argument("args", nargs=argparse.REMAINDER)
    cargo.set_defaults(func=cmd_cargo)

    run = subparsers.add_parser("run")
    run.add_argument("--repo", required=True)
    run.add_argument("command", choices=("test", "clippy", "build"))
    run.add_argument("args", nargs=argparse.REMAINDER)
    run.set_defaults(func=cmd_run)

    scrub = subparsers.add_parser("scrub-env-keys")
    scrub.set_defaults(func=cmd_scrub_env_keys)

    describe = subparsers.add_parser("describe")
    describe.add_argument("--repo", required=True)
    describe.set_defaults(func=cmd_describe)

    cleanup = subparsers.add_parser("cleanup")
    cleanup.set_defaults(func=cmd_cleanup)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
