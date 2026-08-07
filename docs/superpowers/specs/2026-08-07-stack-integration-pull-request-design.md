# Atomic Stack Integration Pull Request Design

## Status

Ready for one final design review. The disposable provider gate passed on
2026-08-07. This document does not authorize implementation, settings changes,
merge, or production activation.

This revision replaces the speculative Actions-authored pull-request and queue
path with the smaller path that the provider experiment actually proved:

1. main-owned automation validates and constructs an immutable integration branch;
2. the operator creates the integration pull request with a fixed GitHub API call;
3. the native code owner approves that exact pull-request head;
4. main-owned automation revalidates it without issuing a queue command;
5. the operator issues the only supported Mergify queue command; and
6. Mergify merges the approved original integration pull request with `merge`.

## Problem

Bolt needs both ordinary merge-queue use and atomic landing of a physical pull-request
stack. Sequentially merging the original stack pull requests is unsafe: after the
predecessor merges, GitHub retargets the successor and can dismiss its approval. The
result is a partial stack on `main` and a stranded successor.

Mergify `merge-batch` does not solve the native-review boundary. Its generated batch
pull request is the merge object, while documented injected review conditions are
evaluated on the queued originals. The batch object does not inherit those approvals.

The required identity is therefore:

- the object approved by the code owner;
- the object admitted to Mergify; and
- the object GitHub merges into `main`.

All three are one immutable integration pull request.

## Goals

- Preserve standalone pull-request capability.
- Preserve complete physical-stack capability.
- Land each standalone change or complete stack through one GitHub pull request.
- Keep the existing native code-owner ruleset as the sole merge authority.
- Keep CI advisory, with zero required status contexts.
- Use one Mergify queue and `merge_method: merge`.
- Add no bypass actor, fast-forward route, required CI gate, alternate GitHub token,
  reviewer login condition, or compatibility path.
- Fail closed when validation, provider lookup, or exact-head checks are uncertain.

## Non-goals

- Preventing a trusted writer from deliberately marking a source pull request ready
  and pursuing a native merge. GitHub cannot express that distinction without a
  required status context or a bypass-based design, both rejected here.
- Treating GitHub Actions as a general security sandbox for same-repository branches.
  The provider gate proved that a workflow introduced only on a non-default branch
  can run with requested repository permissions. This design removes merge authority
  from that identity instead of claiming the workflow cannot run.
- Making Mergify's undocumented dashboard or browser-extension behavior authoritative.
- Making advisory CI a merge veto.

## Decision summary

### One generated merge object for both capabilities

A standalone source pull request compiles to one integration pull request. A complete
physical stack of two through twenty source pull requests also compiles to one
integration pull request. Mergify sees only independent integration pull requests;
it never sees the interior of a stack.

The source pull requests remain the development and slice-review records. They stay
draft and are never queued individually. Draft status is a strong procedural default,
not an unforgeable control. A ready or directly merged source is a policy violation
that pauses integration admission and requires operator reconciliation.

### One Mergify queue

There is no stack queue and no alternate standalone queue. The one queue accepts only
non-draft pull requests against `main` whose branch is named
`integration/<40-lowercase-hex-head-sha>`.

Only a repository administrator may issue `queue`, `requeue`, or `dequeue`. The
supported operator path is a fixed `gh api` comment request that executes no
repository-controlled launcher. GitHub Actions does not have administrator permission
and its queue command is rejected.

### Native review is the only merge authority

The required reviewer is selected only by CODEOWNERS and the live `main` ruleset.
Neither the workflow nor `.mergify.yml` contains that reviewer login. Automation never
calls a review-mutating endpoint or a merge endpoint.

The repository setting that allows `GITHUB_TOKEN` to create or approve pull requests
remains disabled. The provider gate proved that, with the setting disabled, Actions
can comment and exercise declared repository permissions but receives HTTP 403 when
it attempts to create a pull request.

Same-repository branch workflows can still mutate unprotected refs or pull-request
metadata within their granted token permissions. Such mutation can cause denial of
service, but it cannot make an unreviewed head satisfy the protected `main` ruleset.
Exact-head validation and stale-review dismissal convert integration-head tampering
into a failed or superseded object, not a different merged object.

## Roles and authority

### Privileged control-plane closure

The admission control plane is a closed repository surface:

- its workflow entrypoints under `.github/workflows/**`;
- all compiler, revalidator, reconciler, and shared executable code under one dedicated
  `tools/merge-admission/**` subtree;
