#!/usr/bin/env python3
"""Assert every pinned NautilusTrader revision is merged upstream.

`build.rs` already asserts that the governed capability manifest revision equals
the `nautilus-binance` pin and that the dependency URL is the official
repository. Neither assertion can prove the revision is *merged*: GitHub serves
fork-only commits from the upstream repository URL through the shared fork
network, so a pin to unmerged fork work satisfies both checks and builds green.
That is not hypothetical -- it is the drift this lane exists to catch.

`build.rs` cannot make the check itself, because proving mergedness needs the
network and builds must stay hermetic. So it lives here, and it covers every
`nautilus-*` declaration in every manifest rather than the single dependency
`build.rs` binds.

Run locally from the repository root, or pass the root as the one argument.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

OFFICIAL_REPOSITORY = "https://github.com/nautechsystems/nautilus_trader.git"
OFFICIAL_API_REPO = "nautechsystems/nautilus_trader"

MANIFESTS = (
    Path("Cargo.toml"),
    Path("crates/backtesting-vertical-slice/Cargo.toml"),
)
CAPABILITY_MANIFEST = Path("ci/nautilus-source-capabilities.toml")

DEPENDENCY = re.compile(
    r'^(?P<name>nautilus-[a-z0-9-]+)\s*=\s*\{[^}]*?'
    r'git\s*=\s*"(?P<url>[^"]+)"[^}]*?'
    r'rev\s*=\s*"(?P<rev>[0-9a-f]{40})"',
    re.MULTILINE,
)
CAPABILITY_REVISION = re.compile(
    r'^revision\s*=\s*"(?P<rev>[0-9a-f]{40})"', re.MULTILINE
)


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    print(message, file=sys.stderr)
    sys.exit(1)


def api(path: str) -> dict:
    request = urllib.request.Request(
        f"https://api.github.com/{path}",
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "bolt-v2-nautilus-pin-check",
        },
    )
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", "replace")[:400]
        fail(f"GitHub API {path} failed: HTTP {error.code} {body}")
    except urllib.error.URLError as error:
        fail(f"GitHub API {path} unreachable: {error.reason}")
    raise AssertionError("unreachable")


def collect_pins() -> dict[str, list[str]]:
    """Map revision -> the declarations pinning it, failing on a foreign URL."""
    pins: dict[str, list[str]] = {}
    for manifest in MANIFESTS:
        if not manifest.is_file():
            fail(f"{manifest}: expected manifest is missing")
        text = manifest.read_text(encoding="utf-8")
        found = list(DEPENDENCY.finditer(text))
        if not found:
            fail(f"{manifest}: no nautilus-* git dependency found; the pin check would be vacuous")
        for match in found:
            owner = f"{manifest}:{match.group('name')}"
            if match.group("url") != OFFICIAL_REPOSITORY:
                fail(
                    f"{owner} must use the official repository {OFFICIAL_REPOSITORY}, "
                    f"got {match.group('url')}"
                )
            pins.setdefault(match.group("rev"), []).append(owner)

    if not CAPABILITY_MANIFEST.is_file():
        fail(f"{CAPABILITY_MANIFEST}: expected capability manifest is missing")
    capability = CAPABILITY_REVISION.search(
        CAPABILITY_MANIFEST.read_text(encoding="utf-8")
    )
    if capability is None:
        fail(f"{CAPABILITY_MANIFEST}: no `revision` declaration found")
    pins.setdefault(capability.group("rev"), []).append(f"{CAPABILITY_MANIFEST}:revision")
    return pins


def assert_merged(revision: str, default_branch: str) -> None:
    """A revision is merged only if the default branch is behind it by nothing."""
    comparison = api(f"repos/{OFFICIAL_API_REPO}/compare/{revision}...{default_branch}")
    behind_by = comparison.get("behind_by")
    status = comparison.get("status")
    if behind_by is None:
        fail(f"{revision}: GitHub comparison returned no `behind_by` field")
    if behind_by != 0:
        fail(
            f"{revision} is NOT merged into {OFFICIAL_API_REPO}@{default_branch}: "
            f"status={status}, behind_by={behind_by}. The default branch lacks "
            f"{behind_by} commit(s) reachable from this revision, so the pin points at "
            "fork or otherwise unmerged work. GitHub serves fork-only commits from the "
            "official repository URL, which is why the URL and revision assertions in "
            "build.rs pass for a pin like this. Pin a merged commit instead: use "
            "`git merge-base <pin> upstream/" + default_branch + "` to find the "
            "upstream commit the work branched from."
        )
    print(f"  {revision}: merged (status={status}, behind_by=0)")


def main() -> None:
    if len(sys.argv) > 2:
        fail(f"usage: {Path(sys.argv[0]).name} [repository-root]")
    if len(sys.argv) == 2:
        root = Path(sys.argv[1])
        if not root.is_dir():
            fail(f"{root}: not a directory")
        os.chdir(root)

    pins = collect_pins()
    total = sum(len(owners) for owners in pins.values())
    print(f"Checking {total} pinned NautilusTrader declaration(s) across {len(pins)} revision(s).")

    if len(pins) != 1:
        detail = "\n".join(
            f"  {revision}: {', '.join(sorted(owners))}" for revision, owners in sorted(pins.items())
        )
        fail(
            "every NautilusTrader declaration must pin one revision; found "
            f"{len(pins)}:\n{detail}"
        )

    default_branch = api(f"repos/{OFFICIAL_API_REPO}").get("default_branch")
    if not default_branch:
        fail(f"could not resolve the default branch of {OFFICIAL_API_REPO}")

    for revision in pins:
        assert_merged(revision, default_branch)

    print(f"All {total} declaration(s) pin one revision, merged into {default_branch}.")


if __name__ == "__main__":
    main()
