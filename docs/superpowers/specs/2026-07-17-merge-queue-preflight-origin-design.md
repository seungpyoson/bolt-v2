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

PR #1439 remains outside the eligible wave until its required code-owner
approval lands.

## Design

`PrivateFetchRefs.fetch_origin()` remains the single place that resolves a
configured remote name or URL against the source checkout. The normalized URL
is passed with the existing alternate-object context into verifier execution.

Before adding a verifier worktree, the temporary private repository enables
Git's worktree-specific configuration extension. The worktree records
`remote.origin.url` only in its own temporary config. A verifier can therefore
resolve `origin`, while the private bare repository remains remote-free and all
fetched refs, synthetic commits, worktree metadata, and cleanup remain isolated
from the user's checkout.

Remote setup must fail closed through the existing `PreflightError` path. The
remote URL remains temporary Git configuration and must not be added to JSON,
plain-text diagnostics, or command arguments emitted by the preflight result.
HTTP(S) URLs containing userinfo and URLs containing passwords are rejected
before any Git call. Configuration errors and verifier streams redact the exact
normalized URL as `<remote-url>`.

## Verification

Add a regression test to `scripts/test_merge_queue_preflight.py` that runs a
real verifier inside an isolated candidate worktree. The verifier requires
`git remote get-url origin` to match the fixture's normalized remote path. The
test must fail on the current implementation with a verifier failure and pass
after the worktree installs its temporary remote configuration. Existing tests
continue to prove that the private bare repository itself has no remotes. The
regression uses a checkout-relative remote and checks the normalized worktree
value directly. Negative tests prove credential-bearing URLs never reach Git,
configuration errors do not expose the URL, and failed verifier streams redact
the URL in both JSON and plain diagnostics.

Run the targeted merge-queue preflight suite, Python syntax checks for changed
scripts, the permitted repository static gates, and the original direct JSON
preflight against PRs #1452, #1448, and #1449. Completion requires the direct
preflight and `just merge-queue` recipe to return `queue_as_one_wave` at exact
remote SHAs before the recipe posts the configured Mergify command.
