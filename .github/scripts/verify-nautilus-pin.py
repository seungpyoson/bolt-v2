#!/usr/bin/env python3
"""Assert every NautilusTrader revision this repository can build is merged upstream.

`build.rs` asserts that the governed capability manifest revision equals the
`nautilus-binance` pin and that the dependency URL is the official repository.
Neither proves the revision is *merged*: GitHub serves fork-only commits from
the upstream repository URL through the shared fork network, so a pin to
unmerged fork work satisfies both checks and builds green. That is the drift
this lane exists to catch, and `build.rs` cannot catch it because proving
mergedness needs the network and builds must stay hermetic.

Design note, learned the hard way across five review rounds and nine bypasses.
Every earlier version began by asking "does this refer to NautilusTrader?" and
was defeated by something that answered no: a hard-coded dependency; a regex
that missed reordered inline-table keys and `branch` pins; a structural walk
that missed `[patch]` and any manifest outside a hard-coded pair; a cargo-config
key list that missed `[alias]`; a read of `?rev=` where cargo reads the commit
after `#`; a URL cased `Nautilus_Trader`; one percent-encoding a letter as
`%6eautilus_trader`; a `[patch."<source>"]` whose *selector* was never read, so
an opaquely-named package could be swapped for a local path; and an opaque URL
that redirected to the official repository. Nine shapes, one mistake: the ways
to spell or indirect a name are unbounded, so recognition by appearance can
never be finished.

So the question is inverted, and this version never asks what a source refers
to. It requires every source in the resolved build to be one this repository
allows, and rejects everything else:

  * a registry source must be crates.io;
  * a git source must be the official repository at an exact lowercase
    revision, with no mutable `branch` or `tag`;
  * a source-less entry must name a crate whose manifest is tracked here,
    because that is the shape a path substitution produces;
  * an override table (`[patch.*]`, `[replace]`) may hold nothing but a
    canonical git pin, whatever selector it hangs under;
  * a tracked cargo config is refused outright, whatever it contains.

The asymmetry that makes this closed: an incomplete *allowlist* causes a false
failure, which someone notices and fixes; an incomplete *denylist* causes a
false pass, which is what all nine bypasses were. A tenth spelling is therefore
not a bypass but a rejection naming the source it could not place.

Both readings survive the inversion, and neither is sufficient alone. Measured,
not assumed: a `[patch]` onto a **git** source appears in the lockfile with that
source; a `[patch]` onto a **path** produces a lockfile entry with no `source`
field at all. The lockfile reading now catches the second as well, because a
source-less entry is judged against the tracked-crate set rather than skipped --
but the manifest reading also holds it, and holds the mutability rules that a
lockfile cannot express, since a lockfile records only what a `branch` resolved
to on the day it was written.

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
import urllib.request
from pathlib import Path

OFFICIAL_REPOSITORY = "https://github.com/nautechsystems/nautilus_trader.git"
OFFICIAL_API_REPO = "nautechsystems/nautilus_trader"
# A real unmerged fork revision, used by the online self-test control.
FORK_REVISION = "01d5af1427d73532f6dd9f2be77acb72f825bec9"
CAPABILITY_MANIFEST = Path("ci/nautilus-source-capabilities.toml")

# The registry sources this repository allows. Both spellings of crates.io are
# listed because cargo has shipped both and either may appear depending on the
# version that wrote the lockfile. Listing them is safe in a way that listing
# forbidden shapes is not: a crates.io spelling missing from here fails the
# lane, it does not slip through it.
ALLOWED_REGISTRIES = frozenset(
    {
        "registry+https://github.com/rust-lang/crates.io-index",
        "sparse+https://index.crates.io/",
    }
)

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


def workspace_crate_names(lockfile: Path) -> set[str]:
    """The crates this lockfile may record without a source.

    A lockfile entry with no `source` is either a crate of this lockfile's own
    workspace or a dependency substituted by path, and the lockfile itself
    cannot tell them apart. The manifests settle it without asking cargo to
    resolve anything -- which matters, because resolution would need the git
    dependency fetched and this lane runs before any build.

    Which manifests, though, is the whole question. Collecting every `[package]
    name` tracked anywhere in the repository was the denylist's silent-false-
    pass shape surviving inside the allowlist: any manifest added anywhere --
    a test fixture, an example -- minted a name, and a source-less entry passed
    by *claiming* that name rather than by being that crate. A path dependency
    pointing outside the repository then rode in on the match, and cargo built
    the outside code under `--locked`.

    So the set is walked from this lockfile's own workspace root through only
    the paths the manifests themselves name -- `[workspace] members` and `path`
    dependencies -- and a manifest contributes its name only if it is tracked
    here. A path dependency that leaves the repository resolves to nothing and
    contributes nothing, which is precisely what makes the name it claims
    unavailable. Identity is where the crate is, not what it is called.

    Limit worth naming, unchanged by this: a substitution pointing at a crate
    manifest *added to this repository* and wired in by path is reachable, so it
    is named and accepted. That is not a spelling trick -- it is vendoring the
    dependency into the tree, which arrives as a diff of hundreds of files. This
    check governs what the build pulls from outside; code added inside is what
    review is for.
    """
    tracked = set(tracked_paths())
    names: set[str] = set()
    seen: set[Path] = set()
    pending = [lockfile.parent / "Cargo.toml"]

    while pending:
        # Collapse `a/../b` lexically rather than against the filesystem: a
        # manifest outside the repository normalises to a path git never listed,
        # so membership in the tracked set is the containment test as well.
        manifest = Path(os.path.normpath(pending.pop()))
        if manifest in seen or manifest not in tracked:
            continue
        seen.add(manifest)

        document = load_toml(manifest)
        directory = manifest.parent
        package = document.get("package")
        if isinstance(package, dict) and isinstance(package.get("name"), str):
            names.add(package["name"])

        workspace = document.get("workspace")
        if isinstance(workspace, dict):
            for member in workspace.get("members", []):
                if isinstance(member, str):
                    # `members` entries are globs, and a literal is a glob that
                    # matches itself, so one path covers both spellings.
                    pending.extend(match / "Cargo.toml" for match in directory.glob(member))

        for _owner, table, _override in dependency_tables(document, ""):
            for spec in table.values():
                if isinstance(spec, dict) and isinstance(spec.get("path"), str):
                    pending.append(directory / spec["path"] / "Cargo.toml")

    return names


def dependency_tables(document: dict, owner: str, override: bool = False):
    """Yield (owner, table, override) for every table that names a dependency.

    `override` marks `[patch.*]` and `[replace]`, the tables that change what
    cargo builds without touching the dependency block. They are held to a
    stricter rule than ordinary dependencies: an ordinary entry is free to come
    from crates.io and is only checked when it carries a `git` key, whereas an
    override exists solely to redirect a source and so must be a canonical git
    pin or nothing.

    The selector a `[patch."<source>"]` table hangs under is deliberately not
    read. Reading it was the ninth bypass -- an override of an opaquely-named
    package went unexamined because the selector did not look like anything in
    particular. What the entry *resolves to* is the only thing that matters, and
    that is judged the same way under every selector.
    """
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        table = document.get(key)
        if isinstance(table, dict):
            yield f"{owner}{key}", table, override

    replace = document.get("replace")
    if isinstance(replace, dict):
        yield f"{owner}replace", replace, True

    nested = document.get("workspace")
    if isinstance(nested, dict):
        yield from dependency_tables(nested, f"{owner}workspace.", override)

    patch = document.get("patch")
    if isinstance(patch, dict):
        for selector, table in patch.items():
            if isinstance(table, dict):
                yield f"{owner}patch.{selector}", table, True

    target = document.get("target")
    if isinstance(target, dict):
        for selector, table in target.items():
            if isinstance(table, dict):
                yield from dependency_tables(table, f"{owner}target.{selector}.", override)


# The keys cargo honours on a dependency that inherits with `workspace = true`.
# Stated as what is allowed rather than what is forbidden: a denylist here was
# both dead text and incomplete -- cargo ignores `git`, `registry`, `package`
# and `version` alike on an inheriting entry, so forbidding four of them while
# missing the others screened nothing and implied a divergent pin was possible
# through this table when it is not.
# Both spellings of default-features appear: cargo accepts the underscore form
# and ignores it here exactly as it ignores the hyphenated one, so rejecting one
# and allowing the other would fail a build over a spelling cargo does not care
# about. Over-rejection is the failure mode an allowlist introduces, and it is
# the one to watch.
INHERITED_KEYS = frozenset(
    {"workspace", "optional", "features", "default-features", "default_features", "public"}
)


def check_declaration(
    owner: str, spec: object, pins: dict[str, list[str]], override: bool
) -> bool:
    """Judge one dependency entry. Returns whether it registered a git pin.

    Ordinary entries are examined only when they carry a `git` key, because a
    crates.io dependency is not this lane's business. Override entries are
    examined unconditionally: an override's whole purpose is to redirect a
    source, so one that is not a canonical git pin is either a path
    substitution or something this check cannot interpret, and both fail.
    """
    if isinstance(spec, dict) and spec.get("workspace") is True:
        # An inheriting member names the dependency but does not pin it: the
        # revision lives in the workspace root's `[workspace.dependencies]`,
        # which `dependency_tables` walks and checks like any other table. So
        # this is not an unpinned reference, it is the same reference read
        # once. Demanding a `git` key here told the engineer to duplicate the
        # pin into the member -- the opposite of the one-declaration shape that
        # makes a pin bump a single edit.
        for key in spec:
            if key not in INHERITED_KEYS:
                fail(
                    f"{owner} inherits with `workspace = true` and also sets `{key}`. "
                    "An inheriting dependency takes its source from the workspace root, "
                    f"so beyond {sorted(INHERITED_KEYS)} any key here is dead text that "
                    "reads like a pin."
                )
        return False

    if not override and not (isinstance(spec, dict) and "git" in spec):
        return False

    if not isinstance(spec, dict):
        fail(
            f"{owner} overrides a dependency but is not an inline table pinning the "
            f"official repository by revision, got {spec!r}"
        )
    git = spec.get("git")
    if git is None:
        fail(
            f"{owner} redirects a dependency with no `git` key. An override may only point "
            "at the official repository at an exact revision; a path substitution builds "
            "code no revision can name."
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
    return True


def collect_from_manifests(pins: dict[str, list[str]]) -> int:
    seen = 0
    for manifest in discover("Cargo.toml"):
        document = load_toml(manifest)
        for table_owner, table, override in dependency_tables(document, ""):
            for name, spec in table.items():
                if check_declaration(f"{manifest}:{table_owner}.{name}", spec, pins, override):
                    seen += 1
    return seen


def collect_from_lockfiles(pins: dict[str, list[str]]) -> int:
    """Lockfiles record what cargo resolved, and every entry must be placeable.

    This is the reading the inversion changed most. It used to find the entries
    that looked like NautilusTrader and check those; now it places *every*
    entry into one of three allowed kinds -- a crates.io registry package, the
    official repository at a revision, or one of this repository's own crates --
    and fails on anything it cannot place. The package name is never consulted
    to decide whether an entry is interesting, which is what let an opaquely
    named substitution through.
    """
    seen = 0
    for lockfile in discover("Cargo.lock"):
        document = load_toml(lockfile)
        # Derived per lockfile, from that workspace's own reachable manifests: a
        # crate belonging to some other workspace in this repository is not a
        # crate *this* build may resolve without a source.
        workspace_crates = workspace_crate_names(lockfile)
        for package in document.get("package", []):
            name = package.get("name", "")
            source = package.get("source")
            owner = f"{lockfile}:{name}"
            if source is None:
                # A workspace crate and a path-substituted dependency are
                # indistinguishable here -- both simply have no source -- so the
                # manifests this workspace reaches by path decide which it is.
                if name in workspace_crates:
                    continue
                fail(
                    f"{owner} resolves without a source and is not a crate this workspace "
                    "reaches by path within this repository, which is what a dependency "
                    "substituted by path looks like. Every dependency must resolve to "
                    "crates.io or to the official repository at an exact revision."
                )
            if not isinstance(source, str):
                fail(f"{owner} records a source that is not a string: {source!r}")
            if source in ALLOWED_REGISTRIES:
                continue
            match = LOCK_SOURCE.match(source)
            if match is None:
                fail(
                    f"{owner} resolves from a source this repository does not allow: "
                    f"{source}. Allowed sources are crates.io and the official repository "
                    f"{OFFICIAL_REPOSITORY} at an exact revision."
                )
            seen += 1
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
        # The two bypasses that ended recognition-by-appearance. Both carry a
        # package name with no `nautilus` in it, which is exactly why the old
        # readings skipped them: the entry was never judged at all. Under the
        # allowlist the name is not consulted, so an opaque name is no longer a
        # way to avoid being looked at.
        "patch_opaque_package_by_path": {
            "table": f'[patch."{official}"]\nopaque-payload = {{ path = "../fork" }}\n',
            "lock": '[[package]]\nname = "opaque-payload"\nversion = "0.1.0"\n',
            "reason": "with no `git` key",
        },
        # The same substitution seen only through the lockfile, so the lockfile
        # reading is proven to catch it without help from the manifest.
        "lock_sourceless_opaque_package": {
            "lock": '[[package]]\nname = "opaque-payload"\nversion = "0.1.0"\n',
            "reason": "reaches by path within this repository",
        },
        # The allowlist's own false-pass shape, found by review after the
        # inversion shipped: the source-less set was every `[package] name`
        # tracked anywhere, so any manifest added anywhere in the tree -- a
        # fixture, an example -- minted a name that a substituted dependency
        # could then claim. The manifest here is tracked and names the crate; it
        # is simply not one this workspace reaches, and reachability is now what
        # decides.
        "lock_sourceless_name_from_unreachable_manifest": {
            "stray": '[package]\nname = "opaque-payload"\nversion = "0.1.0"\n',
            "lock": '[[package]]\nname = "opaque-payload"\nversion = "0.1.0"\n',
            "reason": "reaches by path within this repository",
        },
        # The same hole exercised the way it would actually be used, and the way
        # it was reproduced in review: a path dependency leaving the repository
        # entirely, wearing the name of a crate a tracked manifest declares.
        # Cargo builds the outside code under `--locked`; matching by name let it
        # through, so identity is the resolved path instead.
        "path_dependency_outside_the_repository": {
            "stray": '[package]\nname = "local-thing"\nversion = "0.1.0"\n',
            "dep": 'local-thing = { path = "../fork" }\n',
            "lock": '[[package]]\nname = "local-thing"\nversion = "0.1.0"\n',
            "reason": "reaches by path within this repository",
        },
        # A URL that redirects to the official repository. Nothing here tries to
        # detect the redirect -- an unlisted URL is refused whatever it serves,
        # which is why resolving it is no longer necessary.
        "redirecting_url_in_manifest": {
            "dep": f'opaque-payload = {{ git = "https://example.invalid/mirror.git", '
            f'rev = "{bad}" }}\n',
            "reason": official_repo,
        },
        "redirecting_url_in_lockfile": {
            "lock": '[[package]]\nname = "opaque-payload"\nversion = "0.1.0"\n'
            f'source = "git+https://example.invalid/mirror.git?rev={bad}#{bad}"\n',
            "reason": "not the official repository",
        },
        # An alternative registry can serve a package under any name at all.
        "unrecognized_registry": {
            "lock": '[[package]]\nname = "opaque-payload"\nversion = "0.1.0"\n'
            'source = "registry+https://example.invalid/index"\n',
            "reason": "does not allow",
        },
        # Acceptances, guarding the failure mode the inversion introduces: a rule
        # that refuses everything it does not recognize can refuse the ordinary
        # build. Tightening reachability above narrows the source-less set, which
        # is exactly the direction that breaks a working repository, so the two
        # legitimate shapes it must still admit are pinned here alongside the
        # registry spellings.
        "crates_io_dependency": {
            "lock": '[[package]]\nname = "serde"\nversion = "1.0.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n',
        },
        "sparse_crates_io_dependency": {
            "lock": '[[package]]\nname = "serde"\nversion = "1.0.0"\n'
            'source = "sparse+https://index.crates.io/"\n',
        },
        "tracked_local_crate_is_sourceless": {
            "package": '[package]\nname = "local-thing"\nversion = "0.1.0"\n',
            "lock": '[[package]]\nname = "local-thing"\nversion = "0.1.0"\n',
        },
        # A member reached through a `[workspace] members` glob. Nothing in the
        # repository's own manifests declares this crate as a dependency, so
        # walking dependencies alone would have rejected a perfectly ordinary
        # workspace.
        "workspace_member_is_sourceless": {
            "table": '[workspace]\nmembers = ["member"]\n',
            "stray_dir": "member",
            "stray": '[package]\nname = "member-crate"\nversion = "0.1.0"\n',
            "lock": '[[package]]\nname = "member-crate"\nversion = "0.1.0"\n',
        },
        # A path dependency that stays inside the repository -- the shape the
        # second workspace here actually uses to depend on the first, which
        # resolves source-less and is not a member of the workspace that names
        # it. Rejecting this would break the build outright.
        "path_dependency_inside_the_repository": {
            "dep": 'inner-crate = { path = "inner" }\n',
            "stray_dir": "inner",
            "stray": '[package]\nname = "inner-crate"\nversion = "0.1.0"\n',
            "lock": '[[package]]\nname = "inner-crate"\nversion = "0.1.0"\n',
        },
        # The underscore spelling of default-features on an inheriting entry.
        # Cargo accepts and ignores it exactly as it does the hyphenated form, so
        # a lane that rejected one and allowed the other would fail a build over
        # a spelling cargo does not care about.
        "workspace_inheritance_with_underscore_default_features": {
            "dep": "nautilus-inherited = { workspace = true, default_features = false }\n",
            "table": f'[workspace.dependencies]\nnautilus-inherited = {{ git = "{official}", '
            f'rev = "{good}" }}\n',
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
            "Cargo.toml": f"{control.get('package', '')}"
            + f'[dependencies]\nnautilus-core = {{ git = "{official}", rev = "{good}" }}\n'
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
