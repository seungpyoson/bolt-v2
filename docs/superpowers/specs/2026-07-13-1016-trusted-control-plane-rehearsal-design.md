# #1016 D4 disposable trusted-control-plane rehearsal design

Related authority:

- [GitHub issue #1016](https://github.com/seungpyoson/bolt-v2/issues/1016)
- [Atomic verifier replacement design](2026-07-12-issue-1016-atomic-ci-verifier-rewrite-design.md)
- [D3 trusted authority control-plane proposal](2026-07-13-1016-trusted-control-plane-design.md)
- [Program ledger](../../ci/1016-program-ledger.md)

## Status and decision boundary

This document designs a disposable rehearsal only. It does not authorize implementation, installation in the production repository, production secrets, production GitHub or Mergify changes, publication of a production check, the precursor, the atomic replacement, or the final semantic verifier.

The rehearsal answers the external-control questions that documents and mocks cannot prove. It exercises real GitHub App identity, GitHub rulesets, Mergify queue behavior, Checks API timing, and the D3 append-only publication protocol in an isolated test repository. A policy-free fixture engine supplies predetermined signed results; it is not a prototype or alternate implementation of the final verifier.

The rehearsal creates no new human approval rule. Existing repository governance applies to changes committed here. No quorum, hardware key, backup person, ceremony approver set, or additional owner gate is introduced.

## Purpose

D4 has four outputs:

1. a pass/fail live-proof matrix for the D3 assumptions that only GitHub, Mergify, and delayed API behavior can settle;
2. an independently reconstructable evidence bundle for every scenario;
3. measured distributions for successful-path time, retry behavior, publication visibility, queue invalidation, resource use, and cost; and
4. proposals for the successful-ceremony target, retry count/time budgets, and pre-precursor abort threshold that the owner may approve or reject before any production ceremony.

D4 does not prove semantic policy parity, select the final verifier corpus, authorize production reliance, or make the D5 risk decision. It must not be used as evidence that candidate policy is correct.

## Disposable boundary

Every rehearsal resource is isolated from the production repository and removable without changing production state:

- two dedicated private repositories under the same sandbox owner: the synthetic rehearsal target and an inert anchor containing no workflows, configuration, automation, or branches beyond the provider minimum;
- a dedicated selected-repository installation of the proposed authority App, restricted to exactly the target and anchor; the anchor exists only to keep the installation provider-valid while H9 removes the target;
- a Mergify configuration scoped only to the test repository;
- an ephemeral authority-service deployment, fixture launcher, signing keys, append-only state, and immutable evidence store with rehearsal-only identities;
- synthetic commits, pull requests, queue heads, manifests, engine results, and authority epochs that cannot be accepted by production; and
- a distinct rehearsal check name that cannot satisfy any production required check.

Repository node ID, App installation ID, key roots, authority epoch, context, artifact digests, and proof domains are environment-bound. Copying a rehearsal record, key, check, artifact, or database into production must fail identity validation. Rehearsal credentials have no access to the production repository.

Cleanup deletes or disables all executable rehearsal resources after evidence retention is proven. Signed receipts and redacted immutable evidence remain historical proof; they have no publication capability.

## Smallest sufficient topology

The rehearsal uses one synthetic target, one inert anchor, one authority App installation selected for exactly those two repositories, one target-only Mergify integration, and one logical authority service:

1. **Target and anchor repositories:** the target has protected `main`, synthetic pull requests, minimal `.mergify.yml`, Freeze and Merge Protections as required, and no production source. The empty anchor is never queued, evaluated, mutated by a scenario, or used for a check; it exists only because GitHub rejects removing the last selected repository from an installation.
2. **GitHub App:** repository and pull-request metadata read, Contents and required ruleset state read, plus Checks write, installed on exactly target and anchor. The authority service rejects the anchor repository node ID for invocation, evaluation and publication. Live proof records both repository IDs/full names and demonstrates that target removal leaves only the anchor and makes target publication impossible.
3. **Authority service:** the smallest disposable implementation of the D3 state machine needed for the matrix. It observes exact state, invokes the fixture launcher, validates canonical protocol and identity, appends state, and is the sole check publisher.
4. **Fixture launcher:** returns predeclared, purpose-bound signed allow, deny, malformed, timeout, and classification fixtures. It contains no CI rule definitions and does not inspect repository policy to choose an answer.
5. **State and evidence:** one conditional append-only log with monotonic sequence and uniqueness enforcement, plus one content-addressed immutable evidence store. There is no mirror database, fallback publisher, queue worker, or second verifier.
6. **Fault boundary:** a controllable proxy/test seam around Checks API create responses and subsequent reads. It may delay, drop, duplicate delivery to the service, or hide observations; it cannot forge GitHub's App identity or alter GitHub itself.

Real GitHub and Mergify behavior is required where publisher qualification, ruleset enforcement, queue construction, bypass, Freeze, invalidation, or merge-time re-evaluation is under test. Local simulation is acceptable only for deterministic protocol, append, crash, replay, and audit-corruption tests and cannot substitute for a live-matrix row.

## Fixed protocol ownership

The fixture result includes the required boolean `authority_surface_change`. The fixture engine owns that field exactly as the final protected-base semantic engine will. The authority service may validate its presence, canonical type, signature, invocation binding, and admitted key version; it must carry the value unchanged and must not derive, override, or reinterpret it.

Two fixture classes prove the split:

- `authority_surface_change: false` permits a synthetic ordinary steady-state proof to advance the test base only after all structural checks pass.
- `authority_surface_change: true` is non-authorizing in the ordinary path and is admitted only through the synthetic staged activation purpose.

Changing only publisher configuration must never change this semantic field. Changing the field without a valid fixture-engine signature must fail closed.

## Hypotheses and required observations

Each hypothesis is mandatory. A failed, ambiguous, or non-reproducible observation is a D4 no-go, not a documentation exception.

### H1 — distinct App publisher identity

The ruleset can require the rehearsal authority context from the exact App integration. A success with the same name from GitHub Actions, another App, a commit status, or an unqualified producer does not satisfy the rule. The receipt records the check-run App/installation identity and the rule's qualified binding.

### H2 — Mergify binding and `exempt`

With the App-qualified context required and Mergify configured with `bypass_mode: exempt`, Mergify does not inject or wait for the protected ruleset on its own execution path, while the separately configured queue requirement still waits for the exact App-owned check. Native/direct merging remains blocked. Freeze and exact exclusions behave as configured and are re-evaluated before merge.

### H3 — exact proof-head, base, and constituent binding

Authority for one exact proof-head SHA, ordered parents/tree, protected-base SHA, and constituent set authorizes only that tuple. Base movement, constituent movement, reordered or mixed batches, unexpected queue entries, configuration changes, or a regenerated proof head invalidate the old attempt. An old success cannot authorize the new head.

### H4 — same-name spoof rejection

Same-name success from the wrong publisher, wrong installation, wrong SHA, ordinary PR head, feedback context, or commit-status API remains non-authorizing. The authority service also records it as conflicting evidence and never adopts it.

### H5 — append-only idempotency and replay closure

Duplicate webhooks, deliveries, service replicas, process restarts, restored snapshots, repeated nonces, same-SHA reruns, concurrent reservations, and old binaries/keys cannot create a second reservation or authorizing terminal record for one authority domain. A successful retry is a fresh full proof for its permitted regenerated proof head, not conversion or reuse of an earlier result.

### H6 — delayed create/read visibility

If a Checks API create response is lost or ambiguous, or the created run is hidden from reads beyond every observation window exercised, the domain enters `PUBLISHING_UNCERTAIN`. The service makes no second create call, publishes no replacement domain, and does not treat elapsed time or retry-budget exhaustion as proof of absence. A delayed matching `external_id` is adopted exactly once and validated against current state.

### H7 — invalidation before replacement

Before an uncertain or stale proof domain is superseded, durable live evidence proves the exact old head is dequeued, invalidated or superseded, absent as the current queue/batch head, and unable to merge under current ruleset, Mergify, Freeze, base, and constituent state. The service queries for a delayed old check both before and after invalidation. If inability to merge is ambiguous, replacement remains prohibited indefinitely.

### H8 — steady-state and activation separation

An ordinary synthetic change with signed `authority_surface_change: false` can follow the steady-state path. A signed `true` result cannot. An activation-purpose result applies only to its reserved staged version and exact proof head. Neither candidate metadata nor publisher configuration can select or change the classification.

### H9 — stop-only emergency disable

An independent existing GitHub owner-user session removes the target from the selected-repository authority installation with `DELETE /user/installations/{installation_id}/repositories/{target_repository_id}`. Preflight must prove the principal, exact installation, target and anchor identities and live provider support. The live call must return `204`; `422`, ambiguity, or any other result is `NO_GO`. Re-query must prove the installation contains only the inert anchor, the target no longer exposes the installation, and new target check publication is impossible. This stop-only control cannot create success, approve, restore, rotate, publish to the anchor, or provide another publisher; no credential or approval gate is added to the service.

### H10 — complete audit reconstruction

From retained records, a reviewer can reconstruct every trigger, observation, invocation, signed fixture result, state transition, API attempt, delayed observation, publication, merge or block outcome, key/control change, and cleanup action. GitHub-side bypass audit absence under `exempt` is compensated by the signed append-only evidence. Missing, reordered, altered, forked, or expired evidence is detected and fails the reconstruction.

## Scripted scenario matrix

Every scenario begins from a recorded clean epoch and ends in a terminal receipt. Scenarios use fresh synthetic branches and proof domains; no row reuses a prior success.

| Scenario | Action | Required result |
| --- | --- | --- |
| Baseline allow | Queue one permitted PR; fixture emits signed allow/`false` | One App-qualified success on the exact proof head; expected synthetic main advances; terminal receipt matches |
| Repeated full ceremony | Repeatedly execute admission lock, post-lock refresh, final controls, dormant precursor analogue, promotion/tombstone, nonpublishing canary, activation, terminal proof, and cleanup from a clean baseline | Every phase completes in order; only activation publishes authority; each phase and total duration are measured; cleanup restores the disposable baseline |
| Pre-precursor abort/restore | At each relevant point after admission lock and before precursor-analogue merge, force the measured abort threshold to be breached and execute the reviewed restore sequence | Baseline controls and merge availability are restored, no trusted check is published, no promotion/tombstone occurs, and exact before/after evidence is retained |
| Baseline deny | Fixture emits signed deny | No authority success and no merge; denial and exact tuple retained |
| Same-name spoof | Publish same context through Actions, another App, status API, and ordinary PR head | All remain non-authorizing; exact App/proof-head result still required |
| Wrong identity | Change App installation, repository, context, key version, purpose, or epoch | Attempt terminates without publication |
| Base movement | Advance protected test `main` before reservation, before create, after create, and before merge | Old domain cannot authorize; queue invalidates/regenerates or remains blocked |
| Constituent/queue movement | Add, remove, reorder, or mutate a constituent; introduce a mixed or unauthorized batch | Old attempt terminates; no stale success merges |
| Mergify self-change | Change `.mergify.yml`, admission route, injection, bypass, Freeze, exclusion, or Merge Protections state at each publication cut point | Prior proof invalidates; no silent continuation |
| `exempt` matrix | Compare exact `always`, `pull_request`, and `exempt` observations where safely supported in test state | Recorded behavior proves the selected final mode; any injection or bypass ambiguity is no-go |
| Duplicate delivery/race | Duplicate webhook/check-suite/rerequest events; race service instances and reservations | One authority domain, one create attempt, one terminal outcome |
| Lost create response | Allow create, drop response, delay read visibility | `PUBLISHING_UNCERTAIN`; no repeated create or replacement while old head is eligible; delayed run adopted |
| Create failure before acceptance | Prove GitHub did not accept the request | Non-authorizing infrastructure result; retry only under unchanged identity and approved rehearsal rules |
| Uncertain invalidation | Hold an uncertain domain, dequeue and invalidate old head, query before/after, then regenerate | Replacement allowed only after durable inability-to-merge proof; delayed old success remains harmless |
| Retry allowlist | Inject stale read, network/API timeout, cancelled run, blocked extra queue entry, and permitted proof-head regeneration | Only the enumerated unchanged-identity cases retry with fresh lineage evidence |
| Terminal cases | Inject merits failure, malformed protocol, signature/epoch mismatch, unauthorized constituent, state movement, and budget exhaustion | Terminal; no fallback, conversion, reuse, or publication |
| Classification ownership | Run signed `false`, signed `true`, unsigned mutation, and publisher-side override attempts | Only engine-signed value is carried; ordinary `true` path blocks |
| Stop-only disable | From the external owner-user session remove the target repository while the inert anchor remains | Provider returns 204; only anchor remains; target publication fails; no success/epoch/bootstrap transition becomes available |
| Audit damage | Delete, alter, reorder, fork, roll back, hide a blob, or expire a test copy | Reconstruction detects damage; authority never relies on the damaged chain |
| Cleanup | Disable installation, revoke/delete rehearsal credentials, remove rules/config/resources, then probe access | No executable publisher or repository access remains; retained evidence still verifies |

The stale-proof scenarios must cover every D3 cut point: before reservation, before the create call, during arbitrarily delayed create/read visibility, after lost response, after check creation, and immediately before merge. Success in easier positions does not compensate for an unproven cut point.

## Measurement method

D4 records raw observations before proposing any number. It does not invent a budget in this design.

For each clean successful rehearsal and each permitted infrastructure fault, capture monotonic and externally observed timestamps for:

- admission-lock activation equivalent, post-lock refresh, final-control installation, dormant precursor-analogue merge, promotion/tombstone, nonpublishing canary, activation queue admission and proof-head creation, engine start/finish, terminal re-queries, reservation, create call, first matching read, post-publication validation, merge eligibility, merge observation, terminal proof, and cleanup;
- API calls, retries, queue regenerations, duplicate deliveries, reconciliation polls, rate-limit consumption, runner/service resource use, evidence volume, and direct cost;
- dependency outages and observed recovery; and
- operator elapsed time for the reviewed pre-precursor abort/restore sequence.

The evidence report separates:

1. **successful-path duration:** lock-equivalent start through successful cleanup, including the mandatory post-lock precursor refresh analogue;
2. **retryable-noise duration/count:** per enumerated class while every immutable identity remains unchanged;
3. **publication uncertainty:** observation only, never a basis for abandonment while the head remains eligible;
4. **pre-precursor abort duration:** time needed to detect threshold breach and restore the pre-ceremony state; and
5. **terminal tail:** explicitly unbounded after precursor analogue, not folded into a misleading maximum.

The full happy-path analogue is repeated from a clean baseline so the report derives both per-phase and total distributions rather than extrapolating from isolated rows. Threshold-breach abort/restore runs at the relevant pre-precursor cut points supply the restore-time and safety observations. The final D4 report proposes values from those observations plus stated operational margin. It must show all samples, outliers, fault classes, environment limits, and sensitivity to rate limits rather than presenting only an average. The owner approves or rejects the proposed successful-ceremony target, retry count/time budget, and pre-precursor abort threshold. Approval of those values does not accept D5.

## Evidence and receipts

Each scenario receipt includes:

- scenario ID, hypothesis IDs, exact script/harness revision, artifact and configuration digests, reviewer source, and exact configured model where AI review is used;
- target and anchor repository node IDs/full names, proof/constituent identities, App integration/installation and selected-repository IDs, context/ruleset binding, Mergify configuration digest/epoch, bypass/Freeze/exclusion/Merge Protections observations;
- ordered API request/response metadata with secrets and tokens excluded, webhook/delivery IDs, timestamps, fault controls, state-log sequence/hash, fixture invocation/result/signature digests, publication `external_id`, check-run identity, and final merge/block outcome;
- raw timing, retry, resource, storage, API, and cost measurements;
- expected result, actual result, discrepancies, terminal classification, and cleanup status; and
- immutable evidence-object digests and a standalone reconstruction result.

The aggregate receipt maps every H1–H10 hypothesis and every scenario row to exact evidence. It lists inconclusive or failed rows explicitly; reruns never overwrite history. A separate independent reviewer reconstructs at least one allow, deny, spoof, moved-state, retry, uncertain-publication, invalidation, terminal, stop-only, audit-damage, and cleanup case without access to mutable service state.

## Cleanup proof

Cleanup is part of the rehearsal, not an optional housekeeping step. The terminal cleanup receipt proves:

- H9 evidence proves target removal while only the inert anchor remains, followed during cleanup by removal of the installation/credentials and both disposable repositories under owner/platform authority;
- installation, fixture-signing, audit-signing, deployment, state, and artifact credentials are revoked or destroyed as designed;
- ephemeral service, launcher, fault proxy, schedules, webhooks, environments, and writable state endpoints are absent;
- target ruleset, Mergify, Freeze, protections, exclusions, routes and checks are removed; the anchor is verified inert; target and anchor are then archived/deleted according to the approved retention choice;
- no production repository, production App installation, production secret, production workflow, or production control changed; and
- retained receipts remain readable, hash-verifiable, and incapable of publishing.

A post-cleanup negative probe must fail to authenticate, append, invoke signing, or publish. Any surviving executable credential, webhook, scheduler, mutable route, or check writer is a cleanup failure.

## Go/no-go criteria

D4 is a **go for owner decisions**, not implementation authorization, only when:

1. every H1–H10 hypothesis and every mandatory cut point has reproducible passing evidence;
2. same-name wrong-publisher checks cannot satisfy the App-qualified requirement;
3. stale or moved proof heads cannot merge after any protected identity changes;
4. `PUBLISHING_UNCERTAIN` never repeats create or permits replacement before durable inability-to-merge proof;
5. append-only uniqueness, replay, rollback, stop-only disable, classification ownership, and audit reconstruction all fail closed;
6. successful-path, retry, abort, resource, and cost measurements are complete enough to propose defensible values without hiding outliers;
7. cleanup proof is complete; and
8. independent adversarial review finds no unresolved substantive issue.

Any failure in publisher qualification, stale-success invalidation, exact-head/base/constituent binding, lost-response handling, replay closure, or cleanup is an architecture no-go. It cannot be waived by choosing a larger retry or outage budget. Ambiguous real GitHub or Mergify behavior is also no-go.

## D5 owner presentation

After a D4 go, the owner receives two separate decisions in plain language:

1. **D4 operational limits:** the proposed successful-ceremony target, retry count/time budget, and pre-precursor abort threshold, with measured evidence, margins, cost, and consequences of exhaustion.
2. **D5 residual risk:** after the real precursor merges, a terminal merits, identity, protocol, publication, dependency, or control-state failure has no recovery PR or alternate verifier. Freeze remains active and ordinary merging may remain unavailable for an unbounded period. D4 can demonstrate detection and safe failure but cannot bound that terminal tail.

The owner may approve D4 and reject D5. In that case no production ceremony begins. Acceptance must not be inferred from approval of this design, a successful rehearsal, hosting selection, or approval of the numeric budgets.

## Explicit non-goals

This rehearsal does not:

- implement, compare, or validate the final #1016 semantic verifier;
- read or rewrite the production corpus, rule IDs, policy predicates, or broad Python verifier files;
- perform configuration matching or preserve existing implementation-shape enforcement;
- install production credentials or mutate production GitHub, Mergify, rulesets, Freeze, Merge Protections, workflows, or CI;
- create a compatibility adapter, dual verifier, fallback scanner, alternate authority, recovery success route, or permanent rehearsal controller;
- establish hosting-provider suitability for production beyond the exact properties exercised; or
- authorize precursor, activation, queue, review request, production CI, or merge work.

The next artifact after approval of this design is a bounded implementation plan for the disposable rehearsal only. Production implementation remains blocked until D4 evidence is reviewed, the owner approves its proposed operational values, and D5 is separately accepted.
