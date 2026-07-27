#!/usr/bin/env python3
"""Assert every NautilusTrader revision this repository can build is merged upstream.

`build.rs` asserts that the governed capability manifest revision equals the
`nautilus-binance` pin and that the dependency URL is the official repository.
Neither proves the revision is *merged*: GitHub serves fork-only commits from
the upstream repository URL through the shared fork network, so a pin to
unmerged fork work satisfies both checks and builds green. That is the drift
this lane exists to catch, and `build.rs` cannot catch it because proving
mergedness needs the network and builds must stay hermetic.

Design note, learned the hard way across three review rounds. Every previous
version of this check enumerated what to inspect and was bypassed by a shape it
did not enumerate: one hard-coded dependency, then a regex that missed reordered
inline-table keys and `branch` pins, then a structural walk that missed `[patch]`
and any manifest outside a hard-coded pair. So this version enumerates nothing:

  * manifests and lockfiles are **discovered**, never listed;
  * lockfiles are read as *resolution truth* -- what cargo settled on;
  * manifests are read for *declared intent*, including the override tables;
  * anything referencing NautilusTrader that cannot be interpreted is a
    failure, never a silent skip.

The two readings are both required, and neither is sufficient. Measured, not
assumed: a `[patch]` pointing at a **git** source appears in the lockfile with
that source, so the lockfile catches it; a `[patch]` pointing at a **path**
produces a lockfile entry with no `source` field at all, so the lockfile cannot
see it and only the manifest reading catches it. Do not drop the manifest
reading on the theory that the lockfile covers overrides -- for path patches it
does not.

Run from the repository root, pass the root as an argument, or pass
`--self-test` to run the bypass-control suite.
"""

from __future__ import annotations

import json
import os
import re
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

OFFICIAL_REPOSITORY = "https://github.com/nautechsystems/nautilus_trader.git"
OFFICIAL_API_REPO = "nautechsystems/nautilus_trader"
# A real unmerged fork revision, used by the online self-test control.
FORK_REVISION = "01d5af1427d73532f6dd9f2be77acb72f825bec9"
NAUTILUS = "nautilus"
CAPABILITY_MANIFEST = Path("ci/nautilus-source-capabilities.toml")
SKIP_DIRS = {"target", ".git", "node_modules", ".worktrees"}

# `git+<url>?rev=<rev>#<resolved>` as Cargo.lock records a git source.
LOCK_SOURCE = re.compile(r"^git\+(?P<url>[^?#]+)(?:\?(?P<query>[^#]*))?(?:#(?P<resolved>.*))?$")


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


def load_toml(path: Path) -> dict:
    """Parse a discovered file, naming it if it cannot be read.

    Discovery means this parses files nobody wrote for it, so an unrelated
    malformed manifest must produce an actionable failure rather than a
    traceback that says nothing about which file or why this lane cares.
    """
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError, OSError) as error:
        fail(
            f"{path}: cannot be parsed, so NautilusTrader references in it cannot be "
            f"checked: {error}"
        )
    raise AssertionError("unreachable")


def discover(filename: str) -> list[Path]:
    """Every tracked `filename` in the tree. Discovery, so a new crate is covered."""
    found: list[Path] = []
    for path in Path(".").rglob(filename):
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        found.append(path)
    return sorted(found)


def mentions_nautilus(name: str, spec: object) -> bool:
    if NAUTILUS in name:
        return True
    if isinstance(spec, dict):
        for key in ("package", "git"):
            value = spec.get(key)
            if isinstance(value, str) and NAUTILUS in value:
                return True
    return False


def dependency_tables(document: dict, owner: str):
    """Yield (owner, table) for every table that can name a dependency source.

    Includes the override tables `[patch.*]` and `[replace]`: those change what
    cargo builds without touching the dependency block, which is precisely why a
    walk that skips them can pass while a fork is compiled.
    """
    for key in ("dependencies", "dev-dependencies", "build-dependencies", "replace"):
        table = document.get(key)
        if isinstance(table, dict):
            yield f"{owner}{key}", table

    for container in ("workspace",):
        nested = document.get(container)
        if isinstance(nested, dict):
            yield from dependency_tables(nested, f"{owner}{container}.")

    for container in ("target", "patch"):
        nested = document.get(container)
        if isinstance(nested, dict):
            for selector, table in nested.items():
                if not isinstance(table, dict):
                    continue
                if container == "patch":
                    yield f"{owner}patch.{selector}", table
                else:
                    yield from dependency_tables(table, f"{owner}target.{selector}.")


