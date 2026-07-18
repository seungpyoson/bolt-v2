"""Shared Git remote/path normalization helpers for verifier scripts."""

from __future__ import annotations

import base64
import os
import pathlib
import re
import urllib.parse
from collections.abc import Mapping, Sequence


REMOTE_URL_SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")
REMOTE_URL_SCP_RE = re.compile(r"^(?:[^/@\\]+@)?[^:/\\]+:.+$")
REMOTE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
GITHUB_REPOSITORY_RE = re.compile(
    r"^github\.com/[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$"
)


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


def redact_remote_urls(text: str, remote_urls: Sequence[str]) -> str:
    redacted = text
    for remote_url in remote_urls:
        if remote_url:
            redacted = redacted.replace(remote_url, "<remote-url>")
    return redacted


def require_remote_name(
    remote_name: str,
    *,
    error_cls: type[Exception] = ValueError,
) -> str:
    if REMOTE_NAME_RE.fullmatch(remote_name) is None:
        raise error_cls("configured Git origin must be a remote name")
    return remote_name


def github_https_remote_url(
    repository: str,
    *,
    error_cls: type[Exception] = ValueError,
) -> str:
    if GITHUB_REPOSITORY_RE.fullmatch(repository) is None:
        raise error_cls("configured repository must identify one github.com repository")
    return f"https://{repository}.git"


def isolated_git_transport_environment(
    environ: Mapping[str, str],
) -> dict[str, str]:
    environment = dict(environ)
    for key in tuple(environment):
        if key.startswith("GIT_"):
            environment.pop(key)
    environment["GIT_CONFIG_NOSYSTEM"] = "1"
    environment["GIT_CONFIG_GLOBAL"] = os.devnull
    environment["GIT_CONFIG_COUNT"] = "1"
    environment["GIT_CONFIG_KEY_0"] = "credential.https://github.com.helper"
    environment["GIT_CONFIG_VALUE_0"] = "!gh auth git-credential"
    return environment


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
    """Return GitHub extraheader env for matching repo remotes.

    Outside GitHub Actions, missing ambient GitHub identity intentionally returns
    an empty env so local runs can use the operator's configured Git auth.
    Inside GitHub Actions, the workflow must provide the ephemeral token and
    repository slug explicitly; otherwise fetches fail closed before falling
    back to unauthenticated GitHub access.
    """

    source_env = os.environ if environ is None else environ
    token = source_env.get("GITHUB_TOKEN", "")
    repository = source_env.get("GITHUB_REPOSITORY", "").removesuffix(".git")
    if not token or not repository:
        if source_env.get("GITHUB_ACTIONS") == "true":
            raise RuntimeError("GITHUB_TOKEN and GITHUB_REPOSITORY are both required in GitHub Actions")
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
