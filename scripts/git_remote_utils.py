"""Shared Git remote/path normalization helpers for verifier scripts."""

from __future__ import annotations

import base64
import os
import pathlib
import re
import urllib.parse
from collections.abc import Mapping


REMOTE_URL_SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")
REMOTE_URL_SCP_RE = re.compile(r"^(?:[^/@\\]+@)?[^:/\\]+:.+$")


def is_direct_remote_url(remote_url: str) -> bool:
    return bool(REMOTE_URL_SCHEME_RE.match(remote_url) or REMOTE_URL_SCP_RE.match(remote_url))


def fetchable_remote_url(remote_url: str, source_repo: pathlib.Path) -> str:
    if (
        pathlib.Path(remote_url).is_absolute()
        or remote_url.startswith("~")
        or is_direct_remote_url(remote_url)
    ):
        return remote_url
    return str((source_repo / remote_url).resolve(strict=False))


def looks_like_local_path(value: str) -> bool:
    return (
        pathlib.Path(value).is_absolute()
        or value.startswith(("~", ".", "/", "\\"))
        or value.endswith(".git")
        or "/" in value
        or "\\" in value
    )


def fetchable_origin_argument(origin: str, source_repo: pathlib.Path) -> str:
    if is_direct_remote_url(origin) or looks_like_local_path(origin):
        return fetchable_remote_url(origin, source_repo)
    return origin


def github_repository_slug(remote_url: str) -> str | None:
    value = remote_url.strip()
    path: str | None = None

    if REMOTE_URL_SCHEME_RE.match(value):
        parsed = urllib.parse.urlparse(value)
        if parsed.hostname and parsed.hostname.lower() == "github.com":
            path = parsed.path
    elif REMOTE_URL_SCP_RE.match(value):
        host_path = value.rsplit("@", 1)[-1]
        host, _, scp_path = host_path.partition(":")
        if host.lower() == "github.com":
            path = scp_path

    if path is None:
        return None
    normalized = path.strip("/")
    if normalized.endswith(".git"):
        normalized = normalized[:-4]
    parts = normalized.split("/")
    if len(parts) != 2 or not all(parts):
        return None
    return f"{parts[0]}/{parts[1]}"


def github_actions_git_auth_env(
    remote_url: str,
    environ: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source_env = os.environ if environ is None else environ
    token = source_env.get("GITHUB_TOKEN", "")
    repository = source_env.get("GITHUB_REPOSITORY", "").removesuffix(".git")
    if not token or not repository:
        return {}
    remote_repository = github_repository_slug(remote_url)
    if remote_repository is None or remote_repository.lower() != repository.lower():
        return {}

    try:
        config_index = int(source_env.get("GIT_CONFIG_COUNT", "0"))
    except ValueError:
        config_index = 0
    if config_index < 0:
        config_index = 0

    credential = base64.b64encode(f"x-access-token:{token}".encode("utf-8")).decode("ascii")
    return {
        "GIT_CONFIG_COUNT": str(config_index + 1),
        f"GIT_CONFIG_KEY_{config_index}": "http.https://github.com/.extraheader",
        f"GIT_CONFIG_VALUE_{config_index}": f"AUTHORIZATION: basic {credential}",
    }