def check_declaration(owner: str, spec: object, pins: dict[str, list[str]]) -> None:
    if not isinstance(spec, dict):
        fail(
            f"{owner} references NautilusTrader but is not an inline table pinning the "
            f"official repository by revision, got {spec!r}"
        )
    git = spec.get("git")
    if git is None:
        fail(
            f"{owner} references NautilusTrader with no `git` key. Every NautilusTrader "
            "source must come from the official repository at an exact revision."
        )
    if git != OFFICIAL_REPOSITORY:
        fail(f"{owner} must use the official repository {OFFICIAL_REPOSITORY}, got {git}")
    for mutable in ("branch", "tag"):
        if mutable in spec:
            fail(
                f"{owner} pins `{mutable} = {spec[mutable]!r}`, which is mutable and cannot "
                "be proven merged. Pin an exact `rev` instead."
            )
    revision = spec.get("rev")
    if not isinstance(revision, str) or len(revision) != 40:
        fail(f"{owner} must pin a full 40-character `rev`, got {revision!r}")
    pins.setdefault(revision, []).append(owner)


def collect_from_manifests(pins: dict[str, list[str]]) -> int:
    seen = 0
    for manifest in discover("Cargo.toml"):
        document = load_toml(manifest)
        for table_owner, table in dependency_tables(document, ""):
            for name, spec in table.items():
                if not mentions_nautilus(name, spec):
                    continue
                seen += 1
                check_declaration(f"{manifest}:{table_owner}.{name}", spec, pins)
    return seen


def collect_from_lockfiles(pins: dict[str, list[str]]) -> int:
    """Lockfiles record what cargo resolved.

    A git-sourced override shows up here with its real source. A path-sourced
    one does not appear at all -- such entries carry no `source` field -- which
    is why the manifest reading is not redundant with this one.
    """
    seen = 0
    for lockfile in discover("Cargo.lock"):
        document = load_toml(lockfile)
        for package in document.get("package", []):
            name = package.get("name", "")
            source = package.get("source")
            if not isinstance(source, str) or NAUTILUS not in f"{name}{source}":
                continue
            seen += 1
            owner = f"{lockfile}:{name}"
            match = LOCK_SOURCE.match(source)
            if match is None:
                fail(f"{owner} resolves to a non-git NautilusTrader source: {source}")
            if match.group("url") != OFFICIAL_REPOSITORY:
                fail(
                    f"{owner} resolves to {match.group('url')}, not the official repository "
                    f"{OFFICIAL_REPOSITORY}"
                )
            query = dict(
                pair.split("=", 1)
                for pair in (match.group("query") or "").split("&")
                if "=" in pair
            )
            revision = query.get("rev") or match.group("resolved")
            if not revision or len(revision) != 40:
                fail(f"{owner} does not resolve to a full 40-character revision: {source}")
            pins.setdefault(revision, []).append(owner)
    return seen


def reject_source_overrides() -> int:
    """Refuse cargo config overrides, which redirect a source invisibly.

    Verified by experiment, not assumed: a `paths` override swaps the built code
    while `Cargo.lock` still records the original git revision, so neither the
    manifests nor the lockfiles this script reads would show it. `[source]`
    replacement is the same shape. Both are refused outright rather than
    inspected -- deciding whether a given override happens to shadow
    NautilusTrader means resolving it, and a check that has to resolve overrides
    to stay correct is one more thing to get wrong.

    Limit worth naming: this sees repository files only. `$CARGO_HOME/config.toml`
    and `--config` on the command line are outside the repository and cannot be
    checked here.
    """
    checked = 0
    for name in ("config.toml", "config"):
        for path in discover(name):
            if path.parent.name != ".cargo":
                continue
            checked += 1
            document = load_toml(path)
            for key in ("paths", "source"):
                if key in document:
                    fail(
                        f"{path} declares `{key}`, which redirects a dependency source without "
                        "changing any manifest or lockfile. A NautilusTrader pin cannot be "
                        f"proven merged while `{key}` is in effect; remove it, or verify the "
                        "override explicitly and justify it here."
                    )
    return checked


