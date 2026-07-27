#!/usr/bin/env python3
"""Assert every NautilusTrader revision this repository can build is merged upstream.

`build.rs` asserts that the governed capability manifest revision equals the
`nautilus-binance` pin and that the dependency URL is the official repository.
Neither proves the revision is *merged*: GitHub serves fork-only commits from
the upstream repository URL through the shared fork network, so a pin to
unmerged fork work satisfies both checks and builds green. That is the drift
this lane exists to catch, and `build.rs` cannot catch it because proving
mergedness needs the network and builds must stay hermetic.

Design note, learned the hard way across four review rounds. Every previous
version of this check enumerated what to inspect and was bypassed by a shape it
did not enumerate: one hard-coded dependency, then a regex that missed reordered
inline-table keys and `branch` pins, then a structural walk that missed `[patch]`
and any manifest outside a hard-coded pair, then a cargo-config key list that
missed `[alias]`. A fifth bypass needed no new shape at all -- reading `?rev=`
where cargo reads the commit after `#` validated a revision nothing compiles. So
this version enumerates nothing:

  * manifests and lockfiles are **discovered**, never listed;
  * lockfile git entries register *both* halves -- the commit after `#`, which
    is what cargo checks out, and the `?rev=` that requested it -- so a
    disagreement between them fails as two revisions;
  * manifests are read for *declared intent*, including the override tables;
  * a tracked cargo config is refused outright, not screened key by key;
  * a URL is recognized by what it *resolves to*, not by how it is spelled;
  * anything referencing NautilusTrader that cannot be interpreted is a
    failure, never a silent skip.

Spelling is its own bypass family, and it took two rounds to see as one. Both
members were found by comparing text: a URL cased `Nautilus_Trader`, then one
percent-encoding a single letter as `%6eautilus_trader`. Both resolve to the
official repository -- measured, not assumed, for the second: cargo fetches the
encoded URL, and writes it into `Cargo.lock` still encoded, so a comparison
against raw text is blind in both readings at once. `repository_identity`
answers "what does this resolve to" in one place, and every recognition site
asks it rather than comparing text of its own.

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
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

OFFICIAL_REPOSITORY = "https://github.com/nautechsystems/nautilus_trader.git"
OFFICIAL_API_REPO = "nautechsystems/nautilus_trader"
# A real unmerged fork revision, used by the online self-test control.
FORK_REVISION = "01d5af1427d73532f6dd9f2be77acb72f825bec9"
NAUTILUS = "nautilus"
CAPABILITY_MANIFEST = Path("ci/nautilus-source-capabilities.toml")

# `git+<url>?rev=<rev>#<resolved>` as Cargo.lock records a git source.
LOCK_SOURCE = re.compile(r"^git\+(?P<url>[^?#]+)(?:\?(?P<query>[^#]*))?(?:#(?P<resolved>.*))?$")
# Canonical git object name: full length, lowercase.
CANONICAL_REVISION = re.compile(r"^[0-9a-f]{40}$")


def register(pins: dict[str, list[str]], label: str, revision: str, context: str) -> None:
    """Record one revision reference, under one validity rule for the field.

    Canonical lowercase is required rather than normalized away, for two
    reasons. `build.rs::is_git_head_sha` already requires it of the manifest
    pin, and two guards on one field with different rules is exactly how a value
    passes one and fails the other. And cargo accepts an uppercase `rev`, then
    writes `?rev=<UPPERCASE>#<lowercase>` -- one commit spelled two ways, which
    the single-revision rule would otherwise report as two revisions. Requiring
    the canonical spelling makes that disagreement unrepresentable instead of
    something every comparison has to remember to normalize.
    """
    if not CANONICAL_REVISION.match(revision):
        fail(
            f"{label} must name a full 40-character lowercase revision, got {revision!r}"
            f"{context}"
        )
    pins.setdefault(revision, []).append(label)


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


def tracked_paths() -> list[Path]:
    """Every tracked path, asked of git and filtered here, never by pathspec.

    An earlier version walked the filesystem and skipped directories by name.
    That silently excluded a tracked manifest under one of those names, and it
    inherited `rglob`'s symlink behaviour, which differs by interpreter version.
    Git knows exactly which files are in the repository; nothing else needs to.

    The whole tracked set is returned rather than a pathspec match because git's
    globbing is case-sensitive while the filesystems this is developed on are
    not. `git ls-files -- '*config.toml'` does not match a tracked
    `.cargo/Config.toml`, and cargo opens it anyway. Deciding what a path is
    belongs here, in one place, where case can be handled explicitly.
    """
    result = subprocess.run(["git", "ls-files", "-z"], capture_output=True, text=True)
    if result.returncode != 0:
        fail(
            "cannot list tracked files with git, so the set of manifests to check is "
            f"unknown: {result.stderr.strip()}"
        )
    return sorted({Path(p) for p in result.stdout.split("\0") if p})


def discover(filename: str) -> list[Path]:
    """Tracked files with this name, matched without regard to case."""
    wanted = filename.lower()
    return [path for path in tracked_paths() if path.name.lower() == wanted]


def repository_identity(value: str) -> str:
    """What a URL resolves to, with the spellings that do not change it removed.

    Recognition has to answer "does this fetch NautilusTrader", and text
    comparison answers "is this written the way I expected" instead. Two
    bypasses came from that gap, each a different spelling of the official
    repository: `Nautilus_Trader` (GitHub resolves owner and repository names
    without regard to case) and `%6eautilus_trader` (percent-encoding, which
    git decodes at the transport and cargo copies into `Cargo.lock` verbatim,
    so raw text is blind in both readings at once). Decoding then lowercasing
    collapses both, and any further spelling that survives transport decoding.

    Repeated decoding is deliberate. `%256e` decodes once to `%6e` and again to
    `n`, and whether an intermediary decodes twice is not ours to assume, so
    identity is the fixed point rather than one pass. The loop terminates
    because each pass that changes the string strictly shortens it.
    """
    seen = value
    for _ in range(8):
        decoded = urllib.parse.unquote(seen)
        if decoded == seen:
            break
        seen = decoded
    return seen.lower()


def mentions_nautilus(name: str, spec: object) -> bool:
    """Whether a dependency names NautilusTrader, decided by resolved identity.

    Detection is deliberately looser than the equality that follows it: a
    case-varied or percent-encoded URL is *found* here and then *rejected* by
    `check_declaration` for not being the official repository, which is the
    actionable outcome. Equality stays exact so that one spelling is canonical
    in the tree; only recognition is spelling-blind.
    """
    if NAUTILUS in repository_identity(name):
        return True
    if isinstance(spec, dict):
        for key in ("package", "git"):
            value = spec.get(key)
            if isinstance(value, str) and NAUTILUS in repository_identity(value):
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
    if spec.get("workspace") is True:
        # An inheriting member names the dependency but does not pin it: the
        # revision lives in the workspace root's `[workspace.dependencies]`,
        # which `dependency_tables` walks and checks like any other table. So
        # this is not an unpinned reference, it is the same reference read
        # once. Demanding a `git` key here told the engineer to duplicate the
        # pin into the member -- the opposite of the one-declaration shape that
        # makes a pin bump a single edit.
        for key in ("git", "rev", "branch", "tag", "path", "version"):
            if key in spec:
                fail(
                    f"{owner} inherits with `workspace = true` and also sets `{key}`. "
                    "An inherited dependency takes its source from the workspace root; "
                    "a second source here is either dead text or a divergent pin."
                )
        return
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
    if not isinstance(revision, str):
        fail(f"{owner} must pin a full 40-character `rev`, got {revision!r}")
    register(pins, owner, revision, "")


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
            # Resolved identity for the same reason as `mentions_nautilus`, and
            # this reading is where percent-encoding bites hardest: cargo copies
            # the manifest's spelling into `source` unchanged, so an encoded URL
            # reaches here still encoded and raw text does not see it.
            if NAUTILUS not in repository_identity(f"{name}{source or ''}"):
                continue
            if not isinstance(source, str):
                # A path-substituted dependency resolves with no source at all.
                # Skipping it is how a source override stays invisible here, so
                # this fails regardless of which mechanism produced it.
                fail(
                    f"{lockfile}:{name} resolves without a source, which happens when a "
                    "dependency is substituted by path. A NautilusTrader package must "
                    "resolve to the official repository at an exact revision."
                )
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
            # A git entry records both the request (`?rev=`) and the commit cargo
            # checked out (`#<resolved>`). Cargo builds the fragment: measured,
            # not assumed -- a lockfile reading `?rev=<merged>#<fork>` compiles
            # the fork under `--locked` and is never rewritten, so reading the
            # query would validate a revision nothing compiles. Neither half is
            # preferred here; both are registered, so a lockfile whose halves
            # disagree fails as exactly what it is -- two revisions -- through the
            # same verdict that catches every other multi-revision shape.
            revisions = {
                f"{owner} ({role})": revision
                for role, revision in (
                    ("requested", query.get("rev")),
                    ("resolved by cargo", match.group("resolved")),
                )
                if revision
            }
            if not revisions:
                fail(f"{owner} records no revision at all: {source}")
            for label, revision in revisions.items():
                register(pins, label, revision, f": {source}")
    return seen


def reject_tracked_cargo_config() -> None:
    """Refuse a tracked cargo config, whatever it contains.

    Cargo config can redirect a build in more ways than a reader can enumerate.
    An earlier version screened a key list -- `patch`, `paths`, `source`,
    `include` -- and review found a fifth outside it: `[alias]` redefines the
    subcommands CI runs, and an alias body may carry `--config`, so a lane step
    written as `cargo zigbuild --locked` expands to
    `cargo build --config paths=[...] --locked`. Measured, not assumed: that
    alias builds a local fork, satisfies `--locked`, and leaves `Cargo.lock`
    untouched, exactly as a `paths` override does.

    Adding a fifth key would repeat the mistake this file's design note already
    records. This repository tracks no cargo config at all, so the rule is the
    file's absence rather than its contents, and there is no key list left to
    fall behind.

    What is refused is any tracked path with a `.cargo` component, compared
    without regard to case, rather than a file whose name and parent look right.
    Two demonstrated escapes motivate that. A tracked `.cargo/Config.toml` is
    invisible to a case-sensitive pathspec yet cargo opens it on a
    case-insensitive filesystem. A tracked `.cargo` *symlink* to an
    ordinarily-named directory leaves the real file's parent called something
    else, so a parent-name test skips it while cargo still reads it through the
    link -- and the symlink itself is a tracked path named `.cargo`, so the
    component test catches it where a name test cannot.

    Limit worth naming: this sees tracked files. `$CARGO_HOME/config.toml`, a
    config a workflow step writes at runtime, and `--config` on the command line
    are outside the repository and cannot be checked here.
    """
    for path in tracked_paths():
        if ".cargo" not in [part.lower() for part in path.parts]:
            continue
        fail(
            f"{path} is a tracked cargo config, or makes one reachable. Cargo config can "
            "redirect a dependency source and can redefine the subcommands CI runs, both "
            "without changing any manifest or lockfile, so a NautilusTrader pin cannot be "
            "proven merged while one is tracked. This repository needs none; remove it, or "
            "verify what it does explicitly and justify it here."
        )


def collect_pins() -> dict[str, list[str]]:
    reject_tracked_cargo_config()
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
    if not isinstance(revision, str):
        fail(
            f"{CAPABILITY_MANIFEST}: `revision` must be a full 40-character commit, "
            f"got {revision!r}"
        )
    register(pins, f"{CAPABILITY_MANIFEST}:revision", revision, "")
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


def track_fixture(root: Path) -> None:
    """Make a fixture a git repository, because discovery asks git what exists."""
    for command in (["git", "init", "-q"], ["git", "add", "-A"]):
        outcome = subprocess.run(command, cwd=root, capture_output=True, text=True)
        if outcome.returncode != 0:
            fail(f"self-test fixture setup failed: {outcome.stderr.strip()}")


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

    # Named keys rather than positions: the controls vary along enough axes that
    # a reader cannot tell what `control[4]` means, and adding an axis silently
    # renumbers every control that omitted it.
    #
    # Each rejecting control also names the message that must reject it. An exit
    # code alone cannot tell a working control from one whose fixture is broken
    # in some unrelated way -- which happened here: a control meant to exercise
    # the merged check returned before reaching it, and passed while proving
    # nothing. A control with no message is one that must be accepted, so a
    # rejecting control that does not say why cannot be written.
    one_revision = "one revision; found"
    official_repo = "must use the official repository"
    controls = {
        "clean": {},
        "regex_key_order": {
            "dep": f'nautilus-x = {{ rev = "{bad}", git = "{official}" }}\n',
            "reason": one_revision,
        },
        # Official URL deliberately: the repository check runs first, so a fork
        # URL here would be rejected before mutability was ever consulted and the
        # control would pass with `branch` handling deleted.
        "branch_pin": {
            "dep": f'nautilus-x = {{ git = "{official}", branch = "wip" }}\n',
            "reason": "which is mutable",
        },
        "tag_pin": {
            "dep": f'nautilus-x = {{ git = "{official}", tag = "v1" }}\n',
            "reason": "which is mutable",
        },
        "fork_url": {
            "dep": f'nautilus-x = {{ git = "{fork_url}", rev = "{good}" }}\n',
            "reason": official_repo,
        },
        # Named for what it proves: a second revision is detected. Mergedness
        # itself is the online control below.
        "divergent_rev": {
            "dep": f'nautilus-x = {{ git = "{official}", rev = "{bad}" }}\n',
            "reason": one_revision,
        },
        "renamed": {
            "dep": f'x = {{ package = "nautilus-core", git = "{official}", rev = "{bad}" }}\n',
            "reason": one_revision,
        },
        "patch_override": {
            "table": f'[patch."{official}"]\nnautilus-core = {{ git = "{fork_url}", rev = "{bad}" }}\n',
            "reason": official_repo,
        },
        "replace_override": {
            "table": f'[replace]\nnautilus-core = {{ git = "{official}", rev = "{bad}" }}\n',
            "reason": one_revision,
        },
        "patch_path_override": {
            "table": f'[patch."{official}"]\nnautilus-core = {{ path = "../fork" }}\n',
            "reason": "with no `git` key",
        },
        # The cargo-config rule is the file's absence, so these two vary its
        # contents instead of its keys: one is the `[alias]` shape that defeated
        # the previous key list by smuggling `--config` into the subcommand CI
        # runs, the other declares nothing about sources at all. A rule that
        # rejects the benign one cannot be reading contents, which is what
        # replaced the four key-specific controls these two supersede.
        "cargo_config_alias": {
            "cargo_config": '[alias]\nzigbuild = ["build", "--config", "paths=[\'../fork\']"]\n',
            "reason": "tracked cargo config",
        },
        "cargo_config_unrelated_key": {
            "cargo_config": "[build]\njobs = 2\n",
            "reason": "tracked cargo config",
        },
        # Git's pathspec globbing is case-sensitive; the filesystems this is
        # developed on are not, so cargo opens what a pathspec never matched.
        "cargo_config_case_variant": {
            "cargo_config_name": ".cargo/Config.toml",
            "cargo_config": 'paths = ["../fork"]\n',
            "reason": "tracked cargo config",
        },
        # The real file's parent is not called `.cargo`; the symlink is.
        "cargo_config_via_symlink": {
            "cargo_config_name": "cargo-config/config.toml",
            "cargo_config": 'paths = ["../fork"]\n',
            "cargo_config_symlink": (".cargo", "cargo-config"),
            "reason": "tracked cargo config",
        },
        # GitHub resolves owner and repository names case-insensitively, so this
        # is the official repository under a spelling that contains no lowercase
        # `nautilus` -- which is how it escaped detection entirely, revision and
        # all, while the genuine references kept the check non-vacuous.
        "cased_repository_url": {
            "dep": 'fork-payload = { git = "https://github.com/NautechSystems/Nautilus_Trader.git", '
            f'rev = "{bad}" }}\n',
            "reason": official_repo,
        },
        "cased_repository_url_in_lockfile": {
            "lock": '[[package]]\nname = "fork-payload"\nversion = "0.1.0"\n'
            f'source = "git+https://github.com/NautechSystems/Nautilus_Trader.git?rev={bad}#{bad}"\n',
            "reason": "not the official repository",
        },
        # The same repository again, spelled with `n` percent-encoded. Measured
        # against a local git remote, not assumed: cargo fetches this and writes
        # it into `Cargo.lock` still encoded, so the pair below is one bypass
        # that both readings missed at once rather than two related ones.
        "encoded_repository_url": {
            "dep": 'fork-payload = { git = "https://github.com/nautechsystems/%6eautilus_trader.git", '
            f'rev = "{bad}" }}\n',
            "reason": official_repo,
        },
        "encoded_repository_url_in_lockfile": {
            "lock": '[[package]]\nname = "fork-payload"\nversion = "0.1.0"\n'
            f'source = "git+https://github.com/nautechsystems/%6eautilus_trader.git?rev={bad}#{bad}"\n',
            "reason": "not the official repository",
        },
        # Accepted, and the only control here that pins a shape the repository
        # does not use yet: inheritance is how the duplicated manifest pins
        # collapse to one declaration, and demanding a `git` key from the
        # inheriting member rejected exactly that.
        "workspace_inherited_dependency": {
            "dep": "nautilus-inherited = { workspace = true }\n",
            "table": f'[workspace.dependencies]\nnautilus-inherited = {{ git = "{official}", '
            f'rev = "{good}" }}\n',
        },
        # And the guard that keeps the acceptance above from being a hole: an
        # inheriting member that also carries a source is not inheriting.
        "workspace_inheritance_with_source": {
            "dep": f'nautilus-inherited = {{ workspace = true, git = "{fork_url}", rev = "{bad}" }}\n',
            "table": f'[workspace.dependencies]\nnautilus-inherited = {{ git = "{official}", '
            f'rev = "{good}" }}\n',
            "reason": "and also sets",
        },
        # Cargo accepts an uppercase `rev` and then writes `?rev=<UPPER>#<lower>`
        # -- one commit spelled two ways, which the single-revision rule reported
        # as two revisions until one canonical spelling was required.
        "uppercase_revision": {
            "dep": f'nautilus-x = {{ git = "{official}", rev = "{good.upper()}" }}\n',
            "reason": "lowercase revision",
        },
        # `?rev=` names a merged revision while the fragment cargo actually
        # checks out names another. Reading the query accepted this.
        "lock_fragment_mismatch": {
            "lock": '[[package]]\nname = "nautilus-model"\nversion = "0.1.0"\n'
            f'source = "git+{official}?rev={good}#{bad}"\n',
            "reason": one_revision,
        },
        "lock_sourceless_nautilus": {
            "lock": '[[package]]\nname = "nautilus-polymarket"\nversion = "0.1.0"\n',
            "reason": "resolves without a source",
        },
        "manifest_under_excluded_dir_name": {
            "stray": f'[dependencies]\nnautilus-model = {{ git = "{official}", rev = "{bad}" }}\n',
            "stray_dir": "node_modules/x",
            "reason": one_revision,
        },
        "malformed_manifest": {
            "stray": "not valid [toml at all\n",
            "reason": "cannot be parsed",
        },
        "lock_override": {
            "lock": '[[package]]\nname = "nautilus-core"\nversion = "0.1.0"\n'
            f'source = "git+{fork_url}?rev={bad}#{bad}"\n',
            "reason": "not the official repository",
        },
    }

    failures = []
    online_controls = {
        # Every reference names one unmerged revision, so collection is happy and
        # only the merged check can reject it. Runs online deliberately.
        "unmerged_everywhere": FORK_REVISION,
    }

    for name, control in controls.items():
        # One mapping of path to content, written by one loop: an empty entry is
        # an absent file, which is what several controls exist to produce, so
        # omission needs no branch of its own.
        fixture = {
            "Cargo.toml": f'[dependencies]\nnautilus-core = {{ git = "{official}", rev = "{good}" }}\n'
            + f"{control.get('dep', '')}\n{control.get('table', '')}",
            "Cargo.lock": f"{lock}{control.get('lock', '')}",
            "ci/nautilus-source-capabilities.toml": f'revision = "{good}"\n',
            f"{control.get('stray_dir', 'sub')}/Cargo.toml": control.get("stray", ""),
            control.get("cargo_config_name", ".cargo/config.toml"): control.get(
                "cargo_config", ""
            ),
        }
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for relative, content in fixture.items():
                if not content:
                    continue
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            link = control.get("cargo_config_symlink")
            if link:
                (root / link[0]).symlink_to(link[1])
            track_fixture(root)
            result = subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), "--offline", str(root)],
                capture_output=True,
                text=True,
                env={**os.environ, "NAUTILUS_PIN_SELF_TEST": "1"},
            )
            # A control states the message that must reject it, or states nothing
            # and must be accepted. There is no way to write a rejecting control
            # that does not say why, so nothing needs to check for one.
            reason = control.get("reason")
            outcome = "accepted" if result.returncode == 0 else "rejected"
            expected = "rejected" if reason else "accepted"
            if outcome != expected:
                failures.append(
                    f"  {name}: expected {expected}, got {outcome} "
                    f"(exit {result.returncode}): {(result.stderr or result.stdout)[:300]}"
                )
            elif reason and reason not in result.stderr:
                failures.append(
                    f"  {name}: rejected, but not by {reason!r}: {result.stderr[:300]}"
                )
            else:
                print(f"  {name}: {outcome} as expected")

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
            track_fixture(root)
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