- `config/merge-admission.toml`, which owns every admission runtime value not supplied
  by a trusted GitHub event; and
- the fixed operator procedure under `docs/operations/merge-admission.md`.

Control-plane code may use the pinned external tools named by the construction profile
but may not import repository-local executable code from outside this closure. Adding
such a dependency first expands the closure through the governance ceremony. Queue
values remain exclusively in `.mergify.yml`; the admission TOML must not duplicate,
parse, or validate them.

`.mergify.yml` and `.github/CODEOWNERS` are separately authoritative governance
surfaces. Normal source compilation rejects any add, modification, deletion, rename,
or copy that touches those files or any path in the control-plane closure.

### Main-owned compiler

The compiler is a workflow definition loaded from current `main` through an operator
request that explicitly selects `ref: main`. It accepts a top source pull-request
number as data, re-reads all GitHub and Git state, validates the complete source
snapshot, constructs the deterministic integration commit, and may push only its
exact integration branch.

It treats source and integration trees as data. It may invoke pinned Git plumbing to
read and construct objects, but it does not execute source build scripts, local
actions, workflow definitions, or binaries.

Its token permissions are:

- `contents: write`, for the exact generated branch;
- `pull-requests: read`, for source and review state; and
- all other permissions `none`.

It does not create, approve, queue, merge, close, or delete a pull request.

### Main-owned reconciler

The reconciler is loaded from `main` on a protected-`main` push and is also manually
rerunnable from `main` for repair. It re-derives the merged manifest and may comment on
or close only exact-head source pull requests and delete only exact-head source
branches after the dependency checks in this design.

Its token permissions are `contents: write` and `pull-requests: write`; all others are
`none`. It never creates or approves an integration pull request, posts a Mergify
command, mutates a review, or calls a merge endpoint.

### Operator

The operator uses an authenticated human GitHub session for three fixed operations:

1. dispatch the compiler from `main`;
2. create the integration pull request after inspecting the compiler receipt; and
3. post `@mergifyio queue` after the exact-head review and revalidation receipt.

These commands contain the repository, operation, and object identifiers directly.
They do not invoke the repository `justfile`, a source checkout script, or any other
repository-controlled executable before contacting GitHub.

### Code owner

The code owner reviews and approves the exact integration head. Source comments and
slice reviews are useful evidence but never satisfy merge authority.

### Mergify

Mergify admits the integration pull request, injects the native review conditions as
queue conditions, and calls GitHub's normal pull-request merge API with `merge`.
GitHub's ruleset remains final even if Mergify's injected CODEOWNER representation is
incomplete.

## Source contract

A source is a human-authored draft pull request. A valid standalone source or stack
must satisfy all of the following at compilation:

1. every source pull request is open and draft;
2. every source belongs to this repository and has an exact pinned head SHA;
3. the root targets `main` and declares no `Depends-On` marker;
4. every successor targets its predecessor's head branch;
5. every successor contains exactly one full-line `Depends-On: #<number>` marker that
   names that predecessor;
6. every pinned predecessor head is an ancestor of the pinned successor head;
7. the requested top has no open successor at the snapshot time;
8. the chain contains no duplicate, cycle, fork, retargeted edge, or foreign PR;
9. the chain length is one through twenty;
10. source review threads are resolved;
11. no source is already reserved by another reserving integration object; and
12. the exact commit ownership and scope checks below pass.

Let source `i` have pinned base SHA `Bi` and head SHA `Hi`. Define its Git slice as:

`Si = Reach(Hi) - Reach(Bi)`.

The compiler reads GitHub's complete pull-request commit list `Pi`, paginating and
failing closed at GitHub's documented 250-commit limit. It requires `Pi = Si` for
every source. It also requires:

- the union of all `Si` equals `Reach(Htop) - Reach(Broot)`;
- the source slices are pairwise disjoint; and
- no commit in that union is reserved by another reserving integration object.

These checks prevent a branch-name-only stack, stale predecessor history, undeclared
side history, partial ownership, and overlapping integration objects.

## Deterministic integration object

### Manifest

The integration commit message contains a versioned canonical manifest with:

- repository identity;
- target branch;
- pinned `main` base SHA;
- root-to-top source PR numbers, base branches, base SHAs, head branches, and head SHAs;
- physical dependency edges;
- each source commit set;
- the union commit set;
- construction-profile version; and
- manifest-schema version.

The manifest uses RFC 8785 canonical JSON. The message framing is:

```text
Bolt-Integration-Manifest-Length: <decimal-byte-length>

<exact-canonical-json-bytes>
```

No run ID, API receipt order, wall-clock timestamp, review state, or transient check
state enters the manifest.

### Git object

The integration commit is a clean two-parent merge commit:

1. parent one: the pinned `main` base SHA;
2. parent two: the pinned top source head SHA;
3. tree: the clean merge result of those two commits; and
4. message: the exact manifest framing above.

The construction profile pins the Git implementation and version, SHA-1 object
format, isolated Git configuration, merge strategy/options, author and committer
identity, time derivation, locale, and timezone. Profile changes require a new
profile version in the manifest.

The compiler derives the expected 40-hex commit SHA before any remote write. The
branch is exactly:

```text
integration/<expected-commit-sha>
```

Before pushing, it recomputes the ordered parents, tree, message bytes, and commit
SHA. If the remote branch is absent, it pushes the expected object. If the branch
already points to that object, the operation is idempotent. Any other state is a
conflict and stops without force-pushing.

### Pull-request identity

The operator creates a non-draft pull request from the exact integration branch to
`main`. The pull-request body contains the immutable manifest digest and links to the
source PRs, but not transient CI or review status.

The branch name is routing metadata, not proof. Before review handoff and again before
queueing, validation must prove:

- live head equals the branch-name SHA;
- ordered parents are the manifested base and top;
- tree equals a fresh clean merge of those parents;
- message contains the byte-identical canonical manifest;
- every manifested source head is an ancestor of the integration head; and
- the integration head is not already reachable from `main` unless reconciliation is
  handling an acknowledged merge.

An appended commit, amended merge, evil merge tree, wrong parent, wrong message, or
same-name/different-object branch is rejected.

## Mergify configuration

`.mergify.yml` is the sole queue-configuration authority. This design states semantic
requirements and does not embed or mirror the file.

The implementation configuration must provide exactly one queue rule with these
properties:

- only a non-draft `integration/<40-lowercase-hex-head-sha>` pull request against
  `main` is admissible;
- the merge method is `merge`, and native branch protection is injected for queueing;
- the sole queue's `batch_size` is a fixed integer greater than one, so Mergify's
  documented automatic in-place-check precondition is false and Mergify never updates
  an integration head;
- the queue-control checkbox is disabled, and external merges reset queue state;
- `queue`, the deprecated `requeue` alias while it exists in the live schema, and
  `dequeue` require repository-administrator permission; and
- every currently supported head-mutating or copying command is denied on the
  integration namespace.

Mergify computes the deprecated `allow_inplace_checks` setting automatically. Its
documented predicate requires `max_parallel_checks == 1`, every queue's
`batch_size == 1`, and single-step queue conditions. This design deliberately makes
the `batch_size` clause false; it does not rely on setting the deprecated flag.

The actual batch size, wait time, merge identity, condition syntax, and complete
command inventory live only in `.mergify.yml`. The current command policy is fixed:
the deprecated `requeue` alias remains administrator-only while the live schema
supports it. If the provider removes that key or any required semantic cannot be
expressed, admission pauses and the configuration changes only through the governance
ceremony; implementation does not select a compatibility variant.

Mergify's live validator is authoritative for syntax. Repository code does not parse,
mirror, or independently validate `.mergify.yml`. The disposable live API established
that the supported regex-negation form uses the `-` condition modifier rather than the
invented `!~=` operator.

Mergify currently has no wildcard command default-deny. Every currently documented
head-mutating/copying command is denied on the integration namespace. A future vendor
command is an explicit operational risk, not hidden authority: a head mutation
dismisses native approval and fails exact-head validation; an alternate queue action
still cannot merge without native exact-head code-owner approval. Before any
`.mergify.yml` change, operators review both the complete command inventory and the
queue-mechanics invariant that in-place integration-head updates remain impossible.

The dashboard and browser extension are unsupported admission surfaces. Their use by
an administrator cannot bypass native approval. A resulting merge of a valid exact
integration head is content-safe but operationally unsupported; a non-manifest head
causes reconciliation to refuse source retirement and pause further admission.

## Lifecycle

### Integration object state model

An integration object is keyed by its canonical manifest and expected commit SHA. Its
state is the product of branch state, pull-request state, and queue state; none is
inferred from another. The only supported states and transitions are:

1. **Absent:** no exact branch and no pull request. Compilation may publish the exact
   branch.