def collect_pins() -> dict[str, list[str]]:
    reject_source_overrides()
    pins: dict[str, list[str]] = {}
    declared = collect_from_manifests(pins)
    resolved = collect_from_lockfiles(pins)
    if declared == 0 or resolved == 0:
        fail(
            "no NautilusTrader source found "
            f"(declared={declared}, resolved={resolved}); the pin check would be vacuous"
        )

    if not CAPABILITY_MANIFEST.is_file():
        fail(f"{CAPABILITY_MANIFEST}: expected capability manifest is missing")
    capability = load_toml(CAPABILITY_MANIFEST)
    revision = capability.get("revision")
    if not isinstance(revision, str) or len(revision) != 40:
        fail(
            f"{CAPABILITY_MANIFEST}: `revision` must be a full 40-character commit, "
            f"got {revision!r}"
        )
    pins.setdefault(revision, []).append(f"{CAPABILITY_MANIFEST}:revision")
    return pins


def assert_merged(revision: str, default_branch: str) -> None:
    comparison = api(f"repos/{OFFICIAL_API_REPO}/compare/{revision}...{default_branch}")
    behind_by = comparison.get("behind_by")
    status = comparison.get("status")
    if behind_by is None:
        fail(f"{revision}: GitHub comparison returned no `behind_by` field")
    if behind_by != 0:
        fail(
            f"{revision} is NOT merged into {OFFICIAL_API_REPO}@{default_branch}: "
            f"status={status}, behind_by={behind_by}. The default branch lacks "
            f"{behind_by} commit(s) reachable from this revision, so it points at fork or "
            "otherwise unmerged work. GitHub serves fork-only commits from the official "
            "repository URL, which is why the assertions in build.rs pass for a pin like "
            f"this. Use `git merge-base <pin> upstream/{default_branch}` to find the "
            "upstream commit it branched from."
        )
    print(f"  {revision}: merged (status={status}, behind_by=0)")


def verify(offline_only: bool = False) -> None:
    pins = collect_pins()
    total = sum(len(owners) for owners in pins.values())
    print(f"Checking {total} NautilusTrader reference(s) across {len(pins)} revision(s).")

    if len(pins) != 1:
        detail = "\n".join(
            f"  {revision}: {', '.join(sorted(owners))}"
            for revision, owners in sorted(pins.items())
        )
        fail(
            "every NautilusTrader reference -- declared, resolved, and governed -- must name "
            f"one revision; found {len(pins)}:\n{detail}"
        )
    if offline_only:
        return

    default_branch = api(f"repos/{OFFICIAL_API_REPO}").get("default_branch")
    if not default_branch:
        fail(f"could not resolve the default branch of {OFFICIAL_API_REPO}")
    for revision in pins:
        assert_merged(revision, default_branch)
    print(f"All {total} reference(s) name one revision, merged into {default_branch}.")


