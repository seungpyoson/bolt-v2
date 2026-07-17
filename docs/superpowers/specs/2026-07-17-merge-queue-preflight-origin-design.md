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
Configure the private repository's canonical `origin` remote from the same
normalized, fetchable URL already used to fetch pinned refs. Do not change the
dependency-direction verifier, weaken its fail-closed behavior, add a second
secret or fetch path, touch the operator's source checkout Git metadata, or
alter queue eligibility rules.

PR #1439 remains outside the eligible wave until its required code-owner
approval lands.

## Design

`PrivateFetchRefs.fetch_origin()` remains the single place that resolves a
configured remote name or URL against the source checkout. After resolution,
it ensures the private bare repository has a remote named `origin` pointing to
that normalized URL. Subsequent calls use the existing in-memory cache and do
not add another remote or resolution path.

Verifier worktrees are linked to the private bare repository, so they inherit
its repository-local remote configuration. A verifier can therefore resolve
`origin` while all fetched refs, synthetic commits, worktree metadata, and
cleanup remain isolated from the user's checkout.

Remote setup must fail closed through the existing `PreflightError` path. The
remote URL remains temporary Git configuration and must not be added to JSON,
plain-text diagnostics, or command arguments emitted by the preflight result.

## Verification

Add a regression test to `scripts/test_merge_queue_preflight.py` that runs a
real verifier inside an isolated candidate worktree. The verifier requires
`git remote get-url origin` to match the fixture's normalized remote path. The
test must fail on the current implementation with a verifier failure and pass
after the private repository installs the remote.

Run the targeted merge-queue preflight suite, Python syntax checks for changed
scripts, the permitted repository static gates, and the original direct JSON
preflight against PRs #1452, #1448, and #1449. Completion requires the direct
preflight and `just merge-queue` recipe to return `queue_as_one_wave` at exact
remote SHAs before the recipe posts the configured Mergify command.