2. **Branch only:** the exact branch exists without a pull request. It already reserves
   every manifested source PR and commit. The operator either creates the pull request
   or, after a failed-current-snapshot check, abandons the object by deleting the exact
   branch and rereading its absence.
3. **Open:** the exact branch and one open pull request exist. Review and queue state
   are reread independently. It may transition to queued, closed-unmerged, or
   branch-missing. A second pull request or a different head is a conflict.
4. **Queued:** the open exact pull request has provider-confirmed queue membership.
   It may transition only to merged, provider-dequeued open, closed-unmerged, or
   branch-missing.
5. **Provider-dequeued open:** the exact pull request remains open and unchanged but
   provider-confirmed queue membership is absent after a manual dequeue, lost queue
   condition, validation failure, or checks timeout. Reservations remain. The operator
   records the provider dequeue reason; an unavailable or contradictory reason pauses
   admission. The object remains in this state until the operator either abandons it or
   starts one new queue operation for that dequeue event after full current source,
   object, native-review, and queue-rule revalidation succeeds. Transport recovery for
   that operation follows the ambiguous-comment rule below and does not authorize a
   second semantic requeue. Confirmed membership returns it to queued. Automation never
   requeues it implicitly. Closure transitions to closed-unmerged; branch loss
   transitions to branch-missing.
6. **Closed unmerged:** the same pull request may be reopened only when the exact branch
   still exists and full source and object revalidation succeeds; after reopening it
   requires current native review before queueing. Otherwise it is dequeued if
   necessary, marked abandoned with a durable comment, its exact branch is deleted,
   and its reservations are released.
7. **Branch missing with an open or closed-unmerged pull request:** admission pauses.
   Explicit operator recovery may restore only the byte-identical expected commit to
   the same branch and then resume open, queued, provider-dequeued open, or
   closed-unmerged as determined by fresh pull-request and queue reads; otherwise the
   object is closed and abandoned. A replacement head or replacement pull request is
   forbidden.
8. **Merged, reconciling:** the protected `main` merge commit is the oracle. Reservations
   remain until manifested-snapshot retirement and required successor normalization
   finish, then transition to snapshot-retired. Mutable source-pull-request closure is
   not a prerequisite; contradictory merge or source evidence pauses reconciliation.
9. **Snapshot retired:** every manifested head is confirmed reachable from `main` and
   durably recorded. Each source is either exact-head retired or explicitly recorded as
   follow-up-open at an advanced head, all required successor normalization is complete,
   and a durable snapshot-retired record is confirmed on the integration pull request.
   The old manifest's PR-number and commit reservations are then released. An advanced
   source is eligible for fresh compilation and acquires new reservations only when its
   new exact integration branch is published.
10. **Abandoned:** the pull request is closed if it exists, queue membership is absent,
   the integration branch is absent, and reservations are released. A later identical
   dispatch may restore the same content-addressed branch and reopen the same pull
   request, or create the first pull request if none ever existed; it never creates a
   second live object.

Any same-name/different-object branch, duplicate pull request, overlapping exact branch,
or disagreement among branch, pull-request, manifest, and queue state is a conflict
that pauses admission without releasing reservations. The repository-wide writer
serialization makes the first successfully published exact branch the owner; any
pre-existing ambiguous overlap pauses for repair rather than inventing a winner.

Every mutation has read-after-write resolution. For an ambiguous queue comment, the
operator rereads the exact issue comment, Mergify check/event state, and queue
membership. Confirmed membership is success; confirmed absence permits one retry;
uncertain or contradictory state pauses without posting another command.

### 1. Compile

The operator dispatches the compiler workflow explicitly from `main` with the top
source PR number. The compiler:

1. rereads the live source graph;
2. validates the source contract and reservations;
3. fetches the required Git objects;
4. builds and verifies the deterministic integration commit;
5. pushes only the exact integration branch; and
6. publishes a run summary containing the manifest digest, expected head, source
   list, and branch.

Failure before the exact push creates no pull request and no queue operation. The
successful exact push enters branch-only state and starts reservations immediately;
reservation does not wait for pull-request creation.

### 2. Create the integration pull request

The operator inspects the compile receipt and uses a fixed `gh api` request to create
the pull request. The repository setting preventing `GITHUB_TOKEN` pull-request
creation and approval remains off.

The create call follows the state model above. An exact existing open pull request is
success. Branch-only state resumes creation. Closed-unmerged and missing-branch states
take their single defined reopen/restore-or-abandon transition. A
same-branch/different-head object is a conflict. An ambiguous response is resolved by
rereading branches and pull requests before retry.

