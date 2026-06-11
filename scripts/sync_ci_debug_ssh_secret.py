#!/usr/bin/env python3
"""Bootstrap and sync the CI runner debug SSH key from 1Password to GitHub Actions."""

from __future__ import annotations

import argparse
import pathlib
import shutil
import subprocess
import sys
import tomllib

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_RUNNERS_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
SSH_PUBLIC_KEY_PREFIXES = (
    "ssh-ed25519 ",
    "ssh-rsa ",
    "ecdsa-sha2-",
    "sk-ssh-ed25519@openssh.com ",
)


def load_ci_runner_debug_config(path: pathlib.Path = DEFAULT_RUNNERS_CONFIG) -> dict[str, str]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    section = data.get("ci_runner_debug")
    if not isinstance(section, dict):
        raise ValueError("ci/github-actions-runners.toml must define [ci_runner_debug]")
    required = (
        "ssh_public_key_secret",
        "onepassword_vault",
        "onepassword_item_title",
        "onepassword_public_key_field",
    )
    config: dict[str, str] = {}
    for key in required:
        value = section.get(key)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"ci_runner_debug.{key} must be a non-empty string")
        config[key] = value.strip()
    return config


def require_command(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise RuntimeError(f"{name} is required but not found on PATH")
    return path


def run_checked(command: list[str], *, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        check=True,
        text=True,
        capture_output=True,
        input=input_text,
    )


def called_process_error_message(exc: subprocess.CalledProcessError) -> str:
    detail = (exc.stderr or exc.stdout or "").strip()
    if detail:
        return detail
    if isinstance(exc.cmd, (list, tuple)) and exc.cmd:
        command_name = pathlib.Path(str(exc.cmd[0])).name
    elif isinstance(exc.cmd, str) and exc.cmd.strip():
        command_name = pathlib.Path(exc.cmd.strip().split()[0]).name
    else:
        command_name = "command"
    return f"{command_name} failed with exit {exc.returncode}"


def github_repository() -> str:
    result = run_checked(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])
    repo = result.stdout.strip()
    if not repo or "/" not in repo:
        raise RuntimeError("unable to resolve GitHub repository from gh repo view")
    return repo


def onepassword_item_exists(config: dict[str, str]) -> bool:
    result = subprocess.run(
        [
            "op",
            "item",
            "get",
            config["onepassword_item_title"],
            "--vault",
            config["onepassword_vault"],
            "--format",
            "json",
        ],
        check=False,
        text=True,
        capture_output=True,
    )
    return result.returncode == 0


def read_onepassword_field(config: dict[str, str], field: str) -> str:
    result = run_checked(
        [
            "op",
            "item",
            "get",
            config["onepassword_item_title"],
            "--vault",
            config["onepassword_vault"],
            "--fields",
            f"label={field}",
        ]
    )
    value = result.stdout.strip()
    if not value:
        raise RuntimeError(
            f"1Password item {config['onepassword_item_title']!r} is missing field {field!r}"
        )
    return value


def validate_public_key(public_key: str) -> None:
    if not any(public_key.startswith(prefix) for prefix in SSH_PUBLIC_KEY_PREFIXES):
        raise RuntimeError("1Password public key does not look like an SSH public key")


def sync_public_key_to_github(config: dict[str, str]) -> None:
    public_key = read_onepassword_field(config, config["onepassword_public_key_field"])
    validate_public_key(public_key)
    repo = github_repository()
    run_checked(
        [
            "gh",
            "secret",
            "set",
            config["ssh_public_key_secret"],
            "--repo",
            repo,
            "--body",
            public_key,
        ]
    )
    print(
        f"OK: synced {config['ssh_public_key_secret']} to {repo} "
        f"from 1Password item {config['onepassword_item_title']!r}"
    )


def bootstrap_onepassword_item(config: dict[str, str]) -> None:
    if onepassword_item_exists(config):
        raise RuntimeError(
            "1Password item already exists; use sync instead of bootstrap: "
            f"{config['onepassword_item_title']!r}"
        )

    run_checked(
        [
            "op",
            "item",
            "create",
            "--category",
            "SSH Key",
            "--title",
            config["onepassword_item_title"],
            "--vault",
            config["onepassword_vault"],
            "--ssh-generate-key=ed25519",
        ]
    )

    print(
        "OK: created 1Password SSH key item "
        f"{config['onepassword_item_title']!r} in vault {config['onepassword_vault']!r}"
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=("bootstrap", "sync"),
        help="bootstrap creates the 1Password item; sync publishes the public key to GitHub",
    )
    parser.add_argument(
        "--config",
        type=pathlib.Path,
        default=DEFAULT_RUNNERS_CONFIG,
        help="Path to ci/github-actions-runners.toml",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    require_command("op")
    require_command("gh")

    try:
        config = load_ci_runner_debug_config(args.config)
    except (ValueError, tomllib.TOMLDecodeError, OSError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    try:
        if args.command == "bootstrap":
            bootstrap_onepassword_item(config)
        sync_public_key_to_github(config)
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        if isinstance(exc, subprocess.CalledProcessError):
            message = called_process_error_message(exc)
        else:
            message = str(exc)
        print(f"ERROR: {message}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
