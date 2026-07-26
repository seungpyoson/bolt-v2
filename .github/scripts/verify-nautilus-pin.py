#!/usr/bin/env python3
"""Assert every pinned NautilusTrader dependency is merged upstream.

`build.rs` already asserts that the governed capability manifest revision equals
the `nautilus-binance` pin and that the dependency URL is the official
repository. Neither assertion can prove the revision is *merged*: GitHub serves
fork-only commits from the upstream repository URL through the shared fork
network, so a pin to unmerged fork work satisfies both checks and builds green.
That is not hypothetical -- it is the drift this lane exists to catch.

`build.rs` cannot make the check itself, because proving mergedness needs the
network and builds must stay hermetic. So it lives here, and it covers every
NautilusTrader declaration in every manifest rather than the single dependency
`build.rs` binds.

Manifests are parsed structurally with `tomllib`, never by pattern matching. An
earlier regex-based version of this script was bypassed two ways: writing the
inline table as `{ rev = "...", git = "..." }` (valid TOML, any key order) made
the declaration invisible, and a `branch`/`tag` pin has no `rev` at all, so both
the URL and merged assertions were skipped. A declaration this script cannot
interpret is a failure, never a silent skip.

Run from the repository root, or pass the root as the one argument.
"""

from __future__ import annotations

import json
import os
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

OFFICIAL_REPOSITORY = "https://github.com/nautechsystems/nautilus_trader.git"
OFFICIAL_API_REPO = "nautechsystems/nautilus_trader"
DEPENDENCY_PREFIX = "nautilus-"

MANIFESTS = (
    Path("Cargo.toml"),
    Path("crates/backtesting-vertical-slice/Cargo.toml"),
)
CAPABILITY_MANIFEST = Path("ci/nautilus-source-capabilities.toml")

# Every table a cargo manifest can declare dependencies in. `target.*` and
# `workspace` are walked explicitly below.
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


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


def is_nautilus(name: str, spec: object) -> bool:
    """A declaration is NautilusTrader's if its name or its rename target is."""
    if name.startswith(DEPENDENCY_PREFIX):
        return True
    if isinstance(spec, dict):
        renamed = spec.get("package")
        if isinstance(renamed, str) and renamed.startswith(DEPENDENCY_PREFIX):
            return True
    return False


def dependency_tables(document: dict, owner: str):
    """Yield (owner, table) for every dependency table a manifest can hold."""
    for key in DEPENDENCY_TABLES:
        table = document.get(key)
        if isinstance(table, dict):
            yield f"{owner}{key}", table

    workspace = document.get("workspace")
    if isinstance(workspace, dict):
        yield from dependency_tables(workspace, f"{owner}workspace.")

    targets = document.get("target")
    if isinstance(targets, dict):
        for triple, table in targets.items():
            if isinstance(table, dict):
                yield from dependency_tables(table, f"{owner}target.{triple}.")


def collect_pins() -> dict[str, list[str]]:
    """Map revision -> declarations pinning it, failing on anything unpinnable."""
    pins: dict[str, list[str]] = {}
    for manifest in MANIFESTS:
        if not manifest.is_file():
            fail(f"{manifest}: expected manifest is missing")
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))

        seen = 0
        for table_owner, table in dependency_tables(document, ""):
            for name, spec in table.items():
                if not is_nautilus(name, spec):
                    continue
                seen += 1
                owner = f"{manifest}:{table_owner}.{name}"

                if not isinstance(spec, dict):
                    fail(
                        f"{owner} must be an inline table pinning the official "
                        f"repository by revision, got {spec!r}"
                    )
                git = spec.get("git")
                if git is None:
                    fail(
                        f"{owner} has no `git` key. Every NautilusTrader dependency must "
                        "come from the official repository at an exact revision."
                    )
                if git != OFFICIAL_REPOSITORY:
                    fail(
                        f"{owner} must use the official repository "
                        f"{OFFICIAL_REPOSITORY}, got {git}"
                    )
                for mutable in ("branch", "tag"):
                    if mutable in spec:
                        fail(
                            f"{owner} pins `{mutable} = {spec[mutable]!r}`, which is mutable "
                            "and cannot be proven merged. Pin an exact `rev` instead."
                        )
                revision = spec.get("rev")
                if not isinstance(revision, str) or len(revision) != 40:
                    fail(
                        f"{owner} must pin a full 40-character `rev`, got {revision!r}"
                    )
                pins.setdefault(revision, []).append(owner)

        if seen == 0:
            fail(
                f"{manifest}: no NautilusTrader dependency found; the pin check would "
                "be vacuous. If the dependency was removed, remove this manifest from "
                "MANIFESTS as well."
            )

    if not CAPABILITY_MANIFEST.is_file():
        fail(f"{CAPABILITY_MANIFEST}: expected capability manifest is missing")
    capability = tomllib.loads(CAPABILITY_MANIFEST.read_text(encoding="utf-8"))
    revision = capability.get("revision")
    if not isinstance(revision, str) or len(revision) != 40:
        fail(
            f"{CAPABILITY_MANIFEST}: `revision` must be a full 40-character commit, "
            f"got {revision!r}"
        )
    pins.setdefault(revision, []).append(f"{CAPABILITY_MANIFEST}:revision")
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
            f"build.rs pass for a pin like this. Use `git merge-base <pin> "
            f"upstream/{default_branch}` to find the upstream commit it branched from."
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
    print(f"Checking {total} pinned NautilusTrader item(s) across {len(pins)} revision(s).")

    if len(pins) != 1:
        detail = "\n".join(
            f"  {revision}: {', '.join(sorted(owners))}"
            for revision, owners in sorted(pins.items())
        )
        fail(
            "every NautilusTrader declaration and the capability manifest must pin one "
            f"revision; found {len(pins)}:\n{detail}"
        )

    default_branch = api(f"repos/{OFFICIAL_API_REPO}").get("default_branch")
    if not default_branch:
        fail(f"could not resolve the default branch of {OFFICIAL_API_REPO}")

    for revision in pins:
        assert_merged(revision, default_branch)

    print(f"All {total} item(s) pin one revision, merged into {default_branch}.")


if __name__ == "__main__":
    main()