### 3. Review

GitHub requests the CODEOWNER through the normal native mechanism. Advisory CI runs
as visible evidence under the repository's existing workflow policy and remains
non-required. The code owner approves the exact integration head after reviewing the
combined diff and source references.

### 4. Revalidate

Before queueing, the operator dispatches the read-only validation operation from
`main`. It rereads:

- integration PR author, base, draft/open state, and exact head;
- manifest bytes, parents, tree, and expected SHA;
- source PR heads, topology, completeness, and reservations;
- native review decision, latest-push approval, and unresolved threads; and
- whether the integration head remains outside `main`.

Any mismatch stops. The validation workflow has no permission or code path to queue,
review, or merge.

### 5. Queue

After a successful current receipt, the operator posts exactly:

```text
@mergifyio queue
```

through a fixed GitHub issue-comment API request. Mergify rechecks its queue
conditions and injected native review conditions. The bot-authored form is denied by
the administrator sender restriction. The operator applies the state model's
read-after-write rule before treating a timeout or transport error as permission to
post another queue command.

### 6. Merge

Mergify may batch independent integration pull requests for speculative checking,
but `merge_method: merge` merges each approved original integration pull request.
Batch splitting therefore occurs only between complete integration objects and can
never split a source stack.

Mergify must never update an integration pull request in place. The `.mergify.yml`
requirement that the sole queue's `batch_size` remain greater than one keeps the
documented automatic in-place-check predicate false. The exact approved head is
recorded before queueing and must remain unchanged through merge, including when the
integration pull request is behind `main` at the front of the queue.

GitHub performs the normal protected merge. The expected result is one new
first-parent transition on `main`, whose merge commit has the prior `main` tip as
parent one and the unchanged exact approved integration head as parent two.

### 7. Reconcile and retire sources

After observing the protected merge commit on `main`, the reconciler:

1. verifies the exact integration head is its second parent and reachable from main;
2. verifies every manifested source head is reachable from main;
3. writes a durable source-PR comment containing the integration PR and merge SHA;
4. leaves a source open if its live head advanced after compilation and records the
   manifested head as landed with the current head as follow-up work;
5. otherwise accepts GitHub's indirect merged state or closes the exact-head draft as
   landed; and
6. deletes a source branch only when its remote tip still equals the manifested head
   and no open dependent PR still targets it.

Retirement applies to the immutable manifested snapshot, not necessarily to the mutable
source pull request. Before releasing the old manifest's reservations, every manifested
source must have one durable disposition: exact-head retired, or snapshot-landed with an
advanced head left follow-up-open. An advanced non-root source is normalized with the
same guarded base-and-marker transaction described below only when its consumed
predecessor has no advanced live follow-up. If that predecessor also advanced, the
dependency is preserved as fresh stack topology. Predecessor branches remain until all
dependent advanced or late successors have been normalized or confirmed as part of
that fresh topology. Immediately before terminalization, the reconciler rereads every
live source head and reruns any disposition whose recorded advanced head changed. Once
every disposition and required normalization is current and confirmed, the reconciler
writes and rereads the integration pull request's durable
snapshot-retired record, enters that terminal state, and releases only the old
manifest's reservations, allowing every advanced source to compile again.

Retirement is idempotent. Ambiguous comment, close, branch-delete, disposition, or
reservation-release responses are resolved by rereading GitHub state before retry.
Transport failure is never treated as success.

## Concurrency and snapshot semantics

Every integration object other than absent, snapshot-retired, or abandoned reserves
its source PR numbers and exact commit IDs from branch publication until its terminal
record is confirmed. Compilation rejects overlap. The compiler and reconciler use the
same repository-wide non-cancelling Actions concurrency group so only one admission
mutation runs at a time. GitHub may replace an older pending run with a newer pending
run; a cancelled or coalesced run is therefore recorded as incomplete and must be
rerun, never treated as successful compilation or retirement.

The manifest is a snapshot. Source changes after compilation cannot change the
approved integration object. Prequeue revalidation normally supersedes an object when
its source heads or topology changed. If a source change races after revalidation and
the merge wins, `main` still receives exactly the reviewed integration snapshot; the
advanced source pull request is not closed. The manifested snapshot is retired under
the guarded disposition and normalization rules above, its old reservation is
released, and the advanced head remains eligible follow-up work.

