# Merge Queue Preflight Origin Design

## Problem

`merge_queue_preflight.py` fetches pinned base and pull-request commits into a
private bare repository, creates isolated verifier worktrees from that
repository, and runs the configured verifier commands there. The private
repository has the required objects but no remote named `origin`.

`verify_bolt_v3_dependency_direction.py` must compare the candidate tree with
the configured mainline baseline. It fails closed when `git remote get-url
origin` cannot resolve. Consequently, valid merge-queue candidates are marked
`verifier_failed` even though their required GitHub checks and approvals pass.

## Scope

Preserve the existing private-repository and isolated-worktree architecture.
Configure only each temporary verifier worktree's canonical `origin` remote
from the same normalized, fetchable URL already used to fetch pinned refs. Do
not configure remotes in the private bare repository, change the
dependency-direction verifier, weaken its fail-closed behavior, add a second
secret or fetch path, touch the operator's source checkout Git metadata, or
alter queue eligibility rules.

## Design

The operator copies the TOML bytes into a private immutable-per-run snapshot.
It reads the configured Git remote name and base only from that snapshot,
resolves that name before `ls-remote`, and uses fully qualified
`refs/heads/<base>` and `refs/pull/<number>/head` refs. The same snapshot is
passed to preflight. A missing remote, mutable-source config change, invalid
remote name, or unsafe resolved URL is terminal; no raw path, URL, default
branch, or second config source is substituted. Origin and base are
TOML-authoritative and have no CLI override.

The operator passes only an opaque SHA-256 identity of that resolved URL to the
preflight subprocess. Preflight independently resolves the configured name and
must match that identity before fetching any ref; a source-checkout Git-config
change therefore terminates instead of selecting another origin. The operator
also derives one GitHub repository slug from the same URL, supplies it as
`GH_REPO` for preflight evidence, and passes it explicitly to `gh pr comment`;
non-GitHub remotes terminate before preflight or queueing. Neither the URL nor
an alternate remote mapping crosses the subprocess boundary.

`PrivateFetchRefs.fetch_origin()` resolves the same configured remote name
against the source checkout. Its one normalized URL is used for private ref
fetches and passed with the existing alternate-object context into verifier
execution.

Before adding a verifier worktree, the temporary private repository enables
Git's worktree-specific configuration extension. The worktree records
`remote.origin.url` only in its own temporary config. A verifier can therefore
resolve `origin`, while the private bare repository remains remote-free and all
fetched refs, synthetic commits, worktree metadata, and cleanup remain isolated
from the user's checkout.

Remote setup must fail closed through the existing `PreflightError` path. The
remote URL remains temporary Git configuration and must not be added to JSON,
plain-text diagnostics, or command arguments emitted by the preflight result.
Passwords, URL query or fragment components, and userinfo on non-SSH schemes
are rejected before any Git fetch or config call. Credential-free standard SSH
and SCP usernames remain supported; preflight never retries or switches origin
sources. Configuration errors, verifier streams, progress diagnostics, and
public verifier-command JSON redact the exact normalized URL as `<remote-url>`.

## Verification

Add a regression test to `scripts/test_merge_queue_preflight.py` that runs a
real verifier inside an isolated candidate worktree. The verifier requires
`git remote get-url origin` to match the fixture's normalized remote path. The
test must fail on the current implementation with a verifier failure and pass
after the worktree installs its temporary remote configuration. Existing tests
continue to prove that the private bare repository itself has no remotes. The
regression uses a checkout-relative remote and checks the normalized worktree
value directly. Negative tests cover password, query-token, fragment-token, and
non-SSH-userinfo URLs while preserving credential-free SSH/SCP usernames. They
prove rejected URLs never reach Git and prove configuration errors, verifier
streams, progress diagnostics, and public verifier commands redact the URL.
Dedicated negative tests prove raw path-like origin inputs cannot select an
alternate fetch route. Operator tests prove raw URL/path config values and
unsafe resolved URLs never reach `ls-remote`, base fetch tests require the
fully qualified branch ref, and a mutation test proves operator and preflight
consume the same private TOML snapshot. Remote-identity tests prove the opaque
preflight identity and explicit queue repository remain bound to the
operator's one resolved URL, and that later checkout metadata drift is
terminal.

Run the targeted merge-queue preflight suite, Python syntax checks for changed
scripts, the permitted repository static gates, and the original direct JSON
preflight against PRs #1452, #1448, and #1449. Completion requires the direct
preflight and `just merge-queue` recipe to return `queue_as_one_wave` at exact
remote SHAs before the recipe posts the configured Mergify command.