def self_test() -> None:
    """Run every known bypass as a control, so they cannot silently return.

    Each control is a shape that bypassed some earlier version of this check.
    Collection controls run offline: every bypass worked by making a reference
    invisible, so reaching the single-revision verdict is what they defeated and
    that needs no network. One control runs online, because the merged check is
    a distinct failure mode and a suite that never exercised it would report
    health it had not tested.
    """
    import subprocess
    import tempfile

    official, fork_url = OFFICIAL_REPOSITORY, "https://github.com/someforker/nautilus_trader.git"
    good, bad = "e" * 40, "0" * 40
    lock = (
        '[[package]]\nname = "nautilus-core"\nversion = "0.1.0"\n'
        f'source = "git+{official}?rev={good}#{good}"\n'
    )

    controls = {
        "clean": ("", "", True),
        "regex_key_order": (f'nautilus-x = {{ rev = "{bad}", git = "{official}" }}\n', "", False),
        "branch_pin": (f'nautilus-x = {{ git = "{fork_url}", branch = "wip" }}\n', "", False),
        "tag_pin": (f'nautilus-x = {{ git = "{official}", tag = "v1" }}\n', "", False),
        "fork_url": (f'nautilus-x = {{ git = "{fork_url}", rev = "{good}" }}\n', "", False),
        # Named for what it proves: a second revision is detected. Mergedness
        # itself is the online control below.
        "divergent_rev": (f'nautilus-x = {{ git = "{official}", rev = "{bad}" }}\n', "", False),
        "renamed": (f'x = {{ package = "nautilus-core", git = "{official}", rev = "{bad}" }}\n', "", False),
        "patch_override": (
            "", f'[patch."{official}"]\nnautilus-core = {{ git = "{fork_url}", rev = "{bad}" }}\n', False,
        ),
        "replace_override": (
            "", f'[replace]\nnautilus-core = {{ git = "{official}", rev = "{bad}" }}\n', False,
        ),
        "patch_path_override": (
            "", f'[patch."{official}"]\nnautilus-core = {{ path = "../fork" }}\n', False,
        ),
        "cargo_paths_override": ("", "", False, "", 'paths = ["/tmp/fork"]\n'),
        "cargo_source_override": (
            "", "", False, "",
            '[source."https://github.com/nautechsystems/nautilus_trader.git"]\n'
            'replace-with = "vendored"\n',
        ),
        "malformed_manifest": ("", "", False, "", "", "not valid [toml at all\n"),
        "lock_override": (
            "", "",
            False,
            '[[package]]\nname = "nautilus-core"\nversion = "0.1.0"\n'
            f'source = "git+{fork_url}?rev={bad}#{bad}"\n',
        ),
    }

    failures = []
    online_controls = {
        # Every reference names one unmerged revision, so collection is happy and
        # only the merged check can reject it. Runs online deliberately.
        "unmerged_everywhere": FORK_REVISION,
    }

    for name, control in controls.items():
        extra_dep, extra_table, expect_pass = control[0], control[1], control[2]
        extra_lock = control[3] if len(control) > 3 else ""
        cargo_config = control[4] if len(control) > 4 else ""
        stray_manifest = control[5] if len(control) > 5 else ""
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "ci").mkdir()
            (root / "Cargo.toml").write_text(
                f'[dependencies]\nnautilus-core = {{ git = "{official}", rev = "{good}" }}\n'
                f"{extra_dep}\n{extra_table}"
            )
            (root / "Cargo.lock").write_text(f"{lock}{extra_lock}")
            (root / "ci").joinpath("nautilus-source-capabilities.toml").write_text(
                f'revision = "{good}"\n'
            )
            if stray_manifest:
                (root / "sub").mkdir()
                (root / "sub" / "Cargo.toml").write_text(stray_manifest)
            if cargo_config:
                (root / ".cargo").mkdir()
                (root / ".cargo" / "config.toml").write_text(cargo_config)
            result = subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), "--offline", str(root)],
                capture_output=True,
                text=True,
                env={**os.environ, "NAUTILUS_PIN_SELF_TEST": "1"},
            )
            passed = result.returncode == 0
            if passed != expect_pass:
                failures.append(
                    f"  {name}: expected {'accept' if expect_pass else 'reject'}, "
                    f"got exit {result.returncode}"
                )
            else:
                print(f"  {name}: {'accepted' if passed else 'rejected'} as expected")

    for name, revision in online_controls.items():
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "ci").mkdir()
            (root / "Cargo.toml").write_text(
                f'[dependencies]\nnautilus-core = {{ git = "{official}", rev = "{revision}" }}\n'
            )
            (root / "Cargo.lock").write_text(
                '[[package]]\nname = "nautilus-core"\nversion = "0.1.0"\n'
                f'source = "git+{official}?rev={revision}#{revision}"\n'
            )
            (root / "ci").joinpath("nautilus-source-capabilities.toml").write_text(
                f'revision = "{revision}"\n'
            )
            result = subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), str(root)],
                capture_output=True,
                text=True,
            )
            if result.returncode == 0:
                failures.append(
                    f"  {name}: a single unmerged revision was accepted; the merged check "
                    "did not run or did not reject"
                )
            elif "is NOT merged" not in result.stderr:
                failures.append(
                    f"  {name}: rejected, but not by the merged check: {result.stderr[:200]}"
                )
            else:
                print(f"  {name}: rejected by the merged check as expected")

    if failures:
        fail("pin-check self-test failed:\n" + "\n".join(failures))
    print(f"Self-test passed: {len(controls) + len(online_controls)} controls.")


def main() -> None:
    args = [a for a in sys.argv[1:]]
    if "--self-test" in args:
        self_test()
        return
    offline = "--offline" in args
    # `--offline` stops before the merged check, which is the whole point of the
    # lane. It exists only so the self-test can assert collection behaviour
    # without the network, so refuse it anywhere else rather than leave a flag
    # that silently turns this gate into a no-op.
    if offline and os.environ.get("NAUTILUS_PIN_SELF_TEST") != "1":
        fail(
            "--offline skips the merged check and is only valid inside --self-test; "
            "run without it so the gate actually verifies mergedness"
        )
    positional = [a for a in args if not a.startswith("--")]
    if len(positional) > 1:
        fail(f"usage: {Path(sys.argv[0]).name} [--self-test|--offline] [repository-root]")
    if positional:
        root = Path(positional[0])
        if not root.is_dir():
            fail(f"{root}: not a directory")
        os.chdir(root)
    verify(offline_only=offline)


if __name__ == "__main__":
    main()