A newly opened successor after compilation is not part of the manifested stack and
remains open. The same recovery class includes an advanced manifested successor whose
consumed predecessor has no advanced live follow-up. After the manifested predecessor
lands, reconciliation normalizes either successor as one guarded, idempotent transaction
only when its head is unchanged, its base and single canonical `Depends-On` marker still
name that predecessor, no live manifest other than the merged object being retired
reserves it, and the consumed predecessor head is reachable from `main`.

The transaction makes the successor a new root by setting its base to `main` and
removing that exact dependency marker. After each provider write it rereads both base
and body. A partial update enters normalization-pending state, pauses admission, and
reruns only the missing half after all guards are rechecked. The predecessor branch is
retained until both fields and the unchanged successor head are confirmed. Any
different body, base, head, reservation, or reachability result pauses for operator
repair without deleting the predecessor branch.

## Failure and recovery

- **Validation or lookup failure:** no push, PR creation, or queue command.
- **Conflicting integration branch:** stop; never force-push.
- **Ambiguous PR creation:** reread branch and PR state before retry.
- **Head change after approval, including a queue-initiated update:** native
  stale-review dismissal and latest-push review block merge; revalidation rejects the
  object, Mergify is paused, and the queue configuration is treated as violating the
  no-in-place-update invariant.
- **Bot queue attempt:** command restriction rejects it; no queue entry.
- **Provider dequeue or queue failure:** `main` remains unchanged; record whether the
  provider reported manual dequeue, condition loss, validation failure, or timeout;
  retain reservations and reuse only the same exact reviewed head after full current
  revalidation and one operator requeue operation, or abandon it deterministically.
- **Ambiguous merge response:** Git reachability and the protected `main` merge commit
  are the oracle, not the transport response.
- **Partial retirement:** resume idempotently; never close or delete an advanced head.
- **Coalesced Actions run:** report incomplete work and rerun the exact operation after
  the active writer exits.
- **Invalid direct merge:** pause admission and refuse retirement until reconciled.

No recovery uses `--admin`, a bypass actor, a required CI context, an alternate token,
or restoration of the retired sequential source-PR route.

An exact-head integration pull request merged through GitHub's native API by any
permitted caller after the required code-owner approval is content- and
authority-equivalent but operationally unsupported. Mergify resets its state and
reconciliation proceeds from the same merge oracle. This is not a second configured
path. A caller still cannot merge a different or unreviewed head through the protected
branch.

## Workflow and queue maintenance

Normal source compilation rejects the complete privileged control-plane closure,
`.mergify.yml`, and `.github/CODEOWNERS`, including additions, deletions, nested paths,
renames, and copies. Those executable/configuration surfaces use a bounded maintenance
ceremony:

1. obtain explicit user authorization;
2. pause Mergify and require an empty queue and no integration in flight;
3. disable repository Actions and wait for queued/running jobs to become terminal;
4. merge one non-draft, exact-head code-owner-approved maintenance pull request under
   the unchanged native ruleset, without admin bypass;
5. inspect the exact merged tree and live settings;
6. re-enable Actions only after confirming the default-branch workflow inventory and
   the closed control-plane dependency/configuration set; and
7. resume Mergify only after its configuration validates, its command inventory is
   complete, in-place integration-head updates remain impossible, and the queue is
   empty.

This is governance maintenance, not a product merge path. Product, strategy, runtime,
dependency, or trading changes are forbidden in that pull request. Policy-only prose
outside the control-plane closure remains normal exact-head reviewed documentation;
it has no executable admission authority.

## Provider-gate evidence

Disposable private repository:
`seungpyoson/bolt-merge-provider-gate-20260807`.
Durable evidence record: issue `#1` in that repository. The repository remains
available through final design review.

### GitHub Actions identity

- A workflow existing only on branch `probe/branch-workflow` ran successfully at head
  `a9c43527bfcaa4f7ffd14f963326a2adf492def6`, run `31170886687`.
- GitHub granted its declared contents, issues, and pull-request write permissions.
- It posted comment `5215968634` as `github-actions[bot]`.
- Its pull-request creation attempt failed with HTTP 403 because the repository
  create/approve switch was off.
- Resulting Actions-created pull requests: zero.

### Cache boundary

- Main-ref `workflow_dispatch` run `31170924792` created default-branch cache
  `6424596041`, proving that context is cache-writable.
- Human PR `#2` triggered `pull_request_target` run `31170961202`.
- The low-trust job executed the exact PR head, while cache save was denied with
  `token has no writable scopes`; the cache inventory did not change.

