"""Shared Git remote/path normalization helpers for verifier scripts."""

from __future__ import annotations

import pathlib
import re


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
