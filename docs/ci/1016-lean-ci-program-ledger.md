# Lean CI Two-Board Program Ledger

Architecture owner: #1016

Approved plan: `docs/superpowers/plans/2026-07-15-lean-ci-binary-owned-readiness.md`

Task 0 status: **DONE_WITH_CONCERNS — governance recorded; implementation blocked on unassigned issue rows**

This ledger separates authority/deletion work from runtime-invariant preservation. A row is eligible for implementation only when its current issue body owns the exact row, its dependencies have landed on `main`, and the expected file set has one implementer. A broader or adjacent issue is not treated as ownership.

`Merged PR / main SHA` stays empty until the row is actually merged. Architecture issue #1016 does not complete or auto-complete any row on either board.

## Board A — Authority Retirement and Non-Authoritative Deletion

| Row | Predicate / module IDs | Current callers | Invariant and surviving owner | Mutation / identity proof | Owning issue | Expected files | Cost effect | Reviewer | Merged PR / main SHA |
|---|---|---|---|---|---|---|---|---|---|
| A5 — Task 5 operational replacement | `tag-deploy`, `same-sha-main-evidence`, prior-main artifact selection, mutable `/opt/bolt-v2/bolt-v2` install/start | Tag `push` in `.github/workflows/ci.yml`; `scripts/ci_provenance.py`; `scripts/test_ci_provenance.py`; `deploy/README.md`; `deploy/install.sh`; generated systemd unit | Only manifest-bound bytes at the content-addressed immutable path may reach exact-binary `ops launch`; surviving owners are the release-manifest verifier, installer/unit rendering, and Rust pre-arm boundary | Reject tag, prior-run, same-SHA, mutable-copy, wrong-path/digest/config-bundle, audit-receipt substitution, and restart inheritance; re-hash current executable | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A5`); #930/#880 are adjacent but do not own this exact retirement | `.github/workflows/ci.yml`, `ci/github-actions-runners.toml`, `deploy/install.sh`, systemd template/generated unit, `deploy/README.md`, bounded `ci_provenance` code/tests | Removes tag/reuse jobs and one mutable route; measure jobs, lines, runner-minutes, and operator latency in-slice | Bounded internal S1/L2/L3 review; native `sp-reviewer` (`U_kgDOEZMFhA`) | — |
| A6B — Task 6B mechanical queue | `merge_queue_operator`, `merge_queue_preflight`, required/all-check polling, verifier profiles, `MERGIFY_CONFIG_EXPECTATIONS`, source-fence execution | Public `just merge-queue`; operator invokes preflight; preflight reads `ci/rust-verification.toml`; CI hygiene/provenance suites mirror Mergify and queue policy | Queue admission retains exact PR/head/base identity, native approval and human-thread state, conflicts, and Mergify routing only; surviving owner is the mechanical operator/preflight | Hold PR/head/base/review/conflict inputs constant while green, failed, missing, skipped, cancelled, and unavailable advisory checks all yield the same queue decision | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A6B`) | `justfile`, queue operator/preflight and tests, `ci/rust-verification.toml`, bounded provenance mirrors/tests, `docs/ci/merge-queue-preflight-contract.md` | Removes check polling, verifier execution, and duplicate broad local work from queue admission; record elapsed preflight time before/after | Bounded internal M1/S2/X2 review; native `sp-reviewer` | — |
| A9.1 — Task 9 dynamic gate/readiness/provenance wave | CI policy detector/classifier, dynamic gate names/classes, merge-readiness watcher/finalizer, gate provenance, carry-forward and inherited-result machinery | `.github/workflows/ci.yml`; merge-readiness finalizer; `scripts/merge_readiness.py`; `scripts/ci_provenance.py`; CI hygiene/provenance test suites | Static `trading-binary` evidence remains visible; no result can authorize merge, install, launch, or trading; surviving owners are native human review and exact-binary live readiness | Negative search for dynamic result classes, carry-forward, inherited success, fallback scanning, and advisory authorization edges; workflow graph has no consumer that converts evidence into authority | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A9-1`) | `.github/workflows/ci.yml`, `.github/workflows/merge-readiness-finalizer.yml`, merge-readiness/provenance scripts and tests, coupled CI docs/config | Removes dynamic jobs, polling, policy code, and duplicate result publication; record lines, jobs, wall time, and runner-minutes | Bounded internal M1/B3/S1/S2/X2 review; native `sp-reviewer` | — |
| A9.2 — Task 9 nextest archive/cache wave | nextest archive, fingerprint, reuse, cache policy, partition aggregator, prior-result substitution | `.github/workflows/ci.yml`; nextest fingerprint/cache scripts and tests; CI provenance; `docs/ci/nextest-artifact-cache.md` | `trading-binary` always runs locked nextest and builds once; no archive, cache, fingerprint, or prior result is proof; surviving owner is the exact-file lane | Counter-mutations for archive restore, cache hit, prior-run lookup, fingerprint reuse, and skipped locked nextest must fail the static graph/evidence contract | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A9-2`) | `.github/workflows/ci.yml`, nextest fingerprint/cache/input scripts and tests, coupled provenance/config/docs | Removes four partition runs, archive/reuse jobs, storage traffic, and cache policy; record storage bytes and managed runner-minutes | Bounded internal B1/B3/S1/X2 review; native `sp-reviewer` | — |
| A9.3 — Task 9 CI self-governance and lane wave | giant workflow-hygiene verifier/tests, lane governor/governance, duplicated Rust-under-fence execution, non-runtime `run_fences` residue | `just ci-lint-workflow`; `just source-fence-static`; setup action; merge-queue verifier profiles; cheap-lane registry | Preserve only named runtime/trading invariants on Board B; delete implementation-shape, lane, provenance, broad token/literal, and CI-self-governance policy; surviving owners are focused Rust/static evidence and the simple public recipes that remain | Complete predicate disposition with no unknown row; discriminating Board-B mutations stay caught after each lexical predicate is removed; negative residue/import/caller search | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A9-3`) | `scripts/verify_ci_workflow_hygiene.py` and tests/helpers, lane governor/governance and tests, `scripts/run_fences.py` residue/tests, setup/config/docs | Removes the largest Python suites, repeated full-tree scans, interpreter launches, and duplicate Rust work; record lines and local/remote elapsed time | Bounded internal S2/S3/X2 review; native `sp-reviewer` | — |
| A9.4 — Task 9 obsolete workflow wave | coverage enforcer, dispatch-cancel, coding-plan-smoke, obsolete merge-readiness reporting without a named consumer | Corresponding workflow triggers, Python scripts/tests, CI hygiene registry, coverage/lane config | Named reporting may survive only with a documented consumer and no authority edge; otherwise the workflow/script/test/config closure is deleted | Caller/import/registry search reaches zero; removal leaves no missing required context or Mergify predicate after Task 7; retained reporting has an identified reader | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A9-4`) | `.github/workflows/coverage-enforcer.yml`, `dispatch-ci-cancel.yml`, `ai-review-coding-plan-smoke.yml`, obsolete readiness workflow, corresponding scripts/tests/config/docs | Removes scheduled/polling hosted-runner minutes and workflow noise; record event counts, job minutes, and dollars using the frozen baseline method | Bounded internal M1/S2/X2 review; native `sp-reviewer` | — |
| A9.5 — Task 9 advisory-lane demotion wave | Backtester dynamic gate, `backtester-gate`, host-health required context, compound actionlint suite, advisory aggregation | Backtester/actionlint/root workflows; live required-status list until Task 7; Mergify predicates until Task 7; runner registry | Backtester, host-health, bare actionlint, fmt/clippy, dependency, AI, coverage, flaky, storage, and cost evidence is advisory/manual/scheduled only, with no aggregator or authorization consumer | Static/live inspection after Task 7 shows zero required status consumers; failed/missing advisory outputs do not change mergeability; path/manual/schedule triggers match named consumers | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-A9-5`) | `.github/workflows/backtester-ci.yml`, `actionlint.yml`, host-health sections in `ci.yml`, runner/config registries, focused docs/tests | Removes PR-wide low-signal work and compound suites; record per-event jobs, critical path, and managed runner-minutes | Bounded internal M1/B3/S2/X2 review; native `sp-reviewer` | — |

## Board B — Runtime-Invariant Preservation

| Row | Predicate / module IDs | Current callers | Invariant and surviving owner | Mutation / identity proof | Owning issue | Expected files | Cost effect | Reviewer | Merged PR / main SHA |
|---|---|---|---|---|---|---|---|---|---|
| B8.1 — Task 8 strategy/shared-admission family | `verify_bolt_v3_strategy_policy_fence`, `verify_bolt_v3_no_exit_market_command`; strategy-to-shared-admission and kill-switch layering predicates | `just` fence recipes; `scripts/run_fences.py`; cheap-lane registry; source-fence jobs | Strategies emit intent only and every submit-capable path reaches shared NT-based admission; surviving owners are Rust APIs plus behavior/integration tests under `src/bolt_v3_live_node/tests/` | Plant a bypass that submits or constructs execution outside shared admission and prove retained Rust evidence fails before deleting each lexical predicate | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-B8-1`); #1318 explicitly leaves the successor separate | Two named verifier scripts/tests, affected live-node Rust tests/APIs, `run_fences`/registry entries coupled to their removal | Rust evidence may add small test cost; deletion removes full-tree lexical scans. Record both | Bounded internal strategy/admission review; native `sp-reviewer` | — |
| B8.2 — Task 8 poison/fail-closed family | `verify_bolt_v3_poison_lock_fence`; poisoned locks, shared halt state, and fail-closed error branches | `just` fence recipe; `scripts/run_fences.py`; cheap-lane registry; runtime state modules in live-node/IV/capture code | Poison or ambiguous shared state cannot re-arm submit; surviving owner is typed runtime state plus discriminating Rust tests | Mutate each poison/error arm toward success or unlocked state and prove a Rust behavior test fails with no submit side effect | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-B8-2`); #1179 does not own this fence migration | Poison verifier/test, relevant `src/bolt_v3_live_node/tests/`, `src/bolt_v3_iv/`, `src/nt_runtime_capture.rs`, coupled registry | Replace lexical scans with scoped Rust behavior evidence; record test and fence elapsed time | Bounded internal fail-closed review; native `sp-reviewer` | — |
| B8.3 — Task 8 SSM/config/redaction family | SSM-only resolution, unknown/malformed production config rejection, field-context-only redaction | `src/bolt_v3_secrets.rs`; production-profile tests; boundary registry/fence for the AWS SDK response feeder; exact-binary negatives planned in Tasks 1/2B | Product/runtime secrets come only from AWS SSM; missing credentials and invalid config fail at the named stage without raw SSM path or value; surviving owners are Rust secret/config code and exact-binary/Rust tests | Mutate in an environment fallback, accept unknown config, emit a raw path/value, or advance beyond `secrets-resolve`; retained evidence must fail | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-B8-3`); #991 owns only the narrower feeder-evidence deferral | `src/bolt_v3_secrets.rs`, `tests/bolt_v3_prod_profile.rs`, bounded boundary-registry/fence rows, exact-binary evidence, old predicate/tests identified by census | Preserve one exact-binary negative plus focused Rust tests; remove duplicate lexical/profile policy only after proof | Bounded internal secrets/redaction review; native `sp-reviewer` | — |
| B8.4 — Task 8 provider-boundary/reference-health family | `verify_bolt_v3_boundary_evidence`; boundary registry completeness, WebSocket-frame non-deferral, production decode/routing, reference-health degradation | `scripts/run_fences.py`; boundary self-tests/registry; deploy/readiness feeders; source-fence jobs | Every provider-dependent deploy/readiness feeder is registered; WebSocket frames stay non-deferrable; production decoding reaches runtime observation and degraded reference health blocks arming; surviving owner is focused real-boundary Rust/integration evidence plus minimal registry completeness proof | Replace binary with text frame, omit a feeder/registry row, corrupt production envelope/routing, or suppress degraded health; retained behavior/integration evidence must fail | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-B8-4`); #874/#991 predate and do not own this exact migration | Boundary verifier/test/registry, provider/reference-health Rust tests and fixtures, coupled `run_fences`/config entries | May retain a small registry check; remove broad provenance/shape scans and record delta | Bounded internal provider-boundary review; native `sp-reviewer` | — |
| B8.5 — Task 8 packaged-systemd family | `verify_install_unit_generated`; template/generated-unit equality, exact ExecStart, restart reruns readiness | install renderer; install/unit verifier; `tests/deploy_systemd.rs`; deploy docs | Generated systemd always invokes the single immutable exact binary's `ops launch`; every restart reruns readiness and cannot inherit a permit; surviving owner is renderer equality plus Rust/deploy integration evidence | Mutate template/rendered bytes, ExecStart path/subcommand, restart behavior, or permit inheritance and prove focused tests fail | **ISSUE ASSIGNMENT REQUIRED — IMPLEMENTATION BLOCKED** (`DRAFT-B8-5`); #909/#930 do not own permit/restart migration | `scripts/verify_install_unit_generated.py` and tests, renderer, unit template/generated unit, `tests/deploy_systemd.rs`, deploy docs | Keep cheap render equality; move behavioral ownership out of broad fences. Record elapsed delta | Bounded internal L2/L3/X2 review; native `sp-reviewer` | — |

## Issue Drafts for Blocked Rows

These are exact proposed issue texts. They are not live issues and assign no authority until an operator creates them and records the number in the applicable row.

### DRAFT-A5

**Title:** Retire legacy deploy authority and activate one immutable exact-artifact launch path

**Body:**

> ## Problem
> Tag-triggered deployment, `same-sha-main-evidence`, prior-main artifact lookup, and the mutable manual-copy/systemd target are alternate operational authority paths. None binds one selected manifest, exact bytes, config bundle, and in-process readiness boundary.
>
> ## Scope
> From fresh `main` after the immutable installer and Rust permit are proved under legacy gates, atomically activate the manifest-bound content-addressed install/systemd path and delete or hard-disable tag deploy, same-SHA/prior-run selection, and manual copy without manifest binding. Remove exact provenance callers/tests/docs in the same slice.
>
> ## Evidence
> Reject tag, prior-run, same-SHA, mutable-copy, wrong-path, wrong-digest, wrong-config-bundle, audit-receipt substitution, and restart inheritance. Prove current-executable re-hash and fresh-permit restart behavior. Drill failed cutover with deploy/trading paused and recover only by forward fix. Obtain bounded S1/L2/L3 review.
>
> ## Dependencies and non-goals
> Requires Tasks 3 and 4 under legacy merge gates. Do not add a signer, publisher, cache-as-proof, alternate installer, fallback, deploy, launch, or trade during rehearsal. Related architecture: issue 1016; adjacent deploy tracking: issues 930 and 880.

### DRAFT-A6B

**Title:** Reduce merge-queue admission to mechanical identity and native-review checks

**Body:**

> ## Problem
> `just merge-queue` currently polls required and all checks, aggregates CI verdicts, reads mirrored Mergify expectations, selects verifier profiles, and may run source fences. That duplicates merge policy and lets advisory CI veto queue admission.
>
> ## Scope
> Delete check polling, CI verdict aggregation, required-check workflow maps, queue-CI Mergify mirrors, source-fence/verifier execution, verifier profiles, and CI-dependent queue tests. Retain exact PR/head/base identity, required native approval and human-thread state, merge-conflict state, and Mergify routing. Update the public recipe, operator/preflight, focused tests, TOML, provenance mirrors, and queue contract together.
>
> ## Evidence
> Through public `just merge-queue`, hold all mechanical inputs constant and prove green, failed, missing, skipped, cancelled, and unavailable advisory checks produce the same decision. Prove wrong head/base, missing native approval, unresolved human thread, conflict, or wrong routing still blocks. Run focused Python/static checks and bounded internal M1/S2/X2 review.
>
> ## Dependencies and non-goals
> Start from fresh `main` after Task 5. Legacy GitHub statuses and Mergify predicates remain active until the governed Task 7 cutover. Do not edit live rules, Mergify state, deploy, launch, or trading. Related architecture: issue 1016.

### DRAFT-A9-1

**Title:** Delete dynamic CI gate, merge-readiness, and result-provenance authority residue

**Body:**

> ## Problem
> After zero-status cutover, dynamic CI policy classes, gate names, merge-readiness polling/finalization, carry-forward, and inherited-result provenance have no legitimate authority consumer and preserve a second verdict system.
>
> ## Scope
> In one bounded caller-closed slice, delete the dynamic detector/classifier and gate envelope, dynamic result classes, merge-readiness watcher/finalizer, gate provenance, carry-forward, inherited-success, and fallback-result machinery. Remove coupled tests, registries, and docs. Preserve the static informational `trading-binary` evidence and native human merge controls.
>
> ## Evidence
> Prove exact callers/imports are zero or removed, no advisory result reaches merge/install/launch/trading decisions, and negative searches find no dynamic result alternative, carry-forward, inherited success, or fallback scan. Record lines, jobs, wall time, and runner-minutes removed; obtain bounded M1/B3/S1/S2/X2 review.
>
> ## Dependencies and non-goals
> Requires Tasks 5 and 7 and any relevant Task 8 migrations. Do not recreate an aggregator, trusted publisher, compatibility adapter, or fallback. Related architecture: issue 1016.

### DRAFT-A9-2

**Title:** Delete nextest archive, fingerprint, cache-reuse, and partition-aggregation proof paths

**Body:**

> ## Problem
> The old CI graph treats archives, fingerprints, cache state, partitions, and prior results as substitutes for running the exact current trading binary's locked test/build path.
>
> ## Scope
> Delete nextest archive, fingerprint, reuse, cache-proof, prior-result, and separate partition-aggregator jobs plus their exact scripts, tests, registries, and docs. Keep ordinary build caches only where they are performance aids and cannot satisfy evidence or authority.
>
> ## Evidence
> Static graph proof shows every `trading-binary` invocation runs locked nextest and one locked ARM64 release build. Counter-mutations that restore archive/prior-result/cache substitution or skip locked nextest must fail. Record storage bytes, jobs, wall time, and runner-minutes removed; obtain bounded B1/B3/S1/X2 review.
>
> ## Dependencies and non-goals
> Requires Task 1 exact-binary evidence and Task 7. No cache, archive, fingerprint, or prior run may become proof. Related architecture: issue 1016.

### DRAFT-A9-3

**Title:** Delete CI self-governance, lane-policy, and duplicated source-fence machinery

**Body:**

> ## Problem
> Giant workflow-hygiene tests, lane-governance policy, repeated Python tree scans, and duplicated Rust execution under source fences govern CI's implementation shape instead of trading/runtime behavior.
>
> ## Scope
> Use the completed predicate ledger to delete CI-shape, lane, provenance, broad token/literal, and non-runtime fence predicates; their tests/helpers; duplicated Rust execution; and non-runtime `run_fences` residue. Retain only focused public recipes and issue-owned Board-B evidence whose consumer remains named.
>
> ## Evidence
> No predicate disposition is unknown. Each genuine runtime invariant has a discriminating Rust/integration mutation before its old lexical predicate is removed. Exact import/caller/registry residue reaches zero for deleted modules. Record Python lines, interpreter launches, local elapsed time, and remote job time removed; obtain bounded S2/S3/X2 review.
>
> ## Dependencies and non-goals
> Requires Task 7 and every relevant Task 8 family. Do not translate implementation-shape rules into new Rust tests or another policy engine. Related architecture: issue 1016.

### DRAFT-A9-4

**Title:** Delete obsolete CI reporting workflows with no named consumer

**Body:**

> ## Problem
> Coverage polling/enforcement, dispatch cancellation, coding-plan smoke, and obsolete merge-readiness workflows consume runner time or publish noise after their authority consumers are removed.
>
> ## Scope
> For each named workflow, prove whether a current human or machine consumer exists. Delete the complete workflow/script/test/config/doc closure when none exists. Retain only simple reporting with a documented reader and no merge/install/launch/trading authorization edge.
>
> ## Evidence
> Caller/import/registry searches reach zero for deleted components; removal creates no missing required context or Mergify predicate after Task 7. Record trigger counts, job minutes, polling duration, and estimated cost removed; obtain bounded M1/S2/X2 review.
>
> ## Dependencies and non-goals
> Requires Task 7 and the relevant authority-deletion waves. Do not replace a deleted workflow with a new aggregator or publisher. Related architecture: issue 1016.

### DRAFT-A9-5

**Title:** Demote retained CI lanes to explicit advisory, manual, or scheduled evidence

**Body:**

> ## Problem
> Backtester, host-health, actionlint, formatting/clippy, dependency, AI, coverage, flaky, storage, and cost lanes currently include PR-wide, compound, dynamic, or gate-shaped behavior that is unnecessary once CI has no merge authority.
>
> ## Scope
> Make every retained lane advisory, path-scoped, manual, or scheduled according to its named consumer. Remove `backtester-gate`, host-health required-context production, unrelated suites from bare actionlint, dynamic gate aggregation, and any advisory-result consumer. Do not add a replacement aggregator.
>
> ## Evidence
> Live and repository inspection after Task 7 shows zero required CI statuses and no Mergify/operator authorization edge. Failed or missing advisory outputs leave mergeability unchanged, while native approval/thread controls still bind. Record per-event jobs, critical path, and runner-minutes before/after; obtain bounded M1/B3/S2/X2 review.
>
> ## Dependencies and non-goals
> Requires Task 7; relevant Task 8 evidence must already survive. Human review stays mandatory. Related architecture: issue 1016.

### DRAFT-B8-3

**Title:** Migrate SSM-only config and redaction invariants to exact-binary and Rust evidence

**Body:**

> ## Problem
> SSM-only resolution, strict production config rejection, and secret redaction must survive deletion of broad Python/source-shape governance. Existing issue 991 covers only the narrower AWS SDK response feeder deferral.
>
> ## Scope
> Inventory every old predicate for SSM-only resolution, malformed/unknown production config, field-context-only errors, and raw SSM path/value suppression. Add or identify discriminating exact-binary/Rust evidence for each genuine behavior, then remove the old lexical predicate in a caller-closed slice.
>
> ## Evidence
> Mutations that add an environment/alternate-secret fallback, accept an unknown field, expose a raw parameter path/value, or advance beyond the expected `secrets-resolve` failure stage must fail. Valid generated production config with AWS credential sources and IMDS disabled must fail closed without entering Start. Obtain bounded secrets/redaction and X2 review.
>
> ## Dependencies and non-goals
> Requires Tasks 1 and 2B; coordinate the feeder row with issue 991. No alternate secret backend, raw credential output, compatibility path, or duplicated config parser. Related architecture: issue 1016.

### DRAFT-B8-1

**Title:** Migrate strategy and shared-admission invariants from lexical fences to Rust behavior evidence

**Body:**

> ## Problem
> Strategy-intent-only routing, no strategy-local submit mechanics, shared admission, and kill-switch layering currently depend in part on `verify_bolt_v3_strategy_policy_fence` and `verify_bolt_v3_no_exit_market_command`. Existing issue 1318 explicitly leaves the successor as separate work.
>
> ## Scope
> Inventory every predicate in both fences. For each genuine runtime invariant, add or identify a discriminating Rust behavior/integration test owned by the shared admission API, then remove the old lexical predicate and its caller/test/registry closure. Delete implementation-shape checks instead of translating them.
>
> ## Evidence
> Mutations that submit or construct venue execution outside shared admission, bypass kill-switch state, or place submit mechanics under strategy modules must fail retained Rust evidence. Run focused Rust proof remotely under the applicable governance plus targeted static checks and bounded strategy/admission X2 review.
>
> ## Dependencies and non-goals
> Requires the complete Task 8 predicate census. No new strategy authority, duplicate admission path, or broad token fence. Related architecture: issue 1016; adjacent invariant inventory: issue 1318.

### DRAFT-B8-2

**Title:** Migrate poison and fail-closed shared-state invariants to Rust behavior evidence

**Body:**

> ## Problem
> `verify_bolt_v3_poison_lock_fence` lexically guards poisoned-lock and error-direction behavior across live-node, IV, and runtime-capture state, but no current issue owns its full semantic migration.
>
> ## Scope
> Classify every poison-fence predicate. Preserve genuine behavior in typed runtime-state APIs and discriminating Rust tests, then remove each replaced lexical predicate with its test, `run_fences`, recipe, and registry callers. Delete pure implementation-shape rules.
>
> ## Evidence
> Mutate each poisoned/error/ambiguous state toward success, unlocked, or re-armed and prove retained Rust tests fail with no submit side effect. Cover cross-module shared state rather than file text. Obtain bounded fail-closed and X2 review.
>
> ## Dependencies and non-goals
> Coordinate with live incident behavior without treating issue 1179 as ownership. No tolerance band, fail-open recovery, duplicate halt state, or fallback path. Related architecture: issue 1016.

### DRAFT-B8-4

**Title:** Migrate provider-boundary and reference-health invariants without retaining CI authority

**Body:**

> ## Problem
> Boundary registry completeness, non-deferrable WebSocket evidence, production frame/envelope/routing behavior, and reference-health degradation must survive deletion of broad `verify_bolt_v3_boundary_evidence` policy. Existing issues 874 and 991 predate the zero-status architecture and do not own this complete migration.
>
> ## Scope
> Partition the boundary verifier by semantic predicate. Preserve genuine provider/runtime behavior in Rust/integration evidence and retain only the smallest registry-completeness proof needed for external facts and expiring non-WebSocket deferrals. Remove provenance, lane, and implementation-shape residue with exact callers/tests.
>
> ## Evidence
> Counter-mutations for binary-versus-text WebSocket frames, omitted registry feeders, malformed production envelope/routing, expired deferral, and suppressed degraded reference health must fail. WebSocket-frame evidence cannot be deferred. Obtain bounded provider-boundary and X2 review.
>
> ## Dependencies and non-goals
> Coordinate existing feeder work recorded in issues 874 and 991, but do not convert advisory CI into merge authority or add a fallback fixture/proof path. Related architecture: issue 1016.

### DRAFT-B8-5

**Title:** Move packaged-systemd readiness ownership to exact render and restart behavior evidence

**Body:**

> ## Problem
> Template equality alone does not prove that packaged systemd invokes the exact immutable executable's `ops launch`, reruns the full readiness phase on every restart, and cannot inherit a prior permit. Existing deploy follow-ups do not own that complete post-architecture migration.
>
> ## Scope
> Preserve template/generated-unit byte equality, move launch/restart behavior into focused deploy/Rust integration evidence, and remove obsolete `verify_install_unit_generated` shape predicates and callers only after replacement proof. Cover the renderer, template, generated unit, deploy test, and operator docs together.
>
> ## Evidence
> Mutate the executable path, `ops launch` subcommand, rendered bytes, restart phase invocation, or permit freshness and prove focused tests fail. Every restart must create a fresh in-process permit; audit receipts and prior state cannot substitute. Obtain bounded L2/L3/X2 review.
>
> ## Dependencies and non-goals
> Requires Tasks 3 and 4 and coordinates adjacent deploy tracking in issues 909 and 930. No mutable target, alternate unit/installer, persisted permit, or compatibility adapter. Related architecture: issue 1016.