This design does not add an exact-head advisory executor. Existing advisory workflows
remain outside merge authority. Any future hostile-content runner must separately
prove its own token, secret, OIDC, cache, and runner isolation.

### Mergify access and syntax

- GitHub granted the existing Mergify installation access to the disposable
  repository without removing `bolt-v2` access.
- The live Mergify API rejected `head !~=` as an invalid operator.
- The supported `-head ~=` form passed `mergify config validate` with exit code zero.
- Mergify produced a successful `Configuration changed` check after access was added.
- The live schema command keys were `backport`, `copy`, `dequeue`, `queue`, `rebase`,
  `refresh`, `requeue`, `squash`, and `update`.

### Bot denial and human admission

- Integration PR `#4` exact head:
  `50d0d96af54458555d7bb4a5bc55d2657d4283fa`.
- Actions run `31172422251` posted queue comment `5216198267` as
  `github-actions[bot]`.
- Mergify explicitly replied that the command was disallowed because
  `sender-permission >= admin` was false; the queue remained empty.
- `sp-reviewer`, node `U_kgDOEZMFhA`, approved that exact head.
- Operator queue comment `5216291194` entered rule `default`.

### Protected merge

- Mergify injected and satisfied resolved-thread, latest-push-approval, and approved-
  review conditions.
- It skipped speculative checks because the PR was already up to date.
- It merged the original integration PR with `merge`.
- Protected-main merge commit:
  `062af622599ab0ffe82b2d4559e8c691ae32ec74`.
- Ordered parents: previous main
  `f7f87db9724a390b95c1009a5a4ba3db46e7ac63`, exact approved head
  `50d0d96af54458555d7bb4a5bc55d2657d4283fa`.
- First-parent transition count: exactly one.
- Final Mergify queue: empty.
- Ruleset `20549350` stayed active and merge-only with `bypass_actors: []`.

The provider pull request was already up to date, so this gate did not exercise a
behind-base queue entry. The implementation experiment below must prove that case and
confirm the approved integration head remains unchanged.

## Implementation evidence plan

Implementation must not begin until this design receives final external approval and
an owning GitHub issue defines the implementation slice.

Before implementation review, evidence must include:

### Local behavior tests

- standalone and two-through-twenty-member stack compilation;
- malformed marker, foreign PR, cycle, duplicate, retarget, missing predecessor,
  non-ancestor head, over-depth, unresolved-thread, and open-successor rejection;
- `Pi = Si`, undeclared side-history, 250-commit, and overlap rejection;
- byte-identical manifest and Git object reproduction in two clean environments;
- exact branch idempotency and conflicting-branch failure;
- branch-only reservation, exact-branch/no-PR recovery, closed-unmerged
  reopen-or-abandon, missing-branch/open-PR recovery, and terminal reservation release;
- provider-driven dequeue after condition loss, validation failure, and timeout,
  including reason capture, reservation retention, full revalidation, one operator
  requeue, and deterministic abandonment;
- appended, amended, wrong-parent, wrong-tree, and wrong-message head rejection;
- ambiguous push, PR, and queue-comment state reconciliation;
- source drift, including a merge-winning advanced source whose manifested snapshot is
  retired, whose old reservation is released, and whose advanced head compiles again,
  plus a second advance during reconciliation that forces disposition reread;
- successor race and partial base/body normalization behavior, including preservation
  of a fresh advanced stack when its predecessor also advanced;
- compiler rejection before any branch write for every change touching
  `.github/workflows/**`, `.mergify.yml`, `.github/CODEOWNERS`, or any path in the
  privileged control-plane closure, including nested paths, renames, and copies; and
- exact-head, idempotent source retirement and branch deletion ordering.

Tests verify behavior through fixtures and fake provider responses; they do not scan
implementation source text.

### Static evidence

- workflow syntax and permissions;
- supported operator dispatch pins the reviewed workflow definition to `main`, and a
  non-main invocation cannot create, approve, or queue an integration pull request;
- no review-mutating or merge API call in automation;
- no alternate GitHub token or required status context;
- exact implementation `.mergify.yml` live validation, semantic confirmation that
  in-place checks are impossible, and confirmation that no repository file mirrors,
  parses, or validates it; and
- targeted text review of the narrow `AGENTS.md` governance amendment.

### Implemented disposable experiment

Repeat the proven provider gate with the exact implemented workflow and candidate
configuration. Add one standalone and one two-member physical stack. Record source
heads, manifest bytes, integration heads, native approvals, queue comments, Mergify
events, merge commits, first-parent counts, retirement states, and final queue state.
Add two negative/edge arms: a governance-surface source is rejected before any
integration branch write, and an approved integration pull request deliberately made
behind `main` reaches merge without its head SHA changing between approval and merge.
Also exercise both new lifecycle boundaries: one provider-driven dequeue is observed,
with reservations retained while dequeued, then revalidated and either requeued or
abandoned with terminal release; and one source advance that wins the merge race
reaches snapshot retirement, releases the old reservation, and successfully compiles
the advanced head as new follow-up work.

All evidence is published before disposable-state deletion. A failed provider fact
blocks activation; it is not patched with a bypass or fallback route.

## Coordinated cutover

The implementation changes workflow, queue, compiler, and governance surfaces, so it
uses the bounded maintenance ceremony above. Before its merge:

- the live Mergify queue is empty;
- no stack or integration experiment is in flight;
- every legacy open source PR is explicitly incorporated, converted to the new source
  model, or closed;
- the current ruleset and CODEOWNERS identity are verified live; and
- the Actions create/approve switch is verified off.

After merge, activation verifies the exact `main` versions of the compiler,
`.mergify.yml`, CODEOWNERS, and governance text. Any mismatch keeps admission paused.

Controlled activation is standalone first, then one two-member stack. Each must show:

1. one deterministic integration PR;
2. exact-head code-owner approval;
3. denied bot queue command in the controlled negative arm;
4. accepted operator queue command;
5. an integration head unchanged from exact-head approval through merge;
6. one protected-main first-parent transition;
7. exact source-head reachability and retirement; and
8. an empty final Mergify queue.

## Implementation scope and governance

One implementation issue and pull request may contain only this admission slice:

- the main-owned integration compiler and read-only revalidator;
- deterministic manifest/Git construction;
- the closed `tools/merge-admission/**` implementation subtree,
  `config/merge-admission.toml`, and fixed operator command documentation;
- source retirement reconciler;
- behavior tests;
- replacement of the legacy direct source-queue helper and its fake-`gh` harness;
- the one-queue `.mergify.yml` change; and
- the narrow `AGENTS.md` amendment authorizing generated integration carriers and the
  governance-maintenance ceremony.

No product, strategy, runtime, dependency, deployment, readiness, or trading change is
in scope. Remaining work must be tracked by separate issues.

## Alternatives rejected

### Two Mergify queues or `merge-batch`

Rejected because queue classification was not mechanically exclusive and the
generated batch PR was not the native-reviewed object.

### Sequential original source merges

Rejected because predecessor landing can retarget a successor and dismiss its
approval, reproducing the incident.

### Fast-forward or bypass actor

Rejected because it would transfer merge authority away from the unchanged native
code-owner ruleset.

### Actions-authored integration PR and queue command

Rejected by provider evidence. Same-repository branch workflows can receive the same
`github-actions[bot]` identity and repository permissions. The create/approve switch
therefore stays off, and Mergify deliberately denies bot-authored queue commands.

### Required CI admission check

Rejected because the approved lean-CI state has zero required contexts. CI remains
visible evidence, never merge authority.

### Direct standalone source admission

Rejected because a standalone PR and a stack root are indistinguishable to Mergify:
both are human PRs targeting `main` with no predecessor marker. Wrapping both classes
in the same integration object removes that ambiguity.

## Authoritative references

- Mergify conditions: <https://docs.mergify.com/configuration/conditions/>
- Mergify command restrictions: <https://docs.mergify.com/commands/restrictions/>
- Mergify queue rules: <https://docs.mergify.com/merge-queue/rules/>
- Mergify merge strategies: <https://docs.mergify.com/merge-queue/merge-strategies/>
- Mergify ruleset compatibility:
  <https://docs.mergify.com/merge-queue/github-rulesets/>
- GitHub pull-request commit API:
  <https://docs.github.com/en/rest/pulls/pulls#list-commits-on-a-pull-request>
- GitHub Actions token behavior:
  <https://docs.github.com/en/actions/concepts/security/github_token>
- GitHub Actions repository settings:
  <https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository>
- Git `merge-tree`: <https://git-scm.com/docs/git-merge-tree>
- Git `commit-tree`: <https://git-scm.com/docs/git-commit-tree>
- Git `rev-list`: <https://git-scm.com/docs/git-rev-list>
- RFC 8785: <https://www.rfc-editor.org/rfc/rfc8785>
